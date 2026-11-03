//! MCP server paths not reached by tests/mcp_offline.rs: policy-only asset
//! classes, custom-period start ages, stored stale/incomplete end points,
//! live-provider annotations and unreadable policy narratives. Offline:
//! example_data.json with fixture quotes, temp dirs only.

use portfolio_rs::mcp::{load, McpOptions, PortfolioServer};
use portfolio_rs::services::quotes::{AnyProvider, FixtureProvider};
use rmcp::model::{CallToolRequestParams, CallToolResult, ReadResourceRequestParams};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

type Client = RunningService<RoleClient, ()>;

const POLICY: &str = r#"
version = "1.0"
name = "Cov Policy"
base_currency = "EUR"
time_horizon_years = 10
risk_profile = "moderate"

[[allocations]]
asset_class = "Stocks"
target_percent = 60.0

[[allocations]]
asset_class = "RealEstate"
target_percent = 40.0
"#;

fn example_data() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json").to_string()
}

fn fixture() -> AnyProvider {
    let json = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/quotes_example.json"
    ))
    .unwrap();
    AnyProvider::Fixture(Box::new(FixtureProvider::from_json(&json).unwrap()))
}

fn opts() -> McpOptions {
    McpOptions {
        file: Some(example_data()),
        currency: "EUR".into(),
        quotes: Some(fixture()),
        stale_after_days: Some(100_000),
        ..Default::default()
    }
}

async fn connect_with(opts: McpOptions, network: bool) -> Client {
    let mut served = load(&opts).await.expect("portfolio loads");
    served.network = network;
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let running = PortfolioServer::new(served).serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn connect(opts: McpOptions) -> Client {
    connect_with(opts, false).await
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    let params = CallToolRequestParams::new(name.to_string())
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    client.call_tool(params).await.expect("call_tool transport")
}

async fn ok(client: &Client, name: &str, args: Value) -> Value {
    let result = call(client, name, args).await;
    assert_ne!(result.is_error, Some(true), "{name}: {:?}", result.content);
    result.structured_content.expect("structured content")
}

async fn err(client: &Client, name: &str, args: Value) -> Value {
    let result = call(client, name, args).await;
    assert_eq!(result.is_error, Some(true), "{name} unexpectedly succeeded");
    let text = result.content[0].as_text().unwrap().text.clone();
    serde_json::from_str(&text).unwrap()
}

fn day(offset: i64) -> String {
    (chrono::Local::now().date_naive() + chrono::Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

fn warning<'a>(v: &'a Value, code: &str) -> Option<&'a Value> {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == code)
}

/// A class only the policy names is a valid filter (no rows); the error
/// for an unknown class lists the policy's classes too.
#[tokio::test]
async fn asset_class_filter_knows_policy_classes() {
    let tmp = TempDir::new().unwrap();
    let policy = tmp.path().join("policy.toml");
    std::fs::write(&policy, POLICY).unwrap();
    let client = connect(McpOptions {
        policy: Some(policy.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_positions",
        json!({"assetClass": "realestate"}),
    )
    .await;
    assert_eq!(v["positions"], json!([]), "{v}");
    let e = err(&client, "get_positions", json!({"assetClass": "Art"})).await;
    assert_eq!(e["error"]["code"], "invalid_argument");
    let msg = e["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("RealEstate") && msg.contains("Stocks"),
        "{msg}"
    );
    client.cancel().await.unwrap();
}

/// Without `maxStartAgeDays`, a custom period's start may be 5 days old for
/// windows of 8 to 31 days and 10 days beyond that.
#[tokio::test]
async fn custom_period_start_age_follows_window_length() {
    let tmp = TempDir::new().unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    for (from, max_age) in [(-19, 5), (-60, 10)] {
        let v = ok(
            &client,
            "get_performance",
            json!({"period": "custom", "from": day(from), "to": day(-10)}),
        )
        .await;
        assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
        let w = warning(&v, "insufficient_history").expect("insufficient_history");
        let msg = w["message"].as_str().unwrap();
        assert!(msg.contains(&format!("at most {max_age} day(s)")), "{msg}");
    }
    client.cancel().await.unwrap();
}

/// Record today's snapshot of example_data and return its path and value.
async fn seed_today(dir: &Path) -> (PathBuf, Value) {
    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        data_dir: Some(dir.to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    (path, serde_json::from_str(text.trim()).unwrap())
}

/// A past end point stored as stale gets `end_point_stale`; an incomplete
/// snapshot inside the window makes the TWR unavailable.
#[tokio::test]
async fn historical_end_point_stale_and_incomplete_middle() {
    let tmp = TempDir::new().unwrap();
    let (path, snap) = seed_today(tmp.path()).await;
    let at = |offset: i64, status: &str, complete: bool| {
        let mut s = snap.clone();
        s["date"] = json!(day(offset));
        s["dataStatus"] = json!(status);
        s["complete"] = json!(complete);
        s
    };
    let mut end = at(-2, "stale", true);
    let id = end["positions"][0]["positionId"]
        .as_str()
        .unwrap()
        .to_string();
    end["positions"][0]["stale"] = json!(true);
    let lines = [at(-5, "complete", true), at(-3, "partial", false), end];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, body).unwrap();

    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "custom", "from": day(-4), "to": day(-2)}),
    )
    .await;
    assert_eq!(v["dataStatus"], "stale", "{v}");
    let w = warning(&v, "end_point_stale").expect("end_point_stale");
    assert_eq!(
        w["detail"],
        json!({"point": "end", "date": day(-2), "positions": [id]})
    );
    assert!(v["stalePositions"].as_array().unwrap().contains(&json!(id)));
    assert!(warning(&v, "performance_twr_unavailable").is_some(), "{v}");
    assert!(v["twrPercent"].is_null(), "{v}");
    assert_eq!(v["method"], "modified_dietz", "{v}");
    client.cancel().await.unwrap();
}

/// With a live quote provider the server leaves the tools' own
/// open-world hints alone (no network is used: quotes are fixtures).
#[tokio::test]
async fn live_provider_keeps_open_world_hints() {
    let client = connect_with(opts(), true).await;
    let tools = client.list_all_tools().await.unwrap();
    let hint = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap()
            .annotations
            .as_ref()
            .unwrap()
            .open_world_hint
    };
    assert_eq!(hint("get_positions"), Some(true));
    assert_eq!(hint("list_portfolios"), Some(false));
    client.cancel().await.unwrap();
}

/// An INVESTMENT_POLICY.md that is not UTF-8 is an internal error, not an
/// empty narrative.
#[tokio::test]
async fn unreadable_policy_narrative_is_an_internal_error() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::copy(example_data(), ws.join("positions.json")).unwrap();
    std::fs::write(ws.join("INVESTMENT_POLICY.md"), [0xff, 0xfe, 0x00, 0xc3]).unwrap();
    let client = connect(McpOptions {
        file: None,
        workspace: Some(ws.to_string_lossy().into_owned()),
        data_dir: Some(tmp.path().join("data")),
        ..opts()
    })
    .await;
    let e = client
        .read_resource(ReadResourceRequestParams::new("portfolio://main/policy.md"))
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("could not read INVESTMENT_POLICY.md"),
        "{e}"
    );
    client.cancel().await.unwrap();
}

/// context.md for an unparsable positions file reports the data error.
#[tokio::test]
async fn context_resource_reports_invalid_data() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    std::fs::write(&file, "[{\"Name\": ").unwrap();
    let client = connect(McpOptions {
        file: Some(file.to_string_lossy().into_owned()),
        data_dir: Some(tmp.path().join("data")),
        ..opts()
    })
    .await;
    let e = client
        .read_resource(ReadResourceRequestParams::new(
            "portfolio://main/context.md",
        ))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("positions.json"), "{e}");
    let e = err(&client, "get_context", json!({})).await;
    assert_eq!(e["error"]["code"], "invalid_data");
    client.cancel().await.unwrap();
}

/// A negative purchase is a sell; entries on the same day keep their order.
#[tokio::test]
async fn sells_are_sell_transactions_in_entry_order() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    std::fs::write(
        &file,
        r#"[{"Name": "S&P", "Ticker": "SPY", "AssetClass": "Stocks", "Amount": 7,
             "Purchases": [{"Date": "2025-03-01", "Quantity": 10, "Price": 400},
                           {"Date": "2025-03-01", "Quantity": -3, "Price": 410}]},
           {"Name": "Cash", "AssetClass": "Cash", "Amount": 70,
             "Purchases": [{"Date": "2025-02-01", "Quantity": 100},
                           {"Date": "2025-02-02", "Quantity": -30}]}]"#,
    )
    .unwrap();
    let client = connect(McpOptions {
        file: Some(file.to_string_lossy().into_owned()),
        data_dir: Some(tmp.path().join("data")),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_transactions", json!({})).await;
    let txs: Vec<_> = v["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["type"].clone(),
                t["quantity"].clone(),
                t["amount"].clone(),
            )
        })
        .collect();
    assert_eq!(
        txs,
        vec![
            (json!("deposit"), json!(100.0), json!(100.0)),
            (json!("withdrawal"), json!(30.0), json!(30.0)),
            (json!("buy"), json!(10.0), json!(4000.0)),
            (json!("sell"), json!(3.0), json!(1230.0)),
        ],
        "{v}"
    );
    client.cancel().await.unwrap();
}

/// A workspace whose policy.toml does not parse still serves the portfolio,
/// with the policy reported as invalid.
#[tokio::test]
async fn workspace_with_a_broken_policy_is_served() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("portfolio")).unwrap();
    std::fs::copy(example_data(), ws.join("positions.json")).unwrap();
    std::fs::write(ws.join("portfolio/policy.toml"), "not = [valid").unwrap();
    let served = load(&McpOptions {
        file: None,
        workspace: Some(ws.to_string_lossy().into_owned()),
        data_dir: Some(tmp.path().join("data")),
        ..opts()
    })
    .await
    .unwrap();
    assert!(served.policy_error.is_some());
    let client = connect(McpOptions {
        file: None,
        workspace: Some(ws.to_string_lossy().into_owned()),
        data_dir: Some(tmp.path().join("data")),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    let codes: Vec<_> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].clone())
        .collect();
    assert!(codes.contains(&json!("invalid_policy")), "{v}");
    client.cancel().await.unwrap();
}

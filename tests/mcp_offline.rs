//! MCP round trips against example_data.json with fixture quotes.
//!
//! In-process tests run the real rmcp server and client over a duplex pipe;
//! the stdio tests spawn the binary with an isolated HOME. No network.

use portfolio_rs::mcp::{load, CacheMode, McpOptions, PortfolioServer};
use portfolio_rs::services::quotes::{AnyProvider, FixtureProvider};
use rmcp::model::{CallToolRequestParams, CallToolResult, ReadResourceRequestParams};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

type Client = RunningService<RoleClient, ()>;

fn example_data() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json").to_string()
}

fn fixture_path() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/quotes_example.json"
    )
    .to_string()
}

fn fixture() -> AnyProvider {
    fixture_named("quotes_example.json")
}

fn fixture_named(name: &str) -> AnyProvider {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let json = std::fs::read_to_string(path).unwrap();
    AnyProvider::Fixture(Box::new(FixtureProvider::from_json(&json).unwrap()))
}

const POLICY: &str = r#"
version = "1.0"
name = "MCP Test Policy"
base_currency = "EUR"
time_horizon_years = 10
risk_profile = "moderate"

[constraints]
minimum_cash_amount = 1000.0
minimum_cash_months = 6
single_position_limit_percent = 15.0
asset_class_limit_percent = 50.0

[[allocations]]
asset_class = "stocks"
target_percent = 40.0
tolerance_percent = 10.0

[[allocations]]
asset_class = "Crypto"
target_percent = 20.0
tolerance_percent = 10.0

[[allocations]]
asset_class = "Cash"
target_percent = 20.0
tolerance_percent = 10.0

[[allocations]]
asset_class = "RealEstate"
target_percent = 20.0
"#;

fn write_policy(dir: &Path) -> String {
    let path = dir.join("policy.toml");
    std::fs::write(&path, POLICY).unwrap();
    path.to_string_lossy().into_owned()
}

/// A workspace with positions.json, portfolio/policy.toml and INVESTMENT_POLICY.md.
fn workspace() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("demo-ws");
    std::fs::create_dir_all(dir.join("portfolio")).unwrap();
    std::fs::copy(example_data(), dir.join("positions.json")).unwrap();
    std::fs::write(dir.join("portfolio/policy.toml"), POLICY).unwrap();
    std::fs::write(
        dir.join("INVESTMENT_POLICY.md"),
        "# Fake policy\n\nStay the course.\n",
    )
    .unwrap();
    (tmp, dir)
}

fn opts() -> McpOptions {
    McpOptions {
        file: Some(example_data()),
        currency: "EUR".into(),
        quotes: Some(fixture()),
        // Fixture asOf dates are fixed (2026-01-02); stale tests set their own.
        stale_after_days: Some(100_000),
        ..Default::default()
    }
}

async fn connect(opts: McpOptions) -> Client {
    let served = load(&opts).await.expect("portfolio loads");
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let running = PortfolioServer::new(served).serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    let params = CallToolRequestParams::new(name.to_string())
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    client.call_tool(params).await.expect("call_tool transport")
}

/// Structured content of a successful call, validated against the tool's outputSchema.
async fn ok(client: &Client, name: &str, args: Value) -> Value {
    let result = call(client, name, args).await;
    assert_ne!(
        result.is_error,
        Some(true),
        "{name} failed: {:?}",
        result.content
    );
    let value = result
        .structured_content
        .clone()
        .expect("structured content");
    let tools = client.list_all_tools().await.unwrap();
    let tool = tools.iter().find(|t| t.name == name).unwrap();
    let schema = Value::Object((*tool.output_schema.clone().expect("output schema")).clone());
    let validator = jsonschema::validator_for(&schema).expect("valid schema");
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "{name} output violates its schema: {errors:?}"
    );
    value
}

/// The error code of a failed call (`isError: true`).
async fn err(client: &Client, name: &str, args: Value) -> String {
    let result = call(client, name, args).await;
    assert_eq!(result.is_error, Some(true), "{name} unexpectedly succeeded");
    let text = serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => v["error"]["code"].as_str().unwrap_or_default().to_string(),
        Err(_) => text,
    }
}

async fn read(client: &Client, uri: &str) -> Result<(String, String), rmcp::ServiceError> {
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    let content = serde_json::to_value(&result.contents[0]).unwrap();
    assert_eq!(content["uri"], uri);
    Ok((
        content["mimeType"].as_str().unwrap_or_default().to_string(),
        content["text"].as_str().unwrap().to_string(),
    ))
}

fn by_ticker<'a>(positions: &'a Value, ticker: &str) -> &'a Value {
    positions
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ticker"] == ticker)
        .unwrap_or_else(|| panic!("no {ticker}"))
}

fn codes(findings: &Value) -> Vec<String> {
    findings
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap().to_string())
        .collect()
}

fn assert_envelope(v: &Value, id: &str) {
    assert_eq!(v["schemaVersion"], 1);
    assert_eq!(v["portfolioId"], id);
    assert_eq!(v["baseCurrency"], "EUR");
    let as_of = v["asOf"].as_str().unwrap();
    chrono::DateTime::parse_from_rfc3339(as_of).expect("asOf is RFC 3339");
    assert!(as_of.contains('+') || as_of.ends_with('Z') || as_of[19..].contains('-'));
    assert!(v["warnings"].is_array());
}

#[tokio::test]
async fn tools_list_has_the_seven_read_only_tools() {
    let client = connect(opts()).await;
    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "check_policy",
            "get_allocation",
            "get_context",
            "get_performance",
            "get_positions",
            "get_transactions",
            "list_portfolios"
        ]
    );
    for tool in &tools {
        let ann = tool.annotations.as_ref().expect("annotations");
        assert_eq!(ann.read_only_hint, Some(true), "{}", tool.name);
        // Fixture quotes: nothing reaches the network.
        assert_eq!(ann.open_world_hint, Some(false), "{}", tool.name);
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false)),
            "{}",
            tool.name
        );
        assert!(tool.output_schema.is_some(), "{}", tool.name);
    }
    let perf = tools
        .iter()
        .find(|tool| tool.name == "get_performance")
        .unwrap();
    let required = perf.input_schema.get("required");
    assert!(
        required.is_none_or(|req| {
            req.as_array()
                .is_none_or(|items| !items.iter().any(|item| item.as_str() == Some("period")))
        }),
        "period defaults and must not be required: {required:?}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_positions_matches_the_fixture_quotes() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_envelope(&v, "main");
    assert_eq!(v["totalValue"], 97208.0);
    assert_eq!(v["cashValue"], 50000.0);
    assert_eq!(v["networkStatus"], "fixture");
    let positions = &v["positions"];
    assert_eq!(positions.as_array().unwrap().len(), 6);
    // Sorted by value, descending.
    assert_eq!(positions[0]["kind"], "cash");
    assert_eq!(positions[0]["priceStatus"], "cash");
    assert_eq!(positions[1]["ticker"], "BTC-USD");
    for p in positions.as_array().unwrap() {
        assert!(p["id"].as_str().unwrap().starts_with("pos_"));
        assert!(p["isin"].is_null());
    }
    let spy = by_ticker(positions, "SPY");
    assert_eq!(spy["kind"], "market");
    assert_eq!(spy["priceStatus"], "live");
    assert_eq!(spy["quantity"], 50.0);
    assert_eq!(spy["price"], 500.0);
    assert_eq!(spy["priceCurrency"], "USD");
    assert_eq!(spy["fxRate"], 0.8);
    assert_eq!(spy["valueBase"], 20000.0);
    assert_eq!(spy["weightPercent"], 20.57);
    assert_eq!(spy["averageCost"], 400.0);
    assert_eq!(spy["invested"], 16000.0);
    assert_eq!(spy["unrealizedPnl"], 4000.0);
    assert_eq!(spy["unrealizedPnlPercent"], 25.0);
    assert_eq!(spy["dayChangePercent"], 1.01);
    assert_eq!(spy["purchasesCount"], 2);
    // Every position has a live price, so no data-quality warnings about prices.
    assert!(!codes(&v["warnings"]).contains(&"no_live_price".to_string()));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_positions_filters_and_sorts() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_positions", json!({ "assetClass": "STOCKS" })).await;
    assert_eq!(v["positions"].as_array().unwrap().len(), 1);
    // Totals stay portfolio-wide.
    assert_eq!(v["totalValue"], 97208.0);

    let v = ok(&client, "get_positions", json!({ "sortBy": "name" })).await;
    let names: Vec<_> = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);

    let v = ok(
        &client,
        "get_positions",
        json!({ "sortBy": "pnl", "includeManual": false, "refresh": true }),
    )
    .await;
    let first = &v["positions"][0];
    assert!(first["unrealizedPnl"].as_f64().unwrap() > 0.0);

    let v = ok(&client, "get_positions", json!({ "sortBy": "dayChange" })).await;
    assert!(v["positions"][0]["dayChangePercent"].as_f64().is_some());
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn offline_quotes_are_null_not_zero() {
    let client = connect(McpOptions {
        quotes: Some(AnyProvider::Offline),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["totalValue"], 50000.0);
    assert_eq!(v["networkStatus"], "offline");
    assert_eq!(v["complete"], false);
    assert_eq!(
        v["dataQuality"]["missingPrices"].as_array().unwrap().len(),
        5
    );
    let spy = by_ticker(&v["positions"], "SPY");
    assert_eq!(spy["priceStatus"], "unavailable");
    assert!(spy["valueBase"].is_null());
    assert!(spy["price"].is_null());
    assert!(spy["weightPercent"].is_null());
    assert!(spy["unrealizedPnl"].is_null());
    let warnings = codes(&v["warnings"]);
    assert_eq!(warnings.iter().filter(|c| *c == "no_live_price").count(), 5);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_allocation_without_policy_warns_no_policy() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_envelope(&v, "main");
    assert!(v["policy"].is_null());
    assert!(codes(&v["warnings"]).contains(&"no_policy".to_string()));
    let classes = v["classes"].as_array().unwrap();
    assert_eq!(classes.len(), 6);
    assert!(classes.iter().all(|c| c["status"] == "untargeted"));
    let sum: f64 = classes
        .iter()
        .map(|c| c["actualPercent"].as_f64().unwrap())
        .sum();
    assert!((sum - 100.0).abs() < 0.05, "{sum}");

    let v = ok(&client, "get_allocation", json!({ "includeCash": false })).await;
    assert_eq!(v["totalValue"], 47208.0);
    assert!(v["classes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["assetClass"] != "Cash"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_allocation_against_policy_bands() {
    let tmp = TempDir::new().unwrap();
    let client = connect(McpOptions {
        policy: Some(write_policy(tmp.path())),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(v["policy"]["name"], "MCP Test Policy");
    assert_eq!(v["policy"]["source"], "portfolio://main/policy");
    let class = |name: &str| {
        v["classes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["assetClass"] == name)
            .unwrap()
            .clone()
    };
    // Target "stocks" matches holdings "Stocks" case-insensitively.
    let stocks = class("Stocks");
    assert_eq!(stocks["targetPercent"], 40.0);
    assert_eq!(stocks["bandLow"], 30.0);
    assert_eq!(stocks["bandHigh"], 50.0);
    assert_eq!(stocks["driftPoints"], -19.43);
    assert_eq!(stocks["status"], "below_band");
    assert_eq!(stocks["amountToTarget"], 18883.2);
    assert_eq!(class("Crypto")["status"], "within_band");
    assert_eq!(class("Cash")["status"], "above_band");
    let real_estate = class("RealEstate");
    assert_eq!(real_estate["value"], 0.0);
    assert_eq!(real_estate["tolerancePercent"], 5.0);
    assert_eq!(real_estate["status"], "below_band");
    assert_eq!(v["missingClasses"], json!(["RealEstate"]));
    assert_untargeted_in_class_order(&v);
    let mut untargeted: Vec<_> = v["untargetedClasses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    untargeted.sort_unstable();
    assert_eq!(untargeted, ["Bonds", "Commodities", "Gold"]);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn check_policy_reports_breaches_with_codes() {
    let tmp = TempDir::new().unwrap();
    let client = connect(McpOptions {
        policy: Some(write_policy(tmp.path())),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_envelope(&v, "main");
    assert_eq!(v["policyName"], "MCP Test Policy");
    assert_eq!(v["status"], "breach");
    let found = codes(&v["findings"]);
    for code in [
        "allocation_out_of_band",
        "unknown_asset_class",
        "missing_target_class",
        "single_position_limit",
        "asset_class_limit",
        "cash_months_unavailable",
    ] {
        assert!(
            found.contains(&code.to_string()),
            "missing {code}: {found:?}"
        );
    }
    assert!(!found.contains(&"cash_below_minimum_amount".to_string()));
    // BTC (22.22 %) and SPY (20.57 %) exceed the 15 % single-position limit.
    assert_eq!(
        found
            .iter()
            .filter(|c| *c == "single_position_limit")
            .count(),
        2
    );
    let breaches = codes(&v["breaches"]);
    assert!(breaches.contains(&"asset_class_limit".to_string()));
    // Cash drifts +31.44 pts against ±10: beyond 2× tolerance is critical.
    let cash = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "allocation_out_of_band" && f["assetClass"] == "Cash")
        .unwrap();
    assert_eq!(cash["severity"], "critical");
    assert_eq!(cash["category"], "allocation");
    let checks = v["constraintChecks"].as_array().unwrap();
    let cash_check = checks
        .iter()
        .find(|c| c["code"] == "cash_below_minimum_amount")
        .unwrap();
    assert_eq!(cash_check["passed"], true);
    assert_eq!(cash_check["limitAmount"], 1000.0);
    assert!(checks.iter().any(|c| c["code"] == "asset_class_limit"
        && c["assetClass"] == "Cash"
        && c["passed"] == false));
    // Messages never carry monetary amounts.
    for f in v["findings"].as_array().unwrap() {
        let msg = f["message"].as_str().unwrap();
        for amount in ["97208", "50000", "20000", "21600", "18883"] {
            assert!(!msg.contains(amount), "{msg}");
        }
    }
    let n = v["findings"].as_array().unwrap().len();
    let counts = &v["counts"];
    assert_eq!(
        counts["info"].as_u64().unwrap()
            + counts["warning"].as_u64().unwrap()
            + counts["critical"].as_u64().unwrap(),
        n as u64
    );

    let v = ok(
        &client,
        "check_policy",
        json!({ "minSeverity": "critical" }),
    )
    .await;
    assert!(v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["severity"] == "critical"));
    assert_eq!(v["counts"]["warning"], 0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn check_policy_without_policy_and_with_invalid_policy() {
    let client = connect(opts()).await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["status"], "warnings");
    assert!(v["policyName"].is_null());
    assert_eq!(codes(&v["findings"]), ["no_policy"]);
    client.cancel().await.unwrap();

    let tmp = TempDir::new().unwrap();
    let bad = tmp.path().join("bad.toml");
    std::fs::write(&bad, "this is = not [a policy").unwrap();
    let client = connect(McpOptions {
        policy: Some(bad.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["status"], "breach");
    assert_eq!(codes(&v["findings"]), ["invalid_policy"]);
    // The finding never leaks the file path.
    assert!(!v["findings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("bad.toml"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_transactions_synthesizes_purchases() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_transactions", json!({})).await;
    assert_envelope(&v, "main");
    assert_eq!(v["source"], "synthesized_from_purchases");
    assert_eq!(v["totals"]["count"], 7);
    assert!(v["nextCursor"].is_null());
    let txs = v["transactions"].as_array().unwrap();
    assert_eq!(txs.len(), 7);
    let dates: Vec<_> = txs
        .iter()
        .map(|t| t["date"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = dates.clone();
    sorted.sort();
    assert_eq!(dates, sorted);
    for t in txs {
        assert!(t["id"].as_str().unwrap().starts_with("tx_pos_"));
        let expected = if t["assetClass"] == "Cash" {
            "deposit"
        } else {
            "buy"
        };
        assert_eq!(t["type"], expected);
    }
    let spy: Vec<_> = txs.iter().filter(|t| t["ticker"] == "SPY").collect();
    assert_eq!(spy.len(), 2);
    assert_eq!(spy[0]["priceSource"], "historic_close");
    assert_eq!(spy[0]["price"], 400.0);
    assert_eq!(spy[0]["quantity"], 37.0);
    assert_eq!(spy[0]["amount"], 14800.0);
    assert_eq!(spy[0]["currency"], "USD");
    // No historic FX: foreign-currency amounts have no base value.
    assert!(spy[0]["amountBase"].is_null());
    assert!(v["totals"]["buyAmountBase"].is_null());
    assert!(codes(&v["warnings"]).contains(&"fx_historic_unavailable".to_string()));
    let cash = txs.iter().find(|t| t["ticker"].is_null()).unwrap();
    assert_eq!(cash["priceSource"], "cash");
    assert_eq!(cash["currency"], "EUR");
    assert_eq!(cash["amountBase"], 50000.0);

    let v = ok(
        &client,
        "get_transactions",
        json!({ "ticker": "spy", "from": "2025-05-01", "to": "2025-12-31" }),
    )
    .await;
    assert_eq!(v["totals"]["count"], 1);
    assert_eq!(v["transactions"][0]["date"], "2025-07-03");
    let v = ok(&client, "get_transactions", json!({ "types": ["sell"] })).await;
    assert_eq!(v["totals"]["count"], 0);
    assert_eq!(v["totals"]["sellAmountBase"], 0.0);
    let v = ok(&client, "get_transactions", json!({ "assetClass": "cash" })).await;
    assert_eq!(v["totals"]["count"], 1);
    let v = ok(
        &client,
        "get_transactions",
        json!({ "isin": "XX0000000000" }),
    )
    .await;
    assert_eq!(v["totals"]["count"], 0);
    let position_id = spy[0]["positionId"].as_str().unwrap();
    let v = ok(
        &client,
        "get_transactions",
        json!({ "positionId": position_id }),
    )
    .await;
    assert_eq!(v["totals"]["count"], 2);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_transactions_paginates_with_a_cursor() {
    let client = connect(opts()).await;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({ "limit": 3 });
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let v = ok(&client, "get_transactions", args).await;
        assert_eq!(v["totals"]["count"], 7);
        for t in v["transactions"].as_array().unwrap() {
            seen.push(t["id"].as_str().unwrap().to_string());
        }
        match v["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 7);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 7);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn invalid_arguments_are_tool_errors() {
    let client = connect(opts()).await;
    assert_eq!(
        err(&client, "get_transactions", json!({ "from": "15.04.2025" })).await,
        "invalid_argument"
    );
    assert_eq!(
        err(
            &client,
            "get_transactions",
            json!({ "from": "2025-02-01", "to": "2025-01-01" })
        )
        .await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_transactions", json!({ "limit": 0 })).await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_transactions", json!({ "limit": 501 })).await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_transactions", json!({ "cursor": "garbage" })).await,
        "invalid_argument"
    );
    // Unknown fields and bad enum values: structured invalid_argument.
    assert_eq!(
        err(&client, "get_positions", json!({ "bogus": true })).await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_positions", json!({ "sortBy": "random" })).await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_context", json!({ "format": "html" })).await,
        "invalid_argument"
    );
    for tool in [
        "get_positions",
        "get_allocation",
        "check_policy",
        "get_transactions",
        "get_context",
    ] {
        assert_eq!(
            err(&client, tool, json!({ "portfolioId": "nope" })).await,
            "unknown_portfolio",
            "{tool}"
        );
    }
    // Errors never contain monetary values.
    let result = call(&client, "get_positions", json!({ "portfolioId": "nope" })).await;
    assert!(!serde_json::to_string(&result.content)
        .unwrap()
        .contains("97208"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_portfolios_hides_paths_by_default() {
    let client = connect(McpOptions {
        portfolio_id: Some("demo".into()),
        ..opts()
    })
    .await;
    let v = ok(&client, "list_portfolios", json!({})).await;
    assert_eq!(v["schemaVersion"], 1);
    let list = v["portfolios"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    let p = &list[0];
    assert_eq!(p["id"], "demo");
    assert_eq!(p["label"], "example_data.json");
    assert_eq!(p["mode"], "simple");
    assert_eq!(p["encrypted"], false);
    assert_eq!(p["hasPolicy"], false);
    assert_eq!(p["isDefault"], true);
    assert!(p.get("path").is_none());
    let v = ok(&client, "list_portfolios", json!({ "includePaths": true })).await;
    assert_eq!(v["portfolios"][0]["path"], example_data());
    // The configured id is the one tools accept.
    let v = ok(&client, "get_positions", json!({ "portfolioId": "demo" })).await;
    assert_eq!(v["portfolioId"], "demo");
    assert_eq!(
        err(&client, "get_positions", json!({ "portfolioId": "main" })).await,
        "unknown_portfolio"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_context_markdown_and_json() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_context", json!({})).await;
    assert_envelope(&v, "main");
    assert_eq!(v["format"], "markdown");
    assert!(v["markdown"]
        .as_str()
        .unwrap()
        .contains("Portfolio Context"));
    assert!(v.get("context").is_none());
    let v = ok(&client, "get_context", json!({ "format": "json" })).await;
    assert_eq!(v["format"], "json");
    assert!(v.get("markdown").is_none());
    assert_eq!(v["context"]["summary"]["totalValue"], 97208.0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn resources_in_file_mode() {
    let client = connect(opts()).await;
    let resources = client.list_all_resources().await.unwrap();
    let uris: Vec<_> = resources.iter().map(|r| r.uri.as_str()).collect();
    assert_eq!(
        uris,
        [
            "portfolio://main/context.md",
            "portfolio://schema/positions",
            "portfolio://schema/findings"
        ]
    );
    let templates = client.list_all_resource_templates().await.unwrap();
    let templates: Vec<_> = templates.iter().map(|t| t.uri_template.as_str()).collect();
    assert_eq!(
        templates,
        [
            "portfolio://{id}/policy",
            "portfolio://{id}/policy.md",
            "portfolio://{id}/context.md"
        ]
    );

    let (mime, text) = read(&client, "portfolio://main/context.md").await.unwrap();
    assert_eq!(mime, "text/markdown");
    assert!(text.contains("Portfolio Context"));

    let (mime, text) = read(&client, "portfolio://schema/positions").await.unwrap();
    assert_eq!(mime, "application/schema+json");
    let schema: Value = serde_json::from_str(&text).unwrap();
    assert!(schema["properties"]["valueBase"].is_object());
    jsonschema::validator_for(&schema).expect("a valid JSON Schema");

    let (mime, text) = read(&client, "portfolio://schema/findings").await.unwrap();
    assert_eq!(mime, "application/json");
    let catalog: Value = serde_json::from_str(&text).unwrap();
    let catalog_codes = codes(&catalog["codes"]);
    for code in [
        "allocation_out_of_band",
        "no_policy",
        "invalid_policy",
        "no_live_price",
        "fx_historic_unavailable",
    ] {
        assert!(catalog_codes.contains(&code.to_string()), "{code}");
    }

    // No policy loaded, no workspace narrative, unknown ids and paths: not found.
    for uri in [
        "portfolio://main/policy",
        "portfolio://main/policy.md",
        "portfolio://other/context.md",
        "portfolio://main/secrets",
        "portfolio://schema/ledger",
        "file:///etc/passwd",
    ] {
        let e = read(&client, uri).await.expect_err(uri);
        let text = e.to_string();
        assert!(text.contains("not found"), "{uri}: {text}");
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn workspace_mode_exposes_policy_resources() {
    let (_tmp, dir) = workspace();
    let client = connect(McpOptions {
        file: None,
        workspace: Some(dir.to_string_lossy().into_owned()),
        portfolio_id: Some("ws".into()),
        ..opts()
    })
    .await;
    let v = ok(&client, "list_portfolios", json!({})).await;
    assert_eq!(v["portfolios"][0]["mode"], "workspace");
    assert_eq!(v["portfolios"][0]["label"], "demo-ws");
    assert_eq!(v["portfolios"][0]["hasPolicy"], true);

    let uris: Vec<_> = client
        .list_all_resources()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.uri)
        .collect();
    assert!(uris.contains(&"portfolio://ws/policy".to_string()));
    assert!(uris.contains(&"portfolio://ws/policy.md".to_string()));

    let (mime, text) = read(&client, "portfolio://ws/policy").await.unwrap();
    assert_eq!(mime, "application/json");
    let policy: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(policy["schemaVersion"], 1);
    assert_eq!(policy["source"], "workspace");
    assert_eq!(policy["policy"]["name"], "MCP Test Policy");
    let (mime, text) = read(&client, "portfolio://ws/policy.md").await.unwrap();
    assert_eq!(mime, "text/markdown");
    assert!(text.contains("Stay the course"));

    let v = ok(&client, "check_policy", json!({ "portfolioId": "ws" })).await;
    assert_eq!(v["status"], "breach");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn load_rejects_bad_selections() {
    let missing = McpOptions {
        file: Some("/nonexistent/positions.json".into()),
        ..opts()
    };
    assert!(load(&missing).await.is_err());
    let none = McpOptions {
        file: None,
        ..opts()
    };
    assert!(load(&none).await.is_err());
    let bad_id = McpOptions {
        portfolio_id: Some("../x".into()),
        ..opts()
    };
    assert!(load(&bad_id).await.is_err());
    let tmp = TempDir::new().unwrap();
    let no_ws = McpOptions {
        file: None,
        workspace: Some(tmp.path().to_string_lossy().into_owned()),
        ..opts()
    };
    assert!(load(&no_ws).await.is_err());
}

// ------------------------------------------------------------ stdio binary

mod stdio {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run<L: std::fmt::Display>(args: &[&str], lines: &[L]) -> (std::process::Output, Vec<Value>) {
        let home = TempDir::new().unwrap();
        let mut child = Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
            .arg("mcp")
            .args(args)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("XDG_DATA_HOME", home.path().join(".local/share"))
            .env("PORTFOLIO_RS_QUOTES", format!("fixture:{}", fixture_path()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            for line in lines {
                writeln!(stdin, "{line}").unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout.clone()).unwrap();
        // stdout carries the protocol only: every line is a JSON-RPC message.
        let messages = stdout
            .lines()
            .map(|l| {
                let v: Value =
                    serde_json::from_str(l).unwrap_or_else(|_| panic!("non-JSON on stdout: {l}"));
                assert_eq!(v["jsonrpc"], "2.0");
                v
            })
            .collect();
        (output, messages)
    }

    fn initialize() -> Value {
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}})
    }

    fn response(messages: &[Value], id: u64) -> &Value {
        messages
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("no response {id}"))
    }

    /// Finance item 4: a malformed line gets -32700 (id null), the server
    /// keeps serving, and the diagnostic goes to stderr only.
    #[test]
    fn malformed_line_gets_parse_error_and_server_keeps_serving() {
        let example = example_data();
        let lines = vec![
            initialize().to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            "this is not json".to_string(),
            "{\"jsonrpc\": \"2.0\", \"id\": 7,".to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_allocation","arguments":{}}}).to_string(),
        ];
        let (output, messages) = run(&["--offline", &example], &lines);
        let parse_errors: Vec<&Value> = messages
            .iter()
            .filter(|m| m["error"]["code"] == -32700)
            .collect();
        assert_eq!(parse_errors.len(), 2, "{messages:?}");
        assert!(parse_errors.iter().all(|m| m["id"].is_null()));
        assert!(response(&messages, 2)["result"].is_object());
        assert!(response(&messages, 3)["result"]["structuredContent"]["classes"].is_array());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("malformed input line"), "{stderr}");
    }

    #[test]
    fn stdio_round_trip_with_fixture_quotes() {
        let example = example_data();
        let (output, messages) = run(
            &["--offline", &example],
            &[
                initialize(),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_positions","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"portfolio://main/context.md"}}),
                json!({"jsonrpc":"2.0","id":4,"method":"no/such/method"}),
                json!({"jsonrpc":"2.0","method":"notifications/unknown"}),
            ],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // One response per request; notifications get none.
        assert_eq!(messages.len(), 4, "{messages:?}");
        let init = response(&messages, 1);
        assert_eq!(init["result"]["serverInfo"]["name"], "portfolio_rs");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert!(init["result"]["capabilities"]["resources"].is_object());
        // --offline keeps the configured fixture (it never touches the network).
        let positions = &response(&messages, 2)["result"]["structuredContent"];
        assert_eq!(positions["totalValue"], 97208.0);
        assert!(response(&messages, 3)["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Portfolio Context"));
        assert_eq!(response(&messages, 4)["error"]["code"], -32601);
        // Diagnostics go to stderr.
        assert!(String::from_utf8_lossy(&output.stderr).contains("MCP server"));
    }

    #[test]
    fn stdio_missing_file_fails_before_speaking() {
        let (output, messages) = run(&["/nonexistent/positions.json"], &[initialize()]);
        assert!(!output.status.success());
        assert!(messages.is_empty());
    }

    #[test]
    fn stdio_conflicting_selection_is_a_usage_error() {
        let example = example_data();
        let (output, messages) = run(&[&example, "--workspace", "/tmp"], &[] as &[Value]);
        assert_eq!(output.status.code(), Some(2));
        assert!(messages.is_empty());
    }

    #[test]
    fn tools_call_without_name_is_invalid_params() {
        let example = example_data();
        let (_output, messages) = run(
            &["--offline", &example],
            &[
                initialize(),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"arguments":{}}}),
                json!({"jsonrpc":"2.0","id":3,"method":"prompts/list"}),
                json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
                json!([1, 2]),
                json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_positions","arguments":[1]}}),
                json!({"jsonrpc":"2.0","id":6,"method":"ping"}),
            ],
        );
        let batch: Vec<&Value> = messages
            .iter()
            .filter(|m| m["error"]["code"] == -32600)
            .collect();
        assert_eq!(batch.len(), 1, "{messages:?}");
        assert!(batch[0]["id"].is_null());
        assert_eq!(response(&messages, 5)["error"]["code"], -32602);
        assert!(response(&messages, 6)["result"].is_object());
        assert_eq!(
            response(&messages, 2)["error"]["code"],
            -32602,
            "{messages:?}"
        );
        assert_eq!(
            response(&messages, 3)["error"]["code"],
            -32601,
            "{messages:?}"
        );
        assert!(response(&messages, 4)["result"].is_object());
    }
}

#[tokio::test]
async fn docs_document_every_tool_and_resource() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/mcp.md")).unwrap();
    let client = connect(opts()).await;
    for tool in client.list_all_tools().await.unwrap() {
        assert!(
            doc.contains(&format!("`{}`", tool.name)),
            "docs/mcp.md misses tool {}",
            tool.name
        );
    }
    for template in client.list_all_resource_templates().await.unwrap() {
        let uri = template.uri_template.replace("{id}", "<id>");
        assert!(doc.contains(&uri), "docs/mcp.md misses resource {uri}");
    }
    for resource in client.list_all_resources().await.unwrap() {
        let uri = resource
            .uri
            .replace("portfolio://main/", "portfolio://<id>/");
        assert!(doc.contains(&uri), "docs/mcp.md misses resource {uri}");
    }
    client.cancel().await.unwrap();
}

/// Finance t1b: ids for positions without a stored `Id` must survive a
/// restart, stored ids win, and the read-only server never writes the file.
#[tokio::test]
async fn position_ids_are_stable_across_server_starts() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    let mut data: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(example_data()).unwrap()).unwrap();
    data[0]["Id"] = json!("pos_stored");
    let original = serde_json::to_string_pretty(&data).unwrap();
    std::fs::write(&file, &original).unwrap();
    let opts = || McpOptions {
        file: Some(file.to_string_lossy().into_owned()),
        ..opts()
    };
    let ids = |v: &Value| {
        let mut ids: Vec<(String, String)> = v["positions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["name"].as_str().unwrap().to_string(),
                    p["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        ids.sort();
        ids
    };
    let client = connect(opts()).await;
    let first = ids(&ok(&client, "get_positions", json!({})).await);
    let tx1 = ok(&client, "get_transactions", json!({})).await;
    client.cancel().await.unwrap();
    let client = connect(opts()).await;
    let second = ids(&ok(&client, "get_positions", json!({})).await);
    let tx2 = ok(&client, "get_transactions", json!({})).await;
    client.cancel().await.unwrap();
    assert_eq!(first, second);
    assert_eq!(tx1["transactions"], tx2["transactions"]);
    assert!(first.iter().any(|(_, id)| id == "pos_stored"));
    assert!(first.iter().all(|(_, id)| id.starts_with("pos_")));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
}

/// Finance t7 policy: no Commodities target, 25 % single-position limit.
const POLICY_LOWER: &str = r#"
version = "1.0"
name = "Balanced Growth"
base_currency = "EUR"
time_horizon_years = 15
risk_profile = "moderate"

[constraints]
minimum_cash_months = 6
minimum_cash_amount = 30000.0
single_position_limit_percent = 25.0
asset_class_limit_percent = 60.0

[[allocations]]
asset_class = "stocks"
target_percent = 55.0
tolerance_percent = 10.0

[[allocations]]
asset_class = "Bonds"
target_percent = 25.0
tolerance_percent = 5.0

[[allocations]]
asset_class = "Cash"
target_percent = 10.0
tolerance_percent = 5.0

[[allocations]]
asset_class = "CRYPTO"
target_percent = 5.0
tolerance_percent = 3.0

[[allocations]]
asset_class = "Gold"
target_percent = 5.0
tolerance_percent = 3.0
"#;

async fn connect_partial(quotes: &str) -> (TempDir, Client) {
    let tmp = TempDir::new().unwrap();
    let policy = tmp.path().join("policy_lower.toml");
    std::fs::write(&policy, POLICY_LOWER).unwrap();
    let client = connect(McpOptions {
        policy: Some(policy.to_string_lossy().into_owned()),
        quotes: Some(fixture_named(quotes)),
        ..opts()
    })
    .await;
    (tmp, client)
}

fn class<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["assetClass"] == name)
        .unwrap_or_else(|| panic!("class {name} disappeared: {}", v["classes"]))
}

/// Finance t7 (GSG quote missing): the class must not vanish, nothing is
/// judged against the partial total, and every tool says it is incomplete.
#[tokio::test]
async fn missing_quote_is_visible_everywhere() {
    let (_tmp, client) = connect_partial("quotes_no_gsg.json").await;
    for tool in [
        "get_positions",
        "get_allocation",
        "check_policy",
        "get_transactions",
        "get_context",
    ] {
        let v = ok(&client, tool, json!({})).await;
        assert_eq!(v["complete"], false, "{tool}");
        assert_eq!(v["dataQuality"]["missingPrices"], json!(["GSG"]), "{tool}");
        assert_eq!(
            v["dataQuality"]["affectedClasses"],
            json!(["Commodities"]),
            "{tool}"
        );
        assert_eq!(
            v["dataQuality"]["affectedPositions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["networkStatus"], "fixture");
    assert_eq!(v["dataStatus"], "partial");
    let gsg = by_ticker(&v["positions"], "GSG");
    assert!(gsg["valueBase"].is_null());
    assert!(gsg["weightPercent"].is_null());
    assert_eq!(
        codes(&v["warnings"])
            .iter()
            .filter(|c| *c == "no_live_price")
            .count(),
        1
    );

    let v = ok(&client, "get_allocation", json!({})).await;
    let commodities = class(&v, "Commodities");
    assert!(commodities["value"].is_null());
    assert!(commodities["actualPercent"].is_null());
    // Finance item 1: only the class with the unpriced position is unknown;
    // the others are judged on the priced part.
    assert!(["unknown", "untargeted"].contains(&commodities["status"].as_str().unwrap()));
    assert!(commodities["driftPoints"].is_null());
    // Finance retest B1/B3: no percentages or drift against a partial
    // total, only a possible range.
    let bonds = class(&v, "Bonds");
    assert!(bonds["actualPercent"].is_null());
    assert!(bonds["driftPoints"].is_null());
    assert!(bonds["amountToTarget"].is_null());
    assert!(
        bonds["actualPercentMin"].as_f64().unwrap() <= bonds["actualPercentMax"].as_f64().unwrap()
    );
    assert_eq!(v["excludedPositions"].as_array().unwrap().len(), 1);
    // GSG has no price, no cache and no known currency: no estimate.
    assert!(v["excludedValueShare"].is_null());
    assert!(v["excludedValueBasis"].is_null());

    let v = ok(&client, "check_policy", json!({})).await;
    let found = codes(&v["findings"]);
    assert!(!found.contains(&"checks_incomplete".to_string()));
    assert!(codes(&v["warnings"]).contains(&"checks_incomplete".to_string()));
    // Data-quality warnings appear once, in the top-level warnings only.
    assert!(!found.contains(&"no_live_price".to_string()));
    assert!(codes(&v["warnings"]).contains(&"no_live_price".to_string()));
    assert_ne!(v["status"], "ok");
    client.cancel().await.unwrap();
}

/// Finance t7 (EURUSD missing): USD positions are unpriced, not 0 and not
/// labelled as EUR; the status is not `connected`.
#[tokio::test]
async fn missing_fx_is_visible_everywhere() {
    let (_tmp, client) = connect_partial("quotes_no_fx.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["complete"], false);
    assert_eq!(v["dataQuality"]["missingFx"], json!(["USD/EUR"]));
    assert_ne!(v["networkStatus"], "connected");
    let spy = by_ticker(&v["positions"], "SPY");
    assert!(spy["valueBase"].is_null());
    assert!(spy["weightPercent"].is_null());
    assert_eq!(v["totalValue"], 50000.0);
    let v = ok(&client, "get_allocation", json!({})).await;
    assert!(class(&v, "Stocks")["value"].is_null());
    // Finance item 1: drift on the priced part; only classes holding an
    // unpriced position are unknown.
    assert_eq!(class(&v, "Stocks")["status"], "unknown");
    // Finance retest B1: Cash is 100% of the priced part, but the unpriced
    // USD positions could be worth anything: no breach, no amountToTarget.
    let cash = class(&v, "Cash");
    assert_eq!(cash["status"], "unknown");
    assert!(cash["amountToTarget"].is_null());
    assert!(cash["driftPoints"].is_null());
    assert!(cash["actualPercent"].is_null());
    assert_eq!(cash["actualPercentMin"], 0.0);
    assert_eq!(cash["actualPercentMax"], 100.0);
    assert!(!v["excludedPositions"].as_array().unwrap().is_empty());
    let v = ok(&client, "check_policy", json!({})).await;
    assert!(!codes(&v["breaches"]).contains(&"asset_class_limit".to_string()));
    assert!(!codes(&v["breaches"]).contains(&"allocation_out_of_band".to_string()));
    let possible: Vec<_> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "possible_breach")
        .collect();
    assert!(
        possible.iter().any(|f| f["assetClass"] == "Cash"),
        "{possible:?}"
    );
    assert!(possible.iter().all(|f| f["severity"] == "warning"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn complete_data_says_so() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(v["complete"], true);
    assert_eq!(v["dataQuality"]["affectedPositions"], json!([]));
    client.cancel().await.unwrap();
}

/// Finance item 3: get_context shows unpriced values as null, real quote
/// currencies, no false cash flag when incomplete, and MCP follow-ups.
#[tokio::test]
async fn get_context_is_honest_about_partial_data() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_context", json!({ "format": "json" })).await;
    let ctx = &v["context"];
    assert_eq!(ctx["complete"], true);
    let spy = ctx["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ticker"] == "SPY")
        .unwrap();
    assert_eq!(spy["price"], 500.0);
    assert_eq!(spy["priceCurrency"], "USD");
    let follow: Vec<String> = ctx["followUpCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert!(follow.iter().any(|c| c == "get_positions"), "{follow:?}");
    assert!(
        !follow.iter().any(|c| c.contains("portfolio_rs ")),
        "{follow:?}"
    );
    let v = ok(&client, "get_context", json!({})).await;
    let md = v["markdown"].as_str().unwrap();
    assert!(md.contains("500.00 USD"), "{md}");
    assert!(!md.contains("500.00 EUR"), "{md}");
    assert!(md.contains("`get_positions`"));
    client.cancel().await.unwrap();

    let (_tmp, client) = connect_partial("quotes_no_fx.json").await;
    let v = ok(&client, "get_context", json!({ "format": "json" })).await;
    let ctx = &v["context"];
    assert_eq!(ctx["complete"], false);
    assert_ne!(ctx["networkStatus"], "connected");
    for p in ctx["positions"].as_array().unwrap() {
        if p["ticker"].is_string() {
            assert!(p["value"].is_null(), "{p}");
            assert!(p["weightPercent"].is_null(), "{p}");
        }
    }
    let flags = ctx["riskFlags"].to_string();
    assert!(!flags.contains("100.00%"), "{flags}");
    assert!(!flags.contains("Asset-class concentration"), "{flags}");
    let v = ok(&client, "get_context", json!({})).await;
    assert!(v["markdown"].as_str().unwrap().contains("INCOMPLETE"));
    client.cancel().await.unwrap();

    let (_tmp, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "get_context", json!({ "format": "json" })).await;
    let commodities = v["context"]["allocation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["assetClass"] == "Commodities")
        .expect("Commodities stays in the context allocation");
    assert!(commodities["value"].is_null());
    client.cancel().await.unwrap();
}

fn position<'a>(v: &'a Value, ticker: &str) -> &'a Value {
    v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ticker"] == ticker)
        .unwrap_or_else(|| panic!("no {ticker}"))
}

/// Finance item 5: cost basis is never a USD amount labelled as base
/// currency, a missing quote raises no false missing_cost_basis, and the
/// current-FX conversion is flagged once, in get_positions and get_context.
#[tokio::test]
async fn cost_basis_is_never_unconverted_and_current_fx_is_flagged() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_positions", json!({})).await;
    let spy = position(&v, "SPY");
    assert_eq!(spy["averageCost"], 400.0);
    assert_eq!(spy["priceCurrency"], "USD");
    assert_eq!(spy["invested"], 16000.0, "400 USD x 50 x 0.8 EUR/USD");
    let warn = codes(&v["warnings"]);
    assert_eq!(
        warn.iter()
            .filter(|c| *c == "cost_basis_current_fx")
            .count(),
        1,
        "{warn:?}"
    );
    let ctx = ok(&client, "get_context", json!({ "format": "json" })).await;
    assert!(ctx["context"]["dataQualityFlags"]
        .to_string()
        .contains("current FX rate"));
    client.cancel().await.unwrap();

    let (_tmp, client) = connect_partial("quotes_no_fx.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    for t in ["SPY", "BTC-USD", "TLT", "GLD"] {
        let p = position(&v, t);
        assert!(
            p["invested"].is_null(),
            "USD cost must not be labelled EUR: {p}"
        );
        assert!(
            p["averageCost"].is_number(),
            "average cost stays (in USD): {p}"
        );
        assert_eq!(p["priceCurrency"], "USD");
    }
    let warn = codes(&v["warnings"]);
    assert!(warn.contains(&"fx_unavailable".to_string()));
    assert!(
        !warn.contains(&"missing_cost_basis".to_string()),
        "{warn:?}"
    );
    assert!(!warn.contains(&"cost_basis_current_fx".to_string()));
    let ctx = ok(&client, "get_context", json!({ "format": "json" })).await;
    let spy = position(&ctx["context"], "SPY");
    assert!(spy["invested"].is_null(), "{spy}");
    assert!(!ctx["context"]["dataQualityFlags"]
        .to_string()
        .contains("Missing cost basis"));
    client.cancel().await.unwrap();

    let (_tmp, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(position(&v, "SPY")["invested"], 16000.0);
    assert!(
        !codes(&v["warnings"]).contains(&"missing_cost_basis".to_string()),
        "the gap comes from the missing quote, reported as no_live_price"
    );
    client.cancel().await.unwrap();
}

/// Finance item 6: the single-position limit agrees across CLI `review`,
/// get_context riskFlags and check_policy. Cash is excluded everywhere.
/// Example weights: Cash 51.4 %, BTC 22.2 %, SPY 20.6 %; limit 21 %.
#[tokio::test]
async fn single_position_limit_agrees_everywhere_and_excludes_cash() {
    let tmp = TempDir::new().unwrap();
    let policy = tmp.path().join("policy.toml");
    std::fs::write(
        &policy,
        POLICY_LOWER.replace(
            "single_position_limit_percent = 25.0",
            "single_position_limit_percent = 21.0",
        ),
    )
    .unwrap();
    let client = connect(McpOptions {
        policy: Some(policy.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;

    let v = ok(&client, "check_policy", json!({})).await;
    let mut breached: Vec<String> = v["constraintChecks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["code"] == "single_position_limit" && c["passed"] == false)
        .map(|c| c["positionId"].as_str().unwrap().to_string())
        .collect();
    breached.sort();
    let positions = ok(&client, "get_positions", json!({})).await;
    let btc_id = position(&positions, "BTC-USD")["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(breached, vec![btc_id]);

    let ctx = ok(&client, "get_context", json!({ "format": "json" })).await;
    let flags: Vec<String> = ctx["context"]["riskFlags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .filter(|f| f.starts_with("Single-position"))
        .collect();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(
        flags[0].contains("Bitcoin") || flags[0].contains("BTC"),
        "{flags:?}"
    );
    assert!(flags[0].contains("cash excluded"));
    client.cancel().await.unwrap();

    let home = TempDir::new().unwrap();
    let out = std::process::Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
        .args(["review", &example_data(), "--policy"])
        .arg(&policy)
        .args(["--format", "json"])
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env("PORTFOLIO_RS_QUOTES", format!("fixture:{}", fixture_path()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let review: Value = serde_json::from_slice(&out.stdout).unwrap();
    let single: Vec<&Value> = review["constraintChecks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == "Single Position Limit")
        .collect();
    assert_eq!(single.len(), 1, "{single:?}");
    assert!(!single[0]["message"].as_str().unwrap().contains("Cash"));
}

/// Finance item 7: `breaches` holds only critical findings, deduplicated;
/// warning-severity findings are in the top-level `warnings`; without a
/// policy every held class, Cash included, is untargeted.
#[tokio::test]
async fn check_policy_separates_breaches_from_warnings() {
    let tmp = TempDir::new().unwrap();
    let policy = tmp.path().join("policy.toml");
    std::fs::write(&policy, POLICY_LOWER).unwrap();
    let client = connect(McpOptions {
        policy: Some(policy.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["status"], "breach");
    let breaches = v["breaches"].as_array().unwrap();
    assert!(!breaches.is_empty());
    assert!(
        breaches.iter().all(|b| b["severity"] == "critical"),
        "{breaches:?}"
    );
    let keys: Vec<String> = breaches
        .iter()
        .map(|b| format!("{}|{}|{}", b["code"], b["positionId"], b["assetClass"]))
        .collect();
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(keys.len(), unique.len(), "duplicate breaches: {keys:?}");
    // Gold drifts -3.35 pts with ±3 tolerance: a warning, not a breach.
    assert!(!breaches.iter().any(|b| b["assetClass"] == "Gold"));
    let gold = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["assetClass"] == "Gold" && w["code"] == "allocation_out_of_band")
        .expect("Gold drift is a top-level warning");
    assert_eq!(gold["severity"], "warning");
    // Commodities has no target in POLICY_LOWER.
    assert!(codes(&v["warnings"]).contains(&"unknown_asset_class".to_string()));
    client.cancel().await.unwrap();

    let client = connect(opts()).await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_untargeted_in_class_order(&v);
    let mut untargeted: Vec<String> = v["untargetedClasses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    untargeted.sort();
    assert_eq!(
        untargeted,
        ["Bonds", "Cash", "Commodities", "Crypto", "Gold", "Stocks"]
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn protocol_edges_from_finance_low_items() {
    let client = connect(opts()).await;
    // Wrong type: structured invalid_argument, not rmcp's plain text.
    assert_eq!(
        err(&client, "get_positions", json!({"includeManual": "yes"})).await,
        "invalid_argument"
    );
    // Limit schema states the real range.
    let tools = client.list_all_tools().await.unwrap();
    let tx = tools.iter().find(|t| t.name == "get_transactions").unwrap();
    let limit = &Value::Object((*tx.input_schema).clone())["properties"]["limit"];
    assert_eq!(limit["minimum"], 1, "{limit}");
    assert_eq!(limit["maximum"], 500, "{limit}");
    // Prompts are not advertised, so prompts/list is not served.
    assert!(client.list_prompts(None).await.is_err());
    client.cancel().await.unwrap();
}

/// Loading with Yahoo fetches quotes, so this one needs the network.
#[tokio::test]
#[ignore = "requires network access to Yahoo Finance"]
async fn live_provider_is_open_world() {
    // A live quote provider is open-world for the tools that price positions.
    let mut live = opts();
    live.cache = portfolio_rs::mcp::CacheMode::Off;
    live.quotes = Some(AnyProvider::Yahoo);
    let client = connect(live).await;
    for tool in client.list_all_tools().await.unwrap() {
        let ann = tool.annotations.unwrap();
        assert_eq!(
            ann.open_world_hint,
            Some(tool.name != "list_portfolios"),
            "{}",
            tool.name
        );
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn unknown_filters_are_errors_not_empty_results() {
    let client = connect(opts()).await;
    assert_eq!(
        err(&client, "get_positions", json!({"assetClass": "Nope"})).await,
        "invalid_argument"
    );
    assert_eq!(
        err(&client, "get_transactions", json!({"assetClass": "Nope"})).await,
        "invalid_argument"
    );
    assert_eq!(
        err(
            &client,
            "get_transactions",
            json!({"positionId": "pos_nope"})
        )
        .await,
        "not_found"
    );
    // Known values still filter (case-insensitive).
    let v = ok(&client, "get_positions", json!({"assetClass": "crypto"})).await;
    assert!(!v["positions"].as_array().unwrap().is_empty());
    let id = v["positions"][0]["id"].as_str().unwrap().to_string();
    let t = ok(&client, "get_transactions", json!({"positionId": id})).await;
    assert!(t["transactions"].is_array());
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn cash_buffer_in_check_policy_and_get_context() {
    let (_tmp, client) = connect_partial("quotes_example.json").await;
    let expected = json!({"minimum": 30000.0, "actual": 50000.0, "surplus": 20000.0});
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["cashBuffer"], expected);
    let c = ok(&client, "get_context", json!({"format": "json"})).await;
    assert_eq!(c["context"]["cashBuffer"], expected);
    let m = ok(&client, "get_context", json!({})).await;
    assert!(m["markdown"]
        .as_str()
        .unwrap()
        .contains("Cash Buffer: minimum"));
    client.cancel().await.unwrap();

    // Without a policy there is no minimum, so no surplus either.
    let client = connect(opts()).await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(
        v["cashBuffer"],
        json!({"minimum": null, "actual": 50000.0, "surplus": null})
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_positions_portfolio_totals_currency_and_price_as_of() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_positions", json!({})).await;
    // Sum of the market rows (cash has no cost basis).
    assert_eq!(v["invested"], 39900.0);
    assert_eq!(v["unrealizedPnl"], 7308.0);
    assert_eq!(v["unrealizedPnlPercent"], 18.32);
    assert!(v["dayPnl"].is_number());
    let cash = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "cash")
        .unwrap();
    assert_eq!(cash["currency"], "EUR");
    assert_eq!(by_ticker(&v["positions"], "SPY")["currency"], "USD");
    // priceAsOf comes from the provider; null when it reports none.
    assert_eq!(
        by_ticker(&v["positions"], "SPY")["priceAsOf"],
        "2026-01-02T21:00:00Z"
    );
    assert!(by_ticker(&v["positions"], "TLT")["priceAsOf"].is_null());
    assert!(cash["priceAsOf"].is_null());
    client.cancel().await.unwrap();

    // One market position without a quote: sums over the priced positions,
    // flagged partial (Finance item 1).
    let (_tmp, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["pnlPartial"], true);
    assert!(v["unrealizedPnl"].is_number(), "{}", v["unrealizedPnl"]);
    assert!(v["invested"].is_number());
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_transactions_cash_is_a_deposit_and_fees_are_never_mixed_null_zero() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_transactions", json!({})).await;
    let txs = v["transactions"].as_array().unwrap();
    // The cash entry is a deposit at par, not a "buy" with a missing price.
    let deposits: Vec<&Value> = txs.iter().filter(|t| t["type"] == "deposit").collect();
    assert_eq!(deposits.len(), 1, "{txs:?}");
    assert_eq!(deposits[0]["price"], 1.0);
    assert_eq!(deposits[0]["amountBase"], 50000.0);
    assert_eq!(deposits[0]["currency"], "EUR");
    assert_eq!(v["totals"]["depositAmountBase"], 50000.0);
    assert_eq!(v["totals"]["withdrawalAmountBase"], 0.0);
    // fees is a number whenever amount is computed (0 = none recorded).
    for t in txs {
        assert!(t["fees"].is_number(), "{t}");
    }
    let only = ok(&client, "get_transactions", json!({"types": ["buy"]})).await;
    assert!(only["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["type"] == "buy"));
    client.cancel().await.unwrap();
}

/// t-cache: a successful fetch fills `.<datafile>.quotes-cache.json` (0600,
/// data file untouched); a failed fetch falls back to it as stale; the max age
/// and `--no-cache` are honoured.
#[tokio::test]
async fn last_known_price_cache_fills_and_falls_back() {
    use portfolio_rs::mcp::CacheMode;
    let (_tmp, dir) = workspace();
    let data = dir.join("positions.json");
    let before = std::fs::read(&data).unwrap();
    let ws = |quotes: AnyProvider, cache: CacheMode, max_age: Option<i64>| McpOptions {
        workspace: Some(dir.to_string_lossy().into_owned()),
        currency: "EUR".into(),
        quotes: Some(quotes),
        cache,
        cache_max_age_days: max_age,
        stale_after_days: Some(100_000),
        ..Default::default()
    };

    // Live (fixture) fetch writes the cache.
    let client = connect(ws(fixture(), CacheMode::On, None)).await;
    let live = ok(&client, "get_positions", json!({})).await;
    assert_eq!(live["hasStale"], false);
    assert_eq!(live["stalePositions"], json!([]));
    assert_eq!(live["priceAsOf"], "2026-01-02T21:00:00Z");
    assert_eq!(
        by_ticker(&live["positions"], "SPY")["priceSource"],
        "fixture"
    );
    assert_eq!(by_ticker(&live["positions"], "SPY")["isStale"], false);
    let cash = live["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "cash")
        .unwrap();
    assert_eq!(cash["priceSource"], "none");
    let cache = dir.join(".positions.json.quotes-cache.json");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    assert_eq!(file["version"], 1);
    assert_eq!(file["quotes"]["SPY"]["price"], 500.0);
    assert_eq!(file["quotes"]["SPY"]["previousClose"], 495.0);
    assert_eq!(file["quotes"]["SPY"]["currency"], "USD");
    assert_eq!(file["quotes"]["SPY"]["asOf"], "2026-01-02T21:00:00Z");
    assert!(file["fx"]["EURUSD=X"]["price"].is_number());
    assert!(file["quotes"]["SPY"].get("quantity").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cache).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(std::fs::read(&data).unwrap(), before, "data file untouched");
    drop(client);

    // Offline: every price from the cache; SPY's asOf is older than the
    // default 7 days, so it does not count. A cached price is informational,
    // not stale by itself (Answer 3: stale is age-based).
    let client = connect(ws(AnyProvider::Offline, CacheMode::Auto, None)).await;
    let v = ok(&client, "get_positions", json!({})).await;
    let tlt = by_ticker(&v["positions"], "TLT");
    assert_eq!(tlt["priceSource"], "cache");
    assert_eq!(tlt["isStale"], false);
    assert_eq!(tlt["priceStatus"], "cache");
    assert_eq!(tlt["price"], 90.0);
    assert!(tlt["priceAsOf"].is_string());
    assert!(tlt["dayChangePercent"].is_number(), "cached previous close");
    assert!(by_ticker(&v["positions"], "SPY")["price"].is_null());
    assert_eq!(by_ticker(&v["positions"], "SPY")["priceSource"], "none");
    assert_eq!(v["complete"], false);
    assert_eq!(v["hasStale"], false);
    assert_eq!(v["networkStatus"], "offline");
    drop(client);

    // A longer max age lets SPY count again: complete; SPY's asOf is months
    // old, so with the default --stale-after (3 days) it is stale.
    let client = connect(McpOptions {
        stale_after_days: None,
        ..ws(AnyProvider::Offline, CacheMode::Auto, Some(3650))
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["complete"], true, "{}", v["dataQuality"]);
    assert_eq!(v["hasStale"], true);
    assert_eq!(v["totalValue"], live["totalValue"]);
    assert_eq!(v["priceAsOf"], "2026-01-02T21:00:00Z");
    let market = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] == "market")
        .count();
    // Only positions with an old asOf (SPY) are stale; fresh cached prices
    // without a provider asOf are not (Answer 3).
    let stale = v["stalePositions"].as_array().unwrap().len();
    assert!(stale >= 1 && stale < market, "{}", v["stalePositions"]);
    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    let text = ctx.to_string();
    assert!(text.contains("\"isStale\":true"), "{text}");
    assert!(text.contains("\"priceAsOf\":\"2026-01-02T21:00:00Z\""));
    drop(client);
    // Offline never writes.
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    assert_eq!(after, file);

    // --no-cache: nothing from the cache.
    let client = connect(ws(AnyProvider::Offline, CacheMode::Off, Some(3650))).await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert!(by_ticker(&v["positions"], "TLT")["price"].is_null());
    assert_eq!(v["hasStale"], false);
    assert_eq!(std::fs::read(&data).unwrap(), before, "data file untouched");
}

/// t9 nofx/offline/full: one networkStatus across every tool and get_context;
/// quotes without FX are `partial`, not `offline`.
#[tokio::test]
async fn network_status_is_shared_across_tools() {
    async fn statuses(client: &Client) -> Vec<String> {
        let mut out = Vec::new();
        for tool in [
            "get_positions",
            "get_allocation",
            "check_policy",
            "get_transactions",
            "get_context",
        ] {
            let v = ok(client, tool, json!({})).await;
            out.push(v["networkStatus"].as_str().unwrap().to_string());
        }
        let v = ok(client, "get_context", json!({"format": "json"})).await;
        out.push(v["context"]["networkStatus"].as_str().unwrap().to_string());
        let md = ok(client, "get_context", json!({})).await;
        let md = md["markdown"].as_str().unwrap().to_string();
        assert!(md.contains(&format!("Network Status: {}", out[0])), "{md}");
        out
    }
    // Under a quote fixture the status is always "fixture" (Finance item 6).
    let (_t, client) = connect_partial("quotes_no_fx.json").await;
    assert_eq!(statuses(&client).await, vec!["fixture"; 6]);
    let (_t, client) = connect_partial("quotes_no_gsg.json").await;
    assert_eq!(statuses(&client).await, vec!["fixture"; 6]);
    let client = connect(opts()).await;
    assert_eq!(statuses(&client).await, vec!["fixture"; 6]);
    let client = connect(McpOptions {
        quotes: Some(AnyProvider::Offline),
        ..opts()
    })
    .await;
    assert_eq!(statuses(&client).await, vec!["offline"; 6]);
}

/// (d) list_portfolios names the quote source, without complete/dataQuality.
#[tokio::test]
async fn list_portfolios_reports_quote_source() {
    let client = connect(opts()).await;
    let v = ok(&client, "list_portfolios", json!({})).await;
    assert_eq!(v["portfolios"][0]["quoteSource"], "fixture");
    assert!(v.get("complete").is_none() && v.get("dataQuality").is_none());
    let client = connect(McpOptions {
        quotes: Some(AnyProvider::Offline),
        ..opts()
    })
    .await;
    let v = ok(&client, "list_portfolios", json!({})).await;
    assert_eq!(v["portfolios"][0]["quoteSource"], "offline");
}

/// Item 11: class and position order in the context markdown and JSON is
/// deterministic, also when unknown (null) values tie.
#[tokio::test]
async fn context_order_is_deterministic() {
    let mut seen_md = None;
    let mut seen_json = None;
    for _ in 0..12 {
        let (_t, client) = connect_partial("quotes_no_fx.json").await;
        let md = ok(&client, "get_context", json!({})).await["markdown"].clone();
        let js = ok(&client, "get_context", json!({"format": "json"})).await["context"].clone();
        let classes: Vec<Value> = js["allocation"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["assetClass"].clone())
            .collect();
        let names: Vec<Value> = js["positions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].clone())
            .collect();
        let md = md
            .as_str()
            .unwrap()
            .lines()
            .filter(|l| !l.to_ascii_lowercase().contains("generated"))
            .collect::<Vec<_>>()
            .join("\n");
        if let (Some(m), Some(j)) = (&seen_md, &seen_json) {
            assert_eq!(&md, m);
            assert_eq!(&(classes.clone(), names.clone()), j);
        } else {
            seen_md = Some(md);
            seen_json = Some((classes, names));
        }
    }
}

/// Item 5: get_context JSON (shared with /api/context) rounds money and
/// percentages to 2 decimals; quantities keep their precision.
#[tokio::test]
async fn context_json_is_rounded_to_two_decimals() {
    fn check(v: &Value, key: &str, bad: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, v)| check(v, k, bad)),
            Value::Array(a) => a.iter().for_each(|v| check(v, key, bad)),
            Value::Number(n) if key != "quantity" && key != "positionCount" => {
                let f = n.as_f64().unwrap();
                if ((f * 100.0).round() / 100.0 - f).abs() > 1e-9 {
                    bad.push(format!("{key}={f}"));
                }
            }
            _ => {}
        }
    }
    let partial = connect_partial("quotes_no_gsg.json").await;
    for (client, complete) in [(connect(opts()).await, true), (partial.1, false)] {
        let v = ok(&client, "get_context", json!({"format": "json"})).await;
        let mut bad = Vec::new();
        check(&v["context"], "", &mut bad);
        assert!(bad.is_empty(), "{bad:?}");
        let mut saw_fraction = false;
        for p in v["context"]["positions"].as_array().unwrap() {
            if let Some(w) = p["weightPercent"].as_f64() {
                saw_fraction |= w.fract() != 0.0;
            }
        }
        // Partial data has no weights at all (retest B3).
        assert_eq!(
            saw_fraction, complete,
            "fractional weights only when complete"
        );
    }
}

/// t12 / item 9: the synthesized opening deposit is dated (earliest purchase,
/// else the file date), has fees 0, and survives date filters.
#[tokio::test]
async fn opening_deposit_is_dated_and_filterable() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    let data = json!([
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 50000, "Purchases": []},
        {"Name": "Stocks", "AssetClass": "Stocks", "Ticker": "SPY", "Amount": 50,
         "Purchases": [{"Date": "2025/07/03", "Quantity": 13}, {"Date": "2025/04/15", "Quantity": 37}]}
    ]);
    std::fs::write(&file, data.to_string()).unwrap();
    let client = connect(McpOptions {
        file: Some(file.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_transactions", json!({"types": ["deposit"]})).await;
    let tx = &v["transactions"][0];
    assert_eq!(tx["date"], "2025-04-15", "{v}");
    assert_eq!(tx["fees"], 0.0);
    assert_eq!(tx["synthesized"], true);
    assert_eq!(tx["dateSource"], "earliest_purchase");
    assert_eq!(tx["amount"], 50000.0);
    let v = ok(&client, "get_transactions", json!({})).await;
    assert_eq!(v["transactions"][0]["type"], "deposit", "opening first");
    assert_eq!(v["transactions"][1]["synthesized"], false);
    assert!(v["transactions"][1].get("dateSource").is_none());
    let from = ok(&client, "get_transactions", json!({"from": "2025-01-01"})).await;
    assert!(from["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["type"] == "deposit"));
    let v = ok(
        &client,
        "get_transactions",
        json!({"to": "2025-12-31", "types": ["deposit"]}),
    )
    .await;
    assert_eq!(v["totals"]["depositAmountBase"], 50000.0);
    let v = ok(&client, "get_transactions", json!({"to": "2025-01-01"})).await;
    assert_eq!(v["transactions"], json!([]));

    // Without any dated purchase the file date is used.
    std::fs::write(
        &file,
        json!([{"Name": "Cash", "AssetClass": "Cash", "Amount": 10, "Purchases": []}]).to_string(),
    )
    .unwrap();
    let client = connect(McpOptions {
        file: Some(file.to_string_lossy().into_owned()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_transactions", json!({})).await;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(v["transactions"][0]["date"], today);
    assert_eq!(v["transactions"][0]["dateSource"], "file_mtime");
}

/// Item 10 / retest A3: `complete` describes current values only; missing
/// historic FX is counted in fxHistoricMissing and warned about.
#[tokio::test]
async fn transactions_complete_reflects_current_values_only() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_transactions", json!({})).await;
    assert_eq!(v["complete"], true, "{v}");
    assert!(v["totals"]["buyAmountBase"].is_null());
    let n = v["fxHistoricMissing"].as_u64().unwrap();
    assert!(n > 0);
    let w = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "fx_historic_unavailable")
        .unwrap();
    assert_eq!(w["severity"], "warning");
    assert_eq!(w["detail"]["fxHistoricMissing"].as_u64().unwrap(), n);
    assert!(!w["detail"]["positionIds"].as_array().unwrap().is_empty());
    let v = ok(&client, "get_transactions", json!({"types": ["deposit"]})).await;
    assert_eq!(v["complete"], true, "{v}");
    assert_eq!(v["fxHistoricMissing"], 0);
    assert!(v["totals"]["depositAmountBase"].is_number());
    assert!(!codes(&v["warnings"]).contains(&"fx_historic_unavailable".to_string()));
}

/// t9 nofx / item 7: without FX the native quote stays (price, priceCurrency,
/// day change); only base-currency values are null.
#[tokio::test]
async fn no_fx_keeps_native_price() {
    let (_t, client) = connect_partial("quotes_no_fx.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    let spy = by_ticker(&v["positions"], "SPY");
    assert_eq!(spy["price"], 500.0, "{spy}");
    assert_eq!(spy["priceCurrency"], "USD");
    assert_eq!(spy["priceStatus"], "fx_unavailable");
    assert!(spy["dayChangePercent"].is_number(), "{spy}");
    assert!(spy["valueBase"].is_null());
    assert!(spy["weightPercent"].is_null());
    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    let spy = ctx["context"]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ticker"] == "SPY")
        .unwrap()
        .clone();
    assert_eq!(spy["price"], 500.0, "{spy}");
    assert_eq!(spy["priceCurrency"], "USD");
    assert!(spy["value"].is_null());
    let md = ok(&client, "get_context", json!({})).await["markdown"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(md.contains("500.00 USD"), "{md}");
}

/// Finance item 3: an incomplete context nulls the cash weights and shares
/// derived from the partial total, and leads with an incomplete banner.
#[tokio::test]
async fn incomplete_context_nulls_cash_weights_and_has_banner() {
    let (_t, client) = connect_partial("quotes_no_fx.json").await;
    let ctx = ok(&client, "get_context", json!({"format": "json"})).await["context"].clone();
    assert_eq!(ctx["complete"], false);
    let summary = &ctx["summary"];
    assert!(summary["incomplete"]
        .as_str()
        .unwrap()
        .starts_with("INCOMPLETE"));
    assert!(summary["cashPercent"].is_null(), "{summary}");
    assert!(summary["largestPositionPercent"].is_null(), "{summary}");
    for p in ctx["positions"].as_array().unwrap() {
        if p["isCash"] == true {
            assert!(p["weightPercent"].is_null(), "{p}");
        }
    }
    let cash_alloc = ctx["allocation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["assetClass"] == "Cash")
        .unwrap()
        .clone();
    assert!(cash_alloc["percent"].is_null(), "{cash_alloc}");
    assert!(
        cash_alloc["value"].is_number(),
        "cash value itself is known"
    );
    let md = ok(&client, "get_context", json!({})).await["markdown"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        md.starts_with("# Portfolio Context\n\n> **INCOMPLETE"),
        "{md}"
    );
    assert!(md.contains("- Cash: ") && md.contains("(n/a)"), "{md}");
}

/// Finance retest B3: with partial data no tool derives a weight or class
/// share from the incomplete total (cash or not).
#[tokio::test]
async fn partial_data_has_no_weights_anywhere() {
    for quotes in ["quotes_no_gsg.json", "quotes_no_fx.json"] {
        let (_t, client) = connect_partial(quotes).await;
        let v = ok(&client, "get_positions", json!({})).await;
        assert_eq!(v["complete"], false);
        for p in v["positions"].as_array().unwrap() {
            assert!(p["weightPercent"].is_null(), "{quotes} {p}");
        }
        let v = ok(&client, "get_allocation", json!({})).await;
        for c in v["classes"].as_array().unwrap() {
            assert!(c["actualPercent"].is_null(), "{quotes} {c}");
            assert!(c["actualPercentMin"].is_number(), "{quotes} {c}");
        }
        let v = ok(&client, "get_context", json!({"format": "json"})).await;
        let ctx = &v["context"];
        for p in ctx["positions"].as_array().unwrap() {
            assert!(p["weightPercent"].is_null(), "{quotes} {p}");
        }
        for a in ctx["allocation"].as_array().unwrap() {
            assert!(a["percent"].is_null(), "{quotes} {a}");
        }
        assert!(ctx["summary"]["cashPercent"].is_null());
        assert!(ctx["summary"]["largestPositionPercent"].is_null());
        client.cancel().await.unwrap();
    }
}

/// Finance item 8 / retest A6: `counts`, `breaches` and `policyStatus` come
/// from the policy findings only; data-quality items never change them and
/// appear only in the top-level `warnings` (fixture and partial data, every
/// minSeverity).
#[tokio::test]
async fn check_policy_counts_match_lists() {
    for quotes in [
        "quotes_example.json",
        "quotes_no_fx.json",
        "quotes_no_gsg.json",
    ] {
        let (_t, client) = connect_partial(quotes).await;
        for min in ["info", "warning", "critical"] {
            let v = ok(&client, "check_policy", json!({"minSeverity": min})).await;
            let findings = v["findings"].as_array().unwrap();
            assert!(
                findings.iter().all(|f| f["category"] != "data_quality"),
                "{quotes} {min} {v}"
            );
            let sev = |s: &str| findings.iter().filter(|f| f["severity"] == s).count() as u64;
            let counts = &v["counts"];
            for s in ["info", "warning", "critical"] {
                assert_eq!(
                    counts[s].as_u64().unwrap(),
                    sev(s),
                    "{quotes} {min} {s} {v}"
                );
            }
            assert_eq!(
                v["breaches"].as_array().unwrap().len() as u64,
                sev("critical"),
                "{quotes} {min} {v}"
            );
            // Policy warnings are in the top-level warnings too.
            let top = v["warnings"].as_array().unwrap();
            for f in findings.iter().filter(|f| f["severity"] == "warning") {
                assert!(top.contains(f), "{quotes} {min} {f}");
            }
            let expected = if sev("critical") > 0 {
                "breaches"
            } else if sev("warning") > 0 {
                "warnings"
            } else {
                "ok"
            };
            assert_eq!(v["policyStatus"], expected, "{quotes} {min}");
            assert_eq!(
                v["status"],
                if expected == "breaches" {
                    "breach"
                } else {
                    expected
                }
            );
            let data = if quotes == "quotes_example.json" {
                "complete"
            } else {
                "partial"
            };
            assert_eq!(v["dataStatus"], data, "{quotes}");
        }
    }
}

/// Finance retest A6: data-quality warnings alone leave policyStatus ok.
#[tokio::test]
async fn data_quality_does_not_drive_policy_status() {
    let (_t, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "check_policy", json!({"minSeverity": "critical"})).await;
    assert!(
        codes(&v["warnings"]).is_empty(),
        "minSeverity filters warnings"
    );
    let v = ok(&client, "check_policy", json!({})).await;
    assert!(codes(&v["warnings"]).contains(&"no_live_price".to_string()));
    assert!(!codes(&v["findings"]).contains(&"no_live_price".to_string()));
    let policy_warnings = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "warning")
        .count() as u64;
    assert_eq!(v["counts"]["warning"].as_u64().unwrap(), policy_warnings);
    client.cancel().await.unwrap();
}

/// Finance retest B4: cached prices are complete but `stale`: a
/// stale_prices finding, dataStatus stale, and a marker in context.md.
#[tokio::test]
async fn stale_cache_prices_drive_data_status() {
    let (_tmp, data, policy) = seeded_cache("quotes_example.json").await;
    // Answer 3: a fresh cached price is informational only.
    let client = connect(McpOptions {
        file: Some(data.clone()),
        policy: Some(policy.clone()),
        quotes: Some(fixture_without(&["TLT"])),
        cache: CacheMode::On,
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(by_ticker(&v["positions"], "TLT")["priceSource"], "cache");
    assert_eq!(v["hasStale"], false);
    assert_eq!(v["dataStatus"], "complete");
    client.cancel().await.unwrap();
    // Older than --stale-after: stale.
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["TLT"])),
        cache: CacheMode::On,
        stale_after_days: Some(0),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["complete"], true, "{}", v["dataQuality"]);
    assert_eq!(v["hasStale"], true);
    assert_eq!(v["dataStatus"], "stale");
    assert!(
        codes(&v["warnings"]).contains(&"stale_prices".to_string()),
        "{v}"
    );
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["dataStatus"], "stale");
    assert!(!codes(&v["findings"]).contains(&"stale_prices".to_string()));
    let v = ok(&client, "get_context", json!({ "format": "json" })).await;
    assert_eq!(v["context"]["summary"]["dataStatus"], "stale");
    let v = ok(&client, "get_context", json!({})).await;
    let md = v["markdown"].as_str().unwrap();
    assert!(md.contains("(stale)"), "{md}");
    assert!(md.contains("Data Status: stale"), "{md}");
    client.cancel().await.unwrap();
}

/// Finance retest L5: copy example_data.json into a temp dir and fill its
/// quote cache from a fixture (CacheMode::On forces the cache for fixtures).
/// Returns the temp dir, the data file and a policy file.
async fn seeded_cache(seed_fixture: &str) -> (TempDir, String, String) {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("positions.json");
    std::fs::copy(example_data(), &data).unwrap();
    let policy = tmp.path().join("policy_lower.toml");
    std::fs::write(&policy, POLICY_LOWER).unwrap();
    let data = data.to_string_lossy().into_owned();
    let seed = McpOptions {
        file: Some(data.clone()),
        quotes: Some(fixture_named(seed_fixture)),
        cache: CacheMode::On,
        ..opts()
    };
    let _ = load(&seed).await.expect("seed load");
    let cache = tmp.path().join(".positions.json.quotes-cache.json");
    assert!(cache.is_file(), "cache seeded");
    (tmp, data, policy.to_string_lossy().into_owned())
}

/// Finance retest L2: a position whose native price is shown but whose FX
/// rate is missing or expired reports where that price came from: the
/// fixture (not stale) or the cache (stale), never `none`.
#[tokio::test]
async fn price_shown_without_fx_reports_its_source() {
    // Live (fixture) price, no FX.
    let (_t, client) = connect_partial("quotes_no_fx.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    let spy = by_ticker(&v["positions"], "SPY");
    assert_eq!(spy["priceStatus"], "fx_unavailable", "{spy}");
    assert_eq!(spy["priceSource"], "fixture", "{spy}");
    assert_eq!(spy["isStale"], false, "{spy}");
    client.cancel().await.unwrap();

    // Cached price, FX rate expired (dropped from the cache and the fixture).
    let (tmp, data, policy) = seeded_cache("quotes_example.json").await;
    let cache = tmp.path().join(".positions.json.quotes-cache.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    c["fx"] = json!({});
    std::fs::write(&cache, c.to_string()).unwrap();
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["TLT", "EURUSD=X"])),
        cache: CacheMode::On,
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    let tlt = by_ticker(&v["positions"], "TLT");
    assert_eq!(tlt["priceStatus"], "fx_unavailable", "{tlt}");
    assert_eq!(tlt["price"], 90.0, "{tlt}");
    assert_eq!(tlt["priceSource"], "cache", "{tlt}");
    // Answer 3: a fresh cached price is informational, not stale.
    assert_eq!(tlt["isStale"], false, "{tlt}");
    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    let row = ctx["context"]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ticker"] == "TLT")
        .unwrap()
        .clone();
    assert_eq!(row["isStale"], false, "{row}");
    client.cancel().await.unwrap();
}

/// Finance repro t21 (fake data): BTC is ~86% of the portfolio and only the
/// small GSG position is unpriced.
const BIG_POSITIONS: &str = r#"[
 {"Amount": 10, "AssetClass": "Crypto", "Name": "Bitcoin", "Purchases": [{"Date": "2025/05/28", "Quantity": 10}], "Ticker": "BTC-USD"},
 {"Amount": 50000, "AssetClass": "Cash", "Name": "Cash", "Purchases": [{"Date": "2025/07/10", "Quantity": 50000}]},
 {"Amount": 50, "AssetClass": "Stocks", "Name": "Stocks", "Purchases": [{"Date": "2025/04/15", "Quantity": 37}, {"Date": "2025/07/03", "Quantity": 13}], "Ticker": "SPY"},
 {"Amount": 55, "AssetClass": "Bonds", "Name": "Bonds", "Purchases": [{"Date": "2025/05/10", "Quantity": 55}], "Ticker": "TLT"},
 {"Amount": 10, "AssetClass": "Gold", "Name": "Gold", "Purchases": [{"Date": "2025/06/18", "Quantity": 10.0}], "Ticker": "GLD"},
 {"Amount": 3, "AssetClass": "Commodities", "Name": "Diversified Commodities", "Purchases": [{"Date": "2025/03/22", "Quantity": 3.0}], "Ticker": "GSG"GSG_EXTRA}
]"#;

const BIG_POLICY: &str = r#"version = "1.0"
name = "Fake"
base_currency = "EUR"
time_horizon_years = 15
risk_profile = "moderate"
[constraints]
single_position_limit_percent = 25.0
asset_class_limit_percent = 60.0
[[allocations]]
asset_class = "Stocks"
target_percent = 55.0
tolerance_percent = 10.0
[[allocations]]
asset_class = "Bonds"
target_percent = 25.0
tolerance_percent = 5.0
[[allocations]]
asset_class = "Cash"
target_percent = 10.0
tolerance_percent = 5.0
[[allocations]]
asset_class = "Crypto"
target_percent = 5.0
tolerance_percent = 3.0
[[allocations]]
asset_class = "Gold"
target_percent = 5.0
tolerance_percent = 3.0
"#;

fn big_files(gsg_extra: &str) -> (TempDir, String, String) {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("positions.json");
    std::fs::write(&data, BIG_POSITIONS.replace("GSG_EXTRA", gsg_extra)).unwrap();
    let policy = tmp.path().join("policy.toml");
    std::fs::write(&policy, BIG_POLICY).unwrap();
    (
        tmp,
        data.to_string_lossy().into_owned(),
        policy.to_string_lossy().into_owned(),
    )
}

fn finding<'a>(v: &'a Value, list: &str, code: &str, class: &str) -> Option<&'a Value> {
    v[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == code && f["assetClass"] == class)
}

/// Finance N1/N3: an unpriced position with no currency anywhere is sized at
/// cost basis in the base currency (flagged), so a tiny unpriced GSG cannot
/// hide BTC's certain breaches.
#[tokio::test]
async fn tiny_unpriced_position_keeps_certain_breaches_critical() {
    let (_tmp, data, policy) = big_files("");
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["GSG"])),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(v["complete"], false);
    assert_eq!(v["excludedValueBasis"], "cost_basis_assumed_base", "{v}");
    let v = ok(&client, "check_policy", json!({})).await;
    for (code, class) in [
        ("allocation_out_of_band", "Crypto"),
        ("single_position_limit", "Crypto"),
        ("asset_class_limit", "Crypto"),
    ] {
        let f = finding(&v, "breaches", code, class).unwrap_or_else(|| panic!("{code}: {v}"));
        assert_eq!(f["severity"], "critical", "{f}");
    }
    // N4: GSG is far below 0.5% of the total, so no possible_breach at all.
    assert!(
        !codes(&v["findings"]).contains(&"possible_breach".to_string()),
        "{v}"
    );
    assert!(!codes(&v["findings"]).contains(&"checks_incomplete".to_string()));
    assert!(codes(&v["warnings"]).contains(&"checks_incomplete".to_string()));
    client.cancel().await.unwrap();
}

/// Finance N3: a recorded `Currency` converts the cost basis.
#[tokio::test]
async fn recorded_currency_converts_cost_basis_of_unpriced_position() {
    let (_tmp, data, policy) = big_files(r#", "Currency": "USD""#);
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["GSG"])),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(v["excludedValueBasis"], "cost_basis", "{v}");
    client.cancel().await.unwrap();
}

/// Finance retest N4: when only unpriced positions need an FX pair, the
/// cost-basis share still converts at the current FX rate (flagged).
#[tokio::test]
async fn cost_basis_share_falls_back_to_current_fx() {
    let (_tmp, data, policy) = big_files(r#", "Currency": "USD""#);
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["BTC-USD", "SPY", "TLT", "GLD", "GSG"])),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert!(v["excludedValueShare"].is_number(), "{v}");
    let f = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "cost_basis_current_fx")
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(
        f["detail"]["estimatePositionIds"].as_array().unwrap().len(),
        1,
        "{f}"
    );
    client.cancel().await.unwrap();
}

/// Finance N4: with the materiality threshold off, possible breaches appear,
/// at most one per class and limit, with the priced-only percent.
#[tokio::test]
async fn possible_breaches_are_one_per_class_and_limit() {
    let (_tmp, data, policy) = big_files("");
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["GSG", "TLT"])),
        possible_breach_min_share: Some(0.0),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    let mut seen = std::collections::HashSet::new();
    for f in v["warnings"].as_array().unwrap() {
        if f["code"] != "possible_breach" {
            continue;
        }
        let key = (
            f["detail"]["code"].as_str().unwrap_or("band").to_string(),
            f["assetClass"].as_str().unwrap_or("").to_string(),
        );
        assert!(seen.insert(key), "duplicate possible_breach {f}");
        assert!(
            f["message"]
                .as_str()
                .unwrap()
                .contains("of the priced part"),
            "{f}"
        );
    }
    client.cancel().await.unwrap();
}

/// Finance A1/A2: an expired cache entry only estimates the excluded share;
/// a 0.01% GSG must not cap BTC's 86% concentration at warning.
#[tokio::test]
async fn expired_cache_estimate_is_no_bound_and_tiny_share_keeps_critical() {
    let (tmp, data, policy) = big_files("");
    let seed = McpOptions {
        file: Some(data.clone()),
        quotes: Some(fixture()),
        cache: CacheMode::On,
        ..opts()
    };
    let _ = load(&seed).await.expect("seed load");
    assert!(tmp
        .path()
        .join(".positions.json.quotes-cache.json")
        .is_file());
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["GSG"])),
        cache: CacheMode::On,
        cache_max_age_days: Some(0),
        ..opts()
    })
    .await;
    let v = ok(&client, "check_policy", json!({})).await;
    for code in [
        "allocation_out_of_band",
        "single_position_limit",
        "asset_class_limit",
    ] {
        let f = finding(&v, "breaches", code, "Crypto").unwrap_or_else(|| panic!("{code}: {v}"));
        assert_eq!(f["severity"], "critical", "{f}");
        assert!(f["detail"].get("severityCappedFrom").is_none(), "{f}");
    }
    assert!(!codes(&v["findings"]).contains(&"possible_breach".to_string()));
    let note = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "checks_incomplete")
        .expect("note");
    assert_eq!(note["detail"]["basis"], "priced_part", "{note}");
    client.cancel().await.unwrap();
}

fn low_btc_files(currency: &str) -> (TempDir, String, String) {
    let (tmp, data, policy) = big_files("");
    let s = std::fs::read_to_string(&data).unwrap().replace(
        r#"{"Amount": 10, "AssetClass": "Crypto", "Name": "Bitcoin", "Purchases": [{"Date": "2025/05/28", "Quantity": 10}], "Ticker": "BTC-USD"}"#,
        &format!(r#"{{"Amount": 0.45, "AssetClass": "Crypto", "Name": "Bitcoin", "Purchases": [{{"Date": "2025/05/28", "Quantity": 0.45, "Price": 1000.0}}], "Ticker": "BTC-USD"{currency}}}"#),
    );
    assert!(s.contains("1000.0"));
    std::fs::write(&data, s).unwrap();
    (tmp, data, policy)
}

/// Finance A1 (t21 L1): a cost basis far below the market value is no
/// bound. With a material estimated share the unpriced BTC is [0, inf):
/// only certain breaches are critical, the rest is one possible_breach per
/// class and limit.
#[tokio::test]
async fn cost_basis_is_no_bound_for_a_material_unpriced_position() {
    let (_tmp, data, policy) = low_btc_files("");
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["BTC-USD"])),
        ..opts()
    })
    .await;
    let a = ok(&client, "get_allocation", json!({})).await;
    let share = a["excludedValueShare"].as_f64().unwrap();
    assert!(share > 0.5, "{a}");
    let v = ok(&client, "check_policy", json!({})).await;
    // Cash 66% of the priced part could be 51% (truth) or less: not certain.
    assert!(
        finding(&v, "breaches", "asset_class_limit", "Cash").is_none(),
        "{v}"
    );
    assert!(
        finding(&v, "breaches", "allocation_out_of_band", "Cash").is_none(),
        "{v}"
    );
    // Stocks is below its band whatever BTC is worth.
    let f = finding(&v, "breaches", "allocation_out_of_band", "Stocks").expect("certain");
    assert_eq!(f["severity"], "critical");
    let mut seen = std::collections::HashSet::new();
    for f in v["warnings"].as_array().unwrap() {
        if f["code"] == "possible_breach" {
            let key = (f["detail"]["code"].to_string(), f["assetClass"].to_string());
            assert!(seen.insert(key), "duplicate {f}");
            assert!(f["message"]
                .as_str()
                .unwrap()
                .contains("of the priced part"));
        }
    }
    assert!(seen.len() >= 2, "{v}");
    client.cancel().await.unwrap();
}

/// Finance L2 (t24): a crypto cost-basis estimate is never immaterial, however
/// small: BTC falls through to rule (c). Only the true breaches (Stocks and
/// Bonds below their bands) are critical; the rest is possible_breach.
#[tokio::test]
async fn crypto_cost_basis_is_never_immaterial() {
    let (_tmp, data, policy) = low_btc_files(r#", "Currency": "USD""#);
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["BTC-USD"])),
        ..opts()
    })
    .await;
    let a = ok(&client, "get_allocation", json!({})).await;
    assert!(a["excludedValueShare"].as_f64().unwrap() < 0.5, "{a}");
    let v = ok(&client, "check_policy", json!({})).await;
    let mut criticals: Vec<(String, String)> = v["breaches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "critical")
        .map(|f| {
            (
                f["code"].as_str().unwrap().to_string(),
                f["assetClass"].to_string(),
            )
        })
        .collect();
    criticals.sort();
    assert_eq!(
        criticals,
        vec![
            (
                "allocation_out_of_band".to_string(),
                "\"Bonds\"".to_string()
            ),
            (
                "allocation_out_of_band".to_string(),
                "\"Stocks\"".to_string()
            ),
        ],
        "{v}"
    );
    assert!(has_code(&v, "possible_breach").is_some(), "{v}");
    let note = has_code(&v, "checks_incomplete").expect("note");
    assert_ne!(note["detail"]["basis"], "priced_part", "{note}");
    client.cancel().await.unwrap();
}

/// Finance L2 (t25 OK-nogsg): a commodities cost-basis estimate of 0.08% ×
/// the default 3× factor = 0.24% stays immaterial, and the note names the
/// factor. With the factor configured to `never` it becomes material.
const OK_POSITIONS: &str = r#"[{"Amount": 0.45, "AssetClass": "Crypto", "Name": "Bitcoin", "Purchases": [{"Date": "2025/05/28", "Quantity": 0.45}], "Ticker": "BTC-USD"}, {"Amount": 50000, "AssetClass": "Cash", "Name": "Cash", "Purchases": [{"Date": "2025/07/10", "Quantity": 50000}]}, {"Amount": 50, "AssetClass": "Stocks", "Name": "Stocks", "Purchases": [{"Date": "2025/04/15", "Quantity": 37}, {"Date": "2025/07/03", "Quantity": 13}], "Ticker": "SPY"}, {"Amount": 55, "AssetClass": "Bonds", "Name": "Bonds", "Purchases": [{"Date": "2025/05/10", "Quantity": 55}], "Ticker": "TLT"}, {"Amount": 10, "AssetClass": "Gold", "Name": "Gold", "Purchases": [{"Date": "2025/06/18", "Quantity": 10.0}], "Ticker": "GLD"}, {"Amount": 3, "AssetClass": "Commodities", "Name": "Diversified Commodities", "Purchases": [{"Date": "2025/03/22", "Quantity": 3.0}], "Ticker": "GSG"}]"#;

const OK_POLICY: &str = r#"version = "1.0"
name = "Fake OK"
base_currency = "EUR"
time_horizon_years = 15
risk_profile = "moderate"
[constraints]
minimum_cash_amount = 30000.0
single_position_limit_percent = 60.0
asset_class_limit_percent = 60.0
[[allocations]]
asset_class = "Stocks"
target_percent = 20.0
tolerance_percent = 10.0
[[allocations]]
asset_class = "Bonds"
target_percent = 5.0
tolerance_percent = 5.0
[[allocations]]
asset_class = "Cash"
target_percent = 50.0
tolerance_percent = 9.0
[[allocations]]
asset_class = "Crypto"
target_percent = 22.0
tolerance_percent = 10.0
[[allocations]]
asset_class = "Gold"
target_percent = 2.0
tolerance_percent = 2.0
[[allocations]]
asset_class = "Commodities"
target_percent = 1.0
tolerance_percent = 1.0
"#;

async fn connect_ok_without_gsg(gain_factors: Vec<(String, Option<f64>)>) -> (TempDir, Client) {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("positions.json");
    let policy = tmp.path().join("policy.toml");
    std::fs::write(&data, OK_POSITIONS).unwrap();
    std::fs::write(&policy, OK_POLICY).unwrap();
    let client = connect(McpOptions {
        file: Some(data.to_string_lossy().into_owned()),
        policy: Some(policy.to_string_lossy().into_owned()),
        quotes: Some(fixture_without(&["GSG"])),
        gain_factors,
        ..opts()
    })
    .await;
    (tmp, client)
}

#[tokio::test]
async fn commodity_cost_basis_uses_its_gain_factor() {
    let (_t, client) = connect_ok_without_gsg(vec![]).await;
    let v = ok(&client, "check_policy", json!({})).await;
    assert_eq!(v["policyStatus"], "ok", "{v}");
    let note = has_code(&v, "checks_incomplete").expect("note");
    assert_eq!(note["detail"]["basis"], "priced_part", "{note}");
    assert_eq!(note["detail"]["gainFactors"]["Commodities"], 3.0, "{note}");
    let m = note["detail"]["materialityShare"].as_f64().unwrap();
    let e = note["detail"]["excludedValueShare"].as_f64().unwrap();
    assert!((m - 3.0 * e).abs() < 0.01 && m < 0.5, "{note}");
    assert!(
        note["message"].as_str().unwrap().contains("3x Commodities"),
        "{note}"
    );
    client.cancel().await.unwrap();

    let (_t2, client) = connect_ok_without_gsg(vec![("commodities".into(), None)]).await;
    let v = ok(&client, "check_policy", json!({})).await;
    let note = has_code(&v, "checks_incomplete").expect("note");
    assert_ne!(note["detail"]["basis"], "priced_part", "{note}");
    client.cancel().await.unwrap();
}

/// Finance N5 and gaps: get_positions and get_context list positions in the
/// same order on partial data; list_portfolios carries dataStatus.
#[tokio::test]
async fn partial_data_order_matches_and_list_portfolios_has_data_status() {
    let (_t, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    let names: Vec<Value> = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].clone())
        .collect();
    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    let ctx_names: Vec<Value> = ctx["context"]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].clone())
        .collect();
    assert_eq!(names, ctx_names);
    let l = ok(&client, "list_portfolios", json!({})).await;
    assert_eq!(l["portfolios"][0]["dataStatus"], "partial", "{l}");
    client.cancel().await.unwrap();
}

/// Finance L4: untargetedClasses lists classes in `classes` order.
fn assert_untargeted_in_class_order(v: &Value) {
    let untargeted: Vec<&Value> = v["untargetedClasses"].as_array().unwrap().iter().collect();
    let in_order: Vec<&Value> = v["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["assetClass"])
        .filter(|c| untargeted.contains(c))
        .collect();
    assert_eq!(untargeted, in_order, "{v}");
}

/// A fixture without the given symbols' latest quotes.
fn fixture_without(symbols: &[&str]) -> AnyProvider {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quotes_example.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for s in symbols {
        v["latest"].as_object_mut().unwrap().remove(*s);
    }
    AnyProvider::Fixture(Box::new(
        FixtureProvider::from_json(&v.to_string()).unwrap(),
    ))
}

/// Finance retest B1/B2/A4/A5: with only BTC unpriced (its cached price is
/// past the max age), the excluded share comes from the last cached price,
/// statuses hold for the whole possible range and no amountToTarget is given.
#[tokio::test]
async fn partial_allocation_uses_last_price_and_certain_breaches_only() {
    let (_tmp, data, policy) = seeded_cache("quotes_example.json").await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let client = connect(McpOptions {
        file: Some(data),
        policy: Some(policy),
        quotes: Some(fixture_without(&["BTC-USD"])),
        cache: CacheMode::On,
        cache_max_age_days: Some(0),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(v["complete"], false);
    assert_eq!(v["excludedValueBasis"], "last_price", "{v}");
    let share = v["excludedValueShare"].as_f64().unwrap();
    assert!(share > 0.0 && share < 100.0, "{share}");
    for c in v["classes"].as_array().unwrap() {
        assert!(c["actualPercent"].is_null(), "{c}");
        assert!(c["amountToTarget"].is_null(), "{c}");
        assert!(c["driftPoints"].is_null(), "{c}");
        let (min, max) = (
            c["actualPercentMin"].as_f64().unwrap(),
            c["actualPercentMax"].as_f64().unwrap(),
        );
        assert!(min <= max, "{c}");
        if c["targetPercent"].is_null() {
            continue;
        }
        let (low, high) = (
            c["bandLow"].as_f64().unwrap(),
            c["bandHigh"].as_f64().unwrap(),
        );
        match c["status"].as_str().unwrap() {
            "above_band" => assert!(min > high, "{c}"),
            "below_band" => assert!(max < low, "{c}"),
            "within_band" => assert!(min >= low && max <= high, "{c}"),
            "unknown" => {}
            s => panic!("unexpected status {s}"),
        }
    }
    let v = ok(&client, "check_policy", json!({})).await;
    for b in v["breaches"].as_array().unwrap() {
        if b["code"] == "allocation_out_of_band" {
            assert_eq!(b["detail"]["basis"], "partial", "{b}");
        }
    }
    // Answer 1: checks_incomplete is data quality (top-level warnings only).
    assert!(!codes(&v["findings"]).contains(&"checks_incomplete".to_string()));
    let inc = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "checks_incomplete")
        .unwrap();
    assert_eq!(inc["category"], "data_quality");
    assert_eq!(inc["detail"]["excludedValueBasis"], "last_price");
    client.cancel().await.unwrap();
}

// ---- Phase 2: snapshots and get_performance (fake data only) ----

fn day(offset: i64) -> String {
    (chrono::Local::now().date_naive() + chrono::Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// Record today's snapshot of example_data into `dir` and return it parsed.
async fn seed_today(dir: &Path) -> (PathBuf, Value) {
    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        data_dir: Some(dir.to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    (
        path.clone(),
        serde_json::from_str(text.lines().next().unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn snapshot_command_writes_private_jsonl_once_per_day() {
    let tmp = TempDir::new().unwrap();
    let (path, snap) = seed_today(tmp.path()).await;
    assert_eq!(path, tmp.path().join("main/snapshots.jsonl"));
    assert_eq!(snap["date"], day(0));
    assert_eq!(snap["complete"], true, "{snap}");
    assert!(snap["totalBase"].as_f64().unwrap() > 0.0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A second run the same day replaces the line.
    let (_, again) = seed_today(tmp.path()).await;
    assert_eq!(again["date"], day(0));
}

/// A seeded history: 8 days ago every market position was worth 80% of
/// today, cash and manual values unchanged. With no purchases in the
/// window, the whole change is market effect, and the identity holds.
#[tokio::test]
async fn performance_from_seeded_snapshots_splits_change() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_today(tmp.path()).await;
    let mut old = today.clone();
    old["date"] = json!(day(-8));
    let mut total = 0.0;
    for p in old["positions"].as_array_mut().unwrap() {
        if p["kind"] == "market" {
            let v = p["valueBase"].as_f64().unwrap() * 0.8;
            p["valueBase"] = json!((v * 100.0).round() / 100.0);
        }
        total += p["valueBase"].as_f64().unwrap();
    }
    old["totalBase"] = json!(total);
    std::fs::write(&path, format!("{old}\n")).unwrap();

    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "week", "groupBy": "assetClass"}),
    )
    .await;
    assert_eq!(v["dataStatus"], "complete", "{v}");
    // Reported `from` is the base snapshot, not the requested week start.
    assert_eq!(v["from"], day(-8));
    assert_eq!(v["dataCoverage"]["startPointDate"], day(-8));
    assert_eq!(v["dataCoverage"]["endIsLive"], true);
    let f = |k: &str| v[k].as_f64().unwrap_or_else(|| panic!("{k} null: {v}"));
    let identity =
        f("netContributions") + f("marketEffect") + f("incomeEffect") + f("manualRevaluation");
    assert!((f("change") - identity).abs() <= 0.01, "{v}");
    assert!(
        (f("change") - (f("endValue") - f("startValue"))).abs() <= 0.01,
        "{v}"
    );
    assert_eq!(f("netContributions"), 0.0, "{v}");
    assert!(f("marketEffect") > 0.0);
    assert_eq!(v["method"], "modified_dietz");
    // Breakdown sums to the totals.
    let sum: f64 = v["breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["marketEffect"].as_f64().unwrap())
        .sum();
    assert!((sum - f("marketEffect")).abs() <= 0.05, "{v}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn performance_without_history_or_with_partial_data_is_null() {
    let tmp = TempDir::new().unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "ytd"})).await;
    assert!(v["change"].is_null(), "{v}");
    assert!(v["startValue"].is_null(), "{v}");
    assert_eq!(v["method"], "none", "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert_eq!(
        err(
            &client,
            "get_performance",
            json!({"period": "custom", "from": day(-3)})
        )
        .await,
        "invalid_argument"
    );
    assert_eq!(
        err(
            &client,
            "get_performance",
            json!({"period": "week", "from": day(-3)})
        )
        .await,
        "invalid_argument"
    );
    client.cancel().await.unwrap();

    // A seeded start, but today's valuation is missing a quote (GSG).
    let (path, today) = seed_today(tmp.path()).await;
    let mut old = today.clone();
    old["date"] = json!(day(-2));
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let policy = tmp.path().join("policy_lower.toml");
    std::fs::write(&policy, POLICY_LOWER).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        quotes: Some(fixture_named("quotes_no_gsg.json")),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(v["dataStatus"], "partial", "{v}");
    assert_eq!(v["complete"], false);
    assert!(v["change"].is_null() && v["marketEffect"].is_null(), "{v}");
    assert!(has_code(&v, "performance_partial").is_some(), "{v}");
    client.cancel().await.unwrap();
}

fn has_code<'a>(v: &'a Value, code: &str) -> Option<&'a Value> {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == code)
}

/// Write `snaps` (today's snapshot re-dated) as the history in `dir`.
async fn seed_history(dir: &Path, offsets: &[i64]) -> (PathBuf, Value) {
    let (path, today) = seed_today(dir).await;
    let mut text = String::new();
    for o in offsets {
        let mut s = today.clone();
        s["date"] = json!(day(*o));
        text.push_str(&format!("{s}\n"));
    }
    std::fs::write(&path, text).unwrap();
    (path, today)
}

/// Finance B1: the start snapshot has a maximum age per period; a past end
/// must lie inside the window; a "day" spanning more days gets a note.
#[tokio::test]
async fn performance_start_point_has_a_maximum_age() {
    let tmp = TempDir::new().unwrap();
    seed_history(tmp.path(), &[-20]).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "week"})).await;
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert!(v["change"].is_null(), "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
    assert!(v["endValue"].is_number(), "live end kept: {v}");
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "week", "maxStartAgeDays": 20}),
    )
    .await;
    assert!(v["change"].is_number(), "{v}");
    assert_eq!(v["from"], day(-20), "{v}");
    assert_eq!(v["dataCoverage"]["startPointDate"], day(-20), "{v}");
    assert!(has_code(&v, "start_point_gap").is_some(), "{v}");
    // Custom range after the only snapshot: never start = end from before.
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "custom", "from": day(-5), "to": day(-3), "maxStartAgeDays": 100}),
    )
    .await;
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert!(v["startValue"].is_null(), "{v}");
    client.cancel().await.unwrap();

    let tmp = TempDir::new().unwrap();
    seed_history(tmp.path(), &[-3]).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert!(v["change"].is_number(), "{v}");
    let gap = has_code(&v, "start_point_gap").expect("note");
    assert_eq!(gap["detail"]["gapDays"], 2, "{gap}");
    client.cancel().await.unwrap();
}

/// Finance B2 (run6 S1): a position removed from the data file without a
/// sell entry is an unknown external flow, not a loss; figures are partial.
#[tokio::test]
async fn vanished_position_is_a_flow_not_a_loss() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_today(tmp.path()).await;
    let mut old = today.clone();
    old["date"] = json!(day(-2));
    old["positions"].as_array_mut().unwrap().push(json!({
        "positionId": "pos_gone", "kind": "market", "assetClass": "Crypto",
        "quantity": 1.0, "price": 1000.0, "priceCurrency": "EUR", "fxRate": null, "valueBase": 1000.0
    }));
    old["totalBase"] = json!(old["totalBase"].as_f64().unwrap() + 1000.0);
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    let w = has_code(&v, "vanished_position").expect("flag");
    assert_eq!(w["detail"]["positionIds"][0], "pos_gone", "{w}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "partial", "{v}");
    let f = |k: &str| v[k].as_f64().unwrap_or_else(|| panic!("{k}: {v}"));
    assert!((f("netContributions") + 1000.0).abs() < 0.01, "{v}");
    assert!(f("marketEffect").abs() < 0.01, "{v}");
    client.cancel().await.unwrap();
}

/// Finance B5: strict dates; skipped lines warn; Modified Dietz is flagged.
#[tokio::test]
async fn performance_dates_are_strict_and_fallbacks_warn() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_history(tmp.path(), &[-3, -1]).await;
    let mut loose = today.clone();
    loose["date"] = json!(format!("{}T00:00:00", day(-2)));
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{loose}\n{{broken\n"));
    std::fs::write(&path, text).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let text = err(
        &client,
        "get_performance",
        json!({"period": "custom", "from": format!("{}T00:00:00", day(-2)), "to": day(0)}),
    )
    .await;
    assert_eq!(text, "invalid_argument");
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "custom", "from": day(-2), "to": day(0)}),
    )
    .await;
    assert_eq!(v["dataCoverage"]["skippedLines"], 2, "{v}");
    let w = has_code(&v, "snapshot_lines_skipped").expect("skipped warning");
    assert_eq!(w["severity"], "warning", "{w}");
    // -3 → -1 → live today is not a daily chain: Modified Dietz, flagged.
    assert_eq!(v["method"], "modified_dietz", "{v}");
    assert!(has_code(&v, "performance_approximate").is_some(), "{v}");
    client.cancel().await.unwrap();
}

/// Finance B7: a baseCurrency change between snapshots warns and is partial.
#[tokio::test]
async fn base_currency_change_makes_performance_partial() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_today(tmp.path()).await;
    let mut old = today.clone();
    old["date"] = json!(day(-1));
    old["baseCurrency"] = json!("USD");
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    let w = has_code(&v, "base_currency_changed").expect("warning");
    assert_eq!(w["detail"]["baseCurrencies"], json!(["USD", "EUR"]), "{w}");
    assert!(v["change"].is_null(), "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "partial", "{v}");
    client.cancel().await.unwrap();
}

/// Finance B3 (run6 S3): a position missing from the start snapshot whose
/// only buy predates the window is an unknown flow, not a gain.
#[tokio::test]
async fn late_recorded_position_is_a_flow_not_a_gain() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_today(tmp.path()).await;
    let mut old = today.clone();
    old["date"] = json!(day(-2));
    let positions = old["positions"].as_array_mut().unwrap();
    let i = positions
        .iter()
        .position(|p| p["kind"] == "market" && p["valueBase"].as_f64().is_some_and(|v| v > 0.0))
        .expect("a market position");
    let gone = positions.remove(i);
    let value = gone["valueBase"].as_f64().unwrap();
    old["totalBase"] = json!(old["totalBase"].as_f64().unwrap() - value);
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    let w = has_code(&v, "untracked_position").expect("flag");
    assert_eq!(w["detail"]["positionIds"][0], gone["positionId"], "{w}");
    assert_eq!(v["dataStatus"], "partial", "{v}");
    let f = |k: &str| v[k].as_f64().unwrap_or_else(|| panic!("{k}: {v}"));
    assert!((f("netContributions") - value).abs() < 0.01, "{v}");
    assert!(f("marketEffect").abs() < 0.01, "{v}");
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "day", "groupBy": "position"}),
    )
    .await;
    let rows = v["breakdown"].as_array().unwrap();
    assert!(!rows.is_empty(), "{v}");
    assert!(rows.iter().all(|r| r["name"].is_string()), "{v}");
    assert!(rows.iter().any(|r| r["ticker"].is_string()), "{v}");
    client.cancel().await.unwrap();
}

/// Finance B8 (run6 S4): snapshots record their own flows, and a purchase
/// list edited afterwards is flagged instead of silently moving the figures.
#[tokio::test]
async fn edited_ledger_contradicting_snapshots_warns() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_today(tmp.path()).await;
    let recorded = today["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "market")
        .expect("market position")
        .clone();
    assert!(recorded["ledgerBase"].is_number(), "{recorded}");
    assert!(today["positions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["kind"] != "cash" || p.get("ledgerBase").is_none()));
    let mut old = today.clone();
    old["date"] = json!(day(-1));
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert!(has_code(&v, "ledger_changed").is_none(), "{v}");
    client.cancel().await.unwrap();

    // The purchase list said something else when the snapshot was taken.
    for p in old["positions"].as_array_mut().unwrap() {
        if p["positionId"] == recorded["positionId"] {
            p["ledgerBase"] = json!(recorded["ledgerBase"].as_f64().unwrap() + 240.0);
        }
    }
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    let w = has_code(&v, "ledger_changed").expect("flag");
    assert_eq!(w["detail"]["positionIds"][0], recorded["positionId"], "{w}");
    assert_eq!(w["detail"]["snapshotDates"][0], day(-1), "{w}");
    assert_eq!(v["dataStatus"], "partial", "{v}");
    client.cancel().await.unwrap();
}

/// Finance LOW: a snapshot's cash quantity is the balance (`Amount`).
#[tokio::test]
async fn snapshot_cash_quantity_is_the_balance() {
    let tmp = TempDir::new().unwrap();
    let (_, today) = seed_today(tmp.path()).await;
    let cash: Vec<&Value> = today["positions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] == "cash")
        .collect();
    assert!(!cash.is_empty(), "{today}");
    for p in cash {
        let fx = p["fxRate"].as_f64().unwrap_or(1.0);
        let q = p["quantity"].as_f64().unwrap();
        let v = p["valueBase"].as_f64().unwrap();
        assert!((q * fx - v).abs() < 0.01, "{p}");
    }
}

/// Finance N1: a sell without a price is valued at the historic close like a
/// buy, with a performance_approximate note and no spurious ledger_changed.
#[tokio::test]
async fn sell_without_price_uses_historic_close() {
    priceless_tx_uses_historic_close(0.4, -0.05, "sell").await;
}

/// Finance S6: a ticker add-on buy without a price is valued the same way.
#[tokio::test]
async fn ticker_add_on_buy_without_price_uses_historic_close() {
    priceless_tx_uses_historic_close(0.5, 0.05, "buy").await;
}

async fn priceless_tx_uses_historic_close(amount: f64, qty: f64, ty: &str) {
    let tmp = TempDir::new().unwrap();
    let (path, original) = seed_today(tmp.path()).await;
    let sell_day = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let data = tmp.path().join("positions.json");
    let mut positions: Value =
        serde_json::from_str(&std::fs::read_to_string(example_data()).unwrap()).unwrap();
    let btc = positions
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["Ticker"] == "BTC-USD")
        .expect("BTC in example data");
    btc["Amount"] = json!(amount);
    btc["Purchases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"Date": sell_day.format("%Y/%m/%d").to_string(), "Quantity": qty}));
    let text = positions.to_string();
    std::fs::write(&data, text).unwrap();
    let data = data.to_string_lossy().into_owned();
    let (_, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        file: Some(data.clone()),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let mut mid: Value = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    mid["date"] = json!(day(-1));
    let mut start = original;
    start["date"] = json!(day(-7));
    std::fs::write(&path, format!("{start}\n{mid}\n")).unwrap();
    let client = connect(McpOptions {
        file: Some(data),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let t = ok(&client, "get_transactions", json!({"types": [ty]})).await;
    let sell = t["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["date"] == json!(day(-1)))
        .unwrap_or_else(|| panic!("{t}"));
    assert_eq!(sell["priceSource"], "historic_close", "{t}");
    assert!(sell["amount"].is_number(), "{t}");
    let v = ok(&client, "get_performance", json!({"period": "week"})).await;
    assert!(has_code(&v, "ledger_changed").is_none(), "{v}");
    assert!(
        v["change"].is_number() && v["netContributions"].is_number(),
        "{v}"
    );
    let notes: Vec<&Value> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "performance_approximate")
        .collect();
    assert!(
        notes
            .iter()
            .any(|w| w["detail"]["reason"] == "estimated_transaction_price"),
        "{v}"
    );
    // Item 6: per-class contributions split into internal/external/unknown.
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "week", "groupBy": "assetClass"}),
    )
    .await;
    for r in v["breakdown"].as_array().unwrap() {
        let parts: f64 = ["internalTransfers", "externalFlows", "externalUnknown"]
            .iter()
            .map(|k| r[*k].as_f64().unwrap())
            .sum();
        assert!(
            (parts - r["netContributions"].as_f64().unwrap()).abs() < 0.02,
            "{r}"
        );
    }
    client.cancel().await.unwrap();
}

/// A deposit on one snapshot step and a buy from that cash on the next match
/// as internal transfers. Over the whole span both external flows are positive,
/// so a single split would leave the buy external.
#[tokio::test]
async fn stepwise_split_matches_deposit_then_buy_from_cash() {
    let tmp = TempDir::new().unwrap();
    let buy_day = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let data = tmp.path().join("positions.json");
    let positions = json!([
        {
            "Name": "Index",
            "AssetClass": "Stocks",
            "Amount": 3940.0,
            "Purchases": [
                {"Date": "2020/01/02", "Quantity": 1, "Price": 100.0},
                {"Date": buy_day.format("%Y/%m/%d").to_string(), "Quantity": 1, "Price": 3840.0}
            ]
        },
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 6160.0}
    ]);
    std::fs::write(&data, positions.to_string()).unwrap();
    let data = data.to_string_lossy().into_owned();
    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        file: Some(data.clone()),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let today: Value = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let retarget = |date: &str, cash: f64, stock: f64| {
        let mut snap = today.clone();
        snap["date"] = json!(date);
        let mut total = 0.0;
        for p in snap["positions"].as_array_mut().unwrap() {
            p["ledgerBase"] = Value::Null;
            if p["kind"] == "cash" {
                p["valueBase"] = json!(cash);
                p["quantity"] = json!(cash);
            } else if p["assetClass"] == "Stocks" {
                p["valueBase"] = json!(stock);
            }
            total += p["valueBase"].as_f64().unwrap();
        }
        snap["cashBase"] = json!(cash);
        snap["totalBase"] = json!(total);
        snap
    };
    let start = retarget(&day(-7), 0.0, 100.0);
    let mid = retarget(&day(-3), 10_000.0, 100.0);
    std::fs::write(&path, format!("{start}\n{mid}\n")).unwrap();
    let client = connect(McpOptions {
        file: Some(data),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "week", "groupBy": "assetClass"}),
    )
    .await;
    assert!(
        v["change"].is_number() && v["netContributions"].is_number(),
        "{v}"
    );
    // Totals are the deposit, not the buy: the buy is internal.
    assert!(
        (v["netContributions"].as_f64().unwrap() - 10_000.0).abs() < 0.02,
        "{v}"
    );
    assert!(v["marketEffect"].as_f64().unwrap().abs() < 0.02, "{v}");
    assert!(v["manualRevaluation"].as_f64().unwrap().abs() < 0.02, "{v}");
    let row = |key: &str| {
        v["breakdown"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == key)
            .unwrap_or_else(|| panic!("no {key} in {v}"))
            .clone()
    };
    let stocks = row("Stocks");
    let cash = row("Cash");
    assert!(
        (stocks["internalTransfers"].as_f64().unwrap() - 3_840.0).abs() < 0.02,
        "{stocks}"
    );
    assert!(
        stocks["externalFlows"].as_f64().unwrap().abs() < 0.02,
        "{stocks}"
    );
    assert!(
        (cash["internalTransfers"].as_f64().unwrap() + 3_840.0).abs() < 0.02,
        "{cash}"
    );
    assert!(
        (cash["externalFlows"].as_f64().unwrap() - 10_000.0).abs() < 0.02,
        "{cash}"
    );
    let internal: f64 = v["breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["internalTransfers"].as_f64().unwrap())
        .sum();
    assert!(internal.abs() < 0.02, "{v}");
    for r in v["breakdown"].as_array().unwrap() {
        let parts: f64 = ["internalTransfers", "externalFlows", "externalUnknown"]
            .iter()
            .map(|k| r[*k].as_f64().unwrap())
            .sum();
        assert!(
            (parts - r["netContributions"].as_f64().unwrap()).abs() < 0.02,
            "{r}"
        );
    }
    client.cancel().await.unwrap();
}

/// A manual add-on without a price must not null the whole period.
#[tokio::test]
async fn manual_add_on_without_price_does_not_null_performance() {
    let tmp = TempDir::new().unwrap();
    let open = chrono::Local::now().date_naive() - chrono::Duration::days(8);
    let addon = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let data = tmp.path().join("positions.json");
    let positions = json!([{
        "Name": "Apartment",
        "AssetClass": "RealEstate",
        "Amount": 215000,
        "Purchases": [
            {"Date": open.format("%Y/%m/%d").to_string(), "Quantity": 1, "Price": 200000.0},
            {"Date": addon.format("%Y/%m/%d").to_string(), "Quantity": 5000}
        ]
    }]);
    std::fs::write(&data, positions.to_string()).unwrap();
    let data = data.to_string_lossy().into_owned();
    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        file: Some(data.clone()),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let today: Value = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let mut before = today;
    before["date"] = json!(day(-7));
    for p in before["positions"].as_array_mut().unwrap() {
        p["valueBase"] = json!(200000.0);
        p["quantity"] = json!(1.0);
        p["ledgerBase"] = Value::Null;
    }
    before["totalBase"] = json!(200000.0);
    before["cashBase"] = json!(0.0);
    std::fs::write(&path, format!("{before}\n")).unwrap();
    let client = connect(McpOptions {
        file: Some(data),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "week", "groupBy": "assetClass"}),
    )
    .await;
    assert!(
        v["change"].is_number() && v["netContributions"].is_number(),
        "{v}"
    );
    let notes: Vec<&Value> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "performance_approximate")
        .collect();
    assert!(
        notes
            .iter()
            .any(|w| w["detail"]["reason"] == "estimated_manual_value"),
        "{v}"
    );
    for r in v["breakdown"].as_array().unwrap() {
        let parts: f64 = ["internalTransfers", "externalFlows", "externalUnknown"]
            .iter()
            .map(|k| r[*k].as_f64().unwrap())
            .sum();
        assert!(
            (parts - r["netContributions"].as_f64().unwrap()).abs() < 0.02,
            "{r}"
        );
    }
    client.cancel().await.unwrap();
}

/// Finance N2/N3: implausible snapshot lines (a negative valueBase, a
/// totalBase that is not the sum of the positions) are skipped, and every
/// skipped line names its real reason.
#[tokio::test]
async fn implausible_snapshot_lines_are_skipped_with_reasons() {
    let tmp = TempDir::new().unwrap();
    let (path, today) = seed_history(tmp.path(), &[-7]).await;
    let mut null_total = today.clone();
    null_total["date"] = json!(day(-4));
    null_total["totalBase"] = Value::Null;
    let mut negative = today.clone();
    negative["date"] = json!(day(-3));
    negative["positions"][0]["valueBase"] = json!(-1.0);
    let mut mismatch = today.clone();
    mismatch["date"] = json!(day(-2));
    mismatch["totalBase"] = json!(today["totalBase"].as_f64().unwrap() + 100.0);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{null_total}\n{negative}\n{mismatch}\n"));
    std::fs::write(&path, text).unwrap();
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "week"})).await;
    assert_eq!(v["dataCoverage"]["skippedLines"], 3, "{v}");
    assert_eq!(v["dataCoverage"]["startPointDate"], day(-7), "{v}");
    let w = has_code(&v, "snapshot_lines_skipped").expect("skipped warning");
    let reasons: Vec<&str> = w["detail"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        vec!["total_base_null", "negative_value", "total_mismatch"],
        "{w}"
    );
    let msg = w["message"].as_str().unwrap();
    assert!(!msg.contains("bad JSON"), "{msg}");
    assert!(msg.contains("total base null"), "{msg}");
    client.cancel().await.unwrap();
}

/// One snapshot, dated today: since_inception must not invert to tomorrow
/// and then treat that snapshot as the start (a zero-change Dietz).
#[tokio::test]
async fn since_inception_with_only_today_is_insufficient_history() {
    let tmp = TempDir::new().unwrap();
    seed_today(tmp.path()).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(
        &client,
        "get_performance",
        json!({"period": "since_inception"}),
    )
    .await;
    assert_eq!(v["method"], "none", "{v}");
    assert!(v["change"].is_null(), "{v}");
    assert!(v["startValue"].is_null(), "{v}");
    assert!(v["netContributions"].is_null(), "{v}");
    assert!(v["marketEffect"].is_null(), "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
    let from = v["from"].as_str().unwrap();
    let to = v["to"].as_str().unwrap();
    assert!(from <= to, "{from} > {to}: {v}");
    let today = day(0);
    assert_eq!((from, to), (today.as_str(), today.as_str()), "{v}");
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert!(has_code(&v, "performance_twr_unavailable").is_none(), "{v}");

    // Same path: period day with only today's snapshot has no start before today.
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(v["method"], "none", "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
    assert!(v["change"].is_null() && v["startValue"].is_null(), "{v}");
    assert!(
        v["netContributions"].is_null() && v["marketEffect"].is_null(),
        "{v}"
    );
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert!(has_code(&v, "performance_twr_unavailable").is_none(), "{v}");
    let from = v["from"].as_str().unwrap();
    let to = v["to"].as_str().unwrap();
    assert!(from <= to, "{from} > {to}: {v}");
    assert_eq!((from, to), (today.as_str(), today.as_str()), "{v}");
    client.cancel().await.unwrap();
}

/// Snapshots only on D-1 and D: `from` is the base when one is used, and
/// `snapshotDays` counts the same points in every period.
#[tokio::test]
async fn performance_from_and_snapshot_days_follow_the_base_point() {
    let tmp = TempDir::new().unwrap();
    seed_history(tmp.path(), &[-1, 0]).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let mut days = Vec::new();
    for period in ["day", "since_inception"] {
        let v = ok(&client, "get_performance", json!({"period": period})).await;
        assert_eq!(v["from"], day(-1), "{period}: {v}");
        assert_eq!(v["to"], day(0), "{period}: {v}");
        assert!(
            v["from"].as_str().unwrap() <= v["to"].as_str().unwrap(),
            "{v}"
        );
        assert_eq!(
            v["dataCoverage"]["startPointDate"],
            day(-1),
            "{period}: {v}"
        );
        assert_eq!(v["dataCoverage"]["snapshotDays"], 2, "{period}: {v}");
        assert_eq!(v["dataCoverage"]["missingDays"], 0, "{period}: {v}");
        assert!(v["change"].is_number(), "{period}: {v}");
        assert_eq!(v["dataStatus"], "complete", "{period}: {v}");
        days.push(v["dataCoverage"]["snapshotDays"].as_u64().unwrap());
    }
    assert_eq!(days, [2, 2]);

    let v = ok(&client, "get_performance", json!({"period": "week"})).await;
    assert_eq!(v["from"], day(-6), "{v}");
    assert_eq!(v["to"], day(0), "{v}");
    assert!(v["dataCoverage"]["startPointDate"].is_null(), "{v}");
    assert_eq!(v["dataCoverage"]["snapshotDays"], 2, "{v}");
    assert_eq!(v["complete"], false, "{v}");
    assert_eq!(v["dataStatus"], "insufficient_history", "{v}");
    assert_eq!(v["method"], "none", "{v}");
    assert!(has_code(&v, "insufficient_history").is_some(), "{v}");
    assert!(
        v["startValue"].is_null() && v["change"].is_null() && v["marketEffect"].is_null(),
        "{v}"
    );
    for period in ["month", "ytd"] {
        let v = ok(&client, "get_performance", json!({"period": period})).await;
        assert_eq!(v["dataStatus"], "insufficient_history", "{period}: {v}");
        assert_eq!(v["complete"], false, "{period}: {v}");
        assert_eq!(v["method"], "none", "{period}: {v}");
        assert!(
            v["startValue"].is_null() && v["change"].is_null(),
            "{period}: {v}"
        );
    }
    client.cancel().await.unwrap();
}

/// Totals are the sum of rounded per-position values, not a rounding of the
/// unrounded sum (1.375 + 1.375 → 2.76, not 2.75).
#[tokio::test]
async fn totals_match_the_sum_of_rounded_position_values() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("positions.json");
    let positions = json!([
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 1.375},
        {"Name": "Bar", "AssetClass": "Gold", "Amount": 1.375}
    ]);
    std::fs::write(&data, positions.to_string()).unwrap();
    let data = data.to_string_lossy().into_owned();
    let client = connect(McpOptions {
        file: Some(data.clone()),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    let values: Vec<f64> = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["valueBase"].as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![1.38, 1.38], "{v}");
    let sum: f64 = values.iter().sum();
    let rounded_sum = (sum * 100.0).round() / 100.0;
    assert_eq!(v["totalValue"], rounded_sum, "{v}");
    assert_eq!(v["totalValue"], 2.76, "{v}");
    assert_eq!(v["cashValue"], 1.38, "{v}");

    let a = ok(&client, "get_allocation", json!({})).await;
    assert_eq!(a["totalValue"], v["totalValue"], "{a}");
    let classes = a["classes"].as_array().unwrap();
    assert!(
        classes.iter().all(|c| c["value"].as_f64() == Some(1.38)),
        "{a}"
    );
    let class_sum: f64 = classes.iter().map(|c| c["value"].as_f64().unwrap()).sum();
    assert_eq!((class_sum * 100.0).round() / 100.0, a["totalValue"], "{a}");

    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    assert_eq!(
        ctx["context"]["summary"]["totalValue"], v["totalValue"],
        "{ctx}"
    );
    assert_eq!(ctx["context"]["summary"]["cashValue"], 1.38, "{ctx}");

    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        file: Some(data),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let mut snap: Value = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    snap["date"] = json!(day(-1));
    std::fs::write(&path, format!("{snap}\n")).unwrap();
    let p = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(p["endValue"], v["totalValue"], "{p}");
    client.cancel().await.unwrap();
}

fn approx_reason<'a>(v: &'a Value, reason: &str) -> Vec<&'a Value> {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "performance_approximate" && w["detail"]["reason"] == reason)
        .collect()
}

/// A same-step deposit and buy with no dated cash deposit books
/// externalUnknown and must be flagged once, including with groupBy none.
/// A window with no such flows must not carry the note. `complete` is not
/// changed by the note.
#[tokio::test]
async fn external_unknown_flows_are_flagged_once() {
    let tmp = TempDir::new().unwrap();
    let buy_day = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let data = tmp.path().join("positions.json");
    let positions = json!([
        {
            "Name": "Index",
            "AssetClass": "Stocks",
            "Amount": 3940.0,
            "Purchases": [
                {"Date": "2020/01/02", "Quantity": 1, "Price": 100.0},
                {"Date": buy_day.format("%Y/%m/%d").to_string(), "Quantity": 1, "Price": 3840.0}
            ]
        },
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 6160.0}
    ]);
    std::fs::write(&data, positions.to_string()).unwrap();
    let data = data.to_string_lossy().into_owned();
    let (path, _, _) = portfolio_rs::mcp::record_snapshot(&McpOptions {
        file: Some(data.clone()),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await
    .unwrap();
    let today: Value = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let mut start = today;
    start["date"] = json!(day(-7));
    let mut total = 0.0;
    for p in start["positions"].as_array_mut().unwrap() {
        p["ledgerBase"] = Value::Null;
        if p["kind"] == "cash" {
            p["valueBase"] = json!(0.0);
            p["quantity"] = json!(0.0);
        } else if p["assetClass"] == "Stocks" {
            p["valueBase"] = json!(100.0);
        }
        total += p["valueBase"].as_f64().unwrap();
    }
    start["cashBase"] = json!(0.0);
    start["totalBase"] = json!(total);
    std::fs::write(&path, format!("{start}\n")).unwrap();
    let client = connect(McpOptions {
        file: Some(data),
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    for group in ["assetClass", "none", "position"] {
        let v = ok(
            &client,
            "get_performance",
            json!({"period": "week", "groupBy": group}),
        )
        .await;
        let notes = approx_reason(&v, "external_unknown_flows");
        assert_eq!(notes.len(), 1, "groupBy {group}: {v}");
        let note = notes[0];
        assert_eq!(note["severity"], "info", "{note}");
        assert_eq!(note["detail"]["method"], v["method"], "{note}");
        let total = note["detail"]["externalUnknown"].as_f64().unwrap();
        assert!((total - 3_840.0).abs() < 0.05, "{note}");
        assert!(
            note["message"]
                .as_str()
                .unwrap()
                .contains("position(s) have flows of unknown origin"),
            "{note}"
        );
        let ids = note["detail"]["positionIds"].as_array().unwrap();
        assert!(!ids.is_empty(), "{note}");
        let classes: Vec<&str> = note["detail"]["classes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(classes.contains(&"Stocks"), "{note}");
        let mut uniq = classes.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(classes.len(), uniq.len(), "classes not deduped: {note}");
        let reasons: Vec<&str> = v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == "performance_approximate")
            .map(|w| w["detail"]["reason"].as_str().unwrap())
            .collect();
        let mut deduped = reasons.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(reasons.len(), deduped.len(), "{v}");
        // The note is Info and must not flip complete (same for every groupBy).
        assert_eq!(v["complete"], true, "groupBy {group}: {v}");
        assert_eq!(v["dataStatus"], "complete", "{v}");
    }
    client.cancel().await.unwrap();

    // Clean window: seeded history, no flows of unknown origin.
    let tmp = TempDir::new().unwrap();
    seed_history(tmp.path(), &[-1]).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert!(v["change"].is_number(), "{v}");
    assert!(
        approx_reason(&v, "external_unknown_flows").is_empty(),
        "{v}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_performance_period_defaults_to_day() {
    let client = connect(opts()).await;
    let empty = ok(&client, "get_performance", json!({})).await;
    let explicit = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(empty["period"], "day");
    assert_eq!(empty["period"], explicit["period"]);
    assert_eq!(empty["from"], explicit["from"]);
    assert_eq!(empty["to"], explicit["to"]);
    assert_eq!(empty["method"], explicit["method"]);
    assert_eq!(empty["dataStatus"], explicit["dataStatus"]);

    let result = call(&client, "get_performance", json!({"period": "fortnight"})).await;
    assert_eq!(result.is_error, Some(true));
    let text = serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert_eq!(parsed["error"]["code"], "invalid_argument", "{text}");
    let message = parsed["error"]["message"].as_str().unwrap_or(&text);
    for allowed in [
        "day",
        "week",
        "month",
        "mtd",
        "ytd",
        "1y",
        "since_inception",
        "custom",
    ] {
        assert!(message.contains(allowed), "missing {allowed} in {message}");
    }

    let tools = client.list_all_tools().await.unwrap();
    let perf = tools
        .iter()
        .find(|tool| tool.name == "get_performance")
        .unwrap();
    let schema = Value::Object((*perf.input_schema).clone());
    let required = schema.get("required").and_then(|req| req.as_array());
    assert!(
        required.is_none_or(|items| !items.iter().any(|item| item == "period")),
        "{schema}"
    );
    let desc = schema["properties"]["period"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(desc.contains("period defaults to day"), "{desc}");
    client.cancel().await.unwrap();
}

/// Rewrite today's seeded snapshot as day D-1 with `dataStatus: stale`.
/// When `mark` is true, one market position is `stale: true`; otherwise the
/// per-position marker is stripped (a line written before it existed).
async fn redate_stale_start(dir: &Path, mark: bool) -> String {
    let (path, mut old) = seed_today(dir).await;
    old["date"] = json!(day(-1));
    old["dataStatus"] = json!("stale");
    let stale_id = old["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "market")
        .expect("market position")["positionId"]
        .as_str()
        .unwrap()
        .to_string();
    for p in old["positions"].as_array_mut().unwrap() {
        let obj = p.as_object_mut().unwrap();
        if mark && obj.get("positionId").and_then(|id| id.as_str()) == Some(stale_id.as_str()) {
            obj.insert("stale".into(), json!(true));
        } else if !mark {
            obj.remove("stale");
        }
    }
    std::fs::write(&path, format!("{old}\n")).unwrap();
    stale_id
}

/// A stale start snapshot must name the positions, not only flip dataStatus.
#[tokio::test]
async fn get_performance_names_stale_start_point() {
    let tmp = TempDir::new().unwrap();
    let stale_id = redate_stale_start(tmp.path(), true).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(v["dataStatus"], "stale", "{v}");
    assert_eq!(v["hasStale"], true, "{v}");
    assert_eq!(v["complete"], true, "{v}");
    assert!(
        v["stalePositions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &stale_id),
        "{v}"
    );
    assert_eq!(v["dataQuality"]["affectedPositions"], json!([]), "{v}");
    let w = has_code(&v, "start_point_stale").expect("note");
    assert_eq!(w["severity"], "warning", "{w}");
    assert_eq!(w["category"], "data_quality", "{w}");
    assert_eq!(w["detail"]["point"], "start", "{w}");
    assert_eq!(w["detail"]["date"], day(-1), "{w}");
    assert!(
        w["detail"]["positions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &stale_id),
        "{w}"
    );
    let msg = w["message"].as_str().unwrap();
    assert!(
        msg.contains("start") && msg.contains(&day(-1)) && msg.contains(&stale_id),
        "{msg}"
    );
    assert!(has_code(&v, "end_point_stale").is_none(), "{v}");
    client.cancel().await.unwrap();
}

/// A pre-marker stale snapshot still names the point and date, with no ids.
#[tokio::test]
async fn get_performance_legacy_stale_snapshot_notes_unknown_positions() {
    let tmp = TempDir::new().unwrap();
    redate_stale_start(tmp.path(), false).await;
    let client = connect(McpOptions {
        data_dir: Some(tmp.path().to_path_buf()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_performance", json!({"period": "day"})).await;
    assert_eq!(v["dataStatus"], "stale", "{v}");
    assert_eq!(v["hasStale"], true, "{v}");
    assert_eq!(v["stalePositions"], json!([]), "{v}");
    assert_eq!(v["dataQuality"]["affectedPositions"], json!([]), "{v}");
    let w = has_code(&v, "start_point_stale").expect("note");
    assert_eq!(w["detail"]["point"], "start", "{w}");
    assert_eq!(w["detail"]["date"], day(-1), "{w}");
    assert_eq!(w["detail"]["positions"], json!([]), "{w}");
    let msg = w["message"].as_str().unwrap();
    assert!(msg.contains(&day(-1)), "{msg}");
    assert!(
        msg.contains("did not record which positions were stale"),
        "{msg}"
    );
    client.cancel().await.unwrap();
}

/// Manual holdings without a ticker count in invested and unrealized P&L,
/// matching CLI `balances` (cash excluded; manual at the ledger amount).
#[tokio::test]
async fn manual_cost_basis_matches_cli_balances() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("positions.json");
    std::fs::write(
        &data,
        json!([
            {
                "Name": "Fake ETF",
                "AssetClass": "Stocks",
                "Ticker": "FAKEETF",
                "Amount": 10,
                "Purchases": [{"Date": "2025/01/15", "Quantity": 10, "Price": 100.0}]
            },
            {
                "Name": "Ledger item",
                "AssetClass": "Other",
                "Amount": 98,
                "Purchases": [{"Date": "2025/01/15", "Quantity": 1, "Price": 100.0}]
            }
        ])
        .to_string(),
    )
    .unwrap();
    let quotes = tmp.path().join("quotes.json");
    std::fs::write(
        &quotes,
        r#"{"latest":{"FAKEETF":{"price":150.0,"currency":"EUR","asOf":"2026-10-27T12:00:00Z"}},"previous_close":{"FAKEETF":149.0}}"#,
    )
    .unwrap();
    let fixture = FixtureProvider::from_json(&std::fs::read_to_string(&quotes).unwrap()).unwrap();
    let client = connect(McpOptions {
        file: Some(data.to_string_lossy().into_owned()),
        quotes: Some(AnyProvider::Fixture(Box::new(fixture))),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["invested"], 1100.0, "{v}");
    assert_eq!(v["unrealizedPnl"], 498.0, "{v}");
    assert_eq!(v["unrealizedPnlPercent"], 45.27, "{v}");
    assert_eq!(v["pnlPartial"], false, "{v}");
    assert_eq!(v["totalValue"], 1598.0, "{v}");
    let etf = by_ticker(&v["positions"], "FAKEETF");
    assert_eq!(etf["valueBase"], 1500.0, "{etf}");
    assert_eq!(etf["invested"], 1000.0, "{etf}");
    assert_eq!(etf["unrealizedPnl"], 500.0, "{etf}");
    let manual = v["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "manual")
        .expect("manual row");
    assert_eq!(manual["invested"], 100.0, "{manual}");
    assert_eq!(manual["valueBase"], 98.0, "{manual}");
    assert_eq!(manual["unrealizedPnl"], -2.0, "{manual}");
    let sum = |key: &str| -> f64 {
        v["positions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p[key].as_f64())
            .sum()
    };
    assert!((sum("invested") - 1100.0).abs() < 0.001, "{v}");
    assert!((sum("unrealizedPnl") - 498.0).abs() < 0.001, "{v}");

    let ctx = ok(&client, "get_context", json!({"format": "json"})).await;
    let row = ctx["context"]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Ledger item")
        .expect("context row");
    assert_eq!(row["invested"], 100.0, "{row}");
    assert_eq!(row["pnl"], -2.0, "{row}");
    let tools = client.list_all_tools().await.unwrap();
    let desc = tools
        .iter()
        .find(|t| t.name == "get_positions")
        .unwrap()
        .description
        .clone()
        .unwrap_or_default();
    assert!(desc.contains("manual at their ledger amount"), "{desc}");
    client.cancel().await.unwrap();

    let home = TempDir::new().unwrap();
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
        .arg("balances")
        .arg(&data)
        .current_dir(tmp.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env(
            "PORTFOLIO_RS_QUOTES",
            format!("fixture:{}", quotes.display()),
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "balances failed: {stderr}\n{stdout}"
    );
    let total = stdout
        .lines()
        .find(|line| line.contains("TOTAL"))
        .unwrap_or_else(|| panic!("{stdout}\n{stderr}"));
    assert!(total.contains("1100.00"), "{total}");
    assert!(total.contains("498.00"), "{total}");
    assert!(total.contains("45.27"), "{total}");
}

const FIXTURE_QUOTES: &str = r#"{"latest":{"FAKEETF":{"price":150.0,"currency":"EUR","asOf":"2026-10-27T12:00:00Z"}},"previous_close":{"FAKEETF":149.0}}"#;

fn fake_quotes() -> AnyProvider {
    AnyProvider::Fixture(Box::new(
        FixtureProvider::from_json(FIXTURE_QUOTES).unwrap(),
    ))
}

/// (code, message) of a failed call.
async fn err_parts(client: &Client, name: &str, args: Value) -> (String, String) {
    let result = call(client, name, args).await;
    assert_eq!(result.is_error, Some(true), "{name} unexpectedly succeeded");
    let text = serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"));
    (
        v["error"]["code"].as_str().unwrap_or_default().to_string(),
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn write_positions(tmp: &TempDir, positions: Value) -> String {
    let data = tmp.path().join("positions.json");
    std::fs::write(&data, positions.to_string()).unwrap();
    data.to_string_lossy().into_owned()
}

fn by_name<'a>(positions: &'a Value, name: &str) -> &'a Value {
    positions
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// An unparseable positions file is a tool error, never an empty portfolio.
#[tokio::test]
async fn invalid_positions_file_is_an_error() {
    let tmp = TempDir::new().unwrap();
    let file = write_positions(
        &tmp,
        json!([{"Name": "Broken", "AssetClass": "Stocks", "Ticker": "FAKEETF"}]),
    );
    let client = connect(McpOptions {
        file: Some(file.clone()),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    for tool in [
        "get_positions",
        "get_allocation",
        "check_policy",
        "get_transactions",
        "list_portfolios",
        "get_performance",
        "get_context",
    ] {
        let (code, message) = err_parts(&client, tool, json!({})).await;
        assert_eq!(code, "invalid_data", "{tool}: {message}");
        assert!(message.contains("Amount"), "{tool}: {message}");
        assert!(message.contains(&file), "{tool}: {message}");
    }
    assert!(read(&client, "portfolio://main/context.md").await.is_err());
    client.cancel().await.unwrap();

    // An empty but valid file still works.
    let empty = write_positions(&tmp, json!([]));
    let client = connect(McpOptions {
        file: Some(empty),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["positions"], json!([]), "{v}");
    assert_eq!(v["dataStatus"], "complete", "{v}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_performance_null_period_defaults_to_day() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_performance", json!({"period": null})).await;
    assert_eq!(v["period"], "day", "{v}");
    let tools = client.list_all_tools().await.unwrap();
    let tool = tools.iter().find(|t| t.name == "get_performance").unwrap();
    let schema = Value::Object((*tool.input_schema).clone()).to_string();
    assert!(schema.contains("since_inception"), "{schema}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_performance_non_string_period_lists_allowed_values() {
    let client = connect(opts()).await;
    for bad in [json!(5), json!(true), json!({}), json!("fortnight")] {
        let (code, message) = err_parts(&client, "get_performance", json!({"period": bad})).await;
        assert_eq!(code, "invalid_argument", "{bad}: {message}");
        for allowed in [
            "day",
            "week",
            "month",
            "mtd",
            "ytd",
            "1y",
            "since_inception",
            "custom",
        ] {
            assert!(message.contains(allowed), "{bad}: {message}");
        }
        assert!(!message.contains("invalid type"), "{bad}: {message}");
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn pnl_partial_warns_with_excluded_positions() {
    let tmp = TempDir::new().unwrap();
    let file = write_positions(
        &tmp,
        json!([
            {
                "Name": "Fake ETF", "AssetClass": "Stocks", "Ticker": "FAKEETF", "Amount": 10,
                "Purchases": [{"Date": "2025/01/15", "Quantity": 10, "Price": 100.0}]
            },
            {"Name": "No cost", "AssetClass": "Stocks", "Ticker": "FAKEETF", "Amount": 2}
        ]),
    );
    let client = connect(McpOptions {
        file: Some(file),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["pnlPartial"], true, "{v}");
    let warnings: Vec<&Value> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "cost_basis_missing")
        .collect();
    assert_eq!(warnings.len(), 1, "{v}");
    let w = warnings[0];
    assert_eq!(w["severity"], "warning", "{w}");
    assert_eq!(w["category"], "data_quality", "{w}");
    assert!(w["message"].as_str().unwrap().contains("No cost"), "{w}");
    let missing = by_name(&v["positions"], "No cost")["id"].clone();
    assert_eq!(w["detail"]["positions"], json!([missing]), "{w}");
    client.cancel().await.unwrap();

    // Complete cost basis: no warning.
    let tmp = TempDir::new().unwrap();
    let file = write_positions(
        &tmp,
        json!([{
            "Name": "Fake ETF", "AssetClass": "Stocks", "Ticker": "FAKEETF", "Amount": 10,
            "Purchases": [{"Date": "2025/01/15", "Quantity": 10, "Price": 100.0}]
        }]),
    );
    let client = connect(McpOptions {
        file: Some(file),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_eq!(v["pnlPartial"], false, "{v}");
    assert!(has_code(&v, "cost_basis_missing").is_none(), "{v}");
    client.cancel().await.unwrap();
}

/// A manual position without purchases is one item valued at `Amount`; a
/// market position without purchases holds `Amount` units. `balances` shows
/// "-" instead of a 0.00 price for manual items.
#[tokio::test]
async fn manual_without_purchases_has_quantity_one_and_cli_price_dash() {
    let tmp = TempDir::new().unwrap();
    let file = write_positions(
        &tmp,
        json!([
            {"Name": "Fake ETF", "AssetClass": "Stocks", "Ticker": "FAKEETF", "Amount": 4},
            {"Name": "Ledger item", "AssetClass": "Other", "Amount": 250}
        ]),
    );
    let client = connect(McpOptions {
        file: Some(file.clone()),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    let v = ok(&client, "get_positions", json!({})).await;
    let manual = by_name(&v["positions"], "Ledger item");
    assert_eq!(manual["quantity"], 1.0, "{manual}");
    assert_eq!(manual["valueBase"], 250.0, "{manual}");
    let etf = by_name(&v["positions"], "Fake ETF");
    assert_eq!(etf["quantity"], 4.0, "{etf}");
    assert_eq!(etf["valueBase"], 600.0, "{etf}");
    assert_eq!(v["totalValue"], 850.0, "{v}");
    client.cancel().await.unwrap();

    let quotes = tmp.path().join("quotes.json");
    std::fs::write(&quotes, FIXTURE_QUOTES).unwrap();
    let home = TempDir::new().unwrap();
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
        .arg("balances")
        .arg(&file)
        .current_dir(tmp.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env(
            "PORTFOLIO_RS_QUOTES",
            format!("fixture:{}", quotes.display()),
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    let row = stdout
        .lines()
        .find(|l| l.contains("Ledger item"))
        .unwrap_or_else(|| panic!("{stdout}"));
    let cells: Vec<&str> = row.split('┆').map(str::trim).collect();
    // Name, Class, Quantity, Avg Cost, Invested, Price, Value, ...
    assert!(stdout.contains("Quantity"), "{stdout}");
    assert_eq!(cells[2], "1.0000", "{row}");
    assert_eq!(cells[5], "-", "{row}");
    assert_eq!(cells[6], "250.00", "{row}");
}

/// A multi-line positions file where the second position lacks `Amount`.
fn multiline_missing_amount() -> &'static str {
    r#"[
 {
  "Name": "Fake ETF",
  "Ticker": "FAKEETF",
  "AssetClass": "Stocks",
  "Amount": 10
 },
 {
  "Name": "Fake Watch",
  "AssetClass": "Collectibles"
 }
]"#
}

/// Run the CLI with an isolated home, fixture quotes and snapshot dir.
fn run_cli(tmp: &TempDir, args: &[&str]) -> std::process::Output {
    let quotes = tmp.path().join("quotes.json");
    std::fs::write(&quotes, FIXTURE_QUOTES).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::process::Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
        .args(args)
        .current_dir(tmp.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("PORTFOLIO_RS_DATA_DIR", tmp.path().join("data"))
        .env(
            "PORTFOLIO_RS_QUOTES",
            format!("fixture:{}", quotes.display()),
        )
        .output()
        .unwrap()
}

/// Every file under `dir` with its bytes, for before/after comparisons.
fn dir_contents(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path, bytes));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn snapshot_bad_file_keeps_existing_snapshot() {
    let tmp = TempDir::new().unwrap();
    let good = tmp.path().join("good.json");
    std::fs::write(
        &good,
        r#"[{"Name": "Fake ETF", "Ticker": "FAKEETF", "AssetClass": "Stocks", "Amount": 10}]"#,
    )
    .unwrap();
    let out = run_cli(&tmp, &["snapshot", good.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data = tmp.path().join("data");
    let before = dir_contents(&data);
    assert_eq!(before.len(), 1, "{before:?}");

    for bad in [
        multiline_missing_amount(),
        "",
        "not json",
        r#"{"Name": "x"}"#,
        r#"[{"Name": "x", "AssetClass": "Stocks", "Amount": "ten"}]"#,
    ] {
        let file = tmp.path().join("bad.json");
        std::fs::write(&file, bad).unwrap();
        let out = run_cli(&tmp, &["snapshot", file.to_str().unwrap()]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{bad}: {stderr}");
        assert!(stderr.contains("Error parsing positions"), "{stderr}");
        assert!(!stderr.contains("Snapshot written"), "{stderr}");
        assert_eq!(dir_contents(&data), before, "{bad}");
    }
    let file = tmp.path().join("bad.json");
    std::fs::write(&file, multiline_missing_amount()).unwrap();
    let out = run_cli(&tmp, &["snapshot", file.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Amount"));
}

#[test]
fn cli_bad_positions_exit_2() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("bad.json");
    std::fs::write(&file, multiline_missing_amount()).unwrap();
    let file = file.to_str().unwrap();
    for cmd in ["balances", "allocation", "performance", "context", "sort"] {
        let out = run_cli(&tmp, &[cmd, file]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{cmd}: {stderr}");
        assert!(
            stderr.contains("Error parsing positions"),
            "{cmd}: {stderr}"
        );
        assert!(stderr.contains("Amount"), "{cmd}: {stderr}");
        assert!(
            !stdout.contains('┆') && !stdout.contains("Quantity"),
            "{cmd}: {stdout}"
        );
    }
}

fn line_column(text: &str) -> String {
    format!(
        "line {} column {}",
        words_number(text, "line "),
        words_number(text, "column ")
    )
}

fn words_number(text: &str, key: &str) -> String {
    let at = text.find(key).unwrap_or_else(|| panic!("no {key}: {text}")) + key.len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

#[tokio::test]
async fn mcp_and_cli_report_same_parse_position() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    std::fs::write(&file, multiline_missing_amount()).unwrap();
    let file = file.to_string_lossy().into_owned();

    let client = connect(McpOptions {
        file: Some(file.clone()),
        quotes: Some(fake_quotes()),
        ..opts()
    })
    .await;
    let (code, message) = err_parts(&client, "get_positions", json!({})).await;
    assert_eq!(code, "invalid_data", "{message}");
    client.cancel().await.unwrap();

    let out = run_cli(&tmp, &["balances", &file]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        line_column(&message),
        line_column(&stderr),
        "{message} / {stderr}"
    );
    assert_eq!(line_column(&stderr), "line 11 column 2");
}

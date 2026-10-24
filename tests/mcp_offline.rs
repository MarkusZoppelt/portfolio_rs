//! MCP round trips against example_data.json with fixture quotes.
//!
//! In-process tests run the real rmcp server and client over a duplex pipe;
//! the stdio tests spawn the binary with an isolated HOME. No network.

use portfolio_rs::mcp::{load, McpOptions, PortfolioServer};
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
async fn tools_list_has_the_six_read_only_tools() {
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
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_positions_matches_the_fixture_quotes() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert_envelope(&v, "main");
    assert_eq!(v["totalValue"], 97208.0);
    assert_eq!(v["cashValue"], 50000.0);
    assert_eq!(v["networkStatus"], "connected");
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
    let mut untargeted: Vec<_> = v["untargetedClasses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    untargeted.sort();
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
async fn readme_documents_every_tool_and_resource() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let section = readme
        .split("## MCP server")
        .nth(1)
        .expect("README has an MCP server section");
    let client = connect(opts()).await;
    for tool in client.list_all_tools().await.unwrap() {
        assert!(
            section.contains(&format!("`{}`", tool.name)),
            "README misses tool {}",
            tool.name
        );
    }
    for template in client.list_all_resource_templates().await.unwrap() {
        let uri = template.uri_template.replace("{id}", "<id>");
        assert!(section.contains(&uri), "README misses resource {uri}");
    }
    for resource in client.list_all_resources().await.unwrap() {
        let uri = resource
            .uri
            .replace("portfolio://main/", "portfolio://<id>/");
        assert!(section.contains(&uri), "README misses resource {uri}");
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
    assert_eq!(v["networkStatus"], "partial");
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
    for c in v["classes"].as_array().unwrap() {
        assert!(c["amountToTarget"].is_null(), "{c}");
        assert!(c["driftPoints"].is_null(), "{c}");
        assert!(
            ["unknown", "untargeted"].contains(&c["status"].as_str().unwrap()),
            "{c}"
        );
    }
    assert_eq!(class(&v, "Bonds")["status"], "unknown");

    let v = ok(&client, "check_policy", json!({})).await;
    let found = codes(&v["findings"]);
    assert!(
        !found.contains(&"allocation_out_of_band".to_string()),
        "{found:?}"
    );
    assert!(found.contains(&"checks_incomplete".to_string()));
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
    assert_eq!(class(&v, "Cash")["status"], "unknown");
    assert!(class(&v, "Cash")["amountToTarget"].is_null());
    let v = ok(&client, "check_policy", json!({})).await;
    // Cash is 100 % of the priced total, but that is not a known breach.
    assert!(!codes(&v["breaches"]).contains(&"asset_class_limit".to_string()));
    assert!(!codes(&v["findings"]).contains(&"allocation_out_of_band".to_string()));
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

    // One market position without a quote: no partial sum labelled as total.
    let (_tmp, client) = connect_partial("quotes_no_gsg.json").await;
    let v = ok(&client, "get_positions", json!({})).await;
    assert!(v["unrealizedPnl"].is_null(), "{}", v["unrealizedPnl"]);
    assert!(v["unrealizedPnlPercent"].is_null());
    assert!(v["dayPnl"].is_null());
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

    // Offline: every price from the cache, stale; SPY's asOf is older than the
    // default 7 days, so it does not count.
    let client = connect(ws(AnyProvider::Offline, CacheMode::Auto, None)).await;
    let v = ok(&client, "get_positions", json!({})).await;
    let tlt = by_ticker(&v["positions"], "TLT");
    assert_eq!(tlt["priceSource"], "cache");
    assert_eq!(tlt["isStale"], true);
    assert_eq!(tlt["priceStatus"], "stale");
    assert_eq!(tlt["price"], 90.0);
    assert!(tlt["priceAsOf"].is_string());
    assert!(tlt["dayChangePercent"].is_number(), "cached previous close");
    assert!(by_ticker(&v["positions"], "SPY")["price"].is_null());
    assert_eq!(by_ticker(&v["positions"], "SPY")["priceSource"], "none");
    assert_eq!(v["complete"], false);
    assert_eq!(v["hasStale"], true);
    assert_eq!(v["networkStatus"], "offline");
    drop(client);

    // A longer max age lets SPY count again: complete, every price stale.
    let client = connect(ws(AnyProvider::Offline, CacheMode::Auto, Some(3650))).await;
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
    assert_eq!(v["stalePositions"].as_array().unwrap().len(), market);
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
    let (_t, client) = connect_partial("quotes_no_fx.json").await;
    assert_eq!(statuses(&client).await, vec!["partial"; 6]);
    let (_t, client) = connect_partial("quotes_no_gsg.json").await;
    assert_eq!(statuses(&client).await, vec!["partial"; 6]);
    let client = connect(opts()).await;
    assert_eq!(statuses(&client).await, vec!["connected"; 6]);
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
        match (&seen_md, &seen_json) {
            (Some(m), Some(j)) => {
                assert_eq!(&md, m);
                assert_eq!(&(classes.clone(), names.clone()), j);
            }
            _ => {
                seen_md = Some(md);
                seen_json = Some((classes, names));
            }
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
    for client in [
        connect(opts()).await,
        connect_partial("quotes_no_gsg.json").await.1,
    ] {
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
        assert!(saw_fraction, "fixture should yield fractional weights");
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
    assert_eq!(tx["amount"], 50000.0);
    let v = ok(&client, "get_transactions", json!({})).await;
    assert_eq!(v["transactions"][0]["type"], "deposit", "opening first");
    assert_eq!(v["transactions"][1]["synthesized"], false);
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
}

/// Item 10: get_transactions is incomplete when matching amountBase values are
/// null (no historic FX), and complete when every base amount is known.
#[tokio::test]
async fn transactions_complete_reflects_base_amounts() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_transactions", json!({})).await;
    assert_eq!(v["complete"], false);
    assert!(v["totals"]["buyAmountBase"].is_null());
    let w = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "fx_historic_unavailable")
        .unwrap();
    assert!(!w["detail"]["positionIds"].as_array().unwrap().is_empty());
    let v = ok(&client, "get_transactions", json!({"types": ["deposit"]})).await;
    assert_eq!(v["complete"], true, "{v}");
    assert!(v["totals"]["depositAmountBase"].is_number());
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

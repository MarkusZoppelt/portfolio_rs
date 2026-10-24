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
    let json = std::fs::read_to_string(fixture_path()).unwrap();
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
        assert_eq!(
            ann.open_world_hint,
            Some(tool.name != "list_portfolios"),
            "{}",
            tool.name
        );
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
    assert_envelope(&v, "default");
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
    assert_eq!(v["networkStatus"], "disconnected");
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
    assert_envelope(&v, "default");
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
    assert_eq!(v["policy"]["source"], "portfolio://default/policy");
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
    assert_envelope(&v, "default");
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
    assert_envelope(&v, "default");
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
        assert_eq!(t["type"], "buy");
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
    assert_eq!(cash["priceSource"], "missing");
    assert_eq!(cash["currency"], "EUR");

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
    // Unknown fields and bad enum values are rejected by the input schema.
    assert!(err(&client, "get_positions", json!({ "bogus": true }))
        .await
        .contains("unknown field"));
    assert!(err(&client, "get_positions", json!({ "sortBy": "random" }))
        .await
        .contains("unknown variant"));
    assert!(err(&client, "get_context", json!({ "format": "html" }))
        .await
        .contains("unknown variant"));
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
        err(
            &client,
            "get_positions",
            json!({ "portfolioId": "default" })
        )
        .await,
        "unknown_portfolio"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_context_markdown_and_json() {
    let client = connect(opts()).await;
    let v = ok(&client, "get_context", json!({})).await;
    assert_envelope(&v, "default");
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
            "portfolio://default/context.md",
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

    let (mime, text) = read(&client, "portfolio://default/context.md")
        .await
        .unwrap();
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
        "portfolio://default/policy",
        "portfolio://default/policy.md",
        "portfolio://other/context.md",
        "portfolio://default/secrets",
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

    fn run(args: &[&str], lines: &[Value]) -> (std::process::Output, Vec<Value>) {
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

    #[test]
    fn stdio_round_trip_with_fixture_quotes() {
        let example = example_data();
        let (output, messages) = run(
            &["--offline", &example],
            &[
                initialize(),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_positions","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"portfolio://default/context.md"}}),
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
        let (output, messages) = run(&[&example, "--workspace", "/tmp"], &[]);
        assert_eq!(output.status.code(), Some(2));
        assert!(messages.is_empty());
    }
}

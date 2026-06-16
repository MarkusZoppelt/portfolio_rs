//! End-to-end tests that require network access to Yahoo Finance.
//!
//! All tests are `#[ignore]`d; run them explicitly with:
//! `cargo test --test e2e_network -- --ignored`

use assert_cmd::Command;
use predicates::str;
use serde_json::Value;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("portfolio_rs").unwrap()
}

fn example_data() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json").to_string()
}

/// Write a broad test policy into a fresh temp dir and return (guard, path).
fn test_policy() -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let policy_toml = r#"
version = "1.0"
name = "E2E Test Policy"
base_currency = "EUR"
time_horizon_years = 10
risk_profile = "moderate"

[constraints]
minimum_cash_amount = 1000.0
single_position_limit_percent = 80.0

[[allocations]]
asset_class = "Stocks"
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
asset_class = "Bonds"
target_percent = 10.0
tolerance_percent = 5.0

[[allocations]]
asset_class = "Gold"
target_percent = 5.0
tolerance_percent = 3.0

[[allocations]]
asset_class = "Commodities"
target_percent = 5.0
tolerance_percent = 3.0
"#;
    let path = tmp.path().join("policy.toml");
    std::fs::write(&path, policy_toml).unwrap();
    let path = path.to_str().unwrap().to_string();
    (tmp, path)
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_context_json_output_structure() {
    let mut cmd = bin();
    cmd.args(["context", &example_data(), "--format", "json"]);
    let assert = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let parsed: Value = serde_json::from_str(&stdout).unwrap();

    assert!(parsed.get("generatedAt").is_some());
    assert!(parsed.get("currency").is_some());
    assert!(parsed.get("networkStatus").is_some());
    assert!(parsed.get("summary").is_some());
    assert!(parsed.get("allocation").is_some());
    assert!(parsed.get("positions").is_some());
    assert!(parsed.get("riskFlags").is_some());
    assert!(parsed.get("dataQualityFlags").is_some());
    assert!(parsed.get("followUpCommands").is_some());

    let summary = parsed.get("summary").unwrap();
    assert!(summary.get("totalValue").is_some());
    assert!(summary.get("cashValue").is_some());
    assert!(summary.get("positionCount").is_some());

    let positions = parsed.get("positions").unwrap().as_array().unwrap();
    assert!(!positions.is_empty());

    let first = &positions[0];
    assert!(first.get("name").is_some());
    assert!(first.get("value").is_some());
    assert!(first.get("weightPercent").is_some());
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_context_markdown_output() {
    let mut cmd = bin();
    cmd.args(["context", &example_data(), "--format", "markdown"]);
    cmd.assert()
        .success()
        .stdout(str::contains("# Portfolio Context"))
        .stdout(str::contains("## Summary"))
        .stdout(str::contains("## Allocation"))
        .stdout(str::contains("## Positions"))
        .stdout(str::contains("## Risk Flags"))
        .stdout(str::contains("## Data Quality Flags"))
        .stdout(str::contains("## Useful Follow-Up Commands"));
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_review_json_output_structure() {
    let (_tmp, policy_path) = test_policy();

    let mut cmd = bin();
    cmd.args([
        "review",
        &example_data(),
        "--policy",
        &policy_path,
        "--format",
        "json",
    ]);
    let assert = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let parsed: Value = serde_json::from_str(&stdout).unwrap();

    assert!(parsed.get("portfolioValue").is_some());
    assert!(parsed.get("currency").is_some());
    assert!(parsed.get("policyName").is_some());
    assert!(parsed.get("findings").is_some());
    assert!(parsed.get("allocations").is_some());
    assert!(parsed.get("constraintChecks").is_some());
    assert!(parsed.get("suggestedActions").is_some());

    let findings = parsed.get("findings").unwrap().as_array().unwrap();
    assert!(!findings.is_empty());

    let allocations = parsed.get("allocations").unwrap().as_array().unwrap();
    assert!(!allocations.is_empty());

    let first_alloc = &allocations[0];
    assert!(first_alloc.get("assetClass").is_some());
    assert!(first_alloc.get("targetPercent").is_some());
    assert!(first_alloc.get("actualPercent").is_some());
    assert!(first_alloc.get("driftPercent").is_some());
    assert!(first_alloc.get("withinTolerance").is_some());
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_review_markdown_output() {
    let (_tmp, policy_path) = test_policy();

    let mut cmd = bin();
    cmd.args([
        "review",
        &example_data(),
        "--policy",
        &policy_path,
        "--format",
        "markdown",
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("# Portfolio Review"))
        .stdout(str::contains("## Findings"))
        .stdout(str::contains("## Allocation Review"))
        .stdout(str::contains("## Constraint Checks"))
        .stdout(str::contains("## Suggested Actions"));
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_simulate_json_output_structure() {
    let (_tmp, policy_path) = test_policy();

    let mut cmd = bin();
    cmd.args([
        "simulate",
        &example_data(),
        "--policy",
        &policy_path,
        "--format",
        "json",
    ]);
    let assert = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let parsed: Value = serde_json::from_str(&stdout).unwrap();

    assert!(parsed.get("portfolioValue").is_some());
    assert!(parsed.get("currency").is_some());
    assert!(parsed.get("policyName").is_some());
    assert!(parsed.get("scenarios").is_some());

    let scenarios = parsed.get("scenarios").unwrap().as_array().unwrap();
    assert!(!scenarios.is_empty());

    let first = &scenarios[0];
    assert!(first.get("name").is_some());
    assert!(first.get("description").is_some());
    assert!(first.get("trades").is_some());
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_simulate_markdown_output() {
    let (_tmp, policy_path) = test_policy();

    let mut cmd = bin();
    cmd.args([
        "simulate",
        &example_data(),
        "--policy",
        &policy_path,
        "--format",
        "markdown",
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("# Rebalance Simulation"))
        .stdout(str::contains("Scenario 1"))
        .stdout(str::contains("Disclaimer"));
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_balances_output() {
    // Run in a temp dir so the sled database side effect stays out of the repo.
    let tmp = TempDir::new().unwrap();
    let mut cmd = bin();
    cmd.current_dir(tmp.path());
    cmd.args(["balances", &example_data()]);
    cmd.assert()
        .success()
        .stdout(str::contains("Name"))
        .stdout(str::contains("Class"))
        .stdout(str::contains("Value"));
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_allocation_output() {
    let mut cmd = bin();
    cmd.args(["allocation", &example_data()]);
    cmd.assert().success();
}

#[test]
#[ignore = "requires network access to Yahoo Finance"]
fn test_sort_output() {
    let mut cmd = bin();
    cmd.args(["sort", &example_data()]);
    cmd.assert()
        .success()
        .stdout(str::contains("sorted by current value"));
}

//! `allocation` and `performance` print through `Portfolio`'s terminal helpers.
//! Offline: fixture quotes, a throwaway HOME, and a temp working directory
//! for the `./database` balance history.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str;
use tempfile::TempDir;

fn bin(home: &TempDir, cwd: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("portfolio_rs").unwrap();
    cmd.current_dir(cwd.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env(
            "PORTFOLIO_RS_QUOTES",
            concat!(
                "fixture:",
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/quotes_example.json"
            ),
        );
    cmd
}

fn example_data() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json")
}

#[test]
fn allocation_draws_the_pie_chart_and_lists_classes() {
    let (home, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    bin(&home, &cwd)
        .args(["allocation", example_data()])
        .assert()
        .success()
        .stdout(str::contains("Bitcoin"))
        .stdout(str::contains("===================================="))
        .stdout(str::is_match(r"\n\s+Cash \| +\d+\.\d\d\n").unwrap());
}

#[test]
fn performance_compares_with_the_stored_balance() {
    let (home, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    bin(&home, &cwd)
        .args(["balances", example_data()])
        .assert()
        .success();
    assert!(cwd.path().join("database").is_dir(), "balance was stored");
    bin(&home, &cwd)
        .args(["performance", example_data()])
        .assert()
        .success()
        .stdout(str::contains("Since"))
        .stdout(str::contains("Top Gainers"))
        // The stored balance is the current total: 0 % since then.
        .stdout(str::is_match(r"0\.00%\s*│\s*\n").unwrap());
}

#[test]
fn performance_without_a_database_warns_and_prints_nothing() {
    let (home, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    // A regular file where the database directory belongs.
    std::fs::write(cwd.path().join("database"), "").unwrap();
    bin(&home, &cwd)
        .args(["performance", example_data()])
        .assert()
        .success()
        .stdout(str::contains("Since").not())
        .stderr(str::contains(
            "Warning: failed to open performance database",
        ));
}

#[test]
fn performance_with_an_empty_database_compares_with_nothing() {
    let (home, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    bin(&home, &cwd)
        .args(["performance", example_data()])
        .assert()
        .success()
        .stdout(str::contains("Since"));
    assert!(
        cwd.path().join("database").is_dir(),
        "an empty database was opened"
    );
}

//! End-to-end tests for the CLI dispatch and command handlers, offline.
//!
//! Every run gets its own HOME/XDG dirs (so the real config is never read
//! or written) and a temp CWD (so `./database` lands in a tempdir).

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use tempfile::TempDir;

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json");
const QUOTES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/quotes_example.json"
);

/// An isolated environment: HOME, XDG dirs and CWD inside one tempdir.
struct Env {
    tmp: TempDir,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("cwd")).unwrap();
        Self { tmp }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn cwd(&self) -> PathBuf {
        self.path("cwd")
    }

    /// Where the app keeps its config (XDG layout, also on macOS).
    fn config_file(&self) -> PathBuf {
        self.path(".config/portfolio/config.yml")
    }

    fn write_config(&self, yaml: &str) {
        let file = self.config_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, yaml).unwrap();
    }

    /// The CLI with offline quotes.
    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("portfolio_rs").unwrap();
        cmd.env("HOME", self.tmp.path())
            .env("XDG_CONFIG_HOME", self.path(".config"))
            .env("XDG_DATA_HOME", self.path(".local/share"))
            .env_remove("PORTFOLIO_RS_DATA_DIR")
            .env("PORTFOLIO_RS_QUOTES", "offline")
            .current_dir(self.cwd())
            .timeout(std::time::Duration::from_secs(60));
        cmd
    }

    /// The CLI with deterministic fixture quotes.
    fn fixture_bin(&self) -> Command {
        let mut cmd = self.bin();
        cmd.env("PORTFOLIO_RS_QUOTES", format!("fixture:{QUOTES}"));
        cmd
    }

    /// `positions.json` + the balanced-growth policy in `<tmp>/ws`.
    fn workspace(&self) -> PathBuf {
        let ws = self.path("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::copy(EXAMPLE, ws.join("positions.json")).unwrap();
        let out = self
            .bin()
            .args(["policy", "init"])
            .arg(&ws)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        ws
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run(cmd: &mut Command) -> (Output, String, String) {
    let out = cmd.output().unwrap();
    let stdout = text(&out.stdout);
    let stderr = text(&out.stderr);
    (out, stdout, stderr)
}

fn policy(ws: &Path) -> String {
    ws.join("portfolio/policy.toml")
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn config_prints_the_isolated_config_path() {
    let env = Env::new();
    let (out, stdout, _) = run(env.bin().arg("config"));
    assert!(out.status.success());
    let path = env.config_file();
    assert_eq!(
        stdout,
        format!("Your config file is located here: \n{}\n", path.display())
    );
    // Loading the config created it there.
    assert!(path.is_file());
}

#[test]
fn components_lists_every_tui_component() {
    let env = Env::new();
    let (out, stdout, _) = run(env.bin().arg("components"));
    assert!(out.status.success());
    assert!(stdout.starts_with("Available TUI components that can be disabled:\n"));
    for name in ["tab_bar", "total_value", "help", "name"] {
        assert!(
            stdout.lines().any(|l| l.trim_start().starts_with(name)),
            "{name}: {stdout}"
        );
    }
    assert!(stdout.contains("portfolio_rs --disable tab_bar,help"));
}

#[test]
fn tui_without_a_portfolio_file_shows_the_error_and_usage() {
    let env = Env::new();
    let missing = env.path("missing.json");
    let (out, stdout, stderr) = run(env.bin().arg(&missing));
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("missing.json"), "{stderr}");
    assert!(stdout.contains("Usage: portfolio_rs"), "{stdout}");
}

#[test]
fn unparsable_config_is_an_error_and_left_untouched() {
    let env = Env::new();
    env.write_config("currency: [EUR\n");
    let (out, _, stderr) = run(env.bin().arg("components"));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr.contains("could not be parsed"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(env.config_file()).unwrap(),
        "currency: [EUR\n"
    );
}

#[test]
fn balances_allocation_performance_and_sort_with_fixture_quotes() {
    let env = Env::new();

    let (out, stdout, stderr) = run(env.fixture_bin().args(["balances", EXAMPLE]));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("│ Bitcoin "), "{stdout}");
    assert!(!stderr.contains("Warning"), "{stderr}");
    assert!(env.cwd().join("database").is_dir());

    let (out, stdout, _) = run(env.fixture_bin().args(["allocation", EXAMPLE]));
    assert!(out.status.success());
    assert!(stdout.contains("Stocks"), "{stdout}");
    assert!(stdout.contains('%'), "{stdout}");

    let (out, stdout, stderr) = run(env.fixture_bin().args(["performance", EXAMPLE]));
    assert!(out.status.success(), "{stderr}");
    assert!(!stdout.is_empty());

    let (out, stdout, _) = run(env.fixture_bin().args(["sort", EXAMPLE]));
    assert!(out.status.success());
    assert!(
        stdout.starts_with("Positions sorted by current value (display only, file unchanged):\n")
    );
    assert!(stdout.contains("│ Bitcoin "), "{stdout}");
}

#[test]
fn balances_warns_when_the_database_cannot_be_opened() {
    let env = Env::new();
    // A regular file where the sled directory should be.
    std::fs::write(env.cwd().join("database"), "").unwrap();
    let (out, stdout, stderr) = run(env.fixture_bin().args(["balances", EXAMPLE]));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("│ Bitcoin "), "{stdout}");
    assert!(
        stderr.contains("Warning: failed to store balance in database:"),
        "{stderr}"
    );
}

#[test]
fn review_and_simulate_print_markdown_by_default() {
    let env = Env::new();
    let ws = env.workspace();
    let positions = ws.join("positions.json");

    let (out, stdout, stderr) = run(env
        .fixture_bin()
        .arg("review")
        .arg(&positions)
        .args(["--policy", &policy(&ws)]));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.starts_with('#'), "{stdout}");
    assert!(stdout.contains("Balanced Growth"), "{stdout}");

    let (out, stdout, stderr) = run(env
        .fixture_bin()
        .arg("simulate")
        .arg(&positions)
        .args(["--policy", &policy(&ws)]));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.starts_with("# Rebalance Simulation\n"), "{stdout}");
    assert!(stdout.contains("Policy: Balanced Growth"), "{stdout}");
}

#[test]
fn validate_reports_missing_file_errors_and_warnings() {
    let env = Env::new();

    let (out, _, stderr) = run(env.bin().arg("validate"));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr.contains("no portfolio file specified"), "{stderr}");

    let (out, _, stderr) = run(env.bin().arg("validate").arg(env.path("missing.json")));
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr.contains("Error: failed to validate portfolio file:"),
        "{stderr}"
    );

    let invalid = env.path("invalid.json");
    std::fs::write(
        &invalid,
        r#"[{"Name": "No Class", "Ticker": "FAKEA", "AssetClass": "", "Amount": 1},
            {"Name": "Zero", "Ticker": "FAKEB", "AssetClass": "Stocks", "Amount": 0}]"#,
    )
    .unwrap();
    let (out, _, stderr) = run(env.bin().arg("validate").arg(&invalid));
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr.contains("Validation failed with 1 errors and 1 warnings."),
        "{stderr}"
    );
    assert!(
        stderr.contains("❌ No Class: Position has no AssetClass"),
        "{stderr}"
    );
    assert!(
        stderr.contains("⚠️ Zero: Position has zero amount"),
        "{stderr}"
    );

    let warn = env.path("warn.json");
    std::fs::write(
        &warn,
        r#"[{"Name": "Zero", "Ticker": "FAKEB", "AssetClass": "Stocks", "Amount": 0}]"#,
    )
    .unwrap();
    let (out, stdout, stderr) = run(env.bin().arg("validate").arg(&warn));
    assert!(out.status.success(), "{stderr}");
    assert_eq!(stdout, "⚠️  Portfolio file is valid but has 1 warnings.\n");
    assert_eq!(stderr, "⚠️  Zero: Position has zero amount\n");
}

#[test]
fn doctor_reports_an_invalid_policy_as_an_issue() {
    let env = Env::new();
    let ws = env.workspace();
    std::fs::write(ws.join("portfolio/policy.toml"), "not = [toml").unwrap();
    let (out, stdout, _) = run(env.bin().arg("doctor").arg(&ws));
    assert!(out.status.success());
    assert!(stdout.contains("❌ Policy file is invalid:"), "{stdout}");
    assert!(stdout.contains("Issues: 1,"), "{stdout}");
    assert!(
        stdout.contains("Please address the issues above."),
        "{stdout}"
    );
}

#[test]
fn skill_export_dry_run_names_an_existing_skill() {
    let env = Env::new();
    let dir = env.path("skills");
    let file = dir.join("portfolio-rs/SKILL.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "mine").unwrap();
    let (out, stdout, _) = run(env
        .bin()
        .args(["agent", "skill", "export"])
        .arg(&dir)
        .arg("--dry-run"));
    assert!(out.status.success());
    assert!(
        stdout.contains(&format!("skip (exists): {}", file.display())),
        "{stdout}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");
}

#[test]
fn policy_validate_and_init_dry_run() {
    let env = Env::new();
    let (out, stdout, _) = run(env
        .bin()
        .args(["policy", "init", "--strategy", "custom", "--dry-run"])
        .arg(env.path("ws")));
    assert!(out.status.success());
    assert!(
        stdout.starts_with("Dry-run: would create policy at "),
        "{stdout}"
    );
    assert!(stdout.contains("name = \"Custom\""), "{stdout}");
    assert!(!env.path("ws").exists());

    let (out, _, _) = run(env
        .bin()
        .args(["policy", "init", "--strategy", "custom"])
        .arg(env.path("ws")));
    assert!(out.status.success());
    let (out, stdout, _) = run(env
        .bin()
        .args(["policy", "validate"])
        .arg(env.path("ws/portfolio/policy.toml")));
    assert!(out.status.success());
    // The custom policy has no tolerance band.
    assert!(stdout.contains("    Cash: 100% \n"), "{stdout}");
}

#[test]
fn decision_draft_includes_review_context() {
    let env = Env::new();
    let ws = env.workspace();
    let (out, stdout, stderr) = run(env
        .fixture_bin()
        .args(["decision", "draft"])
        .arg(ws.join("positions.json"))
        .args(["--policy", &policy(&ws), "--title", "Trim Crypto", "--dir"])
        .arg(&ws)
        .arg("--dry-run"));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("# Decision: Trim Crypto\n"), "{stdout}");
    assert!(
        stdout.contains("## Context\n\nPortfolio value: "),
        "{stdout}"
    );
    assert!(
        stdout.contains("Key findings from review:\n\n- "),
        "{stdout}"
    );
    assert!(stdout.contains("## Decision\n"), "{stdout}");
}

#[test]
fn weekly_report_includes_summary_allocation_findings_and_actions() {
    let env = Env::new();
    let ws = env.workspace();
    let (out, stdout, stderr) = run(env
        .fixture_bin()
        .args(["report", "weekly"])
        .arg(ws.join("positions.json"))
        .args(["--policy", &policy(&ws), "--dir"])
        .arg(&ws)
        .arg("--dry-run"));
    assert!(out.status.success(), "{stderr}");
    for section in [
        "## Portfolio Summary\n\n- Total Value: ",
        "- Network Status: ",
        "## Allocation vs Target\n",
        "| Stocks | 55.0% |",
        "## Findings\n\n- **",
        "## Suggested Actions\n\n- ",
        "## Notes\n",
    ] {
        assert!(stdout.contains(section), "{section}: {stdout}");
    }
}

#[test]
fn weekly_report_for_a_missing_file_is_a_skeleton() {
    let env = Env::new();
    let (out, stdout, stderr) = run(env
        .bin()
        .args(["report", "weekly"])
        .arg(env.path("missing.json"))
        .arg("--dir")
        .arg(env.path("ws"))
        .arg("--dry-run"));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("# Weekly Portfolio Report"), "{stdout}");
    assert!(!stdout.contains("## Portfolio Summary"), "{stdout}");
    assert!(stdout.contains("## Notes"), "{stdout}");
}

fn snapshot(mut cmd: Command, data_dir: &Path) -> String {
    let (out, _, stderr) = run(cmd
        .args(["snapshot", "--no-cache", "--data-dir"])
        .arg(data_dir));
    assert!(out.status.success(), "{stderr}");
    stderr
}

#[test]
fn snapshot_uses_the_configured_workspace_and_keeps_a_complete_line() {
    let env = Env::new();
    let ws = env.workspace();
    env.write_config(&format!(
        "version: 1\ncurrency: EUR\ntheme: dark\nlast_mode: workspace\nworkspace_dir: {}\n",
        ws.display()
    ));
    let data = env.path("data");
    let file = data.join("main/snapshots.jsonl");

    let stderr = snapshot(env.fixture_bin(), &data);
    assert_eq!(stderr, format!("Snapshot written to {}\n", file.display()));
    let complete = std::fs::read_to_string(&file).unwrap();

    // Offline, today's snapshot would be incomplete: the complete one stays.
    let stderr = snapshot(env.bin(), &data);
    assert_eq!(
        stderr,
        format!(
            "Warning: today's snapshot would be incomplete; kept the complete one already in {}\n",
            file.display()
        )
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), complete);
}

#[test]
fn snapshot_uses_the_configured_file_and_marks_missing_prices() {
    let env = Env::new();
    env.write_config(&format!(
        "version: 1\ncurrency: EUR\ntheme: dark\nlast_mode: simple\nportfolio_file: {EXAMPLE}\n"
    ));
    let data = env.path("data");
    let file = data.join("main/snapshots.jsonl");
    let stderr = snapshot(env.bin(), &data);
    assert_eq!(
        stderr,
        format!(
            "Snapshot written to {} (incomplete: some prices or FX rates are missing)\n",
            file.display()
        )
    );
    let line: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&file).unwrap().trim()).unwrap();
    assert_eq!(line["complete"], false, "{line}");
}

/// Cash only, against a 100% cash policy: nothing to report.
fn on_target_workspace(env: &Env) -> PathBuf {
    let ws = env.path("cash");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("positions.json"),
        r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 1000}]"#,
    )
    .unwrap();
    let (out, _, stderr) = run(env
        .bin()
        .args(["policy", "init", "--strategy", "custom"])
        .arg(&ws));
    assert!(out.status.success(), "{stderr}");
    ws
}

fn draft(env: &Env, file: &Path, policy: &str, dir: &Path, kind: &[&str]) -> String {
    let (out, stdout, stderr) = run(env
        .bin()
        .args(kind)
        .arg(file)
        .args(["--policy", policy, "--dir"])
        .arg(dir)
        .arg("--dry-run"));
    assert!(out.status.success(), "{kind:?}: {stderr}");
    stdout
}

#[test]
fn drafts_on_target_report_alignment() {
    let env = Env::new();
    let ws = on_target_workspace(&env);
    let file = ws.join("positions.json");

    let decision = draft(&env, &file, &policy(&ws), &ws, &["decision", "draft"]);
    assert!(
        decision.contains(
            "## Context\n\nPortfolio value: 1000.00 EUR\n\nKey findings from review:\n\n\
             - Policy Alignment: Portfolio is aligned with policy. No issues found.\n\n## Decision"
        ),
        "{decision}"
    );

    let report = draft(&env, &file, &policy(&ws), &ws, &["report", "weekly"]);
    assert!(
        report.contains(
            "| Cash | 100.0% | 100.0% | +0.0% | ✅ |\n\n## Findings\n\n\
             - **Policy Alignment**: Portfolio is aligned with policy. No issues found.\n\n\
             ## Suggested Actions\n\n- Run 'portfolio_rs context --format json'"
        ),
        "{report}"
    );
}

#[test]
fn drafts_without_a_policy_or_file_skip_the_review() {
    let env = Env::new();
    let ws = on_target_workspace(&env);
    let file = ws.join("positions.json");
    let no_policy = env.path("missing/policy.toml");
    let no_policy = no_policy.to_str().unwrap();

    let decision = draft(&env, &file, no_policy, &ws, &["decision", "draft"]);
    assert!(!decision.contains("## Context"), "{decision}");
    assert!(decision.contains("## Decision"), "{decision}");

    let report = draft(&env, &file, no_policy, &ws, &["report", "weekly"]);
    assert!(report.contains("## Portfolio Summary"), "{report}");
    assert!(!report.contains("## Allocation vs Target"), "{report}");

    let missing = env.path("missing.json");
    let decision = draft(&env, &missing, &policy(&ws), &ws, &["decision", "draft"]);
    assert!(!decision.contains("## Context"), "{decision}");
    assert!(decision.contains("Status: Proposed"), "{decision}");
}

#[test]
fn mcp_accepts_gain_factors_and_stops_at_end_of_input() {
    let env = Env::new();
    let (out, stdout, stderr) = run(env
        .bin()
        .args(["mcp", "--offline", "--gain-factor", "stocks=2", "--gain-factor"])
        .arg("crypto=never")
        .arg(EXAMPLE)
        .write_stdin(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cov","version":"0"}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n"
        )));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains(r#""id":1"#), "{stdout}");
    assert!(stdout.contains("protocolVersion"), "{stdout}");

    let (out, _, stderr) = run(env
        .bin()
        .args(["mcp", "--gain-factor", "stocks=0.5", EXAMPLE]));
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr.contains("factor must be a finite number >= 1"),
        "{stderr}"
    );
}

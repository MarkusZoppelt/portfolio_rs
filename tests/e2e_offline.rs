use assert_cmd::Command;
use predicates::str;
use tempfile::TempDir;

/// Throwaway HOME shared by this test binary, so no test reads or writes the
/// real `~/.config/portfolio`.
fn isolated_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| TempDir::new().unwrap()).path()
}

/// The CLI with an isolated HOME/XDG dirs and no network quotes.
fn bin() -> Command {
    let home = isolated_home();
    let mut cmd = Command::cargo_bin("portfolio_rs").unwrap();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("PORTFOLIO_RS_QUOTES", "offline");
    cmd
}

fn quotes_fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/quotes_example.json"
    )
    .to_string()
}

#[test]
fn test_help_shows_all_commands() {
    let mut cmd = bin();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(str::contains("context"))
        .stdout(str::contains("review"))
        .stdout(str::contains("simulate"))
        .stdout(str::contains("decision"))
        .stdout(str::contains("report"))
        .stdout(str::contains("doctor"))
        .stdout(str::contains("mcp"))
        .stdout(str::contains("policy"))
        .stdout(str::contains("init-workspace"))
        .stdout(str::contains("agent"));
}

#[test]
fn test_context_help() {
    let mut cmd = bin();
    cmd.args(["context", "--help"]);
    cmd.assert()
        .success()
        .stdout(str::contains("format"))
        .stdout(str::contains("markdown"))
        .stdout(str::contains("json"));
}

#[test]
fn test_review_help() {
    let mut cmd = bin();
    cmd.args(["review", "--help"]);
    cmd.assert()
        .success()
        .stdout(str::contains("policy"))
        .stdout(str::contains("format"));
}

#[test]
fn test_simulate_help() {
    let mut cmd = bin();
    cmd.args(["simulate", "--help"]);
    cmd.assert()
        .success()
        .stdout(str::contains("policy"))
        .stdout(str::contains("format"));
}

#[test]
fn test_decision_help() {
    let mut cmd = bin();
    cmd.args(["decision", "--help"]);
    cmd.assert().success().stdout(str::contains("draft"));
}

#[test]
fn test_report_help() {
    let mut cmd = bin();
    cmd.args(["report", "--help"]);
    cmd.assert().success().stdout(str::contains("weekly"));
}

#[test]
fn test_doctor_help() {
    let mut cmd = bin();
    cmd.args(["doctor", "--help"]);
    cmd.assert()
        .success()
        .stdout(str::contains("Target workspace directory"));
}

#[test]
fn test_init_workspace_creates_directories_and_files() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    assert!(dir.join("portfolio/diary").is_dir());
    assert!(dir.join("portfolio/decisions").is_dir());
    assert!(dir.join("portfolio/theses").is_dir());
    assert!(dir.join("portfolio/reports").is_dir());
    assert!(dir.join("portfolio/watchlist.json").is_file());
    assert!(dir.join("INVESTMENT_POLICY.md").is_file());
    assert!(dir.join(".gitignore").is_file());
    assert!(dir.join("AGENTS.md").is_file());
    assert!(dir.join("CLAUDE.md").is_file());
}

#[test]
fn test_init_workspace_dry_run_does_not_create() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap(), "--dry-run"]);
    cmd.assert()
        .success()
        .stdout(str::contains("Dry-run"))
        .stdout(str::contains("No files were created"));

    assert!(!dir.exists());
}

#[test]
fn test_init_workspace_skips_existing_files() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    // First init
    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    // Second init should not fail, just skip
    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("skip (exists)"));
}

#[test]
fn test_agent_init_creates_files() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    // Need workspace first (agent init requires existing dir)
    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    // Remove agent files to test agent init
    std::fs::remove_file(dir.join("AGENTS.md")).unwrap();
    std::fs::remove_file(dir.join("CLAUDE.md")).unwrap();

    let mut cmd = bin();
    cmd.args(["agent", "init", dir.to_str().unwrap()]);
    cmd.assert().success();

    assert!(dir.join("AGENTS.md").is_file());
    assert!(dir.join("CLAUDE.md").is_file());

    let agents_content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(agents_content.contains("Local Files"));
    assert!(agents_content.contains("Safety"));
    assert!(agents_content.contains("Agent Skill"));
    assert!(agents_content.contains("portfolio_rs agent skill export"));

    let claude_content = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert!(claude_content.contains("AGENTS.md"));
}

#[test]
fn test_agent_init_dry_run_does_not_create() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    // Remove agent files to test agent init dry-run
    std::fs::remove_file(dir.join("AGENTS.md")).unwrap();
    std::fs::remove_file(dir.join("CLAUDE.md")).unwrap();

    let mut cmd = bin();
    cmd.args(["agent", "init", dir.to_str().unwrap(), "--dry-run"]);
    cmd.assert()
        .success()
        .stdout(str::contains("Dry-run"))
        .stdout(str::contains("No files were created"));

    assert!(!dir.join("AGENTS.md").exists());
    assert!(!dir.join("CLAUDE.md").exists());
}

#[test]
fn test_agent_init_skips_existing_files() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["agent", "init", dir.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("skip (exists)"));
}

#[test]
fn test_agent_init_fails_on_missing_dir() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("nonexistent");

    let mut cmd = bin();
    cmd.args(["agent", "init", dir.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(str::contains("does not exist"));
}

#[test]
fn test_agent_skill_show_outputs_skill() {
    let mut cmd = bin();
    cmd.args(["agent", "skill", "show"]);
    cmd.assert()
        .success()
        .stdout(str::contains("name: portfolio-rs"))
        .stdout(str::contains("description:"))
        .stdout(str::contains("Startup"))
        .stdout(str::contains("Safety"));
}

#[test]
fn test_agent_skill_export_creates_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    let mut cmd = bin();
    cmd.args(["agent", "skill", "export", skills_dir.to_str().unwrap()]);
    cmd.assert().success();

    let skill_file = skills_dir.join("portfolio-rs/SKILL.md");
    assert!(skill_file.is_file());

    let content = std::fs::read_to_string(&skill_file).unwrap();
    assert!(content.contains("name: portfolio-rs"));
    assert!(content.contains("Startup"));
    assert!(content.contains("Safety"));
}

#[test]
fn test_agent_skill_export_dry_run_does_not_create() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    let mut cmd = bin();
    cmd.args([
        "agent",
        "skill",
        "export",
        skills_dir.to_str().unwrap(),
        "--dry-run",
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("Dry-run"))
        .stdout(str::contains("No files were created"));

    assert!(!skills_dir.exists());
}

#[test]
fn test_agent_skill_export_skips_existing() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    // First export
    let mut cmd = bin();
    cmd.args(["agent", "skill", "export", skills_dir.to_str().unwrap()]);
    cmd.assert().success();

    // Second export should skip
    let mut cmd = bin();
    cmd.args(["agent", "skill", "export", skills_dir.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("skip (exists)"));
}

#[test]
fn test_agent_skill_path_shows_info() {
    let mut cmd = bin();
    cmd.args(["agent", "skill", "path"]);
    cmd.assert()
        .success()
        .stdout(str::contains("Built-in skill"))
        .stdout(str::contains("portfolio-rs"));
}

#[test]
fn test_policy_init_creates_valid_policy() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    // Need workspace first
    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args([
        "policy",
        "init",
        "--strategy",
        "balanced-growth",
        dir.to_str().unwrap(),
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("Created policy"));

    let policy_path = dir.join("portfolio/policy.toml");
    assert!(policy_path.exists());

    let content = std::fs::read_to_string(&policy_path).unwrap();
    assert!(content.contains("Balanced Growth"));
    assert!(content.contains("Stocks"));
    assert!(content.contains("Bonds"));
}

#[test]
fn test_policy_init_warns_when_policy_exists() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["policy", "init", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["policy", "init", dir.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(str::contains("already exists"));
}

#[test]
fn test_policy_validate_valid_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["policy", "init", dir.to_str().unwrap()]);
    cmd.assert().success();

    let policy_path = dir.join("portfolio/policy.toml");
    let mut cmd = bin();
    cmd.args(["policy", "validate", policy_path.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("Policy file is valid"))
        .stdout(str::contains("Balanced Growth"))
        .stdout(str::contains("EUR"));
}

#[test]
fn test_policy_validate_invalid_file() {
    let tmp = TempDir::new().unwrap();
    let policy_path = tmp.path().join("bad-policy.toml");
    std::fs::write(&policy_path, "not valid toml").unwrap();

    let mut cmd = bin();
    cmd.args(["policy", "validate", policy_path.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(str::contains("failed to parse"));
}

#[test]
fn test_doctor_reports_healthy_workspace() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["policy", "init", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["doctor", dir.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("Workspace looks healthy"))
        .stdout(str::contains("Policy file is valid"))
        .stdout(str::contains("Directory exists"));
}

#[test]
fn test_doctor_finds_missing_policy() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    // Only init workspace, no policy
    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["doctor", dir.to_str().unwrap()]);
    cmd.assert()
        .success()
        .stdout(str::contains("Policy file not found"));
}

#[test]
fn test_decision_draft_dry_run() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args([
        "decision",
        "draft",
        "--dir",
        dir.to_str().unwrap(),
        "--title",
        "Test Decision",
        "--dry-run",
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("Dry-run"))
        .stdout(str::contains("Decision: Test Decision"))
        .stdout(str::contains("Proposed"));

    // Verify no file was created
    let decisions_dir = dir.join("portfolio/decisions");
    let files: Vec<_> = std::fs::read_dir(&decisions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(files.len(), 0);
}

#[test]
fn test_report_weekly_dry_run() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args([
        "report",
        "weekly",
        "--dir",
        dir.to_str().unwrap(),
        "--dry-run",
    ]);
    cmd.assert()
        .success()
        .stdout(str::contains("Dry-run"))
        .stdout(str::contains("Weekly Portfolio Report"));
}

#[test]
fn test_mcp_tools_list() {
    use std::io::Write;
    use std::process::Stdio;

    let home = isolated_home();
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("portfolio_rs"))
        .args(["mcp", "--offline", &example_data()])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("PORTFOLIO_RS_QUOTES", "offline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdin = cmd.stdin.take().unwrap();
    let mut stdin_writer = std::io::BufWriter::new(stdin);
    for line in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ] {
        writeln!(stdin_writer, "{line}").unwrap();
    }
    drop(stdin_writer);

    let output = cmd.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    for tool in [
        "get_positions",
        "get_allocation",
        "check_policy",
        "get_transactions",
        "list_portfolios",
        "get_context",
    ] {
        assert!(stdout.contains(tool), "missing {tool}: {stdout}");
    }
    assert!(!stdout.contains("get_portfolio_snapshot"));
}

fn example_data() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json").to_string()
}

/// Set up a workspace with a policy and return (tempdir, workspace dir, policy path).
fn workspace_with_policy() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let mut cmd = bin();
    cmd.args(["policy", "init", dir.to_str().unwrap()]);
    cmd.assert().success();

    let policy = dir.join("portfolio/policy.toml");
    (tmp, dir, policy)
}

#[test]
fn test_context_outputs_markdown_briefing() {
    let mut cmd = bin();
    cmd.args(["context", &example_data()]);
    cmd.assert()
        .success()
        .stdout(str::contains("Portfolio Context"))
        .stdout(str::contains("Allocation"));
}

#[test]
fn test_context_outputs_camel_case_json() {
    let mut cmd = bin();
    cmd.args(["context", &example_data(), "--format", "json"]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(json.get("generatedAt").is_some());
    assert!(json.get("summary").is_some());
    assert!(json["summary"].get("totalValue").is_some());
    assert!(json.get("followUpCommands").is_some());
}

#[test]
fn test_review_outputs_camel_case_json() {
    let (_tmp, _dir, policy) = workspace_with_policy();

    let mut cmd = bin();
    cmd.args([
        "review",
        &example_data(),
        "--policy",
        policy.to_str().unwrap(),
        "--format",
        "json",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(json.get("portfolioValue").is_some());
    assert!(json.get("policyName").is_some());
    assert!(json.get("constraintChecks").is_some());
}

#[test]
fn test_review_missing_policy_fails_with_hint() {
    let mut cmd = bin();
    cmd.args([
        "review",
        &example_data(),
        "--policy",
        "/nonexistent/policy.toml",
    ]);
    cmd.assert().failure().stderr(str::contains("policy init"));
}

#[test]
fn test_simulate_outputs_camel_case_json() {
    let (_tmp, _dir, policy) = workspace_with_policy();

    let mut cmd = bin();
    cmd.args([
        "simulate",
        &example_data(),
        "--policy",
        policy.to_str().unwrap(),
        "--format",
        "json",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(json.get("portfolioValue").is_some());
    assert_eq!(json["scenarios"].as_array().unwrap().len(), 2);
}

#[test]
fn test_validate_example_data_is_valid() {
    let mut cmd = bin();
    cmd.args(["validate", &example_data()]);
    cmd.assert().success().stdout(str::contains("valid"));
}

#[test]
fn test_validate_invalid_json_exits_2() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("bad.json");
    std::fs::write(&file, "not json").unwrap();

    let mut cmd = bin();
    cmd.args(["validate", file.to_str().unwrap()]);
    cmd.assert().code(2);
}

#[test]
fn test_missing_portfolio_file_fails() {
    let mut cmd = bin();
    cmd.args(["context", "/nonexistent/positions.json"]);
    cmd.assert()
        .failure()
        .stderr(str::contains("failed to read file"));
}

// Parent commands without a subcommand must show usage and exit non-zero
// instead of panicking.
#[test]
fn test_parent_commands_require_subcommand() {
    for parent in ["policy", "decision", "report", "agent"] {
        let mut cmd = bin();
        cmd.arg(parent);
        cmd.assert().failure().stderr(str::contains("Usage"));
    }
}

#[test]
fn test_workspace_gitignore_protects_private_data() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-portfolio");

    let mut cmd = bin();
    cmd.args(["init-workspace", dir.to_str().unwrap()]);
    cmd.assert().success();

    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(gitignore.contains("positions.json"));
    assert!(gitignore.contains("/portfolio/"));
    assert!(gitignore.contains("/portfolio/history/"));
    assert!(gitignore.contains(".snapshots.jsonl.tmp-*"));
}

#[test]
fn test_decision_draft_refuses_overwrite() {
    let (_tmp, dir, _policy) = workspace_with_policy();

    let args = [
        "decision",
        "draft",
        "--dir",
        dir.to_str().unwrap(),
        "--title",
        "Test Decision",
    ];
    bin().args(args).assert().success();
    // Same title on the same day: must refuse to overwrite.
    bin()
        .args(args)
        .assert()
        .failure()
        .stderr(str::contains("already exists"));
}

#[test]
fn test_decision_draft_dry_run_previews_existing_file_without_erroring() {
    // A dry-run must only ever preview, never error — even when a real
    // decision record for the same title/day already exists. Regression
    // test: the dry-run guard used to run before the dry-run check.
    let (_tmp, dir, _policy) = workspace_with_policy();

    let args = [
        "decision",
        "draft",
        "--dir",
        dir.to_str().unwrap(),
        "--title",
        "Test Decision",
    ];
    bin().args(args).assert().success();

    bin()
        .args(args)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(str::contains("Dry-run"));
}

#[test]
fn test_decision_draft_dry_run_does_not_block_writes() {
    // A dry-run preview must not trip the overwrite guard on a subsequent write.
    let (_tmp, dir, _policy) = workspace_with_policy();

    let args = [
        "decision",
        "draft",
        "--dir",
        dir.to_str().unwrap(),
        "--title",
        "Test Decision",
    ];

    bin().args(args).arg("--dry-run").assert().success();
    bin().args(args).assert().success();
}

#[test]
fn test_report_weekly_refuses_overwrite() {
    let (_tmp, dir, _policy) = workspace_with_policy();

    let args = ["report", "weekly", "--dir", dir.to_str().unwrap()];
    bin().args(args).assert().success();
    // Same day: must refuse to overwrite.
    bin()
        .args(args)
        .assert()
        .failure()
        .stderr(str::contains("already exists"));
}

#[test]
fn test_report_weekly_dry_run_does_not_block_writes() {
    // A dry-run preview must not trip the overwrite guard on a subsequent write.
    let (_tmp, dir, _policy) = workspace_with_policy();

    bin()
        .args([
            "report",
            "weekly",
            "--dir",
            dir.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success();

    bin()
        .args(["report", "weekly", "--dir", dir.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn test_cli_never_touches_the_real_config() {
    let mut cmd = bin();
    cmd.args(["context", &example_data()]);
    cmd.assert().success();
    assert!(isolated_home()
        .join(".config/portfolio/config.yml")
        .is_file());
}

#[test]
fn test_context_with_fixture_quotes_is_deterministic() {
    let run = || {
        let mut cmd = bin();
        cmd.env(
            "PORTFOLIO_RS_QUOTES",
            format!("fixture:{}", quotes_fixture()),
        )
        .args(["context", &example_data(), "--format", "json"]);
        let output = cmd.assert().success().get_output().stdout.clone();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };
    let json = run();
    // Fixture prices in USD, FX 1.25 USD per EUR, base EUR:
    // BTC 0.45*60000 + SPY 50*500 + TLT 55*90 + GLD 10*200 + GSG 3*20
    // = 59010 USD = 47208 EUR, plus 50000 EUR cash.
    let total = json["summary"]["totalValue"].as_f64().unwrap();
    assert!((total - 97_208.0).abs() < 1e-6, "totalValue {total}");
    let mut again = run();
    again["generatedAt"] = json["generatedAt"].clone();
    assert_eq!(json, again);
}

#[test]
fn test_offline_quotes_keep_positions_without_prices() {
    let mut cmd = bin();
    cmd.args(["context", &example_data(), "--format", "json"]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let total = json["summary"]["totalValue"].as_f64().unwrap();
    assert!(
        (total - 50_000.0).abs() < 1e-6,
        "only cash is priced offline: {total}"
    );
}

#[test]
fn test_invalid_quotes_setting_falls_back_to_offline() {
    let mut cmd = bin();
    cmd.env("PORTFOLIO_RS_QUOTES", "fixture:/nonexistent/quotes.json")
        .args(["context", &example_data(), "--format", "json"]);
    cmd.assert()
        .success()
        .stderr(str::contains("using offline quotes"));
}

/// `portfolio_rs snapshot` writes one 0600 line under --data-dir (and the
/// default goes to the isolated XDG data dir, never the CWD).
#[test]
fn test_snapshot_command_writes_data_dir() {
    let tmp = TempDir::new().unwrap();
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/example_data.json");
    let mut cmd = bin();
    cmd.env(
        "PORTFOLIO_RS_QUOTES",
        format!("fixture:{}", quotes_fixture()),
    )
    .args([
        "snapshot",
        data,
        "--no-cache",
        "--portfolio",
        "demo",
        "--data-dir",
    ])
    .arg(tmp.path());
    cmd.assert().success();
    let path = tmp.path().join("demo/snapshots.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 1);
    let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["complete"], true, "{v}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

use crate::{
    api::{run_server, ServerConfig},
    commands::{
        agent::handle_agent,
        decision::handle_decision,
        doctor::handle_doctor,
        mcp::handle_mcp,
        policy::handle_policy,
        portfolio::{
            handle_allocation, handle_balances, handle_context, handle_performance, handle_sort,
            handle_tui,
        },
        report::handle_report,
        review::handle_review,
        simulate::handle_simulate,
        validate::handle_validate,
        workspace::handle_init_workspace,
    },
    config_path, load_config,
    tui::{Component, Tab},
};
use clap::{arg, Arg, ArgAction, ArgMatches, Command};
use eyre::{Result, WrapErr};

/// `[FILE]` positional arg for the portfolio JSON file, reused by nearly
/// every subcommand (falls back to the config file when omitted).
fn file_arg() -> Arg {
    Arg::new("FILE").help("Portfolio data file (uses config file if not specified)")
}

/// `--policy <FILE>` option pointing at a `policy.toml`, defaulting to the
/// standard workspace location. Callers override `.help(...)` for
/// command-specific wording.
fn policy_arg() -> Arg {
    Arg::new("policy")
        .long("policy")
        .value_name("FILE")
        .default_value("portfolio/policy.toml")
        .help("Path to policy.toml")
}

/// `--format <FORMAT>` option restricted to `markdown`/`json`, defaulting to
/// `markdown`. Callers override `.help(...)` for command-specific wording.
fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .value_name("FORMAT")
        .default_value("markdown")
        .value_parser(["markdown", "json"])
        .help("Output format")
}

/// `--dry-run` flag shared by every command that mutates the filesystem, so
/// previews are spelled and behave identically everywhere.
fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .help("Show what would be created without making changes")
        .action(ArgAction::SetTrue)
}

/// `[DIR]` positional arg for a workspace-rooted directory, with `default`.
fn dir_arg(default: &'static str) -> Arg {
    Arg::new("DIR").default_value(default)
}

/// `--dir <DIR>` option for a workspace directory, with `default`.
fn dir_option_arg(default: &'static str) -> Arg {
    Arg::new("dir")
        .long("dir")
        .value_name("DIR")
        .default_value(default)
}

pub fn build_cli() -> Command {
    Command::new("portfolio_rs")
        .about("Local-first portfolio management for humans and agents — policy-aware reviews, simulations, and durable financial memory")
        .long_about(
            "portfolio_rs is a terminal-native tool for managing investment portfolios.\n\
            \n\
            It provides:\n\
            - Interactive TUI for visual portfolio management\n\
            - CLI commands for agent-friendly analysis\n\
            - Investment policy templates and validation\n\
            - Durable financial memory (diary, decisions, reports)\n\
            \n\
            Quick start:\n\
            portfolio_rs init-workspace my-portfolio\n\
            portfolio_rs policy init --strategy balanced-growth my-portfolio\n\
            portfolio_rs context my-portfolio/positions.json\n\
            \n\
            Run 'portfolio_rs --help' for all commands."
        )
        .author("Markus Zoppelt")
        .arg(file_arg())
        .arg(
            arg!(--tab <TAB> "Tab to open at start")
                .default_value("overview")
                .help("Specify the tab to open at start (overview/balances)"),
        )
        .arg(
            arg!(--disable <COMPONENTS> "Disable specific TUI components")
                .help("Comma-separated list of components to disable.")
                .value_delimiter(',')
                .action(clap::ArgAction::Append),
        )
        .subcommand(
            Command::new("config")
                .about("Print the path to the configuration file")
                .long_about("Shows where portfolio_rs stores its config file.\n\
                    Useful for setting the default portfolio_file path."),
        )
        .subcommand(
            Command::new("components")
                .about("List all available TUI components that can be disabled")
                .long_about("List TUI components you can hide with the --disable flag.\n\
                    Components: tab_bar, total_value, asset_allocation, detailed_allocation,\n\
                    help, name, asset_class, amount, balance."),
        )
        .subcommand(
            Command::new("balances")
                .about("Show the current balances of your portfolio")
                .long_about("Display a table of all positions with amounts, prices, values, PnL, and returns.")
                .arg(file_arg()),
        )
        .subcommand(
            Command::new("allocation")
                .about("Show the current allocation of your portfolio")
                .long_about("Display asset class allocation as a pie chart and percentage breakdown.")
                .arg(file_arg()),
        )
        .subcommand(
            Command::new("performance")
                .about("Show the performance of your portfolio")
                .long_about("Display historical performance metrics including YTD and total returns.")
                .arg(file_arg()),
        )
        .subcommand(
            Command::new("context")
                .about("Show an agent-friendly portfolio briefing")
                .long_about(
                    "Generate a comprehensive portfolio briefing for humans and AI agents.\n\
                    \n\
                    Markdown output is readable and paste-friendly for LLMs.\n\
                    JSON output is structured for scripts and coding agents.\n\
                    \n\
                    Includes: summary, allocation, positions, risk flags,\n\
                    data-quality flags, and follow-up command suggestions."
                )
                .arg(file_arg())
                .arg(format_arg().help("Output format for the context briefing")),
        )
        .subcommand(
            Command::new("agent")
                .about("Agent instruction and skill management")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .long_about(
                    "Create workspace instructions and export a portable agent skill.\n\
                    \n\
                    - agent init: creates AGENTS.md and CLAUDE.md in a workspace\n\
                    - agent skill show: prints the built-in skill to stdout\n\
                    - agent skill export: writes the skill to an agent harness directory\n\
                    - agent skill path: shows how to access the built-in skill\n\
                    \n\
                    The skill is target-agnostic: install it into any agent harness\n\
                    that supports skill files (opencode, Claude Code, Cursor, etc.)."
                )
                .subcommand(
                    Command::new("init")
                        .about("Create agent instruction files in a workspace")
                        .long_about(
                            "Generate AGENTS.md and CLAUDE.md in the target directory.\n\
                            \n\
                            These files contain local workspace facts:\n\
                            - Local file paths and layout\n\
                            - Safety rules (no trades, no broker interaction)\n\
                            - How to install the portable skill\n\
                            \n\
                            Existing files are never overwritten."
                        )
                        .arg(dir_arg(".").help("Target workspace directory"))
                        .arg(dry_run_arg()),
                )
                .subcommand(
                    Command::new("skill")
                        .about("Manage the built-in portable agent skill")
                        .subcommand_required(true)
                        .arg_required_else_help(true)
                        .subcommand(
                            Command::new("show")
                                .about("Print the built-in skill to stdout")
                                .long_about(
                                    "Print the full portfolio-rs skill content.\n\
                                    \n\
                                    Redirect to a file or pipe into your agent harness:\n\
                                    portfolio_rs agent skill show > ~/.some-agent/skills/portfolio-rs/SKILL.md"
                                ),
                        )
                        .subcommand(
                            Command::new("export")
                                .about("Write the skill to a directory")
                                .long_about(
                                    "Export the built-in skill to a directory of your choice.\n\
                                    \n\
                                    Creates: <DIR>/portfolio-rs/SKILL.md\n\
                                    \n\
                                    Examples:\n\
                                    portfolio_rs agent skill export ~/.config/opencode/skills\n\
                                    portfolio_rs agent skill export ~/.claude/skills\n\
                                    portfolio_rs agent skill export ./vendor/skills"
                                )
                                .arg(dir_arg(".").help("Target directory"))
                                .arg(dry_run_arg()),
                        )
                        .subcommand(
                            Command::new("path")
                                .about("Show how to access the built-in skill")
                                .long_about(
                                    "The skill is built into portfolio_rs.\n\
                                    Use 'skill show' to view it or 'skill export' to install it."
                                ),
                        ),
                ),
        )
        .subcommand(
            Command::new("init-workspace")
                .about("Create a new finance workspace")
                .long_about(
                    "Initialize a durable finance workspace with directories\n\
                    and templates for diary, decisions, theses, and reports.\n\
                    \n\
                    Also creates AGENTS.md and CLAUDE.md so coding agents\n\
                    can operate in the workspace immediately.\n\
                    \n\
                    Use --dry-run to preview what would be created.\n\
                    Existing files are never overwritten."
                )
                .arg(dir_arg("portfolio").help("Target directory for the workspace"))
                .arg(dry_run_arg()),
        )
        .subcommand(
            Command::new("policy")
                .about("Investment policy management")
                .long_about(
                    "Create and validate machine-readable investment policies.\n\
                    \n\
                    Policies define your financial goals, risk tolerance,\n\
                    target allocations, and constraints. They enable\n\
                    policy-aware portfolio reviews and guided decision-making."
                )
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("init")
                        .about("Create a policy.toml from a strategy template")
                        .long_about(
                            "Generate a policy.toml file from a predefined strategy template.\n\
                            \n\
                            Strategies:\n\
                            - balanced-growth: Moderate risk, 15yr horizon\n\
                            - capital-preservation: Conservative, 5yr horizon\n\
                            - aggressive-growth: High risk, 20yr horizon\n\
                            - custom: Empty template for manual configuration"
                        )
                        .arg(
                            arg!(--strategy <STRATEGY> "Strategy template to use")
                                .value_parser(["balanced-growth", "capital-preservation", "aggressive-growth", "custom"])
                                .default_value("balanced-growth"),
                        )
                        .arg(dir_arg("portfolio").help("Target workspace directory"))
                        .arg(dry_run_arg()),
                )
                .subcommand(
                    Command::new("validate")
                        .about("Validate a policy.toml file")
                        .long_about(
                            "Check that a policy.toml file is valid and internally consistent.\n\
                            Validates: version, allocation sums to 100%, positive targets,\n\
                            and required fields."
                        )
                        .arg(
                            file_arg()
                                .default_value("portfolio/policy.toml")
                                .help("Path to policy.toml"),
                        ),
                ),
        )
        .subcommand(
            Command::new("review")
                .about("Policy-aware portfolio review")
                .long_about(
                    "Compare your portfolio against your investment policy.\n\
                    \n\
                    Identifies allocation drift, constraint violations,\n\
                    missing data, and suggests corrective actions.\n\
                    \n\
                    Requires: a portfolio JSON file and a policy.toml file."
                )
                .arg(file_arg())
                .arg(policy_arg().help("Investment policy file to compare against"))
                .arg(format_arg()),
        )
        .subcommand(
            Command::new("simulate")
                .about("Simulate portfolio rebalancing scenarios")
                .long_about(
                    "Run what-if scenarios for portfolio rebalancing.\n\
                    \n\
                    Compares current allocation against policy targets and\n\
                    simulates the trades needed to restore alignment.\n\
                    \n\
                    No trades are executed. This is purely analytical."
                )
                .arg(file_arg())
                .arg(policy_arg().help("Investment policy to simulate against"))
                .arg(format_arg()),
        )
        .subcommand(
            Command::new("decision")
                .about("Manage investment decisions")
                .long_about("Create and manage structured decision records for your portfolio.")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("draft")
                        .about("Draft a decision record from a review or manually")
                        .long_about(
                            "Generate a structured decision record markdown file.\n\
                            \n\
                            Can be created from a policy review or from scratch.\n\
                            Decision records help you document why you made a change\n\
                            and provide a basis for future review."
                        )
                        .arg(file_arg())
                        .arg(policy_arg().help("Policy file to base decision on"))
                        .arg(
                            arg!(--title <TITLE> "Decision title")
                                .help("Short title for the decision"),
                        )
                        .arg(dir_option_arg("portfolio").help("Directory containing portfolio/decisions/"))
                        .arg(dry_run_arg()),
                ),
        )
        .subcommand(
            Command::new("report")
                .about("Generate portfolio reports")
                .long_about("Generate periodic portfolio reports with review findings, allocation, and actions.")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("weekly")
                        .about("Generate a weekly portfolio report")
                        .long_about(
                            "Create a structured weekly Markdown report file\n\
                            with portfolio status, policy alignment, and open decisions."
                        )
                        .arg(file_arg())
                        .arg(policy_arg().help("Policy file to compare against"))
                        .arg(dir_option_arg("portfolio").help("Directory containing portfolio/reports/"))
                        .arg(dry_run_arg()),
                ),
        )
        .subcommand(
            Command::new("doctor")
                .about("Check workspace health and completeness")
                .long_about(
                    "Diagnose your finance workspace for common issues:\n\
                    \n\
                    - Missing policy files\n\
                    - Missing workspace directories\n\
                    - Unencrypted sensitive files\n\
                    - Git safety issues\n\
                    - Portfolio data problems"
                )
                .arg(dir_arg("portfolio").help("Target workspace directory")),
        )
        .subcommand(
            Command::new("validate")
                .about("Validate a portfolio JSON file")
                .long_about(
                    "Check that a portfolio JSON file is valid and well-formed.\n\
                    Validates: required fields, positive amounts, known asset classes,\n\
                    and data quality warnings."
                )
                .arg(file_arg().help("Portfolio data file to validate")),
        )
        .subcommand(
            Command::new("api")
                .about("Start the local HTTP API server")
                .long_about(
                    "Start a local JSON-over-HTTP server exposing the portfolio.\n\
                    \n\
                    Endpoints include portfolio summary, positions CRUD,\n\
                    allocation, performance, context, review, simulate,\n\
                    validate, and doctor.\n\
                    \n\
                    The server has NO authentication and is intended for\n\
                    local use only (scripts, agents, GUIs on this machine).\n\
                    Keep the default 127.0.0.1 bind unless you know what\n\
                    you are doing.\n\
                    \n\
                    Position mutations are persisted back to the portfolio\n\
                    file, except for .gpg files, which are never rewritten."
                )
                .arg(
                    arg!(--host <HOST> "Host to bind to")
                        .default_value("127.0.0.1")
                        .help("Host for API server (local use only; no authentication)"),
                )
                .arg(
                    arg!(-p --port <PORT> "Port to bind to")
                        .default_value("3000")
                        .value_parser(clap::value_parser!(u16))
                        .help("Port for API server"),
                )
                .arg(
                    Arg::new("policy")
                        .long("policy")
                        .value_name("FILE")
                        .help("Investment policy file (enables /api/review and /api/simulate)"),
                )
                .arg(file_arg()),
        )
        .subcommand(
            Command::new("mcp")
                .about("Start the MCP server for agent integration (experimental)")
                .long_about(
                    "Start an experimental Model Context Protocol (MCP) server\n\
                    that communicates via JSON-RPC over stdio.\n\
                    \n\
                    This is a preview: tools/list advertises\n\
                    get_portfolio_snapshot, get_allocation,\n\
                    get_context_markdown, and get_investment_policy,\n\
                    but the tools currently return pointers to the\n\
                    equivalent CLI commands instead of live data.\n\
                    \n\
                    For structured data today, prefer:\n\
                    portfolio_rs context <FILE> --format json"
                ),
        )
        .subcommand(
            Command::new("sort")
                .about("Display positions sorted by current value")
                .long_about("Show positions sorted by current market value in descending order.\n\
                    This is display-only and does not modify your data file.")
                .arg(file_arg()),
        )
}

pub fn get_arg_value(matches: Option<&ArgMatches>, arg_name: &str) -> Option<String> {
    matches.and_then(|m| m.get_one::<String>(arg_name).map(|s| s.to_string()))
}

pub fn parse_tab(tab_str: Option<String>) -> Option<Tab> {
    match tab_str {
        Some(s) => Tab::from_str(&s).or(Some(Tab::Overview)),
        None => Some(Tab::Overview),
    }
}

/// Entry point for the CLI — parses arguments and dispatches to subcommands.
pub async fn run() -> Result<()> {
    use crate::{config::AppConfig, load_portfolio_file};

    let cfg: AppConfig = load_config()?;

    let matches = build_cli().get_matches();

    let disabled_components: Vec<String> = matches
        .get_many::<String>("disable")
        .unwrap_or_default()
        .cloned()
        .collect();

    if matches.subcommand_matches("config").is_some() {
        let config_path = config_path()?;
        println!("Your config file is located here: \n{}", config_path);
        return Ok(());
    }

    if matches.subcommand_matches("components").is_some() {
        println!("Available TUI components that can be disabled:\n");

        let components = Component::all();
        let max_width = components
            .iter()
            .map(|c| c.as_str().len())
            .max()
            .unwrap_or(0);

        for component in components {
            println!(
                "  {:width$} - {}",
                component.as_str(),
                component.description(),
                width = max_width
            );
        }

        println!("\nExample usage:");
        println!("  portfolio_rs --disable tab_bar,help");
        println!("  portfolio_rs example_data.json --disable tab_bar,help");
        return Ok(());
    }

    match matches.subcommand() {
        Some(("balances", sub_matches)) => handle_balances(sub_matches, &cfg).await?,
        Some(("allocation", sub_matches)) => handle_allocation(sub_matches, &cfg).await?,
        Some(("performance", sub_matches)) => handle_performance(sub_matches, &cfg).await?,
        Some(("context", sub_matches)) => handle_context(sub_matches, &cfg).await?,
        Some(("sort", sub_matches)) => handle_sort(sub_matches, &cfg).await?,
        Some(("review", sub_matches)) => handle_review(sub_matches, &cfg).await?,
        Some(("simulate", sub_matches)) => handle_simulate(sub_matches, &cfg).await?,
        Some(("policy", sub_matches)) => handle_policy(sub_matches)?,
        Some(("init-workspace", sub_matches)) => handle_init_workspace(sub_matches)?,
        Some(("agent", sub_matches)) => handle_agent(sub_matches)?,
        Some(("decision", sub_matches)) => handle_decision(sub_matches, &cfg).await?,
        Some(("report", sub_matches)) => handle_report(sub_matches, &cfg).await?,
        Some(("doctor", sub_matches)) => handle_doctor(sub_matches)?,
        Some(("validate", sub_matches)) => handle_validate(sub_matches, &cfg).await?,
        Some(("mcp", _sub_matches)) => handle_mcp(),
        Some(("api", sub_matches)) => {
            let host =
                get_arg_value(Some(sub_matches), "host").unwrap_or_else(|| "127.0.0.1".to_string());
            let port = sub_matches.get_one::<u16>("port").copied().unwrap_or(3000);
            let filename = get_arg_value(Some(sub_matches), "FILE")
                .or_else(|| cfg.effective_portfolio_file())
                .unwrap_or_default();
            let policy_file = get_arg_value(Some(sub_matches), "policy");

            let positions_str = load_portfolio_file(&filename)?;
            run_server(ServerConfig {
                host,
                port,
                positions_json: positions_str,
                file_path: Some(filename).filter(|s| !s.is_empty()),
                policy_file,
                currency: cfg.currency.clone(),
            })
            .await?;
        }
        _ => {
            // Default: launch the interactive TUI. On failure, show the error
            // followed by usage help, and exit non-zero.
            if let Err(e) = handle_tui(&matches, &cfg, disabled_components).await {
                eprintln!("{:#}\n", e);
                build_cli()
                    .print_help()
                    .wrap_err("failed to print help message")?;
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    #[test]
    fn test_cli() {
        let matches =
            build_cli().get_matches_from(vec!["portfolio_rs", "balances", "example_data.json"]);
        assert_eq!(matches.subcommand_name(), Some("balances"));
    }

    #[test]
    fn test_cli_with_tab_flag() {
        let matches = build_cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_with_file_and_tab() {
        let matches =
            build_cli().get_matches_from(vec!["portfolio_rs", "data.json", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "FILE"),
            Some("data.json".to_string())
        );
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_tab_order_independence() {
        let matches =
            build_cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances", "data.json"]);
        assert_eq!(
            get_arg_value(Some(&matches), "FILE"),
            Some("data.json".to_string())
        );
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_default_tab() {
        let matches = build_cli().get_matches_from(vec!["portfolio_rs", "data.json"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("overview".to_string())
        );
    }

    #[test]
    fn test_parse_tab_overview() {
        let result = parse_tab(Some("overview".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_balance() {
        let result = parse_tab(Some("balances".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Balances));
    }

    #[test]
    fn test_parse_tab_case_insensitive() {
        assert_eq!(
            parse_tab(Some("OVERVIEW".to_string())),
            Some(crate::tui::Tab::Overview)
        );
        assert_eq!(
            parse_tab(Some("Balances".to_string())),
            Some(crate::tui::Tab::Balances)
        );
        assert_eq!(
            parse_tab(Some("bAlAnCeS".to_string())),
            Some(crate::tui::Tab::Balances)
        );
    }

    #[test]
    fn test_parse_tab_invalid_defaults_to_overview() {
        let result = parse_tab(Some("invalid".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_none_defaults_to_overview() {
        let result = parse_tab(None);
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_empty_string_defaults_to_overview() {
        let result = parse_tab(Some("".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_get_arg_value_existing() {
        let matches = build_cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_get_arg_value_missing() {
        let matches = build_cli().get_matches_from(vec!["portfolio_rs"]);
        assert_eq!(get_arg_value(Some(&matches), "FILE"), None);
    }

    #[test]
    fn test_get_arg_value_none_matches() {
        assert_eq!(get_arg_value(None, "tab"), None);
    }

    #[test]
    fn test_cli_disable_argument() {
        let matches = build_cli().get_matches_from(vec![
            "portfolio_rs",
            "--disable",
            "tab_bar,total_value",
            "balances",
            "example_data.json",
        ]);
        let disabled_components: Vec<String> = matches
            .get_many::<String>("disable")
            .unwrap_or_default()
            .cloned()
            .collect();
        assert_eq!(disabled_components, vec!["tab_bar", "total_value"]);
    }

    #[test]
    fn test_disabled_components_parsing() {
        use crate::tui::Component;
        let disabled = crate::tui::DisabledComponents::new(vec![
            "tab_bar".to_string(),
            "total_value".to_string(),
            "name".to_string(),
        ])
        .unwrap();
        assert!(disabled.is_disabled(Component::TabBar));
        assert!(disabled.is_disabled(Component::TotalValue));
        assert!(disabled.is_disabled(Component::Name));
        assert!(!disabled.is_disabled(Component::AssetBreakdown));
        assert!(!disabled.is_disabled(Component::Help));
    }

    #[test]
    fn test_component_enum_from_string() {
        use crate::tui::Component;
        use std::str::FromStr;

        assert_eq!(Component::from_str("tab_bar").unwrap(), Component::TabBar);
        assert_eq!(
            Component::from_str("total_value").unwrap(),
            Component::TotalValue
        );
        assert_eq!(Component::from_str("HELP").unwrap(), Component::Help);
        assert_eq!(Component::from_str("  name  ").unwrap(), Component::Name);

        assert!(Component::from_str("invalid_component").is_err());
    }

    #[test]
    fn test_component_enum_as_str() {
        use crate::tui::Component;

        assert_eq!(Component::TabBar.as_str(), "tab_bar");
        assert_eq!(Component::TotalValue.as_str(), "total_value");
        assert_eq!(Component::Help.as_str(), "help");
        assert_eq!(Component::Name.as_str(), "name");
    }

    #[test]
    fn test_disabled_components_with_enum() {
        use crate::tui::{Component, DisabledComponents};

        let mut disabled = DisabledComponents::default();
        disabled.disable_component(Component::TabBar);
        disabled.disable_component(Component::Help);
        disabled.disable_component(Component::PortfolioGrowth);
        disabled.disable_component(Component::AssetBreakdown);
        disabled.disable_component(Component::DetailedAllocation);

        assert!(disabled.is_disabled(Component::TabBar));
        assert!(disabled.is_disabled(Component::Help));
        assert!(disabled.is_disabled(Component::PortfolioGrowth));
        assert!(disabled.is_disabled(Component::AssetBreakdown));
        assert!(disabled.is_disabled(Component::DetailedAllocation));
        assert!(!disabled.is_disabled(Component::TotalValue));
        assert!(!disabled.is_disabled(Component::Name));
    }

    #[tokio::test]
    async fn test_create_live_portfolio() {
        let positions_str = read_to_string("example_data.json").unwrap();
        let (portfolio, _network_status) = crate::create_live_portfolio(positions_str).await;
        assert!(!portfolio.positions.is_empty());
    }

    #[test]
    fn test_disabled_components_backward_compatibility() {
        use crate::tui::{Component, DisabledComponents};

        let disabled = DisabledComponents::new(vec!["asset_allocation".to_string()]).unwrap();

        assert!(disabled.is_disabled(Component::AssetBreakdown));
    }

    #[test]
    fn test_disabled_components_error_handling() {
        use crate::tui::DisabledComponents;

        let result = DisabledComponents::new(vec![
            "portfolio_growth".to_string(),
            "invalid_component".to_string(),
            "asset_breakdown".to_string(),
            "another_invalid".to_string(),
        ]);

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(errors.contains(&"Unknown component: 'invalid_component'".to_string()));
        assert!(errors.contains(&"Unknown component: 'another_invalid'".to_string()));
    }
}

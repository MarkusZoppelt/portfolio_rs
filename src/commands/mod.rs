//! Handlers for the CLI subcommands.
//!
//! Each `handle_*` function takes parsed [`clap::ArgMatches`] plus the loaded
//! [`crate::AppConfig`] and returns an [`eyre::Result`], so failures propagate
//! to `main` and produce a non-zero exit code.

pub mod agent;
pub mod api;
pub mod decision;
pub mod doctor;
pub mod mcp;
pub mod policy;
pub mod portfolio;
pub mod report;
pub mod review;
pub mod simulate;
pub mod validate;
pub mod workspace;

use clap::ArgMatches;
use eyre::{Result, WrapErr};

use crate::policy::Policy;
use crate::services::portfolio_loader::{create_live_portfolio_with_logging, load_portfolio_file};
use crate::{AppConfig, NetworkStatus, Portfolio};

/// Load the positions text and refuse to go on when it does not parse.
///
/// Prints `Error parsing positions in <file>: <serde error>` (line and column
/// refer to the file as written) and exits with code 2, like `validate`, so
/// no command shows an empty portfolio for a broken file.
pub(crate) fn load_positions_checked(filename: &str) -> eyre::Result<String> {
    let text = load_portfolio_file(filename)?;
    exit_if_unparsable(filename, &text);
    Ok(text)
}

/// [`load_positions_checked`] for commands that tolerate a missing file
/// (report/decision drafts): only a file that reads but does not parse exits.
pub(crate) fn exit_if_file_unparsable(filename: &str) {
    if filename.is_empty() {
        return;
    }
    if let Ok(text) = load_portfolio_file(filename) {
        exit_if_unparsable(filename, &text);
    }
}

fn exit_if_unparsable(filename: &str, text: &str) {
    if let Err(e) = serde_json::from_str::<Vec<crate::position::PortfolioPosition>>(text) {
        eprintln!("Error parsing positions in {filename}: {e}");
        std::process::exit(2);
    }
}

/// Resolve the portfolio file: an explicit `FILE` argument wins, otherwise
/// fall back to the configured portfolio file (workspace-aware).
pub(crate) fn resolve_portfolio_file(matches: &ArgMatches, cfg: &AppConfig) -> String {
    matches
        .get_one::<String>("FILE")
        .filter(|f| !f.is_empty())
        .cloned()
        .or_else(|| cfg.effective_portfolio_file())
        .unwrap_or_default()
}

/// A string argument with a `default_value` in the CLI definition, so clap
/// always provides it.
pub(crate) fn defaulted_arg<'a>(matches: &'a ArgMatches, name: &str) -> &'a str {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .expect("argument has a default value")
}

/// Load the resolved portfolio file and value it with live quotes, logging
/// quote errors.
pub(crate) async fn load_live_portfolio(
    matches: &ArgMatches,
    cfg: &AppConfig,
) -> Result<(Portfolio, NetworkStatus)> {
    let filename = resolve_portfolio_file(matches, cfg);
    let positions_str = load_positions_checked(&filename)?;
    Ok(create_live_portfolio_with_logging(positions_str, true).await)
}

/// Load the `--policy` file, hinting at `policy init` when that fails.
pub(crate) fn load_policy(matches: &ArgMatches) -> Result<Policy> {
    let policy_file = defaulted_arg(matches, "policy");
    Policy::from_file(policy_file).wrap_err_with(|| {
        format!(
            "failed to load policy '{policy_file}' (hint: run 'portfolio_rs policy init' to create one)"
        )
    })
}

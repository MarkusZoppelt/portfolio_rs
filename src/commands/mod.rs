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
pub mod tui_io;
pub mod validate;
pub mod workspace;

use clap::ArgMatches;
use eyre::{Result, WrapErr};

use crate::policy::Policy;
use crate::services::portfolio_loader::{
    check_positions, create_live_portfolio_with_logging, load_portfolio_file,
};
use crate::{AppConfig, NetworkStatus, Portfolio};

/// Load the positions text, failing with a
/// [`PositionsParseError`](crate::services::portfolio_loader::PositionsParseError)
/// when it does not parse, so no command shows an empty portfolio for a
/// broken file. `cli::run` turns that error into exit code 2.
pub(crate) fn load_positions_checked(filename: &str) -> Result<String> {
    let text = load_portfolio_file(filename)?;
    check_positions(filename, &text)?;
    Ok(text)
}

/// For drafts that tolerate a missing or unreadable file (report, decision):
/// only a file that reads but does not parse is an error.
pub(crate) fn check_positions_file(filename: &str) -> Result<()> {
    if filename.is_empty() {
        return Ok(());
    }
    match load_portfolio_file(filename) {
        Ok(text) => Ok(check_positions(filename, &text)?),
        Err(_) => Ok(()),
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

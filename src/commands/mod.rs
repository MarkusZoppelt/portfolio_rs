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
    let positions_str = load_portfolio_file(&filename)?;
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

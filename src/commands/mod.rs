//! Handlers for the CLI subcommands.
//!
//! Each `handle_*` function takes parsed [`clap::ArgMatches`] plus the loaded
//! [`crate::AppConfig`] and returns an [`eyre::Result`], so failures propagate
//! to `main` and produce a non-zero exit code.

pub mod agent;
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

use crate::cli::get_arg_value;
use crate::AppConfig;

/// Resolve the portfolio file: an explicit `FILE` argument wins, otherwise
/// fall back to the configured portfolio file (workspace-aware).
pub(crate) fn resolve_portfolio_file(matches: Option<&ArgMatches>, cfg: &AppConfig) -> String {
    get_arg_value(matches, "FILE")
        .filter(|f| !f.is_empty())
        .or_else(|| cfg.effective_portfolio_file())
        .unwrap_or_default()
}

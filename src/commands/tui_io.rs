//! Launch the interactive TUI (default when no subcommand is given).
//!
//! Excluded from coverage (`*_io.rs`) because it cannot run under
//! `cargo test`: it picks the process-wide quote provider (Yahoo unless
//! `PORTFOLIO_RS_QUOTES` says otherwise) and hands the terminal to
//! [`crate::tui::run_tui`] (raw mode, alternate screen, key-event loop).
//! Everything before that is [`prepare_tui`], which is tested.

use std::sync::Arc;

use clap::ArgMatches;
use eyre::Result;

use crate::commands::portfolio::prepare_tui;
use crate::config::configured_base_currency;
use crate::{tui, AnyProvider, AppConfig};

pub async fn handle_tui(
    matches: &ArgMatches,
    cfg: &AppConfig,
    disabled_components: Vec<String>,
) -> Result<()> {
    let provider = Arc::new(AnyProvider::configured().clone());
    let t = prepare_tui(matches, cfg, provider, &configured_base_currency()).await?;
    tui::run_tui(
        t.portfolio,
        t.currency,
        t.positions_str,
        t.filename,
        t.tab,
        disabled_components,
    )
    .await
}

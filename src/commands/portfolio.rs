use std::sync::Arc;

use clap::ArgMatches;
use eyre::Result;

use crate::cli::{get_arg_value, parse_tab};
use crate::commands::{defaulted_arg, load_live_portfolio, resolve_portfolio_file};
use crate::context::{ContextOutputFormat, PortfolioContext};
use crate::services::persistence::store_balance_in_db;
use crate::services::portfolio_loader::create_live_portfolio_with;
use crate::services::quotes::QuoteProvider;
use crate::tui::Tab;
use crate::{AppConfig, Portfolio};

pub async fn handle_balances(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (mut portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    portfolio.sort_positions_by_value_desc();
    portfolio.print(true);
    if let Err(e) = store_balance_in_db(&portfolio) {
        eprintln!("Warning: failed to store balance in database: {e:#}");
    }
    Ok(())
}

pub async fn handle_allocation(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (mut portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    portfolio.sort_positions_by_value_desc();
    portfolio.draw_pie_chart();
    portfolio.print_allocation();
    Ok(())
}

pub async fn handle_performance(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (mut portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    portfolio.sort_positions_by_value_desc();
    portfolio.print_performance().await;
    Ok(())
}

pub async fn handle_context(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (mut portfolio, network_status) = load_live_portfolio(sub_matches, cfg).await?;
    portfolio.sort_positions_by_value_desc();

    let format = ContextOutputFormat::parse(defaulted_arg(sub_matches, "format"))
        .unwrap_or(ContextOutputFormat::Markdown);
    let context =
        PortfolioContext::from_portfolio(&portfolio, &cfg.currency, &format!("{network_status:?}"));

    match format {
        ContextOutputFormat::Markdown => println!("{}", context.to_markdown()),
        ContextOutputFormat::Json => println!("{}", context.to_json()?),
    }
    Ok(())
}

pub async fn handle_sort(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (mut portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    portfolio.sort_positions_by_value_desc();
    println!("Positions sorted by current value (display only, file unchanged):");
    portfolio.print(true);
    Ok(())
}

/// Everything the interactive TUI starts with.
#[derive(Debug)]
pub struct TuiLaunch {
    pub portfolio: Portfolio,
    pub currency: String,
    pub positions_str: String,
    pub filename: String,
    pub tab: Option<Tab>,
}

/// Resolve, load and value the portfolio for the TUI (default when no
/// subcommand is given), with quotes and FX from `provider` in
/// `base_currency`. Launching the terminal itself lives in
/// [`crate::commands::tui_io`].
pub async fn prepare_tui<P: QuoteProvider>(
    matches: &ArgMatches,
    cfg: &AppConfig,
    provider: Arc<P>,
    base_currency: &str,
) -> Result<TuiLaunch> {
    let filename = resolve_portfolio_file(matches, cfg);
    let tab = parse_tab(get_arg_value(Some(matches), "tab"));

    let positions_str = crate::commands::load_positions_checked(&filename)?;
    let (mut portfolio, _network_status) =
        create_live_portfolio_with(provider, positions_str.clone(), base_currency, false).await;
    portfolio.sort_positions_by_value_desc();
    Ok(TuiLaunch {
        portfolio,
        currency: cfg.currency.clone(),
        positions_str,
        filename,
        tab,
    })
}

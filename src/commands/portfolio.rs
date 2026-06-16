use clap::ArgMatches;
use eyre::Result;

use crate::cli::{get_arg_value, parse_tab};
use crate::commands::resolve_portfolio_file;
use crate::context::{ContextOutputFormat, PortfolioContext};
use crate::services::persistence::store_balance_in_db;
use crate::services::portfolio_loader::{
    create_live_portfolio, create_live_portfolio_with_logging, load_portfolio_file,
};
use crate::tui;
use crate::AppConfig;

pub async fn handle_balances(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, _network_status) =
        create_live_portfolio_with_logging(positions_str, true).await;
    portfolio.sort_positions_by_value_desc();
    portfolio.print(true);
    if let Err(e) = store_balance_in_db(&portfolio) {
        eprintln!("Warning: failed to store balance in database: {:#}", e);
    }
    Ok(())
}

pub async fn handle_allocation(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, _network_status) =
        create_live_portfolio_with_logging(positions_str, true).await;
    portfolio.sort_positions_by_value_desc();
    portfolio.draw_pie_chart();
    portfolio.print_allocation();
    Ok(())
}

pub async fn handle_performance(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, _network_status) =
        create_live_portfolio_with_logging(positions_str, true).await;
    portfolio.sort_positions_by_value_desc();
    portfolio.print_performance().await;
    Ok(())
}

pub async fn handle_context(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, network_status) =
        create_live_portfolio_with_logging(positions_str, true).await;
    portfolio.sort_positions_by_value_desc();

    let format = sub_matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("markdown");
    let format = ContextOutputFormat::parse(format).unwrap_or(ContextOutputFormat::Markdown);
    let context =
        PortfolioContext::from_portfolio(&portfolio, &cfg.currency, &format!("{network_status:?}"));

    match format {
        ContextOutputFormat::Markdown => println!("{}", context.to_markdown()),
        ContextOutputFormat::Json => println!("{}", context.to_json()?),
    }
    Ok(())
}

pub async fn handle_sort(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, _status) = create_live_portfolio_with_logging(positions_str, true).await;
    portfolio.sort_positions_by_value_desc();
    println!("Positions sorted by current value (display only, file unchanged):");
    portfolio.print(true);
    Ok(())
}

/// Launch the interactive TUI (default when no subcommand is given).
pub async fn handle_tui(
    matches: &ArgMatches,
    cfg: &AppConfig,
    disabled_components: Vec<String>,
) -> Result<()> {
    let filename = resolve_portfolio_file(Some(matches), cfg);
    let tab_value = parse_tab(get_arg_value(Some(matches), "tab"));

    let positions_str = load_portfolio_file(&filename)?;
    let (mut portfolio, _network_status) = create_live_portfolio(positions_str.clone()).await;
    portfolio.sort_positions_by_value_desc();
    tui::run_tui(
        portfolio,
        cfg.currency.clone(),
        positions_str,
        filename,
        tab_value,
        disabled_components,
    )
    .await
}

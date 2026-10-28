use clap::ArgMatches;
use eyre::Result;

use crate::commands::{defaulted_arg, load_live_portfolio, load_policy};
use crate::simulate::simulate_rebalance;
use crate::AppConfig;

pub async fn handle_simulate(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    let policy = load_policy(sub_matches)?;
    let sim = simulate_rebalance(&portfolio, &policy, &cfg.currency);

    match defaulted_arg(sub_matches, "format") {
        "json" => println!("{}", serde_json::to_string_pretty(&sim)?),
        _ => println!("{}", sim.to_markdown()),
    }
    Ok(())
}

use clap::ArgMatches;
use eyre::{Result, WrapErr};

use crate::commands::resolve_portfolio_file;
use crate::policy::Policy;
use crate::services::portfolio_loader::{create_live_portfolio_with_logging, load_portfolio_file};
use crate::simulate::simulate_rebalance;
use crate::AppConfig;

pub async fn handle_simulate(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    let policy_file = sub_matches
        .get_one::<String>("policy")
        .map(|s| s.as_str())
        .unwrap_or("portfolio/policy.toml");
    let format = sub_matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("markdown");

    let positions_str = load_portfolio_file(&filename)?;
    let (portfolio, _network_status) =
        create_live_portfolio_with_logging(positions_str, true).await;

    let policy = Policy::from_file(policy_file).wrap_err_with(|| {
        format!(
            "failed to load policy '{}' (hint: run 'portfolio_rs policy init' to create one)",
            policy_file
        )
    })?;

    let sim = simulate_rebalance(&portfolio, &policy, &cfg.currency);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&sim)?),
        _ => println!("{}", sim.to_markdown()),
    }

    Ok(())
}

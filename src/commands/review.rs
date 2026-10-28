use clap::ArgMatches;
use eyre::Result;

use crate::commands::{defaulted_arg, load_live_portfolio, load_policy};
use crate::review::Review;
use crate::AppConfig;

pub async fn handle_review(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (portfolio, _network_status) = load_live_portfolio(sub_matches, cfg).await?;
    let policy = load_policy(sub_matches)?;
    let review = Review::from_portfolio_and_policy(&portfolio, &policy, &cfg.currency);

    match defaulted_arg(sub_matches, "format") {
        "json" => println!("{}", review.to_json()?),
        _ => println!("{}", review.to_markdown()),
    }
    Ok(())
}

use clap::ArgMatches;
use eyre::Result;

use crate::api::{run_server, ServerConfig};
use crate::commands::{defaulted_arg, resolve_portfolio_file};
use crate::services::portfolio_loader::load_portfolio_file;
use crate::AppConfig;

pub async fn handle_api(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(sub_matches, cfg);
    let positions_json = load_portfolio_file(&filename)?;
    run_server(ServerConfig {
        host: defaulted_arg(sub_matches, "host").to_string(),
        port: *sub_matches
            .get_one::<u16>("port")
            .expect("argument has a default value"),
        positions_json,
        file_path: Some(filename),
        policy_file: sub_matches.get_one::<String>("policy").cloned(),
        currency: cfg.currency.clone(),
        token_file: sub_matches.get_one::<String>("token-file").map(Into::into),
    })
    .await
}

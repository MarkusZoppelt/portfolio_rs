use std::fs::read_to_string;

use crate::portfolio::Portfolio;
use crate::position::from_string;
use crate::position::handle_position;

use clap::{arg, Command};
use serde::Deserialize;
use serde::Serialize;

mod portfolio;
mod position;
mod tui;

#[derive(Serialize, Deserialize)]
struct Config {
    portfolio_file: String,
    currency: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            portfolio_file: "/home/Joe/portfolio.json".to_string(),
            currency: "EUR".to_string(),
        }
    }
}

fn cli() -> Command {
    Command::new("portfolio_rs")
        .about("A portfolio management tool with interactive TUI (default) and CLI commands")
        .author("Markus Zoppelt")
        .arg(
            arg!([FILE] "JSON file with your positions")
                .help("Portfolio data file (uses config file if not specified)"),
        )
        .subcommand(Command::new("config").about("Print the path to the config file"))
        .subcommand(
            Command::new("balances")
                .about("Show the current balances of your portfolio (CLI mode)")
                .arg(
                    arg!([FILE] "JSON file with your positions")
                        .help("Portfolio data file (uses config file if not specified)"),
                ),
        )
        .subcommand(
            Command::new("allocation")
                .about("Show the current allocation of your portfolio (CLI mode)")
                .arg(
                    arg!([FILE] "JSON file with your positions")
                        .help("Portfolio data file (uses config file if not specified)"),
                ),
        )
        .subcommand(
            Command::new("performance")
                .about("Show the performance of your portfolio (CLI mode)")
                .arg(
                    arg!([FILE] "JSON file with your positions")
                        .help("Portfolio data file (uses config file if not specified)"),
                ),
        )
}

// returns a porfolio with the latest quotes from json data
pub async fn create_live_portfolio(positions_str: String) -> Portfolio {
    let positions = from_string(&positions_str);
    let mut portfolio = Portfolio::new();
    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| tokio::spawn(async move { handle_position(&mut position).await }))
        .collect();

    for task in tasks {
        let p = task.await;
        match p {
            Ok(p) => match p {
                Ok(p) => portfolio.add_position(p),
                Err(e) => eprintln!("Error handling position: {e:?}"),
            },
            Err(e) => eprintln!("Error handling position: {e:?}"),
        }
    }
    portfolio
}

// TODO: change this to store entire portfolio in DB
fn store_balance_in_db(portfolio: &Portfolio) {
    let db = sled::open("database").unwrap();
    let curr_value = portfolio.get_total_value();
    let curr_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert(curr_time, curr_value.to_string().as_bytes())
        .unwrap();

    // block until all operations are stable on disk
    db.flush().unwrap();
}

fn open_encrpted_file(filename: String) -> String {
    if filename.ends_with(".gpg") {
        let output = std::process::Command::new("gpg")
            .arg("-d")
            .arg(filename)
            .output()
            .expect("failed to execute gpg process");
        String::from_utf8(output.stdout).unwrap()
    } else {
        read_to_string(filename).unwrap()
    }
}

#[tokio::main]
async fn main() {
    let cfg: Config = confy::load("portfolio", "config").unwrap();

    let matches = cli().get_matches();

    // Handle config subcommand
    if let Some(_matches) = matches.subcommand_matches("config") {
        println!(
            "Your config file is located here: \n{}",
            confy::get_configuration_file_path("portfolio", "config")
                .unwrap()
                .to_str()
                .unwrap()
        );
        return;
    }

    // Get filename from arguments or config
    let get_filename = |matches: Option<&clap::ArgMatches>| -> String {
        let mut filename = String::new();

        // Try to get filename from subcommand or main args
        if let Some(matches) = matches {
            if let Some(f) = matches.get_one::<String>("FILE") {
                filename = f.to_string();
            }
        }

        // Fall back to config file
        if filename.is_empty() {
            filename.clone_from(&cfg.portfolio_file);
        }

        filename
    };

    // Load portfolio data
    let load_portfolio = |filename: String| -> Result<String, String> {
        if filename.is_empty() {
            return Err(
                "No portfolio file specified. Use --help for usage information.".to_string(),
            );
        }

        let positions_str = if filename.ends_with(".gpg") {
            open_encrpted_file(filename.to_string())
        } else if let Ok(s) = read_to_string(&filename) {
            s
        } else {
            return Err(format!("Error reading file: {filename}"));
        };

        Ok(positions_str)
    };

    // Handle subcommands or default to TUI
    match matches.subcommand() {
        Some(("balances", sub_matches)) => {
            let filename = get_filename(Some(sub_matches));
            match load_portfolio(filename) {
                Ok(positions_str) => {
                    let portfolio = create_live_portfolio(positions_str).await;
                    portfolio.print(true);
                    store_balance_in_db(&portfolio);
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        Some(("allocation", sub_matches)) => {
            let filename = get_filename(Some(sub_matches));
            match load_portfolio(filename) {
                Ok(positions_str) => {
                    let portfolio = create_live_portfolio(positions_str).await;
                    portfolio.draw_pie_chart();
                    portfolio.print_allocation();
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        Some(("performance", sub_matches)) => {
            let filename = get_filename(Some(sub_matches));
            match load_portfolio(filename) {
                Ok(positions_str) => {
                    let portfolio = create_live_portfolio(positions_str).await;
                    portfolio.print_performance().await;
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        _ => {
            // Default to TUI when no subcommand is given
            let filename = get_filename(None);
            match load_portfolio(filename.clone()) {
                Ok(positions_str) => {
                    let portfolio = create_live_portfolio(positions_str.clone()).await;
                    if let Err(e) =
                        tui::run_tui(portfolio, cfg.currency.clone(), positions_str, filename).await
                    {
                        eprintln!("Error running TUI: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    cli().print_help().unwrap();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ParseError;

    #[test]
    fn test_cli() {
        let matches = cli().get_matches_from(vec!["portfolio_rs", "balances", "example_data.json"]);
        assert_eq!(matches.subcommand_name(), Some("balances"));
    }

    #[tokio::test]
    async fn test_create_live_portfolio() {
        let positions_str = std::fs::read_to_string("example_data.json").unwrap();
        let portfolio = create_live_portfolio(positions_str).await;
        let x: Result<Portfolio, ParseError> = Ok(portfolio);
        assert!(x.is_ok());
    }
}

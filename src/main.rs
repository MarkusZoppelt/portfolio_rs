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
        .arg(
            arg!(--tab <TAB> "Tab to open at start")
                .default_value("overview")
                .help("Specify the tab to open at start (overview/balances)"),
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
pub async fn create_live_portfolio(
    positions_str: String,
) -> (Portfolio, crate::tui::NetworkStatus) {
    create_live_portfolio_with_logging(positions_str, false).await
}

// returns a porfolio with the latest quotes from json data, with optional error logging
pub async fn create_live_portfolio_with_logging(
    positions_str: String,
    log_errors: bool,
) -> (Portfolio, crate::tui::NetworkStatus) {
    let positions = from_string(&positions_str);
    let mut portfolio = Portfolio::new();
    let _total_positions = positions.len();
    let mut successful_positions = 0;
    let mut failed_positions = 0;

    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| tokio::spawn(async move { handle_position(&mut position).await }))
        .collect();

    for task in tasks {
        let p = task.await;
        match p {
            Ok(p) => match p {
                Ok(p) => {
                    portfolio.add_position(p);
                    successful_positions += 1;
                }
                Err(e) => {
                    if log_errors {
                        eprintln!("Error handling position: {e:?}");
                    }
                    // Skip positions with network errors (will be retried in TUI mode)
                    failed_positions += 1;
                }
            },
            Err(e) => {
                if log_errors {
                    eprintln!("Error handling position: {e:?}");
                }
                // Skip positions with task errors (will be retried in TUI mode)
                failed_positions += 1;
            }
        }
    }

    let network_status = if failed_positions == 0 {
        crate::tui::NetworkStatus::Connected
    } else if successful_positions == 0 {
        crate::tui::NetworkStatus::Disconnected
    } else {
        crate::tui::NetworkStatus::Partial
    };

    (portfolio, network_status)
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

fn get_arg_value(matches: Option<&clap::ArgMatches>, arg_name: &str) -> Option<String> {
    matches.and_then(|m| m.get_one::<String>(arg_name).map(|s| s.to_string()))
}

fn parse_tab(tab_str: Option<String>) -> Option<crate::tui::Tab> {
    match tab_str {
        Some(s) => crate::tui::Tab::from_str(&s).or(Some(crate::tui::Tab::Overview)),
        None => Some(crate::tui::Tab::Overview), // Default to overview
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
        if let Some(f) = get_arg_value(matches, "FILE") {
            filename = f;
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
                    let (portfolio, _network_status) =
                        create_live_portfolio_with_logging(positions_str, true).await;
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
                    let (portfolio, _network_status) =
                        create_live_portfolio_with_logging(positions_str, true).await;
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
                    let (portfolio, _network_status) =
                        create_live_portfolio_with_logging(positions_str, true).await;
                    portfolio.print_performance().await;
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        _ => {
            // Default to TUI when no subcommand is given
            let filename = get_filename(Some(&matches));
            let tab_value = parse_tab(get_arg_value(Some(&matches), "tab"));
            match load_portfolio(filename.clone()) {
                Ok(positions_str) => {
                    let (portfolio, _network_status) =
                        create_live_portfolio(positions_str.clone()).await;
                    if let Err(e) = tui::run_tui(
                        portfolio,
                        cfg.currency.clone(),
                        positions_str,
                        filename,
                        tab_value,
                    )
                    .await
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

    #[test]
    fn test_cli_with_tab_flag() {
        let matches = cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_with_file_and_tab() {
        let matches =
            cli().get_matches_from(vec!["portfolio_rs", "data.json", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "FILE"),
            Some("data.json".to_string())
        );
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_tab_order_independence() {
        let matches =
            cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances", "data.json"]);
        assert_eq!(
            get_arg_value(Some(&matches), "FILE"),
            Some("data.json".to_string())
        );
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_cli_default_tab() {
        let matches = cli().get_matches_from(vec!["portfolio_rs", "data.json"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("overview".to_string())
        );
    }

    #[test]
    fn test_parse_tab_overview() {
        let result = parse_tab(Some("overview".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_balance() {
        let result = parse_tab(Some("balances".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Balances));
    }

    #[test]
    fn test_parse_tab_case_insensitive() {
        assert_eq!(
            parse_tab(Some("OVERVIEW".to_string())),
            Some(crate::tui::Tab::Overview)
        );
        assert_eq!(
            parse_tab(Some("Balances".to_string())),
            Some(crate::tui::Tab::Balances)
        );
        assert_eq!(
            parse_tab(Some("bAlAnCeS".to_string())),
            Some(crate::tui::Tab::Balances)
        );
    }

    #[test]
    fn test_parse_tab_invalid_defaults_to_overview() {
        let result = parse_tab(Some("invalid".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_none_defaults_to_overview() {
        let result = parse_tab(None);
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_parse_tab_empty_string_defaults_to_overview() {
        let result = parse_tab(Some("".to_string()));
        assert_eq!(result, Some(crate::tui::Tab::Overview));
    }

    #[test]
    fn test_get_arg_value_existing() {
        let matches = cli().get_matches_from(vec!["portfolio_rs", "--tab", "balances"]);
        assert_eq!(
            get_arg_value(Some(&matches), "tab"),
            Some("balances".to_string())
        );
    }

    #[test]
    fn test_get_arg_value_missing() {
        let matches = cli().get_matches_from(vec!["portfolio_rs"]);
        assert_eq!(get_arg_value(Some(&matches), "FILE"), None);
    }

    #[test]
    fn test_get_arg_value_none_matches() {
        assert_eq!(get_arg_value(None, "tab"), None);
    }

    #[tokio::test]
    async fn test_create_live_portfolio() {
        let positions_str = std::fs::read_to_string("example_data.json").unwrap();
        let (portfolio, _network_status) = create_live_portfolio(positions_str).await;
        let x: Result<Portfolio, ParseError> = Ok(portfolio);
        assert!(x.is_ok());
    }
}

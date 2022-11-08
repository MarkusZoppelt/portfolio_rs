use crate::portfolio::Portfolio;
use crate::position::from_file;
use crate::position::handle_position;
use chrono::prelude::*;
use clap::{arg, Command};
use colored::*;

mod portfolio;
mod position;

fn cli() -> Command {
    Command::new("portfolio_rs")
        .about("A simple portfolio tool")
        .author("Markus Zoppelt")
        .arg_required_else_help(true)
        .allow_external_subcommands(true)
        .subcommand(
            Command::new("balances")
                .about("Show the current balances of your portfolio")
                .arg(arg!(<FILE> "JSON file with your positions"))
                .arg_required_else_help(true),
        )
        .subcommand(
            Command::new("allocation")
                .about("Show the current allocation of your portfolio")
                .arg(arg!(<FILE> "JSON file with your positions"))
                .arg_required_else_help(true),
        )
        .subcommand(
            Command::new("performance")
                .about("Show the performance of your portfolio")
                .arg(arg!(<FILE> "JSON file with your positions"))
                .arg_required_else_help(true),
        )
}

// returns a porfolio with the latest quotes from a json file
async fn create_live_portfolio(filename: &str) -> Portfolio {
    let positions = from_file(filename);
    let mut portfolio = Portfolio::new();
    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| tokio::spawn(async move { handle_position(&mut position).await }))
        .collect();

    for task in tasks {
        let p = task.await.unwrap();
        portfolio.add_position(p);
    }
    portfolio
}

// TODO: change this to store entire portfolio in DB
fn store_balance_in_db(portfolio: &Portfolio) {
    let db = sled::open("database").unwrap();
    let curr_value = portfolio.get_total_value();
    let curr_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert(curr_time, curr_value.to_string().as_str().as_bytes())
        .unwrap();

    // block until all operations are stable on disk
    db.flush().unwrap();
}

#[tokio::main]
async fn main() {
    let matches = cli().get_matches();

    if let Some(matches) = matches.subcommand_matches("balances") {
        let filename = matches.get_one::<String>("FILE").expect("Cannot get filename");
        let portfolio = create_live_portfolio(filename).await;
        portfolio.print(true);
        store_balance_in_db(&portfolio);
    }

    if let Some(matches) = matches.subcommand_matches("allocation") {
        let filename = matches.get_one::<String>("FILE").expect("Cannot get filename");
        let portfolio = create_live_portfolio(filename).await;
        portfolio.draw_pie_chart();
        portfolio.print_allocation();
    }

    if let Some(matches) = matches.subcommand_matches("performance") {
        let filename = matches.get_one::<String>("FILE").expect("Cannot get filename");
        let portfolio = create_live_portfolio(filename).await;
        let db = sled::open("database").unwrap();

        // Yahoo first of the year is YYYY-01-03
        let first_of_the_year: Date<Utc> = Utc.ymd(Utc::now().year(), 1, 3);
        let first_of_the_month: Date<Utc> = Utc.ymd(Utc::now().year(), Utc::now().month(), 3);
        let value_at_beginning_of_year =
            portfolio.get_historic_total_value(first_of_the_year).await;
        let value_at_beginning_of_month =
            portfolio.get_historic_total_value(first_of_the_month).await;
        let last: f64 = String::from_utf8_lossy(&db.iter().last().unwrap().unwrap().1)
            .parse()
            .unwrap();

        // TODO: add more performance metrics
        let values = vec![
            value_at_beginning_of_year,
            value_at_beginning_of_month,
            portfolio.get_total_value(),
        ];

        for (i, value) in values.iter().enumerate() {
            let performance = (last - value) / value * 100.0;
            if performance >= 0.0 {
                let s = format!("{:.2}%", performance).green();
                if i == 0 {
                    println!("YTD: {}", s);
                } else if i == 1 {
                    println!("Since beginning of month: {}", s);
                } else {
                    println!("Since last balance check: {}", s);
                }
            } else {
                let s = format!("{:.2}%", performance).red();
                if i == 0 {
                    println!("YTD: {}", s);
                } else if i == 1 {
                    println!("Since beginning of month: {}", s);
                } else {
                    println!("Since last balance check: {}", s);
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
        let matches =
            cli().get_matches_from(vec!["portfolio_rs", "balances", "example_data.json"]);
        assert_eq!(matches.subcommand_name(), Some("balances"));
    }

    #[tokio::test]
    async fn test_create_live_portfolio() {
        let portfolio = create_live_portfolio("example_data.json").await;
        let x: Result<Portfolio, ParseError> = Ok(portfolio);
        assert!(x.is_ok());
    }
}

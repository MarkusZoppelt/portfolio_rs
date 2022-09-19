use crate::portfolio::Portfolio;
use crate::position::from_file;
use crate::position::handle_position;
use clap::{arg, Command};

mod portfolio;
mod position;

fn cli() -> Command<'static> {
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
}

#[tokio::main]
async fn main() {
    let matches = cli().get_matches();

    if let Some(matches) = matches.subcommand_matches("balances") {
        let filename = matches.value_of("FILE").expect("Cannot read file");

        let mut portfolio = Portfolio::new();

        let positions = from_file(filename);

        println!(
            "{0: >26} | {1: >12} | {2: >10} | {3: >10}",
            "Name", "Asset Class", "Amount", "Balance"
        );
        println!("====================================================================");

        // move tasks into the async closure passed to tokio::spawn()
        let tasks: Vec<_> = positions
            .into_iter()
            .map(move |mut position| {
                tokio::spawn(async move { handle_position(&mut position).await })
            })
            .collect();

        for task in tasks {
            let p = task.await.unwrap();
            portfolio.add_position(p);
        }

        println!("====================================================================");
        println!("Your total balance is: {:.2}", portfolio.get_total_value());
    }
}

use crate::position::portfolio_position::from_file;
use crate::position::portfolio_position::handle_position;
use crate::position::portfolio_position::PortfolioPosition;
use clap::{arg, Command};

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
                tokio::spawn(async move {
                    return handle_position(&mut position).await;
                })
            })
            .collect();

        // create an empty list to collect the results
        let mut results: Vec<PortfolioPosition> = Vec::new();
        let mut sum = 0.0;

        for task in tasks {
            // wait for the task to complete and add the result to the list
            let p = task.await.unwrap();
            results.push(p);
        }

        for p in results {
            if let Some(_ticker) = p.get_ticker() {
                sum += p.get_balance();
            } else {
                sum += p.get_amount();
            }
        }

        println!("====================================================================");
        println!("Your total balance is: {:.2}", sum);
    }
}

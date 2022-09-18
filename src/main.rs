use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use yahoo_finance_api as yahoo;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PortfolioPosition {
    name: String,
    ticker: String,
    asset_class: String,
    amount: f64,

    #[serde(skip_deserializing)]
    last_spot: f64,
}

impl PortfolioPosition {
    fn update_price(&mut self, last_spot: f64) {
        self.last_spot = last_spot;
    }
}

fn get_positions_from_file(filename: &str) -> Vec<PortfolioPosition> {
    let mut file = File::open(filename).expect("file not found");
    let mut data = String::new();
    file.read_to_string(&mut data)
        .expect("something went wrong reading the file");
    let json =
        serde_json::from_str::<Vec<PortfolioPosition>>(&data).expect("JSON was not well-formatted");
    json
}

// Get the latest price for a ticker
async fn get_quote_price(ticker: &str) -> Result<yahoo::YResponse, yahoo::YahooError> {
    yahoo::YahooConnector::new()
        .get_latest_quotes(&ticker, "1d")
        .await
}

#[tokio::main]
async fn main() {
    let filename = "example_data.json";
    let positions = get_positions_from_file(filename);

    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| {
            tokio::spawn(async move {
                let quote = get_quote_price(&position.ticker).await;
                match quote {
                    Ok(quote) => {
                        position.update_price(quote.last_quote().unwrap().close);
                        println!(
                            "{}: {} {}",
                            position.name,
                            position.last_spot * position.amount,
                            position.asset_class
                        );
                    }
                    Err(e) => {
                        if position.ticker == "-" {
                            println!("Cash: {}", position.amount);
                        } else {
                            panic!("Error: {}", e);
                        }
                    }
                }
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }
}

use yahoo_finance_api as yahoo;

// Get the latest price for a symbol
async fn get_quote_price(symbol: &str) -> Result<yahoo::YResponse, yahoo::YahooError> {
    yahoo::YahooConnector::new()
        .get_latest_quotes(&symbol, "1d")
        .await
}

#[tokio::main]
async fn main() {
    let test_symbols = vec![
        String::from("AAPL"),
        String::from("MSFT"),
        String::from("GOOG"),
        String::from("AMZN"),
    ];

    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = test_symbols
        .into_iter()
        .map(|symbol| {
            tokio::spawn(async move {
                let quote = get_quote_price(&symbol).await;
                match quote {
                    Ok(quote) => {
                        println!("{}: {}", symbol, quote.last_quote().unwrap().close);
                    }
                    Err(e) => {
                        println!("Error: {}", e);
                    }
                }
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }
}

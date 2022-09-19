pub mod portfolio_position {

    use serde::Deserialize;
    use std::fs::File;
    use std::io::Read;
    use yahoo_finance_api as yahoo;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct PortfolioPosition {
        name: String,
        ticker: Option<String>,
        asset_class: String,
        amount: f64,

        #[serde(skip_deserializing)]
        last_spot: f64,
    }

    impl PortfolioPosition {
        fn update_price(&mut self, last_spot: f64) {
            self.last_spot = last_spot;
        }

        pub fn get_balance(&self) -> f64 {
            self.last_spot * self.amount
        }

        pub fn get_amount(&self) -> f64 {
            self.amount
        }

        pub fn get_ticker(&self) -> Option<String> {
            self.ticker.as_ref().map(|s| s.to_string())
        }
    }

    pub fn from_file(filename: &str) -> Vec<PortfolioPosition> {
        let mut file = File::open(filename).expect("file not found");
        let mut data = String::new();
        file.read_to_string(&mut data)
            .expect("something went wrong reading the file");
        serde_json::from_str::<Vec<PortfolioPosition>>(&data).expect("JSON was not well-formatted")
    }

    // Get the latest price for a ticker
    async fn get_quote_price(ticker: &str) -> Result<yahoo::YResponse, yahoo::YahooError> {
        yahoo::YahooConnector::new()
            .get_latest_quotes(ticker, "1d")
            .await
    }

    pub async fn handle_position(position: &mut PortfolioPosition) -> PortfolioPosition {
        if let Some(ticker) = &position.ticker {
            let quote = get_quote_price(ticker).await.unwrap();
            let last_spot = quote.last_quote().unwrap().close;
            position.update_price(last_spot);
            let balance = position.last_spot * position.amount;
            let balance = format!("{:.2}", balance);
            println!(
                "{0: >26} | {1: >12} | {2: >10} | {3: >10}",
                position.name, position.asset_class, position.amount, balance
            );
        } else {
            println!(
                "{0: >26} | {1: >12} | {2: >10} | {3: >10}",
                position.name, "Cash", position.amount, position.amount
            );
        }

        PortfolioPosition {
            name: position.name.clone(),
            ticker: position.ticker.clone(),
            asset_class: position.asset_class.clone(),
            amount: position.amount,
            last_spot: position.last_spot,
        }
    }
}

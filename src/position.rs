use chrono::prelude::*;
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use yahoo_finance_api as yahoo;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortfolioPosition {
    name: Option<String>,
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

    pub fn get_name(&self) -> &str {
        self.name.as_ref().unwrap()
    }

    pub fn get_ticker(&self) -> Option<String> {
        self.ticker.clone()
    }

    pub fn get_asset_class(&self) -> &str {
        &self.asset_class
    }

    pub fn get_balance(&self) -> f64 {
        if let Some(_ticker) = &self.ticker {
            self.last_spot * self.amount
        } else {
            self.amount
        }
    }

    pub fn get_amount(&self) -> f64 {
        self.amount
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


// get the price at a given date
pub async fn get_historic_price(
    ticker: &str,
    date: Date<Utc>,
) -> Result<yahoo::YResponse, yahoo::YahooError> {
    let start = date.and_hms_milli(0, 0, 0, 0);
    let end = date.and_hms_milli(23, 59, 59, 999);
    yahoo::YahooConnector::new()
        .get_quote_history(ticker, start, end)
        .await
}

// Try to get the short name for a ticker from Yahoo Finance
async fn get_quote_name(ticker: &str) -> Result<String, yahoo::YahooError> {
    let connector = yahoo::YahooConnector::new();
    let resp = connector.search_ticker(ticker).await.unwrap();

    // use the first result
    let item = resp.quotes.first();
    let name = &item.unwrap().short_name;

    Ok(name.to_string())
}

// Get the latest price for a ticker and update the positionthen
// then return the updated position as a new object
pub async fn handle_position(position: &mut PortfolioPosition) -> PortfolioPosition {
    if let Some(ticker) = &position.ticker {
        let quote = get_quote_price(ticker).await.unwrap();
        let last_spot = quote.last_quote().unwrap().close;
        position.update_price(last_spot);

        // if no name was provided in the JSON, try to get it from Yahoo Finance
        if position.name.is_none() {
            let ticker = position.ticker.as_ref().unwrap();
            let name = get_quote_name(ticker).await.unwrap();
            position.name = Some(name);
        }
    }

    PortfolioPosition {
        name: position.name.clone(),
        ticker: position.ticker.to_owned(),
        asset_class: position.asset_class.to_string(),
        amount: position.amount,
        last_spot: position.last_spot,
    }
}

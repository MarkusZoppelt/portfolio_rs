use chrono::prelude::*;
use serde::Deserialize;
use time::OffsetDateTime;
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
        if let Some(name) = &self.name {
            name
        } else if let Some(ticker) = &self.ticker {
            ticker
        } else {
            "Unknown"
        }
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

    pub fn get_ticker(&self) -> Option<&str> {
        self.ticker.as_deref()
    }
}

pub fn from_string(data: &str) -> Vec<PortfolioPosition> {
    serde_json::from_str::<Vec<PortfolioPosition>>(data).expect("JSON was not well-formatted")
}

// Get the latest price for a ticker
async fn get_quote_price(ticker: &str) -> Result<yahoo::YResponse, yahoo::YahooError> {
    yahoo::YahooConnector::new()?
        .get_latest_quotes(ticker, "1d")
        .await
}

// get the price at a given date
pub async fn get_historic_price(
    ticker: &str,
    date: DateTime<Utc>,
) -> Result<yahoo::YResponse, yahoo::YahooError> {
    let start = OffsetDateTime::from_unix_timestamp(date.timestamp()).unwrap();

    // get a range of 3 days in case the market is closed on the given date
    let end = start + time::Duration::days(3);

    yahoo::YahooConnector::new()?
        .get_quote_history(ticker, start, end)
        .await
}

// Try to get the short name for a ticker from Yahoo Finance
async fn get_quote_name(ticker: &str) -> Result<String, yahoo::YahooError> {
    let connector = yahoo::YahooConnector::new();
    let resp = connector?.search_ticker(ticker).await?;

    if let Some(item) = resp.quotes.first() {
        Ok(item.short_name.clone())
    } else {
        Err(yahoo::YahooError::NoResult)
    }
}

// Get the latest price for a ticker and update the positionthen
// then return the updated position as a new object
pub async fn handle_position(
    position: &mut PortfolioPosition,
) -> Result<PortfolioPosition, yahoo::YahooError> {
    if let Some(ticker) = &position.ticker {
        let quote = get_quote_price(ticker).await?;
        if let Ok(last_spot) = quote.last_quote() {
            position.update_price(last_spot.close)
        } else {
            // if the market is closed, try to get the last available price
            if let Ok(last_spot) = quote.quotes() {
                if let Some(last_spot) = last_spot.last() {
                    position.update_price(last_spot.close);
                }
            }
        }

        // if no name was provided in the JSON, try to get it from Yahoo Finance
        if position.name.is_none() {
            if let Some(ticker) = &position.ticker {
                let name = get_quote_name(ticker).await?;
                position.name = Some(name);
            }
        }
    }

    Ok(PortfolioPosition {
        name: position.name.clone(),
        ticker: position.ticker.to_owned(),
        asset_class: position.asset_class.to_string(),
        amount: position.amount,
        last_spot: position.last_spot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_get_quote_name() {
        let name = get_quote_name("AAPL").await.unwrap();
        assert_eq!(name, "Apple Inc.");

        let name = get_quote_name("BTC-EUR").await.unwrap();
        assert_eq!(name, "Bitcoin EUR");
    }

    #[tokio::test]
    async fn test_get_quote_price() {
        let quote = get_quote_price("AAPL").await.unwrap();
        assert!(quote.last_quote().unwrap().close > 0.0);
    }

    #[tokio::test]
    async fn test_get_historic_price() {
        let date = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let quote = get_historic_price("AAPL", date).await.unwrap();
        assert_eq!(
            quote.quotes().unwrap().last().unwrap().close,
            74.35749816894531
        );
    }

    #[tokio::test]
    async fn test_handle_position() {
        let mut position = PortfolioPosition {
            name: None,
            ticker: Some("AAPL".to_string()),
            asset_class: "Stock".to_string(),
            amount: 1.0,
            last_spot: 0.0,
        };

        let updated_position = handle_position(&mut position)
            .await
            .expect("Error handling position");
        assert_eq!(updated_position.get_name(), "Apple Inc.");
        assert_eq!(
            updated_position.get_balance(),
            updated_position.get_amount() * updated_position.last_spot
        );
    }

    #[tokio::test]
    async fn test_from_file() {
        let positions_str = fs::read_to_string("example_data.json").unwrap();
        let positions = from_string(&positions_str);
        assert_eq!(positions.len(), 6);
    }
}

use chrono::prelude::*;
use serde::Deserialize;
use time::OffsetDateTime;
use yahoo_finance_api as yahoo;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::sync::{Mutex, Arc};
// Caches for Yahoo API requests
static QUOTE_CACHE: Lazy<Mutex<HashMap<String, Arc<yahoo::YResponse>>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static PREV_CLOSE_CACHE: Lazy<Mutex<HashMap<String, f64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static HISTORIC_CACHE: Lazy<Mutex<HashMap<(String, i64), Arc<yahoo::YResponse>>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static NAME_CACHE: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Purchase {
    // Optional ISO date string (e.g., 2024-01-15)
    pub date: Option<String>,
    pub quantity: f64,
    #[serde(default)]
    pub price: Option<f64>,
    pub fees: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortfolioPosition {
    name: Option<String>,
    ticker: Option<String>,
    asset_class: String,
    amount: f64,

    #[serde(skip_deserializing)]
    last_spot: f64,

    // Optional list of historical purchases to compute cost basis and PnL
    #[serde(default)]
    purchases: Vec<Purchase>,

    // Previous close used to compute daily variation
    #[serde(skip_deserializing)]
    previous_close: Option<f64>,
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
            // Use purchased quantity if available, otherwise fall back to amount
            let quantity = if !self.purchases.is_empty() {
                self.purchases.iter().map(|p| p.quantity).sum::<f64>()
            } else {
                self.amount
            };
            self.last_spot * quantity
        } else {
            self.amount
        }
    }

    pub fn get_amount(&self) -> f64 {
        if !self.purchases.is_empty() {
            self.purchases.iter().map(|p| p.quantity).sum::<f64>()
        } else {
            self.amount
        }
    }

    pub fn get_ticker(&self) -> Option<&str> {
        self.ticker.as_deref()
    }

    pub fn get_name_option(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn set_amount(&mut self, amount: f64) {
        self.amount = amount;
    }

    pub fn market_price(&self) -> f64 {
        self.last_spot
    }

    pub fn market_value(&self) -> f64 {
        self.get_balance()
    }

    pub fn average_cost(&self) -> Option<f64> {
        if self.purchases.is_empty() {
            return None;
        }

        let mut total_quantity = 0.0_f64;
        let mut total_cost = 0.0_f64;

        for p in &self.purchases {
            if let Some(price) = p.price {
                if price > 0.0 {
                    total_quantity += p.quantity;
                    total_cost += p.quantity * price + p.fees.unwrap_or(0.0);
                }
            }
        }

        if total_quantity > 0.0 {
            Some(total_cost / total_quantity)
        } else {
            None
        }
    }

    pub fn total_invested(&self) -> Option<f64> {
        if self.purchases.is_empty() {
            return None;
        }

        let invested = self
            .purchases
            .iter()
            .filter_map(|p| p.price.map(|price| (price, p)))
            .filter(|(price, _)| *price > 0.0)
            .map(|(price, p)| p.quantity * price + p.fees.unwrap_or(0.0))
            .sum::<f64>();

        if invested > 0.0 { Some(invested) } else { None }
    }

    pub fn pnl(&self) -> Option<f64> {
        let invested = self.total_invested()?;
        Some(self.market_value() - invested)
    }

    pub fn historic_variation_percent(&self) -> Option<f64> {
        let invested = self.total_invested()?;
        if invested <= 0.0 {
            return None;
        }
        Some((self.market_value() - invested) / invested * 100.0)
    }

    pub fn daily_variation_percent(&self) -> Option<f64> {
        let prev = self.previous_close?;
        if prev <= 0.0 {
            return None;
        }
        Some((self.market_price() - prev) / prev * 100.0)
    }

    pub fn get_purchases(&self) -> &[Purchase] {
        &self.purchases
    }

    pub fn get_previous_close(&self) -> Option<f64> {
        self.previous_close
    }
}

pub fn from_string(data: &str) -> Vec<PortfolioPosition> {
    serde_json::from_str::<Vec<PortfolioPosition>>(data).expect("JSON was not well-formatted")
}

// Get the latest price for a ticker, cache on success, fallback to cache on failure
async fn get_quote_price(ticker: &str) -> Result<Arc<yahoo::YResponse>, yahoo::YahooError> {
    match yahoo::YahooConnector::new()?.get_latest_quotes(ticker, "1d").await {
        Ok(resp) => {
            QUOTE_CACHE.lock().unwrap().insert(ticker.to_string(), Arc::new(resp));
            Ok(Arc::clone(QUOTE_CACHE.lock().unwrap().get(ticker).unwrap()))
        }
        Err(e) => {
            if let Some(cached) = QUOTE_CACHE.lock().unwrap().get(ticker) {
                Ok(Arc::clone(cached))
            } else {
                Err(e)
            }
        }
    }
}

// Try to get the previous close price for daily variation calculations, cache on success, fallback to cache on failure
async fn get_previous_close(ticker: &str) -> Result<f64, yahoo::YahooError> {
    let end = OffsetDateTime::now_utc();
    let start = end - time::Duration::days(7);
    match yahoo::YahooConnector::new()?.get_quote_history(ticker, start, end).await {
        Ok(resp) => {
            let quotes = resp.quotes()?;
            let prev_close = if quotes.len() >= 2 {
                quotes[quotes.len() - 2].close
            } else if let Some(last) = quotes.last() {
                last.close
            } else {
                return Err(yahoo::YahooError::NoResult);
            };
            PREV_CLOSE_CACHE.lock().unwrap().insert(ticker.to_string(), prev_close);
            Ok(prev_close)
        }
        Err(e) => {
            if let Some(cached) = PREV_CLOSE_CACHE.lock().unwrap().get(ticker) {
                Ok(*cached)
            } else {
                Err(e)
            }
        }
    }
}

pub fn parse_purchase_date(date_str: &str) -> Option<DateTime<Utc>> {
    use chrono::NaiveDate;
    let s = date_str.trim();
    let parsed = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(s, "%Y/%m/%d"))
        .or_else(|_| NaiveDate::parse_from_str(s, "%d-%m-%Y"))
        .ok()?;
    Utc.with_ymd_and_hms(parsed.year(), parsed.month(), parsed.day(), 0, 0, 0)
        .single()
}

// get the price at a given date, cache on success, fallback to cache on failure
pub async fn get_historic_price(
    ticker: &str,
    date: DateTime<Utc>,
) -> Result<Arc<yahoo::YResponse>, yahoo::YahooError> {
    let start = OffsetDateTime::from_unix_timestamp(date.timestamp()).unwrap();

    // get a range of 3 days in case the market is closed on the given date
    let end = start + time::Duration::days(3);
    let cache_key = (ticker.to_string(), date.timestamp());

    match yahoo::YahooConnector::new()?.get_quote_history(ticker, start, end).await {
        Ok(resp) => {
            HISTORIC_CACHE.lock().unwrap().insert(cache_key.clone(), Arc::new(resp));
            Ok(Arc::clone(HISTORIC_CACHE.lock().unwrap().get(&cache_key).unwrap()))
        }
        Err(e) => {
            if let Some(cached) = HISTORIC_CACHE.lock().unwrap().get(&cache_key) {
                Ok(Arc::clone(cached))
            } else {
                Err(e)
            }
        }
    }
}

// Try to get the short name for a ticker from Yahoo Finance, cache on success, fallback to cache on failure
async fn get_quote_name(ticker: &str) -> Result<String, yahoo::YahooError> {
    match yahoo::YahooConnector::new()?.search_ticker(ticker).await {
        Ok(resp) => {
            if let Some(item) = resp.quotes.first() {
                NAME_CACHE.lock().unwrap().insert(ticker.to_string(), item.short_name.clone());
                Ok(item.short_name.clone())
            } else {
                Err(yahoo::YahooError::NoResult)
            }
        }
        Err(e) => {
            if let Some(cached) = NAME_CACHE.lock().unwrap().get(ticker) {
                Ok(cached.clone())
            } else {
                Err(e)
            }
        }
    }
}

// Get the latest price for a ticker and update the positionthen
// then return the updated position as a new object
pub async fn handle_position(
    position: &mut PortfolioPosition,
) -> Result<PortfolioPosition, yahoo::YahooError> {
    if let Some(ticker_owned) = position.ticker.clone() {
        let ticker = ticker_owned.as_str();
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

        // fetch previous close for daily variation
        if let Ok(prev_close) = get_previous_close(ticker).await {
            position.previous_close = Some(prev_close);
        }

        // Fill missing purchase prices using historic data
        if !position.purchases.is_empty() {
            let mut filled_purchases = Vec::with_capacity(position.purchases.len());
            for mut p in position.purchases.clone() {
                let needs_price = p.price.map(|v| v <= 0.0).unwrap_or(true);
                if needs_price && p.quantity > 0.0 {
                    if let Some(date_str) = &p.date {
                        if let Some(date) = parse_purchase_date(date_str) {
                            if let Ok(resp) = get_historic_price(ticker, date).await {
                                if let Ok(q) = resp.last_quote() {
                                    p.price = Some(q.close.max(0.0));
                                } else if let Ok(quotes) = resp.quotes() {
                                    if let Some(last) = quotes.last() {
                                        p.price = Some(last.close.max(0.0));
                                    }
                                }
                            }
                        }
                    }
                }
                filled_purchases.push(p);
            }
            position.purchases = filled_purchases;
        }

        // if no name was provided in the JSON, try to get it from Yahoo Finance
        if position.name.is_none() {
            if let Some(ticker_again) = &position.ticker {
                let name = get_quote_name(ticker_again).await?;
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
        purchases: position.purchases.clone(),
        previous_close: position.previous_close,
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
            purchases: vec![],
            previous_close: None,
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

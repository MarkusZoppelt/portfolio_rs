//! Live Yahoo Finance requests behind [`crate::services::quotes::YahooProvider`].
//!
//! Excluded from the coverage gate (`*_io.rs`): every function here performs a
//! network request, which tests must not do. Each one only builds the client,
//! makes the request and hands the raw result to a function in
//! `position.rs` (caching, fallback and parsing), which is tested offline.
//! The network tests for these calls are `#[ignore]`d below.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use time::OffsetDateTime;
use yahoo_finance_api as yahoo;

use crate::services::quotes::{DailyBar, Quote, QuoteError};

fn provider_error(e: yahoo::YahooError) -> QuoteError {
    QuoteError::Provider(e.to_string())
}

/// Latest quote, falling back to the last good response.
pub(crate) async fn yahoo_latest(symbol: &str) -> Result<Quote, QuoteError> {
    let connector = yahoo::YahooConnector::new().map_err(provider_error)?;
    super::latest_from(symbol, connector.get_latest_quotes(symbol, "1d").await)
}

/// Previous close from the last 7 days, falling back to the last good one.
pub(crate) async fn yahoo_previous_close(symbol: &str) -> Result<f64, QuoteError> {
    let connector = yahoo::YahooConnector::new().map_err(provider_error)?;
    let end = OffsetDateTime::now_utc();
    let start = end - time::Duration::days(7);
    let fetched = connector.get_quote_history(symbol, start, end).await;
    super::previous_close_from(symbol, fetched, Utc::now())
}

/// Daily history between two instants, falling back to the last good response.
pub async fn get_historic_price(
    ticker: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Arc<yahoo::YResponse>, yahoo::YahooError> {
    let connector = yahoo::YahooConnector::new()?;
    let (start, end) = super::offset_range(from, to);
    let fetched = connector.get_quote_history(ticker, start, end).await;
    super::history_from(ticker, from, to, fetched)
}

/// Daily bars in `[from, to]`, dated in the exchange's local time zone.
pub(crate) async fn yahoo_daily_bars(
    symbol: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<DailyBar>, QuoteError> {
    super::daily_bars_from(symbol, get_historic_price(symbol, from, to).await)
}

/// Short name of a ticker, falling back to the last good one.
pub(crate) async fn yahoo_name(symbol: &str) -> Option<String> {
    let connector = yahoo::YahooConnector::new().ok()?;
    super::name_from(symbol, connector.search_ticker(symbol).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::{dec, Decimal};
    use crate::position::{handle_position, PortfolioPosition, PriceStatus};
    use chrono::TimeZone;

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_yahoo_name() {
        assert_eq!(yahoo_name("AAPL").await.as_deref(), Some("Apple Inc."));
        assert_eq!(yahoo_name("BTC-EUR").await.as_deref(), Some("Bitcoin EUR"));
    }

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_yahoo_latest() {
        assert!(yahoo_latest("AAPL").await.unwrap().price > 0.0);
    }

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_get_historic_price() {
        let date = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let quote = get_historic_price("AAPL", date, date + chrono::Duration::days(3))
            .await
            .unwrap();
        assert_eq!(
            quote.quotes().unwrap().last().unwrap().close,
            74.35749816894531
        );
    }

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_handle_position() {
        let mut position = PortfolioPosition {
            id: None,
            name: None,
            ticker: Some("AAPL".to_string()),
            asset_class: "Stock".to_string(),
            amount: dec(1.0),
            last_spot: Decimal::ZERO,
            price_status: PriceStatus::Live,
            price_currency: None,
            price_as_of: None,
            price_source: None,
            fx_from_cache: false,
            excluded_estimate: None,
            currency: None,
            runtime: Default::default(),
            fx_rate: None,
            purchases: vec![],
            previous_close: None,
        };

        let updated_position = handle_position(&mut position)
            .await
            .expect("Error handling position");
        assert_eq!(updated_position.get_name(), "Apple Inc.");
        assert_eq!(
            updated_position.get_balance(),
            updated_position.get_amount() * updated_position.market_price()
        );
    }
}

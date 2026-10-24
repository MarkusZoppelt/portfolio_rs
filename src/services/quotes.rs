//! Quote and FX lookups behind a trait, so network access can be faked in tests.
//!
//! Every lookup sends only a ticker or FX symbol (e.g. `EURUSD=X`). FX rates are
//! fetched through [`QuoteProvider::latest`] like any other symbol.

use std::collections::HashMap;
use std::future::Future;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Latest usable (positive) price of a symbol and its quote currency.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Quote {
    pub price: f64,
    #[serde(default)]
    pub currency: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum QuoteError {
    #[error("no quote available for {0}")]
    Unavailable(String),
    #[error("quote lookup failed: {0}")]
    Provider(String),
}

/// Source of quotes. Implementations must not print.
pub trait QuoteProvider: Send + Sync + 'static {
    /// Latest positive close and its currency.
    fn latest(&self, symbol: &str) -> impl Future<Output = Result<Quote, QuoteError>> + Send;
    /// Previous close, used for the daily change.
    fn previous_close(&self, symbol: &str) -> impl Future<Output = Result<f64, QuoteError>> + Send;
    /// Close on (or shortly after) `date`, in the quote currency.
    fn close_on(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> impl Future<Output = Result<f64, QuoteError>> + Send;
    /// Display name. Never fatal: `None` when unknown.
    fn name(&self, symbol: &str) -> impl Future<Output = Option<String>> + Send;
}

/// Yahoo Finance, using the process-wide caches in [`crate::position`].
#[derive(Debug, Clone, Copy, Default)]
pub struct YahooProvider;

impl QuoteProvider for YahooProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        crate::position::yahoo_latest(symbol).await
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        crate::position::yahoo_previous_close(symbol).await
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        crate::position::yahoo_close_on(symbol, date).await
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        crate::position::yahoo_name(symbol).await
    }
}

/// Fixed prices from memory or JSON, for tests and fixtures.
///
/// JSON shape: `{"latest": {"SYM": {"price": 1.0, "currency": "USD"}},
/// "previous_close": {"SYM": 1.0}, "close_on": {"SYM": 1.0}, "names": {"SYM": "Name"}}`.
/// `close_on` ignores the date.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixtureProvider {
    #[serde(default)]
    pub latest: HashMap<String, Quote>,
    #[serde(default)]
    pub previous_close: HashMap<String, f64>,
    #[serde(default)]
    pub close_on: HashMap<String, f64>,
    #[serde(default)]
    pub names: HashMap<String, String>,
}

impl FixtureProvider {
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }
}

fn lookup(map: &HashMap<String, f64>, symbol: &str) -> Result<f64, QuoteError> {
    map.get(symbol)
        .copied()
        .filter(|v| *v > 0.0 && v.is_finite())
        .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
}

impl QuoteProvider for FixtureProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        self.latest
            .get(symbol)
            .filter(|q| q.price > 0.0 && q.price.is_finite())
            .cloned()
            .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        lookup(&self.previous_close, symbol)
    }

    async fn close_on(&self, symbol: &str, _date: DateTime<Utc>) -> Result<f64, QuoteError> {
        lookup(&self.close_on, symbol)
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        self.names.get(symbol).cloned()
    }
}

/// Never touches the network; every lookup is unavailable.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfflineProvider;

impl QuoteProvider for OfflineProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        Err(QuoteError::Unavailable(symbol.to_string()))
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        Err(QuoteError::Unavailable(symbol.to_string()))
    }

    async fn close_on(&self, symbol: &str, _date: DateTime<Utc>) -> Result<f64, QuoteError> {
        Err(QuoteError::Unavailable(symbol.to_string()))
    }

    async fn name(&self, _symbol: &str) -> Option<String> {
        None
    }
}

//! Quote and FX lookups behind a trait, so network access can be faked in tests.
//!
//! Every lookup sends only a ticker or FX symbol (e.g. `EURUSD=X`). FX rates are
//! fetched through [`QuoteProvider::latest`] like any other symbol.

use std::collections::HashMap;
use std::future::Future;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer};

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

/// One daily bar: the exchange-local trading date and its close.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct DailyBar {
    #[serde(deserialize_with = "de_date")]
    pub date: NaiveDate,
    pub close: f64,
}

fn de_date<'de, D: Deserializer<'de>>(d: D) -> Result<NaiveDate, D::Error> {
    let s = String::deserialize(d)?;
    NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(serde::de::Error::custom)
}

fn usable(bar: &&DailyBar) -> bool {
    bar.close > 0.0 && bar.close.is_finite()
}

/// Close of the first trading day on or after `date` (the price a buy on
/// `date` got, also when it falls on a weekend or holiday).
pub fn first_close_on_or_after(bars: &[DailyBar], date: NaiveDate) -> Option<f64> {
    bars.iter()
        .filter(usable)
        .filter(|b| b.date >= date)
        .min_by_key(|b| b.date)
        .map(|b| b.close)
}

/// Close of the last trading day strictly before `date` (the value a period
/// starting on `date` is measured from).
pub fn last_close_before(bars: &[DailyBar], date: NaiveDate) -> Option<f64> {
    bars.iter()
        .filter(usable)
        .filter(|b| b.date < date)
        .max_by_key(|b| b.date)
        .map(|b| b.close)
}

/// Source of quotes. Implementations must not print.
pub trait QuoteProvider: Send + Sync + 'static {
    /// Latest positive close and its currency.
    fn latest(&self, symbol: &str) -> impl Future<Output = Result<Quote, QuoteError>> + Send;
    /// Previous close, used for the daily change.
    fn previous_close(&self, symbol: &str) -> impl Future<Output = Result<f64, QuoteError>> + Send;
    /// Close used to fill a purchase on `date` (first trading day on or after it).
    fn close_on(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> impl Future<Output = Result<f64, QuoteError>> + Send;
    /// Close of the first trading day on or after `date` (purchase prices).
    /// Defaults to [`Self::close_on`].
    fn close_on_or_after(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> impl Future<Output = Result<f64, QuoteError>> + Send {
        self.close_on(symbol, date)
    }
    /// Close of the last trading day before `date` (period start values).
    /// Defaults to [`Self::close_on`].
    fn close_before(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> impl Future<Output = Result<f64, QuoteError>> + Send {
        self.close_on(symbol, date)
    }
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
        let bars =
            crate::position::yahoo_daily_bars(symbol, date, date + chrono::Duration::days(7))
                .await?;
        first_close_on_or_after(&bars, date.date_naive())
            .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        let bars = crate::position::yahoo_daily_bars(
            symbol,
            date - chrono::Duration::days(10),
            date + chrono::Duration::days(1),
        )
        .await?;
        last_close_before(&bars, date.date_naive())
            .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        crate::position::yahoo_name(symbol).await
    }
}

/// Fixed prices from memory or JSON, for tests and fixtures.
///
/// JSON shape: `{"latest": {"SYM": {"price": 1.0, "currency": "USD"}},
/// "previous_close": {"SYM": 1.0}, "close_on": {"SYM": 1.0}, "names": {"SYM": "Name"},
/// "history": {"SYM": [{"date": "2025-01-02", "close": 1.0}]}}`.
/// Historic lookups use `history` when the symbol has bars; otherwise
/// `close_on`, which ignores the date.
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
    #[serde(default)]
    pub history: HashMap<String, Vec<DailyBar>>,
}

impl FixtureProvider {
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }
}

impl TryFrom<serde_json::Map<String, serde_json::Value>> for FixtureProvider {
    type Error = serde_json::Error;

    fn try_from(map: serde_json::Map<String, serde_json::Value>) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::Value::Object(map))
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

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self.history.get(symbol) {
            Some(bars) => first_close_on_or_after(bars, date.date_naive())
                .ok_or_else(|| QuoteError::Unavailable(symbol.to_string())),
            None => lookup(&self.close_on, symbol),
        }
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self.history.get(symbol) {
            Some(bars) => last_close_before(bars, date.date_naive())
                .ok_or_else(|| QuoteError::Unavailable(symbol.to_string())),
            None => lookup(&self.close_on, symbol),
        }
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        self.names.get(symbol).cloned()
    }
}

/// Runtime-selected provider (the trait itself is not object safe).
#[derive(Debug, Clone, Default)]
pub enum AnyProvider {
    #[default]
    Yahoo,
    Offline,
    Fixture(Box<FixtureProvider>),
}

/// Environment variable that selects the quote source.
///
/// `yahoo` (or unset) uses Yahoo Finance, `offline` makes every lookup fail
/// without network access, and `fixture:<path>` serves prices from a
/// [`FixtureProvider`] JSON file.
pub const QUOTES_ENV: &str = "PORTFOLIO_RS_QUOTES";

impl AnyProvider {
    /// Parse a [`QUOTES_ENV`] value.
    pub fn from_spec(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        match spec {
            "" | "yahoo" => Ok(Self::Yahoo),
            "offline" => Ok(Self::Offline),
            _ => match spec.strip_prefix("fixture:") {
                Some(path) if !path.is_empty() => {
                    let json = std::fs::read_to_string(path)
                        .map_err(|e| format!("cannot read quote fixture {path}: {e}"))?;
                    let fixture = FixtureProvider::from_json(&json)
                        .map_err(|e| format!("invalid quote fixture {path}: {e}"))?;
                    Ok(Self::Fixture(Box::new(fixture)))
                }
                _ => Err(format!(
                    "unknown {QUOTES_ENV} value {spec:?} (expected yahoo, offline or fixture:<path>)"
                )),
            },
        }
    }

    /// The provider selected by [`QUOTES_ENV`], resolved once per process.
    ///
    /// An invalid value never falls back to the network: it prints a warning
    /// and uses [`AnyProvider::Offline`].
    pub fn configured() -> &'static AnyProvider {
        static CONFIGURED: std::sync::OnceLock<AnyProvider> = std::sync::OnceLock::new();
        CONFIGURED.get_or_init(|| {
            let spec = std::env::var(QUOTES_ENV).unwrap_or_default();
            Self::from_spec(&spec).unwrap_or_else(|err| {
                eprintln!("Warning: {err}; using offline quotes");
                Self::Offline
            })
        })
    }
}

impl QuoteProvider for AnyProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        match self {
            Self::Yahoo => YahooProvider.latest(symbol).await,
            Self::Offline => OfflineProvider.latest(symbol).await,
            Self::Fixture(f) => f.latest(symbol).await,
        }
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo => YahooProvider.previous_close(symbol).await,
            Self::Offline => OfflineProvider.previous_close(symbol).await,
            Self::Fixture(f) => f.previous_close(symbol).await,
        }
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo => YahooProvider.close_on(symbol, date).await,
            Self::Offline => OfflineProvider.close_on(symbol, date).await,
            Self::Fixture(f) => f.close_on(symbol, date).await,
        }
    }

    async fn close_on_or_after(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> Result<f64, QuoteError> {
        // Dispatch, so a fixture's date-aware override is not skipped.
        match self {
            Self::Yahoo => YahooProvider.close_on(symbol, date).await,
            Self::Offline => OfflineProvider.close_on(symbol, date).await,
            Self::Fixture(f) => f.close_on(symbol, date).await,
        }
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo => YahooProvider.close_before(symbol, date).await,
            Self::Offline => OfflineProvider.close_before(symbol, date).await,
            Self::Fixture(f) => f.close_before(symbol, date).await,
        }
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        match self {
            Self::Yahoo => YahooProvider.name(symbol).await,
            Self::Offline => OfflineProvider.name(symbol).await,
            Self::Fixture(f) => f.name(symbol).await,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // Fri 2025-01-03, Mon 2025-01-06, Tue 2025-01-07 (weekend in between).
    fn bars() -> Vec<DailyBar> {
        vec![
            DailyBar {
                date: d(2025, 1, 7),
                close: 12.0,
            },
            DailyBar {
                date: d(2025, 1, 3),
                close: 10.0,
            },
            DailyBar {
                date: d(2025, 1, 6),
                close: 11.0,
            },
        ]
    }

    #[test]
    fn test_purchase_uses_first_bar_on_or_after_date() {
        assert_eq!(first_close_on_or_after(&bars(), d(2025, 1, 3)), Some(10.0));
        // Saturday buy -> Monday close, not the last bar of the window.
        assert_eq!(first_close_on_or_after(&bars(), d(2025, 1, 4)), Some(11.0));
        assert_eq!(first_close_on_or_after(&bars(), d(2025, 1, 8)), None);
    }

    #[test]
    fn test_period_start_uses_last_bar_before_date() {
        // Period starting Monday is measured from Friday's close.
        assert_eq!(last_close_before(&bars(), d(2025, 1, 6)), Some(10.0));
        assert_eq!(last_close_before(&bars(), d(2025, 1, 8)), Some(12.0));
        assert_eq!(last_close_before(&bars(), d(2025, 1, 3)), None);
    }

    #[tokio::test]
    async fn test_fixture_history_is_date_aware() {
        let fixture = FixtureProvider::from_json(
            r#"{"history": {"FAKE": [{"date": "2025-01-03", "close": 10.0},
                                     {"date": "2025-01-06", "close": 11.0}]}}"#,
        )
        .unwrap();
        let sat = Utc.with_ymd_and_hms(2025, 1, 4, 0, 0, 0).unwrap();
        assert_eq!(fixture.close_on("FAKE", sat).await.unwrap(), 11.0);
        assert_eq!(fixture.close_on_or_after("FAKE", sat).await.unwrap(), 11.0);
        assert_eq!(fixture.close_before("FAKE", sat).await.unwrap(), 10.0);
    }

    #[test]
    fn test_quotes_spec_parsing() {
        assert!(matches!(AnyProvider::from_spec(""), Ok(AnyProvider::Yahoo)));
        assert!(matches!(
            AnyProvider::from_spec("yahoo"),
            Ok(AnyProvider::Yahoo)
        ));
        assert!(matches!(
            AnyProvider::from_spec("offline"),
            Ok(AnyProvider::Offline)
        ));
        assert!(AnyProvider::from_spec("live").is_err());
        assert!(AnyProvider::from_spec("fixture:").is_err());
        assert!(AnyProvider::from_spec("fixture:/nonexistent/fake.json").is_err());

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("quotes.json");
        std::fs::write(
            &path,
            r#"{"latest": {"FAKE": {"price": 2.5, "currency": "EUR"}}}"#,
        )
        .unwrap();
        match AnyProvider::from_spec(&format!("fixture:{}", path.display())) {
            Ok(AnyProvider::Fixture(f)) => assert_eq!(f.latest["FAKE"].price, 2.5),
            other => panic!("unexpected {other:?}"),
        }
        std::fs::write(&path, "not json").unwrap();
        assert!(AnyProvider::from_spec(&format!("fixture:{}", path.display())).is_err());
    }
}

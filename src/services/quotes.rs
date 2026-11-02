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
    /// When the price was set (RFC 3339), if the provider reports it.
    #[serde(default, alias = "asOf")]
    pub as_of: Option<String>,
    /// Where the price came from (set by the provider, never deserialized).
    #[serde(skip)]
    pub source: PriceSource,
}

/// Origin of a price: a live provider fetch, the last-known-price cache, or a
/// test fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceSource {
    #[default]
    Live,
    Cache,
    Fixture,
}

impl PriceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Cache => "cache",
            Self::Fixture => "fixture",
        }
    }
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
    /// Last known price and currency regardless of age (the quote cache);
    /// only used to estimate excluded values, never as a current price.
    fn last_known(&self, _symbol: &str) -> Option<(f64, Option<String>)> {
        None
    }
    /// As-of (else fetched-at) of [`Self::last_known`]'s entry.
    fn last_known_as_of(&self, _symbol: &str) -> Option<String> {
        None
    }
    /// Quotes come from a fixture (tests, demos), not the network.
    fn is_fixture(&self) -> bool {
        false
    }
}

/// Yahoo Finance, using the process-wide caches in [`crate::position`].
///
/// Its [`QuoteProvider`] impl is network glue in `yahoo_io.rs`; the historic
/// lookups it delegates to ([`bars_close_on_or_after`], [`bars_close_before`])
/// live here.
#[derive(Debug, Clone, Copy, Default)]
pub struct YahooProvider;

/// Daily bars of a symbol in `[from, to]`, dated in the exchange's time zone:
/// the one fetch the historic lookups need.
pub(crate) trait DailyBarSource: Sync {
    fn daily_bars(
        &self,
        symbol: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<DailyBar>, QuoteError>> + Send;
}

/// Purchase price on `date`: the first close in the week from `date`.
pub(crate) async fn bars_close_on_or_after(
    source: &impl DailyBarSource,
    symbol: &str,
    date: DateTime<Utc>,
) -> Result<f64, QuoteError> {
    let bars = source
        .daily_bars(symbol, date, date + chrono::Duration::days(7))
        .await?;
    first_close_on_or_after(&bars, date.date_naive())
        .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
}

/// Period start value: the last close in the ten days before `date`.
pub(crate) async fn bars_close_before(
    source: &impl DailyBarSource,
    symbol: &str,
    date: DateTime<Utc>,
) -> Result<f64, QuoteError> {
    let bars = source
        .daily_bars(
            symbol,
            date - chrono::Duration::days(10),
            date + chrono::Duration::days(1),
        )
        .await?;
    last_close_before(&bars, date.date_naive())
        .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
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
            .map(|q| Quote {
                source: PriceSource::Fixture,
                ..q
            })
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
    /// Another provider behind the last-known-price cache.
    Cached(std::sync::Arc<crate::services::quote_cache::CachedProvider>),
}

/// Environment variable that selects the quote source.
///
/// `yahoo` (or unset) uses Yahoo Finance, `offline` makes every lookup fail
/// without network access, and `fixture:<path>` serves prices from a
/// [`FixtureProvider`] JSON file.
pub const QUOTES_ENV: &str = "PORTFOLIO_RS_QUOTES";

impl AnyProvider {
    /// `yahoo`, `offline`, `fixture` or `cache` (the wrapped provider's name
    /// for a cached provider).
    pub fn source_name(&self) -> &'static str {
        match self {
            Self::Yahoo => "yahoo",
            Self::Offline => "offline",
            Self::Fixture(_) => "fixture",
            Self::Cached(c) => c.inner().source_name(),
        }
    }

    /// Write the last-known-price cache, if this provider has one.
    pub fn flush_cache(&self) {
        if let Self::Cached(c) = self {
            if let Err(e) = c.flush() {
                eprintln!("Warning: quote cache not written: {e:#}");
            }
        }
    }

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
            Self::Cached(c) => c.latest(symbol).await,
            _ => self.base(&YahooProvider).latest(symbol).await,
        }
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        match self {
            Self::Cached(c) => c.previous_close(symbol).await,
            _ => self.base(&YahooProvider).previous_close(symbol).await,
        }
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Cached(c) => c.close_on(symbol, date).await,
            _ => self.base(&YahooProvider).close_on(symbol, date).await,
        }
    }

    async fn close_on_or_after(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> Result<f64, QuoteError> {
        // Dispatch, so a fixture's date-aware override is not skipped.
        match self {
            Self::Cached(c) => c.close_on(symbol, date).await,
            _ => self.base(&YahooProvider).close_on(symbol, date).await,
        }
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Cached(c) => c.close_before(symbol, date).await,
            _ => self.base(&YahooProvider).close_before(symbol, date).await,
        }
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        match self {
            Self::Cached(c) => c.name(symbol).await,
            _ => self.base(&YahooProvider).name(symbol).await,
        }
    }

    fn last_known(&self, symbol: &str) -> Option<(f64, Option<String>)> {
        match self {
            Self::Cached(c) => c.last_known_entry(symbol),
            _ => None,
        }
    }
    fn last_known_as_of(&self, symbol: &str) -> Option<String> {
        match self {
            Self::Cached(c) => c.last_known_as_of(symbol),
            _ => None,
        }
    }
    fn is_fixture(&self) -> bool {
        match self {
            Self::Fixture(_) => true,
            Self::Cached(c) => c.is_fixture(),
            _ => false,
        }
    }
}

/// A non-cached provider: what an [`AnyProvider`] resolves to below the
/// quote cache. `Y` is the Yahoo backend ([`YahooProvider`] outside tests).
enum Base<'a, Y> {
    Yahoo(&'a Y),
    Offline,
    Fixture(&'a FixtureProvider),
}

impl<Y: QuoteProvider> Base<'_, Y> {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        match self {
            Self::Yahoo(y) => y.latest(symbol).await,
            Self::Offline => OfflineProvider.latest(symbol).await,
            Self::Fixture(f) => f.latest(symbol).await,
        }
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo(y) => y.previous_close(symbol).await,
            Self::Offline => OfflineProvider.previous_close(symbol).await,
            Self::Fixture(f) => f.previous_close(symbol).await,
        }
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo(y) => y.close_on(symbol, date).await,
            Self::Offline => OfflineProvider.close_on(symbol, date).await,
            Self::Fixture(f) => f.close_on(symbol, date).await,
        }
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        match self {
            Self::Yahoo(y) => y.close_before(symbol, date).await,
            Self::Offline => OfflineProvider.close_before(symbol, date).await,
            Self::Fixture(f) => f.close_before(symbol, date).await,
        }
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        match self {
            Self::Yahoo(y) => y.name(symbol).await,
            Self::Offline => OfflineProvider.name(symbol).await,
            Self::Fixture(f) => f.name(symbol).await,
        }
    }
}

/// The non-cached providers [`CachedProvider`](crate::services::quote_cache::CachedProvider)
/// wraps (a cached provider inside a cache is treated as offline). Kept
/// separate from [`AnyProvider`]'s impl so the future types are not recursive.
impl AnyProvider {
    /// Resolve to a non-cached provider, with `yahoo` as the Yahoo backend.
    fn base<'a, Y>(&'a self, yahoo: &'a Y) -> Base<'a, Y> {
        match self {
            Self::Yahoo => Base::Yahoo(yahoo),
            Self::Fixture(f) => Base::Fixture(f),
            Self::Offline | Self::Cached(_) => Base::Offline,
        }
    }

    pub(crate) async fn base_latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        self.base(&YahooProvider).latest(symbol).await
    }

    pub(crate) async fn base_previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        self.base(&YahooProvider).previous_close(symbol).await
    }

    pub(crate) async fn base_close_on(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> Result<f64, QuoteError> {
        self.base(&YahooProvider).close_on(symbol, date).await
    }

    pub(crate) async fn base_close_before(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> Result<f64, QuoteError> {
        self.base(&YahooProvider).close_before(symbol, date).await
    }

    pub(crate) async fn base_name(&self, symbol: &str) -> Option<String> {
        self.base(&YahooProvider).name(symbol).await
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
        let parsed = AnyProvider::from_spec(&format!("fixture:{}", path.display())).unwrap();
        assert!(matches!(&parsed, AnyProvider::Fixture(f) if f.latest["FAKE"].price == 2.5));
        std::fs::write(&path, "not json").unwrap();
        assert!(AnyProvider::from_spec(&format!("fixture:{}", path.display())).is_err());
    }

    #[test]
    fn test_price_source_names() {
        assert_eq!(PriceSource::Live.as_str(), "live");
        assert_eq!(PriceSource::Cache.as_str(), "cache");
        assert_eq!(PriceSource::Fixture.as_str(), "fixture");
        assert_eq!(PriceSource::default(), PriceSource::Live);
    }

    fn at(y: i32, m: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, day, 0, 0, 0).unwrap()
    }

    /// Serves fixed bars (or fails) and records every requested window.
    struct Bars {
        bars: Option<Vec<DailyBar>>,
        windows: std::sync::Mutex<Vec<(DateTime<Utc>, DateTime<Utc>)>>,
    }

    impl Bars {
        fn new(bars: Option<Vec<DailyBar>>) -> Self {
            Self {
                bars,
                windows: Default::default(),
            }
        }

        fn windows(&self) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
            self.windows.lock().unwrap().clone()
        }
    }

    impl DailyBarSource for Bars {
        async fn daily_bars(
            &self,
            _symbol: &str,
            from: DateTime<Utc>,
            to: DateTime<Utc>,
        ) -> Result<Vec<DailyBar>, QuoteError> {
            self.windows.lock().unwrap().push((from, to));
            self.bars
                .clone()
                .ok_or_else(|| QuoteError::Provider("down".to_string()))
        }
    }

    #[tokio::test]
    async fn test_bars_close_on_or_after_fetches_a_week_from_the_date() {
        let source = Bars::new(Some(bars()));
        let sat = at(2025, 1, 4);
        assert_eq!(
            bars_close_on_or_after(&source, "FAKE", sat).await.unwrap(),
            11.0
        );
        assert_eq!(source.windows(), vec![(sat, at(2025, 1, 11))]);

        let late = bars_close_on_or_after(&source, "FAKE", at(2025, 1, 8)).await;
        assert!(matches!(late, Err(QuoteError::Unavailable(s)) if s == "FAKE"));

        let down = Bars::new(None);
        let err = bars_close_on_or_after(&down, "FAKE", sat).await;
        assert!(matches!(err, Err(QuoteError::Provider(m)) if m == "down"));
    }

    #[tokio::test]
    async fn test_bars_close_before_fetches_ten_days_back() {
        let source = Bars::new(Some(bars()));
        let mon = at(2025, 1, 6);
        assert_eq!(bars_close_before(&source, "FAKE", mon).await.unwrap(), 10.0);
        assert_eq!(source.windows(), vec![(at(2024, 12, 27), at(2025, 1, 7))]);

        let early = bars_close_before(&source, "FAKE", at(2025, 1, 3)).await;
        assert!(matches!(early, Err(QuoteError::Unavailable(s)) if s == "FAKE"));

        let down = Bars::new(None);
        assert!(matches!(
            bars_close_before(&down, "FAKE", mon).await,
            Err(QuoteError::Provider(_))
        ));
    }

    /// Stand-in for the Yahoo backend with distinct values per lookup.
    struct StubYahoo;

    impl QuoteProvider for StubYahoo {
        async fn latest(&self, _symbol: &str) -> Result<Quote, QuoteError> {
            Ok(Quote {
                price: 7.0,
                currency: Some("USD".to_string()),
                as_of: None,
                source: PriceSource::Live,
            })
        }
        async fn previous_close(&self, _symbol: &str) -> Result<f64, QuoteError> {
            Ok(6.0)
        }
        async fn close_on(&self, _symbol: &str, _date: DateTime<Utc>) -> Result<f64, QuoteError> {
            Ok(5.0)
        }
        async fn close_before(
            &self,
            _symbol: &str,
            _date: DateTime<Utc>,
        ) -> Result<f64, QuoteError> {
            Ok(4.0)
        }
        async fn name(&self, symbol: &str) -> Option<String> {
            Some(format!("Stub {symbol}"))
        }
    }

    #[tokio::test]
    async fn test_yahoo_resolves_to_the_yahoo_backend() {
        let any = AnyProvider::Yahoo;
        let base = any.base(&StubYahoo);
        let date = at(2025, 1, 6);
        assert_eq!(base.latest("X").await.unwrap().price, 7.0);
        assert_eq!(base.previous_close("X").await.unwrap(), 6.0);
        assert_eq!(base.close_on("X", date).await.unwrap(), 5.0);
        assert_eq!(base.close_before("X", date).await.unwrap(), 4.0);
        assert_eq!(base.name("X").await.as_deref(), Some("Stub X"));
    }

    fn cached(inner: AnyProvider, path: Option<std::path::PathBuf>) -> AnyProvider {
        AnyProvider::Cached(std::sync::Arc::new(
            crate::services::quote_cache::CachedProvider::new(inner, path, 7, true),
        ))
    }

    #[tokio::test]
    async fn test_fixture_resolves_to_itself_not_the_backend() {
        let any = fixture();
        let base = any.base(&StubYahoo);
        let sat = at(2025, 1, 4);
        assert_eq!(base.latest("FAKE").await.unwrap().price, 2.5);
        assert_eq!(base.previous_close("FAKE").await.unwrap(), 2.0);
        assert_eq!(base.close_on("HIST", sat).await.unwrap(), 11.0);
        assert_eq!(base.close_before("HIST", sat).await.unwrap(), 10.0);
        assert_eq!(base.name("FAKE").await.as_deref(), Some("Fake Co"));
    }

    #[tokio::test]
    async fn test_offline_and_nested_cache_never_reach_the_backend() {
        let date = at(2025, 1, 6);
        let nested = cached(AnyProvider::Offline, None);
        for any in [AnyProvider::Offline, nested] {
            let base = any.base(&StubYahoo);
            assert!(matches!(
                base.latest("X").await,
                Err(QuoteError::Unavailable(_))
            ));
            assert!(base.previous_close("X").await.is_err());
            assert!(base.close_on("X", date).await.is_err());
            assert!(base.close_before("X", date).await.is_err());
            assert_eq!(base.name("X").await, None);
        }
    }

    const FIXTURE: &str = r#"{
        "latest": {"FAKE": {"price": 2.5, "currency": "EUR", "asOf": "2025-01-06T17:00:00Z"},
                   "ZERO": {"price": 0.0, "currency": "EUR"}},
        "previous_close": {"FAKE": 2.0, "NEG": -1.0},
        "close_on": {"FAKE": 1.5},
        "names": {"FAKE": "Fake Co"},
        "history": {"HIST": [{"date": "2025-01-03", "close": 10.0},
                             {"date": "2025-01-06", "close": 11.0}]}
    }"#;

    fn fixture() -> AnyProvider {
        AnyProvider::Fixture(Box::new(FixtureProvider::from_json(FIXTURE).unwrap()))
    }

    #[tokio::test]
    async fn test_fixture_lookups_through_any_provider() {
        let any = fixture();
        let sat = at(2025, 1, 4);
        let q = any.latest("FAKE").await.unwrap();
        assert_eq!(q.price, 2.5);
        assert_eq!(q.currency.as_deref(), Some("EUR"));
        assert_eq!(q.as_of.as_deref(), Some("2025-01-06T17:00:00Z"));
        assert_eq!(q.source, PriceSource::Fixture);
        assert!(any.latest("ZERO").await.is_err(), "non-positive price");
        assert_eq!(any.previous_close("FAKE").await.unwrap(), 2.0);
        assert!(any.previous_close("NEG").await.is_err());
        // Without history, close_on ignores the date.
        assert_eq!(any.close_on("FAKE", sat).await.unwrap(), 1.5);
        assert_eq!(any.close_before("FAKE", sat).await.unwrap(), 1.5);
        assert_eq!(any.close_on_or_after("HIST", sat).await.unwrap(), 11.0);
        assert_eq!(any.close_before("HIST", sat).await.unwrap(), 10.0);
        assert!(any.close_on("HIST", at(2025, 1, 7)).await.is_err());
        assert!(any.close_before("HIST", at(2025, 1, 3)).await.is_err());
        assert_eq!(any.name("FAKE").await.as_deref(), Some("Fake Co"));
        assert_eq!(any.name("OTHER").await, None);
        assert!(any.is_fixture());
        assert_eq!(any.last_known("FAKE"), None);
        assert_eq!(any.last_known_as_of("FAKE"), None);
    }

    #[tokio::test]
    async fn test_offline_any_provider_fails_every_lookup() {
        let any = AnyProvider::Offline;
        let date = at(2025, 1, 6);
        assert!(matches!(any.latest("X").await, Err(QuoteError::Unavailable(s)) if s == "X"));
        assert!(any.previous_close("X").await.is_err());
        assert!(any.close_on("X", date).await.is_err());
        assert!(any.close_on_or_after("X", date).await.is_err());
        assert!(any.close_before("X", date).await.is_err());
        assert_eq!(any.name("X").await, None);
        assert!(!any.is_fixture());
        assert_eq!(any.last_known("X"), None);
        assert_eq!(any.last_known_as_of("X"), None);
    }

    #[tokio::test]
    async fn test_cached_any_provider_dispatches_to_the_cache() {
        let any = cached(fixture(), None);
        let sat = at(2025, 1, 4);
        assert_eq!(any.latest("FAKE").await.unwrap().price, 2.5);
        assert_eq!(any.previous_close("FAKE").await.unwrap(), 2.0);
        assert_eq!(any.close_on("HIST", sat).await.unwrap(), 11.0);
        assert_eq!(any.close_on_or_after("HIST", sat).await.unwrap(), 11.0);
        assert_eq!(any.close_before("HIST", sat).await.unwrap(), 10.0);
        assert_eq!(any.name("FAKE").await.as_deref(), Some("Fake Co"));
        assert!(any.is_fixture());
        // The successful fetch above is now the last known price.
        assert_eq!(any.last_known("FAKE"), Some((2.5, Some("EUR".to_string()))));
        assert_eq!(
            any.last_known_as_of("FAKE").as_deref(),
            Some("2025-01-06T17:00:00Z")
        );
        assert!(!cached(AnyProvider::Offline, None).is_fixture());
    }

    #[test]
    fn test_source_names() {
        assert_eq!(AnyProvider::Yahoo.source_name(), "yahoo");
        assert_eq!(AnyProvider::Offline.source_name(), "offline");
        assert_eq!(fixture().source_name(), "fixture");
        assert_eq!(cached(AnyProvider::Yahoo, None).source_name(), "yahoo");
        assert_eq!(cached(fixture(), None).source_name(), "fixture");
    }

    #[tokio::test]
    async fn test_flush_cache_writes_and_survives_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".p.json.quotes-cache.json");
        let ok = cached(fixture(), Some(path.clone()));
        ok.latest("FAKE").await.unwrap();
        ok.flush_cache();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"FAKE\""), "{written}");

        // An unwritable cache only warns.
        let missing = tmp.path().join("missing").join(".p.json.quotes-cache.json");
        let bad = cached(fixture(), Some(missing.clone()));
        bad.latest("FAKE").await.unwrap();
        bad.flush_cache();
        assert!(!missing.exists());

        // Providers without a cache have nothing to write.
        fixture().flush_cache();
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}

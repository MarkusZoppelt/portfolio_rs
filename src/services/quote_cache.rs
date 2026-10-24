//! Last-known-price cache: a small JSON file next to the data file.
//!
//! File: `.<datafile>.quotes-cache.json` in the data file's directory, written
//! atomically with mode 0600. It holds only market data (symbol, price,
//! previous close, currency, timestamps), never holdings or quantities:
//!
//! ```json
//! {"version": 1,
//!  "quotes": {"SPY": {"price": 500.0, "previousClose": 495.0, "currency": "USD",
//!                     "asOf": "2026-01-02T21:00:00Z", "fetchedAt": "2026-01-02T22:00:00Z"}},
//!  "fx": {"EURUSD=X": {"price": 1.1, "currency": "USD", "asOf": "…", "fetchedAt": "…"}}}
//! ```
//!
//! A successful fetch updates the entry; a failed one falls back to it
//! ([`PriceSource::Cache`]) when it is not older than the max age, measured
//! from `asOf` (or `fetchedAt` when the provider reported no timestamp).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use eyre::Result;
use serde::{Deserialize, Serialize};

use super::quotes::{AnyProvider, PriceSource, Quote, QuoteError, QuoteProvider};

/// Default max age of a cached price before it no longer counts.
pub const DEFAULT_MAX_AGE_DAYS: i64 = 7;
const VERSION: u32 = 1;

/// One cached symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub price: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_close: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    pub fetched_at: String,
}

impl CacheEntry {
    /// `asOf`, else `fetchedAt`.
    fn timestamp(&self) -> Option<DateTime<Utc>> {
        self.as_of
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .or_else(|| DateTime::parse_from_rfc3339(&self.fetched_at).ok())
            .map(|t| t.with_timezone(&Utc))
    }
}

/// On-disk format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub quotes: BTreeMap<String, CacheEntry>,
    #[serde(default)]
    pub fx: BTreeMap<String, CacheEntry>,
    /// Historic closes used to fill purchase prices, keyed
    /// `SYMBOL|on|after|before|YYYY-MM-DD`; they never expire.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub closes: BTreeMap<String, f64>,
}

/// Largest accepted clock skew for a cached `asOf` in the future.
const MAX_FUTURE_SKEW: Duration = Duration::days(1);

/// Whether a cached entry can be served at all (any age): a positive finite
/// price, a parseable timestamp at most [`MAX_FUTURE_SKEW`] ahead, and a
/// currency for quotes (an FX pair implies its currencies).
fn usable(symbol: &str, e: &CacheEntry) -> bool {
    e.price > 0.0
        && e.price.is_finite()
        && e.timestamp()
            .is_some_and(|t| t <= Utc::now() + MAX_FUTURE_SKEW)
        && (is_fx(symbol) || e.currency.as_deref().is_some_and(|c| !c.trim().is_empty()))
}

fn close_key(symbol: &str, kind: &str, date: DateTime<Utc>) -> String {
    format!("{symbol}|{kind}|{}", date.format("%Y-%m-%d"))
}

fn is_fx(symbol: &str) -> bool {
    symbol.ends_with("=X")
}

/// Cache path for a data file: `<dir>/.<name>.quotes-cache.json`.
pub fn cache_path_for(data_file: &Path) -> Option<PathBuf> {
    let name = data_file.file_name()?.to_string_lossy();
    Some(data_file.with_file_name(format!(".{name}.quotes-cache.json")))
}

#[derive(Default)]
struct Inner {
    file: CacheFile,
    dirty: bool,
    /// Symbols whose latest price was served from the cache this process.
    served: HashSet<String>,
}

/// A provider behind the last-known-price cache.
pub struct CachedProvider {
    inner: AnyProvider,
    path: Option<PathBuf>,
    max_age: Duration,
    /// Whether successful fetches update the file (false with `--offline`).
    write: bool,
    state: Mutex<Inner>,
}

impl std::fmt::Debug for CachedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedProvider")
            .field("inner", &self.inner)
            .field("path", &self.path)
            .field("max_age", &self.max_age)
            .field("write", &self.write)
            .finish()
    }
}

impl CachedProvider {
    /// Wrap `inner`, reading `path` if it exists. An unreadable or invalid
    /// cache is ignored (with a warning) and replaced on the next write.
    pub fn new(inner: AnyProvider, path: Option<PathBuf>, max_age_days: i64, write: bool) -> Self {
        let file = path
            .as_deref()
            .and_then(|p| match std::fs::read_to_string(p) {
                Ok(json) => match serde_json::from_str::<CacheFile>(&json) {
                    Ok(f) if f.version == VERSION => Some(f),
                    Ok(f) => {
                        eprintln!(
                            "Warning: ignoring quote cache {} with unknown version {}",
                            p.display(),
                            f.version
                        );
                        None
                    }
                    Err(e) => {
                        eprintln!("Warning: ignoring invalid quote cache {}: {e}", p.display());
                        None
                    }
                },
                Err(_) => None,
            })
            .unwrap_or_default();
        Self {
            inner,
            path,
            max_age: Duration::days(max_age_days.max(0)),
            write,
            state: Mutex::new(Inner {
                file,
                ..Default::default()
            }),
        }
    }

    pub fn inner(&self) -> &AnyProvider {
        &self.inner
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Last known quote for `symbol` at any age (valid entries only).
    pub fn last_known_entry(&self, symbol: &str) -> Option<(f64, Option<String>)> {
        let st = self.lock();
        let map = if is_fx(symbol) {
            &st.file.fx
        } else {
            &st.file.quotes
        };
        map.get(symbol)
            .filter(|e| usable(symbol, e))
            .map(|e| (e.price, e.currency.clone()))
    }

    /// As-of (else fetched-at) of the entry [`Self::last_known_entry`] returns.
    pub fn last_known_as_of(&self, symbol: &str) -> Option<String> {
        let st = self.lock();
        let map = if is_fx(symbol) {
            &st.file.fx
        } else {
            &st.file.quotes
        };
        map.get(symbol)
            .filter(|e| usable(symbol, e))
            .map(|e| e.as_of.clone().unwrap_or_else(|| e.fetched_at.clone()))
    }

    async fn cached_close(
        &self,
        symbol: &str,
        kind: &str,
        date: DateTime<Utc>,
        fetched: Result<f64, QuoteError>,
    ) -> Result<f64, QuoteError> {
        let key = close_key(symbol, kind, date);
        match fetched {
            Ok(v) if v > 0.0 && v.is_finite() => {
                let mut st = self.lock();
                if st.file.closes.get(&key) != Some(&v) {
                    st.file.closes.insert(key, v);
                    st.dirty = true;
                }
                Ok(v)
            }
            other => self
                .lock()
                .file
                .closes
                .get(&key)
                .copied()
                .filter(|v| *v > 0.0 && v.is_finite())
                .map_or(other, Ok),
        }
    }

    fn fresh(&self, entry: &CacheEntry) -> bool {
        entry
            .timestamp()
            .is_some_and(|t| Utc::now() - t <= self.max_age)
    }

    fn store(&self, symbol: &str, quote: &Quote) {
        let mut st = self.lock();
        st.served.remove(symbol);
        let map = if is_fx(symbol) {
            &mut st.file.fx
        } else {
            &mut st.file.quotes
        };
        map.insert(
            symbol.to_string(),
            CacheEntry {
                price: quote.price,
                // Set by the matching previous_close fetch.
                previous_close: None,
                currency: quote.currency.clone(),
                as_of: quote.as_of.clone(),
                fetched_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            },
        );
        st.dirty = true;
    }

    /// Write the file if anything changed. No-op without a path or with
    /// writes disabled.
    pub fn flush(&self) -> Result<()> {
        let (Some(path), true) = (&self.path, self.write) else {
            return Ok(());
        };
        let json = {
            let mut st = self.lock();
            if !st.dirty {
                return Ok(());
            }
            st.dirty = false;
            st.file.version = VERSION;
            serde_json::to_vec_pretty(&st.file)?
        };
        super::atomic_write::write_atomic_private(path, &json)
    }

    /// Snapshot of the cache contents (tests, diagnostics).
    pub fn snapshot(&self) -> CacheFile {
        self.lock().file.clone()
    }
}

impl QuoteProvider for CachedProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        match self.inner.base_latest(symbol).await {
            Ok(quote) => {
                self.store(symbol, &quote);
                Ok(quote)
            }
            Err(err) => {
                let mut st = self.lock();
                let map = if is_fx(symbol) {
                    &st.file.fx
                } else {
                    &st.file.quotes
                };
                match map
                    .get(symbol)
                    .filter(|e| usable(symbol, e) && self.fresh(e))
                    .cloned()
                {
                    Some(e) => {
                        st.served.insert(symbol.to_string());
                        Ok(Quote {
                            price: e.price,
                            currency: e.currency,
                            as_of: e.as_of.or(Some(e.fetched_at)),
                            source: PriceSource::Cache,
                        })
                    }
                    _ => Err(err),
                }
            }
        }
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        // A cached previous close only pairs with a cached price.
        let served = self.lock().served.contains(symbol);
        if served {
            return self
                .lock()
                .file
                .quotes
                .get(symbol)
                .and_then(|e| e.previous_close)
                .filter(|v| *v > 0.0 && v.is_finite())
                .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()));
        }
        let close = self.inner.base_previous_close(symbol).await?;
        let mut st = self.lock();
        if let Some(e) = st.file.quotes.get_mut(symbol) {
            e.previous_close = Some(close);
            st.dirty = true;
        }
        Ok(close)
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        let fetched = self.inner.base_close_on(symbol, date).await;
        self.cached_close(symbol, "on", date, fetched).await
    }

    async fn close_on_or_after(
        &self,
        symbol: &str,
        date: DateTime<Utc>,
    ) -> Result<f64, QuoteError> {
        let fetched = self.inner.base_close_on(symbol, date).await;
        self.cached_close(symbol, "on", date, fetched).await
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        let fetched = self.inner.base_close_before(symbol, date).await;
        self.cached_close(symbol, "before", date, fetched).await
    }

    fn last_known(&self, symbol: &str) -> Option<(f64, Option<String>)> {
        self.last_known_entry(symbol)
    }
    fn last_known_as_of(&self, symbol: &str) -> Option<String> {
        CachedProvider::last_known_as_of(self, symbol)
    }
    fn is_fixture(&self) -> bool {
        self.inner.is_fixture()
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        self.inner.base_name(symbol).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::quotes::FixtureProvider;

    fn fixture(json: &str) -> AnyProvider {
        AnyProvider::Fixture(Box::new(FixtureProvider::from_json(json).unwrap()))
    }

    #[tokio::test]
    async fn test_fetch_updates_cache_and_failure_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("positions.json");
        let path = cache_path_for(&data).unwrap();
        assert!(path.ends_with(".positions.json.quotes-cache.json"));

        let live = CachedProvider::new(
            fixture(
                r#"{"latest": {"FAKE": {"price": 2.5, "currency": "EUR"}, "EURUSD=X": {"price": 1.1}},
                    "previous_close": {"FAKE": 2.0}}"#,
            ),
            Some(path.clone()),
            7,
            true,
        );
        assert_eq!(live.latest("FAKE").await.unwrap().price, 2.5);
        assert_eq!(live.previous_close("FAKE").await.unwrap(), 2.0);
        live.latest("EURUSD=X").await.unwrap();
        live.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let snap: CacheFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(snap.quotes["FAKE"].previous_close, Some(2.0));
        assert!(snap.fx.contains_key("EURUSD=X"));
        assert!(!data.exists(), "only the cache file is written");

        let offline = CachedProvider::new(AnyProvider::Offline, Some(path.clone()), 7, false);
        let q = offline.latest("FAKE").await.unwrap();
        assert_eq!((q.price, q.source), (2.5, PriceSource::Cache));
        assert!(q.as_of.is_some());
        assert_eq!(offline.previous_close("FAKE").await.unwrap(), 2.0);
        assert!(offline.latest("OTHER").await.is_err());

        // Too old: does not count.
        let expired = CachedProvider::new(AnyProvider::Offline, Some(path.clone()), 0, false);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(expired.latest("FAKE").await.is_err());
    }

    fn write(path: &Path, json: &str) {
        std::fs::write(path, json).unwrap();
    }

    /// Finance retest B5/B6/L3: future timestamps beyond a day, entries
    /// without a currency and unknown versions are not served.
    #[tokio::test]
    async fn test_invalid_entries_and_versions_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".p.json.quotes-cache.json");
        let now = Utc::now();
        let ts = |d: Duration| (now + d).to_rfc3339();
        let file = serde_json::json!({
            "version": 1,
            "quotes": {
                "OK": {"price": 1.0, "currency": "EUR", "asOf": ts(Duration::hours(-1)), "fetchedAt": ts(Duration::zero())},
                "SKEW": {"price": 1.0, "currency": "EUR", "asOf": ts(Duration::hours(12)), "fetchedAt": ts(Duration::zero())},
                "FUTURE": {"price": 1.0, "currency": "EUR", "asOf": "2030-01-01T00:00:00Z", "fetchedAt": "2030-01-01T00:00:00Z"},
                "NOCCY": {"price": 1.0, "asOf": ts(Duration::hours(-1)), "fetchedAt": ts(Duration::zero())},
                "BLANKCCY": {"price": 1.0, "currency": " ", "asOf": ts(Duration::hours(-1)), "fetchedAt": ts(Duration::zero())}
            },
            "fx": {"EURUSD=X": {"price": 1.1, "asOf": ts(Duration::hours(-1)), "fetchedAt": ts(Duration::zero())}}
        });
        write(&path, &file.to_string());
        let c = CachedProvider::new(AnyProvider::Offline, Some(path.clone()), 7, false);
        assert!(c.latest("OK").await.is_ok());
        assert!(c.latest("SKEW").await.is_ok(), "small skew is tolerated");
        assert!(c.latest("FUTURE").await.is_err());
        assert!(c.latest("NOCCY").await.is_err());
        assert!(c.latest("BLANKCCY").await.is_err());
        assert!(
            c.latest("EURUSD=X").await.is_ok(),
            "FX pairs need no currency"
        );
        assert!(c.last_known_entry("FUTURE").is_none());

        let mut v99 = file.clone();
        v99["version"] = 99.into();
        write(&path, &v99.to_string());
        let c = CachedProvider::new(AnyProvider::Offline, Some(path.clone()), 7, false);
        assert!(c.latest("OK").await.is_err(), "unknown version is ignored");
        assert!(c.last_known_entry("OK").is_none());
    }

    /// B7 / A5: historic closes are cached for offline cost basis; the last
    /// known price is available beyond the max age.
    #[tokio::test]
    async fn test_closes_cached_and_last_known_ignores_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".p.json.quotes-cache.json");
        let date = DateTime::parse_from_rfc3339("2025-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let live = CachedProvider::new(
            fixture(
                r#"{"latest": {"FAKE": {"price": 2.5, "currency": "EUR"}}, "close_on": {"FAKE": 2.0}}"#,
            ),
            Some(path.clone()),
            7,
            true,
        );
        live.latest("FAKE").await.unwrap();
        assert_eq!(live.close_on_or_after("FAKE", date).await.unwrap(), 2.0);
        live.flush().unwrap();
        let offline = CachedProvider::new(AnyProvider::Offline, Some(path.clone()), 0, false);
        assert_eq!(offline.close_on_or_after("FAKE", date).await.unwrap(), 2.0);
        assert!(offline
            .close_on_or_after("FAKE", date + Duration::days(1))
            .await
            .is_err());
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            offline.latest("FAKE").await.is_err(),
            "expired for current values"
        );
        assert_eq!(offline.last_known_entry("FAKE").map(|e| e.0), Some(2.5));
    }
}

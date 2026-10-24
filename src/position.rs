use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::prelude::*;
use eyre::{Result, WrapErr};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use yahoo_finance_api as yahoo;

use crate::money::{dec, div, percent_change, to_f64, Decimal};
use crate::services::quotes::{Quote, QuoteError, QuoteProvider, YahooProvider};

// Type alias to reduce type complexity
type HistoricCacheMap = HashMap<(String, i64), Arc<yahoo::YResponse>>;
// Caches for Yahoo API requests
static QUOTE_CACHE: Lazy<Mutex<HashMap<String, Arc<yahoo::YResponse>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static PREV_CLOSE_CACHE: Lazy<Mutex<HashMap<String, f64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static HISTORIC_CACHE: Lazy<Mutex<HistoricCacheMap>> = Lazy::new(|| Mutex::new(HashMap::new()));
static NAME_CACHE: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Purchase {
    // Optional ISO date string (e.g., 2024-01-15)
    pub date: Option<String>,
    /// Money and quantities are [`Decimal`], read and written as JSON numbers.
    #[serde(with = "rust_decimal::serde::float")]
    pub quantity: Decimal,
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub price: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub fees: Option<Decimal>,
    /// Historic close filled in for a missing price. Used for calculations,
    /// never serialized, so guesses are not saved back to the file.
    #[serde(skip)]
    pub estimated_price: Option<Decimal>,
}

impl Purchase {
    /// The recorded price, or the estimated close when none was recorded.
    pub fn effective_price(&self) -> Option<Decimal> {
        match self.price {
            Some(p) if p > Decimal::ZERO => Some(p),
            _ => self.estimated_price.or(self.price),
        }
    }
}

/// Whether a ticker position has a usable market quote.
///
/// Defaults to [`PriceStatus::Live`] so deserialization does not mark every
/// holding unavailable before quotes are loaded. A ticker position still has
/// no market price until `last_spot` is positive; see
/// [`PortfolioPosition::has_market_price`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceStatus {
    #[default]
    Live,
    Unavailable,
    /// A quote exists, but it is in a foreign currency and no FX rate to the
    /// base currency could be loaded. Excluded from totals, never mixed in.
    FxUnavailable,
}

/// Yahoo FX symbol and scale needed to convert `quote_ccy` into `base`.
///
/// An empty symbol with a scale (e.g. base GBP, quote `GBp`) means "no FX
/// request, multiply by the scale".
///
/// Returns `None` when no conversion is needed (same currency, compared
/// case-insensitively after normalising minor units, or empty input). Otherwise
/// returns `("{BASE}{QUOTE}=X", scale)`; that symbol's price is QUOTE per 1 BASE,
/// so the position's rate is `scale / fx_price` base units per quote unit.
/// Yahoo minor units are normalised: `GBp`/`GBX` -> GBP, `ZAc` -> ZAR,
/// `ILA` -> ILS, each with scale 0.01.
pub fn fx_symbol(base: &str, quote_ccy: &str) -> Option<(String, f64)> {
    let base = base.trim().to_uppercase();
    let quote_ccy = quote_ccy.trim();
    if base.is_empty() || quote_ccy.is_empty() {
        return None;
    }
    let (major, scale) = match quote_ccy {
        "GBp" | "GBX" | "GBx" => ("GBP".to_string(), 0.01),
        "ZAc" | "ZAC" => ("ZAR".to_string(), 0.01),
        "ILA" => ("ILS".to_string(), 0.01),
        other => (other.to_uppercase(), 1.0),
    };
    if major == base {
        if scale == 1.0 {
            return None;
        }
        // Same currency in minor units: empty symbol means no FX request,
        // the rate is just the scale.
        return Some((String::new(), scale));
    }
    Some((format!("{base}{major}=X"), scale))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortfolioPosition {
    /// Stable id (`pos_<uuid>`). Optional in the file; generated on first write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) ticker: Option<String>,
    pub(crate) asset_class: String,
    #[serde(with = "rust_decimal::serde::float")]
    pub(crate) amount: Decimal,

    #[serde(skip_serializing, skip_deserializing)]
    pub(crate) last_spot: Decimal,

    // Not persisted. Defaults to Live; ticker positions are unpriced until a quote lands.
    #[serde(skip_serializing, skip_deserializing)]
    pub(crate) price_status: PriceStatus,

    // Not persisted. Currency of `last_spot` as reported by Yahoo (None = unknown,
    // treated as the base currency).
    #[serde(skip_serializing, skip_deserializing)]
    pub(crate) price_currency: Option<String>,

    // Not persisted. Base-currency units per 1 unit of `price_currency`.
    // None means no conversion (same currency or unknown).
    #[serde(skip_serializing, skip_deserializing)]
    pub(crate) fx_rate: Option<Decimal>,

    // Optional list of historical purchases to compute cost basis and PnL
    #[serde(default)]
    pub(crate) purchases: Vec<Purchase>,

    // Previous close used to compute daily variation
    #[serde(skip_serializing, skip_deserializing)]
    pub(crate) previous_close: Option<Decimal>,
}

/// A new stable position id: `pos_` + a time-ordered UUID v7 (encodes only creation time).
pub fn new_position_id() -> String {
    format!("pos_{}", uuid::Uuid::now_v7().simple())
}

/// Give every position without an id a new one. Returns whether any changed.
pub fn ensure_position_ids(positions: &mut [PortfolioPosition]) -> bool {
    let mut changed = false;
    for position in positions.iter_mut().filter(|p| p.id.is_none()) {
        position.id = Some(new_position_id());
        changed = true;
    }
    changed
}

/// [`ensure_position_ids`] on raw JSON objects, keeping every other field as is.
pub fn ensure_json_position_ids(positions: &mut [serde_json::Value]) -> bool {
    let mut changed = false;
    for obj in positions.iter_mut().filter_map(|v| v.as_object_mut()) {
        if !obj.get("Id").is_some_and(|id| id.is_string()) {
            obj.insert(
                "Id".to_string(),
                serde_json::Value::String(new_position_id()),
            );
            changed = true;
        }
    }
    changed
}

impl PortfolioPosition {
    /// Stable id, if the position has been written since ids were introduced.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn update_price(&mut self, last_spot: f64) {
        self.last_spot = dec(last_spot);
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
        to_f64(self.balance_dec())
    }

    /// Market value in the base currency (see [`Self::get_balance`]).
    pub fn balance_dec(&self) -> Decimal {
        if let Some(_ticker) = &self.ticker {
            // An unavailable quote or FX rate is not a real zero-value holding. Never guess.
            if self.price_status != PriceStatus::Live {
                return Decimal::ZERO;
            }
            // Use purchased quantity if available, otherwise fall back to amount
            self.last_spot * self.quantity_dec() * self.fx_factor_dec()
        } else {
            self.amount
        }
    }

    pub fn get_amount(&self) -> f64 {
        to_f64(self.quantity_dec())
    }

    /// Held quantity: sum of purchase quantities, or `Amount` without purchases.
    pub fn quantity_dec(&self) -> Decimal {
        if !self.purchases.is_empty() {
            self.purchases.iter().map(|p| p.quantity).sum()
        } else {
            self.amount
        }
    }

    pub fn get_ticker(&self) -> Option<&str> {
        self.ticker.as_deref()
    }

    /// Cash: no ticker and asset class "cash", compared case-insensitively.
    pub fn is_cash(&self) -> bool {
        self.ticker.is_none() && self.asset_class.eq_ignore_ascii_case("cash")
    }

    pub fn get_name_option(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn set_amount(&mut self, amount: f64) {
        self.amount = dec(amount);
    }

    /// Latest price in the quote currency ([`Self::price_currency`]), not converted.
    pub fn market_price(&self) -> f64 {
        to_f64(self.last_spot)
    }

    /// Currency of [`Self::market_price`] as reported by the quote, if known.
    pub fn price_currency(&self) -> Option<&str> {
        self.price_currency.as_deref()
    }

    /// Base-currency units per 1 unit of the quote currency, if a conversion applies.
    pub fn fx_rate(&self) -> Option<f64> {
        self.fx_rate.map(to_f64)
    }

    /// [`Self::fx_rate`], or 1 when no conversion applies.
    pub fn fx_factor_dec(&self) -> Decimal {
        self.fx_rate.unwrap_or(Decimal::ONE)
    }

    pub fn price_status(&self) -> PriceStatus {
        self.price_status
    }

    /// True for ticker-less holdings, and for ticker holdings only when a live
    /// quote produced a positive price.
    pub fn has_market_price(&self) -> bool {
        if self.ticker.is_none() {
            return true;
        }
        self.price_status == PriceStatus::Live && self.last_spot > Decimal::ZERO
    }

    /// Market value in the base currency.
    pub fn market_value(&self) -> f64 {
        self.get_balance()
    }

    /// Average purchase cost per unit in the quote currency (shown next to
    /// [`Self::market_price`]).
    pub fn average_cost(&self) -> Option<f64> {
        self.average_cost_dec().map(to_f64)
    }

    /// Purchases with a positive price, as `(quantity, cost incl. fees)`.
    fn priced_purchases(&self) -> impl Iterator<Item = (Decimal, Decimal)> + '_ {
        self.purchases
            .iter()
            .filter_map(|p| p.effective_price().map(|price| (price, p)))
            .filter(|(price, _)| *price > Decimal::ZERO)
            .map(|(price, p)| {
                let quantity = p.quantity;
                (quantity, quantity * price + p.fees.unwrap_or(Decimal::ZERO))
            })
    }

    /// [`Self::average_cost`] as a [`Decimal`].
    pub fn average_cost_dec(&self) -> Option<Decimal> {
        if self.purchases.is_empty() {
            return None;
        }
        let (quantity, cost) = self
            .priced_purchases()
            .fold((Decimal::ZERO, Decimal::ZERO), |(q, c), (pq, pc)| {
                (q + pq, c + pc)
            });
        if quantity > Decimal::ZERO {
            div(cost, quantity)
        } else {
            None
        }
    }

    /// Total invested amount in the base currency.
    ///
    /// Purchase prices are interpreted as quote-currency prices (as filled from
    /// Yahoo history) and converted at the CURRENT FX rate. This is an
    /// approximation: it ignores FX moves since purchase.
    pub fn total_invested(&self) -> Option<f64> {
        self.total_invested_dec().map(to_f64)
    }

    /// [`Self::total_invested`] as a [`Decimal`].
    pub fn total_invested_dec(&self) -> Option<Decimal> {
        if self.purchases.is_empty() {
            return None;
        }
        let invested = self
            .priced_purchases()
            .map(|(_, cost)| cost)
            .sum::<Decimal>()
            * self.fx_factor_dec();
        (invested > Decimal::ZERO).then_some(invested)
    }

    pub fn pnl(&self) -> Option<f64> {
        self.pnl_dec().map(to_f64)
    }

    /// [`Self::pnl`] as a [`Decimal`].
    pub fn pnl_dec(&self) -> Option<Decimal> {
        if !self.has_market_price() {
            return None;
        }
        let invested = self.total_invested_dec()?;
        Some(self.balance_dec() - invested)
    }

    pub fn historic_variation_percent(&self) -> Option<f64> {
        if !self.has_market_price() {
            return None;
        }
        percent_change(self.balance_dec(), self.total_invested_dec()?).map(to_f64)
    }

    /// Base-currency value at the previous close (current quantity and FX
    /// rate), or the current value when no daily change is known.
    pub fn previous_day_value_dec(&self) -> Decimal {
        let value = self.balance_dec();
        match (self.daily_variation_percent(), self.previous_close) {
            (Some(_), Some(prev)) => div(value * prev, self.last_spot).unwrap_or(value),
            _ => value,
        }
    }

    /// Base-currency change since the previous close, if a daily change is known.
    pub fn day_pnl_dec(&self) -> Option<Decimal> {
        self.daily_variation_percent()?;
        Some(self.balance_dec() - self.previous_day_value_dec())
    }

    pub fn daily_variation_percent(&self) -> Option<f64> {
        if !self.has_market_price() {
            return None;
        }
        percent_change(self.last_spot, self.previous_close?).map(to_f64)
    }

    pub fn get_purchases(&self) -> &[Purchase] {
        &self.purchases
    }

    pub fn get_previous_close(&self) -> Option<f64> {
        self.previous_close.map(to_f64)
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        ticker: Option<&str>,
        asset_class: &str,
        amount: f64,
        last_spot: f64,
        price_status: PriceStatus,
    ) -> Self {
        Self {
            id: None,
            name: None,
            ticker: ticker.map(str::to_string),
            asset_class: asset_class.to_string(),
            amount: dec(amount),
            last_spot: dec(last_spot),
            purchases: Vec::new(),
            previous_close: None,
            price_status,
            price_currency: None,
            fx_rate: None,
        }
    }
}

pub fn from_string(data: &str) -> Result<Vec<PortfolioPosition>> {
    serde_json::from_str::<Vec<PortfolioPosition>>(data)
        .wrap_err("failed to parse portfolio JSON - data is not well-formatted")
}

// Get the latest price for a ticker, cache on success, fallback to cache on failure
async fn get_quote_price(ticker: &str) -> Result<Arc<yahoo::YResponse>, yahoo::YahooError> {
    match yahoo::YahooConnector::new()?
        .get_latest_quotes(ticker, "1d")
        .await
    {
        Ok(resp) => {
            let response = Arc::new(resp);
            if let Ok(mut cache) = QUOTE_CACHE.lock() {
                cache.insert(ticker.to_string(), response.clone());
            }
            Ok(response)
        }
        Err(e) => {
            if let Ok(cache) = QUOTE_CACHE.lock() {
                if let Some(cached) = cache.get(ticker) {
                    Ok(Arc::clone(cached))
                } else {
                    Err(e)
                }
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
    match yahoo::YahooConnector::new()?
        .get_quote_history(ticker, start, end)
        .await
    {
        Ok(resp) => {
            let quotes = resp.quotes()?;
            let prev_close = if quotes.len() >= 2 {
                quotes[quotes.len() - 2].close
            } else if let Some(last) = quotes.last() {
                last.close
            } else {
                return Err(yahoo::YahooError::NoResult);
            };
            if let Ok(mut cache) = PREV_CLOSE_CACHE.lock() {
                cache.insert(ticker.to_string(), prev_close);
            }
            Ok(prev_close)
        }
        Err(e) => {
            if let Ok(cache) = PREV_CLOSE_CACHE.lock() {
                if let Some(&cached) = cache.get(ticker) {
                    Ok(cached)
                } else {
                    Err(e)
                }
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
    // SAFETY: Valid chrono timestamps are always valid OffsetDateTime timestamps
    let start = OffsetDateTime::from_unix_timestamp(date.timestamp())
        .expect("valid chrono timestamp should convert to OffsetDateTime");

    // get a range of 3 days in case the market is closed on the given date
    let end = start + time::Duration::days(3);
    let cache_key = (ticker.to_string(), date.timestamp());

    match yahoo::YahooConnector::new()?
        .get_quote_history(ticker, start, end)
        .await
    {
        Ok(resp) => {
            let response = Arc::new(resp);
            if let Ok(mut cache) = HISTORIC_CACHE.lock() {
                cache.insert(cache_key.clone(), response.clone());
            }
            Ok(response)
        }
        Err(e) => {
            if let Ok(cache) = HISTORIC_CACHE.lock() {
                if let Some(cached) = cache.get(&cache_key) {
                    Ok(Arc::clone(cached))
                } else {
                    Err(e)
                }
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
                if let Ok(mut cache) = NAME_CACHE.lock() {
                    cache.insert(ticker.to_string(), item.short_name.clone());
                }
                Ok(item.short_name.clone())
            } else {
                Err(yahoo::YahooError::NoResult)
            }
        }
        Err(e) => {
            if let Ok(cache) = NAME_CACHE.lock() {
                if let Some(cached) = cache.get(ticker) {
                    Ok(cached.clone())
                } else {
                    Err(e)
                }
            } else {
                Err(e)
            }
        }
    }
}

/// Positive close from a quote response, if one is present.
fn usable_close(quote: &yahoo::YResponse) -> Option<f64> {
    if let Ok(last) = quote.last_quote() {
        if last.close > 0.0 {
            return Some(last.close);
        }
    }
    // Market may be closed: fall back to the last bar with a positive close.
    if let Ok(quotes) = quote.quotes() {
        if let Some(last) = quotes.last() {
            if last.close > 0.0 {
                return Some(last.close);
            }
        }
    }
    None
}

/// Yahoo-backed lookups for [`crate::services::quotes::YahooProvider`].
pub(crate) async fn yahoo_latest(symbol: &str) -> Result<Quote, QuoteError> {
    let quote = get_quote_price(symbol)
        .await
        .map_err(|e| QuoteError::Provider(e.to_string()))?;
    let price = usable_close(&quote).ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))?;
    let currency = quote.metadata().ok().and_then(|m| m.currency);
    Ok(Quote { price, currency })
}

pub(crate) async fn yahoo_previous_close(symbol: &str) -> Result<f64, QuoteError> {
    get_previous_close(symbol)
        .await
        .map_err(|e| QuoteError::Provider(e.to_string()))
}

pub(crate) async fn yahoo_close_on(symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
    let resp = get_historic_price(symbol, date)
        .await
        .map_err(|e| QuoteError::Provider(e.to_string()))?;
    if let Ok(q) = resp.last_quote() {
        return Ok(q.close);
    }
    resp.quotes()
        .ok()
        .and_then(|quotes| quotes.last().map(|q| q.close))
        .ok_or_else(|| QuoteError::Unavailable(symbol.to_string()))
}

pub(crate) async fn yahoo_name(symbol: &str) -> Option<String> {
    get_quote_name(symbol).await.ok()
}

/// Apply a quote lookup to `position` without dropping it.
///
/// `Some(close)` with `close > 0` marks the position [`PriceStatus::Live`].
/// `None` (lookup failed, or the quote had no usable close) sets `last_spot`
/// to 0 and [`PriceStatus::Unavailable`]. Every other field is left unchanged.
pub(crate) fn apply_quote_result(position: &mut PortfolioPosition, close: Option<f64>) {
    match close {
        Some(close) if close > 0.0 => {
            position.update_price(close);
            position.price_status = PriceStatus::Live;
        }
        _ => {
            position.update_price(0.0);
            position.price_status = PriceStatus::Unavailable;
        }
    }
}

/// Refresh `position` with the latest quote and return the updated position.
///
/// A failed quote lookup, a quote with no usable close, or a failed name
/// lookup does not drop the position. Name lookup failure leaves `name` unset
/// ([`PortfolioPosition::get_name`] falls back to the ticker). The `Err`
/// variant is retained for unexpected failures; quote and name failures return `Ok`.
pub async fn handle_position(
    position: &mut PortfolioPosition,
) -> Result<PortfolioPosition, yahoo::YahooError> {
    Ok(handle_position_with(&YahooProvider, position).await)
}

/// [`handle_position`] with an explicit [`QuoteProvider`]. Never drops the position.
pub async fn handle_position_with<P: QuoteProvider>(
    provider: &P,
    position: &mut PortfolioPosition,
) -> PortfolioPosition {
    if let Some(ticker_owned) = position.ticker.clone() {
        let ticker = ticker_owned.as_str();
        let (close, currency) = match provider.latest(ticker).await {
            Ok(quote) => (Some(quote.price), quote.currency),
            Err(_) => (None, None),
        };
        position.price_currency = currency;
        apply_quote_result(position, close);

        // Quote-dependent enrichment is useless without a live price, and a
        // failed name lookup must not discard an otherwise valid position.
        if position.price_status == PriceStatus::Live {
            if let Ok(prev_close) = provider.previous_close(ticker).await {
                position.previous_close = Some(dec(prev_close));
            }

            if !position.purchases.is_empty() {
                let mut filled_purchases = Vec::with_capacity(position.purchases.len());
                for mut p in position.purchases.clone() {
                    let needs_price = p.price.map(|v| v <= Decimal::ZERO).unwrap_or(true);
                    if needs_price && p.quantity > Decimal::ZERO {
                        if let Some(date) = p.date.as_deref().and_then(parse_purchase_date) {
                            if let Ok(close) = provider.close_on(ticker, date).await {
                                p.estimated_price = Some(dec(close.max(0.0)));
                            }
                        }
                    }
                    filled_purchases.push(p);
                }
                position.purchases = filled_purchases;
            }

            if position.name.is_none() {
                if let Some(name) = provider.name(ticker).await {
                    position.name = Some(name);
                }
            }
        }
    }

    position.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_id_is_optional_and_not_invented_on_read() {
        let legacy = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 1}]"#;
        let positions = from_string(legacy).unwrap();
        assert_eq!(positions[0].id(), None);
        let out = serde_json::to_string(&positions).unwrap();
        assert!(!out.contains("\"Id\""), "{out}");

        let with_id = r#"[{"Id": "pos_fixed", "Name": "Cash", "AssetClass": "Cash", "Amount": 1}]"#;
        let mut positions = from_string(with_id).unwrap();
        assert!(!ensure_position_ids(&mut positions));
        assert_eq!(positions[0].id(), Some("pos_fixed"));
        assert!(serde_json::to_string(&positions)
            .unwrap()
            .contains("\"Id\":\"pos_fixed\""));
    }

    #[test]
    fn test_ensure_json_position_ids_keeps_other_fields() {
        let mut values: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"Name": "A", "Custom": 1}, {"Id": "pos_keep", "Name": "B"}]"#,
        )
        .unwrap();
        assert!(ensure_json_position_ids(&mut values));
        assert!(values[0]["Id"].as_str().unwrap().starts_with("pos_"));
        assert_eq!(values[0]["Custom"], 1);
        assert_eq!(values[1]["Id"], "pos_keep");
        assert_ne!(new_position_id(), new_position_id());
    }

    #[test]
    fn test_new_position_id_is_time_ordered() {
        let a = new_position_id();
        let b = new_position_id();
        assert!(a.starts_with("pos_"), "{a}");
        assert!(b.starts_with("pos_"), "{b}");
        assert_eq!(a.len(), 36, "{a}");
        assert_eq!(b.len(), 36, "{b}");
        assert!(a <= b, "{a} should sort before {b}");
    }

    use std::fs;

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_get_quote_name() {
        let name = get_quote_name("AAPL").await.unwrap();
        assert_eq!(name, "Apple Inc.");

        let name = get_quote_name("BTC-EUR").await.unwrap();
        assert_eq!(name, "Bitcoin EUR");
    }

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_get_quote_price() {
        let quote = get_quote_price("AAPL").await.unwrap();
        assert!(quote.last_quote().unwrap().close > 0.0);
    }

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_get_historic_price() {
        let date = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let quote = get_historic_price("AAPL", date).await.unwrap();
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

    #[tokio::test]
    async fn test_from_file() {
        let positions_str = fs::read_to_string("example_data.json").unwrap();
        let positions = from_string(&positions_str).unwrap();
        assert_eq!(positions.len(), 6);
        assert!(positions
            .iter()
            .all(|p| p.price_status() == PriceStatus::Live));
    }

    #[test]
    fn test_apply_quote_result_failed_quote_keeps_position() {
        let mut position = PortfolioPosition {
            id: None,
            name: Some("S&P 500".to_string()),
            ticker: Some("SPY".to_string()),
            asset_class: "Stocks".to_string(),
            amount: dec(4.0),
            last_spot: dec(500.0),
            price_status: PriceStatus::Live,
            price_currency: None,
            fx_rate: None,
            purchases: vec![Purchase {
                date: Some("2024-01-15".to_string()),
                quantity: dec(4.0),
                price: Some(dec(400.0)),
                fees: None,
                estimated_price: None,
            }],
            previous_close: Some(dec(490.0)),
        };

        apply_quote_result(&mut position, None);

        assert_eq!(position.price_status(), PriceStatus::Unavailable);
        assert_eq!(position.market_price(), 0.0);
        assert!(!position.has_market_price());
        assert_eq!(position.get_balance(), 0.0);
        assert!(position.pnl().is_none());
        assert_eq!(position.get_ticker(), Some("SPY"));
        assert_eq!(position.get_name(), "S&P 500");
        assert_eq!(position.get_asset_class(), "Stocks");
        assert_eq!(position.get_amount(), 4.0);
        assert_eq!(position.get_purchases().len(), 1);
        assert_eq!(position.get_previous_close(), Some(490.0));
    }

    #[test]
    fn test_apply_quote_result_no_usable_close_is_unavailable() {
        let mut position =
            PortfolioPosition::for_test(Some("BTC-USD"), "Crypto", 1.0, 20.0, PriceStatus::Live);
        apply_quote_result(&mut position, Some(0.0));
        assert_eq!(position.price_status(), PriceStatus::Unavailable);
        assert_eq!(position.market_price(), 0.0);
        assert_eq!(position.get_ticker(), Some("BTC-USD"));
        assert_eq!(position.get_balance(), 0.0);

        apply_quote_result(&mut position, Some(-1.0));
        assert_eq!(position.price_status(), PriceStatus::Unavailable);
    }

    #[test]
    fn test_apply_quote_result_positive_close_is_live() {
        let mut position =
            PortfolioPosition::for_test(Some("SPY"), "Stocks", 2.0, 0.0, PriceStatus::Unavailable);
        apply_quote_result(&mut position, Some(450.25));
        assert_eq!(position.price_status(), PriceStatus::Live);
        assert_eq!(position.market_price(), 450.25);
        assert!(position.has_market_price());
        assert_eq!(position.get_balance(), 900.5);
    }

    #[test]
    fn test_unavailable_balance_ignores_stale_spot() {
        let position = PortfolioPosition::for_test(
            Some("SPY"),
            "Stocks",
            10.0,
            999.0,
            PriceStatus::Unavailable,
        );
        assert_eq!(position.get_balance(), 0.0);
        assert!(!position.has_market_price());
    }

    #[test]
    fn test_tickerless_always_has_market_price() {
        let cash = PortfolioPosition::for_test(None, "Cash", 25.0, 0.0, PriceStatus::Unavailable);
        assert!(cash.has_market_price());
        assert_eq!(cash.get_balance(), 25.0);
    }

    #[test]
    fn test_is_cash() {
        assert!(PortfolioPosition::for_test(None, "Cash", 1.0, 0.0, PriceStatus::Live).is_cash());
        assert!(PortfolioPosition::for_test(None, "CASH", 1.0, 0.0, PriceStatus::Live).is_cash());
        assert!(
            !PortfolioPosition::for_test(None, "Stocks", 1.0, 0.0, PriceStatus::Live).is_cash()
        );
        assert!(
            !PortfolioPosition::for_test(Some("CASH"), "Cash", 1.0, 1.0, PriceStatus::Live)
                .is_cash()
        );
    }

    #[test]
    fn test_price_status_is_not_persisted() {
        let position =
            PortfolioPosition::for_test(Some("SPY"), "Stocks", 1.0, 10.0, PriceStatus::Unavailable);
        let json = serde_json::to_string(&position).unwrap();
        assert!(!json.contains("PriceStatus"));
        assert!(!json.contains("Unavailable"));
        let parsed: PortfolioPosition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.price_status(), PriceStatus::Live);
        assert_eq!(parsed.get_ticker(), Some("SPY"));
        assert_eq!(parsed.market_price(), 0.0);
    }

    #[test]
    fn test_fx_symbol_cases() {
        assert_eq!(fx_symbol("EUR", "EUR"), None);
        assert_eq!(fx_symbol("eur", "EUR"), None);
        assert_eq!(fx_symbol("EUR", ""), None);
        assert_eq!(fx_symbol("EUR", "USD"), Some(("EURUSD=X".to_string(), 1.0)));
        assert_eq!(
            fx_symbol("EUR", "GBp"),
            Some(("EURGBP=X".to_string(), 0.01))
        );
        assert_eq!(
            fx_symbol("EUR", "GBX"),
            Some(("EURGBP=X".to_string(), 0.01))
        );
        assert_eq!(
            fx_symbol("USD", "ZAc"),
            Some(("USDZAR=X".to_string(), 0.01))
        );
        assert_eq!(
            fx_symbol("USD", "ILA"),
            Some(("USDILS=X".to_string(), 0.01))
        );
        // Same currency in minor units: no FX request, only the scale.
        assert_eq!(fx_symbol("GBP", "GBp"), Some((String::new(), 0.01)));
    }

    #[test]
    fn test_fx_rate_converts_balance_and_invested_not_price() {
        let mut position =
            PortfolioPosition::for_test(Some("FAKEUS"), "Stocks", 3.0, 100.0, PriceStatus::Live);
        position.price_currency = Some("USD".to_string());
        position.fx_rate = Some(dec(0.5));
        position.purchases = vec![Purchase {
            date: None,
            quantity: dec(3.0),
            price: Some(dec(80.0)),
            fees: None,
            estimated_price: None,
        }];
        assert_eq!(position.market_price(), 100.0);
        assert_eq!(position.get_balance(), 150.0);
        assert_eq!(position.average_cost(), Some(80.0));
        assert_eq!(position.total_invested(), Some(120.0));
        assert_eq!(position.pnl(), Some(30.0));
    }

    #[test]
    fn test_fx_unavailable_has_no_market_value() {
        let mut position = PortfolioPosition::for_test(
            Some("FAKEUS"),
            "Stocks",
            3.0,
            100.0,
            PriceStatus::FxUnavailable,
        );
        position.price_currency = Some("USD".to_string());
        assert!(!position.has_market_price());
        assert_eq!(position.get_balance(), 0.0);
        assert_eq!(position.pnl(), None);
    }

    #[test]
    fn test_decimal_money_math_is_exact() {
        let mut position =
            PortfolioPosition::for_test(Some("FAKEUS"), "Stocks", 0.0, 10.1, PriceStatus::Live);
        position.fx_rate = Some(dec(0.5));
        position.previous_close = Some(dec(10.0));
        position.purchases = (0..3)
            .map(|_| Purchase {
                date: None,
                quantity: dec(0.1),
                price: Some(dec(10.0)),
                fees: Some(dec(0.1)),
                estimated_price: None,
            })
            .collect();
        // In f64, 0.1 + 0.1 + 0.1 == 0.30000000000000004.
        assert_eq!(position.get_amount(), 0.3);
        assert_eq!(position.get_balance(), 1.515);
        assert_eq!(position.total_invested(), Some(1.65));
        assert_eq!(position.previous_day_value_dec(), dec(1.5));
        assert_eq!(position.day_pnl_dec(), Some(dec(0.015)));
    }

    #[test]
    fn test_decimal_fields_keep_json_numbers() {
        let raw = r#"[{"Amount": 0.45, "AssetClass": "Crypto", "Name": "B",
            "Purchases": [{"Date": "2025/05/28", "Quantity": 0.45, "Price": 100.1, "Fees": 0.3},
                          {"Quantity": 2}]}]"#;
        let positions: Vec<PortfolioPosition> = serde_json::from_str(raw).unwrap();
        assert_eq!(positions[0].amount, dec(0.45));
        assert_eq!(positions[0].purchases[1].price, None);
        let out = serde_json::to_value(&positions).unwrap();
        let p = &out[0];
        assert_eq!(p["Amount"], serde_json::json!(0.45));
        assert_eq!(p["Purchases"][0]["Quantity"], serde_json::json!(0.45));
        assert_eq!(p["Purchases"][0]["Price"], serde_json::json!(100.1));
        assert_eq!(p["Purchases"][0]["Fees"], serde_json::json!(0.3));
        assert_eq!(p["Purchases"][1]["Quantity"], serde_json::json!(2.0));
        assert!(p["Purchases"][1]["Price"].is_null());
    }
}

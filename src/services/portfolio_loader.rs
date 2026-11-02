use crate::money::{dec, div};
use eyre::{bail, Result, WrapErr};
use std::fs::read_to_string;

use crate::portfolio::Portfolio;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::configured_base_currency;
use crate::position::{
    apply_quote_result, from_string, fx_symbol, handle_position_with, ExcludedBasis,
    PortfolioPosition, PriceStatus,
};
use crate::services::quotes::{AnyProvider, QuoteProvider};
use crate::tui::NetworkStatus;

/// Read a plaintext file or decrypt a `.gpg` file.
///
/// Decrypted content is only ever held in memory; it is never written to disk.
pub fn open_encrypted_file(filename: &str) -> Result<String> {
    open_with_gpg(filename, std::ffi::OsStr::new("gpg"))
}

/// [`open_encrypted_file`] decrypting with the `gpg` program at `gpg`.
fn open_with_gpg(filename: &str, gpg: &std::ffi::OsStr) -> Result<String> {
    if filename.ends_with(".gpg") {
        let output = std::process::Command::new(gpg)
            .arg("-d")
            .arg(filename)
            .output()
            .wrap_err_with(|| format!("failed to execute gpg to decrypt {filename}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "gpg failed to decrypt {} ({}): {}",
                filename,
                output.status,
                stderr.trim()
            );
        }

        String::from_utf8(output.stdout).wrap_err("gpg output is not valid UTF-8")
    } else {
        read_to_string(filename).wrap_err_with(|| format!("failed to read file: {filename}"))
    }
}

/// Load portfolio JSON from a file path, supporting `.gpg` encryption.
pub fn load_portfolio_file(filename: &str) -> Result<String> {
    if filename.is_empty() {
        bail!("No portfolio file specified.");
    }
    open_encrypted_file(filename)
}

/// A positions file that does not parse as a list of positions.
///
/// Displays as `Error parsing positions in <file>: <reason> at line L column C`.
/// Line and column refer to the text as written. `reason` never contains the
/// offending value (it may be an amount): serde's ``invalid type: floating
/// point `1234.5`, expected a string`` becomes `invalid type, expected a string`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionsParseError {
    pub file: String,
    pub reason: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for PositionsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error parsing positions in {}: {} at line {} column {}",
            self.file, self.reason, self.line, self.column
        )
    }
}

impl std::error::Error for PositionsParseError {}

/// Check that `text` parses as a list of positions; `file` names it in the error.
pub fn check_positions(file: &str, text: &str) -> Result<(), PositionsParseError> {
    let Err(e) = serde_json::from_str::<Vec<PortfolioPosition>>(text) else {
        return Ok(());
    };
    // serde's Display is "<message> at line L column C"; keep only the message.
    let full = e.to_string();
    let message = full
        .strip_suffix(&format!(" at line {} column {}", e.line(), e.column()))
        .unwrap_or(&full);
    // These carry the value from the file; keep the kind and what was expected.
    let kind = ["invalid type", "invalid value", "unknown variant"]
        .into_iter()
        .find(|kind| message.starts_with(kind));
    let reason = match kind {
        Some(kind) => {
            let expected = message.rfind(", expected").map_or("", |at| &message[at..]);
            format!("{kind}{expected}")
        }
        None => message.to_string(),
    };
    Err(PositionsParseError {
        file: file.to_string(),
        reason,
        line: e.line(),
        column: e.column(),
    })
}

/// Returns a portfolio with the latest quotes from JSON data.
///
/// Values are converted to the configured base currency (read-only; see
/// [`configured_base_currency`]).
pub async fn create_live_portfolio(positions_str: String) -> (Portfolio, NetworkStatus) {
    create_live_portfolio_with_logging(positions_str, false).await
}

/// Returns a portfolio with the latest quotes, with optional per-position error logging.
///
/// Values are converted to the configured base currency (read-only; see
/// [`configured_base_currency`]).
pub async fn create_live_portfolio_with_logging(
    positions_str: String,
    log_errors: bool,
) -> (Portfolio, NetworkStatus) {
    let base_currency = configured_base_currency();
    create_live_portfolio_in(positions_str, &base_currency, log_errors).await
}

/// Returns a portfolio with the latest quotes, with values converted to
/// `base_currency`.
///
/// Positions quoted in another currency are converted with one FX quote per
/// currency (e.g. `EURUSD=X`; only the ticker symbol is sent) from the
/// provider chosen by `PORTFOLIO_RS_QUOTES` (Yahoo by default). If that FX
/// quote fails, those positions become [`PriceStatus::FxUnavailable`]: kept,
/// flagged, and excluded from totals. Currencies are never mixed silently.
pub async fn create_live_portfolio_in(
    positions_str: String,
    base_currency: &str,
    log_errors: bool,
) -> (Portfolio, NetworkStatus) {
    create_live_portfolio_with(
        Arc::new(AnyProvider::configured().clone()),
        positions_str,
        base_currency,
        log_errors,
    )
    .await
}

/// [`create_live_portfolio_in`] with an explicit [`QuoteProvider`] for quotes and FX.
pub async fn create_live_portfolio_with<P: QuoteProvider>(
    provider: Arc<P>,
    positions_str: String,
    base_currency: &str,
    log_errors: bool,
) -> (Portfolio, NetworkStatus) {
    let positions = match from_string(&positions_str) {
        Ok(p) => p,
        Err(e) => {
            if log_errors {
                eprintln!("Error parsing positions: {e:#}");
            }
            return (Portfolio::new(), NetworkStatus::Disconnected);
        }
    };
    let mut portfolio = Portfolio::new();
    portfolio.quotes_fixture = provider.is_fixture();

    // Keep a clone so a panicked task (JoinError) can still be added as Unavailable.
    let tasks: Vec<_> = positions
        .into_iter()
        .map(|mut position| {
            let fallback = position.clone();
            let provider = Arc::clone(&provider);
            let task =
                tokio::spawn(async move { handle_position_with(&*provider, &mut position).await });
            (fallback, task)
        })
        .collect();

    for (mut fallback, task) in tasks {
        if let Ok(position) = task.await {
            portfolio.add_position(position)
        } else {
            apply_quote_result(&mut fallback, None);
            portfolio.add_position(fallback);
        }
    }

    // Ticker-less holdings are not quote-dependent and do not affect status.
    let mut ticker_positions = 0usize;
    let mut live_ticker_positions = 0usize;
    for position in &portfolio.positions {
        if position.get_ticker().is_none() {
            continue;
        }
        ticker_positions += 1;
        if position.price_status() == PriceStatus::Live
            && position.price_source() != Some(crate::services::quotes::PriceSource::Cache)
        {
            live_ticker_positions += 1;
        }
    }
    let network_status = if ticker_positions == 0 || live_ticker_positions == ticker_positions {
        NetworkStatus::Connected
    } else if live_ticker_positions == 0 {
        NetworkStatus::Disconnected
    } else {
        NetworkStatus::Partial
    };

    // FX after network status: a missing FX rate is reported separately.
    let symbols: HashSet<String> = portfolio
        .positions
        .iter()
        .filter(|p| p.price_status() == PriceStatus::Live)
        .filter_map(|p| p.price_currency())
        .filter_map(|ccy| fx_symbol(base_currency, ccy))
        .map(|(symbol, _)| symbol)
        .filter(|symbol| !symbol.is_empty())
        .collect();
    let mut fx_prices = HashMap::new();
    let mut fx_cached: HashMap<String, Option<String>> = HashMap::new();
    for symbol in symbols {
        let quote = provider.latest(&symbol).await.ok();
        if quote.is_none() && log_errors {
            eprintln!("Warning: no FX rate {symbol}; affected positions excluded from totals");
        }
        if quote
            .as_ref()
            .is_some_and(|q| q.source == crate::services::quotes::PriceSource::Cache)
        {
            fx_cached.insert(
                symbol.clone(),
                quote
                    .as_ref()
                    .and_then(|q| q.as_of.clone())
                    .or_else(|| provider.last_known_as_of(&symbol)),
            );
        }
        fx_prices.insert(symbol, quote.map(|q| q.price));
    }
    for position in &mut portfolio.positions {
        apply_fx(position, base_currency, &fx_prices);
        let cached = position
            .price_currency()
            .and_then(|ccy| fx_symbol(base_currency, ccy))
            .and_then(|(symbol, _)| fx_cached.get(&symbol).cloned());
        position.fx_from_cache = position.fx_rate.is_some() && cached.is_some();
        position.runtime.fx_as_of = cached.flatten();
    }

    // Size what is excluded from totals: last cached price (any age), else
    // cost basis, converted with the current or last cached FX rate. FX
    // pairs only an excluded position needs are fetched here (Finance N4:
    // no historic FX for the cost basis, so the current rate it is).
    let needed: HashSet<String> = portfolio
        .positions
        .iter()
        .filter(|p| p.get_ticker().is_some() && !p.has_market_price())
        .filter_map(|p| {
            p.price_currency()
                .map(str::to_string)
                .or_else(|| {
                    p.get_ticker()
                        .and_then(|t| provider.last_known(t))
                        .and_then(|(_, c)| c)
                })
                .or_else(|| p.currency.clone())
        })
        .filter_map(|ccy| fx_symbol(base_currency, &ccy))
        .map(|(symbol, _)| symbol)
        .filter(|symbol| !symbol.is_empty() && !fx_prices.contains_key(symbol))
        .collect();
    for symbol in needed {
        let quote = provider.latest(&symbol).await.ok();
        fx_prices.insert(symbol, quote.map(|q| q.price));
    }
    let fx_for = |ccy: &str| -> Option<crate::money::Decimal> {
        let (symbol, scale) = fx_symbol(base_currency, ccy)?;
        if symbol.is_empty() {
            return Some(dec(scale));
        }
        let price = fx_prices
            .get(&symbol)
            .copied()
            .flatten()
            .or_else(|| provider.last_known(&symbol).map(|(p, _)| p))
            .filter(|p| *p > 0.0 && p.is_finite())?;
        div(dec(scale), dec(price))
    };
    for position in &mut portfolio.positions {
        let Some(ticker) = position.get_ticker().map(str::to_string) else {
            continue;
        };
        if position.has_market_price() {
            continue;
        }
        let qty = position.quantity_dec();
        let last = if position.has_native_price() {
            position
                .price_currency()
                .map(|c| (position.last_spot, c.to_string()))
        } else {
            provider
                .last_known(&ticker)
                .and_then(|(p, c)| Some((dec(p), c?)))
        };
        let from_last = last.and_then(|(price, ccy)| Some(price * qty * fx_for(&ccy)?));
        // A cached last price (never served as current: past max-age or the
        // fallback is disabled) is an assumption; remember its as-of.
        let assumed = (!position.has_native_price()).then(|| {
            provider
                .last_known_as_of(&ticker)
                .unwrap_or_else(|| "unknown".to_string())
        });
        position.runtime.estimate_assumed_as_of = None;
        position.runtime.estimate_current_fx = false;
        position.excluded_estimate = match from_last {
            Some(v) if v > crate::money::Decimal::ZERO => {
                position.runtime.estimate_assumed_as_of = assumed;
                Some((v, ExcludedBasis::LastPrice))
            }
            _ => {
                // Currency: quote, cache entry, recorded `Currency`, else
                // assume the base currency (flagged).
                let ccy = position
                    .price_currency()
                    .map(str::to_string)
                    .or_else(|| provider.last_known(&ticker).and_then(|(_, c)| c))
                    .or_else(|| position.currency.clone());
                let foreign = ccy
                    .as_deref()
                    .and_then(|c| fx_symbol(base_currency, c))
                    .is_some_and(|(symbol, _)| !symbol.is_empty());
                let (fx, basis) = match ccy {
                    Some(c) => (fx_for(&c), ExcludedBasis::CostBasis),
                    None => (
                        Some(crate::money::Decimal::ONE),
                        ExcludedBasis::CostBasisAssumedBase,
                    ),
                };
                let est = position
                    .average_cost_dec()
                    .zip(fx)
                    .map(|(cost, fx)| cost * qty * fx)
                    .filter(|v| *v > crate::money::Decimal::ZERO)
                    .map(|v| (v, basis));
                position.runtime.estimate_current_fx = foreign && est.is_some();
                est
            }
        };
    }
    crate::position::mark_stale(&mut portfolio, crate::position::DEFAULT_STALE_AFTER_DAYS);

    if log_errors {
        // Only ticker positions lack a market price.
        let unavailable = portfolio
            .positions_without_price()
            .into_iter()
            .filter(|p| p.price_status() == PriceStatus::Unavailable)
            .filter_map(|p| p.get_ticker());
        for ticker in unavailable {
            eprintln!("Warning: no market price for {ticker}; position excluded from totals");
        }
    }

    (portfolio, network_status)
}

/// Set `fx_rate` (or [`PriceStatus::FxUnavailable`]) from pre-fetched FX prices.
///
/// `fx_prices` maps Yahoo FX symbols to their latest price (QUOTE per 1 BASE),
/// `None` when the lookup failed. Pure, so it is testable offline.
pub(crate) fn apply_fx(
    position: &mut PortfolioPosition,
    base_currency: &str,
    fx_prices: &HashMap<String, Option<f64>>,
) {
    position.fx_rate = None;
    if position.price_status != PriceStatus::Live {
        return;
    }
    let Some(ccy) = position.price_currency.as_deref() else {
        return;
    };
    let Some((symbol, scale)) = fx_symbol(base_currency, ccy) else {
        return;
    };
    if symbol.is_empty() {
        position.fx_rate = Some(dec(scale));
        return;
    }
    match fx_prices.get(&symbol).copied().flatten() {
        Some(price) if price > 0.0 && price.is_finite() => {
            // Exact Decimal rate; None only on overflow.
            position.fx_rate = div(dec(scale), dec(price));
            if position.fx_rate.is_none() {
                position.price_status = PriceStatus::FxUnavailable;
            }
        }
        _ => position.price_status = PriceStatus::FxUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::quotes::{FixtureProvider, OfflineProvider};

    #[test]
    fn test_check_positions_names_field_and_position() {
        let text = "[\n {\n  \"Name\": \"Fake Watch\",\n  \"AssetClass\": \"Other\"\n }\n]";
        let err = check_positions("positions.json", text).unwrap_err();
        assert_eq!(err.reason, "missing field `Amount`");
        assert_eq!((err.line, err.column), (5, 2));
        assert_eq!(
            err.to_string(),
            "Error parsing positions in positions.json: missing field `Amount` at line 5 column 2"
        );
        assert!(check_positions("p.json", "[]").is_ok());
        assert!(check_positions("p.json", "").is_err());
        assert!(check_positions("p.json", r#"{"Name": "x"}"#).is_err());
    }

    #[test]
    fn test_check_positions_never_echoes_the_value() {
        for text in [
            r#"[{"Name": "x", "Ticker": 4321.5, "AssetClass": "Stocks", "Amount": 1}]"#,
            r#"[{"Name": 4321.5, "AssetClass": "Stocks", "Amount": 1}]"#,
        ] {
            let err = check_positions("p.json", text).unwrap_err();
            assert!(!err.to_string().contains("4321"), "{err}");
            assert!(err.reason.starts_with("invalid type, expected"), "{err}");
        }
    }

    fn usd_position() -> PortfolioPosition {
        let mut position =
            PortfolioPosition::for_test(Some("FAKEUS"), "Stocks", 2.0, 100.0, PriceStatus::Live);
        position.price_currency = Some("USD".to_string());
        position
    }

    #[test]
    fn test_apply_fx_converts_foreign_position_and_total() {
        let mut fx = HashMap::new();
        fx.insert("EURUSD=X".to_string(), Some(2.0));
        let mut position = usd_position();
        apply_fx(&mut position, "EUR", &fx);
        assert_eq!(position.fx_rate(), Some(0.5));
        assert_eq!(position.price_status(), PriceStatus::Live);

        let mut portfolio = Portfolio::new();
        portfolio.add_position(position);
        portfolio.add_position(PortfolioPosition::for_test(
            None,
            "Cash",
            50.0,
            0.0,
            PriceStatus::Live,
        ));
        // 2 * 100 USD * 0.5 EUR/USD + 50 EUR cash
        assert_eq!(portfolio.get_total_value(), 150.0);
    }

    #[test]
    fn test_apply_fx_missing_rate_flags_and_excludes() {
        let mut fx = HashMap::new();
        fx.insert("EURUSD=X".to_string(), None);
        let mut failed = usd_position();
        apply_fx(&mut failed, "EUR", &fx);
        assert_eq!(failed.price_status(), PriceStatus::FxUnavailable);

        let mut not_fetched = usd_position();
        apply_fx(&mut not_fetched, "EUR", &HashMap::new());
        assert_eq!(not_fetched.price_status(), PriceStatus::FxUnavailable);

        let mut portfolio = Portfolio::new();
        portfolio.add_position(failed);
        portfolio.add_position(PortfolioPosition::for_test(
            None,
            "Cash",
            50.0,
            0.0,
            PriceStatus::Live,
        ));
        assert_eq!(portfolio.get_total_value(), 50.0);
        assert_eq!(portfolio.positions_without_price().len(), 1);
    }

    #[test]
    fn test_apply_fx_same_or_unknown_currency_is_unchanged() {
        let fx = HashMap::new();
        let mut same = usd_position();
        same.price_currency = Some("EUR".to_string());
        apply_fx(&mut same, "EUR", &fx);
        assert_eq!(same.fx_rate(), None);
        assert_eq!(same.get_balance(), 200.0);

        let mut unknown = usd_position();
        unknown.price_currency = None;
        apply_fx(&mut unknown, "EUR", &fx);
        assert_eq!(unknown.price_status(), PriceStatus::Live);
        assert_eq!(unknown.get_balance(), 200.0);

        let mut pence = usd_position();
        pence.price_currency = Some("GBp".to_string());
        apply_fx(&mut pence, "GBP", &fx);
        assert_eq!(pence.fx_rate(), Some(0.01));
        assert_eq!(pence.get_balance(), 2.0);
    }

    const FAKE_POSITIONS: &str = r#"[
        {"Name": "Fake US", "Ticker": "FAKEUS", "AssetClass": "Stocks", "Amount": 0,
         "Purchases": [{"Date": "2025-01-02", "Quantity": 10}]},
        {"Name": "Fake EU", "Ticker": "FAKEEU", "AssetClass": "Stocks", "Amount": 4},
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 1000}
    ]"#;

    fn fake_provider(with_fx: bool) -> FixtureProvider {
        let fx = if with_fx {
            r#", "USDEUR=X": {"price": 0.8, "currency": "EUR"}"#
        } else {
            ""
        };
        FixtureProvider::from_json(&format!(
            r#"{{"latest": {{"FAKEUS": {{"price": 12.0, "currency": "USD"}},
                "FAKEEU": {{"price": 50.0, "currency": "EUR"}}{fx}}},
               "previous_close": {{"FAKEUS": 11.0}},
               "close_on": {{"FAKEUS": 10.0}},
               "names": {{"FAKEUS": "ignored, name already set"}}}}"#
        ))
        .unwrap()
    }

    fn by_ticker<'a>(portfolio: &'a Portfolio, ticker: &str) -> &'a PortfolioPosition {
        portfolio
            .positions
            .iter()
            .find(|p| p.get_ticker() == Some(ticker))
            .unwrap()
    }

    #[tokio::test]
    async fn fixture_provider_prices_converts_fx_and_fills_purchases() {
        let (portfolio, status) = create_live_portfolio_with(
            Arc::new(fake_provider(true)),
            FAKE_POSITIONS.to_string(),
            "USD",
            false,
        )
        .await;
        assert_eq!(status, NetworkStatus::Connected);

        let us = by_ticker(&portfolio, "FAKEUS");
        assert_eq!(us.price_status(), PriceStatus::Live);
        assert!((us.get_balance() - 120.0).abs() < 1e-9);
        // Missing purchase price filled from the historic close.
        assert!((us.total_invested().unwrap() - 100.0).abs() < 1e-9);
        assert!((us.pnl().unwrap() - 20.0).abs() < 1e-9);

        // 4 * 50 EUR at 0.8 EUR per USD = 250 USD.
        let eu = by_ticker(&portfolio, "FAKEEU");
        assert!((eu.get_balance() - 250.0).abs() < 1e-9);

        assert!((portfolio.get_total_value() - 1370.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn fixture_provider_missing_fx_flags_position() {
        let (portfolio, _) = create_live_portfolio_with(
            Arc::new(fake_provider(false)),
            FAKE_POSITIONS.to_string(),
            "USD",
            false,
        )
        .await;
        let eu = by_ticker(&portfolio, "FAKEEU");
        assert_eq!(eu.price_status(), PriceStatus::FxUnavailable);
        assert!((portfolio.get_total_value() - 1120.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn offline_provider_keeps_positions_unavailable() {
        let (portfolio, status) = create_live_portfolio_with(
            Arc::new(OfflineProvider),
            FAKE_POSITIONS.to_string(),
            "USD",
            false,
        )
        .await;
        assert_eq!(status, NetworkStatus::Disconnected);
        assert_eq!(portfolio.positions.len(), 3);
        assert_eq!(
            by_ticker(&portfolio, "FAKEUS").price_status(),
            PriceStatus::Unavailable
        );
        assert!((portfolio.get_total_value() - 1000.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn fixture_provider_historic_securities_value() {
        let provider = fake_provider(true);
        let (portfolio, _) = create_live_portfolio_with(
            Arc::new(provider.clone()),
            FAKE_POSITIONS.to_string(),
            "USD",
            false,
        )
        .await;
        // FAKEEU has no historic close, so like-for-like keeps only FAKEUS.
        let (historic, current) = portfolio
            .historic_and_current_securities_value_with(&provider, chrono::Utc::now())
            .await
            .unwrap();
        assert!((historic - 100.0).abs() < 1e-9);
        assert!((current - 120.0).abs() < 1e-9);
    }

    /// FAKEUS in USD, FAKEEU in EUR, base EUR, FX via EURUSD=X.
    fn eur_base_map(with_fx: bool) -> serde_json::Map<String, serde_json::Value> {
        let fx = if with_fx {
            r#", "EURUSD=X": {"price": 1.25, "currency": "USD"}"#
        } else {
            ""
        };
        let value = serde_json::from_str::<serde_json::Value>(&format!(
            r#"{{"latest": {{"FAKEUS": {{"price": 100.0, "currency": "USD"}},
                "FAKEEU": {{"price": 40.0, "currency": "EUR"}}{fx}}},
               "previous_close": {{"FAKEUS": 90.0}},
               "close_on": {{"FAKEUS": 80.0}},
               "names": {{"FAKEEU": "Fake Europe"}}}}"#
        ))
        .unwrap();
        value.as_object().unwrap().clone()
    }

    const EUR_BASE_POSITIONS: &str = r#"[
        {"Ticker": "FAKEUS", "AssetClass": "Stocks", "Amount": 2,
         "Purchases": [{"Date": "2024-01-15", "Quantity": 2}]},
        {"Ticker": "FAKEEU", "AssetClass": "Stocks", "Amount": 5},
        {"Ticker": "MISSING", "AssetClass": "Stocks", "Amount": 1, "Name": "Missing Co"},
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 50}
    ]"#;

    #[tokio::test]
    async fn fixture_eur_base_converts_fx_keeps_missing_and_unnamed() {
        let provider = FixtureProvider::try_from(eur_base_map(true)).unwrap();
        let (portfolio, status) = create_live_portfolio_with(
            Arc::new(provider),
            EUR_BASE_POSITIONS.to_string(),
            "EUR",
            false,
        )
        .await;

        assert_eq!(portfolio.positions.len(), 4);
        assert_eq!(status, NetworkStatus::Partial);

        let us = by_ticker(&portfolio, "FAKEUS");
        assert_eq!(us.price_status(), PriceStatus::Live);
        assert_eq!(us.price_currency(), Some("USD"));
        assert!((us.fx_rate().unwrap() - 0.8).abs() < 1e-9);
        // 2 * 100 USD * 0.8 EUR/USD
        assert!((us.get_balance() - 160.0).abs() < 1e-9);
        // Purchase without a price is filled from close_on (80), not persisted.
        assert!(us.get_purchases()[0].price.is_none());
        assert!(
            (crate::money::to_f64(us.get_purchases()[0].effective_price().unwrap()) - 80.0).abs()
                < 1e-9
        );
        // Name lookup missed; the position is kept and the ticker is the fallback.
        assert_eq!(us.get_name_option(), None);
        assert_eq!(us.get_name(), "FAKEUS");

        let eu = by_ticker(&portfolio, "FAKEEU");
        assert_eq!(eu.price_status(), PriceStatus::Live);
        assert_eq!(eu.fx_rate(), None);
        assert!((eu.get_balance() - 200.0).abs() < 1e-9);
        assert_eq!(eu.get_name(), "Fake Europe");

        let missing = by_ticker(&portfolio, "MISSING");
        assert_eq!(missing.price_status(), PriceStatus::Unavailable);
        assert_eq!(missing.get_balance(), 0.0);
        assert_eq!(missing.get_name(), "Missing Co");

        // 160 + 200 + 50 cash; MISSING excluded.
        assert!((portfolio.get_total_value() - 410.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn fixture_missing_eurusd_fx_is_unavailable() {
        let provider = FixtureProvider::try_from(eur_base_map(false)).unwrap();
        let (portfolio, _) = create_live_portfolio_with(
            Arc::new(provider),
            EUR_BASE_POSITIONS.to_string(),
            "EUR",
            false,
        )
        .await;

        assert_eq!(portfolio.positions.len(), 4);
        assert_eq!(
            by_ticker(&portfolio, "FAKEUS").price_status(),
            PriceStatus::FxUnavailable
        );
        assert_eq!(
            by_ticker(&portfolio, "FAKEEU").price_status(),
            PriceStatus::Live
        );
        assert_eq!(
            by_ticker(&portfolio, "MISSING").price_status(),
            PriceStatus::Unavailable
        );
        // FAKEEU 200 + cash 50; FAKEUS excluded, not mixed in.
        assert!((portfolio.get_total_value() - 250.0).abs() < 1e-9);
    }

    #[test]
    fn test_load_portfolio_file_reads_plaintext_and_rejects_empty() {
        assert!(load_portfolio_file("")
            .unwrap_err()
            .to_string()
            .contains("No portfolio file specified"));
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.json");
        std::fs::write(&path, "[]").unwrap();
        assert_eq!(load_portfolio_file(path.to_str().unwrap()).unwrap(), "[]");
        let missing = tmp.path().join("missing.json");
        let err = load_portfolio_file(missing.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("failed to read file"), "{err}");
    }

    /// A stand-in `gpg` that checks its arguments, then runs `body`.
    #[cfg(unix)]
    fn fake_gpg(dir: &std::path::Path, name: &str, file: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\n[ \"$1\" = -d ] && [ \"$2\" = '{file}' ] || exit 9\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn test_gpg_files_are_decrypted_in_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("p.json.gpg");
        std::fs::write(&file, b"ciphertext").unwrap();
        let file = file.to_str().unwrap();

        let ok = fake_gpg(tmp.path(), "ok", file, "printf '[]'");
        assert_eq!(open_with_gpg(file, ok.as_os_str()).unwrap(), "[]");
        // Only the ciphertext and the fake programs are on disk.
        assert_eq!(std::fs::read(file).unwrap(), b"ciphertext");

        let fail = fake_gpg(
            tmp.path(),
            "fail",
            file,
            "echo ' bad passphrase ' >&2; exit 2",
        );
        let err = open_with_gpg(file, fail.as_os_str())
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("gpg failed to decrypt {file} (")),
            "{err}"
        );
        assert!(err.ends_with("): bad passphrase"), "{err}");

        let binary = fake_gpg(tmp.path(), "binary", file, "printf '\\377'");
        let err = open_with_gpg(file, binary.as_os_str()).unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");

        let missing = tmp.path().join("no-such-gpg");
        let err = open_with_gpg(file, missing.as_os_str()).unwrap_err();
        assert!(err.to_string().contains("failed to execute gpg"), "{err}");
    }

    #[test]
    fn test_check_positions_hides_unparseable_decimals() {
        let err = check_positions(
            "p.json",
            r#"[{"Name": "x", "AssetClass": "Stocks", "Amount": 1, "Purchases": [{"Quantity": "lots"}]}]"#,
        )
        .unwrap_err();
        assert_eq!(
            err.reason,
            "invalid value, expected a Decimal type representing a fixed-point number"
        );
    }

    use crate::services::quotes::{PriceSource, Quote, QuoteError};

    /// Scripted quotes: fixture prices, prices served "from the cache"
    /// (without an as-of), last-known entries, and a symbol whose lookup
    /// panics. One provider type, so one instantiation of
    /// [`create_live_portfolio_with`] sees every branch.
    #[derive(Default)]
    struct Scripted {
        fixture: FixtureProvider,
        cached: HashMap<String, f64>,
        last_known: HashMap<String, (f64, Option<String>)>,
        as_of: HashMap<String, String>,
        panic_on: Option<&'static str>,
    }

    impl QuoteProvider for Scripted {
        async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
            assert_ne!(Some(symbol), self.panic_on, "provider bug");
            match self.cached.get(symbol) {
                Some(price) => Ok(Quote {
                    price: *price,
                    currency: None,
                    as_of: None,
                    source: PriceSource::Cache,
                }),
                None => self.fixture.latest(symbol).await,
            }
        }
        async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
            self.fixture.previous_close(symbol).await
        }
        async fn close_on(
            &self,
            symbol: &str,
            date: chrono::DateTime<chrono::Utc>,
        ) -> Result<f64, QuoteError> {
            self.fixture.close_on(symbol, date).await
        }
        async fn name(&self, symbol: &str) -> Option<String> {
            self.fixture.name(symbol).await
        }
        fn last_known(&self, symbol: &str) -> Option<(f64, Option<String>)> {
            self.last_known.get(symbol).cloned()
        }
        fn last_known_as_of(&self, symbol: &str) -> Option<String> {
            self.as_of.get(symbol).cloned()
        }
    }

    async fn run(provider: Scripted, positions: &str, base: &str) -> (Portfolio, NetworkStatus) {
        create_live_portfolio_with(Arc::new(provider), positions.to_string(), base, true).await
    }

    #[tokio::test]
    async fn test_unparseable_positions_give_an_empty_disconnected_portfolio() {
        let (portfolio, status) = run(Scripted::default(), "not json", "EUR").await;
        assert!(portfolio.positions.is_empty());
        assert_eq!(status, NetworkStatus::Disconnected);
        // Also without logging.
        let (portfolio, _) = create_live_portfolio_with(
            Arc::new(OfflineProvider),
            "not json".to_string(),
            "EUR",
            false,
        )
        .await;
        assert!(portfolio.positions.is_empty());
    }

    #[tokio::test]
    async fn test_panicked_lookup_keeps_the_position_unavailable() {
        let provider = Scripted {
            fixture: fake_provider(true),
            panic_on: Some("BOOM"),
            ..Default::default()
        };
        let positions = r#"[
            {"Name": "Boom", "Ticker": "BOOM", "AssetClass": "Stocks", "Amount": 3},
            {"Name": "Fake US", "Ticker": "FAKEUS", "AssetClass": "Stocks", "Amount": 1}
        ]"#;
        let (portfolio, status) = run(provider, positions, "USD").await;
        assert_eq!(portfolio.positions.len(), 2);
        assert_eq!(status, NetworkStatus::Partial);
        let boom = by_ticker(&portfolio, "BOOM");
        assert_eq!(boom.price_status(), PriceStatus::Unavailable);
        assert_eq!(boom.get_balance(), 0.0);
        assert_eq!(
            by_ticker(&portfolio, "FAKEUS").price_status(),
            PriceStatus::Live
        );
    }

    #[tokio::test]
    async fn test_network_status_counts_only_live_fetches() {
        let provider = || Scripted {
            fixture: fake_provider(true),
            cached: HashMap::from([("CACHED".to_string(), 4.0)]),
            ..Default::default()
        };
        let cash = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 5}]"#;
        assert_eq!(
            run(provider(), cash, "USD").await.1,
            NetworkStatus::Connected
        );
        let live = r#"[{"Ticker": "FAKEUS", "AssetClass": "Stocks", "Amount": 1}]"#;
        assert_eq!(
            run(provider(), live, "USD").await.1,
            NetworkStatus::Connected
        );
        // A price served from the cache is not a live fetch.
        let cached = r#"[{"Ticker": "CACHED", "AssetClass": "Stocks", "Amount": 1}]"#;
        let quiet = false;
        let (portfolio, status) =
            create_live_portfolio_with(Arc::new(provider()), cached.to_string(), "USD", quiet)
                .await;
        assert_eq!(status, NetworkStatus::Disconnected);
        assert_eq!(by_ticker(&portfolio, "CACHED").get_balance(), 4.0);
    }

    /// Base EUR. Every way an unpriced position can be sized.
    const EXCLUDED: &str = r#"[
        {"Name": "Cash", "AssetClass": "Cash", "Amount": 100},
        {"Ticker": "LIVEEUR", "AssetClass": "Stocks", "Amount": 1,
         "Purchases": [{"Date": "2025-01-02", "Quantity": 1}]},
        {"Name": "Live USD", "Ticker": "LIVEUSD", "AssetClass": "Stocks", "Amount": 1},
        {"Name": "No CHF rate", "Ticker": "FXLESS", "AssetClass": "Stocks", "Amount": 2},
        {"Name": "No JPY rate", "Ticker": "FXLESS2", "AssetClass": "Stocks", "Amount": 1},
        {"Name": "Gone GBP", "Ticker": "GONELAST", "AssetClass": "Stocks", "Amount": 2},
        {"Name": "Gone USD", "Ticker": "GONEASOF", "AssetClass": "Stocks", "Amount": 1},
        {"Name": "Gone SEK", "Ticker": "GONESEK", "AssetClass": "Stocks", "Amount": 1},
        {"Name": "Gone base", "Ticker": "GONENONE", "AssetClass": "Stocks", "Amount": 1,
         "Purchases": [{"Date": "2025-01-02", "Quantity": 1, "Price": 10}]},
        {"Name": "Gone no cost", "Ticker": "GONEZERO", "AssetClass": "Stocks", "Amount": 1,
         "Currency": "USD"}
    ]"#;

    fn excluded_provider() -> Scripted {
        let fixture = FixtureProvider::from_json(
            r#"{"latest": {"LIVEEUR": {"price": 10.0, "currency": "EUR"},
                           "LIVEUSD": {"price": 20.0, "currency": "USD"},
                           "FXLESS": {"price": 5.0, "currency": "CHF"},
                           "FXLESS2": {"price": 7.0, "currency": "JPY"},
                           "EURGBP=X": {"price": 0.8, "currency": "GBP"}},
                "previous_close": {"LIVEEUR": 9.0},
                "close_on": {"LIVEEUR": 8.0},
                "names": {"LIVEEUR": "Live Europe"}}"#,
        )
        .unwrap();
        let known = |p: f64, c: &str| (p, Some(c.to_string()));
        Scripted {
            fixture,
            cached: HashMap::from([("EURUSD=X".to_string(), 1.25)]),
            last_known: HashMap::from([
                ("EURCHF=X".to_string(), (0.5, None)),
                ("GONELAST".to_string(), known(3.0, "GBP")),
                ("GONEASOF".to_string(), known(2.0, "USD")),
                ("GONESEK".to_string(), known(4.0, "SEK")),
            ]),
            as_of: HashMap::from([
                ("EURUSD=X".to_string(), "2025-01-02T00:00:00Z".to_string()),
                ("GONEASOF".to_string(), "2025-01-01T00:00:00Z".to_string()),
            ]),
            panic_on: None,
        }
    }

    #[tokio::test]
    async fn test_excluded_positions_are_sized_from_last_price_or_cost() {
        let (portfolio, status) = run(excluded_provider(), EXCLUDED, "EUR").await;
        assert_eq!(status, NetworkStatus::Partial);
        let estimate = |t: &str| by_ticker(&portfolio, t).excluded_estimate();

        let eur = by_ticker(&portfolio, "LIVEEUR");
        assert_eq!(eur.get_name(), "Live Europe");
        assert_eq!(eur.excluded_estimate(), None);

        // FX served from the cache without an as-of: the last-known as-of.
        let usd = by_ticker(&portfolio, "LIVEUSD");
        assert_eq!(usd.fx_rate(), Some(0.8));
        assert_eq!(usd.get_balance(), 16.0);
        assert!(usd.fx_from_cache);
        assert_eq!(
            usd.runtime.fx_as_of.as_deref(),
            Some("2025-01-02T00:00:00Z")
        );

        // Native price, no current FX: the last known rate (2 EUR per CHF).
        let chf = by_ticker(&portfolio, "FXLESS");
        assert_eq!(chf.price_status(), PriceStatus::FxUnavailable);
        assert_eq!(
            chf.excluded_estimate(),
            Some((dec(20.0), ExcludedBasis::LastPrice))
        );
        assert_eq!(chf.runtime.estimate_assumed_as_of, None);

        // Native price, no rate at all, no cost: nothing to estimate.
        let jpy = by_ticker(&portfolio, "FXLESS2");
        assert_eq!(jpy.excluded_estimate(), None);
        assert!(!jpy.runtime.estimate_current_fx);

        // Cached last price; its FX pair is fetched only for this estimate.
        let gbp = by_ticker(&portfolio, "GONELAST");
        assert_eq!(gbp.price_status(), PriceStatus::Unavailable);
        assert_eq!(
            gbp.excluded_estimate(),
            Some((dec(7.5), ExcludedBasis::LastPrice))
        );
        assert_eq!(
            gbp.runtime.estimate_assumed_as_of.as_deref(),
            Some("unknown")
        );

        let asof = by_ticker(&portfolio, "GONEASOF");
        assert_eq!(
            asof.excluded_estimate(),
            Some((dec(1.6), ExcludedBasis::LastPrice))
        );
        assert_eq!(
            asof.runtime.estimate_assumed_as_of.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );

        // Last price in a currency without any rate, and no cost basis.
        assert_eq!(estimate("GONESEK"), None);
        assert!(!by_ticker(&portfolio, "GONESEK").runtime.estimate_current_fx);

        assert_eq!(
            estimate("GONENONE"),
            Some((dec(10.0), ExcludedBasis::CostBasisAssumedBase))
        );
        assert_eq!(estimate("GONEZERO"), None);

        // 100 cash + 10 LIVEEUR + 16 LIVEUSD; the rest is excluded.
        assert_eq!(portfolio.get_total_value(), 126.0);
    }

    #[tokio::test]
    async fn test_excluded_cost_basis_in_minor_units_and_foreign_currency() {
        // Unpriced and only a recorded currency: pence need no FX request,
        // USD is converted at the current rate (flagged).
        let positions = r#"[
            {"Name": "Pence Co", "Ticker": "PENCE", "AssetClass": "Stocks", "Amount": 2,
             "Currency": "GBp",
             "Purchases": [{"Date": "2025-01-02", "Quantity": 2, "Price": 150}]},
            {"Name": "Dollar Co", "Ticker": "DOLLAR", "AssetClass": "Stocks", "Amount": 1,
             "Currency": "USD",
             "Purchases": [{"Date": "2025-01-02", "Quantity": 1, "Price": 10}]}
        ]"#;
        let provider = Scripted {
            fixture: FixtureProvider::from_json(
                r#"{"latest": {"GBPUSD=X": {"price": 1.25, "currency": "USD"}}}"#,
            )
            .unwrap(),
            ..Default::default()
        };
        let (portfolio, status) = run(provider, positions, "GBP").await;
        assert_eq!(status, NetworkStatus::Disconnected);
        let pence = by_ticker(&portfolio, "PENCE");
        // 2 * 150 GBp = 3 GBP
        assert_eq!(
            pence.excluded_estimate(),
            Some((dec(3.0), ExcludedBasis::CostBasis))
        );
        assert!(!pence.runtime.estimate_current_fx);
        let dollar = by_ticker(&portfolio, "DOLLAR");
        assert_eq!(
            dollar.excluded_estimate(),
            Some((dec(8.0), ExcludedBasis::CostBasis))
        );
        assert!(dollar.runtime.estimate_current_fx);
    }

    #[test]
    fn test_apply_fx_unrepresentable_rate_is_fx_unavailable() {
        let mut fx = HashMap::new();
        // Positive and finite, but too small for a Decimal: no exact rate.
        fx.insert("EURUSD=X".to_string(), Some(1e-300));
        let mut position = usd_position();
        apply_fx(&mut position, "EUR", &fx);
        assert_eq!(position.fx_rate(), None);
        assert_eq!(position.price_status(), PriceStatus::FxUnavailable);
    }
}

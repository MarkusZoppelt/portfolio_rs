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
    if filename.ends_with(".gpg") {
        let output = std::process::Command::new("gpg")
            .arg("-d")
            .arg(filename)
            .output()
            .wrap_err_with(|| format!("failed to execute gpg to decrypt {}", filename))?;

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
        read_to_string(filename).wrap_err_with(|| format!("failed to read file: {}", filename))
    }
}

/// Load portfolio JSON from a file path, supporting `.gpg` encryption.
pub fn load_portfolio_file(filename: &str) -> Result<String> {
    if filename.is_empty() {
        bail!("No portfolio file specified.");
    }
    open_encrypted_file(filename)
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
                eprintln!("Error parsing positions: {:#}", e);
            }
            return (Portfolio::new(), NetworkStatus::Disconnected);
        }
    };
    let mut portfolio = Portfolio::new();

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
        match task.await {
            Ok(position) => portfolio.add_position(position),
            Err(_) => {
                apply_quote_result(&mut fallback, None);
                portfolio.add_position(fallback);
            }
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
    // cost basis, converted with the current or last cached FX rate.
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
                let (fx, basis) = match ccy {
                    Some(c) => (fx_for(&c), ExcludedBasis::CostBasis),
                    None => (
                        Some(crate::money::Decimal::ONE),
                        ExcludedBasis::CostBasisAssumedBase,
                    ),
                };
                position
                    .average_cost_dec()
                    .zip(fx)
                    .map(|(cost, fx)| cost * qty * fx)
                    .filter(|v| *v > crate::money::Decimal::ZERO)
                    .map(|v| (v, basis))
            }
        };
    }
    crate::position::mark_stale(&mut portfolio, crate::position::DEFAULT_STALE_AFTER_DAYS);

    if log_errors {
        for position in portfolio.positions_without_price() {
            if let Some(ticker) = position.get_ticker() {
                if position.price_status() == PriceStatus::Unavailable {
                    eprintln!(
                        "Warning: no market price for {ticker}; position excluded from totals"
                    );
                }
            }
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
    use std::fs::read_to_string;

    use super::*;
    use crate::services::quotes::{FixtureProvider, OfflineProvider};

    #[tokio::test]
    #[ignore = "requires network access to Yahoo Finance"]
    async fn test_create_live_portfolio() {
        let positions_str = read_to_string("example_data.json").unwrap();
        let (portfolio, _network_status) = create_live_portfolio(positions_str).await;
        assert!(!portfolio.positions.is_empty());
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
}

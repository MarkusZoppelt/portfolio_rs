//! Pure portfolio analytics behind the MCP tools.
//!
//! Everything here works on an already priced [`Portfolio`]; nothing does I/O.

use super::dto::*;
use crate::money::{dec, percent_of, to_f64, Decimal};
use crate::policy::Policy;
use crate::portfolio::Portfolio;
use crate::position::{parse_purchase_date, ExcludedBasis, PortfolioPosition, PriceStatus};
use serde_json::json;
use std::collections::HashMap;

const DEFAULT_TOLERANCE: f64 = 5.0;

fn pid(p: &PortfolioPosition) -> String {
    p.id().unwrap_or_default().to_string()
}

/// cash | market | manual
pub fn kind(p: &PortfolioPosition) -> &'static str {
    if p.is_cash() {
        "cash"
    } else if p.get_ticker().is_some() {
        "market"
    } else {
        "manual"
    }
}

fn price_status(p: &PortfolioPosition) -> &'static str {
    match kind(p) {
        "cash" => "cash",
        "manual" => "manual",
        _ => match p.price_status() {
            PriceStatus::Live if p.has_market_price() && p.is_stale() => "stale",
            PriceStatus::Live
                if p.has_market_price()
                    && p.price_source() == Some(crate::services::quotes::PriceSource::Cache) =>
            {
                "cache"
            }
            PriceStatus::Live if p.has_market_price() => "live",
            PriceStatus::FxUnavailable if p.has_native_price() => "fx_unavailable",
            _ => "unavailable",
        },
    }
}

/// live | cache | fixture | none (cash, manual and unpriced positions: none);
/// set whenever a native price is shown, also when its FX rate is missing.
pub fn price_source(p: &PortfolioPosition) -> &'static str {
    match p.price_source() {
        // Wherever a native price is shown, also without an FX rate.
        Some(src) if kind(p) == "market" && p.has_native_price() => src.as_str(),
        _ => "none",
    }
}

/// Oldest `priceAsOf` among the priced market positions, and the ids of
/// those whose price or FX rate came from the cache.
pub fn price_freshness(portfolio: &Portfolio) -> (Option<String>, Vec<String>) {
    let priced: Vec<_> = portfolio
        .positions
        .iter()
        .filter(|p| kind(p) == "market" && value_base(p).is_some())
        .collect();
    let oldest = priced
        .iter()
        .filter_map(|p| {
            let t = p.price_as_of()?;
            chrono::DateTime::parse_from_rfc3339(t)
                .ok()
                .map(|d| (d.with_timezone(&chrono::Utc), t.to_string()))
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, t)| t);
    let stale = priced
        .iter()
        .filter(|p| p.is_stale())
        .map(|p| pid(p))
        .collect();
    (oldest, stale)
}

/// complete | partial | stale (see `Meta::data_status`).
/// `get_performance` sets `insufficient_history` itself; this helper does not.
pub fn data_status(complete: bool, has_stale: bool) -> &'static str {
    if !complete {
        "partial"
    } else if has_stale {
        "stale"
    } else {
        "complete"
    }
}

/// Base-currency value, `None` when unknown (no live price or FX rate).
pub fn value_base(p: &PortfolioPosition) -> Option<Decimal> {
    if p.get_ticker().is_some() && !p.has_market_price() {
        None
    } else {
        Some(p.balance_dec())
    }
}

fn total_known(positions: &[&PortfolioPosition]) -> Decimal {
    positions.iter().filter_map(|p| value_base(p)).sum()
}

/// Portfolio-level cost basis and PnL over the priced market positions (cash
/// and manual holdings have none). Positions without a base value or without
/// the input are left out and `partial` is set, so a partial sum is always
/// flagged. A total is null only when no position contributes.
pub struct PortfolioPnl {
    pub partial: bool,
    pub invested: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub unrealized_pnl_percent: Option<Decimal>,
    pub day_pnl: Option<Decimal>,
}

pub fn portfolio_pnl(portfolio: &Portfolio) -> PortfolioPnl {
    let all_market: Vec<&PortfolioPosition> = portfolio
        .positions
        .iter()
        .filter(|p| kind(p) == "market")
        .collect();
    let market: Vec<&PortfolioPosition> = all_market
        .iter()
        .copied()
        .filter(|p| value_base(p).is_some())
        .collect();
    let partial = std::cell::Cell::new(market.len() < all_market.len());
    let sum = |f: &dyn Fn(&PortfolioPosition) -> Option<Decimal>| -> Option<Decimal> {
        let parts: Vec<Option<Decimal>> = market.iter().map(|p| f(p)).collect();
        if parts.iter().any(Option::is_none) {
            partial.set(true);
        }
        let known: Vec<Decimal> = parts.into_iter().flatten().collect();
        (!known.is_empty() || all_market.is_empty()).then(|| known.into_iter().sum())
    };
    let invested = sum(&|p| p.total_invested_dec());
    let unrealized_pnl = sum(&|p| p.pnl_dec());
    let day_pnl = sum(&|p| {
        let d = dec(p.daily_variation_percent()?);
        let value = value_base(p)?;
        let denom = Decimal::ONE_HUNDRED + d;
        (!denom.is_zero()).then(|| value * d / denom)
    });
    let unrealized_pnl_percent = match (unrealized_pnl, invested) {
        (Some(pnl), Some(inv)) => percent_of(pnl, inv),
        _ => None,
    };
    PortfolioPnl {
        partial: partial.get(),
        invested,
        unrealized_pnl,
        unrealized_pnl_percent,
        day_pnl,
    }
}

pub fn position_rows(
    portfolio: &Portfolio,
    base: &str,
    include_manual: bool,
    asset_class: Option<&str>,
    sort: SortBy,
) -> (Vec<PositionRow>, f64, f64) {
    let all: Vec<&PortfolioPosition> = portfolio.positions.iter().collect();
    // Weights use the unrounded sum. Reported totals are the sum of the
    // rounded per-position values, so they match `valueBase` to the cent.
    let total = total_known(&all);
    // Weights against a partial total mislead (every share is overstated),
    // so they are null whenever any position lacks a base value.
    let complete = all.iter().all(|p| value_base(p).is_some());
    let reported_total = sum_money(all.iter().filter_map(|p| value_base(p).map(money)));
    let reported_cash = sum_money(
        all.iter()
            .filter(|p| p.is_cash())
            .filter_map(|p| value_base(p).map(money)),
    );
    let mut rows: Vec<PositionRow> = all
        .iter()
        .filter(|p| include_manual || kind(p) != "manual")
        .filter(|p| asset_class.is_none_or(|c| p.get_asset_class().eq_ignore_ascii_case(c)))
        .map(|p| {
            let value = value_base(p);
            let market = kind(p) == "market";
            let invested = p.total_invested_dec();
            let pnl = if market { p.pnl_dec() } else { None };
            PositionRow {
                id: pid(p),
                name: p.get_name().to_string(),
                ticker: p.get_ticker().map(String::from),
                isin: None,
                asset_class: p.get_asset_class().to_string(),
                kind: kind(p).to_string(),
                quantity: qty(p.quantity_dec()),
                // The native quote stays even without FX; only base values
                // are null then.
                price: (market && p.has_native_price())
                    .then(|| to_f64(dec(p.market_price()).round_dp(6))),
                price_currency: if market {
                    p.price_currency().map(String::from)
                } else {
                    None
                },
                price_as_of: (market && p.has_native_price())
                    .then(|| p.price_as_of().map(String::from))
                    .flatten(),
                currency: if market {
                    p.price_currency().map(String::from)
                } else {
                    Some(base.to_string())
                },
                is_stale: market && p.has_native_price() && p.is_stale(),
                price_source: price_source(p).to_string(),
                fx_rate: if market { p.fx_rate() } else { None },
                price_status: price_status(p).to_string(),
                value_base: value.map(money),
                weight_percent: value
                    .filter(|_| complete)
                    .and_then(|v| percent_of(v, total))
                    .map(pct),
                average_cost: if market {
                    p.average_cost_dec().map(|d| to_f64(d.round_dp(6)))
                } else {
                    None
                },
                invested: if market { invested.map(money) } else { None },
                unrealized_pnl: pnl.map(money),
                unrealized_pnl_percent: if market && pnl.is_some() {
                    p.historic_variation_percent().map(|v| pct(dec(v)))
                } else {
                    None
                },
                day_change_percent: if market && p.has_native_price() {
                    p.native_daily_variation_percent().map(|v| pct(dec(v)))
                } else {
                    None
                },
                purchases_count: p.get_purchases().len(),
            }
        })
        .collect();
    let key = |r: &PositionRow| r.value_base.unwrap_or(f64::NEG_INFINITY);
    match sort {
        // Ties (and all unpriced rows) by name: the same order as get_context.
        SortBy::Value => {
            rows.sort_by(|a, b| key(b).total_cmp(&key(a)).then_with(|| a.name.cmp(&b.name)));
        }
        SortBy::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
        SortBy::Pnl => rows.sort_by(|a, b| {
            b.unrealized_pnl
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&a.unrealized_pnl.unwrap_or(f64::NEG_INFINITY))
        }),
        SortBy::DayChange => rows.sort_by(|a, b| {
            b.day_change_percent
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&a.day_change_percent.unwrap_or(f64::NEG_INFINITY))
        }),
    }
    (rows, reported_total, reported_cash)
}

/// Data-quality findings that also go into every response's `warnings`.
pub fn data_quality(portfolio: &Portfolio) -> Vec<Finding> {
    let mut out = Vec::new();
    let (oldest, stale) = price_freshness(portfolio);
    if !stale.is_empty() {
        out.push(
            Finding::new(
                "stale_prices",
                Severity::Warning,
                Category::DataQuality,
                format!(
                    "{} position(s) use last-known prices or FX rates from the quote cache (oldest as of {}); values are not current.",
                    stale.len(),
                    oldest.as_deref().unwrap_or("unknown")
                ),
            )
            .detail(json!({ "stalePositions": stale, "priceAsOf": oldest })),
        );
    }
    let mut tickers: HashMap<String, Vec<String>> = HashMap::new();
    for p in &portfolio.positions {
        let id = p.id();
        match kind(p) {
            "market" => {
                let ticker = p.get_ticker().unwrap_or_default();
                tickers
                    .entry(ticker.to_uppercase())
                    .or_default()
                    .push(pid(p));
                match p.price_status() {
                    PriceStatus::FxUnavailable => out.push(
                        Finding::new(
                            "fx_unavailable",
                            Severity::Warning,
                            Category::DataQuality,
                            format!(
                                "No FX rate for {ticker} ({}); value and invested are null and excluded from totals.",
                                p.price_currency().unwrap_or("?").to_uppercase()
                            ),
                        )
                        .position(id)
                        .class(p.get_asset_class())
                        .detail(json!({ "priceCurrency": p.price_currency() })),
                    ),
                    _ if !p.has_market_price() => out.push(
                        Finding::new(
                            "no_live_price",
                            Severity::Warning,
                            Category::DataQuality,
                            if p.has_complete_cost_basis() {
                                format!("No live price for {ticker}; excluded from totals.")
                            } else {
                                format!("No live price for {ticker}; excluded from totals. Purchase prices estimated from quote history are unknown too.")
                            },
                        )
                        .position(id)
                        .class(p.get_asset_class()),
                    ),
                    _ => {}
                }
                // Without any quote the gap comes from the missing quote
                // (reported above), not from the user's purchase records.
                if !p.has_complete_cost_basis() && p.price_currency().is_some() {
                    out.push(
                        Finding::new(
                            "missing_cost_basis",
                            Severity::Info,
                            Category::DataQuality,
                            format!("{ticker} has no complete cost basis; PnL is unknown."),
                        )
                        .position(id)
                        .class(p.get_asset_class()),
                    );
                }
            }
            "manual" => out.push(
                Finding::new(
                    "missing_ticker",
                    Severity::Info,
                    Category::DataQuality,
                    format!("{} has no ticker and is valued manually.", p.get_name()),
                )
                .position(id)
                .class(p.get_asset_class()),
            ),
            _ => {}
        }
    }
    if portfolio
        .positions
        .iter()
        .any(PortfolioPosition::cost_basis_at_current_fx)
    {
        let estimates: Vec<&str> = portfolio
            .positions
            .iter()
            .filter(|p| p.runtime.estimate_current_fx)
            .filter_map(|p| p.id())
            .collect();
        out.push(
            Finding::new(
                "cost_basis_current_fx",
                Severity::Info,
                Category::DataQuality,
                "Invested and PnL of foreign-currency positions (and the cost-basis estimate of excluded ones) convert purchase costs at the current FX rate, not the rate on the purchase date.".to_string(),
            )
            .detail(json!({ "estimatePositionIds": estimates })),
        );
    }
    let mut dups: Vec<_> = tickers
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .collect();
    dups.sort();
    for (ticker, ids) in dups {
        out.push(
            Finding::new(
                "duplicate_ticker",
                Severity::Warning,
                Category::DataQuality,
                format!("Ticker {ticker} appears in {} positions.", ids.len()),
            )
            .detail(json!({ "ticker": ticker, "positionIds": ids })),
        );
    }
    out
}

/// Missing prices / FX for the `dataQuality` block. `complete` is
/// `affected_positions.is_empty()`.
pub fn data_quality_summary(portfolio: &Portfolio, base: &str) -> DataQuality {
    let mut dq = DataQuality::default();
    for p in portfolio
        .positions
        .iter()
        .filter(|p| value_base(p).is_none())
    {
        let ticker = p.get_ticker().unwrap_or_default().to_string();
        if matches!(p.price_status(), PriceStatus::FxUnavailable) {
            let from = p.price_currency().unwrap_or("?").to_uppercase();
            let pair = format!("{from}/{}", base.to_uppercase());
            if !dq.missing_fx.contains(&pair) {
                dq.missing_fx.push(pair);
            }
        } else if !dq.missing_prices.contains(&ticker) {
            dq.missing_prices.push(ticker);
        }
        dq.affected_positions.push(pid(p));
        let class = p.get_asset_class().to_string();
        if !dq
            .affected_classes
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&class))
        {
            dq.affected_classes.push(class);
        }
    }
    dq
}

/// connected | partial | offline (fixture under a quote fixture), from the quotes actually present: offline
/// when no market position has a fresh (non-cached) quote, connected when
/// every market position has a fresh quote and FX rate, partial otherwise
/// (including quotes without FX).
pub fn network_status(portfolio: &Portfolio) -> &'static str {
    if portfolio.quotes_fixture {
        return "fixture";
    }
    let market: Vec<_> = portfolio
        .positions
        .iter()
        .filter(|p| kind(p) == "market")
        .collect();
    let fresh_quote = |p: &&&PortfolioPosition| {
        matches!(
            p.price_status(),
            PriceStatus::Live | PriceStatus::FxUnavailable
        ) && p.price_source() != Some(crate::services::quotes::PriceSource::Cache)
    };
    let quoted = market.iter().filter(fresh_quote).count();
    let complete = market
        .iter()
        .filter(fresh_quote)
        .filter(|p| value_base(p).is_some() && !p.is_stale())
        .count();
    if complete == market.len() {
        "connected"
    } else if quoted == 0 {
        "offline"
    } else {
        "partial"
    }
}

pub fn no_policy_finding() -> Finding {
    Finding::new(
        "no_policy",
        Severity::Warning,
        Category::Policy,
        "No investment policy is loaded; targets and constraints are unknown.".to_string(),
    )
}

/// An excluded position's upper bound for certain-breach checks. Estimates
/// (cost basis, an expired cache entry) are never bounds, because a position
/// can gain far more than any factor: the position is unbounded (`None`).
/// Only a last price that is not an assumption (a native quote fresh within
/// the cache max-age, excluded for want of an FX rate) bounds it at
/// `PLAUSIBLE_PRICE_FACTOR`× that price.
pub const PLAUSIBLE_PRICE_FACTOR: f64 = 2.0;

fn excluded_max(p: &PortfolioPosition) -> Option<Decimal> {
    match p.excluded_estimate() {
        Some((v, ExcludedBasis::LastPrice)) if p.estimate_assumed_as_of().is_none() => {
            Some(v * dec(PLAUSIBLE_PRICE_FACTOR))
        }
        _ => None,
    }
}

/// Sum of maximum plausible values; `None` (unbounded) if any is unbounded.
fn sum_max<'a>(it: impl Iterator<Item = &'a &'a PortfolioPosition>) -> Option<Decimal> {
    it.map(|p| excluded_max(p)).sum()
}

/// Share range of a part `priced` (plus own excluded positions up to
/// `own_max`) when other excluded positions are worth 0..`other_max`.
/// `None` bounds are unbounded; `own_any`/`other_any` say whether there are
/// such positions at all.
pub fn share_range(
    priced: Decimal,
    total: Decimal,
    own: (bool, Option<Decimal>),
    other: (bool, Option<Decimal>),
) -> (f64, f64) {
    let min = match other {
        (false, _) => percent_of(priced, total),
        (true, Some(m)) => percent_of(priced, total + m),
        (true, None) => Some(Decimal::ZERO),
    }
    .map_or(0.0, pct);
    let max = match own {
        (false, _) => percent_of(priced, total),
        (true, Some(m)) => percent_of(priced + m, total + m),
        (true, None) => Some(dec(100.0)),
    }
    .map_or(100.0, pct);
    (min, max)
}

pub struct Allocation {
    /// Sum of the priced positions only (unrounded; percentages use this).
    pub total: Decimal,
    /// `total` as the sum of per-position rounded values (what `totalValue` reports).
    pub reported_total: f64,
    /// Every included position has a value.
    pub complete: bool,
    /// Partial, but the excluded positions' estimated share is below the
    /// materiality threshold: judged on the priced part as if complete.
    pub immaterial: bool,
    /// Excluded share with cost-basis estimates scaled by their class's
    /// plausible-gain factor (what `immaterial` compares to the threshold);
    /// `None` when an estimate or a factor is missing.
    pub materiality_share: Option<f64>,
    /// Plausible-gain factor used per class with a cost-basis estimate.
    pub gain_factors: std::collections::BTreeMap<String, f64>,
    pub classes: Vec<ClassAllocation>,
    pub untargeted: Vec<String>,
    pub missing: Vec<String>,
    /// Ids of included positions without a base value (left out of `total`).
    pub excluded_positions: Vec<String>,
    /// Estimated share of the excluded positions (last cached price, else
    /// cost basis) in (priced total + estimate); null when one has neither.
    pub excluded_value_share: Option<f64>,
    pub excluded_value_basis: Option<&'static str>,
    /// Maximum plausible value of all excluded positions (`None`: unbounded;
    /// zero when nothing is excluded).
    pub excluded_max: Option<Decimal>,
}

pub fn allocation(
    portfolio: &Portfolio,
    policy: Option<&Policy>,
    include_cash: bool,
    include_manual: bool,
) -> Allocation {
    allocation_with(
        portfolio,
        policy,
        include_cash,
        include_manual,
        &PolicyOptions::default(),
    )
}

/// [`allocation`] with the materiality rule: the excluded positions are
/// immaterial (`immaterial: true`, judged on the priced part as if complete)
/// when every one has an estimate and the estimated share, with each
/// cost-basis estimate scaled by its class's plausible-gain factor, is below
/// `opts.possible_breach_min_share`. A cost-basis estimate in a class without
/// a factor (crypto, single stocks) is never immaterial.
pub fn allocation_with(
    portfolio: &Portfolio,
    policy: Option<&Policy>,
    include_cash: bool,
    include_manual: bool,
    opts: &PolicyOptions,
) -> Allocation {
    let min_share = opts.possible_breach_min_share;
    let included: Vec<&PortfolioPosition> = portfolio
        .positions
        .iter()
        .filter(|p| include_cash || !p.is_cash())
        .filter(|p| include_manual || kind(p) != "manual")
        .collect();
    let total = total_known(&included);
    let complete = included.iter().all(|p| value_base(p).is_some());
    let excluded: Vec<&PortfolioPosition> = included
        .iter()
        .copied()
        .filter(|p| value_base(p).is_none())
        .collect();
    let excluded_max_all = sum_max(excluded.iter());
    let estimates: Option<Vec<(Decimal, ExcludedBasis)>> =
        excluded.iter().map(|p| p.excluded_estimate()).collect();
    let (excluded_value_share, excluded_value_basis) = match estimates {
        _ if excluded.is_empty() => (Some(0.0), None),
        None => (None, None),
        Some(est) => {
            let e: Decimal = est.iter().map(|(v, _)| *v).sum();
            let basis = if est
                .iter()
                .any(|(_, b)| *b == ExcludedBasis::CostBasisAssumedBase)
            {
                ExcludedBasis::CostBasisAssumedBase
            } else if est.iter().any(|(_, b)| *b == ExcludedBasis::CostBasis) {
                ExcludedBasis::CostBasis
            } else {
                ExcludedBasis::LastPrice
            };
            (
                Some(pct(percent_of(e, total + e).unwrap_or(Decimal::ZERO))),
                Some(basis.as_str()),
            )
        }
    };
    // Plausible-gain factors: 1 for a last price (rule b), the class factor
    // for a cost basis; `None` when one has no estimate or no factor.
    let mut gain_factors: std::collections::BTreeMap<String, f64> = Default::default();
    let weighted: Option<Decimal> = excluded
        .iter()
        .map(|p| {
            let (v, basis) = p.excluded_estimate()?;
            let f = if basis == ExcludedBasis::LastPrice {
                1.0
            } else {
                let f = opts.gain_factor(p.get_asset_class())?;
                gain_factors.insert(p.get_asset_class().to_string(), f);
                f
            };
            Some(v * dec(f))
        })
        .sum();
    let estimated: Decimal = excluded
        .iter()
        .filter_map(|p| p.excluded_estimate().map(|(v, _)| v))
        .sum();
    let materiality_share =
        weighted.map(|w| pct(percent_of(w, total + estimated).unwrap_or(Decimal::ZERO)));
    let immaterial = !complete && materiality_share.is_some_and(|v| v < min_share);
    // Group case-insensitively, keeping the first spelling seen. A class with
    // any unpriced position has an unknown value (never a partial sum or 0).
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, (String, Option<Decimal>)> = HashMap::new();
    let mut priced: HashMap<String, Decimal> = HashMap::new();
    // Running sum of money(value) per class; reported class value is round2 of this.
    let mut rounded_sums: HashMap<String, f64> = HashMap::new();
    for p in &included {
        let key = p.get_asset_class().to_lowercase();
        if !values.contains_key(&key) {
            order.push(key.clone());
        }
        let entry = values
            .entry(key.clone())
            .or_insert_with(|| (p.get_asset_class().to_string(), Some(Decimal::ZERO)));
        entry.1 = match (entry.1, value_base(p)) {
            (Some(sum), Some(v)) => Some(sum + v),
            // Immaterial exclusions: the class is its priced part.
            (Some(sum), None) if immaterial => Some(sum),
            _ => None,
        };
        if let Some(v) = value_base(p) {
            *rounded_sums.entry(key.clone()).or_default() += money(v);
        }
        *priced.entry(key).or_default() += value_base(p).unwrap_or(Decimal::ZERO);
    }
    let range = |key: &str| -> Option<(f64, f64)> {
        if complete || immaterial {
            return None;
        }
        let own: Vec<&PortfolioPosition> = excluded
            .iter()
            .copied()
            .filter(|p| p.get_asset_class().to_lowercase() == key)
            .collect();
        let other: Vec<&PortfolioPosition> = excluded
            .iter()
            .copied()
            .filter(|p| p.get_asset_class().to_lowercase() != key)
            .collect();
        Some(share_range(
            priced.get(key).copied().unwrap_or(Decimal::ZERO),
            total,
            (!own.is_empty(), sum_max(own.iter())),
            (!other.is_empty(), sum_max(other.iter())),
        ))
    };
    let band = |t: &crate::policy::AllocationTarget| {
        (
            t.target_percent,
            t.tolerance_percent.unwrap_or(DEFAULT_TOLERANCE),
        )
    };
    let mut classes = Vec::new();
    let mut untargeted = Vec::new();
    for key in &order {
        let (label, value) = &values[key];
        let target = policy.and_then(|pol| pol.get_allocation(label));
        // Without a policy every held class (Cash included) is untargeted.
        if target.is_none() {
            untargeted.push(label.clone());
        }
        let reported = (*value).map(|_| money_f(rounded_sums.get(key).copied().unwrap_or(0.0)));
        classes.push(class_row(
            label,
            *value,
            reported,
            total,
            target.map(band),
            range(key),
        ));
    }
    let mut missing = Vec::new();
    if let Some(pol) = policy {
        for t in &pol.allocations {
            let key = t.asset_class.to_lowercase();
            if !values.contains_key(&key) {
                missing.push(t.asset_class.clone());
                classes.push(class_row(
                    &t.asset_class,
                    Some(Decimal::ZERO),
                    Some(0.0),
                    total,
                    Some(band(t)),
                    range(&key),
                ));
            }
        }
    }
    let key = |c: &ClassAllocation| c.value.unwrap_or(f64::NEG_INFINITY);
    classes.sort_by(|a, b| {
        key(b)
            .total_cmp(&key(a))
            .then_with(|| a.asset_class.cmp(&b.asset_class))
    });
    // Same order as `classes` (value descending, then name).
    let untargeted: Vec<String> = classes
        .iter()
        .map(|c| c.asset_class.clone())
        .filter(|c| untargeted.contains(c))
        .collect();
    let reported_total = sum_money(included.iter().filter_map(|p| value_base(p).map(money)));
    Allocation {
        total,
        reported_total,
        complete,
        immaterial,
        materiality_share,
        gain_factors,
        classes,
        untargeted,
        missing,
        excluded_positions: excluded.iter().map(|p| pid(p)).collect(),
        excluded_value_share,
        excluded_value_basis,
        excluded_max: if excluded.is_empty() {
            Some(Decimal::ZERO)
        } else {
            excluded_max_all
        },
    }
}

/// Band status of a share range: set only when the whole range agrees.
fn range_status(min: f64, max: f64, low: f64, high: f64) -> &'static str {
    if min > high {
        "above_band"
    } else if max < low {
        "below_band"
    } else if min >= low && max <= high {
        "within_band"
    } else {
        "unknown"
    }
}

fn class_row(
    label: &str,
    value: Option<Decimal>,
    reported: Option<f64>,
    total: Decimal,
    target: Option<(f64, f64)>,
    range: Option<(f64, f64)>,
) -> ClassAllocation {
    // Percentages and amountToTarget stay on the unrounded sum. `reported`
    // is the sum of the rounded per-position values.
    let actual = value
        .filter(|_| range.is_none())
        .map(|v| percent_of(v, total).unwrap_or(Decimal::ZERO));
    let mut row = ClassAllocation {
        asset_class: label.to_string(),
        value: reported,
        actual_percent: actual.map(pct),
        actual_percent_min: range.map(|r| r.0),
        actual_percent_max: range.map(|r| r.1),
        target_percent: None,
        tolerance_percent: None,
        band_low: None,
        band_high: None,
        drift_points: None,
        status: "untargeted".to_string(),
        amount_to_target: None,
    };
    let Some((t, tol)) = target else {
        return row;
    };
    let low = money_f((t - tol).max(0.0));
    let high = money_f((t + tol).min(100.0));
    row.target_percent = Some(t);
    row.tolerance_percent = Some(tol);
    row.band_low = Some(low);
    row.band_high = Some(high);
    row.status = "unknown".to_string();
    if let Some((min, max)) = range {
        // Partial data: no drift or amount, a band status only when certain.
        row.status = range_status(min, max, low, high).to_string();
    } else if let (Some(v), Some(actual)) = (value, actual) {
        let drift = actual - dec(t);
        row.status = if drift.abs() <= dec(tol) {
            "within_band"
        } else if drift < Decimal::ZERO {
            "below_band"
        } else {
            "above_band"
        }
        .to_string();
        row.drift_points = Some(pct(drift));
        row.amount_to_target = Some(money(total * dec(t) / dec(100.0) - v));
    }
    row
}

pub struct PolicyCheck {
    pub findings: Vec<Finding>,
    pub checks: Vec<ConstraintCheckRow>,
}

/// Tuning for [`check_policy_with`].
#[derive(Debug, Clone)]
pub struct PolicyOptions {
    /// When the excluded positions' estimated share is below this percent
    /// of the total, judge the priced part with normal severity (and a note).
    pub possible_breach_min_share: f64,
    /// Per-class overrides of the plausible-gain factor applied to cost-basis
    /// estimates (lowercase class → factor; `None`: never immaterial).
    pub gain_factors: Vec<(String, Option<f64>)>,
}

pub const DEFAULT_POSSIBLE_BREACH_MIN_SHARE: f64 = 0.5;

/// Default plausible-gain factor for a cost-basis estimate (Finance L2):
/// diversified classes (funds, ETFs, bonds, gold, commodities) can plausibly
/// be worth up to 3× cost; crypto, single stocks and any other class have no
/// plausible bound, so their cost-basis estimate is never immaterial.
pub const DEFAULT_GAIN_FACTOR: f64 = 3.0;

impl PolicyOptions {
    /// The plausible-gain factor for a cost-basis estimate in `class`.
    pub fn gain_factor(&self, class: &str) -> Option<f64> {
        let key = class.to_lowercase();
        // Repeated `--gain-factor` flags: the last one for this class wins.
        if let Some((_, f)) = self.gain_factors.iter().rev().find(|(c, _)| *c == key) {
            return *f;
        }
        match key.as_str() {
            "fund" | "funds" | "etf" | "etfs" | "bond" | "bonds" | "gold" | "commodity"
            | "commodities" => Some(DEFAULT_GAIN_FACTOR),
            _ => None,
        }
    }
}

impl Default for PolicyOptions {
    fn default() -> Self {
        Self {
            possible_breach_min_share: DEFAULT_POSSIBLE_BREACH_MIN_SHARE,
            gain_factors: Vec::new(),
        }
    }
}

pub fn check_policy(portfolio: &Portfolio, policy: &Policy) -> PolicyCheck {
    check_policy_with(portfolio, policy, PolicyOptions::default())
}

/// `--gain-factor` classes that match no asset class in the portfolio.
pub fn unknown_gain_factor_findings(portfolio: &Portfolio, opts: &PolicyOptions) -> Vec<Finding> {
    let known: std::collections::BTreeSet<String> = portfolio
        .positions
        .iter()
        .map(|p| p.get_asset_class().to_lowercase())
        .collect();
    let mut unknown = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (class, _) in &opts.gain_factors {
        if !known.contains(class) && seen.insert(class.clone()) {
            unknown.push(class.clone());
        }
    }
    if unknown.is_empty() {
        return Vec::new();
    }
    vec![Finding::new(
        "unknown_gain_factor_class",
        Severity::Info,
        Category::DataQuality,
        format!(
            "--gain-factor names {} class(es) with no holding: {}.",
            unknown.len(),
            unknown.join(", ")
        ),
    )
    .detail(json!({ "classes": unknown }))]
}

pub fn check_policy_with(
    portfolio: &Portfolio,
    policy: &Policy,
    opts: PolicyOptions,
) -> PolicyCheck {
    let mut findings = Vec::new();
    let mut checks = Vec::new();
    if let Err(e) = policy.validate() {
        findings.push(Finding::new(
            "invalid_policy",
            Severity::Critical,
            Category::Policy,
            format!("Policy failed validation: {e}"),
        ));
    }
    let alloc = allocation_with(portfolio, Some(policy), true, true, &opts);
    // Share of the priced positions only (what the priced part alone says).
    let priced_only = |v: Decimal| percent_of(v, alloc.total).map_or(0.0, pct);
    let class_priced = |class: &str| -> Decimal {
        portfolio
            .positions
            .iter()
            .filter(|p| p.get_asset_class().eq_ignore_ascii_case(class))
            .filter_map(value_base)
            .sum()
    };
    let range_detail = |c: &ClassAllocation| {
        json!({ "actualPercentMin": c.actual_percent_min, "actualPercentMax": c.actual_percent_max,
                "excludedValueShare": alloc.excluded_value_share, "basis": "partial" })
    };
    for c in &alloc.classes {
        let (Some(t), Some(tol)) = (c.target_percent, c.tolerance_percent) else {
            continue;
        };
        let out = c.status == "below_band" || c.status == "above_band";
        if let (Some(min), Some(max)) = (c.actual_percent_min, c.actual_percent_max) {
            // Partial data: certain breaches use the nearest possible share.
            let (low, high) = (c.band_low.unwrap_or(0.0), c.band_high.unwrap_or(100.0));
            if out {
                let drift = if c.status == "above_band" {
                    min - t
                } else {
                    max - t
                };
                let severity = if drift.abs() > 2.0 * tol {
                    Severity::Critical
                } else {
                    Severity::Warning
                };
                findings.push(
                    Finding::new("allocation_out_of_band", severity, Category::Allocation,
                        format!("{} is {:.2}–{:.2}% vs target {:.2}% ±{:.2} whatever the unpriced positions are worth (drift at least {:+.2} pts).", c.asset_class, min, max, t, tol, drift))
                        .class(&c.asset_class)
                        .detail(json!({ "actualPercentMin": min, "actualPercentMax": max, "targetPercent": t, "tolerancePercent": tol, "driftPointsAtLeast": money_f(drift), "basis": "partial" })),
                );
            } else if c.status == "unknown" && (min < low || max > high) {
                findings.push(
                    Finding::new("possible_breach", Severity::Warning, Category::Allocation,
                        format!("{} may be outside its band ({:.2}–{:.2}% possible vs {:.2}–{:.2}%; {:.2}% of the priced part); checked on a partial basis because prices are missing.", c.asset_class, min, max, low, high, priced_only(class_priced(&c.asset_class))))
                        .class(&c.asset_class)
                        .detail(range_detail(c)),
                );
            }
            continue;
        }
        let own_excluded = alloc.immaterial
            && portfolio.positions.iter().any(|p| {
                value_base(p).is_none() && p.get_asset_class().eq_ignore_ascii_case(&c.asset_class)
            });
        match c.drift_points {
            // Immaterial exclusions: an unpriced position of this class may
            // lift it into its band, so "below" is only possible.
            Some(drift)
                if out
                    && own_excluded
                    && c.status == "below_band"
                    && c.value.is_some_and(|v| v > 0.0) =>
            {
                findings.push(
                    Finding::new("possible_breach", Severity::Warning, Category::Allocation,
                        format!("{} may be below its band ({:.2}% of the priced part vs target {:.2}% ±{:.2}); it holds an unpriced position.", c.asset_class, c.actual_percent.unwrap_or_default(), t, tol))
                        .class(&c.asset_class)
                        .detail(json!({ "code": "allocation_out_of_band", "pricedOnlyPercent": c.actual_percent, "targetPercent": t, "tolerancePercent": tol, "driftPoints": drift, "basis": "priced_part" })),
                );
            }
            Some(drift) if out && c.value.is_some_and(|v| v > 0.0) => {
                let severity = if drift.abs() > 2.0 * tol {
                    Severity::Critical
                } else {
                    Severity::Warning
                };
                findings.push(
                    Finding::new("allocation_out_of_band", severity, Category::Allocation,
                        format!("{} is {:.2}% vs target {:.2}% ±{:.2} (drift {:+.2} pts).", c.asset_class, c.actual_percent.unwrap_or_default(), t, tol, drift))
                        .class(&c.asset_class)
                        .detail(json!({ "actualPercent": c.actual_percent, "targetPercent": t, "tolerancePercent": tol, "driftPoints": drift })),
                );
            }
            _ => {}
        }
    }
    for class in &alloc.untargeted {
        findings.push(
            Finding::new(
                "unknown_asset_class",
                Severity::Warning,
                Category::Allocation,
                format!("{class} has holdings but no policy target."),
            )
            .class(class),
        );
    }
    for class in &alloc.missing {
        let target = policy
            .get_allocation(class)
            .map_or(0.0, |t| t.target_percent);
        if target > 0.0 {
            findings.push(
                Finding::new(
                    "missing_target_class",
                    Severity::Warning,
                    Category::Allocation,
                    format!("Policy targets {target:.2}% {class} but there are no holdings."),
                )
                .class(class),
            );
        }
    }

    let total = alloc.total;
    let c = &policy.constraints;
    // With partial data a share is a range: [v / (total + max excluded),
    // v / total]. A breach is reported only when the whole range is over the
    // limit; a range straddling it is a possible_breach warning.
    let partial = !alloc.complete && !alloc.immaterial;
    let mut possible: Vec<serde_json::Value> = Vec::new();
    if let Some(min) = c.minimum_cash_amount {
        let cash: Decimal = portfolio
            .positions
            .iter()
            .filter(|p| p.is_cash())
            .map(PortfolioPosition::balance_dec)
            .sum();
        let passed = cash >= dec(min);
        checks.push(ConstraintCheckRow {
            code: "cash_below_minimum_amount".into(),
            limit_amount: Some(money_f(min)),
            actual_amount: Some(money(cash)),
            passed,
            ..Default::default()
        });
        if !passed {
            findings.push(Finding::new(
                "cash_below_minimum_amount",
                Severity::Critical,
                Category::Constraint,
                "Cash is below the policy's minimum cash amount.".into(),
            ));
        }
    }
    if c.minimum_cash_months.is_some() {
        findings.push(Finding::new(
            "cash_months_unavailable",
            Severity::Info,
            Category::Constraint,
            "minimum_cash_months is set, but monthly expenses are unknown; not checked.".into(),
        ));
    }
    let excluded_any = partial;
    if let Some(limit) = c.single_position_limit_percent {
        for p in portfolio.positions.iter().filter(|p| !p.is_cash()) {
            let Some(v) = value_base(p) else { continue };
            let (min, max) =
                share_range(v, total, (false, None), (excluded_any, alloc.excluded_max));
            let mut row = ConstraintCheckRow {
                code: "single_position_limit".into(),
                limit_percent: Some(limit),
                position_id: p.id().map(String::from),
                ..Default::default()
            };
            if partial {
                row.actual_percent_min = Some(min);
                row.actual_percent_max = Some(max);
            } else {
                row.actual_percent = Some(max);
            }
            if partial && min <= limit && max > limit {
                possible.push(json!({ "code": "single_position_limit", "positionId": p.id() }));
                findings.push(
                    Finding::new("possible_breach", Severity::Warning, Category::Constraint,
                        format!("{} may exceed the {limit:.2}% position limit ({min:.2}–{max:.2}% possible; {:.2}% of the priced part); checked on a partial basis because prices are missing.", p.get_name(), priced_only(v)))
                        .position(p.id())
                        .class(p.get_asset_class())
                        .detail(json!({ "code": "single_position_limit", "limitPercent": limit, "actualPercentMin": min, "actualPercentMax": max, "pricedOnlyPercent": priced_only(v), "basis": "partial" })),
                );
                continue;
            }
            row.passed = min <= limit;
            checks.push(row);
            if min > limit {
                let shown = if partial {
                    format!("at least {min:.2}%")
                } else {
                    format!("{min:.2}%")
                };
                findings.push(
                    Finding::new(
                        "single_position_limit",
                        Severity::Critical,
                        Category::Constraint,
                        format!(
                            "{} is {shown} of the portfolio (limit {limit:.2}%).",
                            p.get_name()
                        ),
                    )
                    .position(p.id())
                    .class(p.get_asset_class()),
                );
            }
        }
    }
    if let Some(limit) = c.asset_class_limit_percent {
        for cl in &alloc.classes {
            let (min, max) = match (
                cl.actual_percent,
                cl.actual_percent_min,
                cl.actual_percent_max,
            ) {
                (Some(a), _, _) if cl.value.is_some_and(|v| v > 0.0) => (a, a),
                (None, Some(lo), Some(hi)) if hi > 0.0 => (lo, hi),
                _ => continue,
            };
            let mut row = ConstraintCheckRow {
                code: "asset_class_limit".into(),
                limit_percent: Some(limit),
                actual_percent: cl.actual_percent,
                actual_percent_min: cl.actual_percent_min,
                actual_percent_max: cl.actual_percent_max,
                asset_class: Some(cl.asset_class.clone()),
                ..Default::default()
            };
            if min <= limit && max > limit {
                possible.push(json!({ "code": "asset_class_limit", "assetClass": cl.asset_class }));
                findings.push(
                    Finding::new("possible_breach", Severity::Warning, Category::Constraint,
                        format!("{} may exceed the {limit:.2}% asset class limit ({min:.2}–{max:.2}% possible; {:.2}% of the priced part); checked on a partial basis because prices are missing.", cl.asset_class, priced_only(class_priced(&cl.asset_class))))
                        .class(&cl.asset_class)
                        .detail(json!({ "code": "asset_class_limit", "limitPercent": limit, "actualPercentMin": min, "actualPercentMax": max, "basis": "partial" })),
                );
                continue;
            }
            row.passed = min <= limit;
            checks.push(row);
            if min > limit {
                let shown = if partial {
                    format!("at least {min:.2}%")
                } else {
                    format!("{min:.2}%")
                };
                findings.push(
                    Finding::new(
                        "asset_class_limit",
                        Severity::Critical,
                        Category::Constraint,
                        format!(
                            "{} is {shown} of the portfolio (limit {limit:.2}%).",
                            cl.asset_class
                        ),
                    )
                    .class(&cl.asset_class),
                );
            }
        }
    }
    if partial {
        // N4: at most one possible_breach per class and limit; the others'
        // positions are listed on the first.
        let key = |f: &Finding| {
            let code = f
                .detail
                .as_ref()
                .and_then(|d| d.get("code"))
                .and_then(|c| c.as_str())
                .unwrap_or("allocation_band")
                .to_string();
            (
                code,
                f.asset_class.clone().unwrap_or_default().to_lowercase(),
            )
        };
        let mut merged: Vec<Finding> = Vec::new();
        for f in findings.drain(..) {
            if f.code == "possible_breach" {
                if let Some(first) = merged
                    .iter_mut()
                    .find(|m| m.code == "possible_breach" && key(m) == key(&f))
                {
                    if let (Some(d), Some(id)) = (first.detail.as_mut(), f.position_id.clone()) {
                        let ids = d
                            .as_object_mut()
                            .map(|o| o.entry("positionIds").or_insert_with(|| json!([])));
                        if let Some(serde_json::Value::Array(a)) = ids {
                            if a.is_empty() {
                                if let Some(own) = &first.position_id {
                                    a.push(json!(own));
                                }
                            }
                            a.push(json!(id));
                        }
                    }
                    continue;
                }
            }
            merged.push(f);
        }
        findings = merged;
        let unknown: Vec<&str> = alloc
            .classes
            .iter()
            .filter(|c| c.status == "unknown")
            .map(|c| c.asset_class.as_str())
            .collect();
        findings.push(
            Finding::new(
                "checks_incomplete",
                Severity::Warning,
                Category::DataQuality,
                format!(
                    "{} position(s) have no price or FX rate (excluded value share {}). Allocation and limits are \
                     judged on the priced part: only breaches that hold whatever the excluded positions are worth are \
                     reported; anything that might be out of bounds is a possible_breach warning.",
                    alloc.excluded_positions.len(),
                    alloc
                        .excluded_value_share
                        .map_or("unknown".to_string(), |s| format!("{s:.2}%"))
                ),
            )
            .detail(json!({ "unknownClasses": unknown, "possibleBreaches": possible,
                            "excludedPositions": alloc.excluded_positions,
                            "excludedValueShare": alloc.excluded_value_share,
                            "excludedValueBasis": alloc.excluded_value_basis })),
        );
    }
    if alloc.immaterial {
        findings.push(
            Finding::new(
                "checks_incomplete",
                Severity::Warning,
                Category::DataQuality,
                format!(
                    "{} position(s) have no price or FX rate, but their estimated share ({}, {}{}) is below the {:.2}% \
                     materiality threshold: allocation and limits are judged on the priced part with normal severity.",
                    alloc.excluded_positions.len(),
                    alloc
                        .excluded_value_share
                        .map_or("unknown".to_string(), |s| format!("{s:.2}%")),
                    alloc.excluded_value_basis.unwrap_or("estimate"),
                    if alloc.gain_factors.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "; {:.2}% with plausible-gain factor {}",
                            alloc.materiality_share.unwrap_or(0.0),
                            alloc
                                .gain_factors
                                .iter()
                                .map(|(c, f)| format!("{f}x {c}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                    opts.possible_breach_min_share
                ),
            )
            .detail(json!({ "basis": "priced_part",
                            "excludedPositions": alloc.excluded_positions,
                            "excludedValueShare": alloc.excluded_value_share,
                            "excludedValueBasis": alloc.excluded_value_basis,
                            "materialityShare": alloc.materiality_share,
                            "gainFactors": alloc.gain_factors,
                            "minShare": opts.possible_breach_min_share })),
        );
    }
    PolicyCheck { findings, checks }
}

/// Synthesized transactions (one per purchase), sorted by date then id.
///
/// A cash position without entries gets one opening deposit dated at the
/// earliest purchase date in the portfolio, else `file_date` (the data
/// file's modification date), with fees 0, `synthesized: true` and
/// `dateSource` naming which.
pub fn transactions(
    portfolio: &Portfolio,
    base: &str,
    file_date: Option<String>,
) -> Vec<Transaction> {
    let mut out = Vec::new();
    let opening_date = portfolio
        .positions
        .iter()
        .flat_map(PortfolioPosition::get_purchases)
        .filter_map(|pur| pur.date.as_deref().and_then(parse_purchase_date))
        .min()
        .map(|d| (d.format("%Y-%m-%d").to_string(), "earliest_purchase"))
        .or(file_date.map(|d| (d, "file_mtime")));
    for p in &portfolio.positions {
        let id = pid(p);
        let currency = if kind(p) == "market" {
            p.price_currency().map(String::from)
        } else {
            Some(base.to_string())
        };
        if p.is_cash() && p.get_purchases().is_empty() {
            let balance = p.balance_dec();
            out.push(Transaction {
                id: format!("tx_{id}_opening"),
                date: opening_date.as_ref().map(|d| d.0.clone()),
                synthesized: true,
                date_source: opening_date.as_ref().map(|d| d.1.to_string()),
                tx_type: TxType::Deposit,
                position_id: id.clone(),
                ticker: None,
                asset_class: p.get_asset_class().to_string(),
                quantity: qty(balance),
                price: Some(1.0),
                price_source: "cash".to_string(),
                fees: Some(0.0),
                amount: Some(money(balance)),
                currency: currency.clone(),
                amount_base: Some(money(balance)),
            });
        }
        for (i, pur) in p.get_purchases().iter().enumerate() {
            let cash = p.is_cash();
            let (price, source) = match (pur.recorded_price(), pur.estimated_price) {
                // Cash moves at par in its own currency.
                _ if cash => (Some(Decimal::ONE), "cash"),
                (Some(pr), _) => (Some(pr), "recorded"),
                (None, Some(est)) => (Some(est), "historic_close"),
                _ => (None, "missing"),
            };
            let sell = pur.quantity < Decimal::ZERO;
            let q = pur.quantity.abs();
            let fees = pur.fees.unwrap_or(Decimal::ZERO);
            let amount = price.map(|pr| if sell { q * pr - fees } else { q * pr + fees });
            let same_ccy = currency
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(base));
            out.push(Transaction {
                id: format!("tx_{id}_{i}"),
                synthesized: false,
                date_source: None,
                date: pur
                    .date
                    .as_deref()
                    .and_then(parse_purchase_date)
                    .map(|d| d.format("%Y-%m-%d").to_string()),
                tx_type: match (cash, sell) {
                    (true, false) => TxType::Deposit,
                    (true, true) => TxType::Withdrawal,
                    (false, true) => TxType::Sell,
                    (false, false) => TxType::Buy,
                },
                position_id: id.clone(),
                ticker: p.get_ticker().map(String::from),
                asset_class: p.get_asset_class().to_string(),
                quantity: qty(q),
                price: price.map(|d| to_f64(d.round_dp(6))),
                price_source: source.to_string(),
                fees: Some(money(fees)),
                amount: amount.map(money),
                currency: currency.clone(),
                amount_base: if same_ccy { amount.map(money) } else { None },
            });
        }
    }
    // Opening deposits first on their date.
    out.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| b.synthesized.cmp(&a.synthesized))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::Portfolio;
    use crate::position::{PortfolioPosition, PriceStatus};

    #[test]
    fn unknown_gain_factor_class_lists_overrides_with_no_holding() {
        let mut portfolio = Portfolio::new();
        portfolio.add_position(PortfolioPosition::for_test(
            Some("SPY"),
            "Stocks",
            1.0,
            1.0,
            PriceStatus::Live,
        ));
        let opts = PolicyOptions {
            gain_factors: vec![
                ("stocks".into(), Some(2.0)),
                ("widgets".into(), Some(2.0)),
                ("widgets".into(), Some(4.0)),
            ],
            ..PolicyOptions::default()
        };
        let findings = unknown_gain_factor_findings(&portfolio, &opts);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "unknown_gain_factor_class");
        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(findings[0].category, Category::DataQuality);
        let classes = findings[0].detail.as_ref().unwrap()["classes"]
            .as_array()
            .unwrap();
        assert_eq!(classes, &vec![json!("widgets")]);
    }
}

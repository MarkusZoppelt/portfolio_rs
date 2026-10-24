//! Pure portfolio analytics behind the MCP tools.
//!
//! Everything here works on an already priced [`Portfolio`]; nothing does I/O.

use super::dto::*;
use crate::money::{dec, percent_of, to_f64, Decimal};
use crate::policy::Policy;
use crate::portfolio::Portfolio;
use crate::position::{parse_purchase_date, PortfolioPosition, PriceStatus};
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
            PriceStatus::Live if p.has_market_price() => "live",
            _ => "unavailable",
        },
    }
}

/// live | cache | fixture | none (cash and manual positions: none).
pub fn price_source(p: &PortfolioPosition) -> &'static str {
    match p.price_source() {
        Some(src) if kind(p) == "market" && p.has_market_price() => src.as_str(),
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

/// Portfolio-level cost basis and PnL over the market positions (cash and
/// manual holdings have none). Each total is null as soon as one market
/// position lacks the input, so a partial sum is never reported as the total.
pub struct PortfolioPnl {
    pub invested: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub unrealized_pnl_percent: Option<Decimal>,
    pub day_pnl: Option<Decimal>,
}

pub fn portfolio_pnl(portfolio: &Portfolio) -> PortfolioPnl {
    let market: Vec<&PortfolioPosition> = portfolio
        .positions
        .iter()
        .filter(|p| kind(p) == "market")
        .collect();
    let sum = |f: &dyn Fn(&PortfolioPosition) -> Option<Decimal>| -> Option<Decimal> {
        market.iter().map(|p| f(p)).sum::<Option<Decimal>>()
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
) -> (Vec<PositionRow>, Decimal, Decimal) {
    let all: Vec<&PortfolioPosition> = portfolio.positions.iter().collect();
    let total = total_known(&all);
    let cash: Decimal = all
        .iter()
        .filter(|p| p.is_cash())
        .map(|p| p.balance_dec())
        .sum();
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
                price: (market && p.has_market_price())
                    .then(|| to_f64(dec(p.market_price()).round_dp(6))),
                price_currency: if market {
                    p.price_currency().map(String::from)
                } else {
                    None
                },
                price_as_of: (market && p.has_market_price())
                    .then(|| p.price_as_of().map(String::from))
                    .flatten(),
                currency: if market {
                    p.price_currency().map(String::from)
                } else {
                    Some(base.to_string())
                },
                is_stale: market && p.has_market_price() && p.is_stale(),
                price_source: price_source(p).to_string(),
                fx_rate: if market { p.fx_rate() } else { None },
                price_status: price_status(p).to_string(),
                value_base: value.map(money),
                weight_percent: value.and_then(|v| percent_of(v, total)).map(pct),
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
                day_change_percent: if market && p.has_market_price() {
                    p.daily_variation_percent().map(|v| pct(dec(v)))
                } else {
                    None
                },
                purchases_count: p.get_purchases().len(),
            }
        })
        .collect();
    let key = |r: &PositionRow| r.value_base.unwrap_or(f64::NEG_INFINITY);
    match sort {
        SortBy::Value => rows.sort_by(|a, b| key(b).total_cmp(&key(a))),
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
    (rows, total, cash)
}

/// Data-quality findings that also go into every response's `warnings`.
pub fn data_quality(portfolio: &Portfolio) -> Vec<Finding> {
    let mut out = Vec::new();
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
        .any(|p| p.cost_basis_at_current_fx())
    {
        out.push(Finding::new(
            "cost_basis_current_fx",
            Severity::Info,
            Category::DataQuality,
            "Invested and PnL of foreign-currency positions convert purchase costs at the current FX rate, not the rate on the purchase date.".to_string(),
        ));
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

/// connected | partial | offline, from the quotes actually present: offline
/// when no market position has a fresh (non-cached) quote, connected when
/// every market position has a fresh quote and FX rate, partial otherwise
/// (including quotes without FX).
pub fn network_status(portfolio: &Portfolio) -> &'static str {
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

pub struct Allocation {
    /// Sum of the priced positions only.
    pub total: Decimal,
    /// Every included position has a value.
    pub complete: bool,
    pub classes: Vec<ClassAllocation>,
    pub untargeted: Vec<String>,
    pub missing: Vec<String>,
}

pub fn allocation(
    portfolio: &Portfolio,
    policy: Option<&Policy>,
    include_cash: bool,
    include_manual: bool,
) -> Allocation {
    let included: Vec<&PortfolioPosition> = portfolio
        .positions
        .iter()
        .filter(|p| include_cash || !p.is_cash())
        .filter(|p| include_manual || kind(p) != "manual")
        .collect();
    let total = total_known(&included);
    let complete = included.iter().all(|p| value_base(p).is_some());
    // Group case-insensitively, keeping the first spelling seen. A class with
    // any unpriced position has an unknown value (never a partial sum or 0).
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, (String, Option<Decimal>)> = HashMap::new();
    for p in &included {
        let key = p.get_asset_class().to_lowercase();
        if !values.contains_key(&key) {
            order.push(key.clone());
        }
        let entry = values
            .entry(key)
            .or_insert_with(|| (p.get_asset_class().to_string(), Some(Decimal::ZERO)));
        entry.1 = match (entry.1, value_base(p)) {
            (Some(sum), Some(v)) => Some(sum + v),
            _ => None,
        };
    }
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
        classes.push(class_row(label, *value, total, target.map(band), complete));
    }
    let mut missing = Vec::new();
    if let Some(pol) = policy {
        for t in &pol.allocations {
            if !values.contains_key(&t.asset_class.to_lowercase()) {
                missing.push(t.asset_class.clone());
                classes.push(class_row(
                    &t.asset_class,
                    Some(Decimal::ZERO),
                    total,
                    Some(band(t)),
                    complete,
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
    Allocation {
        total,
        complete,
        classes,
        untargeted,
        missing,
    }
}

fn class_row(
    label: &str,
    value: Option<Decimal>,
    total: Decimal,
    target: Option<(f64, f64)>,
    complete: bool,
) -> ClassAllocation {
    let actual = value.map(|v| percent_of(v, total).unwrap_or(Decimal::ZERO));
    let mut row = ClassAllocation {
        asset_class: label.to_string(),
        value: value.map(money),
        actual_percent: actual.map(pct),
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
    row.target_percent = Some(t);
    row.tolerance_percent = Some(tol);
    row.band_low = Some(money_f((t - tol).max(0.0)));
    row.band_high = Some(money_f((t + tol).min(100.0)));
    row.status = "unknown".to_string();
    // Drift and buy/sell amounts from a partial total would be wrong: only
    // judge them when every position is priced.
    if let (true, Some(v), Some(actual)) = (complete, value, actual) {
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

pub fn check_policy(portfolio: &Portfolio, policy: &Policy) -> PolicyCheck {
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
    let alloc = allocation(portfolio, Some(policy), true, true);
    for c in &alloc.classes {
        match (c.target_percent, c.tolerance_percent, c.drift_points) {
            (Some(t), Some(tol), Some(drift))
                if (c.status == "below_band" || c.status == "above_band")
                    && c.value.is_some_and(|v| v > 0.0) =>
            {
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
            .map(|t| t.target_percent)
            .unwrap_or(0.0);
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
    // With partial data a weight against the priced total is an upper bound:
    // a pass is certain, a failure is not. Uncertain checks are skipped.
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    if let Some(min) = c.minimum_cash_amount {
        let cash: Decimal = portfolio
            .positions
            .iter()
            .filter(|p| p.is_cash())
            .map(|p| p.balance_dec())
            .sum();
        let passed = cash >= dec(min);
        checks.push(ConstraintCheckRow {
            code: "cash_below_minimum_amount".into(),
            limit_percent: None,
            actual_percent: None,
            limit_amount: Some(money_f(min)),
            actual_amount: Some(money(cash)),
            position_id: None,
            asset_class: None,
            passed,
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
    if let Some(limit) = c.single_position_limit_percent {
        for p in portfolio.positions.iter().filter(|p| !p.is_cash()) {
            let Some(v) = value_base(p) else { continue };
            let w = percent_of(v, total).unwrap_or(Decimal::ZERO);
            let passed = w <= dec(limit);
            if !passed && !alloc.complete {
                skipped.push(json!({ "code": "single_position_limit", "positionId": p.id() }));
                continue;
            }
            checks.push(ConstraintCheckRow {
                code: "single_position_limit".into(),
                limit_percent: Some(limit),
                actual_percent: Some(pct(w)),
                limit_amount: None,
                actual_amount: None,
                position_id: p.id().map(String::from),
                asset_class: None,
                passed,
            });
            if !passed {
                findings.push(
                    Finding::new(
                        "single_position_limit",
                        Severity::Critical,
                        Category::Constraint,
                        format!(
                            "{} is {:.2}% of the portfolio (limit {limit:.2}%).",
                            p.get_name(),
                            pct(w)
                        ),
                    )
                    .position(p.id())
                    .class(p.get_asset_class()),
                );
            }
        }
    }
    if let Some(limit) = c.asset_class_limit_percent {
        for cl in alloc.classes.iter() {
            let (Some(value), Some(actual)) = (cl.value, cl.actual_percent) else {
                skipped.push(json!({ "code": "asset_class_limit", "assetClass": cl.asset_class }));
                continue;
            };
            if value <= 0.0 {
                continue;
            }
            let passed = actual <= limit;
            if !passed && !alloc.complete {
                skipped.push(json!({ "code": "asset_class_limit", "assetClass": cl.asset_class }));
                continue;
            }
            checks.push(ConstraintCheckRow {
                code: "asset_class_limit".into(),
                limit_percent: Some(limit),
                actual_percent: Some(actual),
                limit_amount: None,
                actual_amount: None,
                position_id: None,
                asset_class: Some(cl.asset_class.clone()),
                passed,
            });
            if !passed {
                findings.push(
                    Finding::new(
                        "asset_class_limit",
                        Severity::Critical,
                        Category::Constraint,
                        format!(
                            "{} is {:.2}% of the portfolio (limit {limit:.2}%).",
                            cl.asset_class, actual
                        ),
                    )
                    .class(&cl.asset_class),
                );
            }
        }
    }
    if !alloc.complete {
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
                Category::Policy,
                "Prices or FX rates are missing: allocation drift is not judged and \
                 percentage limits that could be breached are skipped."
                    .into(),
            )
            .detail(json!({ "unknownClasses": unknown, "skippedChecks": skipped })),
        );
    }
    PolicyCheck { findings, checks }
}

/// Synthesized transactions (one per purchase), sorted by date then id.
///
/// A cash position without entries gets one opening deposit dated at the
/// earliest purchase date in the portfolio, else `file_date` (the data
/// file's modification date), with fees 0 and `synthesized: true`.
pub fn transactions(
    portfolio: &Portfolio,
    base: &str,
    file_date: Option<String>,
) -> Vec<Transaction> {
    let mut out = Vec::new();
    let opening_date = portfolio
        .positions
        .iter()
        .flat_map(|p| p.get_purchases())
        .filter_map(|pur| pur.date.as_deref().and_then(parse_purchase_date))
        .min()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .or(file_date);
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
                date: opening_date.clone(),
                synthesized: true,
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

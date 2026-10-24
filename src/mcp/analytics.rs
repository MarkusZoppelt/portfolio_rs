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
            PriceStatus::Live if p.has_market_price() => "live",
            _ => "unavailable",
        },
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

pub fn position_rows(
    portfolio: &Portfolio,
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
                            format!("No FX rate for {ticker}; excluded from totals."),
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
                            format!("No live price for {ticker}; excluded from totals."),
                        )
                        .position(id)
                        .class(p.get_asset_class()),
                    ),
                    _ => {}
                }
                if !p.has_complete_cost_basis() {
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

pub fn no_policy_finding() -> Finding {
    Finding::new(
        "no_policy",
        Severity::Warning,
        Category::Policy,
        "No investment policy is loaded; targets and constraints are unknown.".to_string(),
    )
}

pub struct Allocation {
    pub total: Decimal,
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
    // Group case-insensitively, keeping the first spelling seen.
    let mut order: Vec<String> = Vec::new();
    let mut values: HashMap<String, (String, Decimal)> = HashMap::new();
    for p in &included {
        let Some(v) = value_base(p) else { continue };
        let key = p.get_asset_class().to_lowercase();
        if !values.contains_key(&key) {
            order.push(key.clone());
        }
        values
            .entry(key)
            .or_insert_with(|| (p.get_asset_class().to_string(), Decimal::ZERO))
            .1 += v;
    }
    let mut classes = Vec::new();
    let mut untargeted = Vec::new();
    for key in &order {
        let (label, value) = &values[key];
        let actual = percent_of(*value, total).unwrap_or(Decimal::ZERO);
        let target = policy.and_then(|pol| pol.get_allocation(label));
        if policy.is_some() && target.is_none() {
            untargeted.push(label.clone());
        }
        classes.push(class_row(
            label,
            *value,
            actual,
            total,
            target.map(|t| {
                (
                    t.target_percent,
                    t.tolerance_percent.unwrap_or(DEFAULT_TOLERANCE),
                )
            }),
        ));
    }
    let mut missing = Vec::new();
    if let Some(pol) = policy {
        for t in &pol.allocations {
            if !values.contains_key(&t.asset_class.to_lowercase()) {
                missing.push(t.asset_class.clone());
                classes.push(class_row(
                    &t.asset_class,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    total,
                    Some((
                        t.target_percent,
                        t.tolerance_percent.unwrap_or(DEFAULT_TOLERANCE),
                    )),
                ));
            }
        }
    }
    classes.sort_by(|a, b| {
        b.value
            .total_cmp(&a.value)
            .then_with(|| a.asset_class.cmp(&b.asset_class))
    });
    Allocation {
        total,
        classes,
        untargeted,
        missing,
    }
}

fn class_row(
    label: &str,
    value: Decimal,
    actual: Decimal,
    total: Decimal,
    target: Option<(f64, f64)>,
) -> ClassAllocation {
    let actual_f = pct(actual);
    match target {
        None => ClassAllocation {
            asset_class: label.to_string(),
            value: money(value),
            actual_percent: actual_f,
            target_percent: None,
            tolerance_percent: None,
            band_low: None,
            band_high: None,
            drift_points: None,
            status: "untargeted".to_string(),
            amount_to_target: None,
        },
        Some((t, tol)) => {
            let drift = actual - dec(t);
            let status = if drift.abs() <= dec(tol) {
                "within_band"
            } else if drift < Decimal::ZERO {
                "below_band"
            } else {
                "above_band"
            };
            ClassAllocation {
                asset_class: label.to_string(),
                value: money(value),
                actual_percent: actual_f,
                target_percent: Some(t),
                tolerance_percent: Some(tol),
                band_low: Some(money_f((t - tol).max(0.0))),
                band_high: Some(money_f((t + tol).min(100.0))),
                drift_points: Some(pct(drift)),
                status: status.to_string(),
                amount_to_target: Some(money(total * dec(t) / dec(100.0) - value)),
            }
        }
    }
}

pub struct PolicyCheck {
    pub findings: Vec<Finding>,
    pub checks: Vec<ConstraintCheckRow>,
}

/// Breach codes: a finding with one of these makes the status `breach`.
pub const BREACH_CODES: &[&str] = &[
    "allocation_out_of_band",
    "single_position_limit",
    "asset_class_limit",
    "cash_below_minimum_amount",
    "invalid_policy",
];

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
            (Some(t), Some(tol), Some(drift)) if c.status != "within_band" && c.value > 0.0 => {
                let severity = if drift.abs() > 2.0 * tol {
                    Severity::Critical
                } else {
                    Severity::Warning
                };
                findings.push(
                    Finding::new("allocation_out_of_band", severity, Category::Allocation,
                        format!("{} is {:.2}% vs target {:.2}% ±{:.2} (drift {:+.2} pts).", c.asset_class, c.actual_percent, t, tol, drift))
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
                Severity::Warning,
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
                        Severity::Warning,
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
        for cl in alloc.classes.iter().filter(|c| c.value > 0.0) {
            let passed = cl.actual_percent <= limit;
            checks.push(ConstraintCheckRow {
                code: "asset_class_limit".into(),
                limit_percent: Some(limit),
                actual_percent: Some(cl.actual_percent),
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
                        Severity::Warning,
                        Category::Constraint,
                        format!(
                            "{} is {:.2}% of the portfolio (limit {limit:.2}%).",
                            cl.asset_class, cl.actual_percent
                        ),
                    )
                    .class(&cl.asset_class),
                );
            }
        }
    }
    PolicyCheck { findings, checks }
}

/// Synthesized transactions (one per purchase), sorted by date then id.
pub fn transactions(portfolio: &Portfolio, base: &str) -> Vec<Transaction> {
    let mut out = Vec::new();
    for p in &portfolio.positions {
        let id = pid(p);
        let currency = if kind(p) == "market" {
            p.price_currency().map(String::from)
        } else {
            Some(base.to_string())
        };
        for (i, pur) in p.get_purchases().iter().enumerate() {
            let (price, source) = match (pur.recorded_price(), pur.estimated_price) {
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
                date: pur
                    .date
                    .as_deref()
                    .and_then(parse_purchase_date)
                    .map(|d| d.format("%Y-%m-%d").to_string()),
                tx_type: if sell { TxType::Sell } else { TxType::Buy },
                position_id: id.clone(),
                ticker: p.get_ticker().map(String::from),
                asset_class: p.get_asset_class().to_string(),
                quantity: qty(q),
                price: price.map(|d| to_f64(d.round_dp(6))),
                price_source: source.to_string(),
                fees: pur.fees.map(money),
                amount: amount.map(money),
                currency: currency.clone(),
                amount_base: if same_ccy { amount.map(money) } else { None },
            });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
    out
}

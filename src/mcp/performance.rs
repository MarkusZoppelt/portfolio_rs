//! Period performance from snapshots (spec §6.4, phase 2).
//!
//! `change = netContributions + marketEffect + incomeEffect + manualRevaluation`
//! holds exactly (up to rounding). Per position between two valuations:
//! - market: flows are buys minus sells dated in the interval (purchases
//!   ledger); the rest of the change is market effect (price + FX);
//! - cash: the whole balance change is a contribution (no cash ledger yet,
//!   so interest and dividends paid to cash count as contributions);
//! - manual: purchases dated in the interval are contributions, the rest is
//!   manual revaluation.

use super::dto::{Transaction, TxType};
use super::snapshots::{round2, Snapshot, SnapshotPosition};
use chrono::NaiveDate;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    pub start_value: f64,
    pub end_value: f64,
    pub net_contributions: f64,
    pub market_effect: f64,
    pub manual_revaluation: f64,
    /// Part of `net_contributions` moved between tracked positions (a buy
    /// paid from tracked cash, a sell into it); sums to 0 over the portfolio.
    pub internal_transfers: f64,
    /// Part of `net_contributions` that entered or left the portfolio.
    pub external_flows: f64,
    /// Part of `net_contributions` from positions that appeared or vanished
    /// without a transaction: external, of unknown origin.
    pub external_unknown: f64,
}

impl Effects {
    fn add(&mut self, o: &Effects) {
        self.start_value += o.start_value;
        self.end_value += o.end_value;
        self.net_contributions += o.net_contributions;
        self.market_effect += o.market_effect;
        self.manual_revaluation += o.manual_revaluation;
        self.internal_transfers += o.internal_transfers;
        self.external_flows += o.external_flows;
        self.external_unknown += o.external_unknown;
    }
    pub fn change(&self) -> f64 {
        self.end_value - self.start_value
    }
}

/// Signed base-currency flow of a transaction (buy/deposit +, sell/withdrawal −).
/// Foreign amounts use `fx` (base per unit); `None` when unknown.
fn flow_sign(tx: &Transaction) -> f64 {
    match tx.tx_type {
        TxType::Buy | TxType::Deposit => 1.0,
        TxType::Sell | TxType::Withdrawal => -1.0,
    }
}

fn tx_flow(tx: &Transaction, fx: Option<f64>) -> Option<f64> {
    let amount = match tx.amount_base {
        Some(a) => a,
        None => tx.amount? * fx?,
    };
    Some(flow_sign(tx) * amount)
}

/// Net flows of position `id` dated on or before `date` (the snapshot's own
/// record of the purchase list, Finance B8). `None` when an amount is unknown.
pub fn ledger_to_date(txs: &[Transaction], id: &str, date: &str, fx: Option<f64>) -> Option<f64> {
    let mut sum = 0.0;
    for tx in txs
        .iter()
        .filter(|t| t.position_id == id && t.date.as_deref().is_some_and(|d| d <= date))
    {
        sum += tx_flow(tx, fx)?;
    }
    Some(round2(sum))
}

/// Positions whose recorded `ledgerBase` in any of `points` differs from
/// what today's purchase list gives for that date (an edited or rewritten
/// ledger, Finance B8 S4/S6). Snapshots without the field are not checked.
pub fn ledger_mismatches(points: &[&Snapshot], txs: &[Transaction]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for snap in points {
        for p in &snap.positions {
            let Some(recorded) = p.ledger_base else {
                continue;
            };
            let now = ledger_to_date(txs, &p.position_id, &snap.date, p.fx_rate);
            if now.is_none_or(|n| (n - recorded).abs() > 0.01) {
                out.push((snap.date.clone(), p.position_id.clone()));
            }
        }
    }
    out
}

fn in_interval(tx: &Transaction, after: &str, upto: &str) -> bool {
    tx.date.as_deref().is_some_and(|d| d > after && d <= upto)
}

/// Per-position effects between two complete valuations. `None` when a value
/// or a non-manual flow amount is unknown. A manual position with a missing
/// price is estimated or booked as an unknown external flow, never `None`.
/// Keys: position id → (assetClass, effects).
pub fn position_effects(
    start: &Snapshot,
    end: &Snapshot,
    txs: &[Transaction],
) -> Option<BTreeMap<String, (String, Effects)>> {
    let mut out: BTreeMap<String, (String, Effects)> = BTreeMap::new();
    let mut cash_ids: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = start
        .positions
        .iter()
        .map(|p| p.position_id.as_str())
        .collect();
    ids.extend(end.positions.iter().map(|p| p.position_id.as_str()));
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let s = start.positions.iter().find(|p| p.position_id == id);
        let e = end.positions.iter().find(|p| p.position_id == id);
        let sv = match s {
            Some(p) => p.value_base?,
            None => 0.0,
        };
        let ev = match e {
            Some(p) => p.value_base?,
            None => 0.0,
        };
        let any = e.or(s)?;
        let fx = e.and_then(|p| p.fx_rate).or(s.and_then(|p| p.fx_rate));
        let mut eff = Effects {
            start_value: sv,
            end_value: ev,
            ..Default::default()
        };
        if any.kind == "cash" {
            eff.net_contributions = ev - sv;
            eff.external_flows = ev - sv;
            cash_ids.push(id.to_string());
        } else if (e.is_none() || s.is_none()) && !has_tx_in(txs, id, start, end) {
            // Vanished without a sell entry, or appeared without a buy
            // dated in the interval: an unknown external flow, never a loss
            // or a gain (see [`vanished`] and [`appeared`]).
            eff.net_contributions = ev - sv;
            eff.external_unknown = ev - sv;
        } else if any.kind == "manual" {
            let interval: Vec<&Transaction> = txs
                .iter()
                .filter(|t| t.position_id == id && in_interval(t, &start.date, &end.date))
                .collect();
            book_manual(&mut eff, s, e, &interval, fx, sv, ev);
        } else {
            let mut flows = 0.0;
            for tx in txs
                .iter()
                .filter(|t| t.position_id == id && in_interval(t, &start.date, &end.date))
            {
                flows += tx_flow(tx, fx)?;
            }
            eff.net_contributions = flows;
            eff.external_flows = flows;
            eff.market_effect = ev - sv - flows;
        }
        out.insert(id.to_string(), (any.asset_class.clone(), eff));
    }
    let deposits = dated_cash_deposits(txs, &cash_ids, &start.date, &end.date);
    split_internal(&mut out, &cash_ids, deposits);
    Some(out)
}

/// Dated non-synthesized cash deposits (+) and withdrawals (−) in
/// `(after, upto]`. `None` when there is no such transaction (a missing
/// `amount_base` is skipped, but the transaction still counts as present).
fn dated_cash_deposits(
    txs: &[Transaction],
    cash_ids: &[String],
    after: &str,
    upto: &str,
) -> Option<f64> {
    let mut sum = 0.0;
    let mut any = false;
    for tx in txs {
        if tx.synthesized
            || !matches!(tx.tx_type, TxType::Deposit | TxType::Withdrawal)
            || !cash_ids.iter().any(|id| id == &tx.position_id)
            || !in_interval(tx, after, upto)
        {
            continue;
        }
        any = true;
        if let Some(amount) = tx.amount_base {
            sum += flow_sign(tx) * amount;
        }
    }
    any.then_some(sum)
}

/// Manual add-on without a price. Estimate `quantity * unit` (start snapshot
/// unit value, else `manual_unit_value`) only when end quantity equals start
/// plus the signed transactions. Otherwise book the value change as an
/// unknown external flow. Never fails the period.
fn book_manual(
    eff: &mut Effects,
    s: Option<&SnapshotPosition>,
    e: Option<&SnapshotPosition>,
    interval: &[&Transaction],
    fx: Option<f64>,
    sv: f64,
    ev: f64,
) {
    let qty_delta: f64 = interval.iter().map(|t| flow_sign(t) * t.quantity).sum();
    let consistent = match (s, e) {
        (Some(s), Some(e)) => (e.quantity - s.quantity - qty_delta).abs() < 1e-6,
        _ => false,
    };
    let start_unit = s.and_then(|p| {
        if p.quantity > 0.0 {
            p.value_base.map(|v| v / p.quantity)
        } else {
            None
        }
    });
    let mut known = 0.0;
    let mut estimated = 0.0;
    let mut all_known = true;
    for tx in interval {
        if let Some(flow) = tx_flow(tx, fx) {
            known += flow;
            estimated += flow;
            continue;
        }
        let unit = if consistent {
            start_unit.or_else(|| {
                if tx.price_source == "manual_unit_value" {
                    tx.price
                } else {
                    None
                }
            })
        } else {
            None
        };
        if let Some(unit) = unit {
            estimated += flow_sign(tx) * tx.quantity * unit;
        } else {
            all_known = false;
        }
    }
    if all_known {
        eff.net_contributions = estimated;
        eff.external_flows = estimated;
        eff.manual_revaluation = ev - sv - estimated;
    } else {
        // Residual of the value change, plus any flows we could price.
        eff.net_contributions = ev - sv;
        eff.external_flows = known;
        eff.external_unknown = ev - sv - known;
        eff.manual_revaluation = 0.0;
    }
}

/// Net security flows matched against tracked cash. `deposits` is the step's
/// dated cash deposits minus withdrawals (`None`: no such transaction).
/// Unknown flows already sit in `external_unknown` and are not touched.
/// Each position keeps `internal + external + unknown == net_contributions`.
fn split_internal(
    out: &mut BTreeMap<String, (String, Effects)>,
    cash_ids: &[String],
    deposits: Option<f64>,
) {
    let is_cash = |id: &String| cash_ids.contains(id);
    let securities: f64 = out
        .iter()
        .filter(|(id, _)| !is_cash(id))
        .map(|(_, (_, e))| e.external_flows)
        .sum();
    let cash: f64 = out
        .iter()
        .filter(|(id, _)| is_cash(id))
        .map(|(_, (_, e))| e.external_flows)
        .sum();
    if let Some(d) = deposits {
        // Cash that left the cash balances toward securities.
        let moved = -(cash - d);
        if securities == 0.0 || moved == 0.0 || securities.signum() != moved.signum() {
            return;
        }
        let n_cash = out.keys().filter(|id| is_cash(id)).count();
        if n_cash == 0 {
            return;
        }
        let matched = moved.abs().min(securities.abs());
        let sec_share = matched / securities.abs();
        let cash_internal = -securities.signum() * matched;
        for (id, (_, e)) in out.iter_mut() {
            if is_cash(id) {
                continue;
            }
            let internal = e.external_flows * sec_share;
            e.internal_transfers = internal;
            e.external_flows -= internal;
        }
        for (id, (_, e)) in out.iter_mut() {
            if !is_cash(id) {
                continue;
            }
            let share = if cash == 0.0 {
                1.0 / n_cash as f64
            } else {
                e.external_flows / cash
            };
            let internal = cash_internal * share;
            e.internal_transfers = internal;
            e.external_flows -= internal;
        }
    } else {
        if securities == 0.0 || cash == 0.0 {
            return;
        }
        if securities.signum() == cash.signum() {
            // Same-step deposit and buy, with no dated cash entry: the
            // security flow is external of unknown origin. Cash stays.
            for (id, (_, e)) in out.iter_mut() {
                if !is_cash(id) {
                    e.external_unknown += e.external_flows;
                    e.external_flows = 0.0;
                }
            }
            return;
        }
        let matched = securities.abs().min(cash.abs());
        let sec_share = matched / securities.abs();
        let cash_share = matched / cash.abs();
        for (id, (_, e)) in out.iter_mut() {
            let share = if is_cash(id) { cash_share } else { sec_share };
            e.internal_transfers = e.external_flows * share;
            e.external_flows -= e.internal_transfers;
        }
    }
}

/// Re-split internal and external flows on each snapshot step and sum them.
/// A deposit on one day and a buy from cash on a later day then match, which
/// a single split over the whole span cannot see. No-op when there is only
/// one step, or when a step cannot be valued (the overall split is kept).
/// `net_contributions` is unchanged: any residual lands in `external_unknown`.
pub fn split_stepwise(
    effects: &mut BTreeMap<String, (String, Effects)>,
    points: &[Snapshot],
    txs: &[Transaction],
) {
    if points.len() <= 2 {
        return;
    }
    let mut sums: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();
    for w in points.windows(2) {
        let Some(step) = position_effects(&w[0], &w[1], txs) else {
            return;
        };
        for (id, (_, e)) in step {
            let acc = sums.entry(id).or_insert((0.0, 0.0, 0.0));
            acc.0 += e.internal_transfers;
            acc.1 += e.external_flows;
            acc.2 += e.external_unknown;
        }
    }
    for (id, (_, e)) in effects.iter_mut() {
        let (internal, external, unknown) = sums.get(id).copied().unwrap_or((0.0, 0.0, 0.0));
        e.internal_transfers = internal;
        e.external_flows = external;
        e.external_unknown = unknown;
        let residual = e.net_contributions - (internal + external + unknown);
        e.external_unknown += residual;
    }
}

fn has_tx_in(txs: &[Transaction], id: &str, start: &Snapshot, end: &Snapshot) -> bool {
    txs.iter()
        .any(|t| t.position_id == id && in_interval(t, &start.date, &end.date))
}

/// Non-cash positions held at the end (non-zero value) that are not in the
/// start snapshot and have no transaction dated in the interval (Finance B3,
/// S3: a buy recorded late, dated before the start). Flows follow the
/// transaction dates, so their value is an unknown external flow, not a gain.
pub fn appeared(start: &Snapshot, end: &Snapshot, txs: &[Transaction]) -> Vec<String> {
    end.positions
        .iter()
        .filter(|p| p.kind != "cash" && p.value_base.is_some_and(|v| v != 0.0))
        .filter(|p| {
            !start
                .positions
                .iter()
                .any(|s| s.position_id == p.position_id)
        })
        .filter(|p| !has_tx_in(txs, &p.position_id, start, end))
        .map(|p| p.position_id.clone())
        .collect()
}

/// Non-cash positions held at the start (non-zero value) that are gone at
/// the end with no transaction in the interval: removed from the data file
/// without a sell entry. Their value is booked as an unknown external flow.
pub fn vanished(start: &Snapshot, end: &Snapshot, txs: &[Transaction]) -> Vec<String> {
    start
        .positions
        .iter()
        .filter(|p| p.kind != "cash" && p.value_base.is_some_and(|v| v != 0.0))
        .filter(|p| !end.positions.iter().any(|e| e.position_id == p.position_id))
        .filter(|p| !has_tx_in(txs, &p.position_id, start, end))
        .map(|p| p.position_id.clone())
        .collect()
}

pub fn total(effects: &BTreeMap<String, (String, Effects)>) -> Effects {
    let mut t = Effects::default();
    for (_, e) in effects.values() {
        t.add(e);
    }
    t
}

pub fn group_by_class(effects: &BTreeMap<String, (String, Effects)>) -> BTreeMap<String, Effects> {
    let mut out: BTreeMap<String, Effects> = BTreeMap::new();
    for (class, e) in effects.values() {
        out.entry(class.clone()).or_default().add(e);
    }
    out
}

fn days(a: &str, b: &str) -> Option<i64> {
    let a = NaiveDate::parse_from_str(a, "%Y-%m-%d").ok()?;
    let b = NaiveDate::parse_from_str(b, "%Y-%m-%d").ok()?;
    Some((b - a).num_days())
}

/// Time-weighted return in percent over `points` (sorted, first = start,
/// last = end). Daily-chained when every gap is one day ("daily_twr"),
/// otherwise Modified Dietz over the whole span with each interval's
/// flows dated at the interval end ("modified_dietz"). `None` when a
/// denominator is zero or a value is unknown.
pub fn twr(points: &[Snapshot], txs: &[Transaction]) -> (Option<f64>, &'static str) {
    if points.len() < 2 {
        return (None, "modified_dietz");
    }
    let daily = points
        .windows(2)
        .all(|w| days(&w[0].date, &w[1].date) == Some(1));
    let mut intervals = Vec::new();
    let mut per_position = Vec::new();
    for w in points.windows(2) {
        match position_effects(&w[0], &w[1], txs) {
            Some(e) => {
                intervals.push((w[1].date.clone(), total(&e)));
                per_position.push(e);
            }
            None => return (None, if daily { "daily_twr" } else { "modified_dietz" }),
        }
    }
    if daily {
        let mut growth = 1.0;
        for (_, e) in &intervals {
            if e.start_value == 0.0 {
                return (None, "daily_twr");
            }
            growth *= (e.end_value - e.net_contributions) / e.start_value;
        }
        (Some((growth - 1.0) * 100.0), "daily_twr")
    } else {
        let start = &points[0];
        let end = points.last().unwrap();
        let Some(span) = days(&start.date, &end.date).filter(|d| *d > 0) else {
            return (None, "modified_dietz");
        };
        let v0 = intervals[0].1.start_value;
        let v1 = intervals.last().unwrap().1.end_value;
        let mut flows = 0.0;
        let mut weighted = 0.0;
        // Weight each buy/sell at its own transaction date; cash changes
        // and unknown flows (no dated transaction) at the snapshot that
        // shows them.
        let weight = |date: &str| {
            let t = days(&start.date, date).unwrap_or(span).clamp(0, span) as f64;
            (span as f64 - t) / span as f64
        };
        for (i, (date, e)) in intervals.iter().enumerate() {
            flows += e.net_contributions;
            let (w0, w1) = (&points[i], &points[i + 1]);
            for (id, (_, pe)) in &per_position[i] {
                let s = w0.positions.iter().find(|p| p.position_id == *id);
                let en = w1.positions.iter().find(|p| p.position_id == *id);
                let is_cash = en.or(s).is_some_and(|p| p.kind == "cash");
                let dated: Vec<&Transaction> = txs
                    .iter()
                    .filter(|t| t.position_id == *id && in_interval(t, &w0.date, &w1.date))
                    .collect();
                if is_cash || dated.is_empty() {
                    weighted += pe.net_contributions * weight(date);
                    continue;
                }
                let fx = en.and_then(|p| p.fx_rate).or(s.and_then(|p| p.fx_rate));
                for tx in dated {
                    let amount = tx_flow(tx, fx).unwrap_or(0.0);
                    weighted += amount * weight(tx.date.as_deref().unwrap_or(date));
                }
            }
        }
        let denom = v0 + weighted;
        if denom == 0.0 {
            return (None, "modified_dietz");
        }
        (Some((v1 - v0 - flows) / denom * 100.0), "modified_dietz")
    }
}

pub fn r2(v: f64) -> f64 {
    round2(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::dto::TxType;
    use crate::mcp::snapshots::SnapshotPosition;

    fn pos(id: &str, kind: &str, class: &str, v: f64) -> SnapshotPosition {
        SnapshotPosition {
            ledger_base: None,
            position_id: id.into(),
            kind: kind.into(),
            asset_class: class.into(),
            quantity: 1.0,
            price: None,
            price_currency: None,
            fx_rate: None,
            value_base: Some(v),
        }
    }

    fn snap(date: &str, a: f64, cash: f64) -> Snapshot {
        Snapshot {
            date: date.into(),
            base_currency: "EUR".into(),
            positions: vec![
                pos("a", "market", "Stocks", a),
                pos("c", "cash", "Cash", cash),
            ],
            cash_base: cash,
            total_base: a + cash,
            net_flows_base: 0.0,
            complete: true,
            data_status: "complete".into(),
        }
    }

    fn buy(date: &str, amount: f64) -> Transaction {
        Transaction {
            id: format!("tx_a_{date}"),
            date: Some(date.into()),
            synthesized: false,
            date_source: None,
            tx_type: TxType::Buy,
            position_id: "a".into(),
            ticker: Some("A".into()),
            asset_class: "Stocks".into(),
            quantity: 1.0,
            price: Some(amount),
            price_source: "recorded".into(),
            fees: Some(0.0),
            amount: Some(amount),
            currency: Some("EUR".into()),
            amount_base: Some(amount),
        }
    }

    #[test]
    fn identity_holds_and_buys_are_contributions() {
        let s = snap("2026-01-01", 100.0, 10.0);
        let e = snap("2026-01-03", 220.0, 10.0);
        let t = total(&position_effects(&s, &e, &[buy("2026-01-02", 100.0)]).unwrap());
        assert_eq!(t.change(), 120.0);
        assert_eq!(t.net_contributions, 100.0);
        assert_eq!(t.market_effect, 20.0);
        assert!(
            (t.change() - (t.net_contributions + t.market_effect + t.manual_revaluation)).abs()
                < 1e-9
        );
        // A buy before the interval does not count.
        let t = total(&position_effects(&s, &e, &[buy("2026-01-01", 100.0)]).unwrap());
        assert_eq!(t.net_contributions, 0.0);
    }

    /// Finding 4 regression: a cash deposit is a contribution, never return.
    #[test]
    fn buy_from_tracked_cash_is_internal_for_both_classes() {
        // a: 100 → 200 with a 100 buy; cash 500 → 450: 50 came from cash,
        // 50 from outside.
        let txs = vec![buy("2026-01-02", 100.0)];
        let fx = position_effects(
            &snap("2026-01-01", 100.0, 500.0),
            &snap("2026-01-10", 200.0, 450.0),
            &txs,
        )
        .unwrap();
        let c = group_by_class(&fx);
        let st = &c["Stocks"];
        assert_eq!((st.internal_transfers, st.external_flows), (50.0, 50.0));
        let ca = &c["Cash"];
        assert_eq!((ca.internal_transfers, ca.external_flows), (-50.0, 0.0));
        let t = total(&fx);
        assert_eq!(t.internal_transfers, 0.0);
        assert_eq!(t.external_flows, t.net_contributions);
    }

    #[test]
    fn cash_deposit_is_not_return() {
        let s = snap("2026-01-01", 100.0, 0.0);
        let e = snap("2026-01-11", 100.0, 500.0);
        let t = total(&position_effects(&s, &e, &[]).unwrap());
        assert_eq!(t.net_contributions, 500.0);
        assert_eq!(t.market_effect, 0.0);
        assert_eq!(twr(&[s, e], &[]).0, Some(0.0));
    }

    #[test]
    fn daily_twr_matches_hand_calculation() {
        // d1 100 → d2 110 (+10%); buy 100 on d3, end 220: (220-100)/110 - 1.
        let pts = [
            snap("2026-01-01", 100.0, 0.0),
            snap("2026-01-02", 110.0, 0.0),
            snap("2026-01-03", 220.0, 0.0),
        ];
        let (r, m) = twr(&pts, &[buy("2026-01-03", 100.0)]);
        assert_eq!(m, "daily_twr");
        let expected = (1.1 * (120.0 / 110.0) - 1.0) * 100.0;
        assert!((r.unwrap() - expected).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn modified_dietz_matches_hand_calculation() {
        // d1: 100 + 0 cash; d6: 105 + 50 cash (deposit, weight 5/10); d11: 120 + 50.
        // MD = (170 - 100 - 50) / (100 + 50 * 0.5) = 16 %.
        let pts = [
            snap("2026-01-01", 100.0, 0.0),
            snap("2026-01-06", 105.0, 50.0),
            snap("2026-01-11", 120.0, 50.0),
        ];
        let (r, m) = twr(&pts, &[]);
        assert_eq!(m, "modified_dietz");
        assert!((r.unwrap() - 16.0).abs() < 1e-9, "{r:?}");
    }

    /// Finance LOW: a buy is weighted at its transaction date, the cash
    /// that paid for it at the snapshot that shows it.
    #[test]
    fn modified_dietz_dates_buys_on_the_transaction_date() {
        // d1: a 100. d11: a 230 after an external 100 buy on d2 (no cash).
        let pts = [
            snap("2026-01-01", 100.0, 0.0),
            snap("2026-01-11", 230.0, 0.0),
        ];
        let tx = buy("2026-01-02", 100.0);
        let (r, m) = twr(&pts, &[tx]);
        assert_eq!(m, "modified_dietz");
        // (230 - 100 - 100) / (100 + 100 * 9/10) = 30 / 190.
        assert!((r.unwrap() - 3000.0 / 190.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn unknown_value_gives_none() {
        let s = snap("2026-01-01", 100.0, 0.0);
        let mut e = snap("2026-01-02", 100.0, 0.0);
        e.positions[0].value_base = None;
        assert!(position_effects(&s, &e, &[]).is_none());
    }

    fn deposit(date: &str, amount: f64) -> Transaction {
        Transaction {
            id: format!("tx_c_{date}"),
            date: Some(date.into()),
            synthesized: false,
            date_source: None,
            tx_type: TxType::Deposit,
            position_id: "c".into(),
            ticker: None,
            asset_class: "Cash".into(),
            quantity: amount.abs(),
            price: Some(1.0),
            price_source: "cash".into(),
            fees: Some(0.0),
            amount: Some(amount.abs()),
            currency: Some("EUR".into()),
            amount_base: Some(amount.abs()),
        }
    }

    /// A deposit on one day and a buy from that cash on the next must match
    /// per step. Over the whole span both external flows are positive, so a
    /// single split leaves the buy external.
    #[test]
    fn stepwise_split_separates_deposit_day_from_buy_day() {
        let pts = [
            snap("2026-01-01", 100.0, 0.0),
            snap("2026-01-02", 100.0, 10_000.0),
            snap("2026-01-03", 3_940.0, 6_160.0),
        ];
        let txs = vec![buy("2026-01-03", 3_840.0)];
        let mut fx = position_effects(&pts[0], &pts[2], &txs).unwrap();
        let before = fx.clone();
        split_stepwise(&mut fx, &pts, &txs);
        let c = group_by_class(&fx);
        let st = &c["Stocks"];
        assert_eq!(st.internal_transfers, 3_840.0);
        assert_eq!(st.external_flows, 0.0);
        let ca = &c["Cash"];
        assert_eq!(ca.internal_transfers, -3_840.0);
        assert_eq!(ca.external_flows, 10_000.0);
        for (id, (_, e)) in &fx {
            let (_, orig) = &before[id];
            assert_eq!(e.net_contributions, orig.net_contributions);
            assert_eq!(e.market_effect, orig.market_effect);
            assert_eq!(e.manual_revaluation, orig.manual_revaluation);
            let parts = e.internal_transfers + e.external_flows + e.external_unknown;
            assert!((parts - e.net_contributions).abs() < 1e-9, "{id} {e:?}");
        }
    }

    #[test]
    fn same_day_deposit_and_buy_without_tx_is_unknown() {
        let s = snap("2026-01-01", 100.0, 0.0);
        let e = snap("2026-01-02", 3_940.0, 6_160.0);
        let fx = position_effects(&s, &e, &[buy("2026-01-02", 3_840.0)]).unwrap();
        let c = group_by_class(&fx);
        let st = &c["Stocks"];
        assert_eq!(st.external_unknown, 3_840.0);
        assert_eq!(st.external_flows, 0.0);
        assert_eq!(c["Cash"].external_flows, 6_160.0);
    }

    #[test]
    fn same_day_deposit_tx_matches_the_buy() {
        let s = snap("2026-01-01", 100.0, 0.0);
        let e = snap("2026-01-02", 3_940.0, 6_160.0);
        let txs = vec![buy("2026-01-02", 3_840.0), deposit("2026-01-02", 10_000.0)];
        let fx = position_effects(&s, &e, &txs).unwrap();
        let c = group_by_class(&fx);
        assert_eq!(c["Stocks"].internal_transfers, 3_840.0);
        assert_eq!(c["Cash"].internal_transfers, -3_840.0);
        assert_eq!(c["Cash"].external_flows, 10_000.0);
        let parts = |e: &Effects| e.internal_transfers + e.external_flows + e.external_unknown;
        assert!((parts(&c["Stocks"]) - c["Stocks"].net_contributions).abs() < 1e-9);
        assert!((parts(&c["Cash"]) - c["Cash"].net_contributions).abs() < 1e-9);
    }

    fn manual_snap(date: &str, qty: f64, value: f64) -> Snapshot {
        Snapshot {
            date: date.into(),
            base_currency: "EUR".into(),
            positions: vec![SnapshotPosition {
                position_id: "apt".into(),
                kind: "manual".into(),
                asset_class: "RealEstate".into(),
                quantity: qty,
                price: None,
                price_currency: None,
                fx_rate: None,
                value_base: Some(value),
                ledger_base: None,
            }],
            cash_base: 0.0,
            total_base: value,
            net_flows_base: 0.0,
            complete: true,
            data_status: "complete".into(),
        }
    }

    fn manual_buy(date: &str, qty: f64, price: Option<f64>, source: &str) -> Transaction {
        Transaction {
            id: format!("tx_apt_{date}"),
            date: Some(date.into()),
            synthesized: false,
            date_source: None,
            tx_type: TxType::Buy,
            position_id: "apt".into(),
            ticker: None,
            asset_class: "RealEstate".into(),
            quantity: qty,
            price,
            price_source: source.into(),
            fees: Some(0.0),
            amount: None,
            currency: Some("EUR".into()),
            amount_base: None,
        }
    }

    #[test]
    fn manual_addon_without_price_estimates_when_quantities_agree() {
        // qty 1 → 3, add-on of 2, unit 200: flow 400, the rest is revaluation.
        let s = manual_snap("2026-01-01", 1.0, 200.0);
        let e = manual_snap("2026-01-02", 3.0, 500.0);
        let fx = position_effects(&s, &e, &[manual_buy("2026-01-02", 2.0, None, "missing")])
            .expect("manual missing price must not null the period");
        let eff = &fx["apt"].1;
        assert_eq!(eff.net_contributions, 400.0);
        assert_eq!(eff.external_flows, 400.0);
        assert_eq!(eff.manual_revaluation, -100.0);
        assert_eq!(eff.external_unknown, 0.0);

        // No start unit (quantity 0): fall back to manual_unit_value.
        let s = manual_snap("2026-01-01", 0.0, 0.0);
        let e = manual_snap("2026-01-02", 2.0, 250.0);
        let tx = manual_buy("2026-01-02", 2.0, Some(100.0), "manual_unit_value");
        let fx = position_effects(&s, &e, &[tx]).unwrap();
        let eff = &fx["apt"].1;
        assert_eq!(eff.net_contributions, 200.0);
        assert_eq!(eff.manual_revaluation, 50.0);
    }

    #[test]
    fn manual_addon_without_price_books_unknown_when_quantities_disagree() {
        let s = manual_snap("2026-01-01", 1.0, 200.0);
        let e = manual_snap("2026-01-02", 1.0, 350.0);
        let fx = position_effects(&s, &e, &[manual_buy("2026-01-02", 2.0, None, "missing")])
            .expect("inconsistent manual add-on must not null the period");
        let eff = &fx["apt"].1;
        assert_eq!(eff.net_contributions, 150.0);
        assert_eq!(eff.external_unknown, 150.0);
        assert_eq!(eff.external_flows, 0.0);
        assert_eq!(eff.manual_revaluation, 0.0);

        // A market position with a missing amount still fails the period.
        let s = snap("2026-01-01", 100.0, 0.0);
        let e = snap("2026-01-02", 200.0, 0.0);
        let mut tx = buy("2026-01-02", 50.0);
        tx.amount = None;
        tx.amount_base = None;
        assert!(position_effects(&s, &e, &[tx]).is_none());
    }
}

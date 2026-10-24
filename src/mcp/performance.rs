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

use super::dto::Transaction;
use super::snapshots::{round2, Snapshot};
use chrono::NaiveDate;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    pub start_value: f64,
    pub end_value: f64,
    pub net_contributions: f64,
    pub market_effect: f64,
    pub manual_revaluation: f64,
}

impl Effects {
    fn add(&mut self, o: &Effects) {
        self.start_value += o.start_value;
        self.end_value += o.end_value;
        self.net_contributions += o.net_contributions;
        self.market_effect += o.market_effect;
        self.manual_revaluation += o.manual_revaluation;
    }
    pub fn change(&self) -> f64 {
        self.end_value - self.start_value
    }
}

/// Signed base-currency flow of a transaction (buy/deposit +, sell/withdrawal −).
/// Foreign amounts use `fx` (base per unit); `None` when unknown.
fn tx_flow(tx: &Transaction, fx: Option<f64>) -> Option<f64> {
    use super::dto::TxType;
    let sign = match tx.tx_type {
        TxType::Buy | TxType::Deposit => 1.0,
        TxType::Sell | TxType::Withdrawal => -1.0,
    };
    let amount = match tx.amount_base {
        Some(a) => a,
        None => tx.amount? * fx?,
    };
    Some(sign * amount)
}

fn in_interval(tx: &Transaction, after: &str, upto: &str) -> bool {
    tx.date.as_deref().is_some_and(|d| d > after && d <= upto)
}

/// Per-position effects between two complete valuations. `None` when a value
/// or a flow amount is unknown. Keys: position id → (assetClass, effects).
pub fn position_effects(
    start: &Snapshot,
    end: &Snapshot,
    txs: &[Transaction],
) -> Option<BTreeMap<String, (String, Effects)>> {
    let mut out: BTreeMap<String, (String, Effects)> = BTreeMap::new();
    let mut ids: Vec<&str> = start
        .positions
        .iter()
        .map(|p| p.position_id.as_str())
        .collect();
    ids.extend(end.positions.iter().map(|p| p.position_id.as_str()));
    ids.sort();
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
        } else if (e.is_none() || s.is_none()) && !has_tx_in(txs, id, start, end) {
            // Vanished without a sell entry, or appeared without a buy
            // dated in the interval: an unknown external flow, never a loss
            // or a gain (see [`vanished`] and [`appeared`]).
            eff.net_contributions = ev - sv;
        } else {
            let mut flows = 0.0;
            for tx in txs
                .iter()
                .filter(|t| t.position_id == id && in_interval(t, &start.date, &end.date))
            {
                flows += tx_flow(tx, fx)?;
            }
            eff.net_contributions = flows;
            if any.kind == "manual" {
                eff.manual_revaluation = ev - sv - flows;
            } else {
                eff.market_effect = ev - sv - flows;
            }
        }
        out.insert(id.to_string(), (any.asset_class.clone(), eff));
    }
    Some(out)
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
    for w in points.windows(2) {
        match position_effects(&w[0], &w[1], txs) {
            Some(e) => intervals.push((w[1].date.clone(), total(&e))),
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
        for (date, e) in &intervals {
            let t = days(&start.date, date).unwrap_or(span) as f64;
            flows += e.net_contributions;
            weighted += e.net_contributions * (span as f64 - t) / span as f64;
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

    #[test]
    fn unknown_value_gives_none() {
        let s = snap("2026-01-01", 100.0, 0.0);
        let mut e = snap("2026-01-02", 100.0, 0.0);
        e.positions[0].value_base = None;
        assert!(position_effects(&s, &e, &[]).is_none());
    }
}

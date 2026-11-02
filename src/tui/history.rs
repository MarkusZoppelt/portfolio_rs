//! Series for the portfolio growth chart.

use chrono::{DateTime, Utc};
use futures::future::join_all;

use super::live::LiveData;
use crate::money::{to_f64, Decimal};
use crate::portfolio::Portfolio;
use crate::position::parse_purchase_date;
use crate::services::quotes::AnyProvider;

/// Weekly cumulative invested amount from the first dated, priced purchase
/// until `now`, ending with the current portfolio value.
pub(super) fn get_historic_portfolio_data(
    portfolio: &Portfolio,
    now: DateTime<Utc>,
) -> Vec<(f64, f64)> {
    // `(date, invested)` of every dated, priced purchase, oldest first.
    let mut purchase_events: Vec<(DateTime<Utc>, f64)> = portfolio
        .positions
        .iter()
        .flat_map(|position| position.get_purchases())
        .filter_map(|purchase| {
            let date = parse_purchase_date(purchase.date.as_deref()?)?;
            let price = purchase.effective_price()?;
            (price > Decimal::ZERO && purchase.quantity > Decimal::ZERO)
                .then(|| (date, to_f64(purchase.quantity * price)))
        })
        .collect();
    purchase_events.sort_by_key(|event| event.0);
    let Some(&(earliest_date, _)) = purchase_events.first() else {
        return vec![];
    };

    // Weekly points of the cumulative invested amount, from the earliest purchase to now
    let mut weekly_data = Vec::new();
    let mut cumulative_invested = 0.0;
    let mut events = purchase_events.iter().peekable();
    let mut current_date = earliest_date;
    let mut week_index: usize = 0;

    while current_date <= now {
        // Add all purchases that happened before or on this date
        while let Some((_, invested)) = events.next_if(|(date, _)| *date <= current_date) {
            cumulative_invested += invested;
        }

        if cumulative_invested > 0.0 {
            weekly_data.push((week_index as f64, cumulative_invested));
        }

        current_date += chrono::Duration::days(7);
        week_index += 1;
    }

    // Add current portfolio value as the final point
    let current_value = portfolio.get_total_value();
    if current_value > 0.0 {
        weekly_data.push((week_index as f64, current_value));
    }

    weekly_data
}

/// At most ~78 weeks (~18 months) for speed and readability.
const MAX_CHART_WEEKS: usize = 78;

/// A ticker position as seen by the weekly chart: units held and the current
/// spot price in the quote currency.
#[derive(Debug, Clone, Copy)]
pub(super) struct Holding {
    pub(super) amount: f64,
    pub(super) spot: f64,
}

/// Sample `weeks` prices evenly across `closes` (linear index mapping as an
/// efficient proxy for weekly closes). `None` when there are no closes.
pub(super) fn sample_weekly(closes: &[f64], weeks: usize) -> Option<Vec<f64>> {
    let last = closes.len().checked_sub(1)?;
    let steps = weeks.saturating_sub(1).max(1) as f64;
    Some(
        (0..weeks)
            .map(|w| closes[(((w as f64 / steps) * last as f64).round() as usize).min(last)])
            .collect(),
    )
}

/// Weekly portfolio value: cash plus each holding's sampled price times its
/// amount. `histories` lines up with `holdings`; a holding without usable
/// history is valued at its current spot price for every week.
pub(super) fn weekly_values(
    cash: f64,
    holdings: &[Holding],
    histories: &[Option<Vec<f64>>],
    weeks: usize,
) -> Vec<f64> {
    let mut totals = vec![cash; weeks];
    for (holding, history) in holdings.iter().zip(histories) {
        let prices = history
            .as_deref()
            .and_then(|closes| sample_weekly(closes, weeks))
            .unwrap_or_else(|| vec![holding.spot; weeks]);
        for (total, price) in totals.iter_mut().zip(prices) {
            *total += price * holding.amount;
        }
    }
    totals
}

/// Weekly portfolio value since the first dated purchase, up to `now`: one
/// history fetch per ticker across the full range, sampled weekly. The last
/// point is the current live total.
pub(super) async fn compute_weekly_series_batch<L: LiveData>(
    portfolio: &Portfolio,
    live: &L,
    now: DateTime<Utc>,
) -> Vec<(f64, f64)> {
    let earliest = portfolio
        .positions
        .iter()
        .flat_map(|position| position.get_purchases())
        .filter_map(|purchase| parse_purchase_date(purchase.date.as_deref()?))
        .min();
    let Some(earliest) = earliest else {
        return vec![];
    };
    // A purchase dated in the future counts as this week.
    let elapsed_days = (now - earliest).num_days().max(0);
    let total_weeks = ((elapsed_days / 7) as usize + 1).min(MAX_CHART_WEEKS);

    let mut cash_sum = 0.0_f64;
    let mut tickers: Vec<&str> = Vec::new();
    let mut holdings: Vec<Holding> = Vec::new();
    for position in &portfolio.positions {
        if let Some(ticker) = position.get_ticker() {
            tickers.push(ticker);
            holdings.push(Holding {
                amount: position.get_amount(),
                spot: position.market_price(),
            });
        } else {
            cash_sum += position.get_amount();
        }
    }

    // Offline and fixture quote modes never reach Yahoo, not even for charts.
    let histories: Vec<Option<Vec<f64>>> = if live.fetches_history() {
        join_all(
            tickers
                .iter()
                .map(|ticker| live.history(ticker, earliest, now)),
        )
        .await
    } else {
        vec![None; tickers.len()]
    };

    // `total_weeks` is at least 1, so the series is never empty.
    let mut series: Vec<(f64, f64)> = weekly_values(cash_sum, &holdings, &histories, total_weeks)
        .into_iter()
        .enumerate()
        .map(|(w, total)| (w as f64, total)) // weeks since start
        .collect();

    // Ensure the final point equals the current live total
    let current_value = portfolio.get_total_value();
    if let Some(last) = series.last_mut().filter(|_| current_value > 0.0) {
        last.1 = current_value;
    }

    series
}

/// Chart history comes straight from Yahoo; only the default quote mode may fetch it.
pub(super) fn fetches_yahoo_history(quotes: &AnyProvider) -> bool {
    matches!(quotes, AnyProvider::Yahoo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_weekly_spreads_over_history() {
        assert_eq!(sample_weekly(&[], 3), None);
        assert_eq!(sample_weekly(&[5.0], 3), Some(vec![5.0, 5.0, 5.0]));
        assert_eq!(
            sample_weekly(&[1.0, 2.0, 3.0, 4.0, 5.0], 3),
            Some(vec![1.0, 3.0, 5.0])
        );
    }

    #[test]
    fn test_weekly_values_keeps_holdings_aligned_with_histories() {
        let holdings = [
            Holding {
                amount: 2.0,
                spot: 10.0,
            },
            Holding {
                amount: 3.0,
                spot: 99.0,
            },
        ];
        // The first ticker has no history (or no quotes): it must fall back to
        // its own spot price without shifting the second ticker's prices.
        for missing in [None, Some(vec![])] {
            let histories = [missing, Some(vec![1.0, 2.0])];
            assert_eq!(
                weekly_values(100.0, &holdings, &histories, 2),
                vec![100.0 + 2.0 * 10.0 + 3.0, 100.0 + 2.0 * 10.0 + 6.0]
            );
        }
    }

    #[test]
    fn test_chart_history_respects_quote_mode() {
        assert!(fetches_yahoo_history(&AnyProvider::Yahoo));
        assert!(!fetches_yahoo_history(&AnyProvider::Offline));
        assert!(!fetches_yahoo_history(
            &AnyProvider::Fixture(Box::default())
        ));
    }
}

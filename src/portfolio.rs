use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::prelude::*;
use chrono::Duration;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, CellAlignment, Color as TColor, ContentArrangement, Table};
use eyre::{bail, Result};
use futures::future::join_all;
use piechart::{Chart, Color};

use crate::money::{dec, div, percent_change, percent_of, to_f64, Decimal};
use crate::position::{PortfolioPosition, PriceStatus};
use crate::services::quotes::{AnyProvider, QuoteError, QuoteProvider};

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub positions: Vec<PortfolioPosition>,
    /// Quotes came from a fixture provider (`networkStatus: "fixture"`).
    pub quotes_fixture: bool,
}

impl Default for Portfolio {
    fn default() -> Self {
        Self::new()
    }
}

impl Portfolio {
    pub fn new() -> Portfolio {
        Portfolio {
            positions: Vec::new(),
            quotes_fixture: false,
        }
    }

    pub fn add_position(&mut self, position: PortfolioPosition) {
        self.positions.push(position);
    }

    pub fn sort_positions_by_value_desc(&mut self) {
        self.positions.sort_by(|a, b| {
            b.get_balance()
                .partial_cmp(&a.get_balance())
                .unwrap_or(Ordering::Equal)
        });
    }

    pub fn get_total_value(&self) -> f64 {
        to_f64(self.total_value_dec())
    }

    /// [`Self::get_total_value`] as a [`Decimal`].
    pub fn total_value_dec(&self) -> Decimal {
        self.positions
            .iter()
            .map(PortfolioPosition::balance_dec)
            .sum()
    }

    /// Positions that count towards market (period) returns: ticker holdings
    /// with a current market price. Cash and ticker-less holdings are excluded.
    fn market_priced_securities(&self) -> impl Iterator<Item = &PortfolioPosition> {
        self.positions
            .iter()
            .filter(|position| position.get_ticker().is_some() && position.has_market_price())
    }

    /// Current base-currency value of [`Self::market_priced_securities`].
    pub fn current_securities_value(&self) -> f64 {
        to_f64(
            self.market_priced_securities()
                .map(PortfolioPosition::balance_dec)
                .sum(),
        )
    }

    /// `(invested, unrealized PnL)` from [`PortfolioPosition::total_invested`]
    /// and [`PortfolioPosition::pnl`], summed only over non-cash positions that
    /// have a market price and a cost basis.
    ///
    /// Cash is neither a gain nor a loss. Unpriced holdings are excluded so a
    /// missing quote is not treated as a full loss against cost basis.
    pub fn cost_basis_totals(&self) -> (f64, f64) {
        let (invested, pnl) = self.cost_basis_totals_dec();
        (to_f64(invested), to_f64(pnl))
    }

    /// [`Self::cost_basis_totals`] as [`Decimal`]s.
    ///
    /// Only securities with both a market price and a complete cost basis
    /// count, on both sides; see [`Self::positions_without_cost_basis`].
    pub fn cost_basis_totals_dec(&self) -> (Decimal, Decimal) {
        let mut invested = Decimal::ZERO;
        let mut pnl = Decimal::ZERO;
        for position in &self.positions {
            if position.is_cash() || !position.has_market_price() {
                continue;
            }
            if let (Some(inv), Some(p)) = (position.total_invested_dec(), position.pnl_dec()) {
                invested += inv;
                pnl += p;
            }
        }
        (invested, pnl)
    }

    /// Priced securities left out of invested/PnL totals because their cost
    /// basis is unknown (no purchases, or a buy without a price).
    pub fn positions_without_cost_basis(&self) -> Vec<&PortfolioPosition> {
        self.positions
            .iter()
            .filter(|p| !p.is_cash() && p.has_market_price() && p.total_invested_dec().is_none())
            .filter(|p| p.get_amount() > 0.0)
            .collect()
    }

    /// Total, cash, securities and previous-close value, in one pass.
    pub fn value_breakdown(&self) -> ValueBreakdown {
        let mut breakdown = ValueBreakdown::default();
        for position in &self.positions {
            let value = position.balance_dec();
            breakdown.total += value;
            if position.is_cash() {
                breakdown.cash += value;
            } else {
                breakdown.securities += value;
            }
            breakdown.previous_day += position.previous_day_value_dec();
        }
        breakdown
    }

    /// Unrealized PnL over invested in percent (cash excluded), 0 without cost basis.
    pub fn hist_percent(&self) -> f64 {
        let (invested, pnl) = self.cost_basis_totals_dec();
        percent_of(pnl, invested).map_or(0.0, to_f64)
    }

    /// `(name, %day, day pnl)` of every position with a known daily change.
    pub fn day_movers(&self) -> Vec<Mover<String>> {
        self.positions
            .iter()
            .filter_map(|position| {
                let pct = position.daily_variation_percent()?;
                let pnl = position.day_pnl_dec()?;
                Some((position.get_name().to_string(), pct, to_f64(pnl)))
            })
            .collect()
    }

    /// Value of the currently held, priced ticker positions at `date`, at
    /// current quantities and the current FX rate.
    pub async fn get_historic_securities_value(&self, date: DateTime<Utc>) -> Result<f64> {
        Ok(self.historic_and_current_securities_value(date).await?.0)
    }

    /// [`Self::get_historic_securities_value`] with an explicit [`QuoteProvider`].
    pub async fn get_historic_securities_value_with<P: QuoteProvider>(
        &self,
        provider: &P,
        date: DateTime<Utc>,
    ) -> Result<f64> {
        Ok(self
            .historic_and_current_securities_value_with(provider, date)
            .await?
            .0)
    }

    /// `(historic, current)` value of the same set of positions: only
    /// positions that have both a current market price and a historic close
    /// at `date` are included on either side, so the two are like for like.
    pub async fn historic_and_current_securities_value(
        &self,
        date: DateTime<Utc>,
    ) -> Result<(f64, f64)> {
        self.historic_and_current_securities_value_with(AnyProvider::configured(), date)
            .await
    }

    /// [`Self::historic_and_current_securities_value`] with an explicit [`QuoteProvider`].
    pub async fn historic_and_current_securities_value_with<P: QuoteProvider>(
        &self,
        provider: &P,
        date: DateTime<Utc>,
    ) -> Result<(f64, f64)> {
        let positions: Vec<&PortfolioPosition> = self.market_priced_securities().collect();
        let tasks = positions
            .iter()
            .filter_map(|position| position.get_ticker())
            .map(|ticker| provider.close_before(ticker, date));
        let results = join_all(tasks).await;

        let mut errors = Vec::new();
        let mut closes = Vec::with_capacity(positions.len());
        for (position, close) in positions.iter().zip(results) {
            let label = position.get_ticker().unwrap_or(position.get_name());
            let close = match close {
                Ok(close) => Some(close),
                Err(QuoteError::Unavailable(_)) => None,
                Err(e) => {
                    let err_str = format!("{e}");
                    // Typically: not listed yet at `date`.
                    if !err_str.contains("Bad Request") {
                        errors.push(format!(
                            "Error getting historic price data for {label}: {err_str}"
                        ));
                    }
                    None
                }
            };
            closes.push(close);
        }

        let (historic, current) = like_for_like_values(positions.into_iter().zip(closes));
        if historic <= 0.0 && !errors.is_empty() {
            bail!(
                "failed to get historic securities value: {}",
                errors.join("; ")
            );
        }
        Ok((historic, current))
    }

    pub fn positions_without_price(&self) -> Vec<&PortfolioPosition> {
        self.positions
            .iter()
            .filter(|position| !position.has_market_price())
            .collect()
    }

    pub fn get_allocation(&self) -> HashMap<String, f64> {
        let total_value = self.total_value_dec();
        if total_value.is_zero() {
            return HashMap::new();
        }

        // Sum balances per class first, then take one percentage per class.
        let mut balances: HashMap<String, Decimal> = HashMap::new();
        for position in &self.positions {
            if !position.has_market_price() {
                continue;
            }
            *balances
                .entry(position.get_asset_class().to_string())
                .or_default() += position.balance_dec();
        }
        balances
            .into_iter()
            .map(|(class, balance)| {
                let percentage =
                    div(balance * Decimal::ONE_HUNDRED, total_value).unwrap_or(Decimal::ZERO);
                (class, to_f64(percentage))
            })
            .collect()
    }

    // Print the portfolio as a table
    pub fn print(&self, include_sum: bool) {
        print!("{}", self.render(include_sum));
    }

    /// The holdings table (and summary when `include_sum`) as printed by [`Self::print`].
    pub fn render(&self, include_sum: bool) -> String {
        let mut out = String::new();
        let mut table = styled_table(
            TABLE_WIDTH,
            &[
                "Name", "Class", "Quantity", "Avg Cost", "Invested", "Price", "Value", "PnL",
                "%Hist", "%Day",
            ],
        );

        for position in &self.positions {
            let is_cash = position.is_cash();
            // Cash has no cost basis, PnL or returns.
            let unless_cash = |v: Option<f64>| v.filter(|_| !is_cash);
            let money_or_dash =
                |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));

            let price = position.market_price();
            // Show the quote currency whenever it is converted (or cannot be).
            let price_ccy = match position.price_status() {
                PriceStatus::FxUnavailable => position.price_currency(),
                _ if position.fx_rate().is_some() => position.price_currency(),
                _ => None,
            };
            let price_str = if is_cash {
                "-".to_string()
            } else if position.price_status() == PriceStatus::FxUnavailable && price > 0.0 {
                format!("{price:.2} {}", price_ccy.unwrap_or_default())
            } else if position.get_ticker().is_none() {
                // Manual holding: valued by hand, no market price.
                "-".to_string()
            } else if !position.has_market_price() {
                "n/a".to_string()
            } else if let Some(ccy) = price_ccy {
                format!("{price:.2} {ccy}")
            } else {
                format!("{price:.2}")
            };
            let value_str = if position.has_market_price() {
                format!("{:.2}", position.market_value())
            } else {
                "n/a".to_string()
            };

            let asset_class = position.get_asset_class();
            table.add_row(vec![
                Cell::new(position.get_name()),
                Cell::new(asset_class).fg(asset_class_color(asset_class)),
                // Quantity, not the data-format `Amount`: a manual item is one item.
                right(format!("{:.4}", to_f64(position.quantity_dec()))),
                right(money_or_dash(unless_cash(position.average_cost()))),
                right(money_or_dash(unless_cash(position.total_invested()))),
                right(price_str),
                right(value_str),
                signed_money_or_dash(unless_cash(position.pnl())),
                signed_pct_or_dash(unless_cash(position.historic_variation_percent())),
                signed_pct_or_dash(unless_cash(position.daily_variation_percent())),
            ]);
        }

        if include_sum {
            let breakdown = self.value_breakdown();
            // Broker-style overview summary above balances
            let _ = writeln!(out, "{}", self.overview_table(&breakdown));

            // Cash is excluded: a cash balance is neither a gain nor a loss.
            let (total_invested, total_pnl) = self.cost_basis_totals();
            let bold = |cell: Cell| cell.add_attribute(Attribute::Bold);
            table.add_row(vec![
                bold(Cell::new("TOTAL")),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                bold(right(format!("{total_invested:.2}"))),
                right(""),
                bold(right(format!("{:.2}", to_f64(breakdown.total)))),
                bold(signed_money(total_pnl)),
                bold(signed_pct(self.hist_percent())),
                bold(signed_pct(breakdown.day_percent())),
            ]);
        }

        let _ = writeln!(out, "{table}");

        let (no_fx, no_price): (Vec<_>, Vec<_>) = self
            .positions_without_price()
            .into_iter()
            .partition(|position| position.price_status() == PriceStatus::FxUnavailable);
        let mut warn = |positions: &[&PortfolioPosition], what: &str| {
            if positions.is_empty() {
                return;
            }
            let labels = positions
                .iter()
                .map(|position| position.get_ticker().unwrap_or_else(|| position.get_name()))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "Warning: {} position(s) {what}: {labels}",
                positions.len()
            );
        };
        warn(&no_price, "without market price excluded from totals");
        warn(
            &self.positions_without_cost_basis(),
            "without complete cost basis excluded from invested/PnL",
        );
        warn(
            &no_fx,
            "without FX rate to the base currency excluded from totals",
        );
        out
    }

    /// The broker-style summary shared by [`Self::render`] and
    /// [`Self::render_performance_with`].
    fn overview_table(&self, breakdown: &ValueBreakdown) -> Table {
        // Inception invested and PnL. Cash is neither a gain nor a loss:
        // do not subtract invested from total value (which includes cash).
        let (total_invested, unrealized_pnl) = self.cost_basis_totals();
        let mut overview = styled_table(
            TABLE_WIDTH,
            &[
                "Total",
                "Securities",
                "Cash",
                "Invested",
                "Unreal. PnL",
                "%PnL",
                "Day PnL",
                "%Day",
            ],
        );
        overview.add_row(vec![
            right(format!("{:.2}", to_f64(breakdown.total))),
            right(format!("{:.2}", to_f64(breakdown.securities))),
            right(format!("{:.2}", to_f64(breakdown.cash))),
            right(if total_invested > 0.0 {
                format!("{total_invested:.2}")
            } else {
                "-".to_string()
            }),
            signed_money(unrealized_pnl),
            signed_pct(self.hist_percent()),
            signed_money(breakdown.day_pnl()),
            // 1D: whole priced portfolio (incl. cash) vs. its previous-close value;
            // both sides contain the same positions, cash at an unchanged value.
            signed_pct(breakdown.day_percent()),
        ]);
        overview
    }

    // Print the allocation in descending order %-wise
    pub fn print_allocation(&self) {
        print!("{}", self.render_allocation());
    }

    /// The allocation list as printed by [`Self::print_allocation`].
    pub fn render_allocation(&self) -> String {
        let mut out = String::new();
        let allocation = self.get_allocation();

        // create a vector and sort it by the %-value of the allocation in descending order
        let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
        allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(Ordering::Equal));

        let _ = writeln!(out, "====================================");
        for (asset_class, percentage) in allocation_vec {
            let _ = writeln!(out, "{asset_class: >12} | {percentage: >10.2}");
        }
        out
    }

    pub fn draw_pie_chart(&self) {
        let colors = [
            Color::Red,
            Color::Green,
            Color::Blue,
            Color::Yellow,
            Color::Cyan,
            Color::White,
            Color::Purple,
            Color::Black,
        ];
        let data: Vec<_> = self
            .positions
            .iter()
            .zip(colors.into_iter().cycle())
            .map(|(position, color)| piechart::Data {
                label: position.get_name().to_string(),
                value: position.get_balance() as f32,
                color: Some(color.into()),
                fill: '•',
            })
            .collect();

        Chart::new()
            .legend(true)
            .radius(9)
            .aspect_ratio(3)
            .draw(&data);
    }

    pub async fn print_performance(&self) {
        let db = match sled::open("database") {
            Ok(db) => db,
            Err(e) => {
                eprintln!("Warning: failed to open performance database: {e}");
                return;
            }
        };
        // Last stored total balance, 0.0 when none.
        let last_db: f64 = match &db.iter().last() {
            Some(Ok(last)) => String::from_utf8_lossy(&last.1).parse().unwrap_or(0.0),
            _ => 0.0,
        };
        print!(
            "{}",
            self.render_performance_with(AnyProvider::configured(), last_db, Utc::now())
                .await
        );
    }

    /// The performance tables printed by [`Self::print_performance`], with an
    /// explicit quote provider, last stored balance (`0.0` = none) and clock.
    pub async fn render_performance_with<P: QuoteProvider>(
        &self,
        provider: &P,
        last_db: f64,
        now: DateTime<Utc>,
    ) -> String {
        let mut out = String::new();
        let breakdown = self.value_breakdown();

        // Period returns (1W, 1M, 3M, 6M, YTD, 1Y): market return of the
        // currently held, priced ticker positions at current quantities and
        // FX. Cash and ticker-less holdings are excluded on both sides, and
        // deposits, withdrawals and trades during the period are not
        // accounted for. Jan 1 is never ambiguous.
        let start_year = Utc.with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0).unwrap();
        let starts = [
            now - Duration::days(7),
            now - Duration::days(30),
            now - Duration::days(90),
            now - Duration::days(182),
            start_year,
            now - Duration::days(365),
        ];
        let mut market_returns = Vec::with_capacity(starts.len());
        for start in starts {
            let values = self
                .historic_and_current_securities_value_with(provider, start)
                .await;
            market_returns.push(
                values
                    .ok()
                    .and_then(|(historic, current)| period_return_percent(current, historic)),
            );
        }

        // "Since": total (incl. cash) vs. the last stored total balance; both
        // are totals, but deposits and withdrawals in between are not accounted for.
        let since_last_check_percent = if last_db.is_finite() {
            percent_change(breakdown.total, dec(last_db)).map_or(0.0, to_f64)
        } else {
            0.0
        };

        let mut periods = styled_table(
            TABLE_WIDTH,
            &["1D", "1W", "1M", "3M", "6M", "YTD", "1Y", "Since"],
        );
        let mut row = vec![signed_pct(breakdown.day_percent())];
        row.extend(market_returns.into_iter().map(signed_pct_or_dash));
        row.push(signed_pct(since_last_check_percent));
        periods.add_row(row);

        // Top movers today
        let (gainers, losers) = top_movers(self.day_movers(), 3);

        let _ = writeln!(out, "{}", self.overview_table(&breakdown));
        let _ = writeln!(out, "{periods}");
        if !gainers.is_empty() {
            let _ = writeln!(out, "{}", movers_table("Top Gainers", &gainers));
        }
        if !losers.is_empty() {
            let _ = writeln!(out, "{}", movers_table("Top Losers", &losers));
        }
        out
    }
}

const TABLE_WIDTH: u16 = 120;
const MOVERS_TABLE_WIDTH: u16 = 64;

/// A table in the CLI style with a bold `header`.
fn styled_table(width: u16, header: &[&str]) -> Table {
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_width(width)
        .set_header(
            header
                .iter()
                .map(|title| Cell::new(title).add_attribute(Attribute::Bold))
                .collect::<Vec<_>>(),
        );
    table
}

/// A right-aligned cell.
fn right(text: impl ToString) -> Cell {
    Cell::new(text).set_alignment(CellAlignment::Right)
}

/// A right-aligned `text`, green for a non-negative `value`, red otherwise.
fn signed(value: f64, text: String) -> Cell {
    let color = if value >= 0.0 {
        TColor::Green
    } else {
        TColor::Red
    };
    right(text).fg(color)
}

fn signed_money(value: f64) -> Cell {
    signed(value, format!("{value:.2}"))
}

fn signed_pct(value: f64) -> Cell {
    signed(value, format!("{value:.2}%"))
}

fn signed_money_or_dash(value: Option<f64>) -> Cell {
    value.map_or_else(|| right("-"), signed_money)
}

fn signed_pct_or_dash(value: Option<f64>) -> Cell {
    value.map_or_else(|| right("-"), signed_pct)
}

fn asset_class_color(asset_class: &str) -> TColor {
    match asset_class.to_lowercase().as_str() {
        "crypto" => TColor::DarkYellow,
        "stocks" | "stock" => TColor::DarkBlue,
        "bonds" | "bond" => TColor::DarkRed,
        "commodities" | "commodity" => TColor::DarkMagenta,
        "gold" => TColor::Yellow,
        "cash" => TColor::DarkGreen,
        _ => TColor::White,
    }
}

/// `title`, `%Day` and `Day PnL` of the top movers.
fn movers_table(title: &str, movers: &[Mover<String>]) -> Table {
    let mut table = styled_table(MOVERS_TABLE_WIDTH, &[title, "%Day", "Day PnL"]);
    for (name, pct, pnl) in movers {
        table.add_row(vec![Cell::new(name), signed_pct(*pct), signed_money(*pnl)]);
    }
    table
}

/// Sum `(historic, current)` base-currency values over positions paired with
/// their historic close (quote currency). Positions without a positive close
/// are left out of BOTH sums. Historic closes use current quantity and the
/// current FX rate.
fn like_for_like_values<'a>(
    pairs: impl IntoIterator<Item = (&'a PortfolioPosition, Option<f64>)>,
) -> (f64, f64) {
    let mut historic = Decimal::ZERO;
    let mut current = Decimal::ZERO;
    for (position, close) in pairs {
        let Some(close) = close.filter(|c| *c > 0.0) else {
            continue;
        };
        historic += dec(close) * position.quantity_dec() * position.fx_factor_dec();
        current += position.balance_dec();
    }
    (to_f64(historic), to_f64(current))
}

/// Base-currency totals of a portfolio, see [`Portfolio::value_breakdown`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ValueBreakdown {
    pub total: Decimal,
    pub cash: Decimal,
    pub securities: Decimal,
    /// Total at the previous close (unchanged value where no daily change is known).
    pub previous_day: Decimal,
}

impl ValueBreakdown {
    /// Change of the total since the previous close.
    pub fn day_pnl(&self) -> f64 {
        to_f64(self.total - self.previous_day)
    }

    /// [`Self::day_pnl`] in percent of the previous-close total, 0 when that is not positive.
    pub fn day_percent(&self) -> f64 {
        percent_change(self.total, self.previous_day).map_or(0.0, to_f64)
    }
}

/// `(name, %day, day pnl)` of one position.
pub type Mover<T> = (T, f64, f64);

/// Split `(name, %day, day pnl)` movers into the top `n` gainers (largest
/// positive %day first) and top `n` losers (most negative first). A position
/// is in at most one list; an unchanged one (0%) is in neither.
pub fn top_movers<T: Clone>(movers: Vec<Mover<T>>, n: usize) -> (Vec<Mover<T>>, Vec<Mover<T>>) {
    let mut gainers: Vec<_> = movers.iter().filter(|m| m.1 > 0.0).cloned().collect();
    gainers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    gainers.truncate(n);
    let mut losers: Vec<_> = movers.into_iter().filter(|m| m.1 < 0.0).collect();
    losers.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
    losers.truncate(n);
    (gainers, losers)
}

/// Percentage return from `historic` to `current`; `None` if `historic <= 0`.
pub fn period_return_percent(current: f64, historic: f64) -> Option<f64> {
    if historic.is_finite() && current.is_finite() {
        percent_change(dec(current), dec(historic)).map(to_f64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::{PortfolioPosition, PriceStatus, Purchase};

    fn with_purchase(
        mut position: PortfolioPosition,
        quantity: f64,
        price: f64,
    ) -> PortfolioPosition {
        position.purchases.push(Purchase {
            date: None,
            quantity: dec(quantity),
            price: Some(dec(price)),
            fees: None,
            estimated_price: None,
        });
        position
    }

    #[test]
    fn unavailable_position_excluded_from_totals_and_allocation() {
        let unavailable = PortfolioPosition::for_test(
            Some("SPY"),
            "Stocks",
            10.0,
            999.0,
            PriceStatus::Unavailable,
        );
        let bond = PortfolioPosition::for_test(Some("TLT"), "Bonds", 2.0, 50.0, PriceStatus::Live);
        let cash = PortfolioPosition::for_test(None, "Cash", 25.0, 0.0, PriceStatus::Live);

        assert_eq!(unavailable.get_balance(), 0.0);

        let mut portfolio = Portfolio::new();
        portfolio.add_position(unavailable);
        portfolio.add_position(bond);
        portfolio.add_position(cash);

        assert_eq!(portfolio.get_total_value(), 125.0);

        let missing = portfolio.positions_without_price();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].get_ticker(), Some("SPY"));

        let allocation = portfolio.get_allocation();
        assert!(!allocation.contains_key("Stocks"));
        assert!((allocation["Bonds"] - 80.0).abs() < 1e-9);
        assert!((allocation["Cash"] - 20.0).abs() < 1e-9);

        let mut only_missing = Portfolio::new();
        only_missing.add_position(PortfolioPosition::for_test(
            Some("BTC-USD"),
            "Crypto",
            1.0,
            50_000.0,
            PriceStatus::Unavailable,
        ));
        assert_eq!(only_missing.get_total_value(), 0.0);
        assert!(only_missing.get_allocation().is_empty());
    }

    #[test]
    fn period_return_compares_like_with_like() {
        // Fake data: 1000 cash + 10 units of a ticker now at 110 (was 100).
        let security =
            PortfolioPosition::for_test(Some("FAKE"), "Stocks", 10.0, 110.0, PriceStatus::Live);
        let cash = PortfolioPosition::for_test(None, "Cash", 1000.0, 0.0, PriceStatus::Live);
        let unpriced =
            PortfolioPosition::for_test(Some("GONE"), "Stocks", 5.0, 0.0, PriceStatus::Unavailable);
        let unlisted =
            PortfolioPosition::for_test(Some("NEW"), "Stocks", 1.0, 500.0, PriceStatus::Live);
        let mut portfolio = Portfolio::new();
        for p in [security, cash, unpriced, unlisted] {
            portfolio.add_position(p);
        }

        // Cash and unpriced positions are not market-return candidates.
        let candidates: Vec<_> = portfolio
            .market_priced_securities()
            .map(|p| p.get_ticker().unwrap())
            .collect();
        assert_eq!(candidates, vec!["FAKE", "NEW"]);
        assert_eq!(portfolio.current_securities_value(), 1600.0);

        // NEW has no historic close at the start date: excluded on both sides.
        let positions: Vec<_> = portfolio.market_priced_securities().collect();
        let (historic, current) =
            like_for_like_values(positions.into_iter().zip([Some(100.0), None]));
        assert_eq!((historic, current), (1000.0, 1100.0));
        assert_eq!(period_return_percent(current, historic), Some(10.0));

        // The old basis (total incl. cash vs. securities-only historic) was wrong.
        assert_ne!(
            period_return_percent(portfolio.get_total_value(), historic),
            Some(10.0)
        );
    }

    #[test]
    fn period_return_handles_fx_and_empty_history() {
        let mut security =
            PortfolioPosition::for_test(Some("FAKEUS"), "Stocks", 2.0, 120.0, PriceStatus::Live);
        security.fx_rate = Some(dec(0.5));
        let (historic, current) = like_for_like_values([(&security, Some(100.0))]);
        assert_eq!((historic, current), (100.0, 120.0));
        assert_eq!(period_return_percent(current, historic), Some(20.0));

        assert_eq!(period_return_percent(100.0, 0.0), None);
        assert_eq!(like_for_like_values([(&security, Some(0.0))]), (0.0, 0.0));
    }

    #[test]
    fn cash_is_neither_gain_nor_loss() {
        // Winner: 10 @ 100, now 110. Invested 1000, PnL +100.
        let winner = with_purchase(
            PortfolioPosition::for_test(Some("UP"), "Stocks", 10.0, 110.0, PriceStatus::Live),
            10.0,
            100.0,
        );
        // Loser: 4 @ 80, now 50. Invested 320, PnL -120.
        let loser = with_purchase(
            PortfolioPosition::for_test(Some("DOWN"), "Stocks", 4.0, 50.0, PriceStatus::Live),
            4.0,
            80.0,
        );
        // Large cash balance. The old total-value-minus-invested basis counted this as profit.
        let cash = PortfolioPosition::for_test(None, "Cash", 5_000.0, 0.0, PriceStatus::Live);
        // Cash with a cost basis above its balance would be a loss if purchases were included.
        let cash_with_basis = with_purchase(
            PortfolioPosition::for_test(None, "CASH", 1_000.0, 0.0, PriceStatus::Live),
            1.0,
            1_500.0,
        );
        // Unpriced cost basis must not be booked as a total loss.
        let unpriced = with_purchase(
            PortfolioPosition::for_test(Some("GONE"), "Stocks", 5.0, 0.0, PriceStatus::Unavailable),
            5.0,
            50.0,
        );

        assert!(cash.is_cash());
        assert!(cash_with_basis.is_cash());
        assert!(!winner.is_cash());

        let mut portfolio = Portfolio::new();
        for position in [winner, loser, cash, cash_with_basis, unpriced] {
            portfolio.add_position(position);
        }

        let (invested, pnl) = portfolio.cost_basis_totals();
        assert_eq!(invested, 1_320.0);
        assert_eq!(pnl, -20.0);
        let pct = if invested > 0.0 {
            pnl / invested * 100.0
        } else {
            0.0
        };
        assert!((pct - (-20.0 / 1_320.0 * 100.0)).abs() < 1e-9);

        // Old basis: total value (incl. cash) minus invested treated cash as profit
        // and would have hidden the securities loss.
        let old_pnl = portfolio.get_total_value() - invested;
        assert!(old_pnl > 0.0);
        assert!(pnl < 0.0);

        let mut cash_only = Portfolio::new();
        cash_only.add_position(PortfolioPosition::for_test(
            None,
            "cash",
            250.0,
            0.0,
            PriceStatus::Live,
        ));
        assert_eq!(cash_only.cost_basis_totals(), (0.0, 0.0));
    }

    #[test]
    fn top_movers_lists_each_position_once_and_skips_unchanged() {
        let movers = vec![
            ("up", 2.0, 20.0),
            ("flat", 0.0, 0.0),
            ("down", -1.0, -5.0),
            ("up2", 5.0, 1.0),
        ];
        let (gainers, losers) = top_movers(movers, 3);
        fn names(v: &[(&'static str, f64, f64)]) -> Vec<&'static str> {
            v.iter().map(|m| m.0).collect()
        }
        assert_eq!(names(&gainers), ["up2", "up"]);
        assert_eq!(names(&losers), ["down"]);
        let (gainers, losers) = top_movers(vec![("flat", 0.0, 0.0)], 3);
        assert!(gainers.is_empty() && losers.is_empty());
    }

    /// Fake holdings: a winner, a loser, cash and one position without a quote.
    fn snapshot_portfolio() -> Portfolio {
        let mut up = with_purchase(
            PortfolioPosition::for_test(Some("FAKEUP"), "Stocks", 10.0, 110.0, PriceStatus::Live),
            10.0,
            100.0,
        );
        up.previous_close = Some(dec(105.0));
        let mut down = with_purchase(
            PortfolioPosition::for_test(Some("FAKEDN"), "Bonds", 5.0, 180.0, PriceStatus::Live),
            5.0,
            200.0,
        );
        down.previous_close = Some(dec(190.0));
        let mut portfolio = Portfolio::new();
        portfolio.add_position(up);
        portfolio.add_position(down);
        portfolio.add_position(PortfolioPosition::for_test(
            None,
            "Cash",
            500.0,
            0.0,
            PriceStatus::Live,
        ));
        portfolio.add_position(PortfolioPosition::for_test(
            Some("FAKEGONE"),
            "Stocks",
            3.0,
            0.0,
            PriceStatus::Unavailable,
        ));
        portfolio.sort_positions_by_value_desc();
        portfolio
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Compare with `tests/snapshots/<name>.snap`; `UPDATE_SNAPSHOTS=1` rewrites it.
    fn assert_snapshot(name: &str, actual: &str) {
        let actual = strip_ansi(actual);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots")
            .join(format!("{name}.snap"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", path.display()));
        assert_eq!(
            actual, expected,
            "snapshot {name} differs; UPDATE_SNAPSHOTS=1 to accept"
        );
    }

    #[test]
    fn snapshot_holdings_table() {
        assert_snapshot("holdings", &snapshot_portfolio().render(true));
    }

    #[test]
    fn snapshot_allocation() {
        assert_snapshot("allocation", &snapshot_portfolio().render_allocation());
    }

    #[tokio::test]
    async fn snapshot_performance() {
        let provider = crate::services::quotes::FixtureProvider {
            close_on: [("FAKEUP".to_string(), 100.0), ("FAKEDN".to_string(), 190.0)]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let now = Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        let rendered = snapshot_portfolio()
            .render_performance_with(&provider, 2000.0, now)
            .await;
        assert_snapshot("performance", &rendered);
    }

    #[tokio::test]
    async fn test_ytd_start_uses_last_close_of_previous_year() {
        use crate::position::PriceStatus;
        let mut portfolio = Portfolio::new();
        portfolio.add_position(PortfolioPosition::for_test(
            Some("FAKE"),
            "Stocks",
            2.0,
            30.0,
            PriceStatus::Live,
        ));
        let provider = crate::services::quotes::FixtureProvider::from_json(
            r#"{"history": {"FAKE": [{"date": "2025-12-30", "close": 18.0},
                                     {"date": "2025-12-31", "close": 20.0},
                                     {"date": "2026-01-02", "close": 25.0}]}}"#,
        )
        .unwrap();
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (historic, current) = portfolio
            .historic_and_current_securities_value_with(&provider, start)
            .await
            .unwrap();
        assert_eq!(historic, 40.0, "YTD base is Dec 31's close");
        assert_eq!(current, 60.0);
    }
}

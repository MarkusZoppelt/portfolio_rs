use std::collections::HashMap;

use crate::position::{PortfolioPosition, PriceStatus};
use crate::services::quotes::{QuoteError, QuoteProvider, YahooProvider};

use chrono::prelude::*;
use eyre::bail;
use eyre::Result;
use piechart::{Chart, Color};

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub positions: Vec<PortfolioPosition>,
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
        }
    }

    pub fn add_position(&mut self, position: PortfolioPosition) {
        self.positions.push(position);
    }

    pub fn sort_positions_by_value_desc(&mut self) {
        self.positions.sort_by(|a, b| {
            b.get_balance()
                .partial_cmp(&a.get_balance())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    pub fn get_total_value(&self) -> f64 {
        let mut sum = 0.0;

        for position in &self.positions {
            sum += position.get_balance();
        }
        sum
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
        self.market_priced_securities()
            .map(PortfolioPosition::get_balance)
            .sum()
    }

    /// `(invested, unrealized PnL)` from [`PortfolioPosition::total_invested`]
    /// and [`PortfolioPosition::pnl`], summed only over non-cash positions that
    /// have a market price and a cost basis.
    ///
    /// Cash is neither a gain nor a loss. Unpriced holdings are excluded so a
    /// missing quote is not treated as a full loss against cost basis.
    pub fn cost_basis_totals(&self) -> (f64, f64) {
        let mut invested = 0.0_f64;
        let mut pnl = 0.0_f64;
        for position in &self.positions {
            if position.is_cash() || !position.has_market_price() {
                continue;
            }
            if let Some(inv) = position.total_invested() {
                invested += inv;
            }
            if let Some(p) = position.pnl() {
                pnl += p;
            }
        }
        (invested, pnl)
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
        self.historic_and_current_securities_value_with(&YahooProvider, date)
            .await
    }

    /// [`Self::historic_and_current_securities_value`] with an explicit [`QuoteProvider`].
    pub async fn historic_and_current_securities_value_with<P: QuoteProvider>(
        &self,
        provider: &P,
        date: DateTime<Utc>,
    ) -> Result<(f64, f64)> {
        use futures::future::join_all;

        let positions: Vec<&PortfolioPosition> = self.market_priced_securities().collect();
        let tasks = positions
            .iter()
            .filter_map(|position| position.get_ticker())
            .map(|ticker| provider.close_on(ticker, date));
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
        let mut allocation: HashMap<String, f64> = HashMap::new();
        let total_value = self.get_total_value();
        if total_value == 0.0 {
            return allocation;
        }

        for position in &self.positions {
            if !position.has_market_price() {
                continue;
            }
            let asset_class = position.get_asset_class();
            let balance = position.get_balance();
            let percentage = balance / total_value * 100.0;

            if let Some(value) = allocation.get_mut(asset_class) {
                *value += percentage;
            } else {
                allocation.insert(asset_class.to_string(), percentage);
            }
        }
        allocation
    }

    // Print the portfolio as a table
    pub fn print(&self, include_sum: bool) {
        use comfy_table::{
            presets::UTF8_FULL, Attribute, Cell, CellAlignment, Color as TColor,
            ContentArrangement, Table,
        };

        let mut table = Table::new();
        table
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(120);

        table.set_header(vec![
            Cell::new("Name").add_attribute(Attribute::Bold),
            Cell::new("Class").add_attribute(Attribute::Bold),
            Cell::new("Amount").add_attribute(Attribute::Bold),
            Cell::new("Avg Cost").add_attribute(Attribute::Bold),
            Cell::new("Invested").add_attribute(Attribute::Bold),
            Cell::new("Price").add_attribute(Attribute::Bold),
            Cell::new("Value").add_attribute(Attribute::Bold),
            Cell::new("PnL").add_attribute(Attribute::Bold),
            Cell::new("%Hist").add_attribute(Attribute::Bold),
            Cell::new("%Day").add_attribute(Attribute::Bold),
        ]);

        let mut total_value = 0.0_f64;
        let mut cash_value = 0.0_f64;
        let mut securities_value = 0.0_f64;

        // Track previous total value for computing portfolio daily change
        let mut total_prev_value_for_day = 0.0_f64;
        for position in &self.positions {
            let amount = position.get_amount();
            let avg_cost = position.average_cost();
            let invested = position.total_invested();
            let price = position.market_price();
            let value = position.market_value();
            let pnl = position.pnl();
            let day_var = position.daily_variation_percent();
            let hist_var = position.historic_variation_percent();

            let is_cash = position.is_cash();

            total_value += value;

            // Estimate previous value from daily % to compute portfolio daily change
            let prev_value_for_row = match day_var {
                Some(dv) => {
                    let ratio = dv / 100.0;
                    if (1.0 + ratio).abs() > f64::EPSILON {
                        value / (1.0 + ratio)
                    } else {
                        value
                    }
                }
                None => value,
            };
            total_prev_value_for_day += prev_value_for_row;

            // Aggregate cash vs securities
            if is_cash {
                cash_value += value;
            } else {
                securities_value += value;
            }

            let avg_cost_str = if is_cash {
                "-".to_string()
            } else {
                avg_cost
                    .map(|v| format!("{v:.2}"))
                    .unwrap_or_else(|| "-".to_string())
            };
            let invested_str = if is_cash {
                "-".to_string()
            } else {
                invested
                    .map(|v| format!("{v:.2}"))
                    .unwrap_or_else(|| "-".to_string())
            };
            let asset_class = position.get_asset_class();
            let asset_color = match asset_class.to_lowercase().as_str() {
                "crypto" => TColor::DarkYellow,
                "stocks" | "stock" => TColor::DarkBlue,
                "bonds" | "bond" => TColor::DarkRed,
                "commodities" | "commodity" => TColor::DarkMagenta,
                "gold" => TColor::Yellow,
                "cash" => TColor::DarkGreen,
                _ => TColor::White,
            };

            let pnl_cell = if is_cash {
                Cell::new("-").set_alignment(CellAlignment::Right)
            } else {
                match pnl {
                    Some(v) => {
                        let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
                        Cell::new(format!("{v:.2}"))
                            .set_alignment(CellAlignment::Right)
                            .fg(c)
                    }
                    None => Cell::new("-").set_alignment(CellAlignment::Right),
                }
            };

            let day_cell = if is_cash {
                Cell::new("-").set_alignment(CellAlignment::Right)
            } else {
                match day_var {
                    Some(v) => {
                        let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
                        Cell::new(format!("{v:.2}%"))
                            .set_alignment(CellAlignment::Right)
                            .fg(c)
                    }
                    None => Cell::new("-").set_alignment(CellAlignment::Right),
                }
            };

            let hist_cell = if is_cash {
                Cell::new("-").set_alignment(CellAlignment::Right)
            } else {
                match hist_var {
                    Some(v) => {
                        let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
                        Cell::new(format!("{v:.2}%"))
                            .set_alignment(CellAlignment::Right)
                            .fg(c)
                    }
                    None => Cell::new("-").set_alignment(CellAlignment::Right),
                }
            };

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
            } else if !position.has_market_price() {
                "n/a".to_string()
            } else if let Some(ccy) = price_ccy {
                format!("{price:.2} {ccy}")
            } else {
                format!("{price:.2}")
            };
            let value_str = if position.has_market_price() {
                format!("{value:.2}")
            } else {
                "n/a".to_string()
            };

            table.add_row(vec![
                Cell::new(position.get_name()),
                Cell::new(asset_class).fg(asset_color),
                Cell::new(format!("{amount:.4}")).set_alignment(CellAlignment::Right),
                Cell::new(avg_cost_str).set_alignment(CellAlignment::Right),
                Cell::new(invested_str).set_alignment(CellAlignment::Right),
                Cell::new(price_str).set_alignment(CellAlignment::Right),
                Cell::new(value_str).set_alignment(CellAlignment::Right),
                pnl_cell,
                hist_cell,
                day_cell,
            ]);
        }

        // Cash is excluded: a cash balance is neither a gain nor a loss.
        let (total_invested, total_pnl) = self.cost_basis_totals();
        let hist_percent = if total_invested > 0.0 {
            total_pnl / total_invested * 100.0
        } else {
            0.0
        };

        // Broker-style overview summary above balances
        if include_sum {
            use comfy_table::{
                presets::UTF8_FULL, Attribute, Cell, CellAlignment, Color as TColor,
                ContentArrangement, Table,
            };
            let day_pnl_abs = total_value - total_prev_value_for_day;
            let total_day_var = if total_prev_value_for_day > 0.0 {
                (total_value - total_prev_value_for_day) / total_prev_value_for_day * 100.0
            } else {
                0.0
            };

            let mut overview = Table::new();
            overview
                .load_style(UTF8_FULL)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_width(120)
                .set_header(vec![
                    Cell::new("Total").add_attribute(Attribute::Bold),
                    Cell::new("Securities").add_attribute(Attribute::Bold),
                    Cell::new("Cash").add_attribute(Attribute::Bold),
                    Cell::new("Invested").add_attribute(Attribute::Bold),
                    Cell::new("Unreal. PnL").add_attribute(Attribute::Bold),
                    Cell::new("%Since").add_attribute(Attribute::Bold),
                    Cell::new("Day PnL").add_attribute(Attribute::Bold),
                    Cell::new("%Day").add_attribute(Attribute::Bold),
                ]);

            let colorize_pct = |v: f64| {
                let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
                Cell::new(format!("{v:.2}%"))
                    .set_alignment(CellAlignment::Right)
                    .fg(c)
            };
            let colorize_money = |v: f64| {
                let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
                Cell::new(format!("{v:.2}"))
                    .set_alignment(CellAlignment::Right)
                    .fg(c)
            };

            overview.add_row(vec![
                Cell::new(format!("{total_value:.2}")).set_alignment(CellAlignment::Right),
                Cell::new(format!("{securities_value:.2}")).set_alignment(CellAlignment::Right),
                Cell::new(format!("{cash_value:.2}")).set_alignment(CellAlignment::Right),
                Cell::new(if total_invested > 0.0 {
                    format!("{total_invested:.2}")
                } else {
                    "-".to_string()
                })
                .set_alignment(CellAlignment::Right),
                colorize_money(total_pnl),
                colorize_pct(hist_percent),
                colorize_money(day_pnl_abs),
                colorize_pct(total_day_var),
            ]);

            println!("{overview}");
        }

        if include_sum {
            let total_day_var = if total_prev_value_for_day > 0.0 {
                (total_value - total_prev_value_for_day) / total_prev_value_for_day * 100.0
            } else {
                0.0
            };
            table.add_row(vec![
                Cell::new("TOTAL").add_attribute(Attribute::Bold),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(format!("{total_invested:.2}"))
                    .set_alignment(CellAlignment::Right)
                    .add_attribute(Attribute::Bold),
                Cell::new("").set_alignment(CellAlignment::Right),
                Cell::new(format!("{total_value:.2}"))
                    .set_alignment(CellAlignment::Right)
                    .add_attribute(Attribute::Bold),
                {
                    let c = if total_pnl >= 0.0 {
                        TColor::Green
                    } else {
                        TColor::Red
                    };
                    Cell::new(format!("{total_pnl:.2}"))
                        .set_alignment(CellAlignment::Right)
                        .add_attribute(Attribute::Bold)
                        .fg(c)
                },
                {
                    let c = if hist_percent >= 0.0 {
                        TColor::Green
                    } else {
                        TColor::Red
                    };
                    Cell::new(format!("{hist_percent:.2}%"))
                        .set_alignment(CellAlignment::Right)
                        .add_attribute(Attribute::Bold)
                        .fg(c)
                },
                {
                    let c = if total_day_var >= 0.0 {
                        TColor::Green
                    } else {
                        TColor::Red
                    };
                    Cell::new(format!("{total_day_var:.2}%"))
                        .set_alignment(CellAlignment::Right)
                        .add_attribute(Attribute::Bold)
                        .fg(c)
                },
            ]);
        }

        println!("{table}");

        let (no_fx, no_price): (Vec<_>, Vec<_>) = self
            .positions_without_price()
            .into_iter()
            .partition(|position| position.price_status() == PriceStatus::FxUnavailable);
        let label = |position: &&PortfolioPosition| {
            position
                .get_ticker()
                .unwrap_or_else(|| position.get_name())
                .to_string()
        };
        if !no_price.is_empty() {
            let labels = no_price.iter().map(label).collect::<Vec<_>>().join(", ");
            println!(
                "Warning: {} position(s) without market price excluded from totals: {}",
                no_price.len(),
                labels
            );
        }
        if !no_fx.is_empty() {
            let labels = no_fx.iter().map(label).collect::<Vec<_>>().join(", ");
            println!(
                "Warning: {} position(s) without FX rate to the base currency excluded from totals: {}",
                no_fx.len(),
                labels
            );
        }
    }

    // Print the allocation in descending order %-wise
    pub fn print_allocation(&self) {
        let allocation = self.get_allocation();

        // create a vector and sort it by the %-value of the allocation in descending order
        let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
        allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

        println!("====================================");
        for (asset_class, percentage) in allocation_vec {
            println!("{asset_class: >12} | {percentage: >10.2}");
        }
    }

    pub fn draw_pie_chart(&self) {
        let mut data = vec![];

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

        for (i, position) in self.positions.iter().enumerate() {
            let name = {
                let this = &position;
                this.get_name()
            };
            let balance = position.get_balance() as f32;

            data.push(piechart::Data {
                label: name.to_string(),
                value: balance,
                color: Some(colors[i % colors.len()].into()),
                fill: '•',
            });
        }

        Chart::new()
            .legend(true)
            .radius(9)
            .aspect_ratio(3)
            .draw(&data);
    }

    fn flow_metrics_since(&self, start: DateTime<Utc>) -> (f64, f64, f64, f64) {
        use crate::position::parse_purchase_date;
        let mut invested = 0.0_f64;
        let mut current = 0.0_f64;
        for position in &self.positions {
            let price_now = position.get_balance() / position.get_amount().max(1e-12);
            for p in position.get_purchases() {
                if let Some(date_str) = &p.date {
                    if let Some(date) = parse_purchase_date(date_str) {
                        if date >= start {
                            if let Some(price) = p.price {
                                if price > 0.0 && p.quantity > 0.0 {
                                    invested += p.quantity * price + p.fees.unwrap_or(0.0);
                                    current += p.quantity * price_now;
                                }
                            }
                        }
                    }
                }
            }
        }
        let pnl = current - invested;
        let pct = if invested > 0.0 {
            (pnl / invested) * 100.0
        } else {
            0.0
        };
        (invested, current, pnl, pct)
    }

    pub async fn print_performance(&self) {
        use comfy_table::{
            presets::UTF8_FULL, Attribute, Cell, CellAlignment, Color as TColor,
            ContentArrangement, Table,
        };

        let db = match sled::open("database") {
            Ok(db) => db,
            Err(e) => {
                eprintln!("Warning: failed to open performance database: {e}");
                return;
            }
        };

        // Reference points — Jan 1 and first of month are never ambiguous
        let start_year = Utc
            .with_ymd_and_hms(Utc::now().year(), 1, 1, 0, 0, 0)
            .unwrap();
        let start_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 1, 0, 0, 0)
            .unwrap();

        let current_value = self.get_total_value();

        // Inception invested and PnL. Cash is neither a gain nor a loss:
        // do not subtract invested from total value (which includes cash).
        let (total_invested, unrealized_pnl) = self.cost_basis_totals();
        let hist_percent = if total_invested > 0.0 {
            (unrealized_pnl / total_invested) * 100.0
        } else {
            0.0
        };

        // 1D: whole priced portfolio (incl. cash) vs. its previous-close value;
        // both sides contain the same positions, cash at an unchanged value.
        let mut total_prev_value_for_day = 0.0_f64;
        for position in &self.positions {
            let value = position.get_balance();
            let prev = position.daily_variation_percent().map(|dv| {
                let ratio = dv / 100.0;
                if (1.0 + ratio).abs() > f64::EPSILON {
                    value / (1.0 + ratio)
                } else {
                    value
                }
            });
            total_prev_value_for_day += prev.unwrap_or(value);
        }
        let daily_percent = if total_prev_value_for_day > 0.0 {
            (current_value - total_prev_value_for_day) / total_prev_value_for_day * 100.0
        } else {
            0.0
        };

        // Period returns (1W..1Y, YTD): market return of the currently held,
        // priced ticker positions at current quantities and FX. Cash and
        // ticker-less holdings are excluded on both sides, and deposits,
        // withdrawals and trades during the period are not accounted for.
        let now = Utc::now();
        let start_week = now - chrono::Duration::days(7);
        let start_1m_roll = now - chrono::Duration::days(30);
        let start_3m_roll = now - chrono::Duration::days(90);
        let start_6m_roll = now - chrono::Duration::days(182);
        let start_1y_roll = now - chrono::Duration::days(365);

        let mut market_returns = Vec::new();
        for start in [
            start_week,
            start_1m_roll,
            start_3m_roll,
            start_6m_roll,
            start_year,
            start_1y_roll,
        ] {
            market_returns.push(
                match self.historic_and_current_securities_value(start).await {
                    Ok((historic, current)) => period_return_percent(current, historic),
                    Err(_) => None,
                },
            );
        }
        let [w1_market, m1_market, m3_market, m6_market, ytd_market, y1_market] =
            market_returns[..]
        else {
            unreachable!("six periods requested")
        };

        // Flow-based (our data) YTD / MTD and rolling periods
        let (_ytd_invested, _ytd_value, _ytd_pnl, _ytd_pct) = self.flow_metrics_since(start_year);
        let (_mtd_invested, _mtd_value, _mtd_pnl, _mtd_pct) = self.flow_metrics_since(start_month);
        let (_w1_invested, _w1_value, _w1_pnl, _w1_pct) = self.flow_metrics_since(start_week);
        let (_m1_invested, _m1_value, _m1_pnl, _m1_pct) = self.flow_metrics_since(start_1m_roll);
        let (_m6_invested, _m6_value, _m6_pnl, _m6_pct) = self.flow_metrics_since(start_6m_roll);
        let (_y1_invested, _y1_value, _y1_pnl, _y1_pct) = self.flow_metrics_since(start_1y_roll);

        // "Since": total (incl. cash) vs. the last stored total balance; both
        // are totals, but deposits and withdrawals in between are not accounted for.
        let last_db: f64 = match &db.iter().last() {
            Some(Ok(last)) => String::from_utf8_lossy(&last.1).parse().unwrap_or(0.0),
            _ => 0.0,
        };
        let since_last_check_percent = if last_db > 0.0 {
            (current_value - last_db) / last_db * 100.0
        } else {
            0.0
        };

        // Helpers
        let colorize_pct = |v: f64| {
            let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
            Cell::new(format!("{v:.2}%"))
                .set_alignment(CellAlignment::Right)
                .fg(c)
        };
        let colorize_money = |v: f64| {
            let c = if v >= 0.0 { TColor::Green } else { TColor::Red };
            Cell::new(format!("{v:.2}"))
                .set_alignment(CellAlignment::Right)
                .fg(c)
        };
        let pct_cell_opt = |ov: Option<f64>| match ov {
            Some(v) => colorize_pct(v),
            None => Cell::new("-").set_alignment(CellAlignment::Right),
        };

        // Compute cash vs securities
        let mut cash_value = 0.0_f64;
        let mut securities_value = 0.0_f64;
        for position in &self.positions {
            let value = position.get_balance();
            let is_cash = position.is_cash();
            if is_cash {
                cash_value += value;
            } else {
                securities_value += value;
            }
        }

        // Overview table (professional summary)
        let day_pnl_abs = current_value - total_prev_value_for_day;
        let mut overview = Table::new();
        overview
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(120)
            .set_header(vec![
                Cell::new("Total").add_attribute(Attribute::Bold),
                Cell::new("Securities").add_attribute(Attribute::Bold),
                Cell::new("Cash").add_attribute(Attribute::Bold),
                Cell::new("Invested").add_attribute(Attribute::Bold),
                Cell::new("Unreal. PnL").add_attribute(Attribute::Bold),
                Cell::new("%Since").add_attribute(Attribute::Bold),
                Cell::new("Day PnL").add_attribute(Attribute::Bold),
                Cell::new("%Day").add_attribute(Attribute::Bold),
            ]);
        overview.add_row(vec![
            Cell::new(format!("{current_value:.2}")).set_alignment(CellAlignment::Right),
            Cell::new(format!("{securities_value:.2}")).set_alignment(CellAlignment::Right),
            Cell::new(format!("{cash_value:.2}")).set_alignment(CellAlignment::Right),
            Cell::new(if total_invested > 0.0 {
                format!("{total_invested:.2}")
            } else {
                "-".to_string()
            })
            .set_alignment(CellAlignment::Right),
            colorize_money(unrealized_pnl),
            colorize_pct(hist_percent),
            colorize_money(day_pnl_abs),
            colorize_pct(daily_percent),
        ]);

        // Period returns (market-based)
        let mut periods = Table::new();
        periods
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(120)
            .set_header(vec![
                Cell::new("1D").add_attribute(Attribute::Bold),
                Cell::new("1W").add_attribute(Attribute::Bold),
                Cell::new("1M").add_attribute(Attribute::Bold),
                Cell::new("3M").add_attribute(Attribute::Bold),
                Cell::new("6M").add_attribute(Attribute::Bold),
                Cell::new("YTD").add_attribute(Attribute::Bold),
                Cell::new("1Y").add_attribute(Attribute::Bold),
                Cell::new("Since").add_attribute(Attribute::Bold),
            ]);
        periods.add_row(vec![
            colorize_pct(daily_percent),
            pct_cell_opt(w1_market),
            pct_cell_opt(m1_market),
            pct_cell_opt(m3_market),
            pct_cell_opt(m6_market),
            pct_cell_opt(ytd_market),
            pct_cell_opt(y1_market),
            colorize_pct(since_last_check_percent),
        ]);

        // Top movers today
        let mut movers: Vec<(String, f64, f64)> = Vec::new(); // name, %day, day pnl
        for position in &self.positions {
            if let Some(pct) = position.daily_variation_percent() {
                let value = position.get_balance();
                let prev = {
                    let ratio = pct / 100.0;
                    if (1.0 + ratio).abs() > f64::EPSILON {
                        value / (1.0 + ratio)
                    } else {
                        value
                    }
                };
                let day_pnl = value - prev;
                movers.push((position.get_name().to_string(), pct, day_pnl));
            }
        }
        movers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let gainers = movers.iter().take(3).cloned().collect::<Vec<_>>();
        movers.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let losers = movers.iter().take(3).cloned().collect::<Vec<_>>();

        let mut top_gainers = Table::new();
        top_gainers
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(64)
            .set_header(vec![
                Cell::new("Top Gainers").add_attribute(Attribute::Bold),
                Cell::new("%Day").add_attribute(Attribute::Bold),
                Cell::new("Day PnL").add_attribute(Attribute::Bold),
            ]);
        for (name, pct, pnl) in &gainers {
            let c = if *pct >= 0.0 {
                TColor::Green
            } else {
                TColor::Red
            };
            top_gainers.add_row(vec![
                Cell::new(name.clone()),
                Cell::new(format!("{pct:.2}%"))
                    .set_alignment(CellAlignment::Right)
                    .fg(c),
                colorize_money(*pnl),
            ]);
        }

        let mut top_losers = Table::new();
        top_losers
            .load_style(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(64)
            .set_header(vec![
                Cell::new("Top Losers").add_attribute(Attribute::Bold),
                Cell::new("%Day").add_attribute(Attribute::Bold),
                Cell::new("Day PnL").add_attribute(Attribute::Bold),
            ]);
        for (name, pct, pnl) in &losers {
            let c = if *pct >= 0.0 {
                TColor::Green
            } else {
                TColor::Red
            };
            top_losers.add_row(vec![
                Cell::new(name.clone()),
                Cell::new(format!("{pct:.2}%"))
                    .set_alignment(CellAlignment::Right)
                    .fg(c),
                colorize_money(*pnl),
            ]);
        }

        // Print sections
        println!("{overview}");
        println!("{periods}");
        if !gainers.is_empty() {
            println!("{top_gainers}");
        }
        if !losers.is_empty() {
            println!("{top_losers}");
        }
    }
}

/// Sum `(historic, current)` base-currency values over positions paired with
/// their historic close (quote currency). Positions without a positive close
/// are left out of BOTH sums. Historic closes use current quantity and the
/// current FX rate.
fn like_for_like_values<'a>(
    pairs: impl IntoIterator<Item = (&'a PortfolioPosition, Option<f64>)>,
) -> (f64, f64) {
    let mut historic = 0.0;
    let mut current = 0.0;
    for (position, close) in pairs {
        let Some(close) = close.filter(|c| *c > 0.0) else {
            continue;
        };
        historic += close * position.get_amount() * position.fx_rate().unwrap_or(1.0);
        current += position.get_balance();
    }
    (historic, current)
}

/// Percentage return from `historic` to `current`; `None` if `historic <= 0`.
pub fn period_return_percent(current: f64, historic: f64) -> Option<f64> {
    if historic > 0.0 && historic.is_finite() && current.is_finite() {
        Some((current - historic) / historic * 100.0)
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
            quantity,
            price: Some(price),
            fees: None,
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
        security.fx_rate = Some(0.5);
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
}

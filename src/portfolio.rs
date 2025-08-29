use crate::position::get_historic_price;
use crate::position::PortfolioPosition;
use chrono::prelude::*;
use piechart::{Chart, Color};
use std::collections::HashMap;

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

    // Get the total value of the portfolio at a specific date
    // TODO: this function is not working as intended and the y_response is often an error
    pub async fn get_historic_total_value(&self, date: DateTime<Utc>) -> Result<f64, String> {
        let mut sum = 0.0;
        let mut errors = Vec::new();

        use futures::future::join_all;
        let mut cash_sum = 0.0;
        let mut tasks = Vec::new();
        let mut positions_with_ticker = Vec::new();

        for position in &self.positions {
            if let Some(ticker) = position.get_ticker() {
                positions_with_ticker.push((
                    ticker,
                    position.get_amount(),
                    position
                        .get_ticker()
                        .unwrap_or(position.get_name())
                        .to_string(),
                ));
                tasks.push(get_historic_price(ticker, date));
            } else {
                cash_sum += position.get_amount();
            }
        }

        let results = join_all(tasks).await;
        for ((_, amount, label), y_response) in positions_with_ticker.into_iter().zip(results) {
            match y_response {
                Ok(response) => match response.last_quote() {
                    Ok(quote) => {
                        sum += quote.close * amount;
                    }
                    Err(e) => {
                        errors.push(format!("Error getting last quote for {label}: {e}"));
                        // tolerate partial failure; continue without this ticker
                        continue;
                    }
                },
                Err(e) => {
                    let err_str = format!("{e}");
                    if err_str.contains("Bad Request") {
                        // Silently skip bad requests to avoid log spam
                        continue;
                    }
                    errors.push(format!(
                        "Error getting historic price data for {label}: {err_str}"
                    ));
                    // tolerate partial failure; continue without this ticker
                    continue;
                }
            }
        }
        sum += cash_sum;

        // Return partial aggregates even if some tickers failed
        // Only error if nothing contributed and there were errors
        if sum <= 0.0 && !errors.is_empty() {
            return Err(errors.join("; "));
        }

        Ok(sum)
    }

    // Get the total value of all non-cash (securities) positions at a specific date
    pub async fn get_historic_securities_value(&self, date: DateTime<Utc>) -> Result<f64, String> {
        let mut sum = 0.0;
        let mut errors = Vec::new();

        use futures::future::join_all;
        let mut tasks = Vec::new();
        let mut positions_with_ticker = Vec::new();

        for position in &self.positions {
            if let Some(ticker) = position.get_ticker() {
                positions_with_ticker.push((
                    ticker,
                    position.get_amount(),
                    position
                        .get_ticker()
                        .unwrap_or(position.get_name())
                        .to_string(),
                ));
                tasks.push(get_historic_price(ticker, date));
            }
        }

        let results = join_all(tasks).await;
        for ((_, amount, label), y_response) in positions_with_ticker.into_iter().zip(results) {
            match y_response {
                Ok(response) => match response.last_quote() {
                    Ok(quote) => {
                        sum += quote.close * amount;
                    }
                    Err(e) => {
                        errors.push(format!("Error getting last quote for {label}: {e}"));
                        continue;
                    }
                },
                Err(e) => {
                    let err_str = format!("{e}");
                    if err_str.contains("Bad Request") {
                        continue;
                    }
                    errors.push(format!(
                        "Error getting historic price data for {label}: {err_str}"
                    ));
                    continue;
                }
            }
        }

        if sum <= 0.0 && !errors.is_empty() {
            return Err(errors.join("; "));
        }

        Ok(sum)
    }

    pub fn get_allocation(&self) -> HashMap<String, f64> {
        let mut allocation: HashMap<String, f64> = HashMap::new();

        for position in &self.positions {
            let asset_class = position.get_asset_class();
            let balance = position.get_balance();
            let total_value = self.get_total_value();

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
            .load_preset(UTF8_FULL)
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
        let mut total_invested = 0.0_f64;
        let mut total_pnl = 0.0_f64;
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

            // Check if this is a cash position (no ticker and cash asset class)
            let is_cash = position.get_ticker().is_none()
                && position.get_asset_class().to_lowercase() == "cash";

            total_value += value;
            if let Some(i) = invested {
                total_invested += i;
            }
            if let Some(p) = pnl {
                total_pnl += p;
            }

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

            let price_str = if is_cash {
                "-".to_string()
            } else {
                format!("{price:.2}")
            };
            let value_str = if is_cash {
                "-".to_string()
            } else {
                format!("{value:.2}")
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
            let hist_percent = if total_invested > 0.0 {
                (total_value - total_invested) / total_invested * 100.0
            } else {
                0.0
            };

            let mut overview = Table::new();
            overview
                .load_preset(UTF8_FULL)
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
            let total_hist_var = if total_invested > 0.0 {
                (total_value - total_invested) / total_invested * 100.0
            } else {
                0.0
            };
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
                    let c = if total_hist_var >= 0.0 {
                        TColor::Green
                    } else {
                        TColor::Red
                    };
                    Cell::new(format!("{total_hist_var:.2}%"))
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
    }

    // Print the allocation in descending order %-wise
    pub fn print_allocation(&self) {
        let allocation = self.get_allocation();

        // create a vector and sort it by the %-value of the allocation in descending order
        let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
        allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

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

    pub async fn get_performance_data(&self) -> Result<(f64, f64, f64), String> {
        let db = sled::open("database").map_err(|e| format!("Database error: {e}"))?;

        // Yahoo first of the year is YYYY-01-03
        let first_of_the_year = Utc
            .with_ymd_and_hms(Utc::now().year(), 1, 1, 0, 0, 0)
            .unwrap();
        let first_of_the_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 3, 0, 0, 0)
            .unwrap();

        let value_at_beginning_of_year = self.get_historic_total_value(first_of_the_year).await?;
        let value_at_beginning_of_month = self.get_historic_total_value(first_of_the_month).await?;

        let last: f64 = match &db.iter().last() {
            Some(Ok(last)) => String::from_utf8_lossy(&last.1).parse().unwrap_or(0.0),
            _ => 0.0,
        };

        let current_value = {
            let mut securities = 0.0_f64;
            for p in &self.positions {
                if p.get_ticker().is_some() {
                    securities += p.get_balance();
                }
            }
            securities
        };

        let ytd_performance =
            (last - value_at_beginning_of_year) / value_at_beginning_of_year * 100.0;
        let monthly_performance =
            (last - value_at_beginning_of_month) / value_at_beginning_of_month * 100.0;
        let recent_performance = (last - current_value) / current_value * 100.0;

        Ok((ytd_performance, monthly_performance, recent_performance))
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

        let db = sled::open("database").unwrap();

        // Reference points
        let start_year = Utc
            .with_ymd_and_hms(Utc::now().year(), 1, 1, 0, 0, 0)
            .unwrap();
        let start_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 1, 0, 0, 0)
            .unwrap();

        let current_value = self.get_total_value();

        // Inception invested and PnL
        let mut total_invested = 0.0_f64;
        for p in &self.positions {
            if let Some(inv) = p.total_invested() {
                total_invested += inv;
            }
        }
        let unrealized_pnl = current_value - total_invested;
        let hist_percent = if total_invested > 0.0 {
            (unrealized_pnl / total_invested) * 100.0
        } else {
            0.0
        };

        // Daily aggregated change using previous close (exclude cash)
        let mut total_prev_value_for_day = 0.0_f64;
        for position in &self.positions {
            if position.get_ticker().is_none() {
                continue;
            }
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

        // Market-based returns computed on securities only (exclude cash)
        let ytd_market = match self.get_historic_securities_value(start_year).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let _mtd_market = match self.get_historic_securities_value(start_month).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let now = Utc::now();
        let start_week = now - chrono::Duration::days(7);
        let start_1m_roll = now - chrono::Duration::days(30);
        let start_3m_roll = now - chrono::Duration::days(90);
        let start_6m_roll = now - chrono::Duration::days(182);
        let start_1y_roll = now - chrono::Duration::days(365);

        let w1_market = match self.get_historic_securities_value(start_week).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let m1_market = match self.get_historic_securities_value(start_1m_roll).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let m3_market = match self.get_historic_securities_value(start_3m_roll).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let m6_market = match self.get_historic_securities_value(start_6m_roll).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };
        let y1_market = match self.get_historic_securities_value(start_1y_roll).await {
            Ok(v) if v > 0.0 => Some((current_value - v) / v * 100.0),
            _ => None,
        };

        // Flow-based (our data) YTD / MTD and rolling periods
        let (_ytd_invested, _ytd_value, _ytd_pnl, _ytd_pct) = self.flow_metrics_since(start_year);
        let (_mtd_invested, _mtd_value, _mtd_pnl, _mtd_pct) = self.flow_metrics_since(start_month);
        let (_w1_invested, _w1_value, _w1_pnl, _w1_pct) = self.flow_metrics_since(start_week);
        let (_m1_invested, _m1_value, _m1_pnl, _m1_pct) = self.flow_metrics_since(start_1m_roll);
        let (_m6_invested, _m6_value, _m6_pnl, _m6_pct) = self.flow_metrics_since(start_6m_roll);
        let (_y1_invested, _y1_value, _y1_pnl, _y1_pct) = self.flow_metrics_since(start_1y_roll);

        // Since last balance check from DB
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
            let is_cash = position.get_ticker().is_none()
                && position.get_asset_class().to_lowercase() == "cash";
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
            .load_preset(UTF8_FULL)
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
            .load_preset(UTF8_FULL)
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
            .load_preset(UTF8_FULL)
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
            .load_preset(UTF8_FULL)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_historic_total_value() {
        use crate::position::from_string;
        let positions_str = std::fs::read_to_string("example_data.json").unwrap();
        let positions = from_string(&positions_str);
        let mut portfolio = Portfolio::new();
        for p in positions {
            portfolio.add_position(p);
        }
        let date = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let value = portfolio.get_historic_total_value(date).await;
        // Should include cash amount directly, and use tickers for others
        match value {
            Ok(v) => assert!(v > 0.0),
            Err(e) => panic!("Error occurred in performance command: {e}"),
        }
    }
}

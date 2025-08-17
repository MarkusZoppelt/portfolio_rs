use crate::position::get_historic_price;
use crate::position::PortfolioPosition;
use chrono::prelude::*;
use colored::Colorize;
use piechart::{Chart, Color};
use std::collections::HashMap;

pub struct Portfolio {
    pub positions: Vec<PortfolioPosition>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoricalPosition {
    pub ticker: String,
    pub amount: f64,
    pub price_at_purchase: f64,
    pub date: DateTime<Utc>,
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

    pub fn get_total_value(&self) -> f64 {
        let mut sum = 0.0;

        for position in &self.positions {
            sum += position.get_balance();
        }
        sum
    }

    pub async fn get_historic_total_value(
        &self,
        db: &sled::Db,
        date: DateTime<Utc>,
    ) -> Result<f64, String> {
        let historical_positions = self
            .get_historical_positions(db, date)
            .map_err(|e| e.to_string())?;

        let mut sum = 0.0;
        let mut errors = vec![];

        use futures::future::join_all;
        let mut tasks = vec![];

        for pos in &historical_positions {
            if !pos.ticker.is_empty() && Self::is_valid_ticker(&pos.ticker) {
                let pos_clone = pos.clone();
                tasks.push(async move {
                    let resp = get_historic_price(&pos_clone.ticker, pos_clone.date).await;
                    (pos_clone, resp)
                });
            } else {
                sum += pos.amount * pos.price_at_purchase;
            }
        }

        let results = join_all(tasks).await;

        for (pos, resp) in results {
            match resp {
                Ok(y_response) => {
                    if let Ok(quote) = y_response.last_quote() {
                        sum += quote.close * pos.amount;
                    } else {
                        sum += pos.amount * pos.price_at_purchase;
                        errors.push(format!("No quote for {}, using purchase price", pos.ticker));
                    }
                }
                Err(e) => {
                    let err_str = format!("{e}");
                    if err_str.contains("Bad Request") || err_str.contains("Not Found") {
                        sum += pos.amount * pos.price_at_purchase;
                        continue;
                    }
                    errors.push(format!("Error fetching {}: {}", pos.ticker, e));
                }
            }
        }

        let critical_errors: Vec<String> = errors
            .into_iter()
            .filter(|e| !e.contains("No quote") && !e.contains("Not Found"))
            .collect();

        if !critical_errors.is_empty() {
            eprintln!("Warnings: {}", critical_errors.join("; "));
        }

        Ok(sum)
    }

    fn is_valid_ticker(ticker: &str) -> bool {
        let valid_tickers = ["AAPL", "GOOGL", "MSFT", "TSLA", "SPY", "QQQ", "VTI", "NVDA"];

        if ticker.to_lowercase().contains("cash")
            || ticker.to_lowercase().contains("bond")
            || ticker.contains(" ")
        {
            return false;
        }

        if valid_tickers.contains(&ticker) {
            return true;
        }
        ticker.len() <= 5 && ticker.chars().all(|c| c.is_alphanumeric())
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
    // maybe replace this function with a library
    pub fn print(&self, include_sum: bool) {
        println!(
            "{0: >26} | {1: >12} | {2: >10} | {3: >10}",
            "Name", "Asset Class", "Amount", "Balance"
        );
        println!("====================================================================");
        for position in &self.positions {
            println!(
                "{0: >26} | {1: >12} | {2: >10.2} | {3: >10.2}",
                position.get_name(),
                position.get_asset_class(),
                position.get_amount(),
                position.get_balance()
            );
        }
        if include_sum {
            println!("====================================================================");
            println!("Your total balance is: {:.2}", self.get_total_value());
        }
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
            .with_ymd_and_hms(Utc::now().year(), 1, 3, 0, 0, 0)
            .unwrap();
        let first_of_the_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 3, 0, 0, 0)
            .unwrap();

        let value_at_beginning_of_year = self
            .get_historic_total_value(&db, first_of_the_year)
            .await?;

        let value_at_beginning_of_month = self
            .get_historic_total_value(&db, first_of_the_month)
            .await?;

        let last: f64 = match &db.iter().last() {
            Some(Ok(last)) => String::from_utf8_lossy(&last.1).parse().unwrap_or(0.0),
            _ => 0.0,
        };

        let current_value = self.get_total_value();

        let ytd_performance = if value_at_beginning_of_year != 0.0 {
            (last - value_at_beginning_of_year) / value_at_beginning_of_year * 100.0
        } else {
            0.0
        };

        let monthly_performance = if value_at_beginning_of_month != 0.0 {
            (last - value_at_beginning_of_month) / value_at_beginning_of_month * 100.0
        } else {
            0.0
        };

        let recent_performance = if current_value != 0.0 {
            (last - current_value) / current_value * 100.0
        } else {
            0.0
        };

        Ok((ytd_performance, monthly_performance, recent_performance))
    }

    pub async fn print_performance(&self) {
        let db = sled::open("database").unwrap();

        let first_of_the_year = Utc
            .with_ymd_and_hms(Utc::now().year(), 1, 3, 0, 0, 0)
            .unwrap();
        let first_of_the_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 3, 0, 0, 0)
            .unwrap();

        let value_at_beginning_of_year =
            self.get_historic_total_value(&db, first_of_the_year).await;
        if let Err(e) = &value_at_beginning_of_year {
            println!("Error getting value for beginning of year: {e}");
            return;
        }

        let value_at_beginning_of_month =
            self.get_historic_total_value(&db, first_of_the_month).await;
        if let Err(e) = &value_at_beginning_of_month {
            println!("Error getting value for beginning of month: {e}");
            return;
        }

        let last: f64 = match &db.iter().last() {
            Some(Ok(last)) => String::from_utf8_lossy(&last.1).parse().unwrap_or(0.0),
            _ => 0.0,
        };

        let values = [
            value_at_beginning_of_year,
            value_at_beginning_of_month,
            Ok(self.get_total_value()),
        ];

        for (i, value) in values.iter().enumerate() {
            let value = match value {
                Ok(value) => *value,
                Err(_) => continue,
            };
            let performance = (last - value) / value * 100.0;
            let s = format!("{performance:.2}%");
            let s = if performance >= 0.0 {
                s.green()
            } else {
                s.red()
            };

            match i {
                0 => println!("YTD: {s}"),
                1 => println!("Since beginning of month: {s}"),
                2 => println!("Since last balance check: {s}"),
                _ => (),
            }
        }
    }

    pub fn add_historical_position(
        &self,
        db: &sled::Db,
        ticker: &str,
        amount: f64,
        price_at_purchase: f64,
        date: DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let hist_pos = HistoricalPosition {
            ticker: ticker.to_string(),
            amount,
            price_at_purchase,
            date,
        };
        let key = format!("{}-{}", date.timestamp(), ticker);
        let value = serde_json::to_vec(&hist_pos)?;
        db.insert(key, value)?;
        Ok(())
    }

    pub fn get_historical_positions(
        &self,
        db: &sled::Db,
        date: DateTime<Utc>,
    ) -> Result<Vec<HistoricalPosition>, Box<dyn std::error::Error>> {
        let mut positions = vec![];

        for item in db.iter() {
            let (_, value) = item?;
            let hist_pos: HistoricalPosition = serde_json::from_slice(&value)?;
            if hist_pos.date <= date {
                positions.push(hist_pos);
            }
        }
        Ok(positions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_get_historic_total_value() {
        let db = sled::open("test_db").unwrap();
        let portfolio = Portfolio::new();
        let test_positions = vec![
            ("AAPL", 10.0, 150.0),
            ("GOOGL", 5.0, 2000.0),
            ("Cash", 1000.0, 1.0),
        ];

        db.clear().unwrap();

        for (ticker, amount, price) in test_positions {
            portfolio
                .add_historical_position(&db, ticker, amount, price, Utc::now())
                .unwrap();
        }
        let date = Utc::now();
        let value = portfolio.get_historic_total_value(&db, date).await;

        match value {
            Ok(v) => {
                println!("Historic total value: {:.2}", v);
                assert!(v > 0.0);
            }
            Err(e) => panic!("Error occurred in performance command: {e}"),
        }

        db.clear().unwrap();
    }

    #[tokio::test]
    async fn test_historical_positions() {
        let db = sled::open("test_db_2").unwrap();
        let portfolio = Portfolio::new();

        db.clear().unwrap();

        portfolio
            .add_historical_position(&db, "AAPL", 10.0, 150.0, Utc::now())
            .unwrap();

        let positions = portfolio.get_historical_positions(&db, Utc::now()).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].ticker, "AAPL");

        db.clear().unwrap();
    }

    #[test]
    fn test_is_valid_ticker() {
        assert!(Portfolio::is_valid_ticker("AAPL"));
        assert!(Portfolio::is_valid_ticker("GOOGL"));
        assert!(!Portfolio::is_valid_ticker("Cash"));
        assert!(!Portfolio::is_valid_ticker("20+yr US Bonds"));
        assert!(!Portfolio::is_valid_ticker("Bitcoin"));
        assert!(!Portfolio::is_valid_ticker("Diversified Commodities"));
    }
}

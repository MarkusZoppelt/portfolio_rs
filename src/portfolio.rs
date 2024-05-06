use crate::position::get_historic_price;
use crate::position::PortfolioPosition;
use chrono::prelude::*;
use colored::Colorize;
use piechart::{Chart, Color};
use std::collections::HashMap;

pub struct Portfolio {
    positions: Vec<PortfolioPosition>,
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

    // Get the total value of the portfolio at a specific date
    // TODO: this function is not working as intended and the y_response is often an error
    pub async fn get_historic_total_value(&self, date: DateTime<Utc>) -> Result<f64, String> {
        let mut sum = 0.0;

        for position in &self.positions {
            let y_response = get_historic_price(
                {
                    let this = &position;
                    this.get_name()
                },
                date,
            )
            .await;

            match y_response {
                Ok(response) => match response.last_quote() {
                    Ok(quote) => {
                        sum += quote.close * position.get_amount();
                    }
                    Err(e) => {
                        return Err(format!(
                            "Error getting last quote for {}: {}",
                            position.get_name(),
                            e
                        ));
                    }
                },
                Err(e) => {
                    return Err(format!(
                        "Error getting historic price data for {}: {}",
                        position.get_name(),
                        e
                    ));
                }
            }
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
            println!("{0: >12} | {1: >10.2}", asset_class, percentage);
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

    pub async fn print_performance(&self) {
        let db = sled::open("database").unwrap();

        // Yahoo first of the year is YYYY-01-03
        let first_of_the_year = Utc
            .with_ymd_and_hms(Utc::now().year(), 1, 1, 0, 0, 0)
            .unwrap();
        let first_of_the_month = Utc
            .with_ymd_and_hms(Utc::now().year(), Utc::now().month(), 3, 0, 0, 0)
            .unwrap();

        let value_at_beginning_of_year = self.get_historic_total_value(first_of_the_year).await;
        if let Err(e) = value_at_beginning_of_year {
            println!("Error getting value for beginning of year: {}", e);
            return;
        }

        let value_at_beginning_of_month = self.get_historic_total_value(first_of_the_month).await;
        if let Err(e) = value_at_beginning_of_month {
            println!("Error getting value for beginning of month: {}", e);
            return;
        }

        let last: f64 = match &db.iter().last() {
            Some(Ok(last)) => {
                let last = String::from_utf8_lossy(&last.1).parse();
                if let Ok(last) = last {
                    last
                } else {
                    0.0
                }
            }
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
            let s = format!("{:.2}%", performance);
            let s = if performance >= 0.0 {
                s.green()
            } else {
                s.red()
            };

            match i {
                0 => println!("YTD: {}", s),
                1 => println!("Since beginning of month: {}", s),
                2 => println!("Since last balance check: {}", s),
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_historic_total_value() {
        let portfolio = Portfolio::new();
        let date = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let value = portfolio.get_historic_total_value(date).await;
        assert_eq!(value, Ok(0.0));
    }
}

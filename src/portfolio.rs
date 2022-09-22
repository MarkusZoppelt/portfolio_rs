use crate::position::PortfolioPosition;
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

        println!("{0: >12} | {1: >10}", "Asset Class", "Percentage");
        println!("====================================");
        for (asset_class, percentage) in allocation_vec {
            println!("{0: >12} | {1: >10.2}", asset_class, percentage);
        }
    }
}

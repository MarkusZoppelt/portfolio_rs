use crate::position::PortfolioPosition;

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
            if let Some(_ticker) = position.get_ticker() {
                sum += position.get_balance();
            } else {
                sum += position.get_amount();
            }
        }
        sum
    }
}

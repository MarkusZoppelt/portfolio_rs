pub mod portfolio {

    use crate::position::portfolio_position::PortfolioPosition;

    pub struct Portfolio {
        positions: Vec<PortfolioPosition>,
        total_value: f64,
    }

    impl Portfolio {
        pub fn new() -> Portfolio {
            Portfolio {
                positions: Vec::new(),
                total_value: 0.0,
            }
        }

        pub fn add_position(&mut self, position: PortfolioPosition) {
            self.positions.push(position);
        }

        pub fn get_positions(&self) -> &Vec<PortfolioPosition> {
            &self.positions
        }

        pub fn get_total_value(&self) -> f64 {
            self.total_value
        }

        pub fn set_total_value(&mut self, total_value: f64) {
            self.total_value = total_value;
        }
    }
}

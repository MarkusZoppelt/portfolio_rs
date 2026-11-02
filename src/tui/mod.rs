//! Interactive terminal UI (the default mode), see `docs/tui.md`.
//!
//! The pure parts are split so they can be tested without a terminal:
//! [`App`] holds the state, `input` maps key events to state changes,
//! `render` draws an [`App`] into any ratatui backend, `history` builds the
//! growth chart series, and `runtime` drives the loop with injected live data
//! and events. Only the real terminal and the live Yahoo calls live in the
//! `*_io.rs` files.

mod app;
mod format;
mod history;
mod input;
mod live;
mod live_io;
mod render;
mod runtime;
mod terminal_io;

use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub use app::App;
pub use terminal_io::run_tui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    // Overview tab components
    TabBar,
    TotalValue,
    PortfolioGrowth,
    AssetBreakdown,
    DetailedAllocation,
    Help,
    // Balances tab components (table columns)
    Name,
    AssetClass,
    Amount,
    Price,
    AvgCost,
    Invested,
    PnL,
    Hist,
    Daily,
    Balance,
}

impl Component {
    /// Returns all available components
    pub fn all() -> Vec<Component> {
        vec![
            Component::TabBar,
            Component::TotalValue,
            Component::PortfolioGrowth,
            Component::AssetBreakdown,
            Component::DetailedAllocation,
            Component::Help,
            Component::Name,
            Component::AssetClass,
            Component::Amount,
            Component::Price,
            Component::AvgCost,
            Component::Invested,
            Component::PnL,
            Component::Hist,
            Component::Daily,
            Component::Balance,
        ]
    }

    /// Returns the string representation of the component
    pub fn as_str(&self) -> &'static str {
        match self {
            Component::TabBar => "tab_bar",
            Component::TotalValue => "total_value",
            Component::PortfolioGrowth => "portfolio_growth",
            Component::AssetBreakdown => "asset_breakdown",
            Component::DetailedAllocation => "detailed_allocation",
            Component::Help => "help",
            Component::Name => "name",
            Component::AssetClass => "asset_class",
            Component::Amount => "amount",
            Component::Price => "price",
            Component::AvgCost => "avg_cost",
            Component::Invested => "invested",
            Component::PnL => "pnl",
            Component::Hist => "%hist",
            Component::Daily => "%day",
            Component::Balance => "balance",
        }
    }

    /// Returns a description of what the component does
    pub fn description(&self) -> &'static str {
        match self {
            Component::TabBar => "Top navigation bar showing active tab",
            Component::TotalValue => "Total portfolio value display",
            Component::PortfolioGrowth => "Portfolio growth chart over time",
            Component::AssetBreakdown => "Asset allocation visualization",
            Component::DetailedAllocation => "Asset percentages",
            Component::Help => "Keyboard shortcuts",
            Component::Name => "Name column in the balances table",
            Component::AssetClass => "Asset Class column in the balances table",
            Component::Amount => "Amount column in the balances table",
            Component::Price => "Market price column in the balances table",
            Component::AvgCost => "Average cost column (from purchases)",
            Component::Invested => "Invested amount column (from purchases)",
            Component::PnL => "Unrealized PnL column",
            Component::Hist => "Historic variation % column (vs invested)",
            Component::Daily => "Daily variation % column (vs previous close)",
            Component::Balance => "Balance column in the balances table",
        }
    }
}

impl FromStr for Component {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "tab_bar" => Ok(Component::TabBar),
            "total_value" => Ok(Component::TotalValue),
            "portfolio_growth" => Ok(Component::PortfolioGrowth),
            "asset_breakdown" => Ok(Component::AssetBreakdown),
            "asset_allocation" => Ok(Component::AssetBreakdown), // Backward compatibility
            "detailed_allocation" => Ok(Component::DetailedAllocation),
            "help" => Ok(Component::Help),
            "name" => Ok(Component::Name),
            "asset_class" => Ok(Component::AssetClass),
            "amount" => Ok(Component::Amount),
            "price" => Ok(Component::Price),
            "avg_cost" => Ok(Component::AvgCost),
            "invested" => Ok(Component::Invested),
            "pnl" => Ok(Component::PnL),
            "%hist" | "hist" => Ok(Component::Hist),
            "%day" | "day" => Ok(Component::Daily),
            "balance" => Ok(Component::Balance),
            _ => Err(format!("Unknown component: '{s}'")),
        }
    }
}

impl Display for Component {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct DisabledComponents {
    disabled: HashSet<Component>,
}

impl DisabledComponents {
    pub fn new(disabled_list: Vec<String>) -> Result<Self, Vec<String>> {
        let mut disabled = HashSet::new();
        let mut errors = Vec::new();

        for component_str in disabled_list {
            match Component::from_str(&component_str) {
                Ok(component) => {
                    disabled.insert(component);
                }
                Err(err) => errors.push(err),
            }
        }

        if errors.is_empty() {
            Ok(DisabledComponents { disabled })
        } else {
            Err(errors)
        }
    }

    pub fn disable_component(&mut self, component: Component) {
        self.disabled.insert(component);
    }

    pub fn is_disabled(&self, component: Component) -> bool {
        self.disabled.contains(&component)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Overview,
    Balances,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppMode {
    Normal,
    Edit,
    PurchaseList,
    AddPurchase,
    EditPurchase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditField {
    Date,
    Quantity,
    Price,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview & Allocation",
            Tab::Balances => "Balances",
        }
    }

    fn all() -> &'static [Tab] {
        &[Tab::Overview, Tab::Balances]
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "overview" => Some(Tab::Overview),
            "balances" => Some(Tab::Balances),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trend {
    Up,
    Down,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NetworkStatus {
    Connected,
    Disconnected,
    Partial,
}

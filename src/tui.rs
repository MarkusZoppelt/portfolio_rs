use crate::portfolio::Portfolio;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem, Paragraph, Row, Table, Tabs, Wrap,
    },
    Frame, Terminal,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tui_big_text::{BigText, PixelSize};
use futures::future::join_all;
use time::OffsetDateTime;
use yahoo_finance_api as yahoo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    // Overview tab components
    TabBar,
    TotalValue,
    AssetAllocation,
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
            Component::AssetAllocation,
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
            Component::AssetAllocation => "asset_allocation",
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
            Component::AssetAllocation => "Asset bar chart",
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
            "asset_allocation" => Ok(Component::AssetAllocation),
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

impl std::fmt::Display for Component {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

    #[cfg(test)]
    pub fn disable_component(&mut self, component: Component) {
        self.disabled.insert(component);
    }

    pub fn is_disabled(&self, component: Component) -> bool {
        self.disabled.contains(&component)
    }
}

fn format_currency(value: f64, currency: &str) -> String {
    let formatted_number = if value >= 1000.0 {
        format_with_commas(value)
    } else {
        match currency {
            "JPY" => format!("{value:.0}"),
            _ => format!("{value:.2}"),
        }
    };

    match currency {
        "USD" | "CAD" | "AUD" | "HKD" | "SGD" => format!("${formatted_number}"),
        "EUR" => format!("{formatted_number} €"),
        "GBP" => format!("£{formatted_number}"),
        "JPY" => {
            let integer_value = value as i64;
            let formatted = format!("{integer_value}");
            let formatted_with_commas = formatted
                .chars()
                .rev()
                .collect::<String>()
                .chars()
                .collect::<Vec<_>>()
                .chunks(3)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(",")
                .chars()
                .rev()
                .collect::<String>();
            format!("{formatted_with_commas} JPY")
        }
        "CHF" => format!("{formatted_number} CHF"),
        "SEK" | "NOK" | "DKK" => format!("{formatted_number} {currency}"),
        _ => format!("{formatted_number} {currency}"),
    }
}

fn format_with_commas(value: f64) -> String {
    let formatted = format!("{value:.2}");
    let parts: Vec<&str> = formatted.split('.').collect();
    let integer_part = parts[0];
    let decimal_part = parts.get(1).unwrap_or(&"00");

    let formatted_integer = integer_part
        .chars()
        .rev()
        .collect::<String>()
        .chars()
        .collect::<Vec<_>>()
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(",")
        .chars()
        .rev()
        .collect::<String>();

    format!("{formatted_integer}.{decimal_part}")
}

fn format_amount(amount: f64) -> String {
    if amount.fract() == 0.0 {
        format!("{amount:.0}")
    } else if amount >= 1.0 {
        format!("{amount:.2}")
    } else if amount >= 0.01 {
        format!("{amount:.4}")
    } else {
        format!("{amount:.8}")
    }
}

fn get_historic_portfolio_data(portfolio: &Portfolio) -> Vec<(f64, f64)> {
    use chrono::prelude::*;
    use crate::position::parse_purchase_date;
    
    let mut purchase_events = Vec::new();
    let now = Utc::now();
    
    // Collect all purchase events with their dates
    for position in &portfolio.positions {
        for purchase in position.get_purchases() {
            if let Some(date_str) = &purchase.date {
                if let Some(date) = parse_purchase_date(date_str) {
                    if let Some(price) = purchase.price {
                        if price > 0.0 && purchase.quantity > 0.0 {
                            let days_ago = (now - date).num_days() as f64;
                            purchase_events.push((date, days_ago, purchase.quantity * price));
                        }
                    }
                }
            }
        }
    }
    
    if purchase_events.is_empty() {
        return vec![];
    }
    
    // Sort by date
    purchase_events.sort_by(|a, b| a.0.cmp(&b.0));
    
    // Create weekly data points by building cumulative portfolio value over time
    let mut weekly_data = Vec::new();
    let mut cumulative_invested = 0.0;
    
    // Get the earliest date
    let earliest_date = purchase_events.first().unwrap().0;
    
    // Create weekly intervals from earliest purchase to now
    let mut current_date = earliest_date;
    let mut event_index = 0;
    let mut week_index: usize = 0;
    
    while current_date <= now {
        // Add all purchases that happened before or on this date
        while event_index < purchase_events.len() && purchase_events[event_index].0 <= current_date {
            cumulative_invested += purchase_events[event_index].2;
            event_index += 1;
        }
        
        if cumulative_invested > 0.0 {
            weekly_data.push((week_index as f64, cumulative_invested));
        }
        
        // Move to next week
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

// Efficient weekly series: one history fetch per ticker across full range, then sample weekly
async fn compute_weekly_series_batch(portfolio: &Portfolio) -> Vec<(f64, f64)> {
    use crate::position::parse_purchase_date;
    use chrono::prelude::*;

    // Determine earliest purchase date
    let mut earliest_opt: Option<DateTime<Utc>> = None;
    for position in &portfolio.positions {
        for p in position.get_purchases() {
            if let Some(ds) = &p.date {
                if let Some(d) = parse_purchase_date(ds) {
                    earliest_opt = match earliest_opt {
                        Some(prev) => Some(prev.min(d)),
                        None => Some(d),
                    };
                }
            }
        }
    }
    let earliest = match earliest_opt { Some(d) => d, None => return vec![] };
    let now = Utc::now();

    // Limit to at most ~78 weeks (~18 months) for speed and readability
    let mut total_weeks = ((now - earliest).num_days() / 7) as usize + 1;
    let max_weeks = 78usize;
    if total_weeks > max_weeks {
        total_weeks = max_weeks;
    }

    // Prepare tasks: fetch full history once per ticker
    let mut cash_sum = 0.0_f64;
    let mut ticker_amounts: Vec<(String, f64)> = Vec::new();
    for position in &portfolio.positions {
        if let Some(ticker) = position.get_ticker() {
            ticker_amounts.push((ticker.to_string(), position.get_amount()));
        } else {
            cash_sum += position.get_amount();
        }
    }

    let start = OffsetDateTime::from_unix_timestamp(earliest.timestamp()).unwrap();
    let end = OffsetDateTime::from_unix_timestamp(now.timestamp()).unwrap();

    let fetches = ticker_amounts.iter().map(|(t, _)| {
        let t2 = t.clone();
        async move {
            // Per-ticker timeout to avoid stalls
            let fut = async {
                let resp = yahoo::YahooConnector::new()?.get_quote_history(&t2, start, end).await?;
                Ok::<yahoo::YResponse, yahoo::YahooError>(resp)
            };
            match tokio::time::timeout(Duration::from_secs(3), fut).await {
                Ok(Ok(resp)) => Some(resp),
                _ => None,
            }
        }
    });

    let responses: Vec<Option<yahoo::YResponse>> = join_all(fetches).await;

    // Build per-ticker sampled prices per week using linear index mapping as an efficient proxy
    let mut per_ticker_weekly: Vec<Vec<f64>> = Vec::new();
    for (i, resp_opt) in responses.into_iter().enumerate() {
        if let Some(resp) = resp_opt {
            if let Ok(quotes) = resp.quotes() {
                let qlen = quotes.len().max(1);
                let mut weekly = Vec::with_capacity(total_weeks);
                for w in 0..total_weeks {
                    let idx = ((w as f64 / (total_weeks - 1).max(1) as f64) * (qlen - 1) as f64).round() as usize;
                    let idx = idx.min(qlen - 1);
                    let price = quotes[idx].close;
                    weekly.push(price);
                }
                per_ticker_weekly.push(weekly);
            }
        } else {
            // No data for this ticker: approximate flat series using current spot via portfolio positions
            let spot = if let Some((_, amt)) = ticker_amounts.get(i) { *amt } else { 0.0 };
            per_ticker_weekly.push(vec![spot; total_weeks]);
        }
    }

    // Sum across tickers per week (price * amount) + cash
    let mut series: Vec<(f64, f64)> = Vec::with_capacity(total_weeks);
    for w in 0..total_weeks {
        let mut total = cash_sum;
        for (ti, (_t, amount)) in ticker_amounts.iter().enumerate() {
            if let Some(weekly) = per_ticker_weekly.get(ti) {
                total += weekly[w] * *amount;
            }
        }
        let x = w as f64; // weeks since start
        series.push((x, total));
    }

    // Ensure final point at now equals current live total
    let current_value = portfolio.get_total_value();
    if current_value > 0.0 {
        let x = (total_weeks.saturating_sub(1)) as f64;
        if let Some(last) = series.last_mut() {
            *last = (x, current_value);
        } else {
            series.push((x, current_value));
        }
    }

    series
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

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "overview" => Some(Tab::Overview),
            "balances" => Some(Tab::Balances),
            _ => None,
        }
    }
}

pub struct App {
    pub current_tab: Tab,
    pub portfolio: Option<Portfolio>,
    pub should_quit: bool,
    pub loading: bool,
    pub error_message: Option<String>,
    pub currency: String,
    pub previous_values: HashMap<String, f64>,
    pub trends: HashMap<String, Trend>,
    pub last_update: Instant,
    pub flash_state: bool,
    pub positions_str: String,
    pub mode: AppMode,
    pub selected_position: usize,
    pub selected_purchase: usize,
    pub edit_input: String,
    pub purchase_date_input: String,
    pub purchase_quantity_input: String,
    pub purchase_price_input: String,
    pub edit_field: EditField,
    pub data_file_path: String,
    pub portfolio_receiver: Option<mpsc::UnboundedReceiver<(Portfolio, NetworkStatus)>>,
    // Historic graph data and channel to receive async updates
    pub historic_data: Option<Vec<(f64, f64)>>,
    pub historic_receiver: Option<mpsc::UnboundedReceiver<Vec<(f64, f64)>>>,
    pub network_status: NetworkStatus,
    pub disabled_components: DisabledComponents,
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

impl App {
    pub fn new(
        currency: String,
        positions_str: String,
        data_file_path: String,
        disabled_components: DisabledComponents,
    ) -> App {
        App {
            current_tab: Tab::Overview,
            portfolio: None,
            should_quit: false,
            loading: false,
            error_message: None,
            currency,
            previous_values: HashMap::new(),
            trends: HashMap::new(),
            last_update: Instant::now(),
            flash_state: false,
            positions_str,
            mode: AppMode::Normal,
            selected_position: 0,
            selected_purchase: 0,
            edit_input: String::new(),
            purchase_date_input: String::new(),
            purchase_quantity_input: String::new(),
            purchase_price_input: String::new(),
            edit_field: EditField::Date,
            data_file_path,
            portfolio_receiver: None,
            historic_data: None,
            historic_receiver: None,
            network_status: NetworkStatus::Connected,
            disabled_components,
        }
    }

    pub fn set_portfolio_receiver(
        &mut self,
        receiver: mpsc::UnboundedReceiver<(Portfolio, NetworkStatus)>,
    ) {
        self.portfolio_receiver = Some(receiver);
    }

    pub fn set_historic_receiver(
        &mut self,
        receiver: mpsc::UnboundedReceiver<Vec<(f64, f64)>>,
    ) {
        self.historic_receiver = Some(receiver);
    }

    pub fn try_receive_portfolio_update(&mut self) -> bool {
        if let Some(receiver) = &mut self.portfolio_receiver {
            if let Ok((portfolio, network_status)) = receiver.try_recv() {
                // Portfolio is already sorted by the background task
                self.update_trends(&portfolio);
                self.set_portfolio(portfolio);
                self.network_status = network_status;
                self.mark_refreshed();
                return true;
            }
        }
        false
    }

    pub fn try_receive_historic_update(&mut self) -> bool {
        if let Some(receiver) = &mut self.historic_receiver {
            if let Ok(series) = receiver.try_recv() {
                self.historic_data = Some(series);
                self.mark_refreshed();
                return true;
            }
        }
        false
    }

    pub fn set_portfolio(&mut self, portfolio: Portfolio) {
        self.portfolio = Some(portfolio);
        self.loading = false;
    }

    pub fn next_tab(&mut self) {
        let tabs = Tab::all();
        let current_index = tabs
            .iter()
            .position(|&t| t == self.current_tab)
            .unwrap_or(0);
        self.current_tab = tabs[(current_index + 1) % tabs.len()];
    }

    pub fn previous_tab(&mut self) {
        let tabs = Tab::all();
        let current_index = tabs
            .iter()
            .position(|&t| t == self.current_tab)
            .unwrap_or(0);
        self.current_tab = tabs[(current_index + tabs.len() - 1) % tabs.len()];
    }

    pub fn update_trends(&mut self, portfolio: &Portfolio) {
        for position in &portfolio.positions {
            let name = position.get_name().to_string();
            let current_value = position.get_balance();

            if let Some(&previous_value) = self.previous_values.get(&name) {
                // Use a small threshold to avoid noise from tiny changes
                let threshold = 0.01; // 1 cent threshold
                let trend = if current_value > previous_value + threshold {
                    Trend::Up
                } else if current_value < previous_value - threshold {
                    Trend::Down
                } else {
                    // Keep the previous trend if change is too small, or set neutral if no previous trend
                    self.trends.get(&name).copied().unwrap_or(Trend::Neutral)
                };
                self.trends.insert(name.clone(), trend);
            } else {
                // First time seeing this position
                self.trends.insert(name.clone(), Trend::Neutral);
            }

            self.previous_values.insert(name, current_value);
        }
    }

    pub fn mark_refreshed(&mut self) {
        self.last_update = Instant::now();
        self.flash_state = !self.flash_state; // Toggle flash state for animation
    }

    pub fn get_trend_color(&self, name: &str, base_color: Color) -> Color {
        match self.trends.get(name) {
            Some(Trend::Up) => {
                if self.flash_state {
                    Color::LightGreen
                } else {
                    Color::Green
                }
            }
            Some(Trend::Down) => {
                if self.flash_state {
                    Color::LightRed
                } else {
                    Color::Red
                }
            }
            _ => base_color,
        }
    }

    pub fn select_next(&mut self) {
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len().saturating_sub(1) {
                self.selected_position += 1;
            }
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_position > 0 {
            self.selected_position -= 1;
        }
    }

    pub fn enter_edit_mode(&mut self) {
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len() {
                self.mode = AppMode::PurchaseList;
                self.selected_purchase = 0;
            }
        }
    }

    pub fn exit_edit_mode(&mut self) {
        self.mode = AppMode::Normal;
        self.edit_input.clear();
        self.purchase_date_input.clear();
        self.purchase_quantity_input.clear();
        self.purchase_price_input.clear();
    }

    pub fn select_next_purchase(&mut self) {
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len() {
                let purchases = portfolio.positions[self.selected_position].get_purchases();
                // 0 = Add New, 1 to purchases.len() = existing purchases
                if self.selected_purchase < purchases.len() {
                    self.selected_purchase += 1;
                }
            }
        }
    }

    pub fn select_previous_purchase(&mut self) {
        if self.selected_purchase > 0 {
            self.selected_purchase -= 1;
        }
    }

    pub fn enter_add_purchase_mode(&mut self) {
        self.mode = AppMode::AddPurchase;
        self.edit_field = EditField::Date;
        self.purchase_date_input.clear();
        self.purchase_quantity_input.clear();
        self.purchase_price_input.clear();
    }

    pub fn enter_edit_purchase_mode(&mut self) {
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len() {
                let position = &portfolio.positions[self.selected_position];
                let purchases = position.get_purchases();
                
                // selected_purchase: 0 = Add New, 1+ = existing purchases
                // We need to map from display order (sorted by date) to original order
                if self.selected_purchase > 0 && self.selected_purchase <= purchases.len() {
                    // Create sorted list to find the actual purchase
                    let mut purchase_list: Vec<(usize, &crate::position::Purchase)> = purchases.iter().enumerate().collect();
                    purchase_list.sort_by(|a, b| {
                        let date_a = a.1.date.as_deref().unwrap_or("");
                        let date_b = b.1.date.as_deref().unwrap_or("");
                        date_b.cmp(date_a) // Reverse order for newest first
                    });
                    
                    let display_index = self.selected_purchase - 1; // Convert to 0-based for sorted list
                    if display_index < purchase_list.len() {
                        let (original_index, purchase) = purchase_list[display_index];
                        
                        self.mode = AppMode::EditPurchase;
                        self.edit_field = EditField::Date;
                        self.purchase_date_input = purchase.date.clone().unwrap_or_default();
                        self.purchase_quantity_input = purchase.quantity.to_string();
                        // Only prefill price input if Price existed in the original JSON
                        // Otherwise keep empty so auto prices don't get saved accidentally
                        let mut price_from_json: Option<String> = None;
                        if let Ok(original_data) = serde_json::from_str::<Vec<serde_json::Value>>(&self.positions_str) {
                            if self.selected_position < original_data.len() {
                                if let Some(purchases_val) = original_data[self.selected_position].get("Purchases") {
                                    if let Some(arr) = purchases_val.as_array() {
                                        if original_index < arr.len() {
                                            if let Some(price_val) = arr[original_index].get("Price") {
                                                if let Some(p) = price_val.as_f64() {
                                                    price_from_json = Some(p.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        self.purchase_price_input = price_from_json.unwrap_or_default();
                    }
                }
            }
        }
    }

    pub fn next_edit_field(&mut self) {
        self.edit_field = match self.edit_field {
            EditField::Date => EditField::Quantity,
            EditField::Quantity => EditField::Price,
            EditField::Price => EditField::Date,
        };
    }

    pub fn previous_edit_field(&mut self) {
        self.edit_field = match self.edit_field {
            EditField::Date => EditField::Price,
            EditField::Quantity => EditField::Date,
            EditField::Price => EditField::Quantity,
        };
    }

    pub fn get_current_input(&self) -> &str {
        match self.edit_field {
            EditField::Date => &self.purchase_date_input,
            EditField::Quantity => &self.purchase_quantity_input,
            EditField::Price => &self.purchase_price_input,
        }
    }

    pub fn get_current_input_mut(&mut self) -> &mut String {
        match self.edit_field {
            EditField::Date => &mut self.purchase_date_input,
            EditField::Quantity => &mut self.purchase_quantity_input,
            EditField::Price => &mut self.purchase_price_input,
        }
    }

    pub fn save_edited_purchase(&mut self) -> Result<(), String> {
        // Validate inputs
        if self.purchase_date_input.trim().is_empty() {
            return Err("Date is required".to_string());
        }
        if self.purchase_quantity_input.trim().is_empty() {
            return Err("Quantity is required".to_string());
        }

        let quantity: f64 = self.purchase_quantity_input.parse()
            .map_err(|_| "Invalid quantity format".to_string())?;
        
        if quantity <= 0.0 {
            return Err("Quantity must be positive".to_string());
        }

        let price: f64 = if self.purchase_price_input.trim().is_empty() {
            0.0 // Will be auto-filled by the system
        } else {
            self.purchase_price_input.parse()
                .map_err(|_| "Invalid price format".to_string())?
        };

        if price < 0.0 {
            return Err("Price cannot be negative".to_string());
        }

                            // Save to file
        self.save_purchase_edit_to_file(&self.purchase_date_input, quantity, price)?;

                            self.exit_edit_mode();
                            Ok(())
    }

    pub fn save_new_purchase(&mut self) -> Result<(), String> {
        // Validate inputs
        if self.purchase_date_input.trim().is_empty() {
            return Err("Date is required".to_string());
        }
        if self.purchase_quantity_input.trim().is_empty() {
            return Err("Quantity is required".to_string());
        }

        let quantity: f64 = self.purchase_quantity_input.parse()
            .map_err(|_| "Invalid quantity format".to_string())?;
        
        if quantity <= 0.0 {
            return Err("Quantity must be positive".to_string());
        }

        let price: f64 = if self.purchase_price_input.trim().is_empty() {
            0.0 // Will be auto-filled by the system
        } else {
            self.purchase_price_input.parse()
                .map_err(|_| "Invalid price format".to_string())?
        };

        if price < 0.0 {
            return Err("Price cannot be negative".to_string());
        }

        // Save to file
        self.save_purchase_to_file(&self.purchase_date_input, quantity, price)?;
        
        self.exit_edit_mode();
        Ok(())
    }

    // Removed unused legacy amount edit functions (save_edit, save_to_file)

    fn save_purchase_to_file(&self, date: &str, quantity: f64, price: f64) -> Result<(), String> {
        // Parse the original file to preserve all data
        let mut original_data: Vec<serde_json::Value> = serde_json::from_str(&self.positions_str)
            .map_err(|e| format!("Failed to parse original data: {e}"))?;

        if self.selected_position >= original_data.len() {
            return Err("Invalid position selected".to_string());
        }

        // Get the position object
        let position_obj = original_data[self.selected_position].as_object_mut()
            .ok_or("Invalid position data")?;

        // Get or create the Purchases array
        let purchases_array = position_obj
            .entry("Purchases".to_string())
            .or_insert_with(|| serde_json::Value::Array(vec![]));

        let purchases = purchases_array.as_array_mut()
            .ok_or("Purchases field is not an array")?;

        // Create new purchase object
        let mut new_purchase = serde_json::Map::new();
        new_purchase.insert("Date".to_string(), serde_json::Value::String(date.to_string()));
        new_purchase.insert("Quantity".to_string(), 
            serde_json::Value::Number(serde_json::Number::from_f64(quantity)
                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())));
        
        // Only persist Price if the user explicitly provided it (>0). Otherwise, omit the field
        if price > 0.0 {
            if let Some(num) = serde_json::Number::from_f64(price) {
                new_purchase.insert("Price".to_string(), serde_json::Value::Number(num));
            }
        }

        // Add the new purchase
        purchases.push(serde_json::Value::Object(new_purchase));

        // Update the Amount field to reflect total quantity
        let total_quantity: f64 = purchases.iter()
            .filter_map(|p| p.get("Quantity")?.as_f64())
            .sum();
        
        position_obj.insert("Amount".to_string(), 
            serde_json::Value::Number(serde_json::Number::from_f64(total_quantity)
                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())));

        // Save the updated data
        let json_string = serde_json::to_string_pretty(&original_data)
            .map_err(|e| format!("Failed to serialize data: {e}"))?;

        std::fs::write(&self.data_file_path, json_string)
            .map_err(|e| format!("Failed to write to file: {e}"))?;

        Ok(())
    }

    fn save_purchase_edit_to_file(&self, date: &str, quantity: f64, price: f64) -> Result<(), String> {
        // Parse the original file to preserve all data
        let mut original_data: Vec<serde_json::Value> = serde_json::from_str(&self.positions_str)
            .map_err(|e| format!("Failed to parse original data: {e}"))?;

        if self.selected_position >= original_data.len() {
            return Err("Invalid position selected".to_string());
        }

        // Get the position object
        let position_obj = original_data[self.selected_position].as_object_mut()
            .ok_or("Invalid position data")?;

        // Get the Purchases array
        let purchases_array = position_obj
            .get_mut("Purchases")
            .ok_or("No purchases found")?;

        let purchases = purchases_array.as_array_mut()
            .ok_or("Purchases field is not an array")?;

        // Find the purchase to edit by mapping from display order to original order
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len() {
                let position = &portfolio.positions[self.selected_position];
                let portfolio_purchases = position.get_purchases();
                
                // Create sorted list to find the actual purchase index
                let mut purchase_list: Vec<(usize, &crate::position::Purchase)> = portfolio_purchases.iter().enumerate().collect();
                purchase_list.sort_by(|a, b| {
                    let date_a = a.1.date.as_deref().unwrap_or("");
                    let date_b = b.1.date.as_deref().unwrap_or("");
                    date_b.cmp(date_a) // Reverse order for newest first
                });
                
                let display_index = self.selected_purchase - 1; // Convert to 0-based for sorted list
                if display_index < purchase_list.len() {
                    let original_index = purchase_list[display_index].0;
                    
                    if original_index < purchases.len() {
                        // Update the purchase at the original index
                        let purchase_obj = purchases[original_index].as_object_mut()
                            .ok_or("Invalid purchase data")?;
                        
                        purchase_obj.insert("Date".to_string(), serde_json::Value::String(date.to_string()));
                        purchase_obj.insert("Quantity".to_string(), 
                            serde_json::Value::Number(serde_json::Number::from_f64(quantity)
                                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())));
                        
                        // Only persist Price if the user explicitly provided it (>0). Otherwise, remove the field
                        if price > 0.0 {
                            if let Some(num) = serde_json::Number::from_f64(price) {
                                purchase_obj.insert("Price".to_string(), serde_json::Value::Number(num));
                            }
                        } else {
                            purchase_obj.remove("Price");
                        }

                        // Update the Amount field to reflect total quantity
                        let total_quantity: f64 = purchases.iter()
                            .filter_map(|p| p.get("Quantity")?.as_f64())
                            .sum();
                        
                        position_obj.insert("Amount".to_string(), 
                            serde_json::Value::Number(serde_json::Number::from_f64(total_quantity)
                                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())));

                        // Save the updated data
                        let json_string = serde_json::to_string_pretty(&original_data)
                .map_err(|e| format!("Failed to serialize data: {e}"))?;

            std::fs::write(&self.data_file_path, json_string)
                .map_err(|e| format!("Failed to write to file: {e}"))?;

                        return Ok(());
                    }
                }
            }
        }

        Err("Could not find purchase to edit".to_string())
    }
}

pub async fn run_tui(
    portfolio: Portfolio,
    currency: String,
    positions_str: String,
    data_file_path: String,
    tab: Option<Tab>,
    disabled_components: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Pre-compute historic series so graph shows on launch (<=5s)
    let initial_series = tokio::time::timeout(
        Duration::from_secs(5),
        compute_weekly_series_batch(&portfolio),
    )
    .await
    .unwrap_or_else(|_| Vec::new());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let disabled = match DisabledComponents::new(disabled_components) {
        Ok(disabled) => disabled,
        Err(errors) => {
            return Err(format!("Invalid disabled components - {}", errors.join(", ")).into());
        }
    };
    let mut app = App::new(currency, positions_str.clone(), data_file_path, disabled);
    app.set_portfolio(portfolio);
    if !initial_series.is_empty() {
        app.historic_data = Some(initial_series);
    }
    if let Some(tab) = tab {
        app.current_tab = tab;
    }

    // Create channel for background portfolio updates
    let (portfolio_sender, portfolio_receiver) = mpsc::unbounded_channel();
    app.set_portfolio_receiver(portfolio_receiver);
    let (historic_sender, historic_receiver) = mpsc::unbounded_channel();
    app.set_historic_receiver(historic_receiver);

    // Spawn background task for portfolio updates
    let positions_str_bg = positions_str.clone();
    let data_file_path_bg = app.data_file_path.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5)); // Update every 5 seconds instead of 1
        loop {
            interval.tick().await;
            // Always read latest file content if possible; fall back to initial string
            let positions_str_current = std::fs::read_to_string(&data_file_path_bg)
                .unwrap_or_else(|_| positions_str_bg.clone());
            let (mut portfolio, network_status) =
                crate::create_live_portfolio(positions_str_current).await;
            // Sort in memory for display only
            portfolio.sort_positions_by_value_desc();
            if portfolio_sender.send((portfolio, network_status)).is_err() {
                break; // Channel closed, exit task
            }
        }
    });

    // Spawn background task to compute weekly historic series from purchases (fast batch)
    let data_file_path_hist = app.data_file_path.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await;
            // Read latest file and build portfolio for valuation lookup if needed
            let positions_str_current = match std::fs::read_to_string(&data_file_path_hist) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let (mut portfolio, _status) = crate::create_live_portfolio_with_logging(positions_str_current, false).await;
            // Sort in memory for consistency
            portfolio.sort_positions_by_value_desc();
            // Batch method is much faster (single fetch per ticker)
            let series = compute_weekly_series_batch(&portfolio).await;
            if historic_sender.send(series).is_err() {
                break;
            }
        }
    });

    let res = run_app(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        // Check for portfolio updates from background task (non-blocking)
        app.try_receive_portfolio_update();
        app.try_receive_historic_update();

        // Use poll to check for events with timeout
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.mode {
                        AppMode::Normal => {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Esc => {
                                    app.should_quit = true;
                                }
                                // Vim navigation - hjkl
                                KeyCode::Char('h') | KeyCode::Left => {
                                    app.previous_tab();
                                }
                                KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => {
                                    app.next_tab();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    if app.current_tab == Tab::Balances {
                                        app.select_next();
                                    }
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    if app.current_tab == Tab::Balances {
                                        app.select_previous();
                                    }
                                }
                                KeyCode::Char('e') => {
                                    if app.current_tab == Tab::Balances {
                                        app.enter_edit_mode();
                                    }
                                }
                                KeyCode::Char('r') => {
                                    // Manual refresh: read latest file and rebuild portfolio immediately
                                    if let Ok(new_positions_str) =
                                        std::fs::read_to_string(&app.data_file_path)
                                    {
                                        app.positions_str = new_positions_str.clone();
                                    }
                                    let (mut portfolio, network_status) =
                                        crate::create_live_portfolio(app.positions_str.clone()).await;
                                    // Sort in memory for display only
                                    portfolio.sort_positions_by_value_desc();
                                    // Also recompute weekly historic series immediately (fast batch)
                                    let hist_series = compute_weekly_series_batch(&portfolio).await;
                                    app.update_trends(&portfolio);
                                    app.set_portfolio(portfolio);
                                    app.historic_data = Some(hist_series);
                                    app.network_status = network_status;
                                    app.mark_refreshed();
                                }
                                KeyCode::BackTab => {
                                    app.previous_tab();
                                }
                                KeyCode::Char('1') => app.current_tab = Tab::Overview,
                                KeyCode::Char('2') => app.current_tab = Tab::Balances,
                                _ => {}
                            }
                        }
                        AppMode::PurchaseList => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.exit_edit_mode();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.select_next_purchase();
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.select_previous_purchase();
                                }
                                KeyCode::Char('a') | KeyCode::Enter => {
                                    if app.selected_purchase == 0 {
                                        // Add new purchase
                                        app.enter_add_purchase_mode();
                                    } else {
                                        // Edit existing purchase
                                        app.enter_edit_purchase_mode();
                                    }
                                }
                                _ => {}
                            }
                        }
                        AppMode::AddPurchase => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.mode = AppMode::PurchaseList;
                                    app.purchase_date_input.clear();
                                    app.purchase_quantity_input.clear();
                                    app.purchase_price_input.clear();
                                }
                                KeyCode::Tab | KeyCode::Down => {
                                    app.next_edit_field();
                                }
                                KeyCode::Up => {
                                    app.previous_edit_field();
                                }
                                KeyCode::Enter => {
                                    match app.save_new_purchase() {
                                        Ok(()) => {
                                            // Update positions_str with new data from file and refresh immediately
                                            if let Ok(new_positions_str) =
                                                std::fs::read_to_string(&app.data_file_path)
                                            {
                                                app.positions_str = new_positions_str;
                                            }
                                             let (mut portfolio, network_status) =
                                                crate::create_live_portfolio(app.positions_str.clone()).await;
                                             // Sort in memory for display only
                                             portfolio.sort_positions_by_value_desc();
                                            let hist_series = compute_weekly_series_batch(&portfolio).await;
                                            app.update_trends(&portfolio);
                                            app.set_portfolio(portfolio);
                                            app.historic_data = Some(hist_series);
                                            app.network_status = network_status;
                                            app.mark_refreshed();
                                        }
                                        Err(e) => {
                                            app.error_message = Some(e);
                                        }
                                    }
                                }
                                KeyCode::Backspace => {
                                    app.get_current_input_mut().pop();
                                }
                                KeyCode::Char(c) => {
                                    match app.edit_field {
                                        EditField::Date => {
                                            if c.is_ascii_digit() {
                                                let current = app.get_current_input_mut();

                                                // Count digits only (ignoring dashes)
                                                let digit_count = current.chars().filter(|c| c.is_ascii_digit()).count();

                                                if digit_count < 8 {
                                                    current.push(c);

                                                    // Auto-add dashes at positions 4 and 7 (after YYYY and MM)
                                                    if current.len() == 4 || current.len() == 7 {
                                                        current.push('-');
                                                    }
                                                }
                                            }
                                        }
                                        EditField::Quantity | EditField::Price => {
                                            // Allow numbers with decimal point
                                            if c.is_ascii_digit() || (c == '.' && !app.get_current_input().contains('.')) {
                                                app.get_current_input_mut().push(c);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        AppMode::EditPurchase => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.mode = AppMode::PurchaseList;
                                    app.purchase_date_input.clear();
                                    app.purchase_quantity_input.clear();
                                    app.purchase_price_input.clear();
                                }
                                KeyCode::Tab | KeyCode::Down => {
                                    app.next_edit_field();
                                }
                                KeyCode::Up => {
                                    app.previous_edit_field();
                                }
                                KeyCode::Enter => {
                                    match app.save_edited_purchase() {
                                        Ok(()) => {
                                            // Update positions_str with new data from file and refresh immediately
                                            if let Ok(new_positions_str) =
                                                std::fs::read_to_string(&app.data_file_path)
                                            {
                                                app.positions_str = new_positions_str;
                                            }
                                             let (mut portfolio, network_status) =
                                                crate::create_live_portfolio(app.positions_str.clone()).await;
                                             // Sort in memory for display only
                                             portfolio.sort_positions_by_value_desc();
                                            let hist_series = compute_weekly_series_batch(&portfolio).await;
                                            app.update_trends(&portfolio);
                                            app.set_portfolio(portfolio);
                                            app.historic_data = Some(hist_series);
                                            app.network_status = network_status;
                                            app.mark_refreshed();
                                        }
                                        Err(e) => {
                                            app.error_message = Some(e);
                                        }
                                    }
                                }
                                KeyCode::Backspace => {
                                    app.get_current_input_mut().pop();
                                }
                                KeyCode::Char(c) => {
                                    match app.edit_field {
                                        EditField::Date => {
                                            if c.is_ascii_digit() {
                                                let current = app.get_current_input_mut();

                                                // Count digits only (ignoring dashes)
                                                let digit_count = current.chars().filter(|c| c.is_ascii_digit()).count();

                                                if digit_count < 8 {
                                                    current.push(c);

                                                    // Auto-add dashes at positions 4 and 7 (after YYYY and MM)
                                                    if current.len() == 4 || current.len() == 7 {
                                                        current.push('-');
                                                    }
                                                }
                                            }
                                        }
                                        EditField::Quantity | EditField::Price => {
                                            // Allow numbers with decimal point
                                            if c.is_ascii_digit() || (c == '.' && !app.get_current_input().contains('.')) {
                                                app.get_current_input_mut().push(c);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        AppMode::Edit => {
                            // Legacy mode - can be removed later
                            if key.code == KeyCode::Esc {
                                app.exit_edit_mode();
                            }
                        }
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = if app.disabled_components.is_disabled(Component::TabBar) {
        // If tab bar is disabled, use the full area for content
        vec![f.area()]
    } else {
        // Normal layout with tab bar
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(f.area())
            .to_vec()
    };

    // Only render tab bar if not disabled
    if !app.disabled_components.is_disabled(Component::TabBar) {
        let tab_titles: Vec<Line> = Tab::all()
            .iter()
            .map(|t| {
                let style = if *t == app.current_tab {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(t.title(), style))
            })
            .collect();

        let tabs = Tabs::new(tab_titles)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Portfolio TUI"),
            )
            .style(Style::default().fg(Color::White))
            .highlight_style(Style::default().fg(Color::Yellow))
            .select(
                Tab::all()
                    .iter()
                    .position(|&t| t == app.current_tab)
                    .unwrap_or(0),
            );

        f.render_widget(tabs, chunks[0]);
    }

    let content_area = if app.disabled_components.is_disabled(Component::TabBar) {
        chunks[0]
    } else {
        chunks[1]
    };

    match app.current_tab {
        Tab::Overview => render_overview(f, content_area, app),
        Tab::Balances => {
            match app.mode {
                AppMode::PurchaseList => render_purchase_list(f, content_area, app),
                AppMode::AddPurchase => render_add_purchase_form(f, content_area, app),
                AppMode::EditPurchase => render_edit_purchase_form(f, content_area, app),
                _ => render_balances(f, content_area, app),
            }
        }
    }

    if let Some(error) = &app.error_message {
        render_error_popup(f, error);
    }
}

fn render_historic_graph(f: &mut Frame, area: Rect, portfolio: &Portfolio, app: &App) {
    // Prefer async-computed weekly series if available, else fallback to simple cumulative
    let historic_data = if let Some(series) = &app.historic_data {
        series.clone()
    } else {
        get_historic_portfolio_data(portfolio)
    };
    
    if historic_data.is_empty() {
        let placeholder = Paragraph::new("No purchase history found\nAdd purchase dates and prices to your portfolio.json")
            .block(Block::default().borders(Borders::ALL).title("Portfolio History (Based on Purchase Dates)"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(placeholder, area);
        return;
    }

    // Find min/max values for scaling
    let min_value = historic_data
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::INFINITY, f64::min);
    let max_value = historic_data
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_week = historic_data
        .iter()
        .map(|(d, _)| *d)
        .fold(f64::INFINITY, f64::min);
    let max_week = historic_data
        .iter()
        .map(|(d, _)| *d)
        .fold(f64::NEG_INFINITY, f64::max);

    // Robust y-range with sensible padding even for flat series
    let mut y_min = min_value.max(0.0);
    let mut y_max = max_value;
    let span = (y_max - y_min).abs();
    let pad = if span > 0.0 {
        span * 0.1
    } else {
        // Flat series: add a small absolute pad based on magnitude, min 1.0
        (y_max.abs().max(1.0)) * 0.05
    };
    y_min = (y_min - pad).max(0.0);
    y_max += pad;
    if y_max <= y_min {
        y_max = y_min + y_min.max(1.0) * 0.1;
    }

    let datasets = vec![Dataset::default()
        .marker(ratatui::symbols::Marker::Braille)
        .style(Style::default().fg(Color::Cyan))
        .graph_type(GraphType::Line)
        .data(&historic_data)];

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Portfolio Growth")
        )
        .x_axis(
            Axis::default()
                .title("Weeks Since First Purchase")
                .style(Style::default().fg(Color::Gray))
                .bounds([min_week, max_week])
                .labels(vec![
                    Line::from("0"),
                    Line::from(format!("{:.0}", max_week * 0.25)),
                    Line::from(format!("{:.0}", max_week * 0.5)),
                    Line::from(format!("{:.0}", max_week * 0.75)),
                    Line::from(format!("{:.0}", max_week)),
                ])
        )
        .y_axis(
            {
                let range = (y_max - y_min).max(f64::EPSILON);
                let tick = |t: f64| y_min + range * t;
                Axis::default()
                    .title(app.currency.as_str())
                    .style(Style::default().fg(Color::Gray))
                    .bounds([y_min, y_max])
                    .labels(vec![
                        Line::from(format!("{:.0}", y_min)),
                        Line::from(format!("{:.0}", tick(0.25))),
                        Line::from(format!("{:.0}", tick(0.5))),
                        Line::from(format!("{:.0}", tick(0.75))),
                        Line::from(format!("{:.0}", y_max)),
                    ])
            }
        );

    f.render_widget(chart, area);
}

fn render_detailed_allocation_positions(f: &mut Frame, area: Rect, portfolio: &Portfolio) {
    let colors = [
        Color::Red, Color::Green, Color::Blue, Color::Yellow, 
        Color::Magenta, Color::Cyan, Color::White, Color::LightRed,
    ];
    
    let positions: Vec<_> = portfolio.positions.iter().take(6).collect(); // Limit to 6 for horizontal display
    let mut pie_lines = Vec::new();
    
    // Create compact horizontal bars with embedded labels
    let mut chart_lines = Vec::new();
    
    for (i, position) in positions.iter().enumerate() {
        let name = position.get_name();
        let percentage = (position.get_balance() / portfolio.get_total_value()) * 100.0;
        let color = colors[i % colors.len()];
        
        // Create horizontal bar (max 30 characters wide)
        let bar_width = ((percentage / 100.0) * 30.0) as usize;
        let bar_width = bar_width.clamp(1, 30);
        
        // Truncate name to fit in available space
        let display_name = if name.len() > 12 { &name[..12] } else { name };
        
        let mut line_spans = Vec::new();
        line_spans.push(Span::styled("● ", Style::default().fg(color)));
        line_spans.push(Span::styled(format!("{:<12}", display_name), Style::default().fg(Color::White)));
        line_spans.push(Span::styled(format!("{:>6.1}% ", percentage), Style::default().fg(color)));
        line_spans.push(Span::styled("█".repeat(bar_width), Style::default().fg(color)));
        
        chart_lines.push(Line::from(line_spans));
    }
    
    pie_lines.extend(chart_lines);
    
    if portfolio.positions.len() > 6 {
        pie_lines.push(Line::from(""));
        pie_lines.push(Line::from(vec![
            Span::styled(format!("... and {} more positions", portfolio.positions.len() - 6), Style::default().fg(Color::Gray))
        ]));
    }

    let pie_widget = Paragraph::new(pie_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Detailed Allocation")
        )
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left);

    f.render_widget(pie_widget, area);
}

fn render_purchase_list(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        if app.selected_position >= portfolio.positions.len() {
            return;
        }

        let position = &portfolio.positions[app.selected_position];
        let purchases = position.get_purchases();

        let mut items = Vec::new();
        
        // Add "Add New Purchase" option first
        let add_style = if app.selected_purchase == 0 {
            Style::default().bg(Color::Green).fg(Color::Black)
        } else {
            Style::default().fg(Color::Green)
        };
        items.push(ListItem::new("+ Add New Purchase").style(add_style));
        
        // Sort purchases by date (newest first)
        let mut purchase_list: Vec<(usize, &crate::position::Purchase)> = purchases.iter().enumerate().collect();
        purchase_list.sort_by(|a, b| {
            let date_a = a.1.date.as_deref().unwrap_or("");
            let date_b = b.1.date.as_deref().unwrap_or("");
            date_b.cmp(date_a) // Reverse order for newest first
        });

        // Add existing purchases (sorted by date, newest first)
        for (display_index, (_original_index, purchase)) in purchase_list.iter().enumerate() {
            let date = purchase.date.as_deref().unwrap_or("No date");
            let quantity = purchase.quantity;
            let price = purchase.price.unwrap_or(0.0);
            let total = quantity * price;
            
            // selected_purchase: 0 = Add New, 1+ = existing purchases
            let style = if (display_index + 1) == app.selected_purchase {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };

            let item_text = if price > 0.0 {
                format!("{} | Qty: {:.4} | Price: ${:.2} | Total: ${:.2}", 
                       date, quantity, price, total)
            } else {
                format!("{} | Qty: {:.4} | Price: Auto-filled", 
                       date, quantity)
            };

            items.push(ListItem::new(item_text).style(style));
        }

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Purchase History - {}", position.get_name()))
            )
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));

        f.render_widget(list, area);

        // Show help at the bottom
        let help_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area)[1];

        let help_text = Paragraph::new("j/k: Navigate | Enter/a: Add New (first) or Edit (others) | Esc: Back")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);

        f.render_widget(help_text, help_area);
    }
}

fn render_add_purchase_form(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        if app.selected_position >= portfolio.positions.len() {
            return;
        }

        let position = &portfolio.positions[app.selected_position];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Length(3), // Date field
                Constraint::Length(3), // Quantity field  
                Constraint::Length(3), // Price field
                Constraint::Min(0),    // Spacer
                Constraint::Length(3), // Help
            ])
            .split(area);

        // Title
        let title = Paragraph::new(format!("Add Purchase - {}", position.get_name()))
            .block(Block::default().borders(Borders::ALL))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        f.render_widget(title, chunks[0]);

        // Date field
        let date_style = if app.edit_field == EditField::Date {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let date_field = Paragraph::new(format!("Date (type digits only): {}", app.purchase_date_input))
            .block(Block::default().borders(Borders::ALL).title("Date"))
            .style(date_style);
        f.render_widget(date_field, chunks[1]);

        // Quantity field
        let qty_style = if app.edit_field == EditField::Quantity {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let qty_field = Paragraph::new(format!("Quantity: {}", app.purchase_quantity_input))
            .block(Block::default().borders(Borders::ALL).title("Quantity"))
            .style(qty_style);
        f.render_widget(qty_field, chunks[2]);

        // Price field
        let price_style = if app.edit_field == EditField::Price {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let price_help = if app.purchase_price_input.is_empty() {
            " (leave empty for auto-fill)"
        } else {
            ""
        };
        let price_field = Paragraph::new(format!("Price (optional): {}{}", app.purchase_price_input, price_help))
            .block(Block::default().borders(Borders::ALL).title("Price"))
            .style(price_style);
        f.render_widget(price_field, chunks[3]);

        // Help
        let help_text = Paragraph::new("Tab/↓: Next Field | ↑: Previous Field | Enter: Save | Esc: Cancel")
            .block(Block::default().borders(Borders::ALL).title("Help"))
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center);
        f.render_widget(help_text, chunks[5]);
    }
}

fn render_edit_purchase_form(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        if app.selected_position >= portfolio.positions.len() {
            return;
        }

        let position = &portfolio.positions[app.selected_position];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Length(3), // Date field
                Constraint::Length(3), // Quantity field  
                Constraint::Length(3), // Price field
                Constraint::Min(0),    // Spacer
                Constraint::Length(3), // Help
            ])
            .split(area);

        // Title
        let title = Paragraph::new(format!("Edit Purchase - {}", position.get_name()))
            .block(Block::default().borders(Borders::ALL))
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        f.render_widget(title, chunks[0]);

        // Date field
        let date_style = if app.edit_field == EditField::Date {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let date_field = Paragraph::new(format!("Date (type digits only): {}", app.purchase_date_input))
            .block(Block::default().borders(Borders::ALL).title("Date"))
            .style(date_style);
        f.render_widget(date_field, chunks[1]);

        // Quantity field
        let qty_style = if app.edit_field == EditField::Quantity {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let qty_field = Paragraph::new(format!("Quantity: {}", app.purchase_quantity_input))
            .block(Block::default().borders(Borders::ALL).title("Quantity"))
            .style(qty_style);
        f.render_widget(qty_field, chunks[2]);

        // Price field
        let price_style = if app.edit_field == EditField::Price {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let price_help = if app.purchase_price_input.is_empty() {
            " (leave empty for auto-fill)"
        } else {
            ""
        };
        let price_field = Paragraph::new(format!("Price (optional): {}{}", app.purchase_price_input, price_help))
            .block(Block::default().borders(Borders::ALL).title("Price"))
            .style(price_style);
        f.render_widget(price_field, chunks[3]);

        // Help
        let help_text = Paragraph::new("Tab/↓: Next Field | ↑: Previous Field | Enter: Save | Esc: Cancel")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(help_text, chunks[5]);
    }
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        // New layout: Top section with total value, middle section with graph and pie chart, bottom with help
        let mut constraints = Vec::new();

        if !app.disabled_components.is_disabled(Component::TotalValue) {
            constraints.push(Constraint::Length(7)); // Total value display
        }

        // Main content area for graph and pie chart
        constraints.push(Constraint::Min(0));

        if !app.disabled_components.is_disabled(Component::Help) {
            constraints.push(Constraint::Length(3)); // Help
        }

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let mut chunk_index = 0;

        // Total Portfolio Value with daily PnL
        if !app.disabled_components.is_disabled(Component::TotalValue) {
            let total_value = portfolio.get_total_value();
            // Compute daily PnL and %Day on securities only (exclude cash)
            let mut prev_sec_sum = 0.0_f64;
            let mut sec_value_sum = 0.0_f64;
            for position in &portfolio.positions {
                let is_cash = position.get_ticker().is_none()
                    && position.get_asset_class().to_lowercase() == "cash";
                if is_cash { continue; }
                let value = position.get_balance();
                sec_value_sum += value;
                let prev = position.daily_variation_percent().map(|dv| {
                    let ratio = dv / 100.0;
                    if (1.0 + ratio).abs() > f64::EPSILON { value / (1.0 + ratio) } else { value }
                });
                prev_sec_sum += prev.unwrap_or(value);
            }
            let day_pnl_abs = sec_value_sum - prev_sec_sum;
            let daily_percent = if prev_sec_sum > 0.0 {
                (sec_value_sum - prev_sec_sum) / prev_sec_sum * 100.0
            } else { 0.0 };
            let big_text_value = match app.currency.as_str() {
                "USD" | "CAD" | "AUD" | "HKD" | "SGD" => {
                    format!("${}", format_with_commas(total_value))
                }
                "EUR" => format!("{} EUR", format_with_commas(total_value)),
                "GBP" => format!("£{}", format_with_commas(total_value)),
                "JPY" => {
                    let integer_value = total_value as i64;
                    let formatted = format!("{integer_value}");
                    let formatted_with_commas = formatted
                        .chars()
                        .rev()
                        .collect::<String>()
                        .chars()
                        .collect::<Vec<_>>()
                        .chunks(3)
                        .map(|chunk| chunk.iter().collect::<String>())
                        .collect::<Vec<_>>()
                        .join(",")
                        .chars()
                        .rev()
                        .collect::<String>();
                    format!("{formatted_with_commas} JPY")
                }
                "CHF" => format!("{} CHF", format_with_commas(total_value)),
                _ => format!("{} {}", format_with_commas(total_value), app.currency),
            };

            let big_text = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .lines(vec![big_text_value.clone().into()])
                .build();

            let refresh_indicator = if app.flash_state { "🔄" } else { "📊" };
            let network_indicator = match app.network_status {
                NetworkStatus::Connected => "🟢",
                NetworkStatus::Partial => "🟡",
                NetworkStatus::Disconnected => "🔴",
            };
            let big_text_widget = Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    "Total Portfolio Value ({}) {} {}",
                    app.currency,
                    refresh_indicator,
                    network_indicator
                ))
                .title_alignment(Alignment::Center);

            f.render_widget(big_text_widget, main_chunks[chunk_index]);

            let inner = main_chunks[chunk_index].inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 1,
            });
            // Center big text across the full inner area
            let big_text_width = big_text_value.len() as u16 * 4;
            let available_width = inner.width;
            let centered_area = if big_text_width < available_width {
                let margin = (available_width - big_text_width) / 2;
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(margin),
                        Constraint::Min(0),
                        Constraint::Length(margin),
                    ])
                    .split(inner)[1]
            } else {
                inner
            };
            f.render_widget(big_text, centered_area);

            let thirds = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
                .split(inner);
            let right_area = thirds[1];

            let pnl_color = if day_pnl_abs >= 0.0 { Color::Green } else { Color::Red };
            let pct_color = if daily_percent >= 0.0 { Color::Green } else { Color::Red };

            let right_content = vec![
                Line::from(vec![
                    Span::styled("Day PnL ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        format_currency(day_pnl_abs, &app.currency),
                        Style::default().fg(pnl_color).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("%Day ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{:+.2}%", daily_percent),
                        Style::default().fg(pct_color).add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];
            // Vertically center inside the right third, and horizontally center the block while left-aligning text
            let content_lines = 2u16; // two lines: Day PnL and %Day
            let vpad = right_area.height.saturating_sub(content_lines).saturating_div(2);
            let vchunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(vpad),
                    Constraint::Length(content_lines),
                    Constraint::Min(0),
                ])
                .split(right_area);

            // Estimate content width to center the block horizontally
            let day_value_str = format_currency(day_pnl_abs, &app.currency);
            let pct_value_str = format!("{:+.2}%", daily_percent);
            let day_line_text = format!("Day PnL {day_value_str}");
            let pct_line_text = format!("%Day {pct_value_str}");
            let max_w = day_line_text.chars().count().max(pct_line_text.chars().count()) as u16;
            let hpad = vchunks[1].width.saturating_sub(max_w).saturating_div(2);
            let hchunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(hpad),
                    Constraint::Length(max_w),
                    Constraint::Min(0),
                ])
                .split(vchunks[1]);

            let right_paragraph = Paragraph::new(right_content).alignment(Alignment::Left);
            f.render_widget(right_paragraph, hchunks[1]);
            chunk_index += 1;
        }

        // Main content area: Historic graph on top, pie chart and allocation on bottom
        let content_area = main_chunks[chunk_index];
        let content_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(content_area);

        // Historic graph
        render_historic_graph(f, content_chunks[0], portfolio, app);

        // Bottom section: Pie chart on left, detailed allocation on right
        let bottom_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(content_chunks[1]);

        render_detailed_allocation_positions(f, bottom_chunks[0], portfolio);
        render_asset_breakdown_grouped(f, bottom_chunks[1], portfolio, app);

            chunk_index += 1;

        // Help text
        if !app.disabled_components.is_disabled(Component::Help) {
            let help_text = Paragraph::new("Navigation: h/l (tabs) | j/k (select in Balances) | e (edit in Balances) | r (refresh) | 1-2 (direct) | q (quit)")
                .block(Block::default().borders(Borders::ALL).title("Help"))
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center);

            f.render_widget(help_text, main_chunks[chunk_index]);
        }
    } else {
        render_loading(f, area);
    }
}

fn render_balances(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        // Build header cells based on disabled components
        let mut header_names = Vec::new();
        let mut constraints = Vec::new();

        if !app.disabled_components.is_disabled(Component::Name) {
            header_names.push("Name");
            constraints.push(Constraint::Length(22));
        }
        if !app.disabled_components.is_disabled(Component::AssetClass) {
            header_names.push("Class");
            constraints.push(Constraint::Length(10));
        }
        if !app.disabled_components.is_disabled(Component::Amount) {
            header_names.push("Amt");
            constraints.push(Constraint::Length(8));
        }
        if !app.disabled_components.is_disabled(Component::Price) {
            header_names.push("Price");
            constraints.push(Constraint::Length(10));
        }
        if !app.disabled_components.is_disabled(Component::AvgCost) {
            header_names.push("Avg");
            constraints.push(Constraint::Length(10));
        }
        if !app.disabled_components.is_disabled(Component::Invested) {
            header_names.push("Invested");
            constraints.push(Constraint::Length(12));
        }
        if !app.disabled_components.is_disabled(Component::Balance) {
            header_names.push("Value");
            constraints.push(Constraint::Length(12));
        }
        if !app.disabled_components.is_disabled(Component::PnL) {
            header_names.push("PnL");
            constraints.push(Constraint::Length(12));
        }
        if !app.disabled_components.is_disabled(Component::Hist) {
            header_names.push("%Hist");
            constraints.push(Constraint::Length(7));
        }
        if !app.disabled_components.is_disabled(Component::Daily) {
            header_names.push("%Day");
            constraints.push(Constraint::Length(7));
        }
        

        // If all columns are disabled, show a placeholder
        if header_names.is_empty() {
            let placeholder = Paragraph::new("All balance columns are disabled")
                .block(Block::default().borders(Borders::ALL).title("Balances"))
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center);
            f.render_widget(placeholder, area);
            return;
        }

        let header_cells = header_names.iter().map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        });
        let header = Row::new(header_cells).height(1).bottom_margin(1);

        let rows = portfolio.positions.iter().enumerate().map(|(i, position)| {
            let name = position.get_name();
            let balance_color = app.get_trend_color(name, Color::White);

            // Highlight selected row
            let row_style = if i == app.selected_position && app.current_tab == Tab::Balances {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            // Build row cells based on disabled components
            let mut cells = Vec::new();

            if !app.disabled_components.is_disabled(Component::Name) {
                // Add indicator for positions with tickers (live data) vs static positions
                let name_with_indicator = if position.get_ticker().is_some() {
                    format!("● {}", position.get_name()) // Live data indicator
                } else {
                    format!("○ {}", position.get_name()) // Static data indicator
                };
                cells.push(
                    Cell::from(name_with_indicator).style(Style::default().fg(balance_color)),
                );
            }

            if !app.disabled_components.is_disabled(Component::AssetClass) {
                cells.push(
                    Cell::from(position.get_asset_class())
                        .style(Style::default().fg(balance_color)),
                );
            }

            if !app.disabled_components.is_disabled(Component::Amount) {
                cells.push(
                    Cell::from(format_amount(position.get_amount()))
                        .style(Style::default().fg(balance_color)),
                );
            }

            // Check if this is a cash position (no ticker and cash asset class)
            let is_cash = position.get_ticker().is_none() && 
                         position.get_asset_class().to_lowercase() == "cash";

            if !app.disabled_components.is_disabled(Component::Price) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    cells.push(
                        Cell::from(format!("{:.2}", position.market_price()))
                            .style(Style::default().fg(balance_color)),
                    );
                }
            }
            if !app.disabled_components.is_disabled(Component::AvgCost) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    let s = position
                        .average_cost()
                        .map(|v| format!("{v:.2}"))
                        .unwrap_or_else(|| "-".to_string());
                    cells.push(Cell::from(s).style(Style::default().fg(balance_color)));
                }
            }
            if !app.disabled_components.is_disabled(Component::Invested) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    let s = position
                        .total_invested()
                        .map(|v| format!("{v:.2}"))
                        .unwrap_or_else(|| "-".to_string());
                    cells.push(Cell::from(s).style(Style::default().fg(balance_color)));
                }
            }
            if !app.disabled_components.is_disabled(Component::Balance) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    cells.push(
                        Cell::from(format_currency(position.get_balance(), &app.currency))
                            .style(Style::default().fg(balance_color)),
                    );
                }
            }
            if !app.disabled_components.is_disabled(Component::PnL) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    let pnl_cell = match position.pnl() {
                        Some(v) => {
                            let color = if v >= 0.0 { Color::Green } else { Color::Red };
                            Cell::from(format!("{v:.2}")).style(Style::default().fg(color))
                        }
                        None => Cell::from("-").style(Style::default().fg(balance_color)),
                    };
                    cells.push(pnl_cell);
                }
            }
            if !app.disabled_components.is_disabled(Component::Hist) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    let hist_cell = match position.historic_variation_percent() {
                        Some(v) => {
                            let color = if v >= 0.0 { Color::Green } else { Color::Red };
                            Cell::from(format!("{v:.2}%")).style(Style::default().fg(color))
                        }
                        None => Cell::from("-").style(Style::default().fg(balance_color)),
                    };
                    cells.push(hist_cell);
                }
            }
            if !app.disabled_components.is_disabled(Component::Daily) {
                if is_cash {
                    cells.push(Cell::from("-").style(Style::default().fg(balance_color)));
                } else {
                    let day_cell = match position.daily_variation_percent() {
                        Some(v) => {
                            let color = if v >= 0.0 { Color::Green } else { Color::Red };
                            Cell::from(format!("{v:.2}%")).style(Style::default().fg(color))
                        }
                        None => Cell::from("-").style(Style::default().fg(balance_color)),
                    };
                    cells.push(day_cell);
                }
            }

            Row::new(cells).height(1).style(row_style)
        });

        // Build total row
        let total_value = portfolio.get_total_value();
        let mut total_cells = Vec::new();

        if !app.disabled_components.is_disabled(Component::Name) {
            total_cells.push(
                Cell::from("TOTAL").style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        }

        if !app.disabled_components.is_disabled(Component::AssetClass) {
            total_cells.push(Cell::from(""));
        }

        if !app.disabled_components.is_disabled(Component::Amount) {
            total_cells.push(Cell::from(""));
        }

        if !app.disabled_components.is_disabled(Component::Price) {
            total_cells.push(Cell::from(""));
        }
        if !app.disabled_components.is_disabled(Component::AvgCost) {
            total_cells.push(Cell::from(""));
        }
        if !app.disabled_components.is_disabled(Component::Invested) {
            // Sum invested where available
            let mut invested_sum = 0.0_f64;
            for p in &portfolio.positions {
                if let Some(i) = p.total_invested() {
                    invested_sum += i;
                }
            }
            total_cells.push(Cell::from(format!("{invested_sum:.2}")));
        }
        if !app.disabled_components.is_disabled(Component::Balance) {
            total_cells.push(
                Cell::from(format_currency(total_value, &app.currency)).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        }
        if !app.disabled_components.is_disabled(Component::PnL) {
            // Securities-only PnL: exclude cash from value side
            let mut invested_sum = 0.0_f64;
            for p in &portfolio.positions {
                if let Some(i) = p.total_invested() { invested_sum += i; }
            }
            let securities_value: f64 = portfolio.positions.iter().filter(|p| !(p.get_ticker().is_none() && p.get_asset_class().to_lowercase()=="cash")).map(|p| p.get_balance()).sum();
            let pnl_total = securities_value - invested_sum;
            let color = if pnl_total >= 0.0 { Color::Green } else { Color::Red };
            total_cells.push(
                Cell::from(format!("{pnl_total:.2}"))
                    .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
            );
        }
        if !app.disabled_components.is_disabled(Component::Hist) {
            let mut invested_sum = 0.0_f64;
            for p in &portfolio.positions {
                if let Some(i) = p.total_invested() { invested_sum += i; }
            }
            let securities_value: f64 = portfolio.positions.iter().filter(|p| !(p.get_ticker().is_none() && p.get_asset_class().to_lowercase()=="cash")).map(|p| p.get_balance()).sum();
            let hist_pct = if invested_sum > 0.0 {
                (securities_value - invested_sum) / invested_sum * 100.0
            } else { 0.0 };
            let color = if hist_pct >= 0.0 { Color::Green } else { Color::Red };
            total_cells.push(
                Cell::from(format!("{hist_pct:.2}%"))
                    .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
            );
        }
        if !app.disabled_components.is_disabled(Component::Daily) {
            // Calculate total daily variation for securities only
            let mut prev_sec_sum = 0.0_f64;
            let mut sec_value_sum = 0.0_f64;
            for position in &portfolio.positions {
                let is_cash = position.get_ticker().is_none() && position.get_asset_class().to_lowercase()=="cash";
                if is_cash { continue; }
                let value = position.get_balance();
                sec_value_sum += value;
                let day_var = position.daily_variation_percent();
                let prev_value_for_position = match day_var {
                    Some(dv) => {
                        let ratio = dv / 100.0;
                        if (1.0 + ratio).abs() > f64::EPSILON { value / (1.0 + ratio) } else { value }
                    }
                    None => value,
                };
                prev_sec_sum += prev_value_for_position;
            }
            let total_day_var = if prev_sec_sum > 0.0 { (sec_value_sum - prev_sec_sum) / prev_sec_sum * 100.0 } else { 0.0 };
            let color = if total_day_var >= 0.0 { Color::Green } else { Color::Red };
            total_cells.push(
                Cell::from(format!("{total_day_var:.2}%"))
                    .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
            );
        }

        let total_row = Row::new(total_cells).height(1);

        let help_text = match app.mode {
            AppMode::Normal => "Navigation: j/k (select) | e (edit) | h/l (tabs) | r (refresh) | q (quit)",
            AppMode::Edit => "Edit Mode: Enter (save) | Esc (cancel)",
            AppMode::PurchaseList => "Purchase List: j/k (select) | Enter/a (add) | Esc (back)",
            AppMode::AddPurchase => "Add Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
            AppMode::EditPurchase => "Edit Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
        };

        let table_title = format!("Portfolio Balances - {help_text}");

        let table = Table::new(rows.chain(std::iter::once(total_row)), constraints)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(table_title))
            .style(Style::default().fg(Color::White));

        f.render_widget(table, area);

        // Render edit dialog if in edit mode
        if app.mode == AppMode::Edit {
            render_edit_dialog(f, app);
        }
    } else {
        render_loading(f, area);
    }
}

fn render_loading(f: &mut Frame, area: Rect) {
    let loading_text = Paragraph::new("Loading portfolio data...")
        .block(Block::default().borders(Borders::ALL).title("Loading"))
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);

    f.render_widget(loading_text, area);
}

fn render_edit_dialog(f: &mut Frame, app: &App) {
    let popup_area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, popup_area);

    if let Some(portfolio) = &app.portfolio {
        if app.selected_position < portfolio.positions.len() {
            let position = &portfolio.positions[app.selected_position];

            // Create main layout for the popup
            let popup_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Title section
                    Constraint::Length(4), // Current value section
                    Constraint::Length(4), // Input section
                    Constraint::Length(3), // Instructions
                ])
                .margin(1)
                .split(popup_area);

            // Main border
            let main_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Edit Position Amount ")
                .title_alignment(Alignment::Center)
                .style(Style::default().bg(Color::Black));
            f.render_widget(main_block, popup_area);

            // Position name and asset class
            let position_info = format!(
                "Position: {} ({})",
                position.get_name(),
                position.get_asset_class()
            );
            let info_paragraph = Paragraph::new(position_info)
                .style(
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(info_paragraph, popup_layout[0]);

            // Current value display with smart decimal formatting
            let current_value = format!("Current Amount: {}", format_amount(position.get_amount()));
            let current_balance = format!(
                "Current Balance: {}",
                format_currency(position.get_balance(), &app.currency)
            );
            let current_text = format!("{current_value}\n{current_balance}");

            let current_paragraph = Paragraph::new(current_text)
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Gray))
                        .title(" Current "),
                );
            f.render_widget(current_paragraph, popup_layout[1]);

            // Input field with cursor
            let input_display = if app.edit_input.is_empty() {
                "".to_string()
            } else {
                app.edit_input.clone()
            };

            // Add blinking cursor indicator
            let cursor = if app.flash_state { "█" } else { "▌" };
            let input_with_cursor = format!("{input_display}{cursor}");

            // Calculate new balance preview if input is valid
            let (preview_text, input_style) = if app.edit_input.is_empty() {
                (
                    "Enter amount...".to_string(),
                    Style::default().fg(Color::Gray),
                )
            } else if let Ok(new_amount) = app.edit_input.parse::<f64>() {
                if new_amount < 0.0 {
                    (
                        "Amount cannot be negative".to_string(),
                        Style::default().fg(Color::Red),
                    )
                } else {
                    let new_balance = if position.get_ticker().is_some() {
                        // For positions with tickers, calculate balance using last spot price
                        let last_spot = position.get_balance() / position.get_amount();
                        new_amount * last_spot
                    } else {
                        // For cash positions, amount equals balance
                        new_amount
                    };

                    let preview = format!(
                        "New Balance: {}",
                        format_currency(new_balance, &app.currency)
                    );
                    (preview, Style::default().fg(Color::Green))
                }
            } else {
                // Check if it's a valid intermediate state (like "1." or "0.")
                let trimmed = app.edit_input.trim();
                if trimmed.ends_with('.') && trimmed.len() > 1 {
                    if trimmed[..trimmed.len() - 1].parse::<f64>().is_ok() {
                        // Valid intermediate state like "1." or "123."
                        (
                            "Enter decimal places...".to_string(),
                            Style::default().fg(Color::Yellow),
                        )
                    } else {
                        (
                            "Invalid number format".to_string(),
                            Style::default().fg(Color::Red),
                        )
                    }
                } else if trimmed == "." {
                    // Just a dot, waiting for digits
                    (
                        "Enter digits...".to_string(),
                        Style::default().fg(Color::Yellow),
                    )
                } else {
                    (
                        "Invalid number format".to_string(),
                        Style::default().fg(Color::Red),
                    )
                }
            };

            // Split input area into input field and preview
            let input_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)])
                .split(popup_layout[2]);

            // Input field
            let input_field = Paragraph::new(input_with_cursor)
                .style(
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow))
                        .title(" New Amount "),
                );
            f.render_widget(input_field, input_chunks[0]);

            // Preview area
            let preview_paragraph = Paragraph::new(preview_text)
                .style(input_style)
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(preview_paragraph, input_chunks[1]);

            // Instructions
            let instructions = "Enter: Save Changes | Esc: Cancel | Type numbers and decimal point";
            let instructions_paragraph = Paragraph::new(instructions)
                .style(Style::default().fg(Color::Cyan))
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(instructions_paragraph, popup_layout[3]);
        }
    }
}

fn render_error_popup(f: &mut Frame, error: &str) {
    let popup_area = centered_rect(60, 20, f.area());
    f.render_widget(Clear, popup_area);

    let error_paragraph = Paragraph::new(error)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Error")
                .style(Style::default().fg(Color::Red)),
        )
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });

    f.render_widget(error_paragraph, popup_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_asset_breakdown_grouped(f: &mut Frame, area: Rect, portfolio: &Portfolio, app: &App) {
    let allocation = portfolio.get_allocation();
    let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
    allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

    // Detailed allocation list
    let detailed_list: Vec<ListItem> = allocation_vec
        .iter()
        .map(|(asset_class, percentage)| {
            // Find a position with this asset class to get trend color
            let trend_color = portfolio
                .positions
                .iter()
                .find(|p| p.get_asset_class() == *asset_class)
                .map(|p| app.get_trend_color(p.get_name(), Color::Cyan))
                .unwrap_or(Color::Cyan);

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{asset_class:<15}"),
                    Style::default().fg(trend_color),
                ),
                Span::styled(
                    format!("{percentage:>8.2}%"),
                    Style::default().fg(trend_color),
                ),
            ]))
        })
        .collect();

    let list = List::new(detailed_list)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Asset Breakdown"),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(list, area);
}

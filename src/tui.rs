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
        BarChart, Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Tabs, Wrap,
    },
    Frame, Terminal,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tui_big_text::{BigText, PixelSize};

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
    pub fn new(disabled_list: Vec<String>) -> Self {
        let mut disabled = HashSet::new();

        for component_str in disabled_list {
            match Component::from_str(&component_str) {
                Ok(component) => {
                    disabled.insert(component);
                }
                Err(err) => eprintln!("Warning: {err}"),
            }
        }

        DisabledComponents { disabled }
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
            format!("¥{formatted_with_commas}")
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Overview,
    Balances,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppMode {
    Normal,
    Edit,
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
    pub edit_input: String,
    pub data_file_path: String,
    pub portfolio_receiver: Option<mpsc::UnboundedReceiver<(Portfolio, NetworkStatus)>>,
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
            edit_input: String::new(),
            data_file_path,
            portfolio_receiver: None,
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

    pub fn try_receive_portfolio_update(&mut self) -> bool {
        if let Some(receiver) = &mut self.portfolio_receiver {
            if let Ok((portfolio, network_status)) = receiver.try_recv() {
                self.update_trends(&portfolio);
                self.set_portfolio(portfolio);
                self.network_status = network_status;
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
                self.mode = AppMode::Edit;
                // Start with the raw number as string to preserve user input format
                let amount = portfolio.positions[self.selected_position].get_amount();
                // Convert to string preserving reasonable precision, but allow user to modify
                self.edit_input = if amount.fract() == 0.0 {
                    format!("{}", amount as i64)
                } else {
                    format!("{amount}")
                };
            }
        }
    }

    pub fn exit_edit_mode(&mut self) {
        self.mode = AppMode::Normal;
        self.edit_input.clear();
    }

    pub fn save_edit(&mut self) -> Result<(), String> {
        if let Some(portfolio) = &mut self.portfolio {
            if self.selected_position < portfolio.positions.len() {
                match self.edit_input.parse::<f64>() {
                    Ok(new_amount) => {
                        if new_amount >= 0.0 {
                            // Update the position amount
                            portfolio.positions[self.selected_position].set_amount(new_amount);

                            // Save to file
                            self.save_to_file()?;

                            self.exit_edit_mode();
                            Ok(())
                        } else {
                            Err("Amount must be non-negative".to_string())
                        }
                    }
                    Err(_) => Err("Invalid number format".to_string()),
                }
            } else {
                Err("Invalid position selected".to_string())
            }
        } else {
            Err("No portfolio loaded".to_string())
        }
    }

    fn save_to_file(&self) -> Result<(), String> {
        if let Some(portfolio) = &self.portfolio {
            let positions_data: Vec<serde_json::Value> = portfolio
                .positions
                .iter()
                .map(|pos| {
                    let mut obj = serde_json::Map::new();

                    // Only include Name if it exists and is different from ticker
                    if let Some(name) = pos.get_name_option() {
                        // Only add Name field if it's explicitly set (not derived from ticker)
                        if pos.get_ticker().is_none() || Some(name) != pos.get_ticker() {
                            obj.insert(
                                "Name".to_string(),
                                serde_json::Value::String(name.to_string()),
                            );
                        }
                    }

                    // Always include Ticker if it exists
                    if let Some(ticker) = pos.get_ticker() {
                        obj.insert(
                            "Ticker".to_string(),
                            serde_json::Value::String(ticker.to_string()),
                        );
                    }

                    obj.insert(
                        "AssetClass".to_string(),
                        serde_json::Value::String(pos.get_asset_class().to_string()),
                    );
                    obj.insert(
                        "Amount".to_string(),
                        serde_json::Value::Number(
                            serde_json::Number::from_f64(pos.get_amount())
                                .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap()),
                        ),
                    );

                    serde_json::Value::Object(obj)
                })
                .collect();

            let json_string = serde_json::to_string_pretty(&positions_data)
                .map_err(|e| format!("Failed to serialize data: {e}"))?;

            std::fs::write(&self.data_file_path, json_string)
                .map_err(|e| format!("Failed to write to file: {e}"))?;
        }
        Ok(())
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
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let disabled = DisabledComponents::new(disabled_components);
    let mut app = App::new(currency, positions_str.clone(), data_file_path, disabled);
    app.set_portfolio(portfolio);
    if let Some(tab) = tab {
        app.current_tab = tab;
    }

    // Create channel for background portfolio updates
    let (portfolio_sender, portfolio_receiver) = mpsc::unbounded_channel();
    app.set_portfolio_receiver(portfolio_receiver);

    // Spawn background task for portfolio updates
    let positions_str_bg = positions_str.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5)); // Update every 5 seconds instead of 1
        loop {
            interval.tick().await;
            let (portfolio, network_status) =
                crate::create_live_portfolio(positions_str_bg.clone()).await;
            if portfolio_sender.send((portfolio, network_status)).is_err() {
                break; // Channel closed, exit task
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
                                KeyCode::BackTab => {
                                    app.previous_tab();
                                }
                                KeyCode::Char('1') => app.current_tab = Tab::Overview,
                                KeyCode::Char('2') => app.current_tab = Tab::Balances,
                                _ => {}
                            }
                        }
                        AppMode::Edit => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.exit_edit_mode();
                                }
                                KeyCode::Enter => {
                                    match app.save_edit() {
                                        Ok(()) => {
                                            // Update positions_str with new data from file
                                            if let Ok(new_positions_str) =
                                                std::fs::read_to_string(&app.data_file_path)
                                            {
                                                app.positions_str = new_positions_str;
                                            }
                                            // Portfolio will be refreshed by background task
                                        }
                                        Err(e) => {
                                            app.error_message = Some(e);
                                            app.exit_edit_mode();
                                        }
                                    }
                                }
                                KeyCode::Backspace => {
                                    app.edit_input.pop();
                                }
                                KeyCode::Char(c) => {
                                    if c.is_ascii_digit()
                                        || (c == '.' && !app.edit_input.contains('.'))
                                    {
                                        app.edit_input.push(c);
                                    }
                                }
                                _ => {}
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
        Tab::Balances => render_balances(f, content_area, app),
    }

    if let Some(error) = &app.error_message {
        render_error_popup(f, error);
    }
}

fn render_asset_allocation(f: &mut Frame, area: Rect, portfolio: &Portfolio) {
    let allocation = portfolio.get_allocation();
    let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
    allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

    // Bar chart for visual allocation
    let data: Vec<(&str, u64)> = allocation_vec
        .iter()
        .map(|(name, percentage)| (name.as_str(), **percentage as u64))
        .collect();

    let barchart = BarChart::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Asset Allocation"),
        )
        .data(&data)
        .bar_width(9)
        .bar_style(Style::default().fg(Color::Yellow))
        .value_style(Style::default().fg(Color::Black).bg(Color::Yellow));

    f.render_widget(barchart, area);
}

fn render_detailed_allocation(f: &mut Frame, area: Rect, portfolio: &Portfolio, app: &App) {
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
                .title("Detailed Allocation"),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(list, area);
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        // Calculate constraints based on disabled components
        let mut constraints = Vec::new();

        if !app.disabled_components.is_disabled(Component::TotalValue) {
            constraints.push(Constraint::Length(7)); // Total value display
        }

        if !app
            .disabled_components
            .is_disabled(Component::AssetAllocation)
            || !app
                .disabled_components
                .is_disabled(Component::DetailedAllocation)
        {
            constraints.push(Constraint::Min(0)); // Allocation section
        }

        if !app.disabled_components.is_disabled(Component::Help) {
            constraints.push(Constraint::Length(3)); // Help
        }

        // If all components are disabled, show a placeholder
        if constraints.is_empty() {
            let placeholder = Paragraph::new("All overview components are disabled")
                .block(Block::default().borders(Borders::ALL).title("Overview"))
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center);
            f.render_widget(placeholder, area);
            return;
        }

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let mut chunk_index = 0;

        // Total Portfolio Value
        if !app.disabled_components.is_disabled(Component::TotalValue) {
            let total_value = portfolio.get_total_value();
            // Create formatted currency for big text with full accuracy
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
                    app.currency, refresh_indicator, network_indicator
                ))
                .title_alignment(Alignment::Center);

            f.render_widget(big_text_widget, main_chunks[chunk_index]);

            // Center the big text within the widget
            let inner = main_chunks[chunk_index].inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 1,
            });
            let big_text_width = big_text_value.len() as u16 * 4; // Approximate width per character in big text
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
            chunk_index += 1;
        }

        // Allocation section: bar chart on left, detailed list on right
        if !app
            .disabled_components
            .is_disabled(Component::AssetAllocation)
            || !app
                .disabled_components
                .is_disabled(Component::DetailedAllocation)
        {
            let allocation_area = main_chunks[chunk_index];

            if app
                .disabled_components
                .is_disabled(Component::AssetAllocation)
                && !app
                    .disabled_components
                    .is_disabled(Component::DetailedAllocation)
            {
                // Only show detailed allocation (full width)
                render_detailed_allocation(f, allocation_area, portfolio, app);
            } else if !app
                .disabled_components
                .is_disabled(Component::AssetAllocation)
                && app
                    .disabled_components
                    .is_disabled(Component::DetailedAllocation)
            {
                // Only show asset allocation bar chart (full width)
                render_asset_allocation(f, allocation_area, portfolio);
            } else {
                // Show both (split horizontally)
                let allocation_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(allocation_area);

                render_asset_allocation(f, allocation_chunks[0], portfolio);
                render_detailed_allocation(f, allocation_chunks[1], portfolio, app);
            }
            chunk_index += 1;
        }

        // Help text
        if !app.disabled_components.is_disabled(Component::Help) {
            let help_text = Paragraph::new("Navigation: h/l (tabs) | j/k (select in Balances) | e (edit in Balances) | 1-2 (direct) | q (quit)")
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
            constraints.push(Constraint::Percentage(30));
        }
        if !app.disabled_components.is_disabled(Component::AssetClass) {
            header_names.push("Asset Class");
            constraints.push(Constraint::Percentage(20));
        }
        if !app.disabled_components.is_disabled(Component::Amount) {
            header_names.push("Amount");
            constraints.push(Constraint::Percentage(20));
        }
        if !app.disabled_components.is_disabled(Component::Balance) {
            header_names.push("Balance");
            constraints.push(Constraint::Percentage(30));
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

            if !app.disabled_components.is_disabled(Component::Balance) {
                cells.push(
                    Cell::from(format_currency(position.get_balance(), &app.currency))
                        .style(Style::default().fg(balance_color)),
                );
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

        if !app.disabled_components.is_disabled(Component::Balance) {
            total_cells.push(
                Cell::from(format_currency(total_value, &app.currency)).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        }

        let total_row = Row::new(total_cells).height(1);

        let help_text = match app.mode {
            AppMode::Normal => "Navigation: j/k (select) | e (edit) | h/l (tabs) | q (quit)",
            AppMode::Edit => "Edit Mode: Enter (save) | Esc (cancel)",
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

use crate::portfolio::Portfolio;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        BarChart, Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table,
        Tabs, Wrap,
    },
    Frame, Terminal,
};
use std::io;
use tui_big_text::{BigText, PixelSize};

fn format_currency(value: f64, currency: &str) -> String {
    let formatted_number = if value >= 1000.0 {
        format_with_commas(value)
    } else {
        match currency {
            "JPY" => format!("{:.0}", value),
            _ => format!("{:.2}", value),
        }
    };
    
    match currency {
        "USD" | "CAD" | "AUD" | "HKD" | "SGD" => format!("${}", formatted_number),
        "EUR" => format!("{} €", formatted_number),
        "GBP" => format!("£{}", formatted_number),
        "JPY" => {
            let integer_value = value as i64;
            let formatted = format!("{}", integer_value);
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
            format!("¥{}", formatted_with_commas)
        },
        "CHF" => format!("{} CHF", formatted_number),
        "SEK" | "NOK" | "DKK" => format!("{} {}", formatted_number, currency),
        _ => format!("{} {}", formatted_number, currency),
    }
}

fn format_with_commas(value: f64) -> String {
    let formatted = format!("{:.2}", value);
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
    
    format!("{}.{}", formatted_integer, decimal_part)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Overview,
    Balances,
    Performance,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview & Allocation",
            Tab::Balances => "Balances",
            Tab::Performance => "Performance",
        }
    }

    fn all() -> &'static [Tab] {
        &[Tab::Overview, Tab::Balances, Tab::Performance]
    }
}

pub struct App {
    pub current_tab: Tab,
    pub portfolio: Option<Portfolio>,
    pub should_quit: bool,
    pub loading: bool,
    pub error_message: Option<String>,
    pub performance_data: Option<PerformanceData>,
    pub currency: String,
    pub previous_values: HashMap<String, f64>,
    pub trends: HashMap<String, Trend>,
    pub last_update: Instant,
    pub flash_state: bool,
    pub positions_str: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trend {
    Up,
    Down,
    Neutral,
}

#[derive(Debug, Clone)]
pub struct PerformanceData {
    pub ytd: Option<f64>,
    pub monthly: Option<f64>,
    pub recent: Option<f64>,
}

impl App {
    pub fn new(currency: String, positions_str: String) -> App {
        App {
            current_tab: Tab::Overview,
            portfolio: None,
            should_quit: false,
            loading: false,
            error_message: None,
            performance_data: None,
            currency,
            previous_values: HashMap::new(),
            trends: HashMap::new(),
            last_update: Instant::now(),
            flash_state: false,
            positions_str,
        }
    }

    pub fn set_portfolio(&mut self, portfolio: Portfolio) {
        self.portfolio = Some(portfolio);
        self.loading = false;
    }

    pub fn set_error(&mut self, error: String) {
        self.error_message = Some(error);
        self.loading = false;
    }

    pub fn set_performance_data(&mut self, data: PerformanceData) {
        self.performance_data = Some(data);
    }

    pub fn next_tab(&mut self) {
        let tabs = Tab::all();
        let current_index = tabs.iter().position(|&t| t == self.current_tab).unwrap_or(0);
        self.current_tab = tabs[(current_index + 1) % tabs.len()];
    }

    pub fn previous_tab(&mut self) {
        let tabs = Tab::all();
        let current_index = tabs.iter().position(|&t| t == self.current_tab).unwrap_or(0);
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

    pub fn should_refresh(&self) -> bool {
        self.last_update.elapsed() >= Duration::from_secs(1)
    }

    pub fn mark_refreshed(&mut self) {
        self.last_update = Instant::now();
        self.flash_state = !self.flash_state; // Toggle flash state for animation
    }

    pub fn get_trend_color(&self, name: &str, base_color: Color) -> Color {
        match self.trends.get(name) {
            Some(Trend::Up) => if self.flash_state { Color::LightGreen } else { Color::Green },
            Some(Trend::Down) => if self.flash_state { Color::LightRed } else { Color::Red },
            _ => base_color,
        }
    }
}

pub async fn run_tui(portfolio: Portfolio, currency: String, positions_str: String) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(currency, positions_str);
    app.set_portfolio(portfolio);

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

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        // Check if we should refresh data
        if app.should_refresh() {
            let positions_str = app.positions_str.clone();
            let portfolio = crate::create_live_portfolio(positions_str).await;
            app.update_trends(&portfolio);
            app.set_portfolio(portfolio);
            app.mark_refreshed();
        }

        // Use poll to check for events with timeout
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
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
                            // Could add scrolling here if needed
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            // Could add scrolling here if needed
                        }
                        KeyCode::BackTab => {
                            app.previous_tab();
                        }
                        KeyCode::Char('1') => app.current_tab = Tab::Overview,
                        KeyCode::Char('2') => app.current_tab = Tab::Balances,
                        KeyCode::Char('3') => app.current_tab = Tab::Performance,
                        _ => {}
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(f.area());

    let tab_titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| {
            let style = if *t == app.current_tab {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(t.title(), style))
        })
        .collect();

    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).title("Portfolio TUI"))
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Yellow))
        .select(Tab::all().iter().position(|&t| t == app.current_tab).unwrap_or(0));

    f.render_widget(tabs, chunks[0]);

    match app.current_tab {
        Tab::Overview => render_overview(f, chunks[1], app),
        Tab::Balances => render_balances(f, chunks[1], app),
        Tab::Performance => render_performance(f, chunks[1], app),
    }

    if let Some(error) = &app.error_message {
        render_error_popup(f, error);
    }
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        // Main layout: top section for total value, bottom for allocation details
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),  // Total value display
                Constraint::Min(0),     // Allocation section
                Constraint::Length(3),  // Help
            ])
            .split(area);

        // Total Portfolio Value (big text display)
        let total_value = portfolio.get_total_value();
        // Create formatted currency for big text with full accuracy
        let big_text_value = match app.currency.as_str() {
            "USD" | "CAD" | "AUD" | "HKD" | "SGD" => format!("${}", format_with_commas(total_value)),
            "EUR" => format!("{} EUR", format_with_commas(total_value)),
            "GBP" => format!("£{}", format_with_commas(total_value)),
            "JPY" => {
                let integer_value = total_value as i64;
                let formatted = format!("{}", integer_value);
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
                format!("{} JPY", formatted_with_commas)
            },
            "CHF" => format!("{} CHF", format_with_commas(total_value)),
            _ => format!("{} {}", format_with_commas(total_value), app.currency),
        };
        
        let big_text = BigText::builder()
            .pixel_size(PixelSize::Quadrant)
            .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            .lines(vec![big_text_value.clone().into()])
            .build();

        let refresh_indicator = if app.flash_state { "●" } else { "○" };
        let big_text_widget = Block::default()
            .borders(Borders::ALL)
            .title(format!("Total Portfolio Value ({}) {}", app.currency, refresh_indicator))
            .title_alignment(Alignment::Center);

        f.render_widget(big_text_widget, main_chunks[0]);
        
        // Center the big text within the widget
        let inner = main_chunks[0].inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
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

        // Allocation section: bar chart on left, detailed list on right
        let allocation_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(main_chunks[1]);

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

        f.render_widget(barchart, allocation_chunks[0]);

        // Detailed allocation list
        let detailed_list: Vec<ListItem> = allocation_vec
            .iter()
            .map(|(asset_class, percentage)| {
                // Find a position with this asset class to get trend color
                let trend_color = portfolio.positions.iter()
                    .find(|p| p.get_asset_class() == *asset_class)
                    .map(|p| app.get_trend_color(p.get_name(), Color::Cyan))
                    .unwrap_or(Color::Cyan);
                
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<15}", asset_class),
                        Style::default().fg(trend_color),
                    ),
                    Span::styled(
                        format!("{:>8.2}%", percentage),
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

        f.render_widget(list, allocation_chunks[1]);

        // Help text
        let help_text = Paragraph::new("Navigation: h/l (tabs) | j/k (up/down) | 1-3 (direct) | q (quit)")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);

        f.render_widget(help_text, main_chunks[2]);
    } else {
        render_loading(f, area);
    }
}

fn render_balances(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        let header_cells = ["Name", "Asset Class", "Amount", "Balance"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        let header = Row::new(header_cells).height(1).bottom_margin(1);

        let rows = portfolio.positions.iter().map(|position| {
            let name = position.get_name();
            let balance_color = app.get_trend_color(name, Color::White);
            
            // Add indicator for positions with tickers (live data) vs static positions
            let name_with_indicator = if position.get_ticker().is_some() {
                format!("● {}", position.get_name()) // Live data indicator
            } else {
                format!("○ {}", position.get_name()) // Static data indicator
            };
            
            let cells = vec![
                Cell::from(name_with_indicator).style(Style::default().fg(balance_color)),
                Cell::from(position.get_asset_class()).style(Style::default().fg(balance_color)),
                Cell::from(format!("{:.2}", position.get_amount())).style(Style::default().fg(balance_color)),
                Cell::from(format_currency(position.get_balance(), &app.currency)).style(Style::default().fg(balance_color)),
            ];
            Row::new(cells).height(1)
        });

        let total_value = portfolio.get_total_value();
        let total_row = Row::new(vec![
            Cell::from("TOTAL").style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(format_currency(total_value, &app.currency)).style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]).height(1);

        let table = Table::new(rows.chain(std::iter::once(total_row)), [
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(30),
        ])
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Portfolio Balances"),
        )
        .style(Style::default().fg(Color::White));

        f.render_widget(table, area);
    } else {
        render_loading(f, area);
    }
}



fn render_performance(f: &mut Frame, area: Rect, app: &App) {
    if let Some(_portfolio) = &app.portfolio {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let performance_text = if let Some(perf_data) = &app.performance_data {
            let mut lines = vec![];
            
            if let Some(ytd) = perf_data.ytd {
                let color = if ytd >= 0.0 { Color::Green } else { Color::Red };
                lines.push(Line::from(vec![
                    Span::styled("YTD Performance: ", Style::default().fg(Color::White)),
                    Span::styled(format!("{:.2}%", ytd), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
            }
            
            if let Some(monthly) = perf_data.monthly {
                let color = if monthly >= 0.0 { Color::Green } else { Color::Red };
                lines.push(Line::from(vec![
                    Span::styled("Monthly Performance: ", Style::default().fg(Color::White)),
                    Span::styled(format!("{:.2}%", monthly), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
            }
            
            if let Some(recent) = perf_data.recent {
                let color = if recent >= 0.0 { Color::Green } else { Color::Red };
                lines.push(Line::from(vec![
                    Span::styled("Recent Performance: ", Style::default().fg(Color::White)),
                    Span::styled(format!("{:.2}%", recent), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                ]));
            }
            
            Text::from(lines)
        } else {
            Text::from("Loading performance data...")
        };

        let performance_paragraph = Paragraph::new(performance_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Performance Metrics"),
            )
            .style(Style::default().fg(Color::White))
            .alignment(Alignment::Left);

        f.render_widget(performance_paragraph, chunks[0]);

        let db_info = Paragraph::new("Historical data is stored locally and used for performance calculations.\nData is fetched from Yahoo Finance API.")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Data Information"),
            )
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true });

        f.render_widget(db_info, chunks[1]);
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
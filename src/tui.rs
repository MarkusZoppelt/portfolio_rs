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
    text::{Line, Span, Text},
    widgets::{
        BarChart, Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table,
        Tabs, Wrap,
    },
    Frame, Terminal,
};
use std::io;
use tui_big_text::{BigText, PixelSize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Overview,
    Balances,
    Allocation,
    Performance,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Balances => "Balances",
            Tab::Allocation => "Allocation",
            Tab::Performance => "Performance",
        }
    }

    fn all() -> &'static [Tab] {
        &[Tab::Overview, Tab::Balances, Tab::Allocation, Tab::Performance]
    }
}

pub struct App {
    pub current_tab: Tab,
    pub portfolio: Option<Portfolio>,
    pub should_quit: bool,
    pub loading: bool,
    pub error_message: Option<String>,
    pub performance_data: Option<PerformanceData>,
}

#[derive(Debug, Clone)]
pub struct PerformanceData {
    pub ytd: Option<f64>,
    pub monthly: Option<f64>,
    pub recent: Option<f64>,
}

impl App {
    pub fn new() -> App {
        App {
            current_tab: Tab::Overview,
            portfolio: None,
            should_quit: false,
            loading: false,
            error_message: None,
            performance_data: None,
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
}

pub async fn run_tui(portfolio: Portfolio) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
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

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        app.should_quit = true;
                    }
                    KeyCode::Right | KeyCode::Tab => {
                        app.next_tab();
                    }
                    KeyCode::Left | KeyCode::BackTab => {
                        app.previous_tab();
                    }
                    KeyCode::Char('1') => app.current_tab = Tab::Overview,
                    KeyCode::Char('2') => app.current_tab = Tab::Balances,
                    KeyCode::Char('3') => app.current_tab = Tab::Allocation,
                    KeyCode::Char('4') => app.current_tab = Tab::Performance,
                    _ => {}
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
        Tab::Allocation => render_allocation(f, chunks[1], app),
        Tab::Performance => render_performance(f, chunks[1], app),
    }

    if let Some(error) = &app.error_message {
        render_error_popup(f, error);
    }
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(area);

        let total_value = portfolio.get_total_value();
        let big_text = BigText::builder()
            .pixel_size(PixelSize::Quadrant)
            .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            .lines(vec![format!("${:.2}", total_value).into()])
            .build();

        let big_text_widget = Block::default()
            .borders(Borders::ALL)
            .title("Total Portfolio Value")
            .title_alignment(Alignment::Center);

        f.render_widget(big_text_widget, chunks[0]);
        
        let inner = chunks[0].inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        f.render_widget(big_text, inner);

        let allocation = portfolio.get_allocation();
        let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
        allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

        let allocation_items: Vec<ListItem> = allocation_vec
            .iter()
            .take(5)
            .map(|(asset_class, percentage)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<15}", asset_class),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:>8.2}%", percentage),
                        Style::default().fg(Color::Yellow),
                    ),
                ]))
            })
            .collect();

        let allocation_list = List::new(allocation_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Top Asset Classes"),
            )
            .style(Style::default().fg(Color::White));

        f.render_widget(allocation_list, chunks[1]);

        let help_text = Paragraph::new("Navigation: Tab/← → to switch tabs | 1-4 for direct tab access | q/Esc to quit")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);

        f.render_widget(help_text, chunks[2]);
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
            let cells = vec![
                Cell::from(position.get_name()),
                Cell::from(position.get_asset_class()),
                Cell::from(format!("{:.2}", position.get_amount())),
                Cell::from(format!("${:.2}", position.get_balance())),
            ];
            Row::new(cells).height(1)
        });

        let total_value = portfolio.get_total_value();
        let total_row = Row::new(vec![
            Cell::from("TOTAL").style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(format!("${:.2}", total_value)).style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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

fn render_allocation(f: &mut Frame, area: Rect, app: &App) {
    if let Some(portfolio) = &app.portfolio {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        let allocation = portfolio.get_allocation();
        let mut allocation_vec: Vec<(&String, &f64)> = allocation.iter().collect();
        allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

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

        f.render_widget(barchart, chunks[0]);



        let detailed_list: Vec<ListItem> = allocation_vec
            .iter()
            .map(|(asset_class, percentage)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<15}", asset_class),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:>8.2}%", percentage),
                        Style::default().fg(Color::Yellow),
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

        f.render_widget(list, chunks[1]);
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
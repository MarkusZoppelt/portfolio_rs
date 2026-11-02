//! Drawing an [`App`] into a ratatui frame.

use chrono::Utc;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem, Paragraph,
        Row, Table, Tabs, Wrap,
    },
    Frame,
};
use tui_big_text::{BigText, PixelSize};

use super::app::purchases_newest_first;
use super::format::{format_amount, format_big_value, format_currency};
use super::history::get_historic_portfolio_data;
use super::{App, AppMode, Component, EditField, NetworkStatus, Tab};
use crate::money::{percent_of, to_f64, Decimal};
use crate::portfolio::Portfolio;
use crate::position::PortfolioPosition;

/// Green for gains (and zero), red for losses.
fn sign_color(value: f64) -> Color {
    if value >= 0.0 {
        Color::Green
    } else {
        Color::Red
    }
}

fn highlight() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn ui(f: &mut Frame, app: &App) {
    let content_area = if app.disabled_components.is_disabled(Component::TabBar) {
        // If tab bar is disabled, use the full area for content
        f.area()
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(f.area());
        render_tab_bar(f, chunks[0], app);
        chunks[1]
    };

    match app.current_tab {
        Tab::Overview => render_overview(f, content_area, app),
        Tab::Balances => match app.mode {
            AppMode::PurchaseList => render_purchase_list(f, content_area, app),
            AppMode::AddPurchase => {
                render_purchase_form(f, content_area, app, "Add", Color::Yellow)
            }
            AppMode::EditPurchase => {
                render_purchase_form(f, content_area, app, "Edit", Color::Cyan)
            }
            AppMode::Normal | AppMode::Edit => render_balances(f, content_area, app),
        },
    }

    if let Some(error) = &app.error_message {
        render_error_popup(f, error);
    }
}

fn render_tab_bar(f: &mut Frame, area: Rect, app: &App) {
    let tab_titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| {
            let style = if *t == app.current_tab {
                highlight()
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(t.title(), style))
        })
        .collect();

    let selected = Tab::all()
        .iter()
        .position(|&t| t == app.current_tab)
        .unwrap_or(0);
    let tabs = Tabs::new(tab_titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Portfolio TUI"),
        )
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Yellow))
        .select(selected);

    f.render_widget(tabs, area);
}

fn render_historic_graph(f: &mut Frame, area: Rect, portfolio: &Portfolio, app: &App) {
    // Prefer async-computed weekly series if available, else fallback to simple cumulative
    let historic_data = match &app.historic_data {
        Some(series) => series.clone(),
        None => get_historic_portfolio_data(portfolio, Utc::now()),
    };

    if historic_data.is_empty() {
        let placeholder = Paragraph::new(
            "No purchase history found\nAdd purchase dates and prices to your portfolio.json",
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Portfolio History (Based on Purchase Dates)"),
        )
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);
        f.render_widget(placeholder, area);
        return;
    }

    // Find min/max values for scaling
    let values = historic_data.iter().map(|(_, v)| *v);
    let min_value = values.clone().fold(f64::INFINITY, f64::min);
    let max_value = values.fold(f64::NEG_INFINITY, f64::max);
    let weeks = historic_data.iter().map(|(d, _)| *d);
    let min_week = weeks.clone().fold(f64::INFINITY, f64::min);
    let max_week = weeks.fold(f64::NEG_INFINITY, f64::max);

    let (y_min, y_max) = y_bounds(min_value, max_value);

    let datasets = vec![Dataset::default()
        .marker(ratatui::symbols::Marker::Braille)
        .style(Style::default().fg(Color::Cyan))
        .graph_type(GraphType::Line)
        .data(&historic_data)];

    let range = (y_max - y_min).max(f64::EPSILON);
    let tick = |t: f64| y_min + range * t;
    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Portfolio Growth"),
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
                    Line::from(format!("{max_week:.0}")),
                ]),
        )
        .y_axis(
            Axis::default()
                .title(app.currency.as_str())
                .style(Style::default().fg(Color::Gray))
                .bounds([y_min, y_max])
                .labels(vec![
                    Line::from(format!("{y_min:.0}")),
                    Line::from(format!("{:.0}", tick(0.25))),
                    Line::from(format!("{:.0}", tick(0.5))),
                    Line::from(format!("{:.0}", tick(0.75))),
                    Line::from(format!("{y_max:.0}")),
                ]),
        );

    f.render_widget(chart, area);
}

/// Chart y-range for values in `min_value..=max_value`: never below zero,
/// padded by 10% of the span, or by 5% of the magnitude for a flat series.
fn y_bounds(min_value: f64, max_value: f64) -> (f64, f64) {
    let y_min = min_value.max(0.0);
    let span = (max_value - y_min).abs();
    let pad = if span > 0.0 {
        span * 0.1
    } else {
        // Flat series: add a small absolute pad based on magnitude, min 1.0
        max_value.abs().max(1.0) * 0.05
    };
    let y_min = (y_min - pad).max(0.0);
    let y_max = max_value + pad;
    if y_max <= y_min {
        (y_min, y_min + y_min.max(1.0) * 0.1)
    } else {
        (y_min, y_max)
    }
}

fn render_detailed_allocation_positions(f: &mut Frame, area: Rect, portfolio: &Portfolio) {
    const COLORS: [Color; 8] = [
        Color::Red,
        Color::Green,
        Color::Blue,
        Color::Yellow,
        Color::Magenta,
        Color::Cyan,
        Color::White,
        Color::LightRed,
    ];
    // Limit to 6 for horizontal display
    const SHOWN: usize = 6;

    // Compact horizontal bars with embedded labels
    let mut lines: Vec<Line> = portfolio
        .positions
        .iter()
        .take(SHOWN)
        .zip(COLORS)
        .map(|(position, color)| {
            let percentage = percent_of(position.balance_dec(), portfolio.total_value_dec())
                .map_or(f64::NAN, to_f64);
            // Horizontal bar, 1 to 30 characters wide
            let bar_width = (((percentage / 100.0) * 30.0) as usize).clamp(1, 30);
            // Truncate name to fit in available space
            let display_name: String = position.get_name().chars().take(12).collect();
            Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(
                    format!("{display_name:<12}"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{percentage:>6.1}% "), Style::default().fg(color)),
                Span::styled("█".repeat(bar_width), Style::default().fg(color)),
            ])
        })
        .collect();

    if portfolio.positions.len() > SHOWN {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!(
                "... and {} more positions",
                portfolio.positions.len() - SHOWN
            ),
            Style::default().fg(Color::Gray),
        )]));
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Detailed Allocation"),
        )
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left);

    f.render_widget(widget, area);
}

fn render_purchase_list(f: &mut Frame, area: Rect, app: &App) {
    let Some(position) = app.selected_portfolio_position() else {
        return;
    };

    // "Add New Purchase" option first
    let add_style = if app.selected_purchase == 0 {
        Style::default().bg(Color::Green).fg(Color::Black)
    } else {
        Style::default().fg(Color::Green)
    };
    let mut items = vec![ListItem::new("+ Add New Purchase").style(add_style)];

    // Existing purchases, newest first
    for (display_index, (_original_index, purchase)) in
        purchases_newest_first(position.get_purchases())
            .iter()
            .enumerate()
    {
        let date = purchase.date.as_deref().unwrap_or("No date");
        let price = purchase.effective_price().unwrap_or(Decimal::ZERO);
        let total = to_f64(purchase.quantity * price);
        let quantity = to_f64(purchase.quantity);
        let price = to_f64(price);

        // selected_purchase: 0 = Add New, 1+ = existing purchases
        let style = if (display_index + 1) == app.selected_purchase {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        };

        let item_text = if price > 0.0 {
            format!("{date} | Qty: {quantity:.4} | Price: ${price:.2} | Total: ${total:.2}")
        } else {
            format!("{date} | Qty: {quantity:.4} | Price: Auto-filled")
        };

        items.push(ListItem::new(item_text).style(style));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Purchase History - {}", position.get_name())),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(list, area);

    // Show help at the bottom
    let help_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area)[1];

    let help_text =
        Paragraph::new("j/k: Navigate | Enter/a: Add New (first) or Edit (others) | Esc: Back")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);

    f.render_widget(help_text, help_area);
}

/// The add (`verb` "Add") or edit (`verb` "Edit") purchase form.
fn render_purchase_form(f: &mut Frame, area: Rect, app: &App, verb: &str, title_color: Color) {
    let Some(position) = app.selected_portfolio_position() else {
        return;
    };

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

    let title = Paragraph::new(format!("{verb} Purchase - {}", position.get_name()))
        .block(Block::default().borders(Borders::ALL))
        .style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    let field_style = |field: EditField| {
        if app.edit_field == field {
            highlight()
        } else {
            Style::default()
        }
    };

    let date_field = Paragraph::new(format!(
        "Date (type digits only): {}",
        app.purchase_date_input
    ))
    .block(Block::default().borders(Borders::ALL).title("Date"))
    .style(field_style(EditField::Date));
    f.render_widget(date_field, chunks[1]);

    let qty_field = Paragraph::new(format!("Quantity: {}", app.purchase_quantity_input))
        .block(Block::default().borders(Borders::ALL).title("Quantity"))
        .style(field_style(EditField::Quantity));
    f.render_widget(qty_field, chunks[2]);

    let price_help = if app.purchase_price_input.is_empty() {
        " (leave empty for auto-fill)"
    } else {
        ""
    };
    let price_field = Paragraph::new(format!(
        "Price (optional): {}{price_help}",
        app.purchase_price_input
    ))
    .block(Block::default().borders(Borders::ALL).title("Price"))
    .style(field_style(EditField::Price));
    f.render_widget(price_field, chunks[3]);

    let help_text =
        Paragraph::new("Tab/↓: Next Field | ↑: Previous Field | Enter: Save | Esc: Cancel")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
    f.render_widget(help_text, chunks[5]);
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    let Some(portfolio) = &app.portfolio else {
        render_loading(f, area);
        return;
    };
    let enabled = |component| !app.disabled_components.is_disabled(component);
    let total_value_enabled = enabled(Component::TotalValue);
    let growth_enabled = enabled(Component::PortfolioGrowth);
    let breakdown_enabled = enabled(Component::AssetBreakdown);
    let detailed_enabled = enabled(Component::DetailedAllocation);
    let help_enabled = enabled(Component::Help);
    let any_main_content_enabled = growth_enabled || breakdown_enabled || detailed_enabled;

    // Top: total value, middle: graph and allocation, bottom: help
    let mut constraints = Vec::new();
    if total_value_enabled {
        constraints.push(Constraint::Length(7));
    }
    if any_main_content_enabled {
        constraints.push(Constraint::Min(0));
    }
    if help_enabled {
        constraints.push(Constraint::Length(3));
    }
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let mut chunks = main_chunks.iter().copied();

    if total_value_enabled {
        render_total_value(f, chunks.next().unwrap_or_default(), portfolio, app);
    }

    if any_main_content_enabled {
        let content_area = chunks.next().unwrap_or_default();
        // Historic graph on top, detailed allocation and asset breakdown below
        let bottom_area = if growth_enabled && (breakdown_enabled || detailed_enabled) {
            let content_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(content_area);
            render_historic_graph(f, content_chunks[0], portfolio, app);
            content_chunks[1]
        } else if growth_enabled {
            render_historic_graph(f, content_area, portfolio, app);
            Rect::default()
        } else {
            content_area
        };

        // Detailed allocation on the left, asset breakdown on the right
        match (detailed_enabled, breakdown_enabled) {
            (true, true) => {
                let bottom_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(bottom_area);
                render_detailed_allocation_positions(f, bottom_chunks[0], portfolio);
                render_asset_breakdown_grouped(f, bottom_chunks[1], portfolio, app);
            }
            (true, false) => render_detailed_allocation_positions(f, bottom_area, portfolio),
            (false, true) => render_asset_breakdown_grouped(f, bottom_area, portfolio, app),
            (false, false) => {}
        }
    }

    if help_enabled {
        let help_text = Paragraph::new("Navigation: h/l (tabs) | j/k (select in Balances) | e (edit in Balances) | r (refresh) | 1-2 (direct) | q (quit)")
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(help_text, chunks.next().unwrap_or_default());
    }
}

/// Total value in big text, with the day's PnL and %Day on the right.
fn render_total_value(f: &mut Frame, area: Rect, portfolio: &Portfolio, app: &App) {
    // Daily PnL and %Day over all positions (cash has no daily variation).
    let breakdown = portfolio.value_breakdown();
    let total_value = to_f64(breakdown.total);
    let day_pnl_abs = breakdown.day_pnl();
    let daily_percent = breakdown.day_percent();
    let big_text_value = format_big_value(total_value, &app.currency);

    let refresh_indicator = if app.flash_state { "🔄" } else { "📊" };
    let network_indicator = match app.network_status {
        NetworkStatus::Connected => "🟢",
        NetworkStatus::Partial => "🟡",
        NetworkStatus::Disconnected => "🔴",
    };
    let frame = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            "Total Portfolio Value ({}) {refresh_indicator} {network_indicator}",
            app.currency
        ))
        .title_alignment(Alignment::Center);
    f.render_widget(frame, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    // Center big text across the full inner area
    let big_text_width = big_text_value.len() as u16 * 4;
    let centered_area = if big_text_width < inner.width {
        let margin = (inner.width - big_text_width) / 2;
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
    let big_text = BigText::builder()
        .pixel_size(PixelSize::Quadrant)
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .lines(vec![big_text_value.into()])
        .build();
    f.render_widget(big_text, centered_area);

    let right_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
        .split(inner)[1];

    let day_value_str = format_currency(day_pnl_abs, &app.currency);
    let pct_value_str = format!("{daily_percent:+.2}%");
    // Estimate content width to center the block horizontally
    let max_w = (8 + day_value_str.chars().count()).max(5 + pct_value_str.chars().count()) as u16;
    let right_content = vec![
        Line::from(vec![
            Span::styled("Day PnL ", Style::default().fg(Color::Gray)),
            Span::styled(
                day_value_str,
                Style::default()
                    .fg(sign_color(day_pnl_abs))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("%Day ", Style::default().fg(Color::Gray)),
            Span::styled(
                pct_value_str,
                Style::default()
                    .fg(sign_color(daily_percent))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    // Vertically center inside the right third, and horizontally center the
    // block while left-aligning text
    let content_lines = 2u16; // two lines: Day PnL and %Day
    let vpad = right_area.height.saturating_sub(content_lines) / 2;
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vpad),
            Constraint::Length(content_lines),
            Constraint::Min(0),
        ])
        .split(right_area);
    let hpad = vchunks[1].width.saturating_sub(max_w) / 2;
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
}

/// A balances table column: its header, width and the cell of one position.
struct Column {
    component: Component,
    header: &'static str,
    width: u16,
}

const BALANCE_COLUMNS: [Column; 10] = [
    Column {
        component: Component::Name,
        header: "Name",
        width: 22,
    },
    Column {
        component: Component::AssetClass,
        header: "Class",
        width: 10,
    },
    Column {
        component: Component::Amount,
        header: "Amt",
        width: 8,
    },
    Column {
        component: Component::Price,
        header: "Price",
        width: 10,
    },
    Column {
        component: Component::AvgCost,
        header: "Avg",
        width: 10,
    },
    Column {
        component: Component::Invested,
        header: "Invested",
        width: 12,
    },
    Column {
        component: Component::Balance,
        header: "Value",
        width: 12,
    },
    Column {
        component: Component::PnL,
        header: "PnL",
        width: 12,
    },
    Column {
        component: Component::Hist,
        header: "%Hist",
        width: 7,
    },
    Column {
        component: Component::Daily,
        header: "%Day",
        width: 7,
    },
];

/// The `component` cell of `position`, colored by its value trend.
fn position_cell(app: &App, position: &PortfolioPosition, component: Component) -> Cell<'static> {
    let trend_style = Style::default().fg(app.get_trend_color(position.get_name(), Color::White));
    let plain = |text: String| Cell::from(text).style(trend_style);
    // A signed value in green or red, `-` when unknown.
    let signed = |value: Option<f64>, suffix: &str| match value {
        Some(v) => Cell::from(format!("{v:.2}{suffix}")).style(Style::default().fg(sign_color(v))),
        None => plain("-".to_string()),
    };
    let or_dash = |value: Option<f64>| value.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
    // Cash has no market price, cost basis or variation.
    let is_cash = position.is_cash();
    match component {
        // Live (ticker) positions are marked ●, static ones ○.
        Component::Name if position.get_ticker().is_some() => {
            plain(format!("● {}", position.get_name()))
        }
        Component::Name => plain(format!("○ {}", position.get_name())),
        Component::AssetClass => plain(position.get_asset_class().to_string()),
        Component::Amount => plain(format_amount(position.get_amount())),
        Component::Balance => plain(format_currency(position.get_balance(), &app.currency)),
        _ if is_cash => plain("-".to_string()),
        Component::Price => plain(format!("{:.2}", position.market_price())),
        Component::AvgCost => plain(or_dash(position.average_cost())),
        Component::Invested => plain(or_dash(position.total_invested())),
        Component::PnL => signed(position.pnl(), ""),
        Component::Hist => signed(position.historic_variation_percent(), "%"),
        // The last table column, Component::Daily.
        _ => signed(position.daily_variation_percent(), "%"),
    }
}

/// The `component` cell of the TOTAL row.
fn total_cell(app: &App, portfolio: &Portfolio, component: Component) -> Cell<'static> {
    let bold = |text: String, color: Color| {
        Cell::from(text).style(Style::default().fg(color).add_modifier(Modifier::BOLD))
    };
    let signed = |value: f64, suffix: &str| bold(format!("{value:.2}{suffix}"), sign_color(value));
    match component {
        Component::Name => bold("TOTAL".to_string(), Color::Green),
        Component::Invested => Cell::from(format!(
            "{:.2}",
            to_f64(portfolio.cost_basis_totals_dec().0)
        )),
        Component::Balance => bold(
            format_currency(portfolio.get_total_value(), &app.currency),
            Color::Green,
        ),
        // Same scope as the CLI and API: priced securities with a cost basis.
        Component::PnL => signed(to_f64(portfolio.cost_basis_totals_dec().1), ""),
        Component::Hist => signed(portfolio.hist_percent(), "%"),
        // Total daily variation over all positions (cash has no daily variation)
        Component::Daily => signed(portfolio.value_breakdown().day_percent(), "%"),
        _ => Cell::from(""),
    }
}

fn render_balances(f: &mut Frame, area: Rect, app: &App) {
    let Some(portfolio) = &app.portfolio else {
        render_loading(f, area);
        return;
    };
    let columns: Vec<&Column> = BALANCE_COLUMNS
        .iter()
        .filter(|column| !app.disabled_components.is_disabled(column.component))
        .collect();

    if columns.is_empty() {
        let placeholder = Paragraph::new("All balance columns are disabled")
            .block(Block::default().borders(Borders::ALL).title("Balances"))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(placeholder, area);
        return;
    }

    let header = Row::new(
        columns
            .iter()
            .map(|column| Cell::from(column.header).style(highlight())),
    )
    .height(1)
    .bottom_margin(1);

    let rows = portfolio.positions.iter().enumerate().map(|(i, position)| {
        // Highlight selected row
        let row_style = if i == app.selected_position && app.current_tab == Tab::Balances {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };
        let cells = columns
            .iter()
            .map(|column| position_cell(app, position, column.component));
        Row::new(cells).height(1).style(row_style)
    });
    let total_row = Row::new(
        columns
            .iter()
            .map(|column| total_cell(app, portfolio, column.component)),
    )
    .height(1);

    let help_text = match app.mode {
        AppMode::Normal => {
            "Navigation: j/k (select) | e (edit) | h/l (tabs) | r (refresh) | q (quit)"
        }
        AppMode::Edit => "Edit Mode: Enter (save) | Esc (cancel)",
        AppMode::PurchaseList => "Purchase List: j/k (select) | Enter/a (add) | Esc (back)",
        AppMode::AddPurchase => "Add Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
        AppMode::EditPurchase => "Edit Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
    };

    let widths: Vec<Constraint> = columns
        .iter()
        .map(|column| Constraint::Length(column.width))
        .collect();
    let table = Table::new(rows.chain(std::iter::once(total_row)), widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Portfolio Balances - {help_text}")),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(table, area);

    // Render edit dialog if in edit mode
    if app.mode == AppMode::Edit {
        render_edit_dialog(f, app);
    }
}

fn render_loading(f: &mut Frame, area: Rect) {
    let loading_text = Paragraph::new("Loading portfolio data...")
        .block(Block::default().borders(Borders::ALL).title("Loading"))
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);

    f.render_widget(loading_text, area);
}

/// The preview line under the amount editor's input and its style.
fn amount_preview(app: &App, position: &PortfolioPosition) -> (String, Style) {
    let message = |text: &str, color: Color| (text.to_string(), Style::default().fg(color));
    if app.edit_input.is_empty() {
        return message("Enter amount...", Color::Gray);
    }
    let input = app.edit_input.trim();
    match app.edit_input.parse::<f64>() {
        Ok(new_amount) if new_amount < 0.0 => message("Amount cannot be negative", Color::Red),
        Ok(new_amount) => {
            let new_balance = if position.get_ticker().is_some() {
                // For positions with tickers, calculate balance using last spot price
                new_amount * (position.get_balance() / position.get_amount())
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
        // Just a dot, waiting for digits
        Err(_) if input == "." => message("Enter digits...", Color::Yellow),
        // A valid intermediate state like "1." or "123."
        Err(_)
            if input
                .strip_suffix('.')
                .is_some_and(|n| n.parse::<f64>().is_ok()) =>
        {
            message("Enter decimal places...", Color::Yellow)
        }
        Err(_) => message("Invalid number format", Color::Red),
    }
}

fn render_edit_dialog(f: &mut Frame, app: &App) {
    let popup_area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, popup_area);

    let Some(position) = app.selected_portfolio_position() else {
        return;
    };

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

    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Edit Position Amount ")
        .title_alignment(Alignment::Center)
        .style(Style::default().bg(Color::Black));
    f.render_widget(main_block, popup_area);

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
        .alignment(Alignment::Center);
    f.render_widget(info_paragraph, popup_layout[0]);

    let current_text = format!(
        "Current Amount: {}\nCurrent Balance: {}",
        format_amount(position.get_amount()),
        format_currency(position.get_balance(), &app.currency)
    );
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

    // Input with a blinking cursor
    let cursor = if app.flash_state { "█" } else { "▌" };
    let (preview_text, preview_style) = amount_preview(app, position);

    let input_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(popup_layout[2]);

    let input_field = Paragraph::new(format!("{}{cursor}", app.edit_input))
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

    let preview_paragraph = Paragraph::new(preview_text)
        .style(preview_style)
        .alignment(Alignment::Center);
    f.render_widget(preview_paragraph, input_chunks[1]);

    let instructions = "Enter: Save Changes | Esc: Cancel | Type numbers and decimal point";
    let instructions_paragraph = Paragraph::new(instructions)
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    f.render_widget(instructions_paragraph, popup_layout[3]);
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
    allocation_vec.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

    let items: Vec<ListItem> = allocation_vec
        .iter()
        .map(|(asset_class, percentage)| {
            // The trend color of the first position in this asset class
            let trend_color = portfolio
                .positions
                .iter()
                .find(|p| p.get_asset_class() == *asset_class)
                .map_or(Color::Cyan, |p| {
                    app.get_trend_color(p.get_name(), Color::Cyan)
                });
            let style = Style::default().fg(trend_color);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{asset_class:<15}"), style),
                Span::styled(format!("{percentage:>8.2}%"), style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Asset Breakdown"),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(list, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::{app_with, buffer_text, disabled, render, POSITIONS};
    use crate::tui::DisabledComponents;
    use ratatui::{backend::TestBackend, Terminal};

    const W: u16 = 140;
    const H: u16 = 40;

    async fn overview(disabled_names: &[&str]) -> String {
        let mut app = app_with(POSITIONS).await;
        app.disabled_components = disabled(disabled_names);
        render(&app, W, H)
    }

    fn assert_shows(screen: &str, texts: &[&str]) {
        for text in texts {
            assert!(screen.contains(text), "missing {text:?} in\n{screen}");
        }
    }

    fn assert_hides(screen: &str, texts: &[&str]) {
        for text in texts {
            assert!(!screen.contains(text), "unexpected {text:?} in\n{screen}");
        }
    }

    #[test]
    fn colors_follow_the_sign() {
        assert_eq!(sign_color(0.0), Color::Green);
        assert_eq!(sign_color(2.0), Color::Green);
        assert_eq!(sign_color(-0.1), Color::Red);
    }

    #[tokio::test]
    async fn overview_shows_every_component() {
        let screen = overview(&[]).await;
        assert_shows(
            &screen,
            &[
                "Portfolio TUI",
                "Overview & Allocation",
                "Total Portfolio Value (USD)",
                "📊",
                "🟢",
                "Day PnL $0.00",
                "%Day +0.00%",
                "Portfolio Growth",
                "Weeks Since First Purchase",
                "Detailed Allocation",
                "Stocks        32.3%",
                "Asset Breakdown",
                "Commodities        6.45%",
                "Navigation: h/l (tabs)",
            ],
        );
    }

    #[tokio::test]
    async fn overview_layouts_drop_disabled_components() {
        let screen = overview(&["tab_bar", "total_value", "help"]).await;
        assert_hides(
            &screen,
            &["Portfolio TUI", "Total Portfolio Value", "Navigation"],
        );
        assert_shows(
            &screen,
            &["Portfolio Growth", "Detailed Allocation", "Asset Breakdown"],
        );

        let screen = overview(&["asset_breakdown"]).await;
        assert_shows(&screen, &["Portfolio Growth", "Detailed Allocation"]);
        assert_hides(&screen, &["Asset Breakdown"]);

        let screen = overview(&["detailed_allocation"]).await;
        assert_shows(&screen, &["Portfolio Growth", "Asset Breakdown"]);
        assert_hides(&screen, &["Detailed Allocation"]);

        let screen = overview(&["detailed_allocation", "asset_breakdown"]).await;
        assert_shows(&screen, &["Portfolio Growth"]);
        assert_hides(&screen, &["Detailed Allocation", "Asset Breakdown"]);

        let screen = overview(&["portfolio_growth"]).await;
        assert_shows(&screen, &["Detailed Allocation", "Asset Breakdown"]);
        assert_hides(&screen, &["Portfolio Growth"]);

        let screen = overview(&["portfolio_growth", "asset_breakdown"]).await;
        assert_shows(&screen, &["Detailed Allocation"]);
        assert_hides(&screen, &["Asset Breakdown"]);

        let screen = overview(&["portfolio_growth", "detailed_allocation"]).await;
        assert_shows(&screen, &["Asset Breakdown", "Navigation"]);
        assert_hides(&screen, &["Detailed Allocation"]);

        let screen =
            overview(&["portfolio_growth", "detailed_allocation", "asset_breakdown"]).await;
        assert_shows(&screen, &["Total Portfolio Value", "Navigation"]);
        assert_hides(
            &screen,
            &["Detailed Allocation", "Asset Breakdown", "Growth"],
        );
    }

    #[tokio::test]
    async fn total_value_indicators_and_losses() {
        let mut app = app_with(
            r#"[{"Name": "Bonds", "Ticker": "TLT", "AssetClass": "Bonds", "Amount": 10}]"#,
        )
        .await;
        app.flash_state = true;
        app.network_status = NetworkStatus::Partial;
        let screen = render(&app, W, H);
        assert_shows(&screen, &["🔄", "🟡", "Day PnL $-10.00", "%Day -1.10%"]);

        app.network_status = NetworkStatus::Disconnected;
        assert_shows(&render(&app, W, H), &["🔴"]);
        // Too narrow to center the big text: still drawn.
        assert_shows(&render(&app, 30, H), &["ortfolio Value (USD)", "%Day -1.1"]);
    }

    #[tokio::test]
    async fn loading_until_a_portfolio_arrives() {
        let mut app = app_with("[]").await;
        app.portfolio = None;
        assert_shows(&render(&app, W, H), &["Loading portfolio data..."]);
        app.current_tab = Tab::Balances;
        assert_shows(&render(&app, W, H), &["Loading portfolio data..."]);
    }

    #[tokio::test]
    async fn growth_chart_series_and_placeholder() {
        let mut app = app_with(POSITIONS).await;
        // Without a computed series, the invested amounts (from 400) up to the
        // current 3,100 are charted, padded by 10%.
        assert_shows(
            &render(&app, W, H),
            &["Portfolio Growth", "│3370│", "│130 │"],
        );

        app.historic_data = Some(vec![(0.0, 100.0), (4.0, 100.0)]);
        assert_shows(&render(&app, W, H), &["Portfolio Growth", "105", "95"]);

        app.historic_data = Some(vec![]);
        assert_shows(&render(&app, W, H), &["No purchase history found"]);
    }

    #[test]
    fn chart_bounds_pad_and_stay_positive() {
        assert_eq!(y_bounds(0.0, 100.0), (0.0, 110.0));
        assert_eq!(y_bounds(100.0, 100.0), (95.0, 105.0));
        assert_eq!(y_bounds(1.0, 3.0), (0.8, 3.2));
        // All values negative: a small range above zero.
        assert_eq!(y_bounds(-5.0, -5.0), (0.0, 0.1));
    }

    #[tokio::test]
    async fn detailed_allocation_lists_six_positions() {
        let positions: Vec<String> = (1..=7)
            .map(|i| {
                format!(
                    r#"{{"Name": "Cash account number {i}", "AssetClass": "Cash", "Amount": {i}}}"#
                )
            })
            .collect();
        let app = app_with(&format!("[{}]", positions.join(","))).await;
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &["Cash account  25.0%", "... and 1 more positions"],
        );
        assert_hides(&screen, &["Cash account number"]);
    }

    #[tokio::test]
    async fn balances_table_shows_every_column() {
        let mut app = app_with(POSITIONS).await;
        app.current_tab = Tab::Balances;
        app.trends.insert("Stocks".into(), crate::tui::Trend::Up);
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &[
                "Portfolio Balances - Navigation: j/k (select) | e (edit)",
                "Name                   Class      Amt      Price      Avg        Invested     Value        PnL          %Hist   %Day",
                "● Stocks               Stocks     2        500.00     425.00     850.00       $1,000.00    150.00       17.65%  1.01%",
                "○ Cash                 Cash       1000     -          -          -            $1,000.00    -            -       -",
                "● Bonds                Bonds      10       90.00      100.00     1000.00      $900.00      -100.00      -10.00% -1.10%",
                "● Gold                 Commoditie 1        200.00     -          -            $200.00      -            -       -",
                "TOTAL                                                            1850.00      $3,100.00    50.00        2.70%   0.00%",
            ],
        );
    }

    #[tokio::test]
    async fn balances_columns_can_be_disabled() {
        let mut app = app_with(POSITIONS).await;
        app.current_tab = Tab::Balances;
        app.disabled_components = disabled(&[
            "asset_class",
            "amount",
            "price",
            "avg_cost",
            "invested",
            "pnl",
            "%hist",
            "%day",
        ]);
        assert_shows(
            &render(&app, W, H),
            &[
                "Name                   Value",
                "TOTAL                  $3,100.00",
            ],
        );

        app.disabled_components = disabled(&[
            "name",
            "asset_class",
            "amount",
            "price",
            "avg_cost",
            "invested",
            "pnl",
            "%hist",
            "%day",
            "balance",
        ]);
        assert_shows(&render(&app, W, H), &["All balance columns are disabled"]);
    }

    #[tokio::test]
    async fn balances_title_names_the_mode() {
        let mut app = app_with(POSITIONS).await;
        app.current_tab = Tab::Balances;
        // These modes draw their own screens, so draw the table directly.
        for (mode, title) in [
            (
                AppMode::PurchaseList,
                "Purchase List: j/k (select) | Enter/a (add) | Esc (back)",
            ),
            (
                AppMode::AddPurchase,
                "Add Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
            ),
            (
                AppMode::EditPurchase,
                "Edit Purchase: Tab (next field) | Enter (save) | Esc (cancel)",
            ),
        ] {
            app.mode = mode;
            let mut terminal = Terminal::new(TestBackend::new(W, H)).unwrap();
            terminal
                .draw(|f| render_balances(f, f.area(), &app))
                .unwrap();
            assert_shows(&buffer_text(&terminal), &[title]);
        }
    }

    #[tokio::test]
    async fn legacy_amount_editor_dialog() {
        let mut app = app_with(POSITIONS).await;
        app.current_tab = Tab::Balances;
        app.mode = AppMode::Edit;
        app.edit_input = "3".into();
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &[
                "Edit Mode: Enter (save) | Esc (cancel)",
                " Edit Position Amount ",
                "Position: Stocks (Stocks)",
                "Current Amount: 2",
                "Current Balance: $1,000.00",
                "3▌",
                "New Balance: $1,500.00",
                "Enter: Save Changes | Esc: Cancel",
            ],
        );
        app.flash_state = true;
        assert_shows(&render(&app, W, H), &["3█"]);

        // The selection is gone (the file shrank): no dialog.
        app.selected_position = 10;
        assert_hides(&render(&app, W, H), &["Edit Position Amount"]);
    }

    #[tokio::test]
    async fn amount_preview_guides_the_input() {
        let mut app = app_with(POSITIONS).await;
        let stocks = app.portfolio.as_ref().unwrap().positions[0].clone();
        let cash = app.portfolio.as_ref().unwrap().positions[1].clone();
        assert_eq!(cash.get_name(), "Cash");
        let mut preview = |input: &str, position: &PortfolioPosition| {
            app.edit_input = input.into();
            let (text, style) = amount_preview(&app, position);
            (text, style.fg.unwrap())
        };
        assert_eq!(
            preview("", &stocks),
            ("Enter amount...".into(), Color::Gray)
        );
        assert_eq!(
            preview("-1", &stocks),
            ("Amount cannot be negative".into(), Color::Red)
        );
        assert_eq!(
            preview("1", &stocks),
            ("New Balance: $500.00".into(), Color::Green)
        );
        assert_eq!(
            preview("5", &cash),
            ("New Balance: $5.00".into(), Color::Green)
        );
        assert_eq!(
            preview(".", &cash),
            ("Enter digits...".into(), Color::Yellow)
        );
        assert_eq!(
            preview("1..", &cash),
            ("Enter decimal places...".into(), Color::Yellow)
        );
        assert_eq!(
            preview("a.", &cash),
            ("Invalid number format".into(), Color::Red)
        );
        assert_eq!(
            preview("abc", &cash),
            ("Invalid number format".into(), Color::Red)
        );
    }

    const PURCHASES: &str = r#"[{"Name": "Mystery", "Ticker": "ZZZ", "AssetClass": "Stocks", "Amount": 3,
        "Purchases": [{"Quantity": 1}, {"Date": "2025-01-01", "Quantity": 2, "Price": 7.5}]}]"#;

    #[tokio::test]
    async fn purchase_list_shows_newest_first() {
        let mut app = app_with(PURCHASES).await;
        app.current_tab = Tab::Balances;
        app.mode = AppMode::PurchaseList;
        app.selected_purchase = 1;
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &[
                "Purchase History - Mystery",
                "+ Add New Purchase",
                "2025-01-01 | Qty: 2.0000 | Price: $7.50 | Total: $15.00",
                "No date | Qty: 1.0000 | Price: Auto-filled",
                "j/k: Navigate | Enter/a: Add New (first) or Edit (others) | Esc: Back",
            ],
        );
        let first_row = screen.find("2025-01-01").unwrap();
        assert!(first_row < screen.find("No date").unwrap());

        // The selected row is highlighted: "Add New" in green, purchases in blue.
        let backgrounds = |app: &crate::tui::App| {
            let mut terminal = Terminal::new(TestBackend::new(W, H)).unwrap();
            terminal.draw(|f| ui(f, app)).unwrap();
            let buffer = terminal.backend().buffer();
            // Row 4 is "Add New", row 5 the newest purchase.
            (buffer[(1, 4)].bg, buffer[(1, 5)].bg)
        };
        assert_eq!(backgrounds(&app), (Color::Reset, Color::Blue));
        app.selected_purchase = 0;
        assert_eq!(backgrounds(&app), (Color::Green, Color::Reset));

        app.selected_position = 1;
        assert_hides(&render(&app, W, H), &["Purchase History"]);
    }

    #[tokio::test]
    async fn purchase_forms_show_the_inputs() {
        let mut app = app_with(PURCHASES).await;
        app.current_tab = Tab::Balances;
        app.mode = AppMode::AddPurchase;
        app.purchase_date_input = "2025-".into();
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &[
                "Add Purchase - Mystery",
                "Date (type digits only): 2025-",
                "Quantity: ",
                "Price (optional):  (leave empty for auto-fill)",
                "Tab/↓: Next Field | ↑: Previous Field | Enter: Save | Esc: Cancel",
            ],
        );

        app.mode = AppMode::EditPurchase;
        app.edit_field = EditField::Price;
        app.purchase_price_input = "7.5".into();
        let screen = render(&app, W, H);
        assert_shows(
            &screen,
            &["Edit Purchase - Mystery", "Price (optional): 7.5"],
        );
        assert_hides(&screen, &["auto-fill"]);

        app.selected_position = 3;
        assert_hides(&render(&app, W, H), &["Edit Purchase"]);
    }

    #[test]
    fn focused_form_field_is_highlighted() {
        let mut app = crate::tui::App::new(
            "USD".into(),
            "[]".into(),
            "p".into(),
            DisabledComponents::default(),
        );
        app.edit_field = EditField::Quantity;
        let mut portfolio = Portfolio::new();
        portfolio.add_position(crate::position::from_string(PURCHASES).unwrap().remove(0));
        app.portfolio = Some(portfolio);
        app.current_tab = Tab::Balances;
        app.mode = AppMode::AddPurchase;
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        // Rows: tab bar (3), title (3), date (3), quantity (3).
        let date = &buffer[(1, 7)];
        let quantity = &buffer[(1, 10)];
        assert_eq!(date.symbol(), "D");
        assert_eq!(quantity.symbol(), "Q");
        assert_eq!(quantity.fg, Color::Yellow);
        assert_ne!(date.fg, Color::Yellow);
    }

    #[tokio::test]
    async fn errors_pop_up_over_any_screen() {
        let mut app = app_with(POSITIONS).await;
        app.error_message = Some("Failed to save: disk full".into());
        let screen = render(&app, W, H);
        assert_shows(&screen, &["Error", "Failed to save: disk full"]);
    }

    #[test]
    fn centered_rect_takes_the_middle() {
        let area = centered_rect(50, 50, Rect::new(0, 0, 100, 40));
        assert_eq!(area, Rect::new(25, 10, 50, 20));
    }
}

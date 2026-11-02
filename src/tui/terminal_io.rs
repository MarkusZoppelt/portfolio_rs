//! The real terminal: raw mode, the alternate screen and crossterm events.
//!
//! Excluded from coverage because it needs an interactive TTY. Everything
//! it calls (app setup, background tasks, the event loop, rendering) lives in
//! covered modules and runs under test with a `TestBackend` and scripted
//! events.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use eyre::Result;
use ratatui::{backend::CrosstermBackend, Terminal};

use super::live_io::ConfiguredLive;
use super::runtime::{
    prepare_app, run_app, spawn_background, Launch, HISTORY_REFRESH, PORTFOLIO_REFRESH,
};
use super::Tab;
use crate::portfolio::Portfolio;

pub async fn run_tui(
    portfolio: Portfolio,
    currency: String,
    positions_str: String,
    data_file_path: String,
    tab: Option<Tab>,
    disabled_components: Vec<String>,
) -> Result<()> {
    let live = Arc::new(ConfiguredLive);
    let launch = Launch {
        portfolio,
        currency,
        positions_str,
        data_file_path,
        tab,
        disabled_components,
    };
    let mut app = prepare_app(&*live, launch, Utc::now()).await?;
    spawn_background(
        &mut app,
        Arc::clone(&live),
        PORTFOLIO_REFRESH,
        HISTORY_REFRESH,
    );

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let res = run_app(&mut terminal, &mut app, &*live, next_event).await;

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

/// The next terminal event, waiting at most 100 ms for one.
fn next_event() -> io::Result<Option<Event>> {
    if event::poll(Duration::from_millis(100))? {
        event::read().map(Some)
    } else {
        Ok(None)
    }
}

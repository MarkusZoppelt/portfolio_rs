//! The TUI event loop and its background refresh tasks, with live data and
//! terminal events injected so they run under test.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossterm::event::Event;
use eyre::{eyre, Result};
use ratatui::{backend::Backend, Terminal};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::history::compute_weekly_series_batch;
use super::input::{handle_key, Action};
use super::live::LiveData;
use super::render::ui;
use super::{App, DisabledComponents, NetworkStatus, Tab};
use crate::portfolio::Portfolio;

/// How long start-up waits for the growth chart before showing the TUI.
const INITIAL_SERIES_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the background task revalues the portfolio.
pub(super) const PORTFOLIO_REFRESH: Duration = Duration::from_secs(5);
/// How often the background task rebuilds the growth chart.
pub(super) const HISTORY_REFRESH: Duration = Duration::from_secs(15);

/// Everything `run_tui` needs to know before the terminal opens.
pub(super) struct Launch {
    pub(super) portfolio: Portfolio,
    pub(super) currency: String,
    pub(super) positions_str: String,
    pub(super) data_file_path: String,
    pub(super) tab: Option<Tab>,
    pub(super) disabled_components: Vec<String>,
}

/// The initial [`App`]: validated components, the start tab, and the growth
/// chart if it can be computed within [`INITIAL_SERIES_TIMEOUT`].
pub(super) async fn prepare_app<L: LiveData>(
    live: &L,
    launch: Launch,
    now: DateTime<Utc>,
) -> Result<App> {
    let disabled = DisabledComponents::new(launch.disabled_components)
        .map_err(|errors| eyre!("Invalid disabled components: {}", errors.join(", ")))?;
    // Pre-compute historic series so the graph shows on launch
    let initial_series = tokio::time::timeout(
        INITIAL_SERIES_TIMEOUT,
        compute_weekly_series_batch(&launch.portfolio, live, now),
    )
    .await
    .unwrap_or_default();

    let mut app = App::new(
        launch.currency,
        launch.positions_str,
        launch.data_file_path,
        disabled,
    );
    app.set_portfolio(launch.portfolio);
    if !initial_series.is_empty() {
        app.historic_data = Some(initial_series);
    }
    if let Some(tab) = launch.tab {
        app.current_tab = tab;
    }
    Ok(app)
}

/// `positions_str` valued with live quotes, sorted by value for display.
async fn sorted_portfolio<L: LiveData>(
    live: &L,
    positions_str: String,
) -> (Portfolio, NetworkStatus) {
    let (mut portfolio, network_status) = live.portfolio(positions_str).await;
    portfolio.sort_positions_by_value_desc();
    (portfolio, network_status)
}

/// One background revaluation: the latest file content, or `fallback` when
/// the file cannot be read.
pub(super) async fn poll_portfolio<L: LiveData>(
    live: &L,
    path: &Path,
    fallback: &str,
) -> (Portfolio, NetworkStatus) {
    let positions_str = std::fs::read_to_string(path).unwrap_or_else(|_| fallback.to_string());
    sorted_portfolio(live, positions_str).await
}

/// One background chart rebuild from the latest file; `None` when the file
/// cannot be read.
pub(super) async fn poll_history<L: LiveData>(
    live: &L,
    path: &Path,
    now: DateTime<Utc>,
) -> Option<Vec<(f64, f64)>> {
    let positions_str = std::fs::read_to_string(path).ok()?;
    let (portfolio, _status) = sorted_portfolio(live, positions_str).await;
    Some(compute_weekly_series_batch(&portfolio, live, now).await)
}

/// Re-read the portfolio file, revalue it and rebuild the chart now.
pub(super) async fn reload<L: LiveData>(app: &mut App, live: &L, now: DateTime<Utc>) {
    if let Ok(positions_str) = std::fs::read_to_string(&app.data_file_path) {
        app.positions_str = positions_str;
    }
    let (portfolio, network_status) = sorted_portfolio(live, app.positions_str.clone()).await;
    let series = compute_weekly_series_batch(&portfolio, live, now).await;
    app.historic_data = Some(series);
    app.apply_refresh(portfolio, network_status);
}

/// Start the background tasks that keep `app` fresh: a revaluation every
/// `portfolio_every` and a chart rebuild every `history_every`. Each task
/// stops once `app` drops its receiver.
pub(super) fn spawn_background<L: LiveData>(
    app: &mut App,
    live: Arc<L>,
    portfolio_every: Duration,
    history_every: Duration,
) -> (JoinHandle<()>, JoinHandle<()>) {
    let (portfolio_sender, portfolio_receiver) = mpsc::unbounded_channel();
    app.set_portfolio_receiver(portfolio_receiver);
    let (historic_sender, historic_receiver) = mpsc::unbounded_channel();
    app.set_historic_receiver(historic_receiver);

    let path = PathBuf::from(&app.data_file_path);
    let fallback = app.positions_str.clone();
    let portfolio_live = Arc::clone(&live);
    let portfolio_path = path.clone();
    let portfolio_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(portfolio_every);
        loop {
            interval.tick().await;
            let update = poll_portfolio(&*portfolio_live, &portfolio_path, &fallback).await;
            if portfolio_sender.send(update).is_err() {
                break; // Channel closed, exit task
            }
        }
    });
    let history_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(history_every);
        loop {
            interval.tick().await;
            if let Some(series) = poll_history(&*live, &path, Utc::now()).await {
                if historic_sender.send(series).is_err() {
                    break;
                }
            }
        }
    });
    (portfolio_task, history_task)
}

/// Draw, apply background updates and handle one event per iteration until
/// the user quits. `next_event` returns `None` when no event is pending.
pub(super) async fn run_app<B: Backend, L: LiveData>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    live: &L,
    mut next_event: impl FnMut() -> io::Result<Option<Event>>,
) -> Result<()> {
    loop {
        terminal
            .draw(|f| ui(f, app))
            .map_err(|e| eyre!("failed to draw terminal frame: {e:?}"))?;

        // Check for updates from the background tasks (non-blocking)
        app.try_receive_portfolio_update();
        app.try_receive_historic_update();

        if let Some(Event::Key(key)) = next_event()? {
            if handle_key(app, key) == Action::Reload {
                reload(app, live, Utc::now()).await;
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

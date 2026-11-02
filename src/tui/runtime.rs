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

/// The error for a frame the terminal failed to draw.
fn draw_error(error: impl std::fmt::Debug) -> eyre::Report {
    eyre!("failed to draw terminal frame: {error:?}")
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
        terminal.draw(|f| ui(f, app)).map_err(draw_error)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::{buffer_text, now, portfolio, FixtureLive, POSITIONS};
    use crate::tui::AppMode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    fn launch(positions_str: &str, path: &Path, disabled: &[&str]) -> Launch {
        Launch {
            portfolio: Portfolio::new(),
            currency: "USD".into(),
            positions_str: positions_str.into(),
            data_file_path: path.to_string_lossy().into(),
            tab: None,
            disabled_components: disabled.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn key(c: char) -> io::Result<Option<Event>> {
        Ok(Some(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        ))))
    }

    /// Events served in order; quits once the script runs out.
    fn script(events: Vec<io::Result<Option<Event>>>) -> impl FnMut() -> io::Result<Option<Event>> {
        let mut events = events.into_iter();
        move || events.next().unwrap_or_else(|| key('q'))
    }

    #[tokio::test]
    async fn prepare_app_rejects_unknown_components() {
        let path = Path::new("/nonexistent/p.json");
        let err = prepare_app(
            &FixtureLive::new(),
            launch("[]", path, &["help", "nope"]),
            now(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "Invalid disabled components: Unknown component: 'nope'"
        );
    }

    #[tokio::test]
    async fn prepare_app_sets_tab_components_and_chart() {
        let path = Path::new("/nonexistent/p.json");
        let mut start = launch(POSITIONS, path, &["help"]);
        start.portfolio = portfolio(POSITIONS).await;
        start.tab = Some(Tab::Balances);
        let app = prepare_app(&FixtureLive::new(), start, now())
            .await
            .unwrap();
        assert_eq!(app.current_tab, Tab::Balances);
        assert!(app
            .disabled_components
            .is_disabled(crate::tui::Component::Help));
        assert_eq!(app.historic_data.as_ref().map(Vec::len), Some(22));
        assert_eq!(app.portfolio.as_ref().unwrap().positions.len(), 4);
        assert_eq!(app.positions_str, POSITIONS);

        // No purchases, no chart; the default tab.
        let app = prepare_app(&FixtureLive::new(), launch("[]", path, &[]), now())
            .await
            .unwrap();
        assert_eq!(app.current_tab, Tab::Overview);
        assert_eq!(app.historic_data, None);
    }

    #[tokio::test]
    async fn polling_prefers_the_file_and_falls_back() {
        let live = FixtureLive::new();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        let (fallback, status) = poll_portfolio(&live, &missing, POSITIONS).await;
        assert_eq!(status, NetworkStatus::Connected);
        // Sorted by value: Stocks 1000, Cash 1000, Bonds 900, Gold 200.
        let names: Vec<&str> = fallback.positions.iter().map(|p| p.get_name()).collect();
        assert_eq!(names.last(), Some(&"Gold"));
        assert_eq!(names.len(), 4);
        assert_eq!(poll_history(&live, &missing, now()).await, None);

        let file = dir.path().join("p.json");
        std::fs::write(
            &file,
            r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 7}]"#,
        )
        .unwrap();
        let (from_file, _) = poll_portfolio(&live, &file, POSITIONS).await;
        assert_eq!(from_file.get_total_value(), 7.0);
        // Without dated purchases the chart is empty, but still an update.
        assert_eq!(poll_history(&live, &file, now()).await, Some(vec![]));
    }

    #[tokio::test]
    async fn reload_reads_the_file_and_refreshes_everything() {
        let live = FixtureLive::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.json");
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            file.to_string_lossy().into(),
            DisabledComponents::default(),
        );
        // Unreadable file: the known content is revalued.
        reload(&mut app, &live, now()).await;
        assert_eq!(app.positions_str, "[]");
        assert_eq!(app.historic_data, Some(vec![]));
        assert!(app.flash_state);

        std::fs::write(&file, POSITIONS).unwrap();
        reload(&mut app, &live, now()).await;
        assert_eq!(app.positions_str, POSITIONS);
        assert_eq!(app.portfolio.as_ref().unwrap().get_total_value(), 3100.0);
        assert_eq!(app.historic_data.as_ref().map(Vec::len), Some(22));
        assert_eq!(app.trends.len(), 4);
        assert!(!app.flash_state);
    }

    #[tokio::test]
    async fn background_tasks_send_updates_until_the_app_hangs_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.json");
        std::fs::write(&file, POSITIONS).unwrap();
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            file.to_string_lossy().into(),
            DisabledComponents::default(),
        );
        let every = Duration::from_millis(1);
        let (portfolio_task, history_task) =
            spawn_background(&mut app, Arc::new(FixtureLive::new()), every, every);

        let (portfolio, status) = app
            .portfolio_receiver
            .as_mut()
            .unwrap()
            .recv()
            .await
            .unwrap();
        assert_eq!(portfolio.get_total_value(), 3100.0);
        assert_eq!(status, NetworkStatus::Connected);
        let series = app
            .historic_receiver
            .as_mut()
            .unwrap()
            .recv()
            .await
            .unwrap();
        assert!(!series.is_empty());

        // Dropping the receivers stops both tasks.
        drop(app);
        let limit = Duration::from_secs(30);
        tokio::time::timeout(limit, portfolio_task)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(limit, history_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn run_app_draws_handles_keys_and_quits() {
        let live = FixtureLive::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.json");
        std::fs::write(&file, POSITIONS).unwrap();
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            file.to_string_lossy().into(),
            DisabledComponents::default(),
        );
        // A pending background update is applied on the first iteration.
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(vec![(0.0, 1.0)]).unwrap();
        app.set_historic_receiver(rx);

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let events = script(vec![
            Ok(None),
            Ok(Some(Event::Resize(120, 30))),
            key('r'),
            key('2'),
        ]);
        run_app(&mut terminal, &mut app, &live, events)
            .await
            .unwrap();

        assert!(app.should_quit);
        assert_eq!(app.current_tab, Tab::Balances);
        assert_eq!(app.positions_str, POSITIONS);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("Portfolio Balances"), "{screen}");
        assert!(screen.contains("Bonds"), "{screen}");
    }

    #[tokio::test]
    async fn run_app_reports_event_errors() {
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            "/nonexistent/p.json".into(),
            DisabledComponents::default(),
        );
        app.mode = AppMode::Normal;
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let events = script(vec![Err(io::Error::other("tty gone"))]);
        let err = run_app(&mut terminal, &mut app, &FixtureLive::new(), events)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "tty gone");
        assert!(!app.should_quit);
    }

    #[test]
    fn draw_errors_name_the_frame() {
        let err = draw_error(io::Error::other("screen gone"));
        assert_eq!(
            err.to_string(),
            r#"failed to draw terminal frame: Custom { kind: Other, error: "screen gone" }"#
        );
    }

    #[tokio::test]
    async fn history_task_waits_for_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.json");
        let mut app = App::new(
            "USD".into(),
            POSITIONS.into(),
            file.to_string_lossy().into(),
            DisabledComponents::default(),
        );
        let every = Duration::from_millis(1);
        let (_, history_task) =
            spawn_background(&mut app, Arc::new(FixtureLive::new()), every, every);
        // The single-threaded test runtime runs the first, fileless rebuild now.
        tokio::task::yield_now().await;
        assert!(app.historic_receiver.as_mut().unwrap().try_recv().is_err());
        // The revaluation falls back to the known content meanwhile.
        let (portfolio, _) = app
            .portfolio_receiver
            .as_mut()
            .unwrap()
            .recv()
            .await
            .unwrap();
        assert_eq!(portfolio.get_total_value(), 3100.0);

        std::fs::write(&file, POSITIONS).unwrap();
        let series = app
            .historic_receiver
            .as_mut()
            .unwrap()
            .recv()
            .await
            .unwrap();
        assert_eq!(series.last().map(|p| p.1), Some(3100.0));
        drop(app);
        tokio::time::timeout(Duration::from_secs(30), history_task)
            .await
            .unwrap()
            .unwrap();
    }
}

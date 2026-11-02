//! Offline fixtures shared by the TUI tests.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use ratatui::{backend::TestBackend, Terminal};

use super::live::LiveData;
use super::render::ui;
use super::{App, DisabledComponents, NetworkStatus};
use crate::portfolio::Portfolio;
use crate::services::portfolio_loader::create_live_portfolio_with;
use crate::services::quotes::{AnyProvider, FixtureProvider};

/// Fixture quotes: SPY gains (also today), TLT loses (also today), GLD has
/// no previous close.
pub(super) const QUOTES: &str = r#"{
  "latest": {"SPY": {"price": 500.0, "currency": "USD"},
             "TLT": {"price": 90.0, "currency": "USD"},
             "GLD": {"price": 200.0, "currency": "USD"}},
  "previous_close": {"SPY": 495.0, "TLT": 91.0},
  "close_on": {"SPY": 400.0, "TLT": 100.0, "GLD": 180.0}
}"#;

/// A portfolio covering every balances-table case: a winner, a loser, a
/// ticker without purchases (no cost basis, no daily change) and cash.
pub(super) const POSITIONS: &str = r#"[
  {"Name": "Stocks", "Ticker": "SPY", "AssetClass": "Stocks", "Amount": 2,
   "Purchases": [{"Date": "2025-01-06", "Quantity": 1},
                 {"Date": "2025-03-03", "Quantity": 1, "Price": 450}]},
  {"Name": "Bonds", "Ticker": "TLT", "AssetClass": "Bonds", "Amount": 10,
   "Purchases": [{"Date": "2025-02-03", "Quantity": 10}]},
  {"Name": "Gold", "Ticker": "GLD", "AssetClass": "Commodities", "Amount": 1},
  {"Name": "Cash", "AssetClass": "Cash", "Amount": 1000}
]"#;

/// A fixed clock: 2025-06-02, a Monday.
pub(super) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 6, 2, 12, 0, 0).unwrap()
}

/// [`LiveData`] from fixture quotes, with optional per-ticker histories.
pub(super) struct FixtureLive {
    provider: Arc<AnyProvider>,
    pub(super) fetch_history: bool,
    pub(super) histories: HashMap<String, Vec<f64>>,
}

impl FixtureLive {
    pub(super) fn new() -> Self {
        let fixture = FixtureProvider::from_json(QUOTES).unwrap();
        Self {
            provider: Arc::new(AnyProvider::Fixture(Box::new(fixture))),
            fetch_history: false,
            histories: HashMap::new(),
        }
    }
}

impl LiveData for FixtureLive {
    async fn portfolio(&self, positions_str: String) -> (Portfolio, NetworkStatus) {
        create_live_portfolio_with(Arc::clone(&self.provider), positions_str, "USD", false).await
    }

    fn fetches_history(&self) -> bool {
        self.fetch_history
    }

    async fn history(
        &self,
        ticker: &str,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
    ) -> Option<Vec<f64>> {
        self.histories.get(ticker).cloned()
    }
}

/// `positions` valued with the fixture quotes, sorted by value.
pub(super) async fn portfolio(positions: &str) -> Portfolio {
    let (mut portfolio, _) = FixtureLive::new().portfolio(positions.to_string()).await;
    portfolio.sort_positions_by_value_desc();
    portfolio
}

/// A USD app on `positions` (not backed by a file) with its valued portfolio.
pub(super) async fn app_with(positions: &str) -> App {
    let mut app = App::new(
        "USD".to_string(),
        positions.to_string(),
        "/nonexistent/positions.json".to_string(),
        DisabledComponents::default(),
    );
    app.set_portfolio(portfolio(positions).await);
    app
}

/// `disabled` component names as [`DisabledComponents`].
pub(super) fn disabled(names: &[&str]) -> DisabledComponents {
    DisabledComponents::new(names.iter().map(|s| s.to_string()).collect()).unwrap()
}

/// The text of a `width` x `height` frame of `app`, one line per row.
pub(super) fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui(f, app)).unwrap();
    buffer_text(&terminal)
}

/// The text of the last frame drawn on `terminal`.
pub(super) fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

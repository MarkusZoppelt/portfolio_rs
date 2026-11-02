//! Where the TUI gets live data from.

use std::future::Future;

use chrono::{DateTime, Utc};

use super::NetworkStatus;
use crate::portfolio::Portfolio;

/// Live quotes and price history for the TUI.
///
/// The real implementation (`live_io::ConfiguredLive`) uses the configured
/// quote provider and Yahoo; tests inject fixture data.
pub(crate) trait LiveData: Send + Sync + 'static {
    /// The portfolio in `positions_str` valued with the latest quotes.
    fn portfolio(
        &self,
        positions_str: String,
    ) -> impl Future<Output = (Portfolio, NetworkStatus)> + Send;

    /// Whether [`Self::history`] may be called at all.
    fn fetches_history(&self) -> bool;

    /// Daily closes of `ticker` from `start` to `end`, oldest first.
    fn history(
        &self,
        ticker: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> impl Future<Output = Option<Vec<f64>>> + Send;
}

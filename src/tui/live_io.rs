//! The real [`LiveData`]: the configured quote provider and Yahoo history.
//!
//! Excluded from coverage because both calls go to the network: unit tests
//! leave `PORTFOLIO_RS_QUOTES` unset, which means Yahoo Finance. The TUI logic
//! around them is tested with fixture data through the same trait.

use std::time::Duration;

use chrono::{DateTime, Utc};
use time::OffsetDateTime;
use yahoo_finance_api as yahoo;

use super::history::fetches_yahoo_history;
use super::live::LiveData;
use super::NetworkStatus;
use crate::portfolio::Portfolio;
use crate::services::portfolio_loader::create_live_portfolio;
use crate::services::quotes::AnyProvider;

/// Live data from `PORTFOLIO_RS_QUOTES` (Yahoo Finance by default).
pub(super) struct ConfiguredLive;

impl LiveData for ConfiguredLive {
    async fn portfolio(&self, positions_str: String) -> (Portfolio, NetworkStatus) {
        create_live_portfolio(positions_str).await
    }

    fn fetches_history(&self) -> bool {
        fetches_yahoo_history(AnyProvider::configured())
    }

    async fn history(
        &self,
        ticker: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Option<Vec<f64>> {
        let start = OffsetDateTime::from_unix_timestamp(start.timestamp()).ok()?;
        let end = OffsetDateTime::from_unix_timestamp(end.timestamp()).ok()?;
        let fetch = async {
            yahoo::YahooConnector::new()?
                .get_quote_history(ticker, start, end)
                .await
        };
        // Per-ticker timeout to avoid stalls
        let response = tokio::time::timeout(Duration::from_secs(3), fetch)
            .await
            .ok()?
            .ok()?;
        Some(response.quotes().ok()?.iter().map(|q| q.close).collect())
    }
}

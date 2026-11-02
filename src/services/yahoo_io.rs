//! Live Yahoo Finance glue for [`YahooProvider`].
//!
//! Excluded from the coverage gate: every method here is a single call into
//! the network fetchers in [`crate::position`], which cannot run offline.
//! There is no logic here; the historic lookups delegate to
//! [`bars_close_on_or_after`] and [`bars_close_before`] in `quotes.rs`, and
//! [`AnyProvider`](super::quotes::AnyProvider) dispatch is tested there with a
//! stand-in backend.

use chrono::{DateTime, Utc};

use super::quotes::{
    bars_close_before, bars_close_on_or_after, DailyBar, DailyBarSource, Quote, QuoteError,
    QuoteProvider, YahooProvider,
};

impl DailyBarSource for YahooProvider {
    async fn daily_bars(
        &self,
        symbol: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyBar>, QuoteError> {
        crate::position::yahoo_daily_bars(symbol, from, to).await
    }
}

impl QuoteProvider for YahooProvider {
    async fn latest(&self, symbol: &str) -> Result<Quote, QuoteError> {
        crate::position::yahoo_latest(symbol).await
    }

    async fn previous_close(&self, symbol: &str) -> Result<f64, QuoteError> {
        crate::position::yahoo_previous_close(symbol).await
    }

    async fn close_on(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        bars_close_on_or_after(self, symbol, date).await
    }

    async fn close_before(&self, symbol: &str, date: DateTime<Utc>) -> Result<f64, QuoteError> {
        bars_close_before(self, symbol, date).await
    }

    async fn name(&self, symbol: &str) -> Option<String> {
        crate::position::yahoo_name(symbol).await
    }
}

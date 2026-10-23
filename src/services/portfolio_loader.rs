use eyre::{bail, Result, WrapErr};
use std::fs::read_to_string;

use crate::portfolio::Portfolio;
use crate::position::{apply_quote_result, from_string, handle_position, PriceStatus};
use crate::tui::NetworkStatus;

/// Read a plaintext file or decrypt a `.gpg` file.
///
/// Decrypted content is only ever held in memory; it is never written to disk.
pub fn open_encrypted_file(filename: &str) -> Result<String> {
    if filename.ends_with(".gpg") {
        let output = std::process::Command::new("gpg")
            .arg("-d")
            .arg(filename)
            .output()
            .wrap_err_with(|| format!("failed to execute gpg to decrypt {}", filename))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "gpg failed to decrypt {} ({}): {}",
                filename,
                output.status,
                stderr.trim()
            );
        }

        String::from_utf8(output.stdout).wrap_err("gpg output is not valid UTF-8")
    } else {
        read_to_string(filename).wrap_err_with(|| format!("failed to read file: {}", filename))
    }
}

/// Load portfolio JSON from a file path, supporting `.gpg` encryption.
pub fn load_portfolio_file(filename: &str) -> Result<String> {
    if filename.is_empty() {
        bail!("No portfolio file specified.");
    }
    open_encrypted_file(filename)
}

/// Returns a portfolio with the latest quotes from JSON data.
pub async fn create_live_portfolio(positions_str: String) -> (Portfolio, NetworkStatus) {
    create_live_portfolio_with_logging(positions_str, false).await
}

/// Returns a portfolio with the latest quotes, with optional per-position error logging.
pub async fn create_live_portfolio_with_logging(
    positions_str: String,
    log_errors: bool,
) -> (Portfolio, NetworkStatus) {
    let positions = match from_string(&positions_str) {
        Ok(p) => p,
        Err(e) => {
            if log_errors {
                eprintln!("Error parsing positions: {:#}", e);
            }
            return (Portfolio::new(), NetworkStatus::Disconnected);
        }
    };
    let mut portfolio = Portfolio::new();

    // Keep a clone so a panicked task (JoinError) can still be added as Unavailable.
    let tasks: Vec<_> = positions
        .into_iter()
        .map(|mut position| {
            let fallback = position.clone();
            let task = tokio::spawn(async move { handle_position(&mut position).await });
            (fallback, task)
        })
        .collect();

    for (mut fallback, task) in tasks {
        match task.await {
            Ok(Ok(position)) => portfolio.add_position(position),
            Ok(Err(_)) | Err(_) => {
                apply_quote_result(&mut fallback, None);
                portfolio.add_position(fallback);
            }
        }
    }

    // Ticker-less holdings are not quote-dependent and do not affect status.
    let mut ticker_positions = 0usize;
    let mut live_ticker_positions = 0usize;
    for position in &portfolio.positions {
        if position.get_ticker().is_none() {
            continue;
        }
        ticker_positions += 1;
        if position.price_status() == PriceStatus::Live {
            live_ticker_positions += 1;
        }
    }
    let network_status = if ticker_positions == 0 || live_ticker_positions == ticker_positions {
        NetworkStatus::Connected
    } else if live_ticker_positions == 0 {
        NetworkStatus::Disconnected
    } else {
        NetworkStatus::Partial
    };

    if log_errors {
        for position in portfolio.positions_without_price() {
            if let Some(ticker) = position.get_ticker() {
                eprintln!("Warning: no market price for {ticker}; position excluded from totals");
            }
        }
    }

    (portfolio, network_status)
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    #[tokio::test]
    async fn test_create_live_portfolio() {
        let positions_str = read_to_string("example_data.json").unwrap();
        let (portfolio, _network_status) = create_live_portfolio(positions_str).await;
        assert!(!portfolio.positions.is_empty());
    }
}

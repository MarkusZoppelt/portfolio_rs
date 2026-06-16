use eyre::{bail, Result, WrapErr};
use std::fs::read_to_string;

use crate::portfolio::Portfolio;
use crate::position::{from_string, handle_position};
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
    let mut successful_positions = 0;
    let mut failed_positions = 0;

    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| tokio::spawn(async move { handle_position(&mut position).await }))
        .collect();

    for task in tasks {
        let p = task.await;
        match p {
            Ok(p) => match p {
                Ok(p) => {
                    portfolio.add_position(p);
                    successful_positions += 1;
                }
                Err(e) => {
                    if log_errors {
                        eprintln!("Error handling position: {e:?}");
                    }
                    failed_positions += 1;
                }
            },
            Err(e) => {
                if log_errors {
                    eprintln!("Error handling position: {e:?}");
                }
                failed_positions += 1;
            }
        }
    }

    let network_status = if failed_positions == 0 {
        NetworkStatus::Connected
    } else if successful_positions == 0 {
        NetworkStatus::Disconnected
    } else {
        NetworkStatus::Partial
    };

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

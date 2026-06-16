use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

use crate::position::from_string;
use crate::services::portfolio_loader::load_portfolio_file;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    pub severity: ValidationSeverity,
    pub position: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub file_path: String,
    pub valid: bool,
    pub position_count: usize,
    pub issues: Vec<ValidationIssue>,
    pub error_count: usize,
    pub warning_count: usize,
}

/// Validate a portfolio JSON file and return a structured report.
pub fn validate_portfolio_file(file_path: &str) -> Result<ValidationReport> {
    let content = load_portfolio_file(file_path)
        .wrap_err_with(|| format!("failed to load portfolio file: {}", file_path))?;

    let positions =
        from_string(&content).wrap_err_with(|| format!("invalid JSON in {}", file_path))?;

    let mut issues = Vec::new();
    let mut seen_tickers = std::collections::HashSet::new();

    for pos in &positions {
        let name = pos.get_name();
        let asset_class = pos.get_asset_class();
        let amount = pos.get_amount();

        if name == "Unknown" {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                position: Some(name.to_string()),
                message: "Position has no Name or Ticker".to_string(),
            });
        }

        if asset_class.is_empty() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                position: Some(name.to_string()),
                message: "Position has no AssetClass".to_string(),
            });
        }

        if amount < 0.0 {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                position: Some(name.to_string()),
                message: format!("Position has negative amount: {}", amount),
            });
        }

        if amount == 0.0 {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                position: Some(name.to_string()),
                message: "Position has zero amount".to_string(),
            });
        }

        let is_cash = asset_class.eq_ignore_ascii_case("cash");
        if is_cash && pos.get_ticker().is_some() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                position: Some(name.to_string()),
                message: "Cash position should not have a ticker".to_string(),
            });
        }

        if !is_cash && pos.get_ticker().is_none() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                position: Some(name.to_string()),
                message: "Position has no ticker — live pricing unavailable".to_string(),
            });
        }

        let purchases = pos.get_purchases();
        if !purchases.is_empty() {
            let purchase_sum: f64 = purchases.iter().map(|p| p.quantity).sum();
            // Tolerate float noise from summing purchase quantities.
            if (purchase_sum - amount).abs() > 1e-6 {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    position: Some(name.to_string()),
                    message: format!(
                        "Purchase quantities ({}) do not match amount ({})",
                        purchase_sum, amount
                    ),
                });
            }

            for purchase in purchases {
                if let Some(price) = purchase.price {
                    if price < 0.0 {
                        issues.push(ValidationIssue {
                            severity: ValidationSeverity::Error,
                            position: Some(name.to_string()),
                            message: format!("Negative purchase price: {}", price),
                        });
                    }
                }
            }
        }

        if let Some(ticker) = pos.get_ticker() {
            if !seen_tickers.insert(ticker) {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    position: Some(name.to_string()),
                    message: format!("Duplicate ticker '{}' in portfolio", ticker),
                });
            }
        }
    }

    let error_count = issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Error)
        .count();
    let warning_count = issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Warning)
        .count();

    Ok(ValidationReport {
        file_path: file_path.to_string(),
        valid: error_count == 0,
        position_count: positions.len(),
        issues,
        error_count,
        warning_count,
    })
}

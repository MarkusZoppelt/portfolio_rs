use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

use crate::money::to_f64;
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
        .wrap_err_with(|| format!("failed to load portfolio file: {file_path}"))?;

    let positions =
        from_string(&content).wrap_err_with(|| format!("invalid JSON in {file_path}"))?;

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
                message: format!("Position has negative amount: {amount}"),
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

        // With purchases, `amount` is already their quantity sum, so the two
        // cannot disagree here.
        for purchase in pos.get_purchases() {
            if let Some(price) = purchase.price.map(to_f64) {
                if price < 0.0 {
                    issues.push(ValidationIssue {
                        severity: ValidationSeverity::Error,
                        position: Some(name.to_string()),
                        message: format!("Negative purchase price: {price}"),
                    });
                }
            }
        }

        if let Some(ticker) = pos.get_ticker() {
            if !seen_tickers.insert(ticker) {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    position: Some(name.to_string()),
                    message: format!("Duplicate ticker '{ticker}' in portfolio"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(json: &str) -> ValidationReport {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        std::fs::write(&path, json).unwrap();
        validate_portfolio_file(path.to_str().unwrap()).unwrap()
    }

    fn messages(report: &ValidationReport) -> Vec<(ValidationSeverity, String, String)> {
        report
            .issues
            .iter()
            .map(|i| (i.severity, i.position.clone().unwrap(), i.message.clone()))
            .collect()
    }

    #[test]
    fn test_clean_file_is_valid_without_issues() {
        let report = validate(
            r#"[{"Name": "Fake ETF", "Ticker": "FAKEETF", "AssetClass": "Stocks",
                 "Amount": 2, "Purchases": [{"Quantity": 2, "Price": 10}]},
                {"Name": "Cash", "AssetClass": "Cash", "Amount": 100}]"#,
        );
        assert!(report.valid);
        assert_eq!(report.position_count, 2);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        assert_eq!((report.error_count, report.warning_count), (0, 0));
    }

    #[test]
    fn test_every_issue_is_reported() {
        use ValidationSeverity::{Error, Warning};
        let report = validate(
            r#"[{"AssetClass": "Stocks", "Amount": 1},
                {"Name": "No Class", "Ticker": "FAKEA", "AssetClass": "", "Amount": -1},
                {"Name": "Zero", "Ticker": "FAKEB", "AssetClass": "Stocks", "Amount": 0},
                {"Name": "Ticker Cash", "Ticker": "FAKEC", "AssetClass": "cash", "Amount": 5},
                {"Name": "Bad Price", "Ticker": "FAKEA", "AssetClass": "Stocks", "Amount": 1,
                 "Purchases": [{"Quantity": 1, "Price": -3}, {"Quantity": 0}]}]"#,
        );
        let s = |x: &str| x.to_string();
        assert_eq!(
            messages(&report),
            [
                (Warning, s("Unknown"), s("Position has no Name or Ticker")),
                (
                    Warning,
                    s("Unknown"),
                    s("Position has no ticker — live pricing unavailable")
                ),
                (Error, s("No Class"), s("Position has no AssetClass")),
                (Error, s("No Class"), s("Position has negative amount: -1")),
                (Warning, s("Zero"), s("Position has zero amount")),
                (
                    Warning,
                    s("Ticker Cash"),
                    s("Cash position should not have a ticker")
                ),
                (Error, s("Bad Price"), s("Negative purchase price: -3")),
                (
                    Warning,
                    s("Bad Price"),
                    s("Duplicate ticker 'FAKEA' in portfolio")
                ),
            ]
        );
        assert!(!report.valid);
        assert_eq!((report.error_count, report.warning_count), (3, 5));
    }

    #[test]
    fn test_unreadable_or_invalid_files_are_errors() {
        let err = validate_portfolio_file("/nonexistent/positions.json").unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to load portfolio file: /nonexistent/positions.json"
        );

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        std::fs::write(&path, "not json").unwrap();
        let path = path.to_str().unwrap();
        let err = validate_portfolio_file(path).unwrap_err();
        assert_eq!(err.to_string(), format!("invalid JSON in {path}"));
    }
}

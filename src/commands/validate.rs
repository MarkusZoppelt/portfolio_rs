use clap::ArgMatches;
use eyre::{bail, Result};

use crate::commands::resolve_portfolio_file;
use crate::validate::{validate_portfolio_file, ValidationIssue, ValidationSeverity};
use crate::AppConfig;

/// Exit codes: 0 = valid, 1 = usage error, 2 = validation failed.
pub async fn handle_validate(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(sub_matches, cfg);
    if filename.is_empty() {
        bail!("no portfolio file specified");
    }

    let report = match validate_portfolio_file(&filename) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("Error: failed to validate portfolio file: {e:#}");
            std::process::exit(2);
        }
    };

    if !report.valid {
        eprintln!(
            "\n❌ Validation failed with {} errors and {} warnings.",
            report.error_count, report.warning_count
        );
        for issue in &report.issues {
            let icon = match issue.severity {
                ValidationSeverity::Error => "❌",
                ValidationSeverity::Warning => "⚠️",
            };
            eprintln!("{}", issue_line(icon, issue));
        }
        std::process::exit(2);
    }

    if report.warning_count > 0 {
        println!(
            "⚠️  Portfolio file is valid but has {} warnings.",
            report.warning_count
        );
        for issue in &report.issues {
            eprintln!("{}", issue_line("⚠️ ", issue));
        }
    } else {
        println!("✅ Portfolio file is valid: {filename}");
        println!("   Positions: {}", report.position_count);
    }
    Ok(())
}

/// `<icon> <position>: <message>`, or `<icon> <message>` for an issue that
/// is not about one position.
fn issue_line(icon: &str, issue: &ValidationIssue) -> String {
    match &issue.position {
        Some(position) => format!("{icon} {position}: {}", issue.message),
        None => format!("{icon} {}", issue.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(position: Option<&str>) -> ValidationIssue {
        ValidationIssue {
            severity: ValidationSeverity::Warning,
            position: position.map(str::to_string),
            message: "Position has zero amount".to_string(),
        }
    }

    #[test]
    fn issue_line_names_the_position_when_there_is_one() {
        assert_eq!(
            issue_line("❌", &issue(Some("Fake ETF"))),
            "❌ Fake ETF: Position has zero amount"
        );
        assert_eq!(
            issue_line("⚠️ ", &issue(None)),
            "⚠️  Position has zero amount"
        );
    }
}

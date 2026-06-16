use clap::ArgMatches;
use eyre::{bail, Result};

use crate::commands::resolve_portfolio_file;
use crate::validate::{validate_portfolio_file, ValidationSeverity};
use crate::AppConfig;

/// Exit codes: 0 = valid, 1 = usage error, 2 = validation failed.
pub async fn handle_validate(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let filename = resolve_portfolio_file(Some(sub_matches), cfg);
    if filename.is_empty() {
        bail!("no portfolio file specified");
    }

    let report = match validate_portfolio_file(&filename) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("Error: failed to validate portfolio file: {:#}", e);
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
            if let Some(position) = &issue.position {
                eprintln!("{} {}: {}", icon, position, issue.message);
            } else {
                eprintln!("{} {}", icon, issue.message);
            }
        }
        std::process::exit(2);
    }

    if report.warning_count > 0 {
        println!(
            "⚠️  Portfolio file is valid but has {} warnings.",
            report.warning_count
        );
        for issue in &report.issues {
            if let Some(position) = &issue.position {
                eprintln!("⚠️  {}: {}", position, issue.message);
            } else {
                eprintln!("⚠️  {}", issue.message);
            }
        }
    } else {
        println!("✅ Portfolio file is valid: {}", filename);
        println!("   Positions: {}", report.position_count);
    }
    Ok(())
}

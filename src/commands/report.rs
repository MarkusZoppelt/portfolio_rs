use clap::ArgMatches;
use eyre::{bail, Result};
use std::path::Path;

use crate::commands::{defaulted_arg, resolve_portfolio_file};
use crate::document::{today, weekly_report_path, write_dated_document};
use crate::report::generate_markdown_report;
use crate::AppConfig;

pub async fn handle_report(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("weekly", weekly_matches)) => {
            let filename = resolve_portfolio_file(weekly_matches, cfg);
            let policy_file = defaulted_arg(weekly_matches, "policy");
            let dir = defaulted_arg(weekly_matches, "dir");
            let dry_run = weekly_matches.get_flag("dry-run");

            report_weekly(filename, policy_file, dir, dry_run, cfg).await
        }
        // `subcommand_required` in the CLI definition guarantees a subcommand.
        _ => bail!("unknown report subcommand"),
    }
}

async fn report_weekly(
    filename: String,
    policy_file: &str,
    dir: &str,
    dry_run: bool,
    cfg: &AppConfig,
) -> Result<()> {
    crate::commands::check_positions_file(&filename)?;
    let date = today();
    let file_path = weekly_report_path(Path::new(dir), &date);
    let content = generate_markdown_report(&filename, policy_file, &date, &cfg.currency).await;

    write_dated_document(
        &file_path,
        &content,
        dry_run,
        "weekly report",
        "Remove it to regenerate.",
    )?;

    if dry_run {
        println!("Dry-run: would create report at {}", file_path.display());
        println!("\n{content}");
    } else {
        println!("Created weekly report at {}", file_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    /// Without a portfolio file the draft needs no quotes or config.
    #[tokio::test]
    async fn draft_without_a_portfolio_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();
        let cli = |dry_run: bool| {
            let mut args = vec!["portfolio_rs", "report", "weekly", "--dir", dir];
            if dry_run {
                args.push("--dry-run");
            }
            let matches = crate::cli::build_cli().get_matches_from(args);
            matches.subcommand_matches("report").unwrap().clone()
        };
        let cfg = AppConfig::default();
        let docs = tmp.path().join("portfolio");

        handle_report(&cli(true), &cfg).await.unwrap();
        assert!(!docs.exists());
        handle_report(&cli(false), &cfg).await.unwrap();
        let written: Vec<_> = walk(&docs);
        assert_eq!(written.len(), 1, "{written:?}");
        // Same day, same name: refused.
        assert!(handle_report(&cli(false), &cfg).await.is_err());
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    #[tokio::test]
    async fn missing_subcommand_is_an_error() {
        let matches = Command::new("report").get_matches_from(["report"]);
        let err = handle_report(&matches, &AppConfig::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "unknown report subcommand");
    }
}

use clap::ArgMatches;
use eyre::{bail, Result};
use std::path::Path;

use crate::commands::resolve_portfolio_file;
use crate::document::write_dated_document;
use crate::report::generate_markdown_report;
use crate::AppConfig;

pub async fn handle_report(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("weekly", weekly_matches)) => {
            let filename = resolve_portfolio_file(Some(weekly_matches), cfg);
            let policy_file = weekly_matches
                .get_one::<String>("policy")
                .map(|s| s.as_str())
                .unwrap_or("portfolio/policy.toml");
            let dir = weekly_matches
                .get_one::<String>("dir")
                .map(|s| s.as_str())
                .unwrap_or("portfolio");
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
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let file_path = Path::new(dir).join(format!("portfolio/reports/{}-weekly.md", date));
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
        println!("\n{}", content);
    } else {
        println!("Created weekly report at {}", file_path.display());
    }

    Ok(())
}

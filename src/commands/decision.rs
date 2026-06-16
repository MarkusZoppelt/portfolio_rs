use clap::ArgMatches;
use eyre::{bail, Result};
use std::path::Path;

use crate::commands::resolve_portfolio_file;
use crate::decision::generate_decision_draft;
use crate::document::write_dated_document;
use crate::AppConfig;

pub async fn handle_decision(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("draft", draft_matches)) => {
            let filename = resolve_portfolio_file(Some(draft_matches), cfg);
            let policy_file = draft_matches
                .get_one::<String>("policy")
                .map(|s| s.as_str())
                .unwrap_or("portfolio/policy.toml");
            let title = draft_matches
                .get_one::<String>("title")
                .map(|s| s.to_string());
            let dir = draft_matches
                .get_one::<String>("dir")
                .map(|s| s.as_str())
                .unwrap_or("portfolio");
            let dry_run = draft_matches.get_flag("dry-run");

            decision_draft(filename, policy_file.to_string(), title, dir, dry_run, cfg).await
        }
        // `subcommand_required` in the CLI definition guarantees a subcommand.
        _ => bail!("unknown decision subcommand"),
    }
}

async fn decision_draft(
    filename: String,
    policy_file: String,
    title: Option<String>,
    dir: &str,
    dry_run: bool,
    cfg: &AppConfig,
) -> Result<()> {
    let title = title.unwrap_or_else(|| "Portfolio Rebalance Review".to_string());
    let slug = title
        .to_lowercase()
        .replace(' ', "-")
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
    let file_path = Path::new(dir).join(format!(
        "portfolio/decisions/{}-{}.md",
        chrono::Local::now().format("%Y-%m-%d"),
        slug
    ));

    let content = generate_decision_draft(&filename, &policy_file, &title, &cfg.currency).await;

    write_dated_document(
        &file_path,
        &content,
        dry_run,
        "decision record",
        "Use a different title.",
    )?;

    if dry_run {
        println!(
            "Dry-run: would create decision record at {}",
            file_path.display()
        );
        println!("\n{}", content);
    } else {
        println!("Created decision record at {}", file_path.display());
    }

    Ok(())
}

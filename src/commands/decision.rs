use clap::ArgMatches;
use eyre::{bail, Result};
use std::path::Path;

use crate::commands::{defaulted_arg, resolve_portfolio_file};
use crate::decision::generate_decision_draft;
use crate::document::{decision_path, write_dated_document, DEFAULT_DECISION_TITLE};
use crate::AppConfig;

pub async fn handle_decision(sub_matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("draft", draft_matches)) => {
            let filename = resolve_portfolio_file(draft_matches, cfg);
            let policy_file = defaulted_arg(draft_matches, "policy");
            let title = draft_matches
                .get_one::<String>("title")
                .map_or(DEFAULT_DECISION_TITLE, String::as_str);
            let dir = defaulted_arg(draft_matches, "dir");
            let dry_run = draft_matches.get_flag("dry-run");

            decision_draft(filename, policy_file, title, dir, dry_run, cfg).await
        }
        // `subcommand_required` in the CLI definition guarantees a subcommand.
        _ => bail!("unknown decision subcommand"),
    }
}

async fn decision_draft(
    filename: String,
    policy_file: &str,
    title: &str,
    dir: &str,
    dry_run: bool,
    cfg: &AppConfig,
) -> Result<()> {
    crate::commands::exit_if_file_unparsable(&filename);
    let file_path = decision_path(Path::new(dir), title);

    let content = generate_decision_draft(&filename, policy_file, title, &cfg.currency).await;

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
        println!("\n{content}");
    } else {
        println!("Created decision record at {}", file_path.display());
    }

    Ok(())
}

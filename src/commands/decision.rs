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
    crate::commands::check_positions_file(&filename)?;
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
            let mut args = vec![
                "portfolio_rs",
                "decision",
                "draft",
                "--title",
                "Fake Title",
                "--dir",
                dir,
            ];
            if dry_run {
                args.push("--dry-run");
            }
            let matches = crate::cli::build_cli().get_matches_from(args);
            matches.subcommand_matches("decision").unwrap().clone()
        };
        let cfg = AppConfig::default();
        let docs = tmp.path().join("portfolio");

        handle_decision(&cli(true), &cfg).await.unwrap();
        assert!(!docs.exists());
        handle_decision(&cli(false), &cfg).await.unwrap();
        let written: Vec<_> = walk(&docs);
        assert_eq!(written.len(), 1, "{written:?}");
        // Same day, same name: refused.
        assert!(handle_decision(&cli(false), &cfg).await.is_err());
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
        let matches = Command::new("decision").get_matches_from(["decision"]);
        let err = handle_decision(&matches, &AppConfig::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "unknown decision subcommand");
    }
}

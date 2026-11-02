use crate::agent_skill::{skill_export, skill_path, skill_show};
use crate::commands::defaulted_arg;
use crate::workspace::init_agent_files;
use clap::ArgMatches;
use eyre::{bail, Result};

pub fn handle_agent(sub_matches: &ArgMatches) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("init", init_matches)) => init_agent_files(
            defaulted_arg(init_matches, "DIR"),
            init_matches.get_flag("dry-run"),
        ),
        Some(("skill", skill_matches)) => match skill_matches.subcommand() {
            Some(("show", _)) => {
                skill_show();
                Ok(())
            }
            Some(("export", export_matches)) => skill_export(
                defaulted_arg(export_matches, "DIR"),
                export_matches.get_flag("dry-run"),
            ),
            Some(("path", _)) => {
                skill_path();
                Ok(())
            }
            // `subcommand_required` in the CLI definition guarantees a subcommand.
            _ => bail!("usage: portfolio_rs agent skill <show | export | path>"),
        },
        _ => bail!("usage: portfolio_rs agent <init | skill>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    /// Matches for `agent <args>` from a CLI without required subcommands,
    /// as a library caller could build them.
    fn matches(args: &[&str]) -> ArgMatches {
        Command::new("agent")
            .subcommand(Command::new("skill").subcommand(Command::new("other")))
            .subcommand(Command::new("other"))
            .get_matches_from(std::iter::once("agent").chain(args.iter().copied()))
    }

    fn cli(args: &[&str]) -> ArgMatches {
        let matches = crate::cli::build_cli()
            .get_matches_from(["portfolio_rs", "agent"].iter().chain(args).copied());
        matches.subcommand_matches("agent").unwrap().clone()
    }

    #[test]
    fn every_agent_subcommand_runs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();

        handle_agent(&cli(&["init", dir, "--dry-run"])).unwrap();
        assert!(!tmp.path().join("AGENTS.md").exists());
        handle_agent(&cli(&["init", dir])).unwrap();
        assert!(tmp.path().join("AGENTS.md").is_file());

        handle_agent(&cli(&["skill", "export", dir, "--dry-run"])).unwrap();
        assert!(!tmp.path().join("portfolio-rs").exists());
        handle_agent(&cli(&["skill", "export", dir])).unwrap();
        assert!(tmp.path().join("portfolio-rs/SKILL.md").is_file());

        handle_agent(&cli(&["skill", "show"])).unwrap();
        handle_agent(&cli(&["skill", "path"])).unwrap();

        let missing = tmp.path().join("missing");
        assert!(handle_agent(&cli(&["init", missing.to_str().unwrap()])).is_err());
    }

    #[test]
    fn missing_or_unknown_subcommand_is_a_usage_error() {
        for args in [&[][..], &["other"]] {
            let err = handle_agent(&matches(args)).unwrap_err();
            assert_eq!(err.to_string(), "usage: portfolio_rs agent <init | skill>");
        }
        for args in [&["skill"][..], &["skill", "other"]] {
            let err = handle_agent(&matches(args)).unwrap_err();
            assert_eq!(
                err.to_string(),
                "usage: portfolio_rs agent skill <show | export | path>"
            );
        }
    }
}

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

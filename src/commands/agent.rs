use crate::agent_skill::{skill_export, skill_path, skill_show};
use crate::commands::defaulted_arg;
use crate::workspace::init_agent_files;
use clap::ArgMatches;
use eyre::{bail, Result};

pub fn handle_agent(sub_matches: &ArgMatches) -> Result<()> {
    if let Some(init_matches) = sub_matches.subcommand_matches("init") {
        let dir = defaulted_arg(init_matches, "DIR");
        let dry_run = init_matches.get_flag("dry-run");
        return init_agent_files(dir, dry_run);
    }

    if let Some(skill_matches) = sub_matches.subcommand_matches("skill") {
        if skill_matches.subcommand_matches("show").is_some() {
            skill_show();
            return Ok(());
        }
        if let Some(export_matches) = skill_matches.subcommand_matches("export") {
            let dir = defaulted_arg(export_matches, "DIR");
            let dry_run = export_matches.get_flag("dry-run");
            return skill_export(dir, dry_run);
        }
        if skill_matches.subcommand_matches("path").is_some() {
            skill_path();
            return Ok(());
        }
    }

    // `subcommand_required` in the CLI definition guarantees a subcommand.
    bail!("usage: portfolio_rs agent <init | skill>")
}

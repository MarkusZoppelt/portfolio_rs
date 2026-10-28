use crate::commands::defaulted_arg;
use crate::workspace::init_workspace;
use clap::ArgMatches;
use eyre::Result;

pub fn handle_init_workspace(sub_matches: &ArgMatches) -> Result<()> {
    init_workspace(
        defaulted_arg(sub_matches, "DIR"),
        sub_matches.get_flag("dry-run"),
    )
}

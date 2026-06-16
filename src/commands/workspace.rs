use crate::workspace::init_workspace;
use clap::ArgMatches;
use eyre::Result;

pub fn handle_init_workspace(sub_matches: &ArgMatches) -> Result<()> {
    let dir = sub_matches
        .get_one::<String>("DIR")
        .map(|s| s.as_str())
        .unwrap_or("portfolio");
    let dry_run = sub_matches.get_flag("dry-run");
    init_workspace(dir, dry_run)
}

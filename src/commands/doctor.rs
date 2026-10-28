use crate::commands::defaulted_arg;
use crate::doctor::{check_workspace, HealthStatus};
use clap::ArgMatches;
use eyre::Result;

pub fn handle_doctor(sub_matches: &ArgMatches) -> Result<()> {
    let dir = defaulted_arg(sub_matches, "DIR");

    let health = check_workspace(dir)?;

    println!("# Workspace Health Check\n");
    println!("Checking: {}\n", health.dir);

    for check in &health.checks {
        let icon = match check.status {
            HealthStatus::Ok => "✅",
            HealthStatus::Warning => "⚠️",
            HealthStatus::Error => "❌",
        };
        println!("{} {}", icon, check.message);
    }

    println!("\n---");
    println!(
        "Issues: {}, Warnings: {}",
        health.issue_count, health.warning_count
    );

    if health.issue_count == 0 && health.warning_count == 0 {
        println!("Workspace looks healthy!");
    } else if health.issue_count > 0 {
        println!("Please address the issues above.");
    } else {
        println!("Workspace is functional but has warnings.");
    }
    Ok(())
}

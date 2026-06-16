use crate::policy::{policy_from_strategy, Policy};
use clap::ArgMatches;
use eyre::{bail, Result, WrapErr};

pub fn handle_policy(sub_matches: &ArgMatches) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("init", policy_matches)) => {
            let strategy = policy_matches
                .get_one::<String>("strategy")
                .map(|s| s.as_str())
                .unwrap_or("balanced-growth");
            let dir = policy_matches
                .get_one::<String>("DIR")
                .map(|s| s.as_str())
                .unwrap_or("portfolio");
            let dry_run = policy_matches.get_flag("dry-run");
            policy_init(strategy, dir, dry_run)
        }
        Some(("validate", policy_matches)) => {
            let file = policy_matches
                .get_one::<String>("FILE")
                .map(|s| s.as_str())
                .unwrap_or("portfolio/policy.toml");
            let policy = Policy::from_file(file)
                .wrap_err_with(|| format!("policy validation failed for {}", file))?;

            println!("Policy file is valid: {}", file);
            println!("  Name: {}", policy.name);
            println!("  Currency: {}", policy.base_currency);
            println!("  Risk profile: {}", policy.risk_profile);
            println!("  Time horizon: {} years", policy.time_horizon_years);
            println!("  Allocations:");
            for alloc in &policy.allocations {
                let tol = alloc
                    .tolerance_percent
                    .map(|t| format!(" ±{:.0}%", t))
                    .unwrap_or_default();
                println!(
                    "    {}: {:.0}% {}",
                    alloc.asset_class, alloc.target_percent, tol
                );
            }
            Ok(())
        }
        // `subcommand_required` in the CLI definition guarantees a subcommand.
        _ => bail!("unknown policy subcommand"),
    }
}

pub fn policy_init(strategy: &str, dir: &str, dry_run: bool) -> Result<()> {
    let policy = policy_from_strategy(strategy)
        .ok_or_else(|| eyre::eyre!("unknown strategy: {}", strategy))?;

    let policy_path = std::path::Path::new(dir).join("portfolio/policy.toml");

    if policy_path.exists() {
        bail!(
            "policy file already exists at {}. Remove it first or use a different directory.",
            policy_path.display()
        );
    }

    let toml_content = policy.to_toml()?;

    if dry_run {
        println!("Dry-run: would create policy at {}", policy_path.display());
        println!("\n{}", toml_content);
    } else {
        if let Some(parent) = policy_path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create directory: {}", parent.display()))?;
        }
        std::fs::write(&policy_path, toml_content)
            .wrap_err_with(|| format!("failed to write policy file: {}", policy_path.display()))?;
        println!("Created policy at {}", policy_path.display());
        println!("Strategy: {}", policy.name);
        println!("Risk profile: {}", policy.risk_profile);
        println!("\nNext steps:");
        println!(
            "  1. Edit {} to customize your policy.",
            policy_path.display()
        );
        println!(
            "  2. Run: portfolio_rs policy validate {}",
            policy_path.display()
        );
    }

    Ok(())
}

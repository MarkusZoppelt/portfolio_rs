use std::fs;
use std::path::Path;

use clap::ArgMatches;
use eyre::{bail, eyre, Result, WrapErr};

use crate::commands::defaulted_arg;
use crate::policy::{policy_from_strategy, Policy};
use crate::services::atomic_write::write_new_private;

pub fn handle_policy(sub_matches: &ArgMatches) -> Result<()> {
    match sub_matches.subcommand() {
        Some(("init", policy_matches)) => {
            let strategy = defaulted_arg(policy_matches, "strategy");
            let dir = defaulted_arg(policy_matches, "DIR");
            let dry_run = policy_matches.get_flag("dry-run");
            policy_init(strategy, dir, dry_run)
        }
        Some(("validate", policy_matches)) => {
            let file = defaulted_arg(policy_matches, "FILE");
            let policy = Policy::from_file(file)
                .wrap_err_with(|| format!("policy validation failed for {file}"))?;

            println!("Policy file is valid: {file}");
            println!("  Name: {}", policy.name);
            println!("  Currency: {}", policy.base_currency);
            println!("  Risk profile: {}", policy.risk_profile);
            println!("  Time horizon: {} years", policy.time_horizon_years);
            println!("  Allocations:");
            for alloc in &policy.allocations {
                let tol = alloc
                    .tolerance_percent
                    .map(|t| format!(" ±{t:.0}%"))
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
    let policy =
        policy_from_strategy(strategy).ok_or_else(|| eyre!("unknown strategy: {strategy}"))?;

    let policy_dir = Path::new(dir).join("portfolio");
    let policy_path = policy_dir.join("policy.toml");

    if policy_path.exists() {
        bail!(
            "policy file already exists at {}. Remove it first or use a different directory.",
            policy_path.display()
        );
    }

    let toml_content = policy.to_toml()?;

    if dry_run {
        println!("Dry-run: would create policy at {}", policy_path.display());
        println!("\n{toml_content}");
    } else {
        fs::create_dir_all(&policy_dir)
            .wrap_err_with(|| format!("failed to create directory: {}", policy_dir.display()))?;
        write_new_private(&policy_path, toml_content.as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    fn cli(args: &[&str]) -> ArgMatches {
        let matches = crate::cli::build_cli()
            .get_matches_from(["portfolio_rs", "policy"].iter().chain(args).copied());
        matches.subcommand_matches("policy").unwrap().clone()
    }

    #[test]
    fn init_then_validate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();
        let file = tmp.path().join("portfolio/policy.toml");
        let file = file.to_str().unwrap();

        handle_policy(&cli(&["init", "--dry-run", dir])).unwrap();
        assert!(!tmp.path().join("portfolio").exists());
        handle_policy(&cli(&["init", "--strategy", "custom", dir])).unwrap();
        // Custom has no tolerance band; the default strategy has them.
        handle_policy(&cli(&["validate", file])).unwrap();
        std::fs::remove_file(file).unwrap();
        handle_policy(&cli(&["init", dir])).unwrap();
        handle_policy(&cli(&["validate", file])).unwrap();
        assert_eq!(Policy::from_file(file).unwrap().name, "Balanced Growth");

        std::fs::write(file, "not = [toml").unwrap();
        let err = handle_policy(&cli(&["validate", file])).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("policy validation failed for {file}")
        );
    }

    #[test]
    fn init_never_overwrites_an_existing_policy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();
        std::fs::create_dir_all(tmp.path().join("portfolio")).unwrap();
        let path = tmp.path().join("portfolio/policy.toml");
        std::fs::write(&path, "mine").unwrap();
        for dry_run in [false, true] {
            let err = policy_init("custom", dir, dry_run).unwrap_err();
            assert!(
                err.to_string()
                    .starts_with("policy file already exists at "),
                "{err}"
            );
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_policy_file_is_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("portfolio")).unwrap();
        // A dangling symlink does not "exist", but cannot be created anew.
        let path = tmp.path().join("portfolio/policy.toml");
        std::os::unix::fs::symlink(tmp.path().join("nowhere/policy.toml"), &path).unwrap();
        let err = policy_init("custom", tmp.path().to_str().unwrap(), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to write policy file: {}", path.display())
        );
    }

    #[test]
    fn missing_subcommand_is_an_error() {
        let matches = Command::new("policy").get_matches_from(["policy"]);
        let err = handle_policy(&matches).unwrap_err();
        assert_eq!(err.to_string(), "unknown policy subcommand");
    }

    #[test]
    fn unknown_strategy_is_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = policy_init("yolo", tmp.path().to_str().unwrap(), false).unwrap_err();
        assert_eq!(err.to_string(), "unknown strategy: yolo");
        assert!(!tmp.path().join("portfolio").exists());
    }

    #[test]
    fn every_strategy_writes_a_valid_private_policy() {
        for (strategy, name) in [
            ("balanced-growth", "Balanced Growth"),
            ("capital-preservation", "Capital Preservation"),
            ("aggressive-growth", "Aggressive Growth"),
            ("custom", "Custom"),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            policy_init(strategy, tmp.path().to_str().unwrap(), false).unwrap();
            let path = tmp.path().join("portfolio/policy.toml");
            let policy = Policy::from_file(path.to_str().unwrap()).unwrap();
            assert_eq!(policy.name, name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{strategy}");
            }
        }
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        policy_init("custom", tmp.path().to_str().unwrap(), true).unwrap();
        assert!(!tmp.path().join("portfolio").exists());
    }

    #[test]
    fn unwritable_policy_dir_is_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        // `portfolio` is a file, so the policy directory cannot be created.
        fs::write(tmp.path().join("portfolio"), "").unwrap();
        let err = policy_init("custom", tmp.path().to_str().unwrap(), false).unwrap_err();
        assert!(
            err.to_string().starts_with("failed to create directory: "),
            "{err:#}"
        );
    }
}

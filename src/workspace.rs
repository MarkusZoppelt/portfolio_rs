use eyre::{Result, WrapErr};
use std::fs;
use std::path::Path;

// Sourced from `templates/` at the crate root (plain Markdown files, so
// they get real syntax highlighting/linting and can be edited without
// touching Rust raw-string escaping rules).
const INVESTMENT_POLICY_TEMPLATE: &str = include_str!("../templates/INVESTMENT_POLICY.md");
const AGENTS_MD_TEMPLATE: &str = include_str!("../templates/AGENTS.md");
const CLAUDE_MD_TEMPLATE: &str = include_str!("../templates/CLAUDE.md");

const WATCHLIST_TEMPLATE: &str = "[]\n";

const GITIGNORE_TEMPLATE: &str = r"# Private financial data — never commit these.
positions.json
/portfolio/diary/
/portfolio/decisions/
/portfolio/reports/
/portfolio/theses/
/portfolio/watchlist.json
";

pub fn init_workspace(dir: &str, dry_run: bool) -> Result<()> {
    let base = Path::new(dir);

    if !dry_run && !base.exists() {
        fs::create_dir_all(base)
            .wrap_err_with(|| format!("failed to create directory: {}", dir))?;
    }

    let dirs = [
        base.join("portfolio/diary"),
        base.join("portfolio/decisions"),
        base.join("portfolio/theses"),
        base.join("portfolio/reports"),
    ];

    let files = [
        (base.join("portfolio/watchlist.json"), WATCHLIST_TEMPLATE),
        (
            base.join("INVESTMENT_POLICY.md"),
            INVESTMENT_POLICY_TEMPLATE,
        ),
        (base.join(".gitignore"), GITIGNORE_TEMPLATE),
        (base.join("AGENTS.md"), AGENTS_MD_TEMPLATE),
        (base.join("CLAUDE.md"), CLAUDE_MD_TEMPLATE),
    ];

    if dry_run {
        println!("Dry-run: would create the following in {}", dir);
    } else {
        println!("Initializing finance workspace in {}", dir);
    }

    for d in &dirs {
        if dry_run {
            println!("  directory: {}", d.display());
        } else {
            fs::create_dir_all(d)
                .wrap_err_with(|| format!("failed to create directory: {}", d.display()))?;
            println!("  created directory: {}", d.display());
        }
    }

    for (path, content) in &files {
        if path.exists() {
            println!("  skip (exists): {}", path.display());
            continue;
        }
        if dry_run {
            println!("  file: {}", path.display());
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).wrap_err_with(|| {
                    format!("failed to create parent directory for: {}", path.display())
                })?;
            }
            fs::write(path, content)
                .wrap_err_with(|| format!("failed to write file: {}", path.display()))?;
            println!("  created file: {}", path.display());
        }
    }

    if dry_run {
        println!("\nNo files were created. Run without --dry-run to apply.");
    } else {
        println!("\nWorkspace initialized. Next steps:");
        println!(
            "  1. Edit {} to set your financial policy.",
            base.join("INVESTMENT_POLICY.md").display()
        );
        println!(
            "  2. Add diary entries in {}/",
            base.join("portfolio/diary").display()
        );
        println!(
            "  3. Add decisions in {}/",
            base.join("portfolio/decisions").display()
        );
    }

    Ok(())
}

pub fn init_agent_files(dir: &str, dry_run: bool) -> Result<()> {
    let base = Path::new(dir);

    if !base.exists() {
        return Err(eyre::eyre!("Directory does not exist: {}", dir));
    }

    let files = [
        (base.join("AGENTS.md"), AGENTS_MD_TEMPLATE),
        (base.join("CLAUDE.md"), CLAUDE_MD_TEMPLATE),
    ];

    if dry_run {
        println!("Dry-run: would create the following in {}", dir);
    } else {
        println!("Initializing agent instruction files in {}", dir);
    }

    for (path, content) in &files {
        if path.exists() {
            println!("  skip (exists): {}", path.display());
            continue;
        }
        if dry_run {
            println!("  file: {}", path.display());
        } else {
            fs::write(path, content)
                .wrap_err_with(|| format!("failed to write file: {}", path.display()))?;
            println!("  created file: {}", path.display());
        }
    }

    if dry_run {
        println!("\nNo files were created. Run without --dry-run to apply.");
    } else {
        println!("\nAgent instruction files initialized.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_init_workspace_creates_directories_and_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("portfolio");
        let dir_str = dir.to_str().unwrap();

        init_workspace(dir_str, false).unwrap();

        assert!(dir.join("portfolio/diary").is_dir());
        assert!(dir.join("portfolio/decisions").is_dir());
        assert!(dir.join("portfolio/theses").is_dir());
        assert!(dir.join("portfolio/reports").is_dir());
        assert!(dir.join("portfolio/watchlist.json").is_file());
        assert!(dir.join("INVESTMENT_POLICY.md").is_file());
    }

    #[test]
    fn test_init_workspace_skips_existing_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("portfolio");
        let dir_str = dir.to_str().unwrap();

        // Create existing files
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("INVESTMENT_POLICY.md"), "existing").unwrap();

        init_workspace(dir_str, false).unwrap();

        let content = fs::read_to_string(dir.join("INVESTMENT_POLICY.md")).unwrap();
        assert_eq!(content, "existing");
    }

    #[test]
    fn test_init_workspace_dry_run_does_not_create() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("portfolio");
        let dir_str = dir.to_str().unwrap();

        init_workspace(dir_str, true).unwrap();

        assert!(!dir.exists());
    }
}

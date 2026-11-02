// Sourced from `templates/skills/portfolio-rs/SKILL.md` at the crate root
// (plain Markdown, so it gets real syntax highlighting/linting and mirrors
// exactly the file `skill_export` writes to disk).
pub const SKILL_CONTENT: &str = include_str!("../templates/skills/portfolio-rs/SKILL.md");

use eyre::{Result, WrapErr};
use std::fs;
use std::path::Path;

pub fn skill_show() {
    println!("{SKILL_CONTENT}");
}

pub fn skill_export(dir: &str, dry_run: bool) -> Result<()> {
    let base = Path::new(dir);
    let skill_dir = base.join("portfolio-rs");
    let skill_file = skill_dir.join("SKILL.md");

    if dry_run {
        println!("Dry-run: would create the following:");
        println!("  directory: {}", skill_dir.display());
        if skill_file.exists() {
            println!("  skip (exists): {}", skill_file.display());
        } else {
            println!("  file: {}", skill_file.display());
        }
        println!("\nNo files were created. Run without --dry-run to apply.");
        return Ok(());
    }

    if !base.exists() {
        fs::create_dir_all(base).wrap_err_with(|| format!("failed to create directory: {dir}"))?;
    }

    fs::create_dir_all(&skill_dir)
        .wrap_err_with(|| format!("failed to create directory: {}", skill_dir.display()))?;

    if skill_file.exists() {
        println!("  skip (exists): {}", skill_file.display());
        println!("\nSkill already installed. Remove the existing file to re-export it.");
        return Ok(());
    }

    fs::write(&skill_file, SKILL_CONTENT)
        .wrap_err_with(|| format!("failed to write file: {}", skill_file.display()))?;
    println!("  created directory: {}", skill_dir.display());
    println!("  created file: {}", skill_file.display());
    println!("\nSkill exported to {}", skill_file.display());

    Ok(())
}

pub fn skill_path() {
    println!("Built-in skill: portfolio-rs");
    println!("  Use 'portfolio_rs agent skill show' to view the skill content.");
    println!("  Use 'portfolio_rs agent skill export <DIR>' to install it to an agent harness.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_export_writes_skill_once() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("skills");
        let file = base.join("portfolio-rs/SKILL.md");

        skill_export(base.to_str().unwrap(), true).unwrap();
        assert!(!base.exists(), "dry-run creates nothing");

        skill_export(base.to_str().unwrap(), false).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), SKILL_CONTENT);

        // An existing skill is kept, also by a dry-run.
        fs::write(&file, "mine").unwrap();
        skill_export(base.to_str().unwrap(), true).unwrap();
        skill_export(base.to_str().unwrap(), false).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn test_export_reports_an_unwritable_skill_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("portfolio-rs/SKILL.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        // A dangling symlink into a missing directory: not "existing", not writable.
        std::os::unix::fs::symlink(tmp.path().join("nowhere/SKILL.md"), &file).unwrap();
        let err = skill_export(tmp.path().to_str().unwrap(), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to write file: {}", file.display())
        );
    }

    #[test]
    fn test_export_reports_uncreatable_directories() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file");
        fs::write(&file, "").unwrap();
        let base = file.join("skills");
        let err = skill_export(base.to_str().unwrap(), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to create directory: {}", base.display())
        );

        fs::write(tmp.path().join("portfolio-rs"), "").unwrap();
        let err = skill_export(tmp.path().to_str().unwrap(), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "failed to create directory: {}",
                tmp.path().join("portfolio-rs").display()
            )
        );
    }
}

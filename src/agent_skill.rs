// Sourced from `templates/skills/portfolio-rs/SKILL.md` at the crate root
// (plain Markdown, so it gets real syntax highlighting/linting and mirrors
// exactly the file `skill_export` writes to disk).
pub const SKILL_CONTENT: &str = include_str!("../templates/skills/portfolio-rs/SKILL.md");

use eyre::{Result, WrapErr};
use std::fs;
use std::path::Path;

pub fn skill_show() {
    println!("{}", SKILL_CONTENT);
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
        fs::create_dir_all(base)
            .wrap_err_with(|| format!("failed to create directory: {}", dir))?;
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

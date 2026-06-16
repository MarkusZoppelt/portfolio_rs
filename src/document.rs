//! Shared helper for writing dated Markdown documents (decision records,
//! weekly reports) with a same-day overwrite guard.
//!
//! Both the CLI handlers (`commands::decision`, `commands::report`)
//! and [`crate::state::AppState`] (consumed by the HTTP API and external
//! GUIs) go through [`write_dated_document`] so the overwrite guard and
//! dry-run semantics can never drift between the two call sites.

use eyre::{bail, Result, WrapErr};
use std::path::Path;

/// Write `content` to `file_path`, refusing to clobber an existing file.
///
/// A dry-run only previews `content`; it never writes to disk and must
/// never trip the overwrite guard, so a subsequent real run against the
/// same file still succeeds. A real (non-dry-run) write refuses to
/// overwrite an existing file — `kind` (e.g. `"decision record"`) and
/// `hint` (e.g. `"Use a different title."`) build a helpful error message.
pub fn write_dated_document(
    file_path: &Path,
    content: &str,
    dry_run: bool,
    kind: &str,
    hint: &str,
) -> Result<()> {
    if dry_run {
        return Ok(());
    }

    if file_path.exists() {
        bail!(
            "{} already exists at {}. {}",
            kind,
            file_path.display(),
            hint
        );
    }

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create directory: {}", parent.display()))?;
    }
    std::fs::write(file_path, content)
        .wrap_err_with(|| format!("failed to write {}: {}", kind, file_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_dated_document_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sub/doc.md");

        write_dated_document(&path, "hello", false, "document", "hint").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn test_write_dated_document_refuses_overwrite() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.md");
        std::fs::write(&path, "existing").unwrap();

        let err = write_dated_document(&path, "new", false, "document", "Try again.").unwrap_err();
        assert!(err.to_string().contains("Try again."));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing");
    }

    #[test]
    fn test_write_dated_document_dry_run_never_writes_or_trips_guard() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.md");

        // Dry-run on a file that doesn't exist yet: must not write.
        write_dated_document(&path, "preview", true, "document", "hint").unwrap();
        assert!(!path.exists());

        // Dry-run on a file that already exists: must not error either.
        std::fs::write(&path, "existing").unwrap();
        write_dated_document(&path, "preview", true, "document", "hint").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing");

        // A subsequent real write against the same path must not be blocked
        // by the earlier dry-run.
        std::fs::remove_file(&path).unwrap();
        write_dated_document(&path, "real", false, "document", "hint").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real");
    }
}

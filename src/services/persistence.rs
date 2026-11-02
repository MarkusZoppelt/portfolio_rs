use crate::portfolio::Portfolio;
use eyre::{Result, WrapErr};
use std::path::Path;

/// The balance history, relative to the current directory.
const DB_PATH: &str = "database";

/// Open the balance history database, readable by the owner only.
pub fn open_db() -> Result<sled::Db> {
    open_private(Path::new(DB_PATH))
}

/// Open the sled database at `dir`. A new `dir` is created 0700 before sled
/// writes into it, so its files are never readable by others; an existing
/// one is tightened to 0700 (subdirectories) and 0600 (files) as well.
fn open_private(dir: &Path) -> Result<sled::Db> {
    create_private_dir(dir)?;
    let db = sled::open(dir).wrap_err("failed to open database")?;
    restrict_permissions(dir);
    Ok(db)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .wrap_err_with(|| format!("failed to create {}", dir.display()))
}

#[cfg(not(unix))]
fn create_private_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

/// Best effort: directories 0700, files 0600, below and including `dir`.
#[cfg(unix)]
fn restrict_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let set = |path: &Path, mode| {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    };
    set(dir, 0o700);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_dir() => restrict_permissions(&entry.path()),
            Ok(t) if t.is_file() => set(&entry.path(), 0o600),
            _ => {}
        }
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_dir: &Path) {}

pub fn store_balance_in_db(portfolio: &Portfolio) -> Result<()> {
    let db = open_db()?;
    let curr_value = portfolio.get_total_value();
    let curr_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let context = format!("failed to insert balance into database at {curr_time}");
    db.insert(&curr_time, curr_value.to_string().as_bytes())
        .wrap_err(context)?;

    // block until all operations are stable on disk
    db.flush().wrap_err("failed to flush database to disk")?;
    // The flush may have created segment files; the 0700 directory already
    // keeps them private, this keeps the modes consistent.
    restrict_permissions(Path::new(DB_PATH));

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn test_new_database_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("database");
        let db = open_private(&dir).unwrap();
        db.insert("k", "v").unwrap();
        db.flush().unwrap();
        drop(db);
        assert_eq!(mode(&dir), 0o700);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                assert_eq!(mode(&entry.path()), 0o600, "{:?}", entry.path());
            }
        }
    }

    #[test]
    fn test_existing_database_is_tightened() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("database");
        let nested = dir.join("blobs");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("1");
        std::fs::write(&file, b"x").unwrap();
        for (path, m) in [(&dir, 0o755), (&nested, 0o755), (&file, 0o644)] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(m)).unwrap();
        }
        restrict_permissions(&dir);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&nested), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn test_reopening_an_existing_database_keeps_its_data() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("database");
        let db = open_private(&dir).unwrap();
        db.insert("k", "v").unwrap();
        db.flush().unwrap();
        drop(db);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = open_private(&dir).unwrap();
        assert_eq!(db.get("k").unwrap().as_deref(), Some(&b"v"[..]));
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn test_restrict_permissions_skips_symlinks_and_missing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("database");
        std::fs::create_dir(&dir).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::write(&outside, b"x").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        restrict_permissions(&dir);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&outside), 0o644, "symlink targets are left alone");

        let missing = tmp.path().join("missing");
        restrict_permissions(&missing);
        assert!(!missing.exists());
    }

    #[test]
    fn test_database_below_a_file_cannot_be_created() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        let dir = file.join("database");
        let err = open_private(&dir).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to create {}", dir.display())
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
    }
}

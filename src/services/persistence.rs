use crate::portfolio::Portfolio;
use eyre::{Result, WrapErr};
use std::path::Path;

const DB_PATH: &str = "database";

/// Open the balance database, restricting it to the owner (unix: dir 0700,
/// files 0600; sled itself creates them with umask-default modes).
pub fn open_db() -> Result<sled::Db> {
    let db = sled::open(DB_PATH).wrap_err("failed to open database")?;
    restrict_permissions(Path::new(DB_PATH));
    Ok(db)
}

/// Best-effort: tighten `dir` to 0700 and its regular files to 0600.
#[cfg(unix)]
fn restrict_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let set = |path: &Path, mode| {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    };
    set(dir, 0o700);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_file()) {
                set(&entry.path(), 0o600);
            }
        }
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_dir: &Path) {}

pub fn store_balance_in_db(portfolio: &Portfolio) -> Result<()> {
    let db = open_db()?;
    let curr_value = portfolio.get_total_value();
    let curr_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert(&curr_time, curr_value.to_string().as_bytes())
        .wrap_err_with(|| format!("failed to insert balance into database at {curr_time}"))?;

    // block until all operations are stable on disk
    db.flush().wrap_err("failed to flush database to disk")?;
    // The flush may have created new segment/snapshot files.
    restrict_permissions(Path::new(DB_PATH));

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_restrict_permissions_sets_owner_only_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("database");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("snap.0000000000000010");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        restrict_permissions(&dir);

        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&file), 0o600);
    }
}

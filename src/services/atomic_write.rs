//! Atomic portfolio file replacement with a single backup.
//!
//! [`write_atomic_with_backup`] writes a uniquely named temp file in the same
//! directory as the target, fsyncs it, copies an existing target to
//! `{path}.bak` (overwriting the previous backup), and renames the temp file
//! over the target. Paths ending in `.gpg` are refused: decrypted portfolio
//! content must never be written to disk.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use eyre::{bail, eyre, Result, WrapErr};

/// Sequence number mixed into temp names so concurrent writers in this process
/// do not collide. Combined with the pid, names stay unique across processes.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomically replace `path` with `contents`, keeping a single `{path}.bak`.
///
/// The temp file is `.{filename}.tmp-{pid}-{counter}` in the same directory as
/// `path`. A new file is created owner-only (0600 on unix). If `path` already
/// exists, its permissions are applied to the new
/// file and its previous contents are copied to `{path}.bak` (overwriting any
/// earlier backup) before the rename. On error the temp file is removed.
///
/// Refuses paths ending in `.gpg`.
pub fn write_atomic_with_backup(path: &Path, contents: &[u8]) -> Result<()> {
    if is_gpg_path(path) {
        bail!(
            "refusing to overwrite encrypted portfolio file {}: \
             writing decrypted data to disk is not supported",
            path.display()
        );
    }

    let existing = match std::fs::metadata(path) {
        Ok(meta) => Some(meta),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err)
                .wrap_err_with(|| format!("failed to stat portfolio file: {}", path.display()));
        }
    };

    let (tmp_path, mut file) = create_temp_file(path)?;
    let guard = TempFileGuard::arm(tmp_path.clone());

    // Apply the original mode before writing, so a 0600 portfolio is never
    // briefly readable with umask-default permissions.
    if let Some(meta) = &existing {
        let context = format!("failed to preserve permissions for {}", path.display());
        file.set_permissions(meta.permissions()).wrap_err(context)?;
    }
    file.write_all(contents)
        .wrap_err(format!("failed to write temp file: {}", tmp_path.display()))?;
    let context = format!("failed to flush temp file to disk: {}", tmp_path.display());
    file.sync_all().wrap_err(context)?;
    // Close the fd before rename. Windows cannot rename an open file, and
    // dropping here also makes the fsync the last operation on this handle.
    drop(file);

    // Re-check at backup time so a file that appeared after the initial stat
    // is still copied, and a file that disappeared is not treated as an error.
    if path.exists() {
        copy_backup(path)?;
    }

    rename_into_place(&tmp_path, path, guard)
}

/// Atomically replace `path` with `contents` as an owner-only (0600) file,
/// without a backup. For derived files such as the quote cache.
pub fn write_atomic_private(path: &Path, contents: &[u8]) -> Result<()> {
    if is_gpg_path(path) {
        bail!("refusing to write {}", path.display());
    }
    let (tmp_path, mut file) = create_temp_file(path)?;
    let guard = TempFileGuard::arm(tmp_path.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let context = format!("failed to set permissions on {}", tmp_path.display());
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .wrap_err(context)?;
    }
    file.write_all(contents)
        .wrap_err(format!("failed to write temp file: {}", tmp_path.display()))?;
    let context = format!("failed to flush temp file to disk: {}", tmp_path.display());
    file.sync_all().wrap_err(context)?;
    drop(file);
    rename_into_place(&tmp_path, path, guard)
}

/// Rename the written temp file over `path`, keep it (disarm `guard`) and
/// sync the directory. On error `guard` removes the temp file.
fn rename_into_place(tmp_path: &Path, path: &Path, mut guard: TempFileGuard) -> Result<()> {
    std::fs::rename(tmp_path, path).wrap_err_with(|| {
        format!(
            "failed to replace {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    guard.disarm();
    sync_parent_dir(path);
    Ok(())
}

/// `OpenOptions` that create new files as owner-only (0600) on unix.
///
/// The mode only applies when a file is created; existing files keep theirs.
pub(crate) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Create a new file at `path` with owner-only (0600) permissions on unix.
///
/// Fails if `path` already exists, so existing files are never overwritten.
pub fn write_new_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = private_options()
        .write(true)
        .create_new(true)
        .open(path)
        .wrap_err_with(|| format!("failed to create file: {}", path.display()))?;
    file.write_all(contents)
        .wrap_err(format!("failed to write file: {}", path.display()))?;
    file.sync_all()
        .wrap_err(format!("failed to flush file to disk: {}", path.display()))?;
    Ok(())
}

fn is_gpg_path(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().ends_with(".gpg")
}

fn backup_path(path: &Path) -> PathBuf {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    PathBuf::from(bak)
}

fn temp_path_for(path: &Path, seq: u64) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| eyre!("portfolio path has no file name: {}", path.display()))?;
    let tmp_name = format!(
        ".{}.tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        seq
    );
    Ok(path.with_file_name(tmp_name))
}

/// Names tried before giving up on a unique temp file.
const TEMP_ATTEMPTS: u32 = 128;

fn create_temp_file(path: &Path) -> Result<(PathBuf, std::fs::File)> {
    create_temp_file_with(path, &mut || TEMP_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// [`create_temp_file`] with `next_seq` supplying the sequence numbers.
fn create_temp_file_with(
    path: &Path,
    next_seq: &mut dyn FnMut() -> u64,
) -> Result<(PathBuf, std::fs::File)> {
    if path.file_name().is_none() {
        bail!("portfolio path has no file name: {}", path.display());
    }

    let mut attempt = 1;
    loop {
        let tmp_path = temp_path_for(path, next_seq())?;
        match private_options()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
        {
            Ok(file) => return Ok((tmp_path, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists && attempt < TEMP_ATTEMPTS => {
                attempt += 1;
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                return Err(err).wrap_err_with(|| {
                    format!("failed to create a unique temp file for {}", path.display())
                });
            }
            Err(err) => {
                return Err(err).wrap_err_with(|| {
                    format!("failed to create temp file: {}", tmp_path.display())
                });
            }
        }
    }
}

fn copy_backup(path: &Path) -> Result<()> {
    let bak = backup_path(path);
    std::fs::copy(path, &bak)
        .wrap_err_with(|| format!("failed to back up {} to {}", path.display(), bak.display()))?;
    // Re-open read-only: `fs::copy` preserves mode, so a read-only source
    // would make a write-only reopen fail. `fsync` works on a read fd.
    let context = format!("failed to open backup for flush: {}", bak.display());
    let bak_file = std::fs::File::open(&bak).wrap_err(context)?;
    bak_file
        .sync_all()
        .wrap_err(format!("failed to flush backup to disk: {}", bak.display()))?;
    Ok(())
}

/// Best-effort: a failure here must not fail the write. The file rename has
/// already committed the new contents.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Ok(dir_file) = std::fs::File::open(parent_dir(path)) {
        let _ = dir_file.sync_all();
    }
}

/// The directory holding `path` (`.` for a bare file name).
#[cfg(unix)]
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

/// Removes the temp file unless [`TempFileGuard::disarm`] was called after a
/// successful rename.
struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    fn arm(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn list_dir(dir: &Path) -> Vec<String> {
        let mut names = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn assert_no_temp_files(names: &[String]) {
        for name in names {
            assert!(!name.contains(".tmp-"), "temp file left behind: {name}");
        }
    }

    #[test]
    fn test_write_new_file_has_no_backup() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");

        write_atomic_with_backup(&path, b"hello").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello");
        let names = list_dir(tmp.path());
        assert_eq!(names, vec!["positions.json".to_string()]);
        assert_no_temp_files(&names);
        assert!(!tmp.path().join("positions.json.bak").exists());
    }

    #[test]
    fn test_overwrite_keeps_exactly_one_backup_of_previous_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        write_atomic_with_backup(&path, b"v1").unwrap();

        write_atomic_with_backup(&path, b"v2").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"v2");
        assert_eq!(
            fs::read(tmp.path().join("positions.json.bak")).unwrap(),
            b"v1"
        );
        let names = list_dir(tmp.path());
        assert_eq!(
            names,
            vec![
                "positions.json".to_string(),
                "positions.json.bak".to_string()
            ]
        );
        assert_no_temp_files(&names);
    }

    #[test]
    fn test_second_overwrite_replaces_backup_with_previous_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        write_atomic_with_backup(&path, b"v1").unwrap();
        write_atomic_with_backup(&path, b"v2").unwrap();

        write_atomic_with_backup(&path, b"v3").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"v3");
        assert_eq!(
            fs::read(tmp.path().join("positions.json.bak")).unwrap(),
            b"v2"
        );
        let names = list_dir(tmp.path());
        let baks: Vec<_> = names.iter().filter(|name| name.ends_with(".bak")).collect();
        assert_eq!(baks, vec!["positions.json.bak"]);
        assert_no_temp_files(&names);
    }

    #[test]
    fn test_gpg_path_is_refused_and_creates_nothing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json.gpg");

        let err = write_atomic_with_backup(&path, b"plaintext").unwrap_err();
        assert!(
            err.to_string().contains("encrypted"),
            "unexpected error: {err}"
        );
        assert!(!path.exists());
        assert!(list_dir(tmp.path()).is_empty());

        // An existing ciphertext file must not be overwritten or backed up.
        fs::write(&path, b"ciphertext").unwrap();
        let err = write_atomic_with_backup(&path, b"plaintext").unwrap_err();
        assert!(err.to_string().contains("encrypted"));
        assert_eq!(fs::read(&path).unwrap(), b"ciphertext");
        assert_eq!(list_dir(tmp.path()), vec!["positions.json.gpg".to_string()]);
    }

    #[test]
    fn test_error_removes_temp_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        fs::write(&path, b"keep").unwrap();
        // A directory at the backup path makes the copy fail after the temp
        // file has already been written.
        fs::create_dir(tmp.path().join("positions.json.bak")).unwrap();

        assert!(write_atomic_with_backup(&path, b"new").is_err());

        assert_eq!(fs::read(&path).unwrap(), b"keep");
        assert_no_temp_files(&list_dir(tmp.path()));
    }

    #[cfg(unix)]
    #[test]
    fn test_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        fs::write(&path, b"v1").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o640);
        fs::set_permissions(&path, perms).unwrap();

        write_atomic_with_backup(&path, b"v2").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(fs::read(&path).unwrap(), b"v2");
    }

    #[test]
    fn test_temp_name_is_in_same_directory() {
        let path = Path::new("/var/tmp/positions.json");
        let tmp = temp_path_for(path, 7).unwrap();
        assert_eq!(tmp.parent(), Some(Path::new("/var/tmp")));
        let name = tmp.file_name().unwrap().to_string_lossy();
        let expected_prefix = format!(".positions.json.tmp-{}-", std::process::id());
        assert!(name.starts_with(&expected_prefix), "{}", name);
        assert!(name.ends_with("-7"), "{}", name);
    }

    #[cfg(unix)]
    #[test]
    fn test_new_files_are_owner_only_and_existing_modes_kept() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("positions.json");
        write_atomic_with_backup(&path, b"[]").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic_with_backup(&path, b"[1]").unwrap();
        assert_eq!(mode(&path), 0o640);

        let doc = tmp.path().join("doc.md");
        write_new_private(&doc, b"x").unwrap();
        assert_eq!(mode(&doc), 0o600);
        assert!(write_new_private(&doc, b"y").is_err());
        assert_eq!(fs::read(&doc).unwrap(), b"x");
    }

    #[test]
    fn test_stat_failure_is_reported() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file");
        fs::write(&file, b"x").unwrap();
        // A regular file as a parent directory: stat fails with ENOTDIR.
        let err = write_atomic_with_backup(&file.join("positions.json"), b"v").unwrap_err();
        assert!(
            format!("{err:#}").contains("failed to stat portfolio file"),
            "{err:#}"
        );
        assert_eq!(list_dir(tmp.path()), vec!["file".to_string()]);
    }

    #[test]
    fn test_missing_directory_fails_to_create_temp_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing").join("positions.json");
        let err = write_atomic_with_backup(&path, b"v").unwrap_err();
        assert!(
            format!("{err:#}").contains("failed to create temp file"),
            "{err:#}"
        );
        assert!(write_atomic_private(&path, b"v").is_err());
        assert!(list_dir(tmp.path()).is_empty());
    }

    #[test]
    fn test_path_without_file_name_is_refused() {
        let tmp = TempDir::new().unwrap();
        let err = write_atomic_with_backup(&tmp.path().join(".."), b"v").unwrap_err();
        assert!(err.to_string().contains("has no file name"), "{err}");
        assert!(temp_path_for(Path::new("/"), 0).is_err());
        assert!(list_dir(tmp.path()).is_empty());
    }

    #[test]
    fn test_temp_name_collisions_retry_then_give_up() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        let taken = temp_path_for(&path, 0).unwrap();
        fs::write(&taken, b"other writer").unwrap();

        // Seq 0 is taken, seq 1 is free.
        let mut seqs = [0, 1].into_iter();
        let (tmp_path, _file) = create_temp_file_with(&path, &mut || seqs.next().unwrap()).unwrap();
        assert_eq!(tmp_path, temp_path_for(&path, 1).unwrap());

        // Every attempt collides: give up after TEMP_ATTEMPTS tries.
        let mut tries = 0;
        let err = create_temp_file_with(&path, &mut || {
            tries += 1;
            0
        })
        .unwrap_err();
        assert_eq!(tries, TEMP_ATTEMPTS);
        assert!(
            format!("{err:#}").contains("failed to create a unique temp file"),
            "{err:#}"
        );
        assert_eq!(fs::read(&taken).unwrap(), b"other writer");
    }

    #[test]
    fn test_private_write_refuses_gpg_and_replaces_files() {
        let tmp = TempDir::new().unwrap();
        let gpg = tmp.path().join("cache.json.gpg");
        let err = write_atomic_private(&gpg, b"x").unwrap_err();
        assert!(err.to_string().contains("refusing to write"), "{err}");
        assert!(!gpg.exists());

        let path = tmp.path().join("cache.json");
        write_atomic_private(&path, b"v1").unwrap();
        write_atomic_private(&path, b"v2").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"v2");
        assert_eq!(list_dir(tmp.path()), vec!["cache.json".to_string()]);
    }

    #[test]
    fn test_failed_rename_reports_and_removes_temp_file() {
        let tmp = TempDir::new().unwrap();
        // A non-empty directory cannot be replaced by a file.
        let path = tmp.path().join("cache.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"x").unwrap();
        let err = write_atomic_private(&path, b"v").unwrap_err();
        assert!(format!("{err:#}").contains("failed to replace"), "{err:#}");
        assert!(path.join("keep").exists());
        assert_no_temp_files(&list_dir(tmp.path()));
    }

    #[cfg(unix)]
    #[test]
    fn test_parent_dir_of_bare_and_nested_paths() {
        assert_eq!(parent_dir(Path::new("positions.json")), Path::new("."));
        assert_eq!(parent_dir(Path::new("/")), Path::new("."));
        assert_eq!(
            parent_dir(Path::new("/var/tmp/positions.json")),
            Path::new("/var/tmp")
        );
    }
}

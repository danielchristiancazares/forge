//! Atomic file write helpers.
//!
//! Uses a temp file + rename pattern. On Windows, rename-over-existing fails, so we
//! use a backup-and-restore fallback to avoid data loss when overwriting.

use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;
use tracing::debug;

#[derive(Debug, Clone, Copy)]
pub struct AtomicWriteOptions {
    /// When true, `sync_all()` is called on the temp file before persisting.
    pub sync_all: bool,
    /// When true, best-effort `sync_all()` is called on the parent directory after
    /// the file has been persisted (renamed).
    ///
    /// This reduces the window where a power loss could lose the directory entry
    /// even though the rename has logically succeeded.
    ///
    /// Best-effort: errors are logged but do not fail the write.
    pub dir_sync: bool,
    /// When set (Unix only), apply this mode to the temp file before writing and to
    /// the final file after persist (e.g. `0o600`).
    pub unix_mode: Option<u32>,
}

impl Default for AtomicWriteOptions {
    fn default() -> Self {
        Self {
            sync_all: true,
            dir_sync: false,
            unix_mode: None,
        }
    }
}

pub fn atomic_write(path: impl AsRef<Path>, bytes: &[u8]) -> std::io::Result<()> {
    atomic_write_with_options(path, bytes, AtomicWriteOptions::default())
}

pub fn atomic_write_new_with_options(
    path: impl AsRef<Path>,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> std::io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    let mut tmp = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    if let Some(mode) = options.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))?;
    }

    tmp.write_all(bytes)?;
    if options.sync_all {
        tmp.as_file().sync_all()?;
    }

    // Persist (rename) but fail if the destination already exists.
    if let Err(err) = tmp.persist_noclobber(path) {
        return Err(err.error);
    }

    #[cfg(unix)]
    if let Some(mode) = options.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }

    if options.dir_sync {
        best_effort_sync_parent_dir(parent);
    }

    Ok(())
}

fn best_effort_sync_parent_dir(parent: &Path) {
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    #[cfg(unix)]
    {
        if let Err(e) = std::fs::File::open(parent).and_then(|d| d.sync_all()) {
            debug!(path = %parent.display(), "Parent directory sync_all failed (best-effort): {e}");
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // From winbase.h. Required to open a directory handle on Windows.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let mut opts = std::fs::OpenOptions::new();
        opts.read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

        if let Err(e) = opts.open(parent).and_then(|d| d.sync_all()) {
            debug!(path = %parent.display(), "Parent directory sync_all failed (best-effort): {e}");
        }
    }
}

pub fn atomic_write_with_options(
    path: impl AsRef<Path>,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> std::io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    let mut tmp = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    if let Some(mode) = options.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))?;
    }

    tmp.write_all(bytes)?;
    if options.sync_all {
        tmp.as_file().sync_all()?;
    }

    // Persist (rename) - handle Windows where rename fails if target exists.
    if let Err(err) = tmp.persist(path) {
        if path.exists() {
            // Windows fallback: backup and restore.
            let backup_path = path.with_extension("bak");
            let _ = std::fs::remove_file(&backup_path);
            std::fs::rename(path, &backup_path)?;

            if let Err(rename_err) = err.file.persist(path) {
                let _ = std::fs::rename(&backup_path, path);
                return Err(rename_err.error);
            }
            if let Err(e) = std::fs::remove_file(&backup_path) {
                tracing::warn!(
                    path = %backup_path.display(),
                    "Failed to remove .bak after atomic write: {e}"
                );
            }
        } else {
            return Err(err.error);
        }
    }

    #[cfg(unix)]
    if let Some(mode) = options.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }

    if options.dir_sync {
        best_effort_sync_parent_dir(parent);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_overwrites_existing_and_cleans_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.txt");
        let opts = AtomicWriteOptions {
            sync_all: false,
            dir_sync: false,
            unix_mode: None,
        };

        atomic_write_with_options(&path, b"one", opts).expect("write one");
        atomic_write_with_options(&path, b"two", opts).expect("write two");

        let content = std::fs::read_to_string(&path).expect("read");
        assert_eq!(content, "two");
        assert!(!path.with_extension("bak").exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_unix_permissions_when_configured() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secure.txt");
        let opts = AtomicWriteOptions {
            sync_all: false,
            dir_sync: false,
            unix_mode: Some(0o600),
        };

        atomic_write_with_options(&path, b"secret", opts).expect("write");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

//! Atomic file write helpers.
//!
//! Uses a temp file + rename pattern. On Windows, rename-over-existing fails, so we
//! use a backup-and-restore fallback to avoid data loss when overwriting.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersistMode {
    /// Allow the file to inherit the default umask.
    #[default]
    Default,
    /// Strictly enforce owner-only read/write permissions (0o600 on Unix).
    SensitiveOwnerOnly,
    /// Preserve an existing Unix mode from a previously-materialized file.
    ///
    /// Ignored on non-Unix platforms.
    Preserve(u32),
}

impl PersistMode {
    #[cfg(unix)]
    pub fn mode(self) -> Option<u32> {
        match self {
            Self::Default => None,
            Self::SensitiveOwnerOnly => Some(0o600),
            Self::Preserve(mode) => Some(mode),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AtomicWriteOptions {
    /// File sync policy for the temp file before persisting.
    pub file_sync: FileSyncPolicy,
    /// Parent directory sync policy after the file has been persisted.
    pub parent_dir_sync: ParentDirSyncPolicy,
    /// Determine the permission policy for the created file.
    pub mode: PersistMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSyncPolicy {
    SyncAll,
    SkipSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentDirSyncPolicy {
    SyncBestEffort,
    SkipSync,
}

impl Default for AtomicWriteOptions {
    fn default() -> Self {
        Self {
            file_sync: FileSyncPolicy::SyncAll,
            parent_dir_sync: ParentDirSyncPolicy::SkipSync,
            // Backwards compatibility: the default option struct previously enforced 0o600.
            mode: PersistMode::SensitiveOwnerOnly,
        }
    }
}

/// Recover from incomplete atomic writes by restoring `.bak` files.
///
/// If `path` does not exist but `path.bak` does, it means a crash occurred
/// during the backup-rename window in [`atomic_write_with_options`]. Rename
/// the backup back to the canonical path so the caller can proceed.
pub fn recover_bak_file(path: &Path) {
    let backup = path.with_extension("bak");
    if !path.exists() && backup.exists() {
        match fs::rename(&backup, path) {
            Ok(()) => {
                tracing::warn!(
                    path = %path.display(),
                    "Recovered .bak file from interrupted atomic write"
                );
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "Failed to recover .bak file: {e}"
                );
            }
        }
    }
}

pub fn atomic_write(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with_options(path, bytes, AtomicWriteOptions::default())
}

pub fn atomic_write_new_with_options(
    path: impl AsRef<Path>,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    let mut tmp = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    if let Some(mode) = options.mode.mode() {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), Permissions::from_mode(mode))?;
    }

    tmp.write_all(bytes)?;
    if matches!(options.file_sync, FileSyncPolicy::SyncAll) {
        tmp.as_file().sync_all()?;
    }

    // Persist (rename) but fail if the destination already exists.
    if let Err(err) = tmp.persist_noclobber(path) {
        return Err(err.error);
    }

    #[cfg(unix)]
    if let Some(mode) = options.mode.mode() {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, Permissions::from_mode(mode))?;
    }
    apply_windows_owner_only_acl(path, options.mode);

    if matches!(options.parent_dir_sync, ParentDirSyncPolicy::SyncBestEffort) {
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
        if let Err(e) = File::open(parent).and_then(|d| d.sync_all()) {
            debug!(path = %parent.display(), "Parent directory sync_all failed (best-effort): {e}");
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // From winbase.h. Required to open a directory handle on Windows.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let mut opts = OpenOptions::new();
        opts.read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

        if let Err(e) = opts.open(parent).and_then(|d| d.sync_all()) {
            debug!(path = %parent.display(), "Parent directory sync_all failed (best-effort): {e}");
        }
    }
}

#[cfg(windows)]
fn apply_windows_owner_only_acl(path: &Path, mode: PersistMode) {
    if matches!(mode, PersistMode::SensitiveOwnerOnly)
        && let Err(e) = crate::set_owner_only_file_acl(path)
    {
        tracing::warn!(
            path = %path.display(),
            "Failed to apply owner-only ACL to file (best-effort): {e}"
        );
    }
}

#[cfg(not(windows))]
fn apply_windows_owner_only_acl(_path: &Path, _mode: PersistMode) {}

pub fn atomic_write_with_options(
    path: impl AsRef<Path>,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    let mut tmp = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    if let Some(mode) = options.mode.mode() {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), Permissions::from_mode(mode))?;
    }

    tmp.write_all(bytes)?;
    if matches!(options.file_sync, FileSyncPolicy::SyncAll) {
        tmp.as_file().sync_all()?;
    }

    // Persist (rename) - handle Windows where rename fails if target exists.
    if let Err(err) = tmp.persist(path) {
        if path.exists() {
            // Windows fallback: backup and restore.
            let backup_path = path.with_extension("bak");
            let _ = fs::remove_file(&backup_path);
            fs::rename(path, &backup_path)?;

            if let Err(rename_err) = err.file.persist(path) {
                let _ = fs::rename(&backup_path, path);
                return Err(rename_err.error);
            }
            if let Err(e) = fs::remove_file(&backup_path) {
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
    if let Some(mode) = options.mode.mode() {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, Permissions::from_mode(mode))?;
    }
    apply_windows_owner_only_acl(path, options.mode);

    if matches!(options.parent_dir_sync, ParentDirSyncPolicy::SyncBestEffort) {
        best_effort_sync_parent_dir(parent);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{AtomicWriteOptions, atomic_write_with_options};

    #[test]
    fn atomic_write_overwrites_existing_and_cleans_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.txt");
        let opts = AtomicWriteOptions {
            file_sync: super::FileSyncPolicy::SkipSync,
            parent_dir_sync: super::ParentDirSyncPolicy::SkipSync,
            mode: super::PersistMode::Default,
        };

        atomic_write_with_options(&path, b"one", opts).expect("write one");
        atomic_write_with_options(&path, b"two", opts).expect("write two");

        let content = fs::read_to_string(&path).expect("read");
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
            file_sync: super::FileSyncPolicy::SkipSync,
            parent_dir_sync: super::ParentDirSyncPolicy::SkipSync,
            mode: super::PersistMode::SensitiveOwnerOnly,
        };

        atomic_write_with_options(&path, b"secret", opts).expect("write");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) fn ensure_secure_dir(path: &Path) -> Result<()> {
    let already_exists = path.is_dir();
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))?;
    // Only apply restrictive permissions to directories we just created.
    // Re-ACL-ing a pre-existing directory (e.g. the system temp dir) can
    // trigger recursive ACL propagation to all its children, which is both
    // slow and destructive.
    if already_exists {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = metadata(path)
            .with_context(|| format!("Failed to read directory metadata: {}", path.display()))?;

        let our_uid = unsafe { libc::getuid() };
        if metadata.uid() != our_uid {
            return Ok(());
        }

        let current_mode = metadata.permissions().mode() & 0o777;
        if current_mode & 0o077 != 0 {
            set_permissions(path, Permissions::from_mode(0o700)).with_context(|| {
                format!("Failed to set directory permissions: {}", path.display())
            })?;
        }
    }
    #[cfg(windows)]
    if let Err(e) = forge_utils::set_owner_only_dir_acl(path) {
        tracing::warn!(
            path = %path.display(),
            "Failed to apply owner-only ACL to directory (best-effort): {e}"
        );
    }
    Ok(())
}

pub(crate) fn ensure_secure_db_files(path: &Path) -> Result<()> {
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            let _file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("Failed to create database file: {}", path.display()))?;
        }
        #[cfg(not(unix))]
        {
            let _file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("Failed to create database file: {}", path.display()))?;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        set_permissions(path, Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set database permissions: {}", path.display()))?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = sqlite_sidecar_path(path, suffix);
            if sidecar.exists() {
                let _ = set_permissions(&sidecar, Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = forge_utils::set_owner_only_file_acl(path) {
            tracing::warn!(
                path = %path.display(),
                "Failed to apply owner-only ACL to database file (best-effort): {e}"
            );
        }
        for suffix in ["-wal", "-shm"] {
            let sidecar = sqlite_sidecar_path(path, suffix);
            if sidecar.exists()
                && let Err(e) = forge_utils::set_owner_only_file_acl(&sidecar)
            {
                tracing::warn!(
                    path = %sidecar.display(),
                    "Failed to apply owner-only ACL to sidecar file (best-effort): {e}"
                );
            }
        }
    }

    Ok(())
}

pub(crate) fn prepare_db_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_secure_dir(parent)?;
    }
    ensure_secure_db_files(path)
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path.file_name().map(|name| name.to_string_lossy());
    match file_name {
        Some(name) => path.with_file_name(format!("{name}{suffix}")),
        None => PathBuf::from(format!("{}{suffix}", path.display())),
    }
}

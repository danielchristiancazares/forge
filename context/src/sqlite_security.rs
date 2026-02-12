use std::fs::OpenOptions;
use std::path::Path;

use anyhow::{Context, Result};

pub(crate) fn ensure_secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Failed to read directory metadata: {}", path.display()))?;

        let our_uid = unsafe { libc::getuid() };
        if metadata.uid() != our_uid {
            return Ok(());
        }

        let current_mode = metadata.permissions().mode() & 0o777;
        if current_mode & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).with_context(
                || format!("Failed to set directory permissions: {}", path.display()),
            )?;
        }
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

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set database permissions: {}", path.display()))?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = sqlite_sidecar_path(path, suffix);
            if sidecar.exists() {
                let _ = std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600));
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

#[cfg(unix)]
fn sqlite_sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let file_name = path.file_name().map(|name| name.to_string_lossy());
    match file_name {
        Some(name) => path.with_file_name(format!("{name}{suffix}")),
        None => std::path::PathBuf::from(format!("{}{suffix}", path.display())),
    }
}

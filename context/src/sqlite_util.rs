//! Shared SQLite and secure-filesystem helpers for context persistence.
//!
//! This module is the single authority (IFA §7) for:
//! - Secure directory creation and Unix permission tightening
//! - Secure SQLite database file creation with permission hardening
//! - SQLite WAL/SHM sidecar path computation
//! - ISO 8601 timestamp formatting ("chrono-lite")
//! - The common `open()` preamble shared by all journal/store modules

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs::OpenOptions;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Open a SQLite database at `path` with secure directory and file permissions.
///
/// Performs the shared preamble used by all context-crate databases:
/// 1. Creates the parent directory if it doesn't exist
/// 2. Tightens directory permissions (Unix: 0o700, owner-only)
/// 3. Creates the DB file with secure permissions (Unix: 0o600)
/// 4. Opens the SQLite connection
pub(crate) fn open_secure_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    if let Some(parent) = path.parent() {
        ensure_secure_dir(parent)?;
    }
    ensure_secure_db_files(path)?;

    Connection::open(path).with_context(|| format!("Failed to open database at {}", path.display()))
}

/// Ensure a directory exists with secure permissions.
///
/// Creates the directory (and parents) if missing, then on Unix tightens
/// permissions to 0o700 if the directory is owned by the current user.
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

/// Ensure a SQLite database file (and its WAL/SHM sidecars) has secure permissions.
///
/// If the file doesn't exist, it is created atomically with 0o600 on Unix.
/// Pre-existing files and sidecars are permission-tightened unconditionally.
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

/// Compute the path to a SQLite sidecar file (e.g. `-wal`, `-shm`).
#[cfg(unix)]
fn sqlite_sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let file_name = path.file_name().map(|name| name.to_string_lossy());
    match file_name {
        Some(name) => path.with_file_name(format!("{name}{suffix}")),
        None => std::path::PathBuf::from(format!("{}{suffix}", path.display())),
    }
}

// ── Chrono-lite: minimal ISO 8601 formatting ────────────────────────────

/// Convert a `SystemTime` to ISO 8601 with millisecond precision.
///
/// Format: `YYYY-MM-DDTHH:MM:SS.mmmZ`
pub(crate) fn system_time_to_iso8601(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    chrono_lite_format(secs, Some(millis))
}

/// Convert a `SystemTime` to ISO 8601 with second precision (no millis).
///
/// Format: `YYYY-MM-DDTHH:MM:SSZ`
pub(crate) fn system_time_to_iso8601_seconds(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();

    chrono_lite_format(secs, None)
}

/// Core ISO 8601 formatter. When `millis` is `Some`, appends `.mmmZ`; otherwise `Z`.
fn chrono_lite_format(secs: u64, millis: Option<u32>) -> String {
    const SECS_PER_DAY: u64 = 86400;
    const SECS_PER_HOUR: u64 = 3600;
    const SECS_PER_MINUTE: u64 = 60;

    let days = secs / SECS_PER_DAY;
    let remaining = secs % SECS_PER_DAY;

    let hours = remaining / SECS_PER_HOUR;
    let remaining = remaining % SECS_PER_HOUR;

    let minutes = remaining / SECS_PER_MINUTE;
    let seconds = remaining % SECS_PER_MINUTE;

    let (year, month, day) = days_to_ymd(days);

    match millis {
        Some(ms) => {
            format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{ms:03}Z")
        }
        None => {
            format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
        }
    }
}

/// Convert days since Unix epoch to (year, month, day).
///
/// Uses Howard Hinnant's civil_from_days algorithm (O(1), correct for all dates).
fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    (year as i32, m, d)
}

/// Parse ISO 8601 string to `SystemTime`.
///
/// Accepts both `YYYY-MM-DDTHH:MM:SS.mmmZ` and `YYYY-MM-DDTHH:MM:SSZ` formats.
#[allow(dead_code)]
pub(crate) fn iso8601_to_system_time(s: &str) -> Option<SystemTime> {
    if s.len() < 19 {
        return None;
    }

    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let minute: u32 = s.get(14..16)?.parse().ok()?;
    let second: u32 = s.get(17..19)?.parse().ok()?;

    let millis: u32 = if s.len() >= 23 && s.get(19..20) == Some(".") {
        s.get(20..23)?.parse().ok()?
    } else {
        0
    };

    let days = ymd_to_days(year, month, day)?;
    let secs =
        days as u64 * 86400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second);
    let duration = Duration::from_secs(secs) + Duration::from_millis(u64::from(millis));

    UNIX_EPOCH.checked_add(duration)
}

/// Convert (year, month, day) to days since Unix epoch.
fn ymd_to_days(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m = month;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    Some(era * 146_097 + i64::from(doe) - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_millis_roundtrip() {
        let original = SystemTime::now();
        let iso = system_time_to_iso8601(original);
        let parsed = iso8601_to_system_time(&iso).unwrap();

        let diff = if original > parsed {
            original.duration_since(parsed).unwrap()
        } else {
            parsed.duration_since(original).unwrap()
        };
        assert!(diff.as_millis() < 1000);
    }

    #[test]
    fn iso8601_seconds_format() {
        let time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let iso = system_time_to_iso8601_seconds(time);
        assert!(!iso.contains('.'), "seconds format must not contain millis");
        assert!(iso.ends_with('Z'));
    }

    #[test]
    fn iso8601_seconds_parseable() {
        let time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let iso = system_time_to_iso8601_seconds(time);
        let parsed = iso8601_to_system_time(&iso).unwrap();
        let diff = time.duration_since(parsed).unwrap_or_default();
        assert!(diff.as_secs() == 0);
    }

    #[test]
    fn known_date() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }
}

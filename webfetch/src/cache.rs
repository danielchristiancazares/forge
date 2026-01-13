//! LRU disk cache with TTL for `WebFetch`.
//!
//! This module implements caching per FR-WF-16 through FR-WF-17:
//! - SHA256-based cache keys (URL + rendering method)
//! - Path layout: `{cache_dir}/{first2}/{keyhex}.json`
//! - Versioned entry format (v2)
//! - LRU eviction with dual limits (count + bytes)
//! - TTL-based expiration
//! - Atomic writes (temp + rename)

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

use crate::resolved::CacheSettings;
use crate::types::{ErrorCode, WebFetchError};

/// Current cache entry format version.
pub const CACHE_VERSION: u32 = 2;

/// Rendering method for cache key derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderingMethod {
    /// Standard HTTP fetch.
    Http,
    /// Headless browser rendering.
    Browser,
}

impl RenderingMethod {
    fn as_str(&self) -> &'static str {
        match self {
            RenderingMethod::Http => "http",
            RenderingMethod::Browser => "browser",
        }
    }
}

/// Cache entry stored on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    /// Format version for compatibility.
    pub version: u32,

    /// Original fetch timestamp (RFC3339, second precision).
    pub fetched_at: String,

    /// Expiration timestamp (RFC3339, second precision).
    pub expires_at: String,

    /// Last access timestamp (RFC3339, second precision).
    /// Updated on read; NOT used for TTL sliding.
    pub last_accessed_at: String,

    /// Final URL after redirects.
    pub final_url: String,

    /// Page title.
    pub title: Option<String>,

    /// Page language.
    pub language: Option<String>,

    /// Canonical extracted document (Markdown).
    /// NOT chunked - caller re-chunks with request's `max_chunk_tokens`.
    pub markdown: String,
}

impl CacheEntry {
    /// Create a new cache entry.
    pub fn new(
        final_url: String,
        title: Option<String>,
        language: Option<String>,
        markdown: String,
        ttl: Duration,
    ) -> Self {
        let now = SystemTime::now();
        let expires = now + ttl;

        Self {
            version: CACHE_VERSION,
            fetched_at: format_rfc3339(now),
            expires_at: format_rfc3339(expires),
            last_accessed_at: format_rfc3339(now),
            final_url,
            title,
            language,
            markdown,
        }
    }

    /// Check if entry is expired.
    pub fn is_expired(&self) -> bool {
        parse_rfc3339(&self.expires_at).is_none_or(|exp| SystemTime::now() > exp)
    }

    /// Update last accessed time (does NOT slide TTL).
    pub fn touch(&mut self) {
        self.last_accessed_at = format_rfc3339(SystemTime::now());
    }

    /// Estimate serialized size in bytes.
    pub fn estimated_size(&self) -> u64 {
        // Rough estimate: JSON overhead + string lengths
        let base_overhead = 200; // JSON structure, field names
        let content_size = self.fetched_at.len()
            + self.expires_at.len()
            + self.last_accessed_at.len()
            + self.final_url.len()
            + self.title.as_ref().map_or(0, std::string::String::len)
            + self.language.as_ref().map_or(0, std::string::String::len)
            + self.markdown.len();

        (base_overhead + content_size) as u64
    }
}

/// Cache operation result.
#[derive(Debug)]
pub enum CacheResult {
    /// Cache hit with entry.
    Hit(CacheEntry),
    /// Cache miss (not found, expired, or corrupted).
    Miss,
    /// Version mismatch (entry deleted).
    VersionMismatch,
}

/// Disk-based LRU cache.
pub struct Cache {
    /// Cache directory path.
    dir: PathBuf,

    /// Maximum entries.
    max_entries: u32,

    /// Maximum total bytes.
    max_bytes: u64,

    /// TTL duration.
    ttl: Duration,

    /// In-memory LRU tracking (key → (`last_access`, size)).
    lru: HashMap<String, (SystemTime, u64)>,
}

impl Cache {
    /// Create a new cache instance.
    pub fn new(settings: &CacheSettings) -> Result<Self, WebFetchError> {
        let dir = settings.dir.clone();

        // Ensure cache directory exists
        fs::create_dir_all(&dir).map_err(|e| {
            WebFetchError::new(
                ErrorCode::Internal,
                format!("failed to create cache directory: {e}"),
                false,
            )
        })?;

        let mut cache = Self {
            dir,
            max_entries: settings.max_entries,
            max_bytes: settings.max_bytes,
            ttl: settings.ttl,
            lru: HashMap::new(),
        };

        // Initialize LRU from existing files
        cache.scan_entries();

        Ok(cache)
    }

    /// Get a cached entry if valid.
    ///
    /// FR-WF-CCH-READ-01: Cache read failures are treated as cache miss.
    /// Updates `last_accessed_at` on hit (but does NOT slide TTL).
    pub fn get(&mut self, url: &Url, method: RenderingMethod) -> CacheResult {
        let key = cache_key(url, method);
        let path = self.entry_path(&key);

        // Check if file exists
        if !path.exists() {
            return CacheResult::Miss;
        }

        // Read and parse entry
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                // FR-WF-CCH-READ-01: Treat as cache miss
                return CacheResult::Miss;
            }
        };

        let mut entry: CacheEntry = if let Ok(e) = serde_json::from_str(&content) {
            e
        } else {
            // Corrupted entry - delete and miss
            let _ = fs::remove_file(&path);
            self.lru.remove(&key);
            return CacheResult::Miss;
        };

        // FR-WF-CCH-VER-01: Version mismatch
        if entry.version != CACHE_VERSION {
            let _ = fs::remove_file(&path);
            self.lru.remove(&key);
            return CacheResult::VersionMismatch;
        }

        // Check expiration
        if entry.is_expired() {
            let _ = fs::remove_file(&path);
            self.lru.remove(&key);
            return CacheResult::Miss;
        }

        // Update last accessed time
        entry.touch();

        // Write back with updated timestamp (non-fatal if fails)
        if let Ok(updated_content) = serde_json::to_string_pretty(&entry) {
            let _ = fs::write(&path, &updated_content);
        }

        // Update LRU tracking
        let size = entry.estimated_size();
        self.lru.insert(key, (SystemTime::now(), size));

        CacheResult::Hit(entry)
    }

    /// Store an entry in the cache.
    ///
    /// Uses atomic write (temp file + rename) per FR-WF-16g.
    /// FR-WF-16h: Returns error for oversized entries (caller adds note).
    pub fn put(
        &mut self,
        url: &Url,
        method: RenderingMethod,
        entry: &CacheEntry,
    ) -> Result<(), CacheWriteError> {
        let size = entry.estimated_size();

        // Check if entry is too large
        if size > self.max_bytes {
            return Err(CacheWriteError::EntryTooLarge {
                size,
                max: self.max_bytes,
            });
        }

        let key = cache_key(url, method);

        // Evict if necessary per FR-WF-CCH-EVICT-01
        self.evict_if_needed(size)?;

        // Serialize entry
        let content = serde_json::to_string_pretty(entry)
            .map_err(|e| CacheWriteError::SerializationFailed(e.to_string()))?;

        // Ensure subdirectory exists
        let path = self.entry_path(&key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Atomic write: temp file + rename
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, &content)?;
        fs::rename(&temp_path, &path)?;

        // Update LRU
        self.lru.insert(key, (SystemTime::now(), size));

        Ok(())
    }

    /// Get TTL duration for creating entries.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Scan existing cache entries to populate LRU.
    fn scan_entries(&mut self) {
        let Ok(subdirs) = fs::read_dir(&self.dir) else {
            return;
        };

        for subdir_entry in subdirs.flatten() {
            let subdir_path = subdir_entry.path();
            if !subdir_path.is_dir() {
                continue;
            }

            let Ok(files) = fs::read_dir(&subdir_path) else {
                continue;
            };

            for file_entry in files.flatten() {
                let file_path = file_entry.path();
                if file_path.extension().is_some_and(|e| e == "json")
                    && let Some(key) = file_path.file_stem().and_then(|s| s.to_str())
                {
                    let size = file_entry.metadata().map(|m| m.len()).unwrap_or(0);
                    let mtime = file_entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(UNIX_EPOCH);
                    self.lru.insert(key.to_string(), (mtime, size));
                }
            }
        }
    }

    /// Evict entries if cache limits exceeded.
    ///
    /// Per FR-WF-CCH-EVICT-01: Dual-limit eviction (interleaved LRU).
    /// Tie-break: oldest `last_accessed_at`, then lexicographic key.
    fn evict_if_needed(&mut self, new_entry_bytes: u64) -> Result<(), CacheWriteError> {
        // Calculate current usage
        let (current_entries, current_bytes) = self.calculate_usage();

        // Check if eviction needed
        let need_entries = current_entries >= self.max_entries;
        let need_bytes = current_bytes + new_entry_bytes > self.max_bytes;

        if !need_entries && !need_bytes {
            return Ok(());
        }

        // Sort by LRU (oldest first), then by key for tie-breaking
        let mut entries: Vec<_> = self
            .lru
            .iter()
            .map(|(k, (t, s))| (k.clone(), *t, *s))
            .collect();
        entries.sort_by(|a, b| {
            a.1.cmp(&b.1) // Primary: oldest first
                .then_with(|| a.0.cmp(&b.0)) // Secondary: lexicographic key
        });

        let mut removed_entries = 0u32;
        let mut removed_bytes = 0u64;

        // Evict until both limits satisfied
        for (key, _, size) in entries {
            let path = self.entry_path(&key);
            if path.exists() && fs::remove_file(&path).is_ok() {
                self.lru.remove(&key);
                removed_entries += 1;
                removed_bytes += size;

                // Check if we've freed enough
                let new_entries = current_entries.saturating_sub(removed_entries);
                let new_bytes = current_bytes.saturating_sub(removed_bytes);

                if new_entries < self.max_entries && new_bytes + new_entry_bytes <= self.max_bytes {
                    break;
                }
            }
        }

        Ok(())
    }

    /// Calculate current cache usage from LRU tracking.
    fn calculate_usage(&self) -> (u32, u64) {
        let entries = self.lru.len() as u32;
        let bytes: u64 = self.lru.values().map(|(_, s)| s).sum();
        (entries, bytes)
    }

    /// Get the path for a cache entry.
    ///
    /// Layout: `{cache_dir}/{first2}/{keyhex}.json`
    fn entry_path(&self, key: &str) -> PathBuf {
        let prefix = if key.len() >= 2 { &key[..2] } else { "00" };
        self.dir.join(prefix).join(format!("{key}.json"))
    }
}

/// Errors that can occur during cache write.
#[derive(Debug)]
pub enum CacheWriteError {
    /// Entry exceeds maximum size.
    EntryTooLarge { size: u64, max: u64 },
    /// Serialization failed.
    SerializationFailed(String),
    /// IO error.
    Io(std::io::Error),
}

impl std::fmt::Display for CacheWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheWriteError::EntryTooLarge { size, max } => {
                write!(f, "entry too large: {size} bytes (max {max})")
            }
            CacheWriteError::SerializationFailed(e) => write!(f, "serialization failed: {e}"),
            CacheWriteError::Io(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl std::error::Error for CacheWriteError {}

impl From<std::io::Error> for CacheWriteError {
    fn from(e: std::io::Error) -> Self {
        CacheWriteError::Io(e)
    }
}

/// Derive cache key from URL and rendering method.
///
/// Per FR-WF-16a: SHA256 of `canonical_url + "\n" + rendering_method`.
/// Fragment is removed from URL before hashing.
pub fn cache_key(url: &Url, method: RenderingMethod) -> String {
    let mut url = url.clone();
    url.set_fragment(None);

    let input = format!("{}\n{}", url.as_str(), method.as_str());

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    // Full SHA256 as hex (64 chars)
    hex_encode(&result)
}

/// Format `SystemTime` as RFC3339 with second precision.
pub fn format_rfc3339(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();

    // Calculate date/time components
    // Days since epoch
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Simple date calculation (good enough for our purposes)
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Parse RFC3339 timestamp to `SystemTime`.
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    // Expected format: YYYY-MM-DDTHH:MM:SSZ
    if s.len() < 20 {
        return None;
    }

    let year: u64 = s.get(0..4)?.parse().ok()?;
    let month: u64 = s.get(5..7)?.parse().ok()?;
    let day: u64 = s.get(8..10)?.parse().ok()?;
    let hour: u64 = s.get(11..13)?.parse().ok()?;
    let min: u64 = s.get(14..16)?.parse().ok()?;
    let sec: u64 = s.get(17..19)?.parse().ok()?;

    let days = ymd_to_days(year, month, day)?;
    let total_secs = days * 86400 + hour * 3600 + min * 60 + sec;

    Some(UNIX_EPOCH + Duration::from_secs(total_secs))
}

/// Convert days since epoch to year/month/day.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Simplified calculation - accurate for dates after 1970
    let mut remaining = days;
    let mut year = 1970;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for days_in_month in month_days {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        month += 1;
    }

    (year, month, remaining + 1)
}

/// Convert year/month/day to days since epoch.
fn ymd_to_days(year: u64, month: u64, day: u64) -> Option<u64> {
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut days = 0u64;

    // Years
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Months
    let leap = is_leap_year(year);
    let month_days = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    for m in 1..month {
        days += month_days[(m - 1) as usize];
    }

    // Days
    days += day - 1;

    Some(days)
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Hex encoding helper.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_deterministic() {
        let url1 = Url::parse("https://example.com/path").unwrap();
        let url2 = Url::parse("https://example.com/path").unwrap();
        assert_eq!(
            cache_key(&url1, RenderingMethod::Http),
            cache_key(&url2, RenderingMethod::Http)
        );
    }

    #[test]
    fn test_cache_key_ignores_fragment() {
        let url1 = Url::parse("https://example.com/path#section1").unwrap();
        let url2 = Url::parse("https://example.com/path#section2").unwrap();
        assert_eq!(
            cache_key(&url1, RenderingMethod::Http),
            cache_key(&url2, RenderingMethod::Http)
        );
    }

    #[test]
    fn test_cache_key_different_urls() {
        let url1 = Url::parse("https://example.com/path1").unwrap();
        let url2 = Url::parse("https://example.com/path2").unwrap();
        assert_ne!(
            cache_key(&url1, RenderingMethod::Http),
            cache_key(&url2, RenderingMethod::Http)
        );
    }

    #[test]
    fn test_cache_key_different_methods() {
        let url = Url::parse("https://example.com/path").unwrap();
        assert_ne!(
            cache_key(&url, RenderingMethod::Http),
            cache_key(&url, RenderingMethod::Browser)
        );
    }

    #[test]
    fn test_rfc3339_roundtrip() {
        let now = SystemTime::now();
        let formatted = format_rfc3339(now);
        let parsed = parse_rfc3339(&formatted).unwrap();

        // Should be within 1 second (we lose sub-second precision)
        let diff = now
            .duration_since(parsed)
            .or_else(|_| parsed.duration_since(now))
            .unwrap();
        assert!(diff.as_secs() <= 1);
    }

    #[test]
    fn test_entry_path_layout() {
        let temp = tempfile::tempdir().unwrap();
        let settings = CacheSettings {
            dir: temp.path().to_path_buf(),
            max_entries: 10,
            max_bytes: 1024 * 1024,
            ttl: Duration::from_secs(60),
        };
        let cache = Cache::new(&settings).unwrap();

        let url = Url::parse("https://example.com/test").unwrap();
        let key = cache_key(&url, RenderingMethod::Http);
        let path = cache.entry_path(&key);

        // Should have subdirectory based on first 2 chars
        let parent = path.parent().unwrap();
        let subdir = parent.file_name().unwrap().to_str().unwrap();
        assert_eq!(subdir, &key[..2]);

        // Should have .json extension
        assert_eq!(path.extension().unwrap(), "json");
    }

    #[test]
    fn test_cache_entry_expiration() {
        let entry = CacheEntry::new(
            "https://example.com".to_string(),
            None,
            None,
            "# Test".to_string(),
            Duration::from_secs(0), // Immediate expiration
        );

        // Should be expired immediately (or very soon)
        std::thread::sleep(Duration::from_millis(10));
        assert!(entry.is_expired());
    }
}

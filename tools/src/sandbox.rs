use std::path::{Path, PathBuf};

use super::{DenialReason, ToolError};

/// Default deny patterns for sensitive files in tool filesystem sandbox policy.
pub const DEFAULT_SANDBOX_DENY_PATTERNS: &[&str] = &[
    "**/.ssh/**",
    "**/.gnupg/**",
    "**/.aws/**",
    "**/.azure/**",
    "**/.config/gcloud/**",
    "**/.git/**",
    "**/.git-credentials",
    "**/.npmrc",
    "**/.pypirc",
    "**/.netrc",
    "**/.env",
    "**/.env.*",
    "**/*.env",
    "**/id_rsa*",
    "**/id_ed25519*",
    "**/id_ecdsa*",
    "**/*.pem",
    "**/*.key",
    "**/*.p12",
    "**/*.pfx",
    "**/*.der",
    "**/core",
    "**/core.*",
    "**/*.core",
    "**/*.dmp",
    "**/*.mdmp",
    "**/*.stackdump",
];

#[must_use]
pub fn default_sandbox_deny_patterns() -> Vec<String> {
    DEFAULT_SANDBOX_DENY_PATTERNS
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

#[derive(Debug, Clone)]
struct DenyPattern {
    pattern: String,
    matcher: globset::GlobMatcher,
}

/// Filesystem sandbox configuration and validation.
#[derive(Debug, Clone)]
pub struct Sandbox {
    allowed_roots: Vec<PathBuf>,
    deny_patterns: Vec<DenyPattern>,
    allow_absolute: bool,
}

/// Strip Windows extended-length path prefix (`\\?\`) if present.
fn strip_extended_prefix(path: &Path) -> &Path {
    #[cfg(windows)]
    {
        if let Some(s) = path.as_os_str().to_str() {
            let stripped = forge_types::strip_windows_extended_prefix(s);
            if stripped.len() != s.len() {
                return Path::new(stripped);
            }
        }
    }
    path
}

impl Sandbox {
    pub fn new(
        allowed_roots: Vec<PathBuf>,
        denied_patterns: Vec<String>,
        allow_absolute: bool,
    ) -> Result<Self, ToolError> {
        let mut roots = Vec::new();
        for root in allowed_roots {
            let canonical = std::fs::canonicalize(&root).map_err(|_e| {
                ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                    attempted: root.clone(),
                    resolved: root.clone(),
                })
            })?;
            roots.push(canonical);
        }

        let mut deny_patterns = Vec::new();
        for pat in denied_patterns {
            let mut builder = globset::GlobBuilder::new(&pat);
            // Always use case-insensitive matching for deny patterns to prevent bypasses
            // (e.g. "Secret.PEM" bypassing "*.pem" rule)
            builder.case_insensitive(true);
            let glob = builder.build().map_err(|e| ToolError::BadArgs {
                message: format!("Invalid denied pattern '{pat}': {e}"),
            })?;
            deny_patterns.push(DenyPattern {
                pattern: pat,
                matcher: glob.compile_matcher(),
            });
        }

        Ok(Self {
            allowed_roots: roots,
            deny_patterns,
            allow_absolute,
        })
    }

    #[must_use]
    pub fn working_dir(&self) -> PathBuf {
        self.allowed_roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Try to truncate an absolute path to a resolved path within an allowed root.
    ///
    /// Handles `\\?\` prefix mismatches on Windows (e.g. LLM passes `C:\foo`
    /// but allowed roots are stored as `\\?\C:\foo` from canonicalization).
    fn truncate_to_allowed_root(&self, path: &Path) -> Option<PathBuf> {
        let clean = strip_extended_prefix(path);
        self.allowed_roots.iter().find_map(|root| {
            let clean_root = strip_extended_prefix(root);
            clean
                .strip_prefix(clean_root)
                .ok()
                .map(|rel| root.join(rel))
        })
    }

    /// Validate and resolve a path within the sandbox.
    pub fn resolve_path(&self, path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
        let resolved = self.validate_and_resolve(path, working_dir)?;
        let canonical = canonicalize_existing(&resolved)?;
        self.check_allowed(&resolved, canonical)
    }

    /// Validate and resolve a path for file creation, allowing non-existent directories.
    ///
    /// Unlike `resolve_path`, this method handles paths where parent directories don't
    /// exist yet (as long as they would be within the sandbox). This is used by
    /// `write_file` to allow creating files in new directories.
    pub fn resolve_path_for_create(
        &self,
        path: &str,
        working_dir: &Path,
    ) -> Result<PathBuf, ToolError> {
        let resolved = self.validate_and_resolve(path, working_dir)?;
        let canonical = canonicalize_for_create(&resolved)?;
        self.check_allowed(&resolved, canonical)
    }

    /// Validate a resolved path (absolute) against sandbox rules.
    pub fn ensure_path_allowed(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let canonical = std::fs::canonicalize(path).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: path.to_path_buf(),
                resolved: path.to_path_buf(),
            })
        })?;
        self.check_allowed(path, canonical)
    }

    /// Shared validation prefix: unsafe chars, NTFS ADS, `..` rejection, absolute
    /// path handling. Returns the resolved-but-not-yet-canonicalized path.
    fn validate_and_resolve(&self, path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
        if contains_unsafe_path_chars(path) {
            return Err(ToolError::BadArgs {
                message: "path contains invalid control characters".to_string(),
            });
        }
        if contains_ntfs_ads(path) {
            return Err(ToolError::BadArgs {
                message: "path contains NTFS alternate data stream syntax".to_string(),
            });
        }
        let input = PathBuf::from(path);
        if input
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ToolError::SandboxViolation(
                DenialReason::PathOutsideSandbox {
                    attempted: input.clone(),
                    resolved: input.clone(),
                },
            ));
        }
        if input.is_absolute() {
            if self.allow_absolute {
                Ok(input)
            } else if let Some(truncated) = self.truncate_to_allowed_root(&input) {
                Ok(truncated)
            } else {
                Err(ToolError::SandboxViolation(
                    DenialReason::PathOutsideSandbox {
                        attempted: input.clone(),
                        resolved: input.clone(),
                    },
                ))
            }
        } else {
            Ok(working_dir.join(input))
        }
    }

    /// Shared post-canonicalize suffix: allowed-root check + deny-pattern check.
    fn check_allowed(&self, resolved: &Path, canonical: PathBuf) -> Result<PathBuf, ToolError> {
        if !self.is_within_allowed_roots(&canonical) {
            return Err(ToolError::SandboxViolation(
                DenialReason::PathOutsideSandbox {
                    attempted: resolved.to_path_buf(),
                    resolved: canonical,
                },
            ));
        }
        if let Some(pat) = self.matches_denied_pattern(&canonical) {
            return Err(ToolError::SandboxViolation(
                DenialReason::DeniedPatternMatched {
                    attempted: canonical,
                    pattern: pat,
                },
            ));
        }
        Ok(canonical)
    }

    /// Post-creation validation for TOCTOU mitigation.
    ///
    /// After `create_dir_all` creates the directory tree and before writing content,
    /// re-canonicalize the parent and verify it's still within allowed roots and
    /// contains no symlinks in the path chain. This closes the race window where
    /// an attacker process could replace a directory with a symlink between
    /// `resolve_path_for_create` and the actual write.
    pub fn validate_created_parent(&self, path: &Path) -> Result<(), ToolError> {
        let parent = path.parent().ok_or_else(|| ToolError::BadArgs {
            message: "path has no parent directory".to_string(),
        })?;

        // Walk the path chain checking for symlinks
        let mut current = parent.to_path_buf();
        loop {
            if let Ok(meta) = std::fs::symlink_metadata(&current)
                && meta.file_type().is_symlink()
            {
                return Err(ToolError::SandboxViolation(
                    DenialReason::PathOutsideSandbox {
                        attempted: path.to_path_buf(),
                        resolved: current,
                    },
                ));
            }
            match current.parent() {
                Some(p) if p != current => current = p.to_path_buf(),
                _ => break,
            }
        }

        // Re-canonicalize and verify within allowed roots
        let canonical = std::fs::canonicalize(parent).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: path.to_path_buf(),
                resolved: parent.to_path_buf(),
            })
        })?;
        if !self.is_within_allowed_roots(&canonical) {
            return Err(ToolError::SandboxViolation(
                DenialReason::PathOutsideSandbox {
                    attempted: path.to_path_buf(),
                    resolved: canonical,
                },
            ));
        }
        Ok(())
    }

    fn is_within_allowed_roots(&self, path: &Path) -> bool {
        self.allowed_roots.iter().any(|root| path.starts_with(root))
    }

    fn matches_denied_pattern(&self, path: &Path) -> Option<String> {
        let normalized = normalize_path(path);
        for pat in &self.deny_patterns {
            if pat.matcher.is_match(&normalized) {
                return Some(pat.pattern.clone());
            }
        }
        None
    }

    /// Check if a path matches any deny pattern (lightweight, no canonicalization).
    ///
    /// Suitable for filtering large scans. Does NOT validate unsafe chars,
    /// NTFS ADS, traversal, or allowed roots — only glob deny patterns.
    /// For full validation, use `resolve_path()`.
    #[must_use]
    pub fn is_path_denied(&self, path: &Path) -> bool {
        self.matches_denied_pattern(path).is_some()
    }
}

/// Canonicalize a path that should exist, or whose parent must exist.
fn canonicalize_existing(resolved: &Path) -> Result<PathBuf, ToolError> {
    if resolved.exists() {
        std::fs::canonicalize(resolved).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: resolved.to_path_buf(),
                resolved: resolved.to_path_buf(),
            })
        })
    } else {
        let parent = resolved.parent().ok_or_else(|| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: resolved.to_path_buf(),
                resolved: resolved.to_path_buf(),
            })
        })?;
        let parent_canon = std::fs::canonicalize(parent).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: resolved.to_path_buf(),
                resolved: resolved.to_path_buf(),
            })
        })?;
        Ok(parent_canon.join(resolved.file_name().unwrap_or_default()))
    }
}

/// Canonicalize for creation: walk up to the nearest existing ancestor.
fn canonicalize_for_create(resolved: &Path) -> Result<PathBuf, ToolError> {
    if resolved.exists() {
        return std::fs::canonicalize(resolved).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: resolved.to_path_buf(),
                resolved: resolved.to_path_buf(),
            })
        });
    }

    let mut existing_ancestor = resolved.parent();
    let mut non_existent_parts: Vec<&std::ffi::OsStr> = Vec::new();

    if let Some(file_name) = resolved.file_name() {
        non_existent_parts.push(file_name);
    }

    while let Some(ancestor) = existing_ancestor {
        if ancestor.exists() {
            break;
        }
        if let Some(dir_name) = ancestor.file_name() {
            non_existent_parts.push(dir_name);
        }
        existing_ancestor = ancestor.parent();
    }

    let existing = existing_ancestor.ok_or_else(|| {
        ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
            attempted: resolved.to_path_buf(),
            resolved: resolved.to_path_buf(),
        })
    })?;

    let canon_existing = std::fs::canonicalize(existing).map_err(|_| {
        ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
            attempted: resolved.to_path_buf(),
            resolved: resolved.to_path_buf(),
        })
    })?;

    // Rejoin non-existent parts in reverse order (they were collected bottom-up)
    let mut result = canon_existing;
    for part in non_existent_parts.into_iter().rev() {
        result = result.join(part);
    }
    Ok(result)
}

fn normalize_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        strip_ads_suffixes(&normalized)
    } else {
        normalized
    }
}

/// Strip NTFS Alternate Data Stream suffixes from path segments for deny matching.
///
/// Defense-in-depth: even if `contains_ntfs_ads` rejects ADS paths at the entry
/// point, this ensures deny patterns match the base filename (e.g. `.env::$DATA`
/// is matched as `.env`).
fn strip_ads_suffixes(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut first = true;
    for segment in path.split('/') {
        if !first {
            result.push('/');
        }
        // Preserve drive letter (e.g. "C:")
        if first
            && segment.len() == 2
            && segment.as_bytes()[0].is_ascii_alphabetic()
            && segment.as_bytes()[1] == b':'
        {
            result.push_str(segment);
        } else if let Some(pos) = segment.find(':') {
            result.push_str(&segment[..pos]);
        } else {
            result.push_str(segment);
        }
        first = false;
    }
    result
}

/// Detect NTFS Alternate Data Stream syntax in path components.
///
/// On Windows, `:` in a `Component::Normal` is always ADS syntax — it's not a
/// valid filename character except in the drive letter prefix (`C:`). Paths like
/// `.env::$DATA` or `file.txt:stream` can bypass deny pattern matching because
/// the glob sees the ADS-suffixed name instead of the base filename.
///
/// Returns `false` on non-Windows platforms (colons are valid in Unix filenames).
fn contains_ntfs_ads(input: &str) -> bool {
    if !cfg!(windows) {
        return false;
    }
    Path::new(input)
        .components()
        .any(|c| matches!(c, std::path::Component::Normal(s) if s.to_string_lossy().contains(':')))
}

fn contains_unsafe_path_chars(input: &str) -> bool {
    input.chars().any(is_unsafe_path_char)
}

///
/// Rejects C0/C1 control characters, DEL, and the full steganographic
/// character set from [`forge_types::is_steganographic_char`]. Invisible
/// characters in paths cause platform-dependent behavior and can bypass
/// security checks or confuse users.
///
/// The steganographic predicate is composed (not copied) from `forge-types`
/// per IFA §7.1 — single point of encoding for shared invariant logic.
fn is_unsafe_path_char(c: char) -> bool {
    // C0/C1 control characters and DEL (path-specific; not in the steganographic set)
    matches!(c, '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{0080}'..='\u{009f}')
        || forge_types::is_steganographic_char(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // is_unsafe_path_char tests

    #[test]
    fn safe_chars_not_flagged() {
        assert!(!is_unsafe_path_char('a'));
        assert!(!is_unsafe_path_char('Z'));
        assert!(!is_unsafe_path_char('0'));
        assert!(!is_unsafe_path_char('/'));
        assert!(!is_unsafe_path_char('\\'));
        assert!(!is_unsafe_path_char('.'));
        assert!(!is_unsafe_path_char('-'));
        assert!(!is_unsafe_path_char('_'));
        assert!(!is_unsafe_path_char(' '));
    }

    #[test]
    fn null_char_is_unsafe() {
        assert!(is_unsafe_path_char('\u{0000}'));
    }

    #[test]
    fn control_chars_are_unsafe() {
        assert!(is_unsafe_path_char('\u{0001}')); // SOH
        assert!(is_unsafe_path_char('\u{001f}')); // US
        assert!(is_unsafe_path_char('\u{007f}')); // DEL
    }

    #[test]
    fn c1_control_chars_are_unsafe() {
        assert!(is_unsafe_path_char('\u{0080}'));
        assert!(is_unsafe_path_char('\u{009f}'));
    }

    #[test]
    fn bidi_chars_are_unsafe() {
        assert!(is_unsafe_path_char('\u{061c}')); // Arabic Letter Mark
        assert!(is_unsafe_path_char('\u{200e}')); // LRM
        assert!(is_unsafe_path_char('\u{200f}')); // RLM
        assert!(is_unsafe_path_char('\u{202a}')); // LRE
        assert!(is_unsafe_path_char('\u{202e}')); // RLO
        assert!(is_unsafe_path_char('\u{2066}')); // LRI
        assert!(is_unsafe_path_char('\u{2069}')); // PDI
    }

    #[test]
    fn zero_width_chars_are_unsafe() {
        assert!(is_unsafe_path_char('\u{200b}')); // ZWSP
        assert!(is_unsafe_path_char('\u{200c}')); // ZWNJ
        assert!(is_unsafe_path_char('\u{200d}')); // ZWJ
    }

    #[test]
    fn unicode_tags_are_unsafe() {
        assert!(is_unsafe_path_char('\u{e0000}'));
        assert!(is_unsafe_path_char('\u{e0041}')); // Tag 'A'
        assert!(is_unsafe_path_char('\u{e007f}'));
    }

    #[test]
    fn variation_selectors_are_unsafe() {
        assert!(is_unsafe_path_char('\u{fe00}')); // VS1
        assert!(is_unsafe_path_char('\u{fe0f}')); // VS16
        assert!(is_unsafe_path_char('\u{e0100}')); // VS17
        assert!(is_unsafe_path_char('\u{e01ef}')); // VS256
    }

    #[test]
    fn soft_hyphen_is_unsafe() {
        assert!(is_unsafe_path_char('\u{00ad}'));
    }

    #[test]
    fn steganographic_fillers_are_unsafe() {
        assert!(is_unsafe_path_char('\u{feff}')); // BOM / ZWNBSP
        assert!(is_unsafe_path_char('\u{034f}')); // CGJ
        assert!(is_unsafe_path_char('\u{115f}')); // Hangul Choseong Filler
        assert!(is_unsafe_path_char('\u{3164}')); // Hangul Filler
        assert!(is_unsafe_path_char('\u{180e}')); // Mongolian Vowel Separator
    }

    #[test]
    fn path_with_zwsp_flagged() {
        assert!(contains_unsafe_path_chars("src/ma\u{200b}in.rs"));
    }

    #[test]
    fn path_with_unicode_tags_flagged() {
        assert!(contains_unsafe_path_chars("src/\u{e0041}\u{e0042}main.rs"));
    }

    // contains_unsafe_path_chars tests

    #[test]
    fn safe_path_not_flagged() {
        assert!(!contains_unsafe_path_chars("/home/user/file.txt"));
        assert!(!contains_unsafe_path_chars("C:\\Users\\test\\file.txt"));
        assert!(!contains_unsafe_path_chars("relative/path/to/file"));
    }

    #[test]
    fn path_with_null_flagged() {
        assert!(contains_unsafe_path_chars("test\u{0000}file"));
    }

    #[test]
    fn path_with_bidi_flagged() {
        assert!(contains_unsafe_path_chars("test\u{202e}file"));
    }

    // normalize_path tests

    #[test]
    fn normalize_converts_backslashes() {
        let path = Path::new("C:\\Users\\test\\file.txt");
        let normalized = normalize_path(path);
        assert_eq!(normalized, "C:/Users/test/file.txt");
    }

    #[test]
    fn normalize_preserves_forward_slashes() {
        let path = Path::new("/home/user/file.txt");
        let normalized = normalize_path(path);
        assert_eq!(normalized, "/home/user/file.txt");
    }

    // Sandbox construction tests

    #[test]
    fn sandbox_new_with_valid_root() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false);
        assert!(sandbox.is_ok());
    }

    #[test]
    fn sandbox_new_with_nonexistent_root_fails() {
        let result = Sandbox::new(vec![PathBuf::from("/nonexistent/path/xyz")], vec![], false);
        assert!(result.is_err());
    }

    #[test]
    fn sandbox_new_with_invalid_glob_pattern_fails() {
        let temp = tempdir().unwrap();
        let result = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["[invalid".to_string()],
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn sandbox_working_dir_returns_first_root() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();
        assert_eq!(
            sandbox.working_dir(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
    }

    #[test]
    fn sandbox_working_dir_fallback_to_dot() {
        // Create a sandbox with no roots by removing them after creation
        // This is tricky since new() requires roots - test the method directly
        let sandbox = Sandbox {
            allowed_roots: vec![],
            deny_patterns: vec![],
            allow_absolute: false,
        };
        assert_eq!(sandbox.working_dir(), PathBuf::from("."));
    }

    // Sandbox resolve_path tests

    #[test]
    fn resolve_path_relative_path_within_sandbox() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "content").unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.resolve_path("test.txt", temp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_path_rejects_parent_dir() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.resolve_path("../escape", temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_path_rejects_unsafe_chars() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.resolve_path("test\u{0000}file.txt", temp.path());
        assert!(result.is_err());
        if let Err(ToolError::BadArgs { message }) = result {
            assert!(message.contains("invalid control characters"));
        }
    }

    #[test]
    fn resolve_path_rejects_absolute_outside_roots() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.resolve_path("/etc/passwd", temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_path_truncates_absolute_within_roots() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        // LLM passes the canonical absolute path — should be truncated, not rejected
        let canonical_root = std::fs::canonicalize(temp.path()).unwrap();
        let abs_file = canonical_root.join("file.txt");
        let result = sandbox.resolve_path(abs_file.to_str().unwrap(), temp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_path_truncates_absolute_root_itself() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        // LLM passes the root directory as an absolute path
        let canonical_root = std::fs::canonicalize(temp.path()).unwrap();
        let result = sandbox.resolve_path(canonical_root.to_str().unwrap(), temp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_path_allows_absolute_when_allowed() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], true).unwrap();

        let abs_path = temp.path().join("file.txt");
        let result = sandbox.resolve_path(abs_path.to_str().unwrap(), temp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_path_rejects_denied_pattern() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("secret.env"), "content").unwrap();
        let sandbox = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["*.env".to_string()],
            false,
        )
        .unwrap();

        let result = sandbox.resolve_path("secret.env", temp.path());
        assert!(result.is_err());
        if let Err(ToolError::SandboxViolation(DenialReason::DeniedPatternMatched {
            pattern,
            ..
        })) = result
        {
            assert_eq!(pattern, "*.env");
        }
    }

    #[test]
    fn resolve_path_new_file_in_sandbox() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        // New file that doesn't exist yet
        let result = sandbox.resolve_path("newfile.txt", temp.path());
        assert!(result.is_ok());
    }

    // Sandbox ensure_path_allowed tests

    #[test]
    fn ensure_path_allowed_within_sandbox() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("allowed.txt");
        std::fs::write(&file_path, "content").unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.ensure_path_allowed(&file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn ensure_path_allowed_rejects_outside_sandbox() {
        let temp1 = tempdir().unwrap();
        let temp2 = tempdir().unwrap();
        let file_path = temp2.path().join("outside.txt");
        std::fs::write(&file_path, "content").unwrap();
        let sandbox = Sandbox::new(vec![temp1.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.ensure_path_allowed(&file_path);
        assert!(result.is_err());
    }

    #[test]
    fn ensure_path_allowed_rejects_denied_pattern() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join(".secret");
        std::fs::write(&file_path, "content").unwrap();
        let sandbox = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["*.secret".to_string()],
            false,
        )
        .unwrap();

        let result = sandbox.ensure_path_allowed(&file_path);
        assert!(result.is_err());
    }

    // NTFS ADS detection tests

    #[test]
    fn contains_ntfs_ads_detects_stream_syntax() {
        if cfg!(windows) {
            assert!(contains_ntfs_ads(".env::$DATA"));
            assert!(contains_ntfs_ads("secret.key:stream"));
            assert!(contains_ntfs_ads("sub/.env::$DATA"));
            // Drive letter is not ADS
            assert!(!contains_ntfs_ads("C:\\Users\\test\\file.txt"));
        } else {
            // On non-Windows, colons are valid filenames
            assert!(!contains_ntfs_ads(".env::$DATA"));
        }
    }

    #[test]
    fn strip_ads_suffixes_removes_stream_names() {
        assert_eq!(strip_ads_suffixes(".env::$DATA"), ".env");
        assert_eq!(strip_ads_suffixes("secret.key:stream"), "secret.key");
        assert_eq!(strip_ads_suffixes("sub/.env::$DATA"), "sub/.env");
        assert_eq!(strip_ads_suffixes("no_ads/file.txt"), "no_ads/file.txt");
        // Preserve drive letter
        assert_eq!(strip_ads_suffixes("C:/Users/test"), "C:/Users/test");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_path_rejects_ntfs_ads() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join(".env"), "SECRET=x").unwrap();
        let sandbox = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["**/.env".to_string()],
            false,
        )
        .unwrap();

        // ADS bypass attempt must be rejected
        let result = sandbox.resolve_path(".env::$DATA", temp.path());
        assert!(result.is_err());
        let result = sandbox.resolve_path(".env:stream", temp.path());
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn normalize_path_strips_ads_for_deny_matching() {
        let path = Path::new("C:\\Users\\test\\.env::$DATA");
        let normalized = normalize_path(path);
        assert!(normalized.contains(".env"));
        assert!(!normalized.contains("::$DATA"));
    }

    // DenyPattern tests

    #[test]
    fn deny_pattern_matches_glob() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("test.log"), "content").unwrap();
        let sandbox = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["**/*.log".to_string()],
            false,
        )
        .unwrap();

        let result = sandbox.resolve_path("test.log", temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn deny_pattern_double_star_matches_nested() {
        let temp = tempdir().unwrap();
        let nested = temp.path().join("sub");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("deep.log"), "content").unwrap();
        let sandbox = Sandbox::new(
            vec![temp.path().to_path_buf()],
            vec!["**/*.log".to_string()],
            false,
        )
        .unwrap();

        let result = sandbox.resolve_path("sub/deep.log", temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn is_path_denied_matches_deny_pattern() {
        let dir = tempdir().unwrap();
        let sandbox = Sandbox::new(
            vec![dir.path().to_path_buf()],
            vec!["**/.env".to_string()],
            false,
        )
        .unwrap();

        assert!(sandbox.is_path_denied(Path::new("subdir/.env")));
        assert!(!sandbox.is_path_denied(Path::new("allowed.txt")));
    }
}

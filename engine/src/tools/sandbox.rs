use std::path::{Path, PathBuf};

use super::{DenialReason, ToolError};

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
            if cfg!(windows) {
                builder.case_insensitive(true);
            }
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

    #[allow(dead_code)]
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }

    #[allow(dead_code)]
    pub fn allow_absolute(&self) -> bool {
        self.allow_absolute
    }

    pub fn working_dir(&self) -> PathBuf {
        self.allowed_roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Validate and resolve a path within the sandbox.
    pub fn resolve_path(&self, path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
        if contains_unsafe_path_chars(path) {
            return Err(ToolError::BadArgs {
                message: "path contains invalid control characters".to_string(),
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
        let resolved = if input.is_absolute() {
            if !self.allow_absolute {
                return Err(ToolError::SandboxViolation(
                    DenialReason::PathOutsideSandbox {
                        attempted: input.clone(),
                        resolved: input.clone(),
                    },
                ));
            }
            input
        } else {
            working_dir.join(input)
        };

        let canonical = if resolved.exists() {
            std::fs::canonicalize(&resolved).map_err(|_| {
                ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                    attempted: resolved.clone(),
                    resolved: resolved.clone(),
                })
            })?
        } else {
            let parent = resolved.parent().ok_or_else(|| {
                ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                    attempted: resolved.clone(),
                    resolved: resolved.clone(),
                })
            })?;
            let parent_canon = std::fs::canonicalize(parent).map_err(|_| {
                ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                    attempted: resolved.clone(),
                    resolved: resolved.clone(),
                })
            })?;
            parent_canon.join(resolved.file_name().unwrap_or_default())
        };

        if !self.is_within_allowed_roots(&canonical) {
            return Err(ToolError::SandboxViolation(
                DenialReason::PathOutsideSandbox {
                    attempted: resolved,
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

    /// Validate a resolved path (absolute) against sandbox rules.
    pub fn ensure_path_allowed(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let canonical = std::fs::canonicalize(path).map_err(|_| {
            ToolError::SandboxViolation(DenialReason::PathOutsideSandbox {
                attempted: path.to_path_buf(),
                resolved: path.to_path_buf(),
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
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn contains_unsafe_path_chars(input: &str) -> bool {
    input.chars().any(is_unsafe_path_char)
}

fn is_unsafe_path_char(c: char) -> bool {
    matches!(
        c,
        '\u{0000}'..='\u{001f}'
            | '\u{007f}'
            | '\u{0080}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ========================================================================
    // is_unsafe_path_char tests
    // ========================================================================

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

    // ========================================================================
    // contains_unsafe_path_chars tests
    // ========================================================================

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

    // ========================================================================
    // normalize_path tests
    // ========================================================================

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

    // ========================================================================
    // Sandbox construction tests
    // ========================================================================

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

    // ========================================================================
    // Sandbox resolve_path tests
    // ========================================================================

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
    fn resolve_path_rejects_absolute_when_disallowed() {
        let temp = tempdir().unwrap();
        let sandbox = Sandbox::new(vec![temp.path().to_path_buf()], vec![], false).unwrap();

        let result = sandbox.resolve_path("/etc/passwd", temp.path());
        assert!(result.is_err());
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

    // ========================================================================
    // Sandbox ensure_path_allowed tests
    // ========================================================================

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

    // ========================================================================
    // DenyPattern tests
    // ========================================================================

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
}

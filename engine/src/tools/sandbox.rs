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

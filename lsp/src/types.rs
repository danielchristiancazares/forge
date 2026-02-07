//! Public types consumed by the engine.
//!
//! These types define the interface between `forge-lsp` and `forge-engine`.
//! The engine constructs [`LspConfig`], receives [`LspEvent`]s, and reads
//! [`DiagnosticsSnapshot`]s for UI display and agent feedback.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for the LSP client subsystem.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LspConfig {
    /// Whether the LSP client is enabled. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Per-language server configurations, keyed by name (e.g. "rust").
    #[serde(default)]
    pub servers: HashMap<String, ServerConfig>,
}

/// Configuration for a single language server.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Executable command (e.g. "rust-analyzer").
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// LSP language identifier (e.g. "rust", "python").
    pub language_id: String,
    /// File extensions this server handles (e.g. `["rs"]`).
    #[serde(default)]
    pub file_extensions: Vec<String>,
    /// Files that indicate a workspace root (e.g. `["Cargo.toml"]`).
    #[serde(default)]
    pub root_markers: Vec<String>,
}

/// Severity level for a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl DiagnosticSeverity {
    /// Convert from LSP numeric severity (1=Error, 2=Warning, 3=Info, 4=Hint).
    #[must_use]
    pub fn from_lsp(value: u64) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            _ => Self::Hint,
        }
    }

    #[must_use]
    pub fn is_error(self) -> bool {
        self == Self::Error
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "info",
            Self::Hint => "hint",
        }
    }
}

/// A single diagnostic from a language server.
#[derive(Debug, Clone)]
pub struct ForgeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed column.
    pub col: u32,
    /// Source of the diagnostic (e.g. "rustc", "clippy").
    pub source: Option<String>,
}

impl ForgeDiagnostic {
    /// Format as `path:line:col: severity: message` (1-indexed for display).
    #[must_use]
    pub fn display_with_path(&self, path: &std::path::Path) -> String {
        let source = self
            .source
            .as_deref()
            .map(|s| format!("[{s}] "))
            .unwrap_or_default();
        format!(
            "{}:{}:{}: {}: {source}{}",
            path.display(),
            self.line + 1,
            self.col + 1,
            self.severity.label(),
            self.message,
        )
    }
}

/// An event emitted by the LSP subsystem.
#[derive(Debug)]
pub enum LspEvent {
    /// Server status changed.
    Status { server: String, state: LspState },
    /// Diagnostics updated for a file.
    Diagnostics {
        path: PathBuf,
        items: Vec<ForgeDiagnostic>,
    },
}

/// State of a language server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspState {
    Starting,
    Running,
    Stopped,
    Failed(String),
}

/// Immutable snapshot of all diagnostics, suitable for UI rendering.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    pub hint_count: usize,
    /// Per-file diagnostics, sorted with error-containing files first.
    pub files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>,
}

impl DiagnosticsSnapshot {
    /// Whether there are any diagnostics.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Total diagnostic count across all files.
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.error_count + self.warning_count + self.info_count + self.hint_count
    }

    /// Format a compact status string like "E:3 W:5".
    #[must_use]
    pub fn status_string(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("E:{} W:{}", self.error_count, self.warning_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(severity: DiagnosticSeverity, msg: &str) -> ForgeDiagnostic {
        ForgeDiagnostic {
            severity,
            message: msg.to_string(),
            line: 10,
            col: 5,
            source: Some("rustc".to_string()),
        }
    }

    // ── DiagnosticSeverity ─────────────────────────────────────────────

    #[test]
    fn test_from_lsp_known_values() {
        assert_eq!(DiagnosticSeverity::from_lsp(1), DiagnosticSeverity::Error);
        assert_eq!(DiagnosticSeverity::from_lsp(2), DiagnosticSeverity::Warning);
        assert_eq!(
            DiagnosticSeverity::from_lsp(3),
            DiagnosticSeverity::Information
        );
        assert_eq!(DiagnosticSeverity::from_lsp(4), DiagnosticSeverity::Hint);
    }

    #[test]
    fn test_from_lsp_unknown_defaults_to_hint() {
        assert_eq!(DiagnosticSeverity::from_lsp(0), DiagnosticSeverity::Hint);
        assert_eq!(DiagnosticSeverity::from_lsp(99), DiagnosticSeverity::Hint);
    }

    #[test]
    fn test_is_error() {
        assert!(DiagnosticSeverity::Error.is_error());
        assert!(!DiagnosticSeverity::Warning.is_error());
        assert!(!DiagnosticSeverity::Information.is_error());
        assert!(!DiagnosticSeverity::Hint.is_error());
    }

    #[test]
    fn test_severity_label() {
        assert_eq!(DiagnosticSeverity::Error.label(), "error");
        assert_eq!(DiagnosticSeverity::Warning.label(), "warning");
        assert_eq!(DiagnosticSeverity::Information.label(), "info");
        assert_eq!(DiagnosticSeverity::Hint.label(), "hint");
    }

    // ── ForgeDiagnostic ────────────────────────────────────────────────

    #[test]
    fn test_display_with_path() {
        let diag = ForgeDiagnostic {
            severity: DiagnosticSeverity::Error,
            message: "expected `;`".to_string(),
            line: 10,
            col: 5,
            source: Some("rustc".to_string()),
        };
        let path = PathBuf::from("src/main.rs");
        // line/col are 0-indexed internally, displayed as 1-indexed
        assert_eq!(
            diag.display_with_path(&path),
            "src/main.rs:11:6: error: [rustc] expected `;`"
        );
    }

    #[test]
    fn test_display_with_path_no_source() {
        let diag = ForgeDiagnostic {
            severity: DiagnosticSeverity::Warning,
            message: "unused variable".to_string(),
            line: 0,
            col: 0,
            source: None,
        };
        let path = PathBuf::from("lib.rs");
        assert_eq!(
            diag.display_with_path(&path),
            "lib.rs:1:1: warning: unused variable"
        );
    }

    // ── DiagnosticsSnapshot ────────────────────────────────────────────

    #[test]
    fn test_snapshot_default_is_empty() {
        let snap = DiagnosticsSnapshot::default();
        assert!(snap.is_empty());
        assert_eq!(snap.total_count(), 0);
        assert_eq!(snap.status_string(), "");
    }

    #[test]
    fn test_snapshot_total_count() {
        let snap = DiagnosticsSnapshot {
            error_count: 3,
            warning_count: 5,
            info_count: 2,
            hint_count: 1,
            files: vec![(
                PathBuf::from("a.rs"),
                vec![make_diag(DiagnosticSeverity::Error, "e")],
            )],
        };
        assert_eq!(snap.total_count(), 11);
        assert!(!snap.is_empty());
    }

    #[test]
    fn test_snapshot_status_string_format() {
        let snap = DiagnosticsSnapshot {
            error_count: 2,
            warning_count: 7,
            info_count: 0,
            hint_count: 0,
            files: vec![(
                PathBuf::from("a.rs"),
                vec![make_diag(DiagnosticSeverity::Error, "e")],
            )],
        };
        assert_eq!(snap.status_string(), "E:2 W:7");
    }

    #[test]
    fn test_snapshot_total_count_includes_info_and_hint() {
        let snap = DiagnosticsSnapshot {
            error_count: 0,
            warning_count: 0,
            info_count: 3,
            hint_count: 4,
            files: vec![(
                PathBuf::from("a.rs"),
                vec![make_diag(DiagnosticSeverity::Hint, "h")],
            )],
        };
        assert_eq!(snap.total_count(), 7);
        // status_string only shows E/W, not info/hint
        assert_eq!(snap.status_string(), "E:0 W:0");
        assert!(!snap.is_empty());
    }

    // ── LspConfig deserialization ──────────────────────────────────────

    #[test]
    fn test_lsp_config_defaults() {
        let config: LspConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.enabled);
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_lsp_config_with_server() {
        let json = serde_json::json!({
            "enabled": true,
            "servers": {
                "rust": {
                    "command": "rust-analyzer",
                    "language_id": "rust",
                    "file_extensions": ["rs"],
                    "root_markers": ["Cargo.toml"]
                }
            }
        });
        let config: LspConfig = serde_json::from_value(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.servers.len(), 1);
        let rust = &config.servers["rust"];
        assert_eq!(rust.command, "rust-analyzer");
        assert_eq!(rust.language_id, "rust");
        assert_eq!(rust.file_extensions, vec!["rs"]);
        assert_eq!(rust.root_markers, vec!["Cargo.toml"]);
        assert!(rust.args.is_empty());
    }
}

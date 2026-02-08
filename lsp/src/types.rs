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
    ///
    /// Returns `None` for values outside the LSP-defined range.
    /// Callers (boundary code) decide the fallback policy.
    #[must_use]
    pub fn from_lsp(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::Error),
            2 => Some(Self::Warning),
            3 => Some(Self::Information),
            4 => Some(Self::Hint),
            _ => None,
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
///
/// Fields are private; construction is restricted to `pub(crate)` (Authority
/// Boundary). External consumers read via accessors.
#[derive(Debug, Clone)]
pub struct ForgeDiagnostic {
    severity: DiagnosticSeverity,
    message: String,
    /// 0-indexed line number.
    line: u32,
    /// 0-indexed column.
    col: u32,
    /// Source of the diagnostic (e.g. "rustc", "clippy").
    /// Resolved to a concrete string at the boundary — no `Option` in core (IFA §11.2).
    source: String,
}

impl ForgeDiagnostic {
    /// Construct a diagnostic with all required fields.
    ///
    /// This is the single construction path (Authority Boundary).
    /// External callers may construct diagnostics for testing; the private
    /// fields prevent mutation after construction.
    #[must_use]
    pub fn new(
        severity: DiagnosticSeverity,
        message: String,
        line: u32,
        col: u32,
        source: String,
    ) -> Self {
        Self {
            severity,
            message,
            line,
            col,
            source,
        }
    }

    #[must_use]
    pub fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 0-indexed line number.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// 0-indexed column.
    #[must_use]
    pub fn col(&self) -> u32 {
        self.col
    }

    /// Source of the diagnostic (e.g. "rustc", "clippy").
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Format as `path:line:col: severity: message` (1-indexed for display).
    #[must_use]
    pub fn display_with_path(&self, path: &std::path::Path) -> String {
        format!(
            "{}:{}:{}: {}: [{}] {}",
            path.display(),
            self.line + 1,
            self.col + 1,
            self.severity.label(),
            self.source,
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
///
/// Fields are private; counts are computed from the canonical source (`files`).
/// This eliminates the synchronization obligation between cached counts and
/// the actual diagnostics (IFA §7.6).
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    /// Per-file diagnostics, sorted with error-containing files first.
    files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>,
}

impl DiagnosticsSnapshot {
    /// Construct a snapshot from sorted per-file diagnostics.
    pub(crate) fn new(files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>) -> Self {
        Self { files }
    }

    /// Per-file diagnostics, sorted with error-containing files first.
    #[must_use]
    pub fn files(&self) -> &[(PathBuf, Vec<ForgeDiagnostic>)] {
        &self.files
    }

    /// Whether there are any diagnostics.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn count_by_severity(&self, severity: DiagnosticSeverity) -> usize {
        self.files
            .iter()
            .flat_map(|(_, items)| items)
            .filter(|d| d.severity() == severity)
            .count()
    }

    /// Number of error-level diagnostics.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Error)
    }

    /// Number of warning-level diagnostics.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Warning)
    }

    /// Number of info-level diagnostics.
    #[must_use]
    pub fn info_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Information)
    }

    /// Number of hint-level diagnostics.
    #[must_use]
    pub fn hint_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Hint)
    }

    /// Total diagnostic count across all files.
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.files.iter().map(|(_, items)| items.len()).sum()
    }

    /// Format a compact status string like "E:3 W:5".
    #[must_use]
    pub fn status_string(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("E:{} W:{}", self.error_count(), self.warning_count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(severity: DiagnosticSeverity, msg: &str) -> ForgeDiagnostic {
        ForgeDiagnostic::new(severity, msg.to_string(), 10, 5, "rustc".to_string())
    }

    // ── DiagnosticSeverity ─────────────────────────────────────────────

    #[test]
    fn test_from_lsp_known_values() {
        assert_eq!(
            DiagnosticSeverity::from_lsp(1),
            Some(DiagnosticSeverity::Error)
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp(2),
            Some(DiagnosticSeverity::Warning)
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp(3),
            Some(DiagnosticSeverity::Information)
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp(4),
            Some(DiagnosticSeverity::Hint)
        );
    }

    #[test]
    fn test_from_lsp_unknown_returns_none() {
        assert_eq!(DiagnosticSeverity::from_lsp(0), None);
        assert_eq!(DiagnosticSeverity::from_lsp(99), None);
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
        let diag = ForgeDiagnostic::new(
            DiagnosticSeverity::Error,
            "expected `;`".to_string(),
            10,
            5,
            "rustc".to_string(),
        );
        let path = PathBuf::from("src/main.rs");
        // line/col are 0-indexed internally, displayed as 1-indexed
        assert_eq!(
            diag.display_with_path(&path),
            "src/main.rs:11:6: error: [rustc] expected `;`"
        );
    }

    #[test]
    fn test_display_with_path_unknown_source() {
        let diag = ForgeDiagnostic::new(
            DiagnosticSeverity::Warning,
            "unused variable".to_string(),
            0,
            0,
            "unknown".to_string(),
        );
        let path = PathBuf::from("lib.rs");
        assert_eq!(
            diag.display_with_path(&path),
            "lib.rs:1:1: warning: [unknown] unused variable"
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
        let snap = DiagnosticsSnapshot::new(vec![(
            PathBuf::from("a.rs"),
            vec![
                make_diag(DiagnosticSeverity::Error, "e1"),
                make_diag(DiagnosticSeverity::Error, "e2"),
                make_diag(DiagnosticSeverity::Error, "e3"),
                make_diag(DiagnosticSeverity::Warning, "w1"),
                make_diag(DiagnosticSeverity::Warning, "w2"),
                make_diag(DiagnosticSeverity::Warning, "w3"),
                make_diag(DiagnosticSeverity::Warning, "w4"),
                make_diag(DiagnosticSeverity::Warning, "w5"),
                make_diag(DiagnosticSeverity::Information, "i1"),
                make_diag(DiagnosticSeverity::Information, "i2"),
                make_diag(DiagnosticSeverity::Hint, "h1"),
            ],
        )]);
        assert_eq!(snap.total_count(), 11);
        assert_eq!(snap.error_count(), 3);
        assert_eq!(snap.warning_count(), 5);
        assert_eq!(snap.info_count(), 2);
        assert_eq!(snap.hint_count(), 1);
        assert!(!snap.is_empty());
    }

    #[test]
    fn test_snapshot_status_string_format() {
        let snap = DiagnosticsSnapshot::new(vec![(
            PathBuf::from("a.rs"),
            vec![
                make_diag(DiagnosticSeverity::Error, "e1"),
                make_diag(DiagnosticSeverity::Error, "e2"),
                make_diag(DiagnosticSeverity::Warning, "w1"),
                make_diag(DiagnosticSeverity::Warning, "w2"),
                make_diag(DiagnosticSeverity::Warning, "w3"),
                make_diag(DiagnosticSeverity::Warning, "w4"),
                make_diag(DiagnosticSeverity::Warning, "w5"),
                make_diag(DiagnosticSeverity::Warning, "w6"),
                make_diag(DiagnosticSeverity::Warning, "w7"),
            ],
        )]);
        assert_eq!(snap.status_string(), "E:2 W:7");
    }

    #[test]
    fn test_snapshot_total_count_includes_info_and_hint() {
        let snap = DiagnosticsSnapshot::new(vec![(
            PathBuf::from("a.rs"),
            vec![
                make_diag(DiagnosticSeverity::Information, "i1"),
                make_diag(DiagnosticSeverity::Information, "i2"),
                make_diag(DiagnosticSeverity::Information, "i3"),
                make_diag(DiagnosticSeverity::Hint, "h1"),
                make_diag(DiagnosticSeverity::Hint, "h2"),
                make_diag(DiagnosticSeverity::Hint, "h3"),
                make_diag(DiagnosticSeverity::Hint, "h4"),
            ],
        )]);
        assert_eq!(snap.total_count(), 7);
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

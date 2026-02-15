//! Public types consumed by the engine.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ServerConfigError {
    #[error("server command must not be empty")]
    EmptyCommand,
    #[error("language_id must not be empty")]
    EmptyLanguageId,
}

#[derive(Deserialize)]
struct RawServerConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    language_id: String,
    #[serde(default)]
    file_extensions: Vec<String>,
    #[serde(default)]
    root_markers: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    servers: HashMap<String, ServerConfig>,
}

impl LspConfig {
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn servers(&self) -> &HashMap<String, ServerConfig> {
        &self.servers
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawServerConfig")]
pub struct ServerConfig {
    command: String,
    args: Vec<String>,
    language_id: String,
    file_extensions: Vec<String>,
    root_markers: Vec<String>,
}

impl TryFrom<RawServerConfig> for ServerConfig {
    type Error = ServerConfigError;

    fn try_from(raw: RawServerConfig) -> Result<Self, Self::Error> {
        if raw.command.trim().is_empty() {
            return Err(ServerConfigError::EmptyCommand);
        }
        if raw.language_id.trim().is_empty() {
            return Err(ServerConfigError::EmptyLanguageId);
        }
        Ok(Self {
            command: raw.command,
            args: raw.args,
            language_id: raw.language_id,
            file_extensions: raw.file_extensions,
            root_markers: raw.root_markers,
        })
    }
}

impl ServerConfig {
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    #[must_use]
    pub fn language_id(&self) -> &str {
        &self.language_id
    }

    #[must_use]
    pub fn file_extensions(&self) -> &[String] {
        &self.file_extensions
    }

    #[must_use]
    pub fn root_markers(&self) -> &[String] {
        &self.root_markers
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl DiagnosticSeverity {
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
    ///
    /// Path is sanitized to neutralize embedded newlines, tabs, and carriage
    /// returns that could break TUI layout or be used for UI spoofing.
    #[must_use]
    pub fn display_with_path(&self, path: &std::path::Path) -> String {
        let path_str = path.display().to_string();
        let safe_path = forge_types::sanitize_path_display(&path_str);
        format!(
            "{}:{}:{}: {}: [{}] {}",
            safe_path,
            self.line + 1,
            self.col + 1,
            self.severity.label(),
            self.source,
            self.message,
        )
    }
}

#[derive(Debug, Clone)]
pub enum ServerStopReason {
    /// Clean shutdown (EOF on stdout).
    Exited,
    /// Crashed or encountered an I/O error.
    Failed(String),
}

#[derive(Debug)]
pub enum LspEvent {
    /// Server stopped running. Manager removes it from the active map.
    ServerStopped {
        server: String,
        reason: ServerStopReason,
    },
    /// Diagnostics updated for a file.
    Diagnostics {
        path: PathBuf,
        items: Vec<ForgeDiagnostic>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    /// Per-file diagnostics, sorted with error-containing files first.
    files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>,
}

impl DiagnosticsSnapshot {
    pub(crate) fn new(files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>) -> Self {
        Self { files }
    }

    /// Per-file diagnostics, sorted with error-containing files first.
    #[must_use]
    pub fn files(&self) -> &[(PathBuf, Vec<ForgeDiagnostic>)] {
        &self.files
    }

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

    #[must_use]
    pub fn error_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Error)
    }

    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Warning)
    }

    #[must_use]
    pub fn info_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Information)
    }

    #[must_use]
    pub fn hint_count(&self) -> usize {
        self.count_by_severity(DiagnosticSeverity::Hint)
    }

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

    #[test]
    fn test_lsp_config_defaults() {
        let config: LspConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.enabled());
        assert!(config.servers().is_empty());
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
        assert!(config.enabled());
        assert_eq!(config.servers().len(), 1);
        let rust = &config.servers()["rust"];
        assert_eq!(rust.command(), "rust-analyzer");
        assert_eq!(rust.language_id(), "rust");
        assert_eq!(rust.file_extensions(), ["rs"]);
        assert_eq!(rust.root_markers(), ["Cargo.toml"]);
        assert!(rust.args().is_empty());
    }

    #[test]
    fn test_server_config_rejects_empty_command() {
        let json = serde_json::json!({
            "command": "",
            "language_id": "rust"
        });
        let result: Result<ServerConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_server_config_rejects_whitespace_command() {
        let json = serde_json::json!({
            "command": "   ",
            "language_id": "rust"
        });
        let result: Result<ServerConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_server_config_rejects_empty_language_id() {
        let json = serde_json::json!({
            "command": "rust-analyzer",
            "language_id": ""
        });
        let result: Result<ServerConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}

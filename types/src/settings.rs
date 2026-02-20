//! Resolved configuration types shared across crates.
//!
//! These types represent fully-validated, resolved configuration state.
//! Raw TOML deserialization structs (with `Option` fields and `bool` flags)
//! stay private in `forge-config`. The config loader resolves them into
//! these types at the parse boundary.
//!
//! Existence of a value is the proof of its validity -- no `Option`, no `bool`.

use serde::Deserialize;
use std::collections::HashMap;

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

/// Validated LSP server configuration.
///
/// Invariant: `command` and `language_id` are non-empty (enforced via
/// `#[serde(try_from)]` at the deserialization boundary).
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

/// Resolved LSP configuration: a non-empty set of validated server configs.
///
/// Existence of this value proves LSP is enabled. There is no `enabled: bool`;
/// the config loader filters disabled configs at the parse boundary and only
/// constructs this type when LSP is active.
#[derive(Debug, Clone)]
pub struct LspConfig {
    servers: HashMap<String, ServerConfig>,
}

impl LspConfig {
    /// Construct from a validated server map.
    #[must_use]
    pub fn new(servers: HashMap<String, ServerConfig>) -> Self {
        Self { servers }
    }

    #[must_use]
    pub fn servers(&self) -> &HashMap<String, ServerConfig> {
        &self.servers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_valid() {
        let json = serde_json::json!({
            "command": "rust-analyzer",
            "language_id": "rust",
            "file_extensions": ["rs"],
            "root_markers": ["Cargo.toml"]
        });
        let sc: ServerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(sc.command(), "rust-analyzer");
        assert_eq!(sc.language_id(), "rust");
        assert_eq!(sc.file_extensions(), ["rs"]);
        assert_eq!(sc.root_markers(), ["Cargo.toml"]);
        assert!(sc.args().is_empty());
    }

    #[test]
    fn server_config_rejects_empty_command() {
        let json = serde_json::json!({ "command": "", "language_id": "rust" });
        assert!(serde_json::from_value::<ServerConfig>(json).is_err());
    }

    #[test]
    fn server_config_rejects_whitespace_command() {
        let json = serde_json::json!({ "command": "   ", "language_id": "rust" });
        assert!(serde_json::from_value::<ServerConfig>(json).is_err());
    }

    #[test]
    fn server_config_rejects_empty_language_id() {
        let json = serde_json::json!({ "command": "rust-analyzer", "language_id": "" });
        assert!(serde_json::from_value::<ServerConfig>(json).is_err());
    }

    #[test]
    fn lsp_config_servers_accessible() {
        let sc: ServerConfig = serde_json::from_value(serde_json::json!({
            "command": "pyright",
            "language_id": "python",
            "file_extensions": ["py"]
        }))
        .unwrap();
        let mut map = HashMap::new();
        map.insert("python".to_string(), sc);
        let lsp = LspConfig::new(map);
        assert_eq!(lsp.servers().len(), 1);
        assert_eq!(lsp.servers()["python"].command(), "pyright");
    }
}

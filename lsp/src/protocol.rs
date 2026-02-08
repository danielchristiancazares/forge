//! Internal LSP message serde types for JSON-RPC communication.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{DiagnosticSeverity, ForgeDiagnostic};

#[derive(Debug, thiserror::Error)]
#[error("cannot convert path to file URI: {}", path.display())]
pub(crate) struct PathToUriError {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
pub(crate) struct Request {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Request {
    pub fn new(id: u64, method: &'static str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Notification {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Notification {
    pub fn new(method: &'static str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

pub(crate) fn initialize_params(root_uri: &str) -> serde_json::Value {
    serde_json::json!({
        "processId": std::process::id(),
        "rootUri": root_uri,
        "capabilities": {
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "willSave": false,
                    "willSaveWaitUntil": false,
                    "didSave": false
                },
                "publishDiagnostics": {
                    "relatedInformation": false
                }
            }
        },
        "workspaceFolders": [{
            "uri": root_uri,
            "name": "workspace"
        }]
    })
}

pub(crate) fn did_open_params(
    uri: &str,
    language_id: &str,
    version: i32,
    text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "textDocument": {
            "uri": uri,
            "languageId": language_id,
            "version": version,
            "text": text
        }
    })
}

pub(crate) fn did_change_params(uri: &str, version: i32, text: &str) -> serde_json::Value {
    serde_json::json!({
        "textDocument": {
            "uri": uri,
            "version": version
        },
        "contentChanges": [{
            "text": text
        }]
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<u64>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LspRange {
    pub start: LspPosition,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LspPosition {
    pub line: u32,
    pub character: u32,
}

impl LspDiagnostic {
    pub fn to_forge_diagnostic(&self) -> ForgeDiagnostic {
        ForgeDiagnostic::new(
            self.severity
                .and_then(DiagnosticSeverity::from_lsp)
                .unwrap_or(DiagnosticSeverity::Warning),
            self.message.clone(),
            self.range.start.line,
            self.range.start.character,
            self.source
                .clone()
                .unwrap_or_else(|| String::from("unknown")),
        )
    }
}

pub(crate) fn path_to_file_uri(path: &Path) -> Result<url::Url, PathToUriError> {
    url::Url::from_file_path(path).map_err(|()| PathToUriError {
        path: path.to_path_buf(),
    })
}

pub(crate) fn file_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_params_has_required_fields() {
        let params = initialize_params("file:///workspace");
        assert!(params["processId"].is_number());
        assert_eq!(params["rootUri"], "file:///workspace");
        assert!(params["capabilities"]["textDocument"]["publishDiagnostics"].is_object());
    }

    #[test]
    fn test_did_open_params() {
        let params = did_open_params("file:///test.rs", "rust", 1, "fn main() {}");
        assert_eq!(params["textDocument"]["uri"], "file:///test.rs");
        assert_eq!(params["textDocument"]["languageId"], "rust");
        assert_eq!(params["textDocument"]["version"], 1);
    }

    #[test]
    fn test_did_change_params() {
        let params = did_change_params("file:///test.rs", 2, "fn main() { 42 }");
        assert_eq!(params["textDocument"]["version"], 2);
        assert_eq!(params["contentChanges"][0]["text"], "fn main() { 42 }");
    }

    #[test]
    fn test_lsp_diagnostic_conversion() {
        let lsp_diag = LspDiagnostic {
            range: LspRange {
                start: LspPosition {
                    line: 10,
                    character: 5,
                },
            },
            severity: Some(1),
            source: Some("rustc".to_string()),
            message: "expected `;`".to_string(),
        };

        let forge_diag = lsp_diag.to_forge_diagnostic();
        assert_eq!(forge_diag.severity(), DiagnosticSeverity::Error);
        assert_eq!(forge_diag.line(), 10);
        assert_eq!(forge_diag.col(), 5);
        assert_eq!(forge_diag.source(), "rustc");
    }

    #[test]
    fn test_publish_diagnostics_deserialization() {
        let json = serde_json::json!({
            "uri": "file:///test.rs",
            "diagnostics": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } },
                "severity": 1,
                "source": "rustc",
                "message": "cannot find value `x`"
            }]
        });

        let params: PublishDiagnosticsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.uri, "file:///test.rs");
        assert_eq!(params.diagnostics.len(), 1);
        assert_eq!(params.diagnostics[0].message, "cannot find value `x`");
    }

    #[test]
    fn test_publish_diagnostics_no_severity() {
        // Severity is optional per LSP spec
        let json = serde_json::json!({
            "uri": "file:///test.rs",
            "diagnostics": [{
                "range": { "start": { "line": 5, "character": 3 }, "end": { "line": 5, "character": 10 } },
                "message": "some warning"
            }]
        });
        let params: PublishDiagnosticsParams = serde_json::from_value(json).unwrap();
        let forge_diag = params.diagnostics[0].to_forge_diagnostic();
        assert_eq!(forge_diag.severity(), DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_publish_diagnostics_empty_diagnostics() {
        // Server clears diagnostics by publishing an empty array
        let json = serde_json::json!({
            "uri": "file:///test.rs",
            "diagnostics": []
        });
        let params: PublishDiagnosticsParams = serde_json::from_value(json).unwrap();
        assert!(params.diagnostics.is_empty());
    }

    #[test]
    fn test_path_to_file_uri_and_back() {
        #[cfg(windows)]
        let path = std::path::PathBuf::from(r"C:\Users\test\src\main.rs");
        #[cfg(not(windows))]
        let path = std::path::PathBuf::from("/home/test/src/main.rs");

        let uri = path_to_file_uri(&path).expect("should create URI");
        let roundtrip = file_uri_to_path(uri.as_str()).expect("should parse back to path");
        assert_eq!(roundtrip, path);
    }

    #[test]
    fn test_file_uri_to_path_invalid_uri() {
        assert!(file_uri_to_path("not-a-uri").is_none());
    }

    #[test]
    fn test_file_uri_to_path_non_file_scheme() {
        assert!(file_uri_to_path("https://example.com/test.rs").is_none());
    }

    #[test]
    fn test_request_serialization_with_params() {
        let req = Request::new(
            42,
            "initialize",
            Some(serde_json::json!({"rootUri": "file:///"})),
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["method"], "initialize");
        assert!(json["params"]["rootUri"].is_string());
    }

    #[test]
    fn test_request_serialization_without_params() {
        let req = Request::new(1, "shutdown", None);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "shutdown");
        assert!(
            json.get("params").is_none(),
            "params must be omitted, not null"
        );
    }

    #[test]
    fn test_notification_serialization_with_params() {
        let notif = Notification::new("initialized", Some(serde_json::json!({})));
        let json = serde_json::to_value(&notif).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "initialized");
        assert!(json.get("id").is_none());
        assert!(json.get("params").is_some());
    }

    #[test]
    fn test_notification_serialization_without_params() {
        let notif = Notification::new("exit", None);
        let json = serde_json::to_value(&notif).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "exit");
        assert!(
            json.get("params").is_none(),
            "params must be omitted, not null"
        );
    }
}

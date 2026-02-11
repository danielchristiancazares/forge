//! Server handle — owns a child process and manages the LSP lifecycle.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use crate::codec::{FrameReader, FrameWriter};
use crate::protocol::{self, Notification, PublishDiagnosticsParams, Request};
use crate::types::{LspEvent, ServerConfig, ServerStopReason};

const INIT_TIMEOUT_SECS: u64 = 30;

const SHUTDOWN_TIMEOUT_SECS: u64 = 2;

const WRITER_CHANNEL_CAPACITY: usize = 64;

enum WriterCommand {
    Send(serde_json::Value),
    Shutdown,
}

enum IncomingFrame {
    Response {
        id: u64,
        body: serde_json::Value,
    },
    ServerRequest {
        id: serde_json::Value,
        method: String,
    },
    Notification {
        method: String,
        params: Option<serde_json::Value>,
    },
}

/// Minimal glob matcher for env var denylist patterns.
/// Handles `*_SUFFIX`, `PREFIX_*`, `*_INFIX*`, and exact match.
/// Both pattern and key are compared in uppercase.
fn env_glob_matches(pattern: &str, key_upper: &str) -> bool {
    let pat = pattern.to_uppercase();
    match (pat.starts_with('*'), pat.ends_with('*')) {
        (true, true) => {
            let inner = &pat[1..pat.len() - 1];
            key_upper.contains(inner)
        }
        (true, false) => {
            let suffix = &pat[1..];
            key_upper.ends_with(suffix)
        }
        (false, true) => {
            let prefix = &pat[..pat.len() - 1];
            key_upper.starts_with(prefix)
        }
        (false, false) => key_upper == pat,
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = Vec::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.iter().collect()
}

fn parse_incoming(frame: &serde_json::Value) -> Option<IncomingFrame> {
    let id = frame.get("id");
    let method = frame
        .get("method")
        .and_then(|m| m.as_str())
        .map(String::from);
    let has_result_or_error = frame.get("result").is_some() || frame.get("error").is_some();

    match (id, method, has_result_or_error) {
        (Some(id_val), None, true) => Some(IncomingFrame::Response {
            id: id_val.as_u64()?,
            body: frame.clone(),
        }),
        (Some(id_val), Some(method), _) => Some(IncomingFrame::ServerRequest {
            id: id_val.clone(),
            method,
        }),
        (None, Some(method), _) => Some(IncomingFrame::Notification {
            method,
            params: frame.get("params").cloned(),
        }),
        _ => None,
    }
}

pub(crate) struct RunningServer {
    name: String,
    language_id: String,
    child: Child,
    writer_tx: mpsc::Sender<WriterCommand>,
    next_id: u64,
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    /// URIs of documents we've sent didOpen for (to distinguish didOpen vs didChange).
    opened_docs: HashSet<String>,
    /// Per-document version counter for didChange.
    doc_versions: HashMap<String, i32>,
    #[allow(dead_code)]
    reader_handle: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    writer_handle: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    pub async fn start(
        name: String,
        config: &ServerConfig,
        workspace_root: &Path,
        event_tx: mpsc::Sender<LspEvent>,
    ) -> Result<Self> {
        let resolved_cmd = which::which(config.command())
            .with_context(|| format!("{} not found in PATH", config.command()))?;
        let mut cmd = Command::new(&resolved_cmd);
        cmd.args(config.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        // Strip secret-bearing env vars using the canonical denylist from forge-types.
        for (key, _) in std::env::vars() {
            let upper = key.to_uppercase();
            if forge_types::ENV_SECRET_DENYLIST
                .iter()
                .any(|pat| env_glob_matches(pat, &upper))
            {
                cmd.env_remove(&key);
            }
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning {}", config.command()))?;

        let stdout = child.stdout.take().context("no stdout from child")?;
        let stdin = child.stdin.take().context("no stdin from child")?;

        let pending: std::sync::Arc<
            tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
        > = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let (writer_tx, mut writer_rx) = mpsc::channel::<WriterCommand>(WRITER_CHANNEL_CAPACITY);
        let writer_handle = tokio::spawn(async move {
            let mut writer = FrameWriter::new(stdin);
            while let Some(cmd) = writer_rx.recv().await {
                match cmd {
                    WriterCommand::Send(frame) => {
                        if let Err(e) = writer.write_frame(&frame).await {
                            tracing::warn!("LSP write error: {e}");
                            break;
                        }
                    }
                    WriterCommand::Shutdown => break,
                }
            }
        });

        let reader_pending = pending.clone();
        let reader_event_tx = event_tx.clone();
        let reader_writer_tx = writer_tx.clone();
        let reader_name = name.clone();
        let reader_workspace_root = normalize_path(workspace_root);
        let reader_handle = tokio::spawn(async move {
            let mut reader = FrameReader::new(stdout);
            loop {
                match reader.read_frame().await {
                    Ok(Some(frame)) => {
                        Self::dispatch_frame(
                            &frame,
                            &reader_pending,
                            &reader_event_tx,
                            &reader_writer_tx,
                            &reader_name,
                            &reader_workspace_root,
                        )
                        .await;
                    }
                    Ok(None) => {
                        tracing::info!("LSP server '{}' closed stdout", reader_name);
                        let _ = reader_event_tx
                            .send(LspEvent::ServerStopped {
                                server: reader_name.clone(),
                                reason: ServerStopReason::Exited,
                            })
                            .await;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("LSP reader error for '{}': {e}", reader_name);
                        let _ = reader_event_tx
                            .send(LspEvent::ServerStopped {
                                server: reader_name.clone(),
                                reason: ServerStopReason::Failed(e.to_string()),
                            })
                            .await;
                        break;
                    }
                }
            }
        });

        let mut handle = Self {
            name,
            language_id: config.language_id().to_string(),
            child,
            writer_tx,
            next_id: 1,
            pending,
            opened_docs: HashSet::new(),
            doc_versions: HashMap::new(),
            reader_handle,
            writer_handle,
        };

        handle.initialize(workspace_root).await?;

        Ok(handle)
    }

    async fn dispatch_frame(
        frame: &serde_json::Value,
        pending: &tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
        event_tx: &mpsc::Sender<LspEvent>,
        writer_tx: &mpsc::Sender<WriterCommand>,
        server_name: &str,
        workspace_root: &Path,
    ) {
        let Some(incoming) = parse_incoming(frame) else {
            tracing::trace!("Ignoring malformed JSON-RPC frame from '{server_name}'");
            return;
        };

        match incoming {
            IncomingFrame::Response { id, body } => {
                let sender = pending.lock().await.remove(&id);
                if let Some(tx) = sender {
                    let _ = tx.send(body);
                }
            }
            IncomingFrame::ServerRequest { id, method } => {
                // Many servers send client/registerCapability, workspace/configuration, etc.
                // We must respond or the server may block.
                tracing::debug!(
                    "LSP '{server_name}' sent request: {method} — replying method not found"
                );
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {method}")
                    }
                });
                let _ = writer_tx.send(WriterCommand::Send(response)).await;
            }
            IncomingFrame::Notification { method, params } => {
                Self::handle_notification(server_name, &method, params, event_tx, workspace_root)
                    .await;
            }
        }
    }

    async fn handle_notification(
        server_name: &str,
        method: &str,
        params: Option<serde_json::Value>,
        event_tx: &mpsc::Sender<LspEvent>,
        workspace_root: &Path,
    ) {
        match method {
            "textDocument/publishDiagnostics" => {
                let Some(params) = params else { return };
                match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                    Ok(diag_params) => {
                        if let Some(path) = protocol::file_uri_to_path(&diag_params.uri) {
                            let normalized = normalize_path(&path);
                            if !normalized.starts_with(workspace_root) {
                                tracing::warn!(
                                    "LSP '{server_name}' reported diagnostics for path outside \
                                     workspace: {}",
                                    path.display()
                                );
                                return;
                            }
                            let items = diag_params
                                .diagnostics
                                .iter()
                                .map(protocol::LspDiagnostic::to_forge_diagnostic)
                                .collect();
                            let _ = event_tx.send(LspEvent::Diagnostics { path, items }).await;
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Failed to parse publishDiagnostics from '{server_name}': {e}"
                        );
                    }
                }
            }
            _ => {
                tracing::trace!("Ignoring notification from '{server_name}': {method}");
            }
        }
    }

    async fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let root_uri = protocol::path_to_file_uri(workspace_root)
            .context("converting workspace root to URI")?;

        let params = protocol::initialize_params(root_uri.as_str());
        let response = self.send_request("initialize", Some(params)).await?;

        if let Some(error) = response.get("error") {
            bail!(
                "LSP initialize failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            );
        }

        self.send_notification("initialized", Some(serde_json::json!({})))
            .await?;

        Ok(())
    }

    async fn send_request(
        &mut self,
        method: &'static str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let request = Request::new(id, method, params);
        let frame = serde_json::to_value(&request).context("serializing request")?;
        if self
            .writer_tx
            .send(WriterCommand::Send(frame))
            .await
            .is_err()
        {
            // If we fail to enqueue the request for writing, make sure we don't leak the
            // pending request entry.
            self.pending.lock().await.remove(&id);
            bail!("writer channel closed");
        }

        let response =
            match tokio::time::timeout(std::time::Duration::from_secs(INIT_TIMEOUT_SECS), rx).await
            {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => {
                    // Reader task dropped / server exited; avoid leaking pending entry.
                    self.pending.lock().await.remove(&id);
                    bail!("response channel dropped");
                }
                Err(_) => {
                    // Timeout: remove the pending entry so repeated failures don't grow the map.
                    self.pending.lock().await.remove(&id);
                    bail!("request timed out");
                }
            };

        Ok(response)
    }

    async fn send_notification(
        &self,
        method: &'static str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let notification = Notification::new(method, params);
        let frame = serde_json::to_value(&notification).context("serializing notification")?;
        self.writer_tx
            .send(WriterCommand::Send(frame))
            .await
            .map_err(|_| anyhow::anyhow!("writer channel closed"))?;
        Ok(())
    }

    /// Tracks per-document versions with monotonically increasing counters.
    ///
    /// No state guard — holding a `RunningServer` is proof of successful init.
    /// Channel errors are returned as `Err` (honest I/O failure).
    pub async fn notify_file_changed(&mut self, uri: &str, text: &str) -> Result<()> {
        if self.opened_docs.contains(uri) {
            let version = self.doc_versions.entry(uri.to_string()).or_insert(0);
            *version += 1;
            let params = protocol::did_change_params(uri, *version, text);
            self.send_notification("textDocument/didChange", Some(params))
                .await
        } else {
            let version = 1;
            self.doc_versions.insert(uri.to_string(), version);
            self.opened_docs.insert(uri.to_string());
            let params = protocol::did_open_params(uri, &self.language_id, version, text);
            self.send_notification("textDocument/didOpen", Some(params))
                .await
        }
    }

    /// Gracefully shut down the server. Consumes self.
    pub async fn shutdown(mut self) {
        if let Ok(response) = self.send_request("shutdown", None).await
            && response.get("error").is_none()
        {
            let _ = self.send_notification("exit", None).await;
        }

        let _ = self.writer_tx.send(WriterCommand::Shutdown).await;

        let kill_result = tokio::time::timeout(
            std::time::Duration::from_secs(SHUTDOWN_TIMEOUT_SECS),
            self.child.wait(),
        )
        .await;

        if kill_result.is_err() {
            tracing::debug!("LSP '{}' didn't exit in time, killing", self.name);
            let _ = self.child.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type PendingMap =
        std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

    fn test_channels() -> (
        PendingMap,
        mpsc::Sender<LspEvent>,
        mpsc::Receiver<LspEvent>,
        mpsc::Sender<WriterCommand>,
        mpsc::Receiver<WriterCommand>,
    ) {
        let pending: PendingMap = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let (event_tx, event_rx) = mpsc::channel(32);
        let (writer_tx, writer_rx) = mpsc::channel(32);
        (pending, event_tx, event_rx, writer_tx, writer_rx)
    }

    #[cfg(windows)]
    fn test_workspace_root() -> PathBuf {
        PathBuf::from(r"C:\test")
    }

    #[cfg(not(windows))]
    fn test_workspace_root() -> PathBuf {
        PathBuf::from("/test")
    }

    #[tokio::test]
    async fn test_dispatch_response_routes_to_pending() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(1, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "capabilities": {} }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        let response = rx.await.unwrap();
        assert!(response["result"]["capabilities"].is_object());
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_notification_publishes_diagnostics() {
        let (pending, event_tx, mut event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        #[cfg(windows)]
        let uri = "file:///C:/test/main.rs";
        #[cfg(not(windows))]
        let uri = "file:///test/main.rs";

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": [{
                    "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 5, "character": 10 } },
                    "severity": 1,
                    "source": "rustc",
                    "message": "expected `;`"
                }]
            }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        let event = event_rx.try_recv().unwrap();
        match event {
            LspEvent::Diagnostics { path, items } => {
                #[cfg(windows)]
                assert_eq!(path, std::path::PathBuf::from(r"C:\test\main.rs"));
                #[cfg(not(windows))]
                assert_eq!(path, std::path::PathBuf::from("/test/main.rs"));
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].message(), "expected `;`");
                assert!(items[0].severity().is_error());
            }
            other @ LspEvent::ServerStopped { .. } => {
                panic!("expected Diagnostics event, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn test_dispatch_rejects_diagnostics_outside_workspace() {
        let (pending, event_tx, mut event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        #[cfg(windows)]
        let uri = "file:///C:/etc/passwd";
        #[cfg(not(windows))]
        let uri = "file:///etc/passwd";

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                    "severity": 1,
                    "source": "evil",
                    "message": "gotcha"
                }]
            }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        assert!(
            event_rx.try_recv().is_err(),
            "diagnostics outside workspace must be rejected"
        );
    }

    #[tokio::test]
    async fn test_dispatch_rejects_diagnostics_with_path_traversal() {
        let (pending, event_tx, mut event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        #[cfg(windows)]
        let uri = "file:///C:/test/../etc/passwd";
        #[cfg(not(windows))]
        let uri = "file:///test/../etc/passwd";

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                    "severity": 1,
                    "source": "evil",
                    "message": "traversal"
                }]
            }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        assert!(
            event_rx.try_recv().is_err(),
            "path traversal diagnostics must be rejected"
        );
    }

    #[tokio::test]
    async fn test_dispatch_server_request_sends_method_not_found() {
        let (pending, event_tx, _event_rx, writer_tx, mut writer_rx) = test_channels();
        let root = test_workspace_root();

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "client/registerCapability",
            "params": {}
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        let cmd = writer_rx.try_recv().unwrap();
        match cmd {
            WriterCommand::Send(response) => {
                assert_eq!(response["id"], 5);
                assert_eq!(response["error"]["code"], -32601);
                let msg = response["error"]["message"].as_str().unwrap();
                assert!(msg.contains("client/registerCapability"));
            }
            WriterCommand::Shutdown => panic!("expected Send, got Shutdown"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_unknown_notification_ignored() {
        let (pending, event_tx, mut event_rx, writer_tx, mut writer_rx) = test_channels();
        let root = test_workspace_root();

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": 3, "message": "hello" }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        assert!(event_rx.try_recv().is_err());
        assert!(writer_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_dispatch_response_with_error_routes_to_pending() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(2, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": { "code": -32600, "message": "invalid request" }
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;

        let response = rx.await.unwrap();
        assert!(response["error"].is_object());
    }

    #[tokio::test]
    async fn test_dispatch_response_for_unknown_id_ignored() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();
        let root = test_workspace_root();

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 999,
            "result": {}
        });

        RunningServer::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test", &root).await;
    }

    #[test]
    fn env_glob_suffix_matches() {
        assert!(env_glob_matches("*_KEY", "API_KEY"));
        assert!(env_glob_matches("*_KEY", "MY_SECRET_KEY"));
        assert!(!env_glob_matches("*_KEY", "KEYRING"));
    }

    #[test]
    fn env_glob_prefix_matches() {
        assert!(env_glob_matches("AWS_*", "AWS_ACCESS_KEY_ID"));
        assert!(env_glob_matches("AWS_*", "AWS_SESSION_TOKEN"));
        assert!(!env_glob_matches("AWS_*", "MY_AWS"));
    }

    #[test]
    fn env_glob_infix_matches() {
        assert!(env_glob_matches("*_CREDENTIAL*", "DB_CREDENTIAL_FILE"));
        assert!(env_glob_matches("*_CREDENTIAL*", "MY_CREDENTIALS"));
        assert!(!env_glob_matches("*_CREDENTIAL*", "CREDENTIAL"));
    }

    #[test]
    fn env_glob_case_insensitive() {
        // The caller uppercases the key before passing it in, so test that path
        assert!(env_glob_matches("*_KEY", &"api_key".to_uppercase()));
        assert!(env_glob_matches("AWS_*", &"aws_secret".to_uppercase()));
    }
}

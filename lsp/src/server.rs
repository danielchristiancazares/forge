//! Server handle — owns a child process and manages the LSP lifecycle.
//!
//! Each [`ServerHandle`] manages one language server process (e.g. rust-analyzer).
//! It spawns the process, performs the LSP initialize handshake, and runs a
//! background reader task that routes responses and dispatches notifications.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use crate::codec::{FrameReader, FrameWriter};
use crate::protocol::{self, Notification, PublishDiagnosticsParams, Request};
use crate::types::{LspEvent, LspState};

/// Timeout for the initialize handshake.
const INIT_TIMEOUT_SECS: u64 = 30;

/// Timeout for shutdown request before killing the process.
const SHUTDOWN_TIMEOUT_SECS: u64 = 2;

/// Channel capacity for the writer command channel.
const WRITER_CHANNEL_CAPACITY: usize = 64;

/// Internal command sent to the writer task.
enum WriterCommand {
    /// Send a frame to the server.
    Send(serde_json::Value),
    /// Shut down the writer task.
    Shutdown,
}

/// Manages a single language server child process.
pub(crate) struct ServerHandle {
    pub name: String,
    pub language_id: String,
    pub state: LspState,
    child: Child,
    writer_tx: mpsc::Sender<WriterCommand>,
    next_id: u64,
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    /// URIs of documents we've sent didOpen for (to distinguish didOpen vs didChange).
    opened_docs: HashSet<String>,
    /// Per-document version counter for didChange.
    doc_versions: HashMap<String, i32>,
    /// Background reader task (kept alive for the server's lifetime).
    #[allow(dead_code)]
    reader_handle: tokio::task::JoinHandle<()>,
    /// Background writer task (kept alive for the server's lifetime).
    #[allow(dead_code)]
    writer_handle: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    /// Spawn a language server and perform the LSP initialize handshake.
    pub async fn start(
        name: String,
        command: &str,
        args: &[String],
        language_id: String,
        workspace_root: &Path,
        event_tx: mpsc::Sender<LspEvent>,
    ) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {command}"))?;

        let stdout = child.stdout.take().context("no stdout from child")?;
        let stdin = child.stdin.take().context("no stdin from child")?;

        let pending: std::sync::Arc<
            tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
        > = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        // Writer task: serializes frames to the server's stdin
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

        // Reader task: routes responses by id, dispatches notifications
        let reader_pending = pending.clone();
        let reader_event_tx = event_tx.clone();
        let reader_writer_tx = writer_tx.clone();
        let reader_name = name.clone();
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
                        )
                        .await;
                    }
                    Ok(None) => {
                        // EOF — server exited
                        tracing::info!("LSP server '{}' closed stdout", reader_name);
                        let _ = reader_event_tx
                            .send(LspEvent::Status {
                                server: reader_name.clone(),
                                state: LspState::Stopped,
                            })
                            .await;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("LSP reader error for '{}': {e}", reader_name);
                        let _ = reader_event_tx
                            .send(LspEvent::Status {
                                server: reader_name.clone(),
                                state: LspState::Failed(e.to_string()),
                            })
                            .await;
                        break;
                    }
                }
            }
        });

        let mut handle = Self {
            name,
            language_id,
            state: LspState::Starting,
            child,
            writer_tx,
            next_id: 1,
            pending,
            opened_docs: HashSet::new(),
            doc_versions: HashMap::new(),
            reader_handle,
            writer_handle,
        };

        // Perform initialize handshake
        handle.initialize(workspace_root).await?;
        handle.state = LspState::Running;

        let _ = event_tx
            .send(LspEvent::Status {
                server: handle.name.clone(),
                state: LspState::Running,
            })
            .await;

        Ok(handle)
    }

    /// Dispatch a single frame from the server.
    ///
    /// Handles three cases:
    /// 1. **Response** (has `id` + `result`/`error`): route to pending request
    /// 2. **Notification** (has `method`, no `id`): dispatch to event handler
    /// 3. **Server→client request** (has `id` + `method`): auto-reply with "method not found"
    async fn dispatch_frame(
        frame: &serde_json::Value,
        pending: &tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
        event_tx: &mpsc::Sender<LspEvent>,
        writer_tx: &mpsc::Sender<WriterCommand>,
        server_name: &str,
    ) {
        let has_id = frame.get("id").is_some();
        let has_method = frame.get("method").is_some();
        let has_result_or_error = frame.get("result").is_some() || frame.get("error").is_some();

        if has_id && has_result_or_error {
            // Case 1: Response to our request
            if let Some(id) = frame["id"].as_u64() {
                let sender = pending.lock().await.remove(&id);
                if let Some(tx) = sender {
                    let _ = tx.send(frame.clone());
                }
            }
        } else if has_id && has_method {
            // Case 3: Server→client request — reply with "method not found"
            // Many servers send client/registerCapability, workspace/configuration, etc.
            // We must respond or the server may block.
            let method = frame["method"].as_str().unwrap_or("<unknown>");
            tracing::debug!(
                "LSP '{server_name}' sent request: {method} — replying method not found"
            );

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": frame["id"],
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {method}")
                }
            });
            let _ = writer_tx.send(WriterCommand::Send(response)).await;
        } else if has_method {
            // Case 2: Notification from server
            let method = frame["method"].as_str().unwrap_or("");
            match method {
                "textDocument/publishDiagnostics" => {
                    if let Some(params) = frame.get("params") {
                        match serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                            Ok(diag_params) => {
                                let path = protocol::file_uri_to_path(&diag_params.uri);
                                if let Some(path) = path {
                                    let items = diag_params
                                        .diagnostics
                                        .iter()
                                        .map(protocol::LspDiagnostic::to_forge_diagnostic)
                                        .collect();
                                    let _ =
                                        event_tx.send(LspEvent::Diagnostics { path, items }).await;
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "Failed to parse publishDiagnostics from '{server_name}': {e}"
                                );
                            }
                        }
                    }
                }
                _ => {
                    tracing::trace!("Ignoring notification from '{server_name}': {method}");
                }
            }
        }
    }

    /// Perform the LSP initialize handshake.
    async fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let root_uri = protocol::path_to_file_uri(workspace_root)
            .context("converting workspace root to URI")?;

        let params = protocol::initialize_params(root_uri.as_str());
        let response = self.send_request("initialize", params).await?;

        // Check for error in response
        if let Some(error) = response.get("error") {
            bail!(
                "LSP initialize failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            );
        }

        // Send initialized notification
        self.send_notification("initialized", serde_json::json!({}))
            .await?;

        Ok(())
    }

    /// Send a JSON-RPC request and wait for the response.
    async fn send_request(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
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

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<()> {
        let notification = Notification::new(method, params);
        let frame = serde_json::to_value(&notification).context("serializing notification")?;
        self.writer_tx
            .send(WriterCommand::Send(frame))
            .await
            .map_err(|_| anyhow::anyhow!("writer channel closed"))?;
        Ok(())
    }

    /// Notify the server that a file was opened or changed.
    ///
    /// Automatically sends `didOpen` for the first notification of a URI,
    /// and `didChange` for subsequent notifications. Tracks per-document
    /// versions with monotonically increasing counters.
    pub async fn notify_file_changed(&mut self, uri: &str, text: &str) -> Result<()> {
        if self.opened_docs.contains(uri) {
            // Already opened — send didChange with incremented version
            let version = self.doc_versions.entry(uri.to_string()).or_insert(0);
            *version += 1;
            let params = protocol::did_change_params(uri, *version, text);
            self.send_notification("textDocument/didChange", params)
                .await
        } else {
            // First time — send didOpen
            let version = 1;
            self.doc_versions.insert(uri.to_string(), version);
            self.opened_docs.insert(uri.to_string());
            let params = protocol::did_open_params(uri, &self.language_id, version, text);
            self.send_notification("textDocument/didOpen", params).await
        }
    }

    /// Gracefully shut down the server.
    pub async fn shutdown(&mut self) {
        // Try graceful shutdown
        if let Ok(response) = self.send_request("shutdown", serde_json::json!(null)).await
            && response.get("error").is_none()
        {
            let _ = self
                .send_notification("exit", serde_json::json!(null))
                .await;
        }

        // Send shutdown to writer task
        let _ = self.writer_tx.send(WriterCommand::Shutdown).await;

        // Wait briefly for process to exit, then kill
        let kill_result = tokio::time::timeout(
            std::time::Duration::from_secs(SHUTDOWN_TIMEOUT_SECS),
            self.child.wait(),
        )
        .await;

        if kill_result.is_err() {
            tracing::debug!("LSP '{}' didn't exit in time, killing", self.name);
            let _ = self.child.kill().await;
        }

        self.state = LspState::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type PendingMap =
        std::sync::Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

    /// Helper: create channels for testing dispatch_frame.
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

    #[tokio::test]
    async fn test_dispatch_response_routes_to_pending() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();

        // Register a pending request with id=1
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(1, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "capabilities": {} }
        });

        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;

        let response = rx.await.unwrap();
        assert!(response["result"]["capabilities"].is_object());
        // Pending map should be empty
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_notification_publishes_diagnostics() {
        let (pending, event_tx, mut event_rx, writer_tx, _writer_rx) = test_channels();

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

        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;

        let event = event_rx.try_recv().unwrap();
        match event {
            LspEvent::Diagnostics { path, items } => {
                #[cfg(windows)]
                assert_eq!(path, std::path::PathBuf::from(r"C:\test\main.rs"));
                #[cfg(not(windows))]
                assert_eq!(path, std::path::PathBuf::from("/test/main.rs"));
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].message, "expected `;`");
                assert!(items[0].severity.is_error());
            }
            other @ LspEvent::Status { .. } => {
                panic!("expected Diagnostics event, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn test_dispatch_server_request_sends_method_not_found() {
        let (pending, event_tx, _event_rx, writer_tx, mut writer_rx) = test_channels();

        // Server→client request (has both id and method)
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "client/registerCapability",
            "params": {}
        });

        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;

        // Should have sent a "method not found" error response
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

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": 3, "message": "hello" }
        });

        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;

        // No events should be emitted
        assert!(event_rx.try_recv().is_err());
        // No writes should occur
        assert!(writer_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_dispatch_response_with_error_routes_to_pending() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();

        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(2, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": { "code": -32600, "message": "invalid request" }
        });

        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;

        let response = rx.await.unwrap();
        assert!(response["error"].is_object());
    }

    #[tokio::test]
    async fn test_dispatch_response_for_unknown_id_ignored() {
        let (pending, event_tx, _event_rx, writer_tx, _writer_rx) = test_channels();

        // No pending request for id=999
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 999,
            "result": {}
        });

        // Should not panic
        ServerHandle::dispatch_frame(&frame, &pending, &event_tx, &writer_tx, "test").await;
    }
}

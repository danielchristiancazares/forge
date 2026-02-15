//! LspManager facade — public API consumed by the engine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::diagnostics::DiagnosticsStore;
use crate::protocol;
use crate::server::RunningServer;
use crate::types::{DiagnosticsSnapshot, ForgeDiagnostic, LspConfig, LspEvent, ServerStopReason};

const EVENT_CHANNEL_CAPACITY: usize = 256;

fn build_extension_map(config: &LspConfig) -> HashMap<String, String> {
    let mut extension_map = HashMap::new();
    let mut server_names: Vec<&String> = config.servers().keys().collect();
    server_names.sort();
    for name in server_names {
        let server_config = &config.servers()[name];
        for ext in server_config.file_extensions() {
            if let Some(existing) = extension_map.get(ext) {
                tracing::warn!(
                    "Multiple LSP servers configured for extension '{ext}': '{existing}' and '{name}'. Using '{existing}'."
                );
                continue;
            }
            extension_map.insert(ext.clone(), name.clone());
        }
    }
    extension_map
}

pub struct LspManager {
    servers: HashMap<String, RunningServer>,
    diagnostics: DiagnosticsStore,
    event_rx: mpsc::Receiver<LspEvent>,
    #[cfg(test)]
    event_tx: mpsc::Sender<LspEvent>,
    extension_map: HashMap<String, String>,
}

impl LspManager {
    pub async fn start(config: LspConfig, workspace_root: &Path) -> Self {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let extension_map = build_extension_map(&config);
        let mut servers = HashMap::new();

        for (name, server_config) in config.servers() {
            tracing::info!(
                "Starting LSP server '{name}' ({})...",
                server_config.command()
            );
            match RunningServer::start(
                name.clone(),
                server_config,
                workspace_root,
                event_tx.clone(),
            )
            .await
            {
                Ok(handle) => {
                    tracing::info!("LSP server '{name}' started successfully");
                    servers.insert(name.clone(), handle);
                }
                Err(e) => {
                    tracing::warn!("Failed to start LSP server '{name}': {e:#}");
                }
            }
        }

        Self {
            servers,
            diagnostics: DiagnosticsStore::new(),
            event_rx,
            #[cfg(test)]
            event_tx,
            extension_map,
        }
    }

    pub async fn on_file_changed(&mut self, path: &Path, text: &str) {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_string(),
            None => return,
        };

        let server_name = match self.extension_map.get(&ext) {
            Some(name) => name.clone(),
            None => return,
        };

        let server = match self.servers.get_mut(&server_name) {
            Some(s) => s,
            None => return,
        };

        let uri = match protocol::path_to_file_uri(path) {
            Ok(u) => u.to_string(),
            Err(e) => {
                tracing::warn!("Skipping LSP notification: {e}");
                return;
            }
        };

        if let Err(e) = server.notify_file_changed(&uri, text).await {
            tracing::warn!(
                "Failed to notify LSP server '{server_name}' about {}: {e}",
                path.display()
            );
        }
    }

    pub fn poll_events(&mut self, budget: usize) -> usize {
        let mut count = 0;
        while count < budget {
            match self.event_rx.try_recv() {
                Ok(event) => {
                    self.handle_event(event);
                    count += 1;
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
        count
    }

    fn handle_event(&mut self, event: LspEvent) {
        match event {
            LspEvent::ServerStopped { server, reason } => {
                match reason {
                    ServerStopReason::Exited => {
                        tracing::info!(server = %server, "LSP server exited");
                    }
                    ServerStopReason::Failed(msg) => {
                        tracing::warn!(server = %server, error = %msg, "LSP server failed");
                    }
                }
                let exts: Vec<&str> = self
                    .extension_map
                    .iter()
                    .filter(|(_, srv)| *srv == &server)
                    .map(|(ext, _)| ext.as_str())
                    .collect();
                self.diagnostics.remove_for_extensions(&exts);
                self.servers.remove(&server);
            }
            LspEvent::Diagnostics { path, items } => {
                tracing::debug!(
                    path = %path.display(),
                    count = items.len(),
                    "Diagnostics updated"
                );
                self.diagnostics.update(path, items);
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        self.diagnostics.snapshot()
    }

    #[must_use]
    pub fn errors_for_files(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Vec<ForgeDiagnostic>)> {
        self.diagnostics.errors_for_files(paths)
    }

    #[must_use]
    pub fn has_running_servers(&self) -> bool {
        !self.servers.is_empty()
    }

    pub async fn shutdown(&mut self) {
        let servers = std::mem::take(&mut self.servers);
        for (name, server) in servers {
            tracing::info!("Shutting down LSP server '{name}'...");
            server.shutdown().await;
        }
    }

    #[cfg(test)]
    pub(crate) fn event_tx(&self) -> &mpsc::Sender<LspEvent> {
        &self.event_tx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiagnosticSeverity;

    fn test_config() -> LspConfig {
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "servers": {
                "rust": {
                    "command": "rust-analyzer",
                    "language_id": "rust",
                    "file_extensions": ["rs"],
                    "root_markers": ["Cargo.toml"]
                },
                "python": {
                    "command": "pyright",
                    "language_id": "python",
                    "file_extensions": ["py", "pyi"],
                    "root_markers": ["pyproject.toml"]
                }
            }
        }))
        .unwrap()
    }

    ///
    /// Uses the event channel for testing event-driven behaviour.
    fn test_manager(config: LspConfig) -> LspManager {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let extension_map = build_extension_map(&config);
        LspManager {
            servers: HashMap::new(),
            diagnostics: DiagnosticsStore::new(),
            event_rx,
            event_tx,
            extension_map,
        }
    }

    fn make_diag(severity: DiagnosticSeverity, msg: &str) -> ForgeDiagnostic {
        ForgeDiagnostic::new(severity, msg.to_string(), 0, 0, "test".to_string())
    }

    #[test]
    fn test_extension_map_built_correctly() {
        let manager = test_manager(test_config());
        assert_eq!(manager.extension_map.get("rs"), Some(&"rust".to_string()));
        assert_eq!(manager.extension_map.get("py"), Some(&"python".to_string()));
        assert_eq!(
            manager.extension_map.get("pyi"),
            Some(&"python".to_string())
        );
        assert!(!manager.extension_map.contains_key("js"));
    }

    #[test]
    fn test_extension_overlap_is_deterministic() {
        let config: LspConfig = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "servers": {
                "b": { "command": "b-ls", "language_id": "b", "file_extensions": ["rs"] },
                "a": { "command": "a-ls", "language_id": "a", "file_extensions": ["rs"] }
            }
        }))
        .unwrap();
        let manager = test_manager(config);
        assert_eq!(manager.extension_map.get("rs"), Some(&"a".to_string()));
    }

    #[test]
    fn test_has_running_servers_initially_false() {
        let manager = test_manager(test_config());
        assert!(!manager.has_running_servers());
    }

    #[test]
    fn test_snapshot_initially_empty() {
        let manager = test_manager(test_config());
        assert!(manager.snapshot().is_empty());
    }

    #[tokio::test]
    async fn test_poll_events_drains_diagnostics() {
        let mut manager = test_manager(test_config());
        let event_tx = manager.event_tx().clone();

        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("src/main.rs"),
                items: vec![make_diag(DiagnosticSeverity::Error, "expected `;`")],
            })
            .await
            .unwrap();

        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("src/lib.rs"),
                items: vec![make_diag(DiagnosticSeverity::Warning, "unused var")],
            })
            .await
            .unwrap();

        let count = manager.poll_events(10);
        assert_eq!(count, 2);

        let snap = manager.snapshot();
        assert_eq!(snap.error_count(), 1);
        assert_eq!(snap.warning_count(), 1);
        assert_eq!(snap.files().len(), 2);
    }

    #[tokio::test]
    async fn test_poll_events_respects_budget() {
        let mut manager = test_manager(test_config());
        let event_tx = manager.event_tx().clone();

        for i in 0..5 {
            event_tx
                .send(LspEvent::Diagnostics {
                    path: PathBuf::from(format!("file{i}.rs")),
                    items: vec![make_diag(DiagnosticSeverity::Error, "err")],
                })
                .await
                .unwrap();
        }

        let count = manager.poll_events(3);
        assert_eq!(count, 3);

        let count = manager.poll_events(10);
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_poll_events_empty_channel() {
        let mut manager = test_manager(test_config());
        let count = manager.poll_events(10);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_errors_for_files_via_events() {
        let mut manager = test_manager(test_config());
        let event_tx = manager.event_tx().clone();

        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("a.rs"),
                items: vec![
                    make_diag(DiagnosticSeverity::Error, "err"),
                    make_diag(DiagnosticSeverity::Warning, "warn"),
                ],
            })
            .await
            .unwrap();

        manager.poll_events(10);

        let errors = manager.errors_for_files(&[PathBuf::from("a.rs")]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].1.len(), 1);
    }

    #[tokio::test]
    async fn test_on_file_changed_skips_unknown_extension() {
        let mut manager = test_manager(test_config());
        manager
            .on_file_changed(Path::new("/test/file.js"), "code")
            .await;
    }

    #[tokio::test]
    async fn test_on_file_changed_skips_no_extension() {
        let mut manager = test_manager(test_config());
        manager
            .on_file_changed(Path::new("/test/Makefile"), "all:")
            .await;
    }

    #[tokio::test]
    async fn test_diagnostics_cleared_when_server_publishes_empty() {
        let mut manager = test_manager(test_config());
        let event_tx = manager.event_tx().clone();

        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("a.rs"),
                items: vec![make_diag(DiagnosticSeverity::Error, "err")],
            })
            .await
            .unwrap();
        manager.poll_events(10);
        assert_eq!(manager.snapshot().error_count(), 1);

        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("a.rs"),
                items: vec![],
            })
            .await
            .unwrap();
        manager.poll_events(10);
        assert!(manager.snapshot().is_empty());
    }

    #[tokio::test]
    async fn test_server_stopped_removes_from_map() {
        let mut manager = test_manager(test_config());
        let event_tx = manager.event_tx().clone();

        // Simulate a server dying
        event_tx
            .send(LspEvent::ServerStopped {
                server: "rust".to_string(),
                reason: ServerStopReason::Failed("crash".to_string()),
            })
            .await
            .unwrap();
        manager.poll_events(10);
        assert!(!manager.has_running_servers());
    }
}

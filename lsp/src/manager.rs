//! LspManager facade — public API consumed by the engine.
//!
//! The engine interacts with language servers through this single type.
//! It handles server lifecycle, file routing by extension, and diagnostics
//! aggregation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::diagnostics::DiagnosticsStore;
use crate::protocol;
use crate::server::ServerHandle;
use crate::types::{DiagnosticsSnapshot, ForgeDiagnostic, LspConfig, LspEvent, LspState};

/// Channel capacity for the event channel between server tasks and the manager.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Public facade for the LSP client subsystem.
pub struct LspManager {
    config: LspConfig,
    servers: HashMap<String, ServerHandle>,
    diagnostics: DiagnosticsStore,
    event_rx: mpsc::Receiver<LspEvent>,
    event_tx: mpsc::Sender<LspEvent>,
    /// Maps file extension (e.g. "rs") → server name (e.g. "rust").
    extension_map: HashMap<String, String>,
    /// Whether servers have been started.
    started: bool,
}

impl LspManager {
    /// Create a new manager from config. Does not start any servers yet.
    #[must_use]
    pub fn new(config: LspConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

        // Build extension → server name map
        let mut extension_map = HashMap::new();
        let mut server_names: Vec<&String> = config.servers.keys().collect();
        server_names.sort();
        for name in server_names {
            let server_config = &config.servers[name];
            for ext in &server_config.file_extensions {
                if let Some(existing) = extension_map.get(ext) {
                    tracing::warn!(
                        "Multiple LSP servers configured for extension '{ext}': '{existing}' and '{name}'. Using '{existing}'."
                    );
                    continue;
                }
                extension_map.insert(ext.clone(), name.clone());
            }
        }

        Self {
            config,
            servers: HashMap::new(),
            diagnostics: DiagnosticsStore::new(),
            event_rx,
            event_tx,
            extension_map,
            started: false,
        }
    }

    /// Spawn all configured servers for the given workspace root.
    ///
    /// Servers that fail to start are logged and skipped — a bad server
    /// config should not prevent the rest from working.
    pub async fn ensure_started(&mut self, workspace_root: &Path) {
        if self.started {
            return;
        }
        self.started = true;

        for (name, server_config) in &self.config.servers {
            tracing::info!(
                "Starting LSP server '{name}' ({})...",
                server_config.command
            );
            match ServerHandle::start(
                name.clone(),
                &server_config.command,
                &server_config.args,
                server_config.language_id.clone(),
                workspace_root,
                self.event_tx.clone(),
            )
            .await
            {
                Ok(handle) => {
                    tracing::info!("LSP server '{name}' started successfully");
                    self.servers.insert(name.clone(), handle);
                }
                Err(e) => {
                    tracing::warn!("Failed to start LSP server '{name}': {e:#}");
                    // Send a failed status event so the UI can show a hint
                    let _ = self
                        .event_tx
                        .send(LspEvent::Status {
                            server: name.clone(),
                            state: LspState::Failed(format!("{e:#}")),
                        })
                        .await;
                }
            }
        }
    }

    /// Notify that a file was created or modified.
    ///
    /// Routes to the appropriate server based on file extension.
    /// Skips files that don't match any configured server.
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

        // Skip if server isn't running
        if server.state != LspState::Running {
            return;
        }

        let uri = if let Some(u) = protocol::path_to_file_uri(path) {
            u.to_string()
        } else {
            tracing::warn!("Failed to convert path to file URI: {}", path.display());
            return;
        };

        if let Err(e) = server.notify_file_changed(&uri, text).await {
            tracing::warn!(
                "Failed to notify LSP server '{server_name}' about {}: {e}",
                path.display()
            );
        }
    }

    /// Drain pending events from server tasks, up to `budget`.
    ///
    /// This is non-blocking — returns immediately if no events are available.
    /// Diagnostics are accumulated in the store; status changes update server state.
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

    /// Handle a single LSP event.
    fn handle_event(&mut self, event: LspEvent) {
        match event {
            LspEvent::Status { server, state } => {
                tracing::info!(server = %server, state = ?state, "LSP server status changed");
                if let Some(handle) = self.servers.get_mut(&server) {
                    handle.state = state;
                }
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

    /// Get an immutable snapshot of all diagnostics.
    #[must_use]
    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        self.diagnostics.snapshot()
    }

    /// Get only errors for specific files (for agent feedback).
    #[must_use]
    pub fn errors_for_files(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Vec<ForgeDiagnostic>)> {
        self.diagnostics.errors_for_files(paths)
    }

    /// Whether the LSP subsystem is enabled and has at least one running server.
    #[must_use]
    pub fn has_running_servers(&self) -> bool {
        self.servers.values().any(|s| s.state == LspState::Running)
    }

    /// Whether server startup has been attempted (via `ensure_started`).
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started
    }

    /// Gracefully shut down all servers.
    pub async fn shutdown(&mut self) {
        for (name, server) in &mut self.servers {
            tracing::info!("Shutting down LSP server '{name}'...");
            server.shutdown().await;
        }
        self.servers.clear();
    }

    /// Get a reference to the event sender (for testing).
    #[cfg(test)]
    pub(crate) fn event_tx(&self) -> &mpsc::Sender<LspEvent> {
        &self.event_tx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DiagnosticSeverity, ServerConfig};

    fn test_config() -> LspConfig {
        let mut servers = HashMap::new();
        servers.insert(
            "rust".to_string(),
            ServerConfig {
                command: "rust-analyzer".to_string(),
                args: vec![],
                language_id: "rust".to_string(),
                file_extensions: vec!["rs".to_string()],
                root_markers: vec!["Cargo.toml".to_string()],
            },
        );
        servers.insert(
            "python".to_string(),
            ServerConfig {
                command: "pyright".to_string(),
                args: vec![],
                language_id: "python".to_string(),
                file_extensions: vec!["py".to_string(), "pyi".to_string()],
                root_markers: vec!["pyproject.toml".to_string()],
            },
        );
        LspConfig {
            enabled: true,
            servers,
        }
    }

    fn make_diag(severity: DiagnosticSeverity, msg: &str) -> ForgeDiagnostic {
        ForgeDiagnostic {
            severity,
            message: msg.to_string(),
            line: 0,
            col: 0,
            source: None,
        }
    }

    #[test]
    fn test_extension_map_built_correctly() {
        let manager = LspManager::new(test_config());
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
        let mut servers = HashMap::new();
        servers.insert(
            "b".to_string(),
            ServerConfig {
                command: "b-ls".to_string(),
                args: vec![],
                language_id: "b".to_string(),
                file_extensions: vec!["rs".to_string()],
                root_markers: vec![],
            },
        );
        servers.insert(
            "a".to_string(),
            ServerConfig {
                command: "a-ls".to_string(),
                args: vec![],
                language_id: "a".to_string(),
                file_extensions: vec!["rs".to_string()],
                root_markers: vec![],
            },
        );

        let manager = LspManager::new(LspConfig {
            enabled: true,
            servers,
        });
        assert_eq!(manager.extension_map.get("rs"), Some(&"a".to_string()));
    }

    #[test]
    fn test_has_running_servers_initially_false() {
        let manager = LspManager::new(test_config());
        assert!(!manager.has_running_servers());
    }

    #[test]
    fn test_snapshot_initially_empty() {
        let manager = LspManager::new(test_config());
        assert!(manager.snapshot().is_empty());
    }

    #[tokio::test]
    async fn test_poll_events_drains_diagnostics() {
        let manager = LspManager::new(test_config());
        let event_tx = manager.event_tx().clone();

        // Reinitialize so we can control the channel
        let mut manager = manager;

        // Send diagnostic events through the channel
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
        assert_eq!(snap.error_count, 1);
        assert_eq!(snap.warning_count, 1);
        assert_eq!(snap.files.len(), 2);
    }

    #[tokio::test]
    async fn test_poll_events_respects_budget() {
        let manager = LspManager::new(test_config());
        let event_tx = manager.event_tx().clone();
        let mut manager = manager;

        // Send 5 events
        for i in 0..5 {
            event_tx
                .send(LspEvent::Diagnostics {
                    path: PathBuf::from(format!("file{i}.rs")),
                    items: vec![make_diag(DiagnosticSeverity::Error, "err")],
                })
                .await
                .unwrap();
        }

        // Poll with budget of 3 — should only process 3
        let count = manager.poll_events(3);
        assert_eq!(count, 3);

        // Remaining 2 still in channel
        let count = manager.poll_events(10);
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_poll_events_empty_channel() {
        let mut manager = LspManager::new(test_config());
        let count = manager.poll_events(10);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_errors_for_files_via_events() {
        let manager = LspManager::new(test_config());
        let event_tx = manager.event_tx().clone();
        let mut manager = manager;

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
        assert_eq!(errors[0].1.len(), 1); // only the error
    }

    #[tokio::test]
    async fn test_on_file_changed_skips_unknown_extension() {
        let mut manager = LspManager::new(test_config());
        // .js is not in any server config — should be a no-op
        manager
            .on_file_changed(Path::new("/test/file.js"), "code")
            .await;
        // No panic, no events
    }

    #[tokio::test]
    async fn test_on_file_changed_skips_no_extension() {
        let mut manager = LspManager::new(test_config());
        manager
            .on_file_changed(Path::new("/test/Makefile"), "all:")
            .await;
        // No panic
    }

    #[tokio::test]
    async fn test_diagnostics_cleared_when_server_publishes_empty() {
        let manager = LspManager::new(test_config());
        let event_tx = manager.event_tx().clone();
        let mut manager = manager;

        // Publish errors
        event_tx
            .send(LspEvent::Diagnostics {
                path: PathBuf::from("a.rs"),
                items: vec![make_diag(DiagnosticSeverity::Error, "err")],
            })
            .await
            .unwrap();
        manager.poll_events(10);
        assert_eq!(manager.snapshot().error_count, 1);

        // Server clears diagnostics (empty array)
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
}

//! LSP integration — connects forge-lsp to the engine.
//!
//! Adds methods on `App` for:
//! - Polling LSP events each tick
//! - Notifying LSP servers about file changes after tool batches
//! - Deferred diagnostics check for agent feedback

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::App;
use crate::notifications::SystemNotification;

/// Maximum number of LSP events to process per tick.
const LSP_EVENT_BUDGET: usize = 32;

/// How long to wait for diagnostics after file changes before injecting feedback.
const DIAG_CHECK_DELAY: Duration = Duration::from_secs(3);

/// Maximum number of error lines in the agent feedback message.
const MAX_DIAG_FEEDBACK_LINES: usize = 20;

impl App {
    /// Poll LSP event channel, update snapshot, and check deferred diagnostics.
    ///
    /// Called from `tick()` each frame. Non-blocking.
    pub(crate) fn poll_lsp_events(&mut self) {
        let Ok(mut guard) = self.runtime.lsp_runtime.manager.try_lock() else {
            return;
        };
        let Some(lsp) = guard.as_mut() else { return };

        let processed = lsp.poll_events(LSP_EVENT_BUDGET);
        if processed > 0 {
            self.runtime.lsp_runtime.snapshot = lsp.snapshot();
        }

        // Check deferred diagnostics: after the deadline, inject errors as agent feedback
        if let Some((paths, deadline)) = &mut self.runtime.lsp_runtime.pending_diag_check
            && Instant::now() >= *deadline
        {
            if !lsp.has_running_servers() {
                // Manager exists but no servers survived — stop waiting.
                self.runtime.lsp_runtime.pending_diag_check = None;
                return;
            }

            let errors = lsp.errors_for_files(paths);
            if errors.is_empty() {
                self.runtime.lsp_runtime.pending_diag_check = None;
                return;
            }

            let summary = format_error_summary(&errors);
            self.core
                .notification_queue
                .push(SystemNotification::DiagnosticsFound { summary });
            self.runtime.lsp_runtime.pending_diag_check = None;
        }
    }

    /// Notify LSP servers about file changes after a tool batch.
    ///
    /// Called from `finish_turn()` after tool execution completes.
    /// On the first call, lazily constructs the `LspManager` via `start()`
    /// which spawns all configured servers. Subsequent calls reuse the manager.
    /// Schedules a deferred diagnostics check after a delay.
    pub(crate) fn notify_lsp_file_changes(
        &mut self,
        created: &BTreeSet<PathBuf>,
        modified: &BTreeSet<PathBuf>,
    ) {
        if created.is_empty() && modified.is_empty() {
            return;
        }

        let mut paths = BTreeSet::new();
        paths.extend(created.iter().cloned());
        paths.extend(modified.iter().cloned());
        let notified_paths: Vec<PathBuf> = paths.into_iter().collect();
        if notified_paths.is_empty() {
            return;
        }

        let lsp = self.runtime.lsp_runtime.manager.clone();
        // Consume config on first call — `take()` ensures single initialization.
        let config = self.runtime.lsp_runtime.config.take();
        let needs_start = config.is_some();

        // If no config to start and no existing manager, LSP is disabled.
        if !needs_start {
            let has_mgr = self
                .runtime
                .lsp_runtime
                .manager
                .try_lock()
                .is_ok_and(|g| g.is_some());
            if !has_mgr {
                return;
            }
        }

        let workspace_root = self.runtime.tool_settings.sandbox.working_dir();
        let task_paths = notified_paths.clone();

        tokio::spawn(async move {
            // Lazy start: construct manager on first call.
            if let Some(config) = config {
                let mgr = forge_lsp::LspManager::start(config, &workspace_root).await;
                let mut guard = lsp.lock().await;
                *guard = Some(mgr);
            }

            for path in task_paths {
                match tokio::fs::metadata(&path).await {
                    Ok(meta) if meta.len() > 1_048_576 => {
                        tracing::debug!(
                            "Skipping LSP notification for large file: {}",
                            path.display()
                        );
                        continue;
                    }
                    Err(_) => continue,
                    _ => {}
                }

                let text = match tokio::fs::read_to_string(&path).await {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::debug!(
                            "Skipping LSP notification (not UTF-8): {}: {e}",
                            path.display()
                        );
                        continue;
                    }
                };

                let mut guard = lsp.lock().await;
                if let Some(mgr) = guard.as_mut() {
                    mgr.on_file_changed(&path, &text).await;
                }
            }
        });

        if !notified_paths.is_empty() {
            self.runtime.lsp_runtime.pending_diag_check =
                Some((notified_paths, Instant::now() + DIAG_CHECK_DELAY));
        }
    }

    #[must_use]
    pub fn lsp_snapshot(&self) -> &forge_lsp::DiagnosticsSnapshot {
        &self.runtime.lsp_runtime.snapshot
    }

    /// Whether the LSP subsystem is active and has running servers.
    #[must_use]
    pub fn lsp_active(&self) -> bool {
        self.runtime
            .lsp_runtime
            .manager
            .try_lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(forge_lsp::LspManager::has_running_servers)
            })
            .unwrap_or(false)
    }

    /// Gracefully shut down all LSP servers.
    pub async fn shutdown_lsp(&mut self) {
        let mut guard = self.runtime.lsp_runtime.manager.lock().await;
        if let Some(mgr) = guard.as_mut() {
            mgr.shutdown().await;
        }
    }
}

/// Format diagnostic errors into a compact summary for agent feedback.
fn format_error_summary(errors: &[(PathBuf, Vec<forge_lsp::ForgeDiagnostic>)]) -> String {
    let mut lines = Vec::new();
    for (path, diags) in errors {
        for diag in diags {
            lines.push(diag.display_with_path(path));
            if lines.len() >= MAX_DIAG_FEEDBACK_LINES {
                lines.push("... (truncated)".to_string());
                return lines.join("\n");
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{MAX_DIAG_FEEDBACK_LINES, format_error_summary};
    use std::path::PathBuf;

    fn make_diag(msg: &str, line: u32) -> forge_lsp::ForgeDiagnostic {
        forge_lsp::ForgeDiagnostic::new(
            forge_lsp::DiagnosticSeverity::Error,
            msg.to_string(),
            line,
            0,
            "rustc".to_string(),
        )
    }

    #[test]
    fn test_format_error_summary_basic() {
        let errors = vec![(
            PathBuf::from("src/main.rs"),
            vec![make_diag("expected `;`", 10)],
        )];
        let summary = format_error_summary(&errors);
        assert!(summary.contains("src/main.rs:11:1"));
        assert!(summary.contains("expected `;`"));
    }

    #[test]
    fn test_format_error_summary_multiple_files() {
        let errors = vec![
            (
                PathBuf::from("a.rs"),
                vec![make_diag("err1", 0), make_diag("err2", 5)],
            ),
            (PathBuf::from("b.rs"), vec![make_diag("err3", 10)]),
        ];
        let summary = format_error_summary(&errors);
        let line_count = summary.lines().count();
        assert_eq!(line_count, 3);
    }

    #[test]
    fn test_format_error_summary_truncates_at_limit() {
        let diags: Vec<forge_lsp::ForgeDiagnostic> =
            (0..25).map(|i| make_diag(&format!("err{i}"), i)).collect();
        let errors = vec![(PathBuf::from("big.rs"), diags)];
        let summary = format_error_summary(&errors);
        assert!(summary.ends_with("... (truncated)"));
        let line_count = summary.lines().count();
        assert_eq!(line_count, MAX_DIAG_FEEDBACK_LINES + 1);
    }

    #[test]
    fn test_format_error_summary_empty() {
        let errors: Vec<(PathBuf, Vec<forge_lsp::ForgeDiagnostic>)> = vec![];
        let summary = format_error_summary(&errors);
        assert!(summary.is_empty());
    }
}

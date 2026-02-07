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
        let Some(lsp) = self.lsp.as_ref() else { return };
        let Ok(mut lsp) = lsp.try_lock() else {
            // LSP manager is busy (e.g. starting servers) — avoid blocking the UI tick.
            return;
        };

        let processed = lsp.poll_events(LSP_EVENT_BUDGET);
        if processed > 0 {
            // `snapshot()` clones + sorts the full diagnostics store; avoid doing it
            // every tick when no new events arrived.
            self.lsp_snapshot = lsp.snapshot();
        }

        // Check deferred diagnostics: after the deadline, inject errors as agent feedback
        if let Some((paths, deadline)) = &mut self.pending_diag_check
            && Instant::now() >= *deadline
        {
            // If servers aren't running yet, keep waiting. Startup can take a while.
            if !lsp.has_running_servers() {
                // If startup already ran and we still have no running servers (e.g. all failed),
                // don't keep retrying forever.
                if lsp.has_started() {
                    self.pending_diag_check = None;
                    return;
                }
                *deadline = Instant::now() + Duration::from_millis(500);
                return;
            }

            let errors = lsp.errors_for_files(paths);
            if errors.is_empty() {
                self.pending_diag_check = None;
                return;
            }

            let summary = format_error_summary(&errors);
            self.notification_queue
                .push(SystemNotification::DiagnosticsFound { summary });
            self.pending_diag_check = None;
        }
    }

    /// Ensure LSP servers are started, then notify about file changes.
    ///
    /// Called from `finish_turn()` after tool execution completes.
    /// Defers server startup to the first tool batch (lazy init).
    /// Schedules a deferred diagnostics check after a delay.
    pub(crate) fn notify_lsp_file_changes(
        &mut self,
        created: &BTreeSet<PathBuf>,
        modified: &BTreeSet<PathBuf>,
    ) {
        if created.is_empty() && modified.is_empty() {
            return;
        }
        let Some(lsp) = self.lsp.clone() else { return };

        let mut paths = BTreeSet::new();
        paths.extend(created.iter().cloned());
        paths.extend(modified.iter().cloned());
        let notified_paths: Vec<PathBuf> = paths.into_iter().collect();
        if notified_paths.is_empty() {
            return;
        }

        // Lazy start: start servers + push file notifications asynchronously.
        //
        // Important: do not block the TUI loop waiting for server startup/initialize.
        let workspace_root = self.tool_settings.sandbox.working_dir();
        let task_paths = notified_paths.clone();
        tokio::spawn(async move {
            // Startup requires exclusive access; do it once, then release the lock so polling
            // can continue while we read files.
            {
                let mut mgr = lsp.lock().await;
                mgr.ensure_started(&workspace_root).await;
            }

            for path in task_paths {
                // Skip very large files (> 1 MiB)
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

                let mut mgr = lsp.lock().await;
                mgr.on_file_changed(&path, &text).await;
            }
        });

        // Schedule deferred diagnostics check
        if !notified_paths.is_empty() {
            self.pending_diag_check = Some((notified_paths, Instant::now() + DIAG_CHECK_DELAY));
        }
    }

    /// Get the current diagnostics snapshot for UI display.
    #[must_use]
    pub fn lsp_snapshot(&self) -> &forge_lsp::DiagnosticsSnapshot {
        &self.lsp_snapshot
    }

    /// Whether the LSP subsystem is active and has running servers.
    #[must_use]
    pub fn lsp_active(&self) -> bool {
        self.lsp
            .as_ref()
            .and_then(|mgr| mgr.try_lock().ok())
            .is_some_and(|mgr| mgr.has_running_servers())
    }

    /// Gracefully shut down all LSP servers.
    pub async fn shutdown_lsp(&mut self) {
        if let Some(lsp) = &self.lsp {
            lsp.lock().await.shutdown().await;
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
    use super::*;

    fn make_diag(msg: &str, line: u32) -> forge_lsp::ForgeDiagnostic {
        forge_lsp::ForgeDiagnostic {
            severity: forge_lsp::DiagnosticSeverity::Error,
            message: msg.to_string(),
            line,
            col: 0,
            source: Some("rustc".to_string()),
        }
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
        // Create more than MAX_DIAG_FEEDBACK_LINES diagnostics
        let diags: Vec<forge_lsp::ForgeDiagnostic> =
            (0..25).map(|i| make_diag(&format!("err{i}"), i)).collect();
        let errors = vec![(PathBuf::from("big.rs"), diags)];
        let summary = format_error_summary(&errors);
        assert!(summary.ends_with("... (truncated)"));
        let line_count = summary.lines().count();
        // MAX_DIAG_FEEDBACK_LINES (20) + 1 for truncation message
        assert_eq!(line_count, MAX_DIAG_FEEDBACK_LINES + 1);
    }

    #[test]
    fn test_format_error_summary_empty() {
        let errors: Vec<(PathBuf, Vec<forge_lsp::ForgeDiagnostic>)> = vec![];
        let summary = format_error_summary(&errors);
        assert!(summary.is_empty());
    }
}

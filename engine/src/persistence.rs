//! History persistence and crash recovery for the App.
//!
//! This module handles:
//! - Saving and loading conversation history to/from disk
//! - Saving and loading session state (draft input, input history)
//! - Crash recovery from incomplete streams
//! - Journal commit/discard operations
//! - Message rollback after errors

use std::io::Write;

use forge_context::{RecoveredStream, StepId};
use forge_types::{Message, NonEmptyStaticStr, NonEmptyString, sanitize_terminal_text};
use tempfile::NamedTempFile;

use crate::session_state::SessionState;
use crate::state::{OperationState, ToolRecoveryState};
use crate::ui::DisplayItem;
use crate::util;
use crate::{App, ContextManager, MessageId};

// Recovery badge constants
pub(crate) const RECOVERY_COMPLETE_BADGE: NonEmptyStaticStr = NonEmptyStaticStr::new("[Recovered]");
pub(crate) const RECOVERY_INCOMPLETE_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Recovered incomplete]");
pub(crate) const RECOVERY_ERROR_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Recovered error]");
pub(crate) const ABORTED_JOURNAL_BADGE: NonEmptyStaticStr = NonEmptyStaticStr::new("[Aborted]");
pub(crate) const EMPTY_RESPONSE_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Empty response]");

impl App {
    pub fn save_history(&self) -> anyhow::Result<()> {
        let path = self.history_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            Self::ensure_secure_dir(parent)?;
        }

        self.context_manager.save(&path)
    }

    /// Load conversation history from disk (called during init if file exists).
    pub(crate) fn load_history_if_exists(&mut self) {
        let path = self.history_path();
        if !path.exists() {
            return;
        }

        match ContextManager::load(&path, self.model.clone()) {
            Ok(mut loaded_manager) => {
                let count = loaded_manager.history().len();
                // Sync output limit before replacing context manager
                loaded_manager.set_output_limit(self.output_limits.max_output_tokens());
                self.context_manager = loaded_manager;
                self.rebuild_display_from_history();
                if count > 0 {
                    let msg = format!("History: {count} msgs");
                    if let Ok(content) = NonEmptyString::try_from(msg) {
                        self.push_local_message(Message::system(content));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load history: {e}");
                if !self.history_load_warning_shown {
                    self.push_notification("Failed to load history.json — starting fresh.");
                    self.history_load_warning_shown = true;
                }
            }
        }
    }

    pub(crate) fn rebuild_display_from_history(&mut self) {
        self.display.clear();
        for entry in self.context_manager.history().entries() {
            self.display.push(DisplayItem::History(entry.id()));
        }
        self.display_version = self.display_version.wrapping_add(1);
    }

    pub(crate) fn push_history_message(&mut self, message: Message) -> MessageId {
        let id = self.context_manager.push_message(message);
        self.display.push(DisplayItem::History(id));
        self.display_version = self.display_version.wrapping_add(1);
        self.invalidate_usage_cache();
        id
    }

    /// Push an assistant message with an associated stream step ID.
    ///
    /// Used for streaming responses to enable idempotent crash recovery.
    pub(crate) fn push_history_message_with_step_id(
        &mut self,
        message: Message,
        step_id: StepId,
    ) -> MessageId {
        let id = self
            .context_manager
            .push_message_with_step_id(message, step_id);
        self.display.push(DisplayItem::History(id));
        self.display_version = self.display_version.wrapping_add(1);
        self.invalidate_usage_cache();
        id
    }

    /// Save history to disk.
    /// Returns true if successful, false if save failed (logged but not propagated).
    /// Called after user messages and assistant completions for crash durability.
    pub(crate) fn autosave_history(&mut self) -> bool {
        match self.save_history() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Autosave failed: {e}");
                if !self.autosave_warning_shown {
                    self.push_notification("Autosave failed — changes may not persist.");
                    self.autosave_warning_shown = true;
                }
                false
            }
        }
    }

    /// Save session state (draft input + input history) to disk.
    pub fn save_session(&self) -> anyhow::Result<()> {
        let path = self.session_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            Self::ensure_secure_dir(parent)?;
        }

        let state = SessionState::new(
            self.input.clone(),
            self.input_history.clone(),
            self.session_changes.clone(),
        );
        let json = serde_json::to_string_pretty(&state)?;

        // Atomic write: temp file + fsync + rename
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let mut tmp = NamedTempFile::new_in(parent)?;

        tmp.write_all(json.as_bytes())?;
        tmp.as_file().sync_all()?;

        // Persist with fallback for Windows
        if let Err(err) = tmp.persist(&path) {
            if path.exists() {
                // Windows: backup-restore pattern
                let backup_path = path.with_extension("bak");
                let _ = std::fs::remove_file(&backup_path);
                std::fs::rename(&path, &backup_path)?;

                if let Err(rename_err) = err.file.persist(&path) {
                    let _ = std::fs::rename(&backup_path, &path);
                    return Err(rename_err.error.into());
                }
                let _ = std::fs::remove_file(&backup_path);
            } else {
                return Err(err.error.into());
            }
        }

        Ok(())
    }

    /// Load session state from disk (called during init if file exists).
    pub(crate) fn load_session(&mut self) {
        let path = self.session_path();
        if !path.exists() {
            return;
        }

        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<SessionState>(&data) {
                Ok(state) if state.is_compatible() => {
                    // Restore input state
                    if let Some(input) = state.input {
                        self.input = input;
                    }
                    // Restore input history
                    self.input_history = state.history;
                    // Restore modified files
                    self.session_changes = state.modified_files;
                    tracing::debug!("Loaded session state from {}", path.display());
                }
                Ok(_) => {
                    tracing::debug!("Session state version mismatch, starting fresh");
                }
                Err(e) => {
                    tracing::warn!("Failed to parse session state: {e}");
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read session state: {e}");
            }
        }
    }

    /// Save session state to disk (non-fatal on failure).
    pub(crate) fn autosave_session(&mut self) -> bool {
        match self.save_session() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Session autosave failed: {e}");
                false
            }
        }
    }

    pub(crate) fn push_local_message(&mut self, message: Message) {
        self.display.push(DisplayItem::Local(message));
        self.display_version = self.display_version.wrapping_add(1);
    }

    /// Push a system notification message to the display.
    ///
    /// This adds a system message that appears in the content pane, visible to
    /// the user but not sent to the model. Used for confirmations, warnings,
    /// and transient feedback.
    pub(crate) fn push_notification(&mut self, message: impl Into<String>) {
        let text = message.into();
        if let Ok(content) = NonEmptyString::new(text) {
            self.push_local_message(Message::system(content));
        }
    }

    /// Check for and recover from a crashed streaming session.
    ///
    /// Returns `Some(RecoveredStream)` if there was an incomplete stream that was recovered.
    /// The recovered partial response is added to the conversation with a warning badge.
    ///
    /// # Idempotent recovery
    ///
    /// If history already contains an entry with the recovered `step_id`, we skip
    /// adding the message (it was already recovered or committed) and just finalize
    /// the journal cleanup.
    pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
        if let Ok(Some(mut recovered_batch)) = self.tool_journal.recover() {
            let stream_recovered = match self.stream_journal.recover() {
                Ok(recovered) => recovered,
                Err(e) => {
                    self.push_notification(format!("Recovery failed: {e}"));
                    return None;
                }
            };

            let (step_id, partial_text, stream_model_name) = match &stream_recovered {
                Some(
                    RecoveredStream::Complete {
                        step_id,
                        partial_text,
                        model_name,
                        ..
                    }
                    | RecoveredStream::Incomplete {
                        step_id,
                        partial_text,
                        model_name,
                        ..
                    }
                    | RecoveredStream::Errored {
                        step_id,
                        partial_text,
                        model_name,
                        ..
                    },
                ) => (
                    Some(*step_id),
                    Some(partial_text.as_str()),
                    model_name.clone(),
                ),
                None => (None, None, None),
            };

            if recovered_batch.assistant_text.trim().is_empty()
                && let Some(text) = partial_text
                && !text.trim().is_empty()
            {
                recovered_batch.assistant_text = text.to_string();
            }

            let model = util::parse_model_name_from_string(&recovered_batch.model_name)
                .or_else(|| {
                    stream_model_name
                        .as_ref()
                        .and_then(|name| util::parse_model_name_from_string(name))
                })
                .unwrap_or_else(|| self.model.clone());

            if let Some(step_id) = step_id {
                // Idempotency guard: if history already contains this step_id,
                // the response was already committed - just clean up stale journals
                if self.context_manager.has_step_id(step_id) {
                    if let Err(e) = self.tool_journal.discard_batch(recovered_batch.batch_id) {
                        tracing::warn!(
                            "Failed to discard idempotent tool batch {}: {e}",
                            recovered_batch.batch_id
                        );
                    }
                    if self.autosave_history() {
                        self.finalize_journal_commit(step_id);
                    } else {
                        tracing::warn!(
                            "Tool batch already in history but autosave failed; keeping step {step_id} recoverable"
                        );
                    }
                    self.push_notification(
                        "Tool batch already committed; cleaned up stale journals",
                    );
                    return stream_recovered;
                }

                if !recovered_batch.corrupted_args.is_empty() {
                    self.push_notification(format!(
                        "Warning: {} tool call(s) had corrupted arguments",
                        recovered_batch.corrupted_args.len()
                    ));
                }
                self.state = OperationState::ToolRecovery(ToolRecoveryState {
                    batch: recovered_batch,
                    step_id,
                    model,
                });
                self.push_notification("Recovered tool batch. Press R to resume or D to discard.");
                return stream_recovered;
            }

            tracing::warn!("Tool batch recovery found but no stream journal step.");
            self.push_notification("Recovered tool batch but stream journal missing; discarding");
            if let Err(e) = self.tool_journal.discard_batch(recovered_batch.batch_id) {
                tracing::warn!("Failed to discard orphaned tool batch: {e}");
            }
            return None;
        }

        let recovered = match self.stream_journal.recover() {
            Ok(Some(recovered)) => recovered,
            Ok(None) => return None,
            Err(e) => {
                self.push_notification(format!("Recovery failed: {e}"));
                return None;
            }
        };

        let (recovery_badge, step_id, last_seq, partial_text, error_text, model_name) =
            match &recovered {
                RecoveredStream::Complete {
                    step_id,
                    partial_text,
                    last_seq,
                    model_name,
                } => (
                    RECOVERY_COMPLETE_BADGE,
                    *step_id,
                    *last_seq,
                    partial_text.as_str(),
                    None,
                    model_name.clone(),
                ),
                RecoveredStream::Incomplete {
                    step_id,
                    partial_text,
                    last_seq,
                    model_name,
                } => (
                    RECOVERY_INCOMPLETE_BADGE,
                    *step_id,
                    *last_seq,
                    partial_text.as_str(),
                    None,
                    model_name.clone(),
                ),
                RecoveredStream::Errored {
                    step_id,
                    partial_text,
                    last_seq,
                    error,
                    model_name,
                } => (
                    RECOVERY_ERROR_BADGE,
                    *step_id,
                    *last_seq,
                    partial_text.as_str(),
                    Some(error.as_str()),
                    model_name.clone(),
                ),
            };

        // Check if history already has this step_id (idempotent recovery)
        if self.context_manager.has_step_id(step_id) {
            // Already recovered or committed - ensure it's persisted before pruning
            if self.autosave_history() {
                self.finalize_journal_commit(step_id);
            } else {
                self.push_notification(format!(
                    "Recovery already in history, but autosave failed; keeping step {step_id} recoverable"
                ));
            }
            return Some(recovered);
        }

        // Use the model from the stream if available, otherwise fall back to current
        let model = model_name
            .and_then(|name| util::parse_model_name_from_string(&name))
            .unwrap_or_else(|| self.model.clone());

        // Add the partial response as an assistant message with recovery badge.
        let mut recovered_content =
            NonEmptyString::try_from(recovery_badge).expect("recovery badge must be non-empty");
        if !partial_text.is_empty() {
            // Sanitize recovered text to prevent stored terminal injection
            let sanitized = sanitize_terminal_text(partial_text);
            recovered_content = recovered_content.append("\n\n").append(&sanitized);
        }
        if let Some(error) = error_text
            && !error.is_empty()
        {
            // Sanitize error text as well
            let sanitized_error = sanitize_terminal_text(error);
            let error_line = format!("Error: {sanitized_error}");
            recovered_content = recovered_content.append("\n\n").append(error_line.as_str());
        }

        // Push recovered partial response with step_id for idempotent future recovery
        self.push_history_message_with_step_id(
            Message::assistant(model, recovered_content),
            step_id,
        );
        let history_saved = self.autosave_history();

        // Seal any unsealed entries, then mark committed and prune
        if let Err(e) = self.stream_journal.seal_unsealed(step_id) {
            tracing::warn!("Failed to seal recovered journal: {e}");
        }
        if history_saved {
            self.finalize_journal_commit(step_id);
            if partial_text.is_empty() {
                // Nothing to recover - just cleaned up stale journal
                tracing::debug!(
                    "Cleared stale session journal (step {}, last seq {})",
                    step_id,
                    last_seq
                );
            } else {
                self.push_notification(format!(
                    "Recovered {} bytes from crashed session",
                    partial_text.len(),
                ));
            }
        } else if partial_text.is_empty() {
            // Nothing to recover but autosave failed - will retry next session
            tracing::debug!(
                "Stale journal cleanup deferred (step {}, last seq {})",
                step_id,
                last_seq
            );
        } else {
            self.push_notification(format!(
                "Recovered {} bytes but autosave failed; recovery will retry",
                partial_text.len(),
            ));
        }

        Some(recovered)
    }

    /// Atomically commit and prune a journal step.
    ///
    /// Called ONLY after history has been successfully persisted to disk.
    pub(crate) fn finalize_journal_commit(&mut self, step_id: StepId) {
        if let Err(e) = self.stream_journal.commit_and_prune_step(step_id) {
            tracing::warn!("Failed to commit/prune journal step {}: {e}", step_id);
        }
    }

    /// Discard a journal step that won't be recovered (error/empty cases).
    pub(crate) fn discard_journal_step(&mut self, step_id: StepId) {
        if let Err(e) = self.stream_journal.discard_step(step_id) {
            tracing::warn!("Failed to discard journal step {}: {e}", step_id);
        }
    }

    /// Commit a message to history with journal durability.
    ///
    /// This encapsulates the critical commit ordering for crash recovery:
    /// 1. Push message to history WITH `step_id` (for idempotent recovery)
    /// 2. Persist history to disk
    /// 3. Mark journal step as committed and prune (only if history persisted)
    ///
    /// Returns true if the full commit succeeded, false if history save failed
    /// (in which case the journal step remains recoverable for next session).
    pub(crate) fn commit_history_message(&mut self, message: Message, step_id: StepId) -> bool {
        self.push_history_message_with_step_id(message, step_id);
        if self.autosave_history() {
            self.finalize_journal_commit(step_id);
            true
        } else {
            // Leave journal recoverable for next session
            false
        }
    }

    /// Rollback a pending user message after stream error with no content.
    ///
    /// This removes the user message from history and display, then restores
    /// the original text to the input box for easy retry.
    pub(crate) fn rollback_pending_user_message(&mut self) {
        let Some((msg_id, original_text)) = self.pending_user_message.take() else {
            return;
        };

        if self.context_manager.rollback_last_message(msg_id).is_some() {
            // Remove from display (should be the last History item)
            if let Some(DisplayItem::History(display_id)) = self.display.last()
                && *display_id == msg_id
            {
                self.display.pop();
            }

            // Restore to input box and enter insert mode for easy retry
            self.input.draft_mut().set_text(original_text);
            self.input = std::mem::take(&mut self.input).into_insert();

            // Invalidate usage cache since we modified history
            self.invalidate_usage_cache();

            // Persist the rollback
            if let Err(e) = self.save_history() {
                tracing::warn!("Failed to save history after rollback: {e}");
            }

            tracing::debug!("Rolled back user message {:?} after stream error", msg_id);
        } else {
            tracing::warn!(
                "Failed to rollback user message {:?} - not the last message in history",
                msg_id
            );
        }
    }
}

//! Compaction (distillation) handling for the App.
//!
//! Engine-level retries were removed in favor of transport-layer retries.
//! See `providers/src/retry.rs` for HTTP retry policy.

use super::{
    ApiConfig, ContextBuildError, DistillationStart, DistillationState, DistillationTask,
    NonEmptyString, OperationState, QueuedUserMessage, TokenCounter, distillation_model,
    generate_distillation,
};
use crate::security;
use crate::util;

impl super::App {
    /// Trigger compaction when context is near capacity.
    ///
    /// Spawns an async task to generate the compaction summary.
    /// The result is polled via `poll_distillation()`.
    pub fn start_distillation(&mut self) {
        let _ = self.try_start_distillation(None);
    }

    /// Try to start compaction with an optional queued request.
    ///
    /// If context fits, queued request is immediately streamed. If compaction
    /// is needed, it's started and the queued request waits for completion.
    pub(crate) fn try_start_distillation(
        &mut self,
        queued_request: Option<QueuedUserMessage>,
    ) -> DistillationStart {
        let fail_with_rollback = |app: &mut Self, queued: Option<QueuedUserMessage>| {
            if let Some(queued) = queued {
                app.rollback_pending_user_message();
                app.finish_turn(queued.turn);
            }
            DistillationStart::Failed
        };

        match self.busy_state() {
            super::BusyState::Idle => {}
            super::BusyState::StreamingResponse
            | super::BusyState::ToolExecution
            | super::BusyState::PlanApproval
            | super::BusyState::ToolRecovery
            | super::BusyState::RecoveryBlocked
            | super::BusyState::Distillation => {
                return fail_with_rollback(self, queued_request);
            }
        }

        // Use the same overhead that start_streaming will use, so both agree on
        // whether context fits. Without this, compaction can loop: prepare(0)
        // says "fits" → start_streaming with real overhead says "doesn't fit" → repeat.
        let overhead = {
            let provider = queued_request
                .as_ref()
                .map_or_else(|| self.core.model.provider(), |q| q.config.provider());
            self.streaming_overhead(provider)
        };

        match self.core.context_manager.prepare(overhead) {
            Ok(_) => {
                if let Some(queued) = queued_request {
                    self.start_streaming(queued);
                }
                return DistillationStart::NotNeeded;
            }
            Err(ContextBuildError::CompactionNeeded) => {}
            Err(ContextBuildError::RecentMessagesTooLarge {
                required_tokens,
                budget_tokens,
                message_count,
            }) => {
                self.push_notification(format!(
                    "Recent {message_count} messages ({required_tokens} tokens) exceed budget ({budget_tokens} tokens). Reduce input or use larger model."
                ));
                return fail_with_rollback(self, queued_request);
            }
        }

        let plan = self.core.context_manager.prepare_compaction();
        let original_tokens = plan.original_tokens;

        self.push_notification(format!("Compacting ~{original_tokens} tokens..."));

        // Build API config for distillation.
        // When a request is queued, use its config (key + model) to ensure provider
        // consistency even if the user switches providers during compaction.
        let (api_key, model) = if let Some(queued) = queued_request.as_ref() {
            (queued.config.api_key_owned(), queued.config.model().clone())
        } else {
            let key = if let Some(key) = self.current_api_key().cloned() {
                util::wrap_api_key(self.core.model.provider(), key)
            } else {
                self.push_notification("Cannot compact: no API key configured");
                return DistillationStart::Failed;
            };
            (key, self.core.model.clone())
        };

        let config = match ApiConfig::new(api_key, model.clone()) {
            Ok(config) => config.with_openai_options(self.openai_options_for_model(&model)),
            Err(e) => {
                self.push_notification(format!("Cannot compact: {e}"));
                if let Some(queued) = queued_request {
                    self.rollback_pending_user_message();
                    self.finish_turn(queued.turn);
                }
                return DistillationStart::Failed;
            }
        };

        let generated_by = distillation_model(config.provider()).to_string();
        let messages = plan.messages;

        let counter = TokenCounter::new();
        let handle =
            tokio::spawn(async move { generate_distillation(&config, &counter, &messages).await });

        let task = DistillationTask {
            generated_by,
            handle,
        };

        self.op_transition(OperationState::Distilling(match queued_request {
            Some(message) => DistillationState::CompletedWithQueued { task, message },
            None => DistillationState::Running(task),
        }));
        DistillationStart::Started
    }

    /// Poll for completed compaction task and apply the result.
    ///
    /// This should be called in the main `tick()` loop. It checks if the background
    /// distillation task has completed, and if so, applies the result via
    /// `context_manager.complete_compaction()`.
    pub fn poll_distillation(&mut self) {
        use futures_util::future::FutureExt;

        let finished = match &self.core.state {
            OperationState::Distilling(state) => state.task().handle.is_finished(),
            _ => return,
        };

        if !finished {
            return;
        }

        let state = match self.op_take_distilling() {
            super::OperationTake::Taken(state) => state,
            super::OperationTake::Skipped => return,
        };
        let (task, queued_request) = match state {
            DistillationState::Running(task) => (task, None),
            DistillationState::CompletedWithQueued { task, message } => (task, Some(message)),
        };

        let DistillationTask {
            generated_by,
            mut handle,
        } = task;

        let result = (&mut handle).now_or_never();

        match result {
            Some(Ok(Ok(distillation_text))) => {
                // Sanitize output before storing — summaries are injected as system
                // messages and must not contain escape sequences, bidi controls, or
                // leaked API keys from the summarized conversation.
                let distillation_text = security::sanitize_display_text(&distillation_text);
                let distillation_text = if let Ok(text) = NonEmptyString::new(distillation_text) {
                    text
                } else {
                    self.handle_distillation_failure(
                        "compaction produced empty output".to_string(),
                        queued_request,
                    );
                    return;
                };

                self.core
                    .context_manager
                    .complete_compaction(distillation_text, generated_by);
                self.push_notification("Compaction complete");
                self.autosave_history();

                if let Some(queued) = queued_request {
                    self.start_streaming(queued);
                }
            }
            Some(Ok(Err(e))) => {
                self.handle_distillation_failure(e.to_string(), queued_request);
            }
            Some(Err(e)) => {
                self.handle_distillation_failure(format!("task panicked: {e}"), queued_request);
            }
            None => {
                // Edge-case: is_finished() was true but join handle isn't ready yet.
                // Restore state and retry next tick rather than failing.
                let task = DistillationTask {
                    generated_by,
                    handle,
                };
                self.op_restore_distilling(match queued_request {
                    Some(message) => DistillationState::CompletedWithQueued { task, message },
                    None => DistillationState::Running(task),
                });
            }
        }
    }

    /// Handle compaction failure.
    ///
    /// Transport-layer retries handle transient HTTP failures, so this function
    /// only sees errors after those retries are exhausted. We rollback any queued
    /// request and notify the user.
    fn handle_distillation_failure(
        &mut self,
        error: String,
        queued_request: Option<QueuedUserMessage>,
    ) {
        self.op_transition(self.idle_state());
        let had_pending = queued_request.is_some();

        if let Some(queued) = queued_request {
            self.rollback_pending_user_message();
            self.finish_turn(queued.turn);
        }
        let suffix = if had_pending {
            " Cancelled queued request."
        } else {
            ""
        };
        self.push_notification(format!("Compaction failed: {error}.{suffix}"));
    }
}

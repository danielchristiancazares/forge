//! Distillation handling for the App.
//!
//! Engine-level retries were removed in favor of transport-layer retries.
//! See `providers/src/retry.rs` for HTTP retry policy.

use super::{
    ApiConfig, ContextBuildError, DistillationStart, DistillationState, DistillationTask,
    NonEmptyString, OperationState, PendingDistillation, QueuedUserMessage, TokenCounter,
    distillation_model, generate_distillation,
};

impl super::App {
    /// Trigger distillation of older messages when context is near capacity.
    ///
    /// This prepares a distillation request from the context manager and spawns
    /// an async task to generate the distillation. The result is polled via `poll_distillation()`.
    pub fn start_distillation(&mut self) {
        let _ = self.try_start_distillation(None);
    }

    /// Try to start distillation with an optional queued request.
    ///
    /// If context fits, queued request is immediately streamed. If distillation
    /// is needed, it's started and the queued request waits for completion.
    ///
    /// Returns the start result indicating whether distillation started,
    /// was not needed, or failed.
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

        if self.busy_reason().is_some() {
            return fail_with_rollback(self, queued_request);
        }

        // Use the same overhead that start_streaming will use, so both agree on
        // whether context fits. Without this, distillation can loop: prepare(0)
        // says "fits" → start_streaming with real overhead says "doesn't fit" → repeat.
        let overhead = {
            let provider = queued_request
                .as_ref()
                .map(|q| q.config.provider())
                .unwrap_or_else(|| self.model.provider());
            self.streaming_overhead(provider)
        };

        let needed = match self.context_manager.prepare(overhead) {
            Ok(_) => {
                if let Some(queued) = queued_request {
                    self.start_streaming(queued);
                }
                return DistillationStart::NotNeeded;
            }
            Err(ContextBuildError::DistillationNeeded(needed)) => needed,
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
        };

        let pending = match self.context_manager.prepare_distillation(&needed) {
            Ok(pending) => pending,
            Err(e) => {
                self.push_notification(format!("Cannot distill: {e}"));
                return fail_with_rollback(self, queued_request);
            }
        };

        let PendingDistillation {
            scope,
            messages,
            original_tokens,
            target_tokens,
        } = pending;

        self.push_notification(format!(
            "Distilling ~{original_tokens} tokens → ~{target_tokens} tokens..."
        ));

        // Build API config for distillation.
        // When a request is queued, use its config (key + model) to ensure provider
        // consistency even if the user switches providers during distillation.
        let (api_key, model) = if let Some(queued) = queued_request.as_ref() {
            (queued.config.api_key_owned(), queued.config.model().clone())
        } else {
            let key = if let Some(key) = self.current_api_key().cloned() {
                crate::util::wrap_api_key(self.model.provider(), key)
            } else {
                self.push_notification("Cannot distill: no API key configured");
                return DistillationStart::Failed;
            };
            (key, self.model.clone())
        };

        let config = match ApiConfig::new(api_key, model.clone()) {
            Ok(config) => config.with_openai_options(self.openai_options_for_model(&model)),
            Err(e) => {
                self.push_notification(format!("Cannot distill: {e}"));
                // Rollback if we have a queued request (shouldn't happen in practice
                // since queued request already has a valid config, but be defensive)
                if let Some(queued) = queued_request {
                    self.rollback_pending_user_message();
                    self.finish_turn(queued.turn);
                }
                return DistillationStart::Failed;
            }
        };

        let generated_by = distillation_model(config.provider()).to_string();

        let counter = TokenCounter::new();
        let handle = tokio::spawn(async move {
            generate_distillation(&config, &counter, &messages, target_tokens).await
        });

        let task = DistillationTask {
            scope,
            generated_by,
            handle,
        };

        self.state = OperationState::Distilling(match queued_request {
            Some(message) => DistillationState::CompletedWithQueued { task, message },
            None => DistillationState::Running(task),
        });
        DistillationStart::Started
    }

    /// Poll for completed distillation task and apply the result.
    ///
    /// This should be called in the main `tick()` loop. It checks if the background
    /// distillation task has completed, and if so, applies the result via
    /// `context_manager.complete_distillation()`.
    pub fn poll_distillation(&mut self) {
        use futures_util::future::FutureExt;

        let finished = match &self.state {
            OperationState::Distilling(state) => state.task().handle.is_finished(),
            _ => return,
        };

        if !finished {
            return;
        }

        let idle = self.idle_state();
        let (task, queued_request) = match std::mem::replace(&mut self.state, idle) {
            OperationState::Distilling(state) => match state {
                DistillationState::Running(task) => (task, None),
                DistillationState::CompletedWithQueued { task, message } => (task, Some(message)),
            },
            other => {
                self.state = other;
                return;
            }
        };

        let DistillationTask {
            scope,
            generated_by,
            mut handle,
        } = task;

        let result = (&mut handle).now_or_never();

        match result {
            Some(Ok(Ok(distillation_text))) => {
                // Sanitize distillation output before storing — distillates are
                // injected as system messages and must not contain escape sequences,
                // bidi controls, or leaked API keys from summarized conversation.
                let distillation_text = crate::security::sanitize_display_text(&distillation_text);
                let distillation_text = if let Ok(text) = NonEmptyString::new(distillation_text) {
                    text
                } else {
                    self.handle_distillation_failure(
                        "distillation was empty".to_string(),
                        queued_request,
                    );
                    return;
                };

                if let Err(e) = self.context_manager.complete_distillation(
                    scope,
                    distillation_text,
                    generated_by,
                ) {
                    self.handle_distillation_failure(
                        format!("failed to apply distillation: {e}"),
                        queued_request,
                    );
                    return;
                }
                self.invalidate_usage_cache();
                self.push_notification("Distillation complete");
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
                // Restore state and retry next tick rather than failing the distillation.
                let task = DistillationTask {
                    scope,
                    generated_by,
                    handle,
                };
                self.state = OperationState::Distilling(match queued_request {
                    Some(message) => DistillationState::CompletedWithQueued { task, message },
                    None => DistillationState::Running(task),
                });
            }
        }
    }

    /// Handle distillation failure.
    ///
    /// Transport-layer retries handle transient HTTP failures, so this function
    /// only sees errors after those retries are exhausted. We rollback any queued
    /// request and notify the user.
    fn handle_distillation_failure(
        &mut self,
        error: String,
        queued_request: Option<QueuedUserMessage>,
    ) {
        self.state = self.idle_state();
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
        self.push_notification(format!("Distillation failed: {error}.{suffix}"));
    }
}

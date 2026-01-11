//! Summarization handling for the App.
//!
//! This module contains the context summarization logic:
//! - `start_summarization` - Initiates summarization request
//! - `poll_summarization` - Polls for summarization completion
//! - Retry logic with exponential backoff

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{
    ApiConfig, ApiKey, ContextBuildError, MAX_SUMMARIZATION_ATTEMPTS, NonEmptyString,
    OperationState, PendingSummarization, Provider, QueuedUserMessage, SUMMARIZATION_RETRY_BASE_MS,
    SUMMARIZATION_RETRY_JITTER_MS, SUMMARIZATION_RETRY_MAX_MS, SummarizationRetry,
    SummarizationRetryState, SummarizationRetryWithQueuedState, SummarizationStart,
    SummarizationState, SummarizationTask, SummarizationWithQueuedState, TokenCounter,
    generate_summary, summarization_model,
};

impl super::App {
    /// Trigger summarization of older messages when context is near capacity.
    ///
    /// This prepares a summarization request from the context manager and spawns
    /// an async task to generate the summary. The result is polled via `poll_summarization()`.
    pub fn start_summarization(&mut self) {
        let _ = self.start_summarization_with_attempt(None, 1);
    }

    pub(crate) fn start_summarization_with_attempt(
        &mut self,
        queued_request: Option<ApiConfig>,
        attempt: u8,
    ) -> SummarizationStart {
        if !self.context_infinity_enabled() {
            self.set_status("ContextInfinity disabled: summarization unavailable");
            return SummarizationStart::Failed;
        }
        if attempt > MAX_SUMMARIZATION_ATTEMPTS {
            return SummarizationStart::Failed;
        }

        if let Some(reason) = self.busy_reason() {
            self.set_status(format!("Cannot summarize: {reason}"));
            return SummarizationStart::Failed;
        }

        // Try to build working context to see if summarization is needed
        let message_ids = match self.context_manager.prepare() {
            Ok(_) => return SummarizationStart::NotNeeded, // No summarization needed
            Err(ContextBuildError::SummarizationNeeded(needed)) => needed.messages_to_summarize,
            Err(ContextBuildError::RecentMessagesTooLarge {
                required_tokens,
                budget_tokens,
                message_count,
            }) => {
                self.set_status(format!(
                    "Recent {} messages ({} tokens) exceed budget ({} tokens). Reduce input or use larger model.",
                    message_count, required_tokens, budget_tokens
                ));
                return SummarizationStart::Failed;
            }
        };

        // Prepare summarization request
        let Some(pending) = self.context_manager.prepare_summarization(&message_ids) else {
            return SummarizationStart::Failed;
        };

        let PendingSummarization {
            scope,
            messages,
            original_tokens,
            target_tokens,
        } = pending;

        let status = if attempt > 1 {
            format!(
                "Summarizing ~{} tokens → ~{} tokens (attempt {}/{})...",
                original_tokens, target_tokens, attempt, MAX_SUMMARIZATION_ATTEMPTS
            )
        } else {
            format!(
                "Summarizing ~{} tokens → ~{} tokens...",
                original_tokens, target_tokens
            )
        };
        self.set_status(status);

        // Build API config for summarization.
        // When a request is queued, use its config (key + model) to ensure provider
        // consistency even if the user switches providers during summarization.
        let (api_key, model) = if let Some(config) = queued_request.as_ref() {
            (config.api_key_owned(), config.model().clone())
        } else {
            let key = match self.current_api_key().cloned() {
                Some(key) => match self.model.provider() {
                    Provider::Claude => ApiKey::Claude(key),
                    Provider::OpenAI => ApiKey::OpenAI(key),
                },
                None => {
                    self.set_status("Cannot summarize: no API key configured");
                    return SummarizationStart::Failed;
                }
            };
            (key, self.model.clone())
        };

        let config = match ApiConfig::new(api_key, model) {
            Ok(config) => config,
            Err(e) => {
                self.set_status(format!("Cannot summarize: {e}"));
                return SummarizationStart::Failed;
            }
        };

        let generated_by = summarization_model(config.provider()).to_string();

        // Spawn background task with real API call
        let counter = TokenCounter::new();
        let handle = tokio::spawn(async move {
            generate_summary(&config, &counter, &messages, target_tokens).await
        });

        let task = SummarizationTask {
            scope,
            generated_by,
            handle,
            attempt,
        };

        self.state = if let Some(config) = queued_request {
            OperationState::SummarizingWithQueued(SummarizationWithQueuedState {
                task,
                queued: config,
            })
        } else {
            OperationState::Summarizing(SummarizationState { task })
        };
        SummarizationStart::Started
    }

    /// Poll for completed summarization task and apply the result.
    ///
    /// This should be called in the main tick() loop. It checks if the background
    /// summarization task has completed, and if so, applies the result via
    /// `context_manager.complete_summarization()`.
    pub fn poll_summarization(&mut self) {
        use futures_util::future::FutureExt;

        if !self.context_infinity_enabled() {
            return;
        }

        let finished = match &self.state {
            OperationState::Summarizing(state) => state.task.handle.is_finished(),
            OperationState::SummarizingWithQueued(state) => state.task.handle.is_finished(),
            _ => return,
        };

        // Check if the task is finished (non-blocking)
        if !finished {
            return;
        }

        // Take ownership of the task
        let (task, queued_request) = match std::mem::replace(&mut self.state, OperationState::Idle)
        {
            OperationState::Summarizing(state) => (state.task, None),
            OperationState::SummarizingWithQueued(state) => (state.task, Some(state.queued)),
            other => {
                self.state = other;
                return;
            }
        };

        let SummarizationTask {
            scope,
            generated_by,
            handle,
            attempt,
        } = task;

        // Get the result using now_or_never since we know it's finished
        let result = handle.now_or_never();

        match result {
            Some(Ok(Ok(summary_text))) => {
                let summary_text = match NonEmptyString::new(summary_text) {
                    Ok(text) => text,
                    Err(_) => {
                        self.handle_summarization_failure(
                            attempt,
                            "summary was empty".to_string(),
                            queued_request,
                        );
                        return;
                    }
                };

                // Apply the summarization result
                if let Err(e) =
                    self.context_manager
                        .complete_summarization(scope, summary_text, generated_by)
                {
                    self.handle_summarization_failure(
                        attempt,
                        format!("failed to apply summary: {e}"),
                        queued_request,
                    );
                    return;
                }
                self.invalidate_usage_cache();
                self.set_status("Summarization complete");
                self.autosave_history(); // Persist summarized history immediately

                // If a request was queued waiting for summarization, start it now.
                if let Some(config) = queued_request {
                    self.start_streaming(QueuedUserMessage { config });
                }
            }
            Some(Ok(Err(e))) => {
                self.handle_summarization_failure(attempt, e.to_string(), queued_request);
            }
            Some(Err(e)) => {
                self.handle_summarization_failure(
                    attempt,
                    format!("task panicked: {e}"),
                    queued_request,
                );
            }
            None => {
                // This shouldn't happen since we checked is_finished()
                self.handle_summarization_failure(
                    attempt,
                    "task not ready".to_string(),
                    queued_request,
                );
            }
        }
    }

    fn handle_summarization_failure(
        &mut self,
        attempt: u8,
        error: String,
        queued_request: Option<ApiConfig>,
    ) {
        self.state = OperationState::Idle;
        let next_attempt = attempt.saturating_add(1);
        let had_pending = queued_request.is_some();

        if next_attempt <= MAX_SUMMARIZATION_ATTEMPTS {
            let delay = summarization_retry_delay(next_attempt);
            let retry = SummarizationRetry {
                attempt: next_attempt,
                ready_at: Instant::now() + delay,
            };
            self.state = if let Some(config) = queued_request {
                OperationState::SummarizationRetryWithQueued(SummarizationRetryWithQueuedState {
                    retry,
                    queued: config,
                })
            } else {
                OperationState::SummarizationRetry(SummarizationRetryState { retry })
            };
            self.set_status(format!(
                "Summarization failed (attempt {}/{}): {}. Retrying in {}ms...",
                attempt,
                MAX_SUMMARIZATION_ATTEMPTS,
                error,
                delay.as_millis()
            ));
            return;
        }

        let suffix = if had_pending {
            " Cancelled queued request."
        } else {
            ""
        };
        self.set_status(format!(
            "Summarization failed after {} attempts: {}.{}",
            MAX_SUMMARIZATION_ATTEMPTS, error, suffix
        ));
    }

    pub(crate) fn poll_summarization_retry(&mut self) {
        if !self.context_infinity_enabled() {
            return;
        }

        let ready = match &self.state {
            OperationState::SummarizationRetry(state) => state.retry.ready_at <= Instant::now(),
            OperationState::SummarizationRetryWithQueued(state) => {
                state.retry.ready_at <= Instant::now()
            }
            _ => return,
        };

        if !ready {
            return;
        }

        let (retry, queued_request) = match std::mem::replace(&mut self.state, OperationState::Idle)
        {
            OperationState::SummarizationRetry(state) => (state.retry, None),
            OperationState::SummarizationRetryWithQueued(state) => {
                (state.retry, Some(state.queued))
            }
            other => {
                self.state = other;
                return;
            }
        };

        let attempt = retry.attempt;
        let had_pending = queued_request.is_some();
        let start_result = self.start_summarization_with_attempt(queued_request, attempt);

        if !matches!(start_result, SummarizationStart::Started) {
            let suffix = if had_pending {
                " Cancelled queued request."
            } else {
                ""
            };
            self.set_status(format!(
                "Summarization retry could not start (attempt {}/{}).{}",
                attempt, MAX_SUMMARIZATION_ATTEMPTS, suffix
            ));
        }
    }
}

/// Calculate exponential backoff delay for summarization retry.
fn summarization_retry_delay(attempt: u8) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10) as u32;
    let base = SUMMARIZATION_RETRY_BASE_MS.saturating_mul(1u64 << exponent);
    let capped = base.min(SUMMARIZATION_RETRY_MAX_MS);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let jitter = nanos % (SUMMARIZATION_RETRY_JITTER_MS + 1);

    let delay_ms = capped
        .saturating_add(jitter)
        .min(SUMMARIZATION_RETRY_MAX_MS);
    Duration::from_millis(delay_ms)
}

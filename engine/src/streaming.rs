//! Streaming response handling for the App.
//!
//! This module contains the core streaming logic:
//! - `start_streaming` - Initiates an API streaming request
//! - `process_stream_events` - Processes incoming stream events
//! - `finish_streaming` - Finalizes a streaming session

use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc;

/// Capacity for the bounded stream event channel.
/// This prevents OOM if the provider sends events faster than we can process them.
/// 1024 events provides ~10 seconds of buffer at 100 events/sec typical streaming rate.
const STREAM_EVENT_CHANNEL_CAPACITY: usize = 1024;

use forge_types::Provider;

use super::{
    ABORTED_JOURNAL_BADGE, ActiveStream, CacheableMessage, ContextBuildError,
    DEFAULT_STREAM_EVENT_BUDGET, EMPTY_RESPONSE_BADGE, GeminiCache, GeminiCacheConfig, Message,
    NonEmptyString, OperationState, QueuedUserMessage, StreamEvent, StreamFinishReason,
    StreamingMessage, SummarizationStart, format_stream_error, sanitize_terminal_text, security,
};

impl super::App {
    /// Start streaming response from the API.
    pub fn start_streaming(&mut self, queued: QueuedUserMessage) {
        if let Some(reason) = self.busy_reason() {
            self.set_status(format!("Busy: {reason}"));
            return;
        }

        let QueuedUserMessage { config } = queued;
        let context_infinity_enabled = self.context_infinity_enabled();

        // When context infinity enabled, use summarization-based context management.
        // Otherwise, use basic mode.
        let api_messages = if context_infinity_enabled {
            match self.context_manager.prepare() {
                Ok(prepared) => prepared.api_messages(),
                Err(ContextBuildError::SummarizationNeeded(needed)) => {
                    self.set_status(format!(
                        "{} (excess ~{} tokens)",
                        needed.suggestion, needed.excess_tokens
                    ));
                    let start_result = self.start_summarization_with_attempt(Some(config), 1);
                    if !matches!(start_result, SummarizationStart::Started) {
                        self.set_status("Cannot start: summarization did not start");
                    }
                    return;
                }
                Err(ContextBuildError::RecentMessagesTooLarge {
                    required_tokens,
                    budget_tokens,
                    message_count,
                }) => {
                    self.set_status(format!(
                        "Recent {message_count} messages ({required_tokens} tokens) exceed budget ({budget_tokens} tokens). Reduce input or use larger model."
                    ));
                    return;
                }
            }
        } else {
            self.build_basic_api_messages()
        };

        let journal = match self.stream_journal.begin_session(config.model().as_str()) {
            Ok(session) => session,
            Err(e) => {
                self.set_status(format!("Cannot start stream: journal unavailable ({e})"));
                return;
            }
        };

        let (tx, rx) = mpsc::channel(STREAM_EVENT_CHANNEL_CAPACITY);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        let active = ActiveStream {
            message: StreamingMessage::new(
                config.model().clone(),
                rx,
                self.tool_settings.limits.max_tool_args_bytes,
            ),
            journal,
            abort_handle,
            tool_batch_id: None,
            tool_call_seq: 0,
        };

        self.state = OperationState::Streaming(active);

        // OutputLimits is pre-validated at config load time - no runtime checks needed
        // Invariant: if thinking is enabled, budget < max_tokens (guaranteed by type)
        let limits = self.output_limits;

        // Convert messages to cacheable format based on cache_enabled setting
        let cache_enabled = self.cache_enabled;
        // Select the correct system prompt for the active provider
        let system_prompt = self
            .system_prompts
            .map(|prompts| prompts.get(config.provider()));
        let cacheable_messages: Vec<CacheableMessage> = if cache_enabled {
            // Cache older messages, keep recent ones fresh
            // Claude allows max 4 cache_control blocks total
            // System prompt uses 1 slot if present, leaving 3 for messages
            let max_cached = if system_prompt.is_some() { 3 } else { 4 };
            let len = api_messages.len();
            let recent_threshold = len.saturating_sub(4); // Don't cache last 4 messages
            let mut cached_count = 0;
            api_messages
                .into_iter()
                .enumerate()
                .map(|(i, msg)| {
                    if i < recent_threshold && cached_count < max_cached {
                        cached_count += 1;
                        CacheableMessage::cached(msg)
                    } else {
                        CacheableMessage::plain(msg)
                    }
                })
                .collect()
        } else {
            api_messages
                .into_iter()
                .map(CacheableMessage::plain)
                .collect()
        };

        // Clone tool definitions for async task
        let tools = self.tool_definitions.clone();

        // Clone Gemini cache state for async task (only relevant for Gemini provider)
        let gemini_cache_arc = self.gemini_cache.clone();
        let gemini_cache_config = self.gemini_cache_config.clone();
        let is_gemini = config.provider() == Provider::Gemini;

        let task = async move {
            let tx_events = tx.clone();
            // Convert tools to Option<&[ToolDefinition]>
            let tools_ref = if tools.is_empty() {
                None
            } else {
                Some(tools.as_slice())
            };

            // Handle Gemini cache lifecycle
            let gemini_cache = if is_gemini
                && gemini_cache_config.enabled
                && let Some(prompt) = system_prompt
            {
                get_or_create_gemini_cache(
                    &gemini_cache_arc,
                    &gemini_cache_config,
                    config.api_key(),
                    config.model().as_str(),
                    prompt,
                )
                .await
            } else {
                None
            };

            let result = forge_providers::send_message(
                &config,
                &cacheable_messages,
                limits,
                system_prompt,
                tools_ref,
                gemini_cache.as_ref(),
                move |event| {
                    // Use try_send with bounded channel - drop events if full.
                    // This is acceptable for streaming UI updates; the consumer will catch up.
                    let _ = tx_events.try_send(event);
                },
            )
            .await;

            if let Err(e) = result {
                let _ = tx.try_send(StreamEvent::Error(e.to_string()));
            }
        };

        tokio::spawn(async move {
            let _ = Abortable::new(task, abort_registration).await;
        });
    }

    /// Process any pending stream events.
    pub fn process_stream_events(&mut self) {
        if !matches!(self.state, OperationState::Streaming(_)) {
            return;
        }

        let max_events = DEFAULT_STREAM_EVENT_BUDGET;
        if max_events == 0 {
            return;
        }

        // Process all available events.
        let mut processed = 0usize;
        loop {
            let event = {
                let active = match &mut self.state {
                    OperationState::Streaming(active) => active,
                    _ => return,
                };

                match active.message.try_recv_event() {
                    Ok(event) => event,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        tracing::warn!("Stream channel disconnected");
                        StreamEvent::Error("stream disconnected".to_string())
                    }
                }
            };

            let event = match event {
                StreamEvent::TextDelta(text) => {
                    // Sanitize untrusted model output to prevent terminal injection
                    StreamEvent::TextDelta(sanitize_terminal_text(&text).into_owned())
                }
                StreamEvent::ThinkingDelta(text) => {
                    // Also sanitize thinking deltas
                    StreamEvent::ThinkingDelta(sanitize_terminal_text(&text).into_owned())
                }
                StreamEvent::Error(msg) => {
                    StreamEvent::Error(security::sanitize_stream_error(&msg))
                }
                other => other,
            };

            let mut journal_error: Option<String> = None;
            let mut finish_reason: Option<StreamFinishReason> = None;
            let update_assistant_text = matches!(event, StreamEvent::TextDelta(_));

            let mut active = match std::mem::replace(&mut self.state, OperationState::Idle) {
                OperationState::Streaming(active) => active,
                other => {
                    self.state = other;
                    return;
                }
            };

            let persist_result = match &event {
                StreamEvent::TextDelta(text) => active
                    .journal
                    .append_text(&mut self.stream_journal, text.clone()),
                StreamEvent::ThinkingDelta(_) => {
                    // Don't persist thinking content to journal - silently consume
                    Ok(())
                }
                StreamEvent::ToolCallStart { .. } | StreamEvent::ToolCallDelta { .. } => Ok(()),
                StreamEvent::Done => active.journal.append_done(&mut self.stream_journal),
                StreamEvent::Error(msg) => active
                    .journal
                    .append_error(&mut self.stream_journal, msg.clone()),
            };

            // Persist BEFORE display.
            if let Err(e) = persist_result {
                journal_error = Some(e.to_string());
            }

            if journal_error.is_none() {
                match &event {
                    StreamEvent::ToolCallStart { id, name, .. } => {
                        if active.tool_batch_id.is_none() {
                            match self
                                .tool_journal
                                .begin_streaming_batch(active.journal.model_name())
                            {
                                Ok(batch_id) => {
                                    active.tool_batch_id = Some(batch_id);
                                }
                                Err(e) => journal_error = Some(e.to_string()),
                            }
                        }
                        if let Some(batch_id) = active.tool_batch_id {
                            let seq = active.tool_call_seq;
                            active.tool_call_seq = active.tool_call_seq.saturating_add(1);
                            if let Err(e) =
                                self.tool_journal.record_call_start(batch_id, seq, id, name)
                            {
                                journal_error = Some(e.to_string());
                            }
                        }
                    }
                    StreamEvent::ToolCallDelta { id, arguments } => {
                        if let Some(batch_id) = active.tool_batch_id
                            && let Err(e) =
                                self.tool_journal.append_call_args(batch_id, id, arguments)
                        {
                            journal_error = Some(e.to_string());
                        }
                    }
                    _ => {}
                }
            }

            if journal_error.is_none() {
                finish_reason = active.message.apply_event(event);
                if update_assistant_text
                    && let Some(batch_id) = active.tool_batch_id
                    && let Err(e) = self
                        .tool_journal
                        .update_assistant_text(batch_id, active.message.content())
                {
                    journal_error = Some(e.to_string());
                }
            }

            self.state = OperationState::Streaming(active);

            if let Some(err) = journal_error {
                // Abort streaming without applying the unpersisted event.
                let active = match self.replace_with_idle() {
                    OperationState::Streaming(active) => active,
                    other => {
                        self.state = other;
                        return;
                    }
                };

                let ActiveStream {
                    message,
                    journal,
                    abort_handle,
                    ..
                } = active;

                abort_handle.abort();

                let step_id = journal.step_id();
                if let Err(seal_err) = journal.seal(&mut self.stream_journal) {
                    tracing::warn!("Journal seal failed after append error: {seal_err}");
                }
                // Discard the step to prevent blocking future sessions
                self.discard_journal_step(step_id);

                let model = message.model_name().clone();
                let partial = message.content().to_string();
                let aborted_badge = NonEmptyString::try_from(ABORTED_JOURNAL_BADGE)
                    .expect("ABORTED_JOURNAL_BADGE must be non-empty");
                let aborted = if partial.is_empty() {
                    aborted_badge
                } else {
                    aborted_badge.append("\n\n").append(partial.as_str())
                };
                self.push_local_message(Message::assistant(model, aborted));
                self.set_status(format!("Journal append failed: {err}"));
                return;
            }

            if let Some(reason) = finish_reason {
                self.finish_streaming(reason);
                return;
            }

            processed = processed.saturating_add(1);
            if processed >= max_events {
                break;
            }
        }
    }

    /// Finish the current streaming session and commit the message.
    ///
    /// # Commit ordering for crash durability
    ///
    /// The order of operations is critical for durability:
    /// 1. Capture `step_id` before consuming the journal
    /// 2. Seal the journal (marks stream as complete in `SQLite`)
    /// 3. Push message to history WITH `step_id` (for idempotent recovery)
    /// 4. Persist history to disk
    /// 5. Mark journal step as committed (after history is persisted)
    /// 6. Prune the journal step (cleanup)
    ///
    /// This ensures that if we crash after step 2 but before step 5, recovery
    /// will find the uncommitted step. If history already has that `step_id`,
    /// recovery will skip it (idempotent).
    pub(crate) fn finish_streaming(&mut self, finish_reason: StreamFinishReason) {
        let active = match self.replace_with_idle() {
            OperationState::Streaming(active) => active,
            other => {
                self.state = other;
                return;
            }
        };

        let ActiveStream {
            mut message,
            journal,
            abort_handle,
            tool_batch_id,
            ..
        } = active;

        abort_handle.abort();

        // Capture step_id before consuming the journal
        let step_id = journal.step_id();

        // Seal the journal (marks stream as complete)
        if let Err(e) = journal.seal(&mut self.stream_journal) {
            tracing::warn!("Journal seal failed: {e}");
            // Continue anyway - we'll try to commit to history
        }

        // Capture metadata before consuming the streaming message.
        let model = message.model_name().clone();

        // SECURITY: Check finish_reason FIRST before processing tool calls.
        // This prevents tools from executing when the stream ended with an error,
        // which could happen if partial tool call data accumulated before the error.
        if let StreamFinishReason::Error(err) = finish_reason {
            // Discard any pending tool batch - do NOT execute tools on error
            if let Some(batch_id) = tool_batch_id
                && let Err(e) = self.tool_journal.discard_batch(batch_id)
            {
                tracing::warn!("Failed to discard tool batch on stream error: {e}");
            }

            // Convert streaming message to completed message (empty content is invalid).
            let message = message.into_message().ok();

            if let Some(message) = message {
                // Partial content received - keep both user message and partial response
                self.pending_user_message = None;
                self.commit_history_message(message, step_id);
            } else {
                // No message content - rollback user message for easy retry
                self.discard_journal_step(step_id);
                self.rollback_pending_user_message();
            }
            // Use stream's model/provider, not current app settings (user may have changed during stream)
            let ui_error = format_stream_error(model.provider(), model.as_str(), &err);
            let system_msg = Message::system(ui_error.message);
            self.push_local_message(system_msg);
            self.set_status(ui_error.status);
            return;
        }

        // Only process tool calls when stream completed successfully (Done)
        if message.has_tool_calls() {
            let parsed = message.take_tool_calls();
            let assistant_text = message.content().to_string();
            // NOTE: We do NOT clear pending_user_message here because:
            // 1. The user message was already committed to history
            // 2. We need the user query for Librarian extraction when the turn completes
            // 3. rollback_pending_user_message() safely fails if it's not the last message
            self.handle_tool_calls(
                assistant_text,
                parsed.calls,
                parsed.pre_resolved,
                model,
                step_id,
                tool_batch_id,
            );
            return;
        }

        // Convert streaming message to completed message (empty content is invalid).
        let Some(message) = message.into_message().ok() else {
            // Stream completed successfully but with empty content - unusual but not an error
            self.pending_user_message = None;
            let empty_badge = NonEmptyString::try_from(EMPTY_RESPONSE_BADGE)
                .expect("EMPTY_RESPONSE_BADGE must be non-empty");
            let empty_msg = Message::assistant(model, empty_badge);
            self.push_local_message(empty_msg);
            self.set_status("Warning: API returned empty response");
            // Empty response - discard the step (nothing to recover)
            self.discard_journal_step(step_id);
            return;
        };

        // Stream completed successfully with content
        self.pending_user_message = None;
        self.commit_history_message(message, step_id);
    }
}

/// Get an existing valid Gemini cache or create a new one.
///
/// This function checks if there's a valid (non-expired, matching) cache.
/// If not, it creates a new cache via the Gemini API and stores it.
async fn get_or_create_gemini_cache(
    cache_arc: &std::sync::Arc<tokio::sync::Mutex<Option<GeminiCache>>>,
    config: &GeminiCacheConfig,
    api_key: &str,
    model: &str,
    system_prompt: &str,
) -> Option<GeminiCache> {
    // First, check if we have a valid cache
    {
        let guard = cache_arc.lock().await;
        if let Some(cache) = guard.as_ref() {
            if !cache.is_expired() && cache.matches_prompt(system_prompt) {
                tracing::debug!("Using existing Gemini cache: {}", cache.name);
                return Some(cache.clone());
            }
            tracing::debug!(
                "Gemini cache invalid (expired: {}, prompt mismatch: {})",
                cache.is_expired(),
                !cache.matches_prompt(system_prompt)
            );
        }
    }

    // Cache is invalid or doesn't exist - create a new one
    tracing::info!("Creating new Gemini cache for system prompt");
    match forge_providers::gemini::create_cache(api_key, model, system_prompt, config.ttl_seconds)
        .await
    {
        Ok(new_cache) => {
            tracing::info!(
                "Created Gemini cache: {} (expires: {})",
                new_cache.name,
                new_cache.expire_time
            );
            let cache_clone = new_cache.clone();
            // Store the new cache
            let mut guard = cache_arc.lock().await;
            *guard = Some(new_cache);
            Some(cache_clone)
        }
        Err(e) => {
            // Log warning and continue without cache (graceful degradation)
            tracing::warn!("Failed to create Gemini cache: {e}");
            None
        }
    }
}

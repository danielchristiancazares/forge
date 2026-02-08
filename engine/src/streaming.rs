//! Streaming response handling for the App.

use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc;

const STREAM_EVENT_CHANNEL_CAPACITY: usize = 1024;
const REDACTED_THINKING_PLACEHOLDER: &str = "[Thinking hidden]";

use forge_context::{BeginSessionError, TokenCounter};
use forge_types::{ModelName, Provider, ToolDefinition};

use super::{
    ABORTED_JOURNAL_BADGE, ActiveStream, CacheableMessage, ContextBuildError,
    DEFAULT_STREAM_EVENT_BUDGET, DistillationStart, EMPTY_RESPONSE_BADGE, GeminiCache,
    GeminiCacheConfig, Message, NonEmptyString, OperationState, QueuedUserMessage, StreamEvent,
    StreamFinishReason, StreamingMessage, ThoughtSignatureState, notifications,
    sanitize_terminal_text, security,
};
use crate::errors::format_stream_error;

fn build_thinking_message(
    model: ModelName,
    content: String,
    signature: ThoughtSignatureState,
) -> Option<Message> {
    // Thinking content is untrusted external text (provider output). Sanitize before it can
    // reach history/local display paths.
    let sanitized = security::sanitize_display_text(&content);
    if let Ok(thinking) = NonEmptyString::new(sanitized) {
        return Some(match signature {
            ThoughtSignatureState::Signed(sig) => {
                Message::thinking_with_signature(model, thinking, sig.as_str().to_string())
            }
            ThoughtSignatureState::Unsigned => Message::thinking(model, thinking),
        });
    }

    match signature {
        ThoughtSignatureState::Signed(sig) => {
            let placeholder = NonEmptyString::try_from(REDACTED_THINKING_PLACEHOLDER)
                .expect("REDACTED_THINKING_PLACEHOLDER must be non-empty");
            Some(Message::thinking_with_signature(
                model,
                placeholder,
                sig.as_str().to_string(),
            ))
        }
        ThoughtSignatureState::Unsigned => None,
    }
}

/// Geometric grid of cache breakpoint positions (0-indexed message indices).
///
/// Each value is approximately 1.5× the previous, providing stability proportional
/// to position depth: a breakpoint at position P is stable for roughly P/2 messages
/// of conversation growth before the next grid point is reached and breakpoints shift.
///
/// The grid covers conversations up to ~1024 messages. Beyond that, the last three
/// grid points still provide substantial coverage.
const CACHE_BREAKPOINT_GRID: &[usize] = &[
    3, 7, 15, 23, 31, 47, 63, 95, 127, 191, 255, 383, 511, 767, 1023,
];

/// Select up to 3 cache breakpoint positions from the geometric grid.
///
/// Takes the 3 highest grid positions that fit within `eligible` (the number of
/// messages excluding the fresh tail). This maximizes cache prefix coverage while
/// maintaining cross-turn stability — breakpoints only shift when the conversation
/// grows past the next grid boundary, and when they do, the lower breakpoints
/// typically survive as cache hits.
///
/// Returns an empty vec when `eligible` is 0.
fn cache_breakpoint_positions(eligible: usize) -> Vec<usize> {
    let fitting: Vec<usize> = CACHE_BREAKPOINT_GRID
        .iter()
        .copied()
        .filter(|&pos| pos < eligible)
        .collect();

    let start = fitting.len().saturating_sub(3);
    fitting[start..].to_vec()
}

impl super::App {
    pub fn start_streaming(&mut self, queued: QueuedUserMessage) {
        if self.busy_reason().is_some() {
            return;
        }

        let QueuedUserMessage { config, turn } = queued;
        let memory_enabled = self.memory_enabled();

        // Calculate overhead from system prompt and tools to avoid context overflow
        let system_prompt = self.system_prompts.get(config.provider());
        let provider = config.provider();
        let tools: Vec<_> = self
            .tool_definitions
            .iter()
            .filter(|t| t.provider.is_none() || t.provider == Some(provider))
            .cloned()
            .collect();

        let counter = TokenCounter::new();
        let sys_tokens = counter.count_str(system_prompt);
        let tool_tokens = if tools.is_empty() {
            0
        } else {
            // Estimate tool definition size
            match serde_json::to_string(&tools) {
                Ok(s) => counter.count_str(&s),
                Err(_) => 0,
            }
        };
        let overhead = sys_tokens + tool_tokens;

        // When memory enabled, use distillation-based context management.
        // Otherwise, use basic mode.
        let api_messages = if memory_enabled {
            match self.context_manager.prepare() {
                Ok(prepared) => prepared.api_messages(),
                Err(ContextBuildError::DistillationNeeded(needed)) => {
                    self.push_notification(format!(
                        "{} (excess ~{} tokens)",
                        needed.suggestion, needed.excess_tokens
                    ));
                    let queued = QueuedUserMessage { config, turn };
                    let start_result = self.try_start_distillation(Some(queued));
                    if !matches!(start_result, DistillationStart::Started) {
                        self.push_notification("Cannot start: distillation did not start");
                        // Note: rollback is handled inside try_start_distillation
                        // when it fails with a queued request
                    }
                    return;
                }
                Err(ContextBuildError::RecentMessagesTooLarge {
                    required_tokens,
                    budget_tokens,
                    message_count,
                }) => {
                    self.push_notification(format!(
                        "Recent {message_count} messages ({required_tokens} tokens) exceed budget ({budget_tokens} tokens). Reduce input or use larger model."
                    ));
                    // Rollback the pending user message so user can retry with smaller input
                    self.rollback_pending_user_message();
                    self.finish_turn(turn);
                    return;
                }
            }
        } else {
            self.build_basic_api_messages(overhead)
        };

        let journal = match self.stream_journal.begin_session(config.model().as_str()) {
            Ok(session) => session,
            Err(BeginSessionError::RecoverableStepExists(_step_id)) => {
                // A recoverable step exists — attempt recovery without corrupting
                // chronology.  Rollback the pending user message first, otherwise
                // recovered assistant/tool output would be appended after this prompt.
                self.rollback_pending_user_message();
                let recovered = self.check_crash_recovery();
                if recovered.is_some() || matches!(self.state, OperationState::ToolRecovery(_)) {
                    self.push_notification(
                        "Recovered unfinished work. Review it, then press Enter to retry.",
                    );
                    self.finish_turn(turn);
                    return;
                }
                // Recovery didn't find anything actionable; fall back to normal error path.
                self.push_notification(
                    "Cannot start stream: recoverable step exists but recovery found nothing actionable",
                );
                self.finish_turn(turn);
                return;
            }
            Err(e @ (BeginSessionError::AlreadyStreaming(_) | BeginSessionError::Db(_))) => {
                self.push_notification(format!("Cannot start stream: journal unavailable ({e})"));
                // Rollback the pending user message so user can retry
                self.rollback_pending_user_message();
                self.finish_turn(turn);
                return;
            }
        };

        let (tx, rx) = mpsc::channel(STREAM_EVENT_CHANNEL_CAPACITY);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        let active = ActiveStream::Transient {
            message: StreamingMessage::new(
                config.model().clone(),
                rx,
                self.tool_settings.limits.max_tool_args_bytes,
            ),
            journal,
            abort_handle,
            tool_call_seq: 0,
            tool_args_journal_bytes: std::collections::HashMap::new(),
            turn,
        };

        self.state = OperationState::Streaming(active);

        // OutputLimits is pre-validated at config load time - no runtime checks needed
        // Invariant: if thinking is enabled, budget < max_tokens (guaranteed by type)
        let limits = self.output_limits;

        // Convert messages to cacheable format based on cache_enabled setting
        let cache_enabled = self.cache_enabled;
        // system_prompt already retrieved above
        let cacheable_messages: Vec<CacheableMessage> = if cache_enabled {
            // Claude allows max 4 cache_control blocks total.
            // System prompt uses 1 slot, leaving 3 for messages.
            // Keep the last 4 messages uncached (fresh tail — still evolving).
            let eligible = api_messages.len().saturating_sub(4);
            let breakpoints = cache_breakpoint_positions(eligible);
            api_messages
                .into_iter()
                .enumerate()
                .map(|(i, msg)| {
                    if breakpoints.contains(&i) {
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

        // Inject any pending system notifications as an assistant message
        let cacheable_messages = self.inject_pending_notifications(cacheable_messages);

        // tools already retrieved above (cloned)

        // Clone Gemini cache state for async task (only relevant for Gemini provider)
        let gemini_cache_arc = self.gemini_cache.clone();
        let gemini_cache_config = self.gemini_cache_config.clone();
        let is_gemini = config.provider() == Provider::Gemini;

        let task = async move {
            // Convert tools to Option<&[ToolDefinition]>
            let tools_ref = if tools.is_empty() {
                None
            } else {
                Some(tools.as_slice())
            };

            // Handle Gemini cache lifecycle
            let gemini_cache = if is_gemini && gemini_cache_config.enabled {
                get_or_create_gemini_cache(
                    &gemini_cache_arc,
                    &gemini_cache_config,
                    config.api_key(),
                    config.model().as_str(),
                    system_prompt,
                    tools_ref,
                )
                .await
            } else {
                None
            };

            let result = forge_providers::send_message(
                &config,
                &cacheable_messages,
                limits,
                Some(system_prompt),
                tools_ref,
                gemini_cache.as_ref(),
                tx.clone(),
            )
            .await;

            if let Err(e) = result {
                tracing::warn!("LLM streaming request failed: {e}");
                let _ = tx.send(StreamEvent::Error(e.to_string())).await;
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
        // Consecutive TextDelta events are coalesced to reduce processing overhead
        // and minimize the risk of unbounded channel growth under slow consumption.
        let mut processed = 0usize;
        let mut pending_event: Option<StreamEvent> = None;

        loop {
            // First check if there's a pending event from a previous coalescing pass
            let event = if let Some(event) = pending_event.take() {
                event
            } else {
                let active = match &mut self.state {
                    OperationState::Streaming(active) => active,
                    _ => return,
                };

                match active.message_mut().try_recv_event() {
                    Ok(event) => event,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        tracing::warn!("Stream channel disconnected");
                        StreamEvent::Error("stream disconnected".to_string())
                    }
                }
            };

            // Coalesce consecutive TextDelta / ThinkingDelta events to reduce processing overhead.
            // This merges multiple deltas into one before journaling/displaying.
            // We count ALL coalesced events towards the budget to prevent bypassing it.
            let (event, already_counted) = match event {
                StreamEvent::TextDelta(mut text) => {
                    // Count the initial event towards the budget
                    processed = processed.saturating_add(1);
                    // Try to coalesce more TextDelta events (up to remaining budget)
                    while processed < max_events {
                        let active = match &mut self.state {
                            OperationState::Streaming(active) => active,
                            _ => break,
                        };
                        match active.message_mut().try_recv_event() {
                            Ok(StreamEvent::TextDelta(more_text)) => {
                                text.push_str(&more_text);
                                processed = processed.saturating_add(1);
                            }
                            Ok(other) => {
                                // Non-TextDelta event - save for next iteration
                                pending_event = Some(other);
                                break;
                            }
                            Err(_) => break, // Empty or disconnected
                        }
                    }
                    // Sanitize untrusted model output to prevent terminal injection
                    (
                        StreamEvent::TextDelta(sanitize_terminal_text(&text).into_owned()),
                        true, // Already counted in budget
                    )
                }
                StreamEvent::ThinkingDelta(mut thinking) => {
                    processed = processed.saturating_add(1);
                    while processed < max_events {
                        let active = match &mut self.state {
                            OperationState::Streaming(active) => active,
                            _ => break,
                        };
                        match active.message_mut().try_recv_event() {
                            Ok(StreamEvent::ThinkingDelta(more_thinking)) => {
                                thinking.push_str(&more_thinking);
                                processed = processed.saturating_add(1);
                            }
                            Ok(other) => {
                                pending_event = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    (
                        StreamEvent::ThinkingDelta(sanitize_terminal_text(&thinking).into_owned()),
                        true,
                    )
                }
                other => {
                    let sanitized = match other {
                        StreamEvent::Error(msg) => {
                            StreamEvent::Error(security::sanitize_stream_error(&msg))
                        }
                        other => other,
                    };
                    (sanitized, false) // Not yet counted in budget
                }
            };

            let mut journal_error: Option<String> = None;
            let mut finish_reason: Option<StreamFinishReason> = None;

            let mut active = match std::mem::replace(&mut self.state, OperationState::Idle) {
                OperationState::Streaming(active) => active,
                other => {
                    self.state = other;
                    return;
                }
            };

            let persist_result = match &event {
                StreamEvent::TextDelta(text) => active
                    .journal_mut()
                    .append_text(&mut self.stream_journal, text.clone()),
                StreamEvent::ThinkingDelta(_)
                | StreamEvent::ThinkingSignature(_)
                | StreamEvent::Usage(_) => {
                    // Don't persist thinking or usage to journal - silently consume
                    Ok(())
                }
                StreamEvent::ToolCallStart { .. } | StreamEvent::ToolCallDelta { .. } => Ok(()),
                StreamEvent::Done => active.journal_mut().append_done(&mut self.stream_journal),
                StreamEvent::Error(msg) => active
                    .journal_mut()
                    .append_error(&mut self.stream_journal, msg.clone()),
            };

            // Persist BEFORE display.
            if let Err(e) = persist_result {
                journal_error = Some(e.to_string());
            }

            if journal_error.is_none() {
                match &event {
                    StreamEvent::ToolCallStart {
                        id,
                        name,
                        thought_signature,
                    } => {
                        // Ensure we're in Journaled state (transition if Transient)
                        if matches!(&active, ActiveStream::Transient { .. }) {
                            match self
                                .tool_journal
                                .begin_streaming_batch(active.journal().model_name())
                            {
                                Ok(batch_id) => {
                                    active = active.transition_to_journaled(batch_id);
                                }
                                Err(e) => journal_error = Some(e.to_string()),
                            }
                        }
                        // Record tool call if journaled
                        if let ActiveStream::Journaled { tool_batch_id, .. } = &active {
                            let batch_id = *tool_batch_id;
                            let seq = active.tool_call_seq();
                            active.increment_tool_call_seq();
                            if let Err(e) = self.tool_journal.record_call_start(
                                batch_id,
                                seq,
                                id,
                                name,
                                thought_signature,
                            ) {
                                journal_error = Some(e.to_string());
                            }
                        }
                    }
                    StreamEvent::ToolCallDelta { id, arguments } => {
                        if let ActiveStream::Journaled { tool_batch_id, .. } = &active {
                            let batch_id = *tool_batch_id;
                            // Check size cap before appending to journal
                            let max_bytes = self.tool_settings.limits.max_tool_args_bytes;
                            let current_bytes = active
                                .tool_args_journal_bytes_mut()
                                .get(id)
                                .copied()
                                .unwrap_or(0);
                            let new_total = current_bytes.saturating_add(arguments.len());

                            if new_total <= max_bytes {
                                // Under limit: append to journal and update tracker
                                if let Err(e) =
                                    self.tool_journal.append_call_args(batch_id, id, arguments)
                                {
                                    journal_error = Some(e.to_string());
                                } else {
                                    active
                                        .tool_args_journal_bytes_mut()
                                        .insert(id.clone(), new_total);
                                }
                            }
                            // Over limit: silently skip appending to journal
                            // (StreamingMessage will mark as exceeded via apply_event)
                        }
                    }
                    _ => {}
                }
            }

            if journal_error.is_none() {
                finish_reason = active.message_mut().apply_event(event);
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

                let (message, journal, abort_handle, tool_batch_id, turn) = active.into_parts();

                abort_handle.abort();

                // Discard any in-progress tool batch to prevent stale recovery
                if let Some(batch_id) = tool_batch_id
                    && let Err(e) = self.tool_journal.discard_batch(batch_id)
                {
                    tracing::warn!("Failed to discard tool batch on stream error: {e}");
                }

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
                self.push_notification(format!("Journal append failed: {err}"));
                // Clean up turn state (pending_user_message, tool_iterations, etc.)
                self.finish_turn(turn);
                return;
            }

            if let Some(reason) = finish_reason {
                self.finish_streaming(reason);
                return;
            }

            // Increment processed only if not already counted (e.g., by TextDelta coalescing)
            if !already_counted {
                processed = processed.saturating_add(1);
            }
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

        let (mut message, journal, abort_handle, tool_batch_id, turn) = active.into_parts();

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
        let stream_usage = message.usage();

        // Aggregate API usage for this turn
        if stream_usage.has_data() {
            let turn_usage = self.turn_usage.get_or_insert_with(Default::default);
            turn_usage.record_call(stream_usage);
        }

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
            let error_msg = format_stream_error(model.provider(), model.as_str(), &err);
            let system_msg = Message::system(error_msg);
            self.push_local_message(system_msg);
            self.finish_turn(turn);
            return;
        }

        // Only process tool calls when stream completed successfully (Done)
        if message.has_tool_calls() {
            // Capture thinking before returning so signatures survive tool-call turns.
            let thinking_content = message.thinking().to_owned();
            let thinking_signature = message.thinking_signature_state().clone();
            let thinking_message =
                build_thinking_message(model.clone(), thinking_content, thinking_signature);
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
                turn,
                thinking_message,
            );
            return;
        }

        // Capture thinking content and signature before consuming the streaming message.
        // Thinking is stored separately for UI toggles and signature replay.
        let thinking_content = message.thinking().to_owned();
        let thinking_signature = message.thinking_signature_state().clone();

        // Convert streaming message to completed message (empty content is invalid).
        let Some(assistant_message) = message.into_message().ok() else {
            // Stream completed successfully but with empty content - unusual but not an error
            self.pending_user_message = None;
            let empty_badge = NonEmptyString::try_from(EMPTY_RESPONSE_BADGE)
                .expect("EMPTY_RESPONSE_BADGE must be non-empty");
            let empty_msg = Message::assistant(model.clone(), empty_badge);
            // Still push thinking if we captured any before the empty response
            if let Ok(thinking) = NonEmptyString::new(thinking_content) {
                let thinking_msg = match &thinking_signature {
                    ThoughtSignatureState::Signed(sig) => {
                        Message::thinking_with_signature(model, thinking, sig.as_str().to_string())
                    }
                    ThoughtSignatureState::Unsigned => Message::thinking(model, thinking),
                };
                self.push_local_message(thinking_msg);
            }
            self.push_local_message(empty_msg);
            // Empty response - discard the step (nothing to recover)
            self.discard_journal_step(step_id);
            self.finish_turn(turn);
            return;
        };

        // Stream completed successfully with content
        self.pending_user_message = None;

        // Push thinking message first (if any), then assistant message
        let has_thinking_signature = thinking_signature.is_signed();
        if let Some(thinking_msg) =
            build_thinking_message(model, thinking_content, thinking_signature)
        {
            if has_thinking_signature {
                self.push_history_message(thinking_msg);
            } else {
                self.push_local_message(thinking_msg);
            }
        }
        self.commit_history_message(assistant_message, step_id);
        self.finish_turn(turn);
    }

    /// Inject pending system notifications as an assistant message.
    ///
    /// This takes all queued notifications, formats them, and appends them
    /// as an assistant message at the tail of the message list. Because
    /// assistant messages can only come from API responses or Forge's injection
    /// layer, this creates a trusted channel for system-level communication
    /// that cannot be forged by user input.
    ///
    /// Cache impact: None - injection at tail preserves cache prefix.
    fn inject_pending_notifications(
        &mut self,
        mut messages: Vec<CacheableMessage>,
    ) -> Vec<CacheableMessage> {
        let notifications = self.notification_queue.take();
        if notifications.is_empty() {
            return messages;
        }

        let combined = notifications
            .iter()
            .map(notifications::SystemNotification::format)
            .collect::<Vec<_>>()
            .join("\n");

        if let Ok(content) = NonEmptyString::new(combined) {
            let msg = Message::assistant(self.model.clone(), content);
            messages.push(CacheableMessage::plain(msg)); // tail = cache-safe
        }

        messages
    }
}

/// Get an existing valid Gemini cache or create a new one.
///
/// This function checks if there's a valid (non-expired, matching) cache.
/// If not, it creates a new cache via the Gemini API and stores it.
///
/// Note: Tools must be included in the cache because Gemini's API doesn't allow
/// specifying `tools` in GenerateContent when using cached content.
async fn get_or_create_gemini_cache(
    cache_arc: &std::sync::Arc<tokio::sync::Mutex<Option<GeminiCache>>>,
    config: &GeminiCacheConfig,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    tools: Option<&[ToolDefinition]>,
) -> Option<GeminiCache> {
    // First, check if we have a valid cache
    {
        let guard = cache_arc.lock().await;
        if let Some(cache) = guard.as_ref() {
            if !cache.is_expired() && cache.matches_config(system_prompt, tools) {
                tracing::debug!("Using existing Gemini cache: {}", cache.name);
                return Some(cache.clone());
            }
            tracing::debug!(
                "Gemini cache invalid (expired: {}, config mismatch: {})",
                cache.is_expired(),
                !cache.matches_config(system_prompt, tools)
            );
        }
    }

    // Cache is invalid or doesn't exist - create a new one
    tracing::info!("Creating new Gemini cache for system prompt and tools");
    match forge_providers::gemini::create_cache(
        api_key,
        model,
        system_prompt,
        tools,
        config.ttl_seconds,
    )
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

#[cfg(test)]
mod cache_breakpoint_tests {
    use super::cache_breakpoint_positions;

    #[test]
    fn empty_eligible_returns_no_breakpoints() {
        assert_eq!(cache_breakpoint_positions(0), Vec::<usize>::new());
    }

    #[test]
    fn small_eligible_below_first_grid_point() {
        assert_eq!(cache_breakpoint_positions(3), Vec::<usize>::new());
    }

    #[test]
    fn single_grid_point_fits() {
        // eligible=8: grid points 3 and 7 fit
        assert_eq!(cache_breakpoint_positions(8), vec![3, 7]);
    }

    #[test]
    fn three_grid_points_for_medium_conversation() {
        // eligible=100: grid points ≤99 are [3,7,15,23,31,47,63,95], last 3 = [47,63,95]
        assert_eq!(cache_breakpoint_positions(100), vec![47, 63, 95]);
    }

    #[test]
    fn stable_across_small_growth() {
        let base = cache_breakpoint_positions(100);
        assert_eq!(cache_breakpoint_positions(105), base);
        assert_eq!(cache_breakpoint_positions(110), base);
        assert_eq!(cache_breakpoint_positions(120), base);
        assert_eq!(cache_breakpoint_positions(127), base);
    }

    #[test]
    fn shifts_at_grid_boundary() {
        let before = cache_breakpoint_positions(127);
        let after = cache_breakpoint_positions(128);
        assert_eq!(before, vec![47, 63, 95]);
        assert_eq!(after, vec![63, 95, 127]);
        // Two positions survive the transition
        assert_eq!(before[1], after[0]);
        assert_eq!(before[2], after[1]);
    }

    #[test]
    fn large_conversation_coverage() {
        // 500 messages: highest fitting grid point is 383
        let positions = cache_breakpoint_positions(500);
        assert_eq!(positions, vec![191, 255, 383]);
    }

    #[test]
    fn max_grid_coverage() {
        let positions = cache_breakpoint_positions(2000);
        assert_eq!(positions, vec![511, 767, 1023]);
    }
}

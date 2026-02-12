//! Streaming response handling for the App.

use std::sync::OnceLock;
use std::time::Duration;

use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc;

const STREAM_EVENT_CHANNEL_CAPACITY: usize = 1024;
const REDACTED_THINKING_PLACEHOLDER: &str = "[Thinking hidden]";

const DEFAULT_TOOL_ARGS_JOURNAL_FLUSH_BYTES: usize = 8_192;
const DEFAULT_TOOL_ARGS_JOURNAL_FLUSH_INTERVAL_MS: u64 = 250;

fn tool_args_journal_flush_bytes() -> usize {
    static THRESHOLD: OnceLock<usize> = OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("FORGE_TOOL_JOURNAL_FLUSH_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_TOOL_ARGS_JOURNAL_FLUSH_BYTES)
    })
}

fn tool_args_journal_flush_interval() -> Duration {
    static INTERVAL: OnceLock<Duration> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::env::var("FORGE_TOOL_JOURNAL_FLUSH_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(
                DEFAULT_TOOL_ARGS_JOURNAL_FLUSH_INTERVAL_MS,
            ))
    })
}

use forge_context::{BeginSessionError, TokenCounter};
use forge_types::{CacheBudget, CacheHint, ModelName, Provider, ToolDefinition};

use super::{
    ABORTED_JOURNAL_BADGE, ActiveStream, CacheableMessage, ContextBuildError,
    DEFAULT_STREAM_EVENT_BUDGET, DistillationStart, EMPTY_RESPONSE_BADGE, GeminiCache,
    GeminiCacheConfig, Message, NonEmptyString, OperationState, QueuedUserMessage, StreamEvent,
    StreamFinishReason, StreamingMessage, ThinkingReplayState, notifications,
    sanitize_terminal_text, security,
};
use crate::errors::format_stream_error;

pub(crate) fn build_thinking_message(
    model: ModelName,
    content: String,
    replay: ThinkingReplayState,
) -> Option<Message> {
    let sanitized = security::sanitize_display_text(&content);
    if let Ok(thinking) = NonEmptyString::new(sanitized) {
        return Some(match replay {
            ThinkingReplayState::ClaudeSigned { signature } => {
                Message::thinking_with_signature(model, thinking, signature.as_str().to_string())
            }
            ThinkingReplayState::OpenAIReasoning { items } => {
                Message::thinking_with_openai_reasoning(model, thinking, items)
            }
            ThinkingReplayState::Unsigned | ThinkingReplayState::Unknown => {
                Message::thinking(model, thinking)
            }
        });
    }

    if replay.requires_persistence() {
        let placeholder = NonEmptyString::try_from(REDACTED_THINKING_PLACEHOLDER)
            .expect("REDACTED_THINKING_PLACEHOLDER must be non-empty");
        return Some(match replay {
            ThinkingReplayState::ClaudeSigned { signature } => {
                Message::thinking_with_signature(model, placeholder, signature.as_str().to_string())
            }
            ThinkingReplayState::OpenAIReasoning { items } => {
                Message::thinking_with_openai_reasoning(model, placeholder, items)
            }
            ThinkingReplayState::Unsigned | ThinkingReplayState::Unknown => unreachable!(),
        });
    }

    None
}

/// Minimum token count for a content section to be worth caching.
/// Claude's minimum cacheable prefix is 1024 tokens, but smaller sections
/// waste a slot for negligible savings. 4096 is a practical threshold.
const MIN_CACHEABLE_TOKENS: u32 = 4096;

/// Token step for placing message breakpoints. Breakpoints are placed at
/// cumulative-token boundaries that are multiples of this value, so they
/// shift only when a boundary is crossed — not on every new message.
const CACHE_TOKEN_STEP: u32 = 4096;

/// A fully resolved cache slot allocation for a Claude API request.
///
/// Produced solely by `plan_cache_allocation`. The sum of allocated slots
/// is bounded by `CacheBudget::MAX` (4) by construction.
pub(crate) struct CachePlan {
    pub cache_system: bool,
    pub cache_tools: bool,
    pub message_breakpoints: Vec<usize>,
}

/// Allocate cache slots across system prompt, tools, and messages.
///
/// Policy: system and tools each get a slot only if they exceed
/// `MIN_CACHEABLE_TOKENS`. Remaining budget goes to message breakpoints
/// placed at `CACHE_TOKEN_STEP` boundaries.
fn plan_cache_allocation(
    budget: CacheBudget,
    system_tokens: u32,
    tool_tokens: u32,
    message_tokens: &[u32],
) -> CachePlan {
    let mut budget = budget;
    let mut cache_system = false;
    let mut cache_tools = false;

    if system_tokens >= MIN_CACHEABLE_TOKENS
        && let Some(b) = budget.take_one()
    {
        budget = b;
        cache_system = true;
    }

    if tool_tokens >= MIN_CACHEABLE_TOKENS
        && let Some(b) = budget.take_one()
    {
        budget = b;
        cache_tools = true;
    }

    // Remaining budget goes to message breakpoints
    let message_breakpoints = select_token_breakpoints(budget.remaining() as usize, message_tokens);

    CachePlan {
        cache_system,
        cache_tools,
        message_breakpoints,
    }
}

/// Select message breakpoint indices at cumulative token-step boundaries.
///
/// Excludes the last message (still evolving). Places breakpoints where
/// cumulative tokens cross multiples of `CACHE_TOKEN_STEP`. Takes the
/// last `max_breakpoints` such indices for maximum prefix coverage.
fn select_token_breakpoints(max_breakpoints: usize, message_tokens: &[u32]) -> Vec<usize> {
    if max_breakpoints == 0 || message_tokens.is_empty() {
        return Vec::new();
    }

    // Exclude last message (still evolving)
    let eligible = message_tokens.len().saturating_sub(1);
    if eligible == 0 {
        return Vec::new();
    }

    let mut cumulative: u32 = 0;
    let mut boundary_indices: Vec<usize> = Vec::new();
    let mut next_boundary = CACHE_TOKEN_STEP;

    for (i, &tokens) in message_tokens[..eligible].iter().enumerate() {
        cumulative = cumulative.saturating_add(tokens);
        while cumulative >= next_boundary {
            boundary_indices.push(i);
            next_boundary = next_boundary.saturating_add(CACHE_TOKEN_STEP);
        }
    }

    // Take the last `max_breakpoints` — deepest stable breakpoints
    // Deduplicate: multiple boundaries can land on the same message index
    boundary_indices.dedup();
    let start = boundary_indices.len().saturating_sub(max_breakpoints);
    boundary_indices[start..].to_vec()
}

impl super::App {
    /// Estimate token overhead from system prompt and tool definitions for a provider.
    ///
    /// Both `start_streaming` and `try_start_distillation` must use the same overhead
    /// when checking context budget, otherwise distillation can loop: `try_start_distillation`
    /// declares "fits" (with 0 overhead) while `start_streaming` declares "doesn't fit"
    /// (with real overhead), causing an infinite ping-pong.
    pub(crate) fn streaming_overhead(&self, provider: Provider) -> u32 {
        let counter = TokenCounter::new();
        let assembled = crate::environment::assemble_prompt(
            self.system_prompts.get(provider),
            &self.environment,
            self.model.as_str(),
        );
        let sys_tokens = counter.count_str(&assembled);
        let tool_tokens = {
            let tools: Vec<&ToolDefinition> = self
                .tool_definitions
                .iter()
                .filter(|t| t.provider.is_none() || t.provider == Some(provider))
                .collect();
            if tools.is_empty() {
                0
            } else {
                match serde_json::to_string(&tools) {
                    Ok(s) => counter.count_str(&s),
                    Err(_) => 0,
                }
            }
        };
        sys_tokens + tool_tokens
    }

    pub fn start_streaming(&mut self, queued: QueuedUserMessage) {
        if self.busy_reason().is_some() {
            return;
        }

        let QueuedUserMessage { config, turn } = queued;
        let memory_enabled = self.memory_enabled();

        let provider = config.provider();
        let overhead = self.streaming_overhead(provider);

        let system_prompt = crate::environment::assemble_prompt(
            self.system_prompts.get(provider),
            &self.environment,
            config.model().as_str(),
        );
        let tools: Vec<_> = self
            .tool_definitions
            .iter()
            .filter(|t| t.provider.is_none() || t.provider == Some(provider))
            .cloned()
            .collect();

        // When memory enabled, use distillation-based context management.
        // Otherwise, use basic mode.
        let api_messages = if memory_enabled {
            match self.context_manager.prepare(overhead) {
                Ok(prepared) => prepared.api_messages(),
                Err(ContextBuildError::CompactionNeeded) => {
                    let queued = QueuedUserMessage { config, turn };
                    let start_result = self.try_start_distillation(Some(queued));
                    if !matches!(start_result, DistillationStart::Started) {
                        self.push_notification("Cannot start: compaction did not start");
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
                if recovered.is_some()
                    || matches!(
                        self.state,
                        OperationState::ToolRecovery(_) | OperationState::RecoveryBlocked(_)
                    )
                {
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

        // Plan cache allocation and convert messages to cacheable format
        let cache_enabled = self.cache_enabled;
        let (cacheable_messages, system_cache_hint, cache_last_tool): (
            Vec<CacheableMessage>,
            CacheHint,
            bool,
        ) = if cache_enabled && provider == Provider::Claude {
            let counter = TokenCounter::new();
            let sys_tokens = counter.count_str(&system_prompt);
            let tool_tokens = if tools.is_empty() {
                0
            } else {
                match serde_json::to_string(&tools) {
                    Ok(s) => counter.count_str(&s),
                    Err(_) => 0,
                }
            };
            let msg_tokens: Vec<u32> = api_messages
                .iter()
                .map(|m| counter.count_message(m))
                .collect();

            let plan =
                plan_cache_allocation(CacheBudget::full(), sys_tokens, tool_tokens, &msg_tokens);

            let msgs = api_messages
                .into_iter()
                .enumerate()
                .map(|(i, msg)| {
                    if plan.message_breakpoints.contains(&i) {
                        CacheableMessage::cached(msg)
                    } else {
                        CacheableMessage::plain(msg)
                    }
                })
                .collect();

            let sys_hint = if plan.cache_system {
                CacheHint::Ephemeral
            } else {
                CacheHint::Default
            };

            (msgs, sys_hint, plan.cache_tools)
        } else {
            let msgs = api_messages
                .into_iter()
                .map(CacheableMessage::plain)
                .collect();
            (msgs, CacheHint::Default, false)
        };

        // Inject any pending system notifications as an assistant message
        let cacheable_messages = self.inject_pending_notifications(cacheable_messages);

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
                    &system_prompt,
                    tools_ref,
                )
                .await
            } else {
                None
            };

            let result = forge_providers::send_message(forge_providers::SendMessageRequest {
                config: &config,
                messages: &cacheable_messages,
                limits,
                system_prompt: Some(&system_prompt),
                tools: tools_ref,
                system_cache_hint,
                cache_last_tool,
                gemini_cache: gemini_cache.as_ref(),
                tx: tx.clone(),
            })
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

            let idle = self.idle_state();
            let mut active = match std::mem::replace(&mut self.state, idle) {
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
                | StreamEvent::OpenAIReasoningDone { .. }
                | StreamEvent::Usage(_)
                | StreamEvent::ToolCallStart { .. }
                | StreamEvent::ToolCallDelta { .. } => Ok(()),
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
                            match self.tool_journal.begin_streaming_batch(
                                active.journal().step_id(),
                                active.journal().model_name(),
                            ) {
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
                                // Under limit: buffer deltas and flush periodically to avoid
                                // per-delta SQLite UPDATEs (perf + UI responsiveness).
                                active
                                    .tool_args_journal_bytes_mut()
                                    .insert(id.clone(), new_total);

                                let pending_flush =
                                    if let Some(buffer) = active.tool_args_buffer_mut() {
                                        buffer.push_delta(id, arguments);
                                        if buffer.should_flush(
                                            tool_args_journal_flush_bytes(),
                                            tool_args_journal_flush_interval(),
                                        ) {
                                            buffer.take_pending()
                                        } else {
                                            Vec::new()
                                        }
                                    } else {
                                        Vec::new()
                                    };

                                if !pending_flush.is_empty()
                                    && let Err(e) = self
                                        .tool_journal
                                        .append_call_args_batch(batch_id, pending_flush)
                                {
                                    journal_error = Some(e.to_string());
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
        let mut active = match self.replace_with_idle() {
            OperationState::Streaming(active) => active,
            other => {
                self.state = other;
                return;
            }
        };

        // Flush any buffered tool-argument deltas before we can execute tools.
        // This preserves the "journal-before-execute" invariant for crash recovery
        // even when the engine buffers tool-call deltas for performance.
        if matches!(finish_reason, StreamFinishReason::Done)
            && let ActiveStream::Journaled {
                tool_batch_id,
                tool_args_buffer,
                ..
            } = &mut active
        {
            let pending = tool_args_buffer.take_pending();
            if !pending.is_empty()
                && let Err(e) = self
                    .tool_journal
                    .append_call_args_batch(*tool_batch_id, pending)
            {
                // Fail closed: if we can't persist tool arguments, refuse tool execution.
                self.disable_tools_due_to_tool_journal_error("flush tool args", e);
            }
        }

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
            let thinking_content = message.thinking().to_owned();
            let thinking_replay = message.thinking_replay_state().clone();
            let thinking_message =
                build_thinking_message(model.clone(), thinking_content, thinking_replay);
            let parsed = message.take_tool_calls();
            let assistant_text = message.content().to_string();
            // NOTE: We do NOT clear pending_user_message here because:
            // 1. The user message was already committed to history
            // 2. We need the user query for Librarian extraction when the turn completes
            // 3. rollback_pending_user_message() safely fails if it's not the last message
            self.handle_tool_calls(crate::state::ToolLoopInput {
                assistant_text,
                thinking_message,
                calls: parsed.calls,
                pre_resolved: parsed.pre_resolved,
                model,
                step_id,
                tool_batch_id,
                turn,
            });
            return;
        }

        let thinking_content = message.thinking().to_owned();
        let thinking_replay = message.thinking_replay_state().clone();

        let Some(assistant_message) = message.into_message().ok() else {
            self.pending_user_message = None;
            let empty_badge = NonEmptyString::try_from(EMPTY_RESPONSE_BADGE)
                .expect("EMPTY_RESPONSE_BADGE must be non-empty");
            let empty_msg = Message::assistant(model.clone(), empty_badge);
            if let Ok(thinking) = NonEmptyString::new(thinking_content) {
                let thinking_msg = match &thinking_replay {
                    ThinkingReplayState::ClaudeSigned { signature } => {
                        Message::thinking_with_signature(
                            model,
                            thinking,
                            signature.as_str().to_string(),
                        )
                    }
                    ThinkingReplayState::OpenAIReasoning { items } => {
                        Message::thinking_with_openai_reasoning(model, thinking, items.clone())
                    }
                    ThinkingReplayState::Unsigned | ThinkingReplayState::Unknown => {
                        Message::thinking(model, thinking)
                    }
                };
                self.push_local_message(thinking_msg);
            }
            self.push_local_message(empty_msg);
            self.discard_journal_step(step_id);
            self.finish_turn(turn);
            return;
        };

        self.pending_user_message = None;

        let requires_persistence = thinking_replay.requires_persistence();
        if let Some(thinking_msg) = build_thinking_message(model, thinking_content, thinking_replay)
        {
            if requires_persistence {
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

        // Sanitize before injection — DiagnosticsFound contains workspace-derived
        // content (file paths, error messages) that could carry escape sequences
        // or bidi controls from malicious filenames.
        let combined = crate::security::sanitize_display_text(&combined);

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
mod token_cache_planner_tests {
    use super::*;
    use forge_types::CacheBudget;

    #[test]
    fn empty_messages_no_breakpoints() {
        let plan = plan_cache_allocation(CacheBudget::full(), 0, 0, &[]);
        assert!(!plan.cache_system);
        assert!(!plan.cache_tools);
        assert!(plan.message_breakpoints.is_empty());
    }

    #[test]
    fn below_min_tokens_no_breakpoints() {
        let plan = plan_cache_allocation(CacheBudget::full(), 100, 100, &[100, 200, 300]);
        assert!(!plan.cache_system);
        assert!(!plan.cache_tools);
        assert!(plan.message_breakpoints.is_empty());
    }

    #[test]
    fn system_skipped_when_small() {
        // System < 4096, tools >= 4096
        let plan = plan_cache_allocation(CacheBudget::full(), 1000, 5000, &[2000, 2000, 2000]);
        assert!(!plan.cache_system);
        assert!(plan.cache_tools);
    }

    #[test]
    fn tools_skipped_when_small() {
        // System >= 4096, tools < 4096
        let plan = plan_cache_allocation(CacheBudget::full(), 5000, 1000, &[2000, 2000, 2000]);
        assert!(plan.cache_system);
        assert!(!plan.cache_tools);
    }

    #[test]
    fn budget_exhausted_by_system_and_tools() {
        // System + tools use 2 slots, leaving 2 for messages
        // 8 messages × 1000 tokens = 8000 tokens → boundaries at 4096 and 8192
        // Only 4096 boundary exists (8000 < 8192), so 1 breakpoint
        let plan = plan_cache_allocation(
            CacheBudget::full(),
            5000,
            5000,
            &[1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000],
        );
        assert!(plan.cache_system);
        assert!(plan.cache_tools);
        // Cumulative at eligible[0..8]: 1000,2000,3000,4000,5000,6000,7000,8000
        // 4096 boundary crossed at index 3 (cumulative=4000→nope, 4th=4000 still < 4096)
        // Actually: index 4 → cumulative=5000 >= 4096 → breakpoint at 4
        // 8192 boundary: index 7 → cumulative=8000 < 8192 → no
        assert_eq!(plan.message_breakpoints, vec![4]);
    }

    #[test]
    fn single_step_boundary() {
        // 5 messages of 1000 tokens each, last excluded
        // Cumulative eligible[0..4]: 1000, 2000, 3000, 4000
        // 4096 boundary NOT crossed (4000 < 4096)
        let plan =
            plan_cache_allocation(CacheBudget::full(), 0, 0, &[1000, 1000, 1000, 1000, 1000]);
        assert!(plan.message_breakpoints.is_empty());

        // 6 messages of 1000 tokens each, last excluded
        // Cumulative eligible[0..5]: 1000, 2000, 3000, 4000, 5000
        // 4096 boundary crossed at index 4 (cumulative=5000)
        let plan = plan_cache_allocation(
            CacheBudget::full(),
            0,
            0,
            &[1000, 1000, 1000, 1000, 1000, 1000],
        );
        assert_eq!(plan.message_breakpoints, vec![4]);
    }

    #[test]
    fn stable_across_small_growth() {
        // Same breakpoints when adding small messages that don't cross a boundary
        let base = plan_cache_allocation(CacheBudget::full(), 0, 0, &[2000, 2000, 2000, 100]);
        let grown = plan_cache_allocation(CacheBudget::full(), 0, 0, &[2000, 2000, 2000, 100, 100]);
        assert_eq!(base.message_breakpoints, grown.message_breakpoints);
    }

    #[test]
    fn shifts_at_step_boundary() {
        // 10 messages × 2048 tokens, last excluded → 9 eligible
        // Cumulative: 2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432
        // Boundaries at: index 1 (4096), 2 (→8192? no, 6144<8192), 3 (8192), 5 (12288→12288=3×4096), etc.
        // Let's be precise: boundary crossing at multiples of 4096
        // idx0: 2048 (<4096)
        // idx1: 4096 (>=4096) → breakpoint, next_boundary=8192
        // idx2: 6144 (<8192)
        // idx3: 8192 (>=8192) → breakpoint, next_boundary=12288
        // idx4: 10240 (<12288)
        // idx5: 12288 (>=12288) → breakpoint, next_boundary=16384
        // idx6: 14336 (<16384)
        // idx7: 16384 (>=16384) → breakpoint, next_boundary=20480
        // idx8: 18432 (<20480)
        // Boundary indices: [1, 3, 5, 7], last 4 = [1, 3, 5, 7]
        let tokens: Vec<u32> = vec![2048; 10];
        let plan = plan_cache_allocation(CacheBudget::full(), 0, 0, &tokens);
        assert_eq!(plan.message_breakpoints, vec![1, 3, 5, 7]);
    }

    #[test]
    fn total_slots_never_exceed_four() {
        // System + tools + 4 message breakpoints would be 6, but budget caps at 4
        let tokens: Vec<u32> = vec![4096; 20];
        let plan = plan_cache_allocation(CacheBudget::full(), 5000, 5000, &tokens);
        let total =
            plan.cache_system as u8 + plan.cache_tools as u8 + plan.message_breakpoints.len() as u8;
        assert!(total <= CacheBudget::MAX);
    }
}

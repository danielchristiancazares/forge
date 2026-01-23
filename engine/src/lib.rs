//! Core engine for Forge - state machine and orchestration.
//!
//! This crate contains the App state machine without TUI dependencies.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

// UI types - separated for clarity
mod ui;
use ui::InputState;
pub use ui::{
    DisplayItem, InputMode, ModalEffect, ModalEffectKind, PredefinedModel, ScrollState, UiOptions,
    ViewState,
};

// Re-export from crates for public API
pub use forge_context::{
    ActiveJournal, ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus,
    ExtractionResult, Fact, FactType, FullHistory, Librarian, MessageId, ModelLimits,
    ModelLimitsSource, ModelRegistry, PendingSummarization, PreparedContext, RecoveredStream,
    RecoveredToolBatch, RetrievalResult, StreamJournal, SummarizationNeeded, SummarizationScope,
    TokenCounter, ToolBatchId, ToolJournal, extract_facts, format_facts_for_context,
    generate_summary, retrieve_relevant, summarization_model,
};
pub use forge_providers::{self, ApiConfig, gemini::GeminiCache, gemini::GeminiCacheConfig};
pub use forge_types::{
    ApiKey, CacheHint, CacheableMessage, EmptyStringError, Message, ModelName, ModelNameKind,
    NonEmptyStaticStr, NonEmptyString, OpenAIReasoningEffort, OpenAIRequestOptions,
    OpenAITextVerbosity, OpenAITruncation, OutputLimits, Provider, StreamEvent, StreamFinishReason,
    ToolCall, ToolDefinition, ToolResult, sanitize_terminal_text,
};

// Config types - passed in from caller
mod config;
pub use config::{AppConfig, ForgeConfig};

mod tools;

mod commands;
mod errors;
mod init;
mod input_modes;
mod persistence;
mod security;
mod state;
mod streaming;
mod summarization;
mod tool_loop;
mod util;

// Re-export input mode types
pub use input_modes::{
    CommandMode, CommandToken, EnteredCommand, InsertMode, InsertToken, QueuedUserMessage,
};

// Re-export command metadata used by UI
pub use commands::{CommandSpec, command_help_summary, command_specs};

// Re-export persistence constants used by streaming module
pub(crate) use persistence::{ABORTED_JOURNAL_BADGE, EMPTY_RESPONSE_BADGE};

// Re-export init constants used by tool_loop module
pub(crate) use init::{
    DEFAULT_TOOL_CAPACITY_BYTES, TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

/// Maximum number of stream events to process per UI tick.
pub const DEFAULT_STREAM_EVENT_BUDGET: usize = 512;

// Re-export public state types
pub use state::SummarizationTask;

// Internal state imports
use state::{
    ActiveStream, DataDir, OperationState, SummarizationRetry, SummarizationRetryState,
    SummarizationRetryWithQueuedState, SummarizationStart, SummarizationState,
    SummarizationWithQueuedState, ToolLoopPhase, ToolRecoveryDecision,
};

// ============================================================================
// StreamingMessage - async message being streamed
// ============================================================================

/// Accumulator for a single tool call during streaming.
///
/// As tool call events arrive, we accumulate the JSON arguments string
/// until the stream completes, then parse into a complete `ToolCall`.
#[derive(Debug, Clone)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments_json: String,
    thought_signature: Option<String>,
    args_exceeded: bool,
}

#[derive(Debug)]
struct ParsedToolCalls {
    calls: Vec<ToolCall>,
    pre_resolved: Vec<ToolResult>,
}

/// A message being streamed - existence proves streaming is active.
/// Typestate: consuming this produces a complete assistant `Message`.
#[derive(Debug)]
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    receiver: mpsc::Receiver<StreamEvent>,
    /// Accumulated tool calls during streaming.
    tool_calls: Vec<ToolCallAccumulator>,
    max_tool_args_bytes: usize,
}

impl StreamingMessage {
    #[must_use]
    pub fn new(
        model: ModelName,
        receiver: mpsc::Receiver<StreamEvent>,
        max_tool_args_bytes: usize,
    ) -> Self {
        Self {
            model,
            content: String::new(),
            receiver,
            tool_calls: Vec::new(),
            max_tool_args_bytes,
        }
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    #[must_use]
    pub fn model_name(&self) -> &ModelName {
        &self.model
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn try_recv_event(&mut self) -> Result<StreamEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn apply_event(&mut self, event: StreamEvent) -> Option<StreamFinishReason> {
        match event {
            StreamEvent::TextDelta(text) => {
                self.content.push_str(&text);
                None
            }
            StreamEvent::ThinkingDelta(_) => {
                // Silently consume thinking content - not displayed for now
                None
            }
            StreamEvent::ToolCallStart {
                id,
                name,
                thought_signature,
            } => {
                self.tool_calls.push(ToolCallAccumulator {
                    id,
                    name,
                    arguments_json: String::new(),
                    thought_signature,
                    args_exceeded: false,
                });
                None
            }
            StreamEvent::ToolCallDelta { id, arguments } => {
                if let Some(acc) = self.tool_calls.iter_mut().find(|t| t.id == id) {
                    if acc.args_exceeded {
                        return None;
                    }
                    let new_len = acc.arguments_json.len().saturating_add(arguments.len());
                    if new_len > self.max_tool_args_bytes {
                        acc.args_exceeded = true;
                        return None;
                    }
                    acc.arguments_json.push_str(&arguments);
                }
                None
            }
            StreamEvent::Done => Some(StreamFinishReason::Done),
            StreamEvent::Error(err) => Some(StreamFinishReason::Error(err)),
        }
    }

    /// Returns true if any tool calls were received during streaming.
    #[must_use]
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Take accumulated tool calls, parsing JSON arguments.
    ///
    /// Returns parsed tool calls plus pre-resolved errors for invalid JSON.
    pub(crate) fn take_tool_calls(&mut self) -> ParsedToolCalls {
        let mut calls = Vec::new();
        let mut pre_resolved = Vec::new();

        for acc in self.tool_calls.drain(..) {
            if acc.args_exceeded {
                pre_resolved.push(ToolResult::error(
                    acc.id.clone(),
                    acc.name.clone(),
                    "Tool arguments exceeded maximum size",
                ));
                calls.push(ToolCall::new_with_thought_signature(
                    acc.id,
                    acc.name,
                    serde_json::Value::Object(serde_json::Map::new()),
                    acc.thought_signature,
                ));
                continue;
            }

            if acc.arguments_json.trim().is_empty() {
                calls.push(ToolCall::new_with_thought_signature(
                    acc.id,
                    acc.name,
                    serde_json::Value::Object(serde_json::Map::new()),
                    acc.thought_signature,
                ));
                continue;
            }

            match serde_json::from_str(&acc.arguments_json) {
                Ok(arguments) => calls.push(ToolCall::new_with_thought_signature(
                    acc.id,
                    acc.name,
                    arguments,
                    acc.thought_signature,
                )),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse tool call arguments for '{}': {}",
                        acc.name,
                        e
                    );
                    pre_resolved.push(ToolResult::error(
                        acc.id.clone(),
                        acc.name.clone(),
                        "Invalid tool arguments JSON",
                    ));
                    calls.push(ToolCall::new_with_thought_signature(
                        acc.id,
                        acc.name,
                        serde_json::Value::Object(serde_json::Map::new()),
                        acc.thought_signature,
                    ));
                }
            }
        }

        ParsedToolCalls {
            calls,
            pre_resolved,
        }
    }

    /// Consume streaming message and produce a complete message.
    pub fn into_message(self) -> Result<Message, forge_types::EmptyStringError> {
        let content = NonEmptyString::new(self.content)?;
        Ok(Message::assistant(self.model, content))
    }
}

// ============================================================================
// Provider-specific system prompts
// ============================================================================

/// Provider-specific system prompts.
///
/// Allows different prompts to be used for different LLM providers.
/// The prompt is selected at streaming time based on the active provider.
#[derive(Debug, Clone, Copy)]
pub struct SystemPrompts {
    /// Default prompt for Claude and OpenAI.
    pub default: &'static str,
    /// Gemini-specific prompt.
    pub gemini: &'static str,
}

impl SystemPrompts {
    /// Get the system prompt for the given provider.
    #[must_use]
    pub fn get(&self, provider: Provider) -> &'static str {
        match provider {
            Provider::Gemini => self.gemini,
            Provider::Claude | Provider::OpenAI => self.default,
        }
    }
}

// ============================================================================
// Constants
// ============================================================================

pub(crate) const MAX_SUMMARIZATION_ATTEMPTS: u8 = 5;
pub(crate) const SUMMARIZATION_RETRY_BASE_MS: u64 = 500;
pub(crate) const SUMMARIZATION_RETRY_MAX_MS: u64 = 8000;
pub(crate) const SUMMARIZATION_RETRY_JITTER_MS: u64 = 200;

/// Application state
pub struct App {
    input: InputState,
    display: Vec<DisplayItem>,
    /// Version counter for display changes - incremented when display items change.
    /// Used by TUI to cache rendered output and avoid rebuilding every frame.
    display_version: usize,
    should_quit: bool,
    /// View state for rendering (scroll, status, modal effects).
    view: ViewState,
    api_keys: HashMap<Provider, String>,
    model: ModelName,
    tick: usize,
    data_dir: DataDir,
    /// Context manager for adaptive context window management.
    context_manager: ContextManager,
    /// Stream journal for crash recovery.
    stream_journal: StreamJournal,
    /// Current operation state.
    state: OperationState,
    /// Whether `ContextInfinity` (automatic summarization) is enabled.
    /// This is determined at init from config/env and does not change during runtime.
    context_infinity: bool,
    /// Validated output limits (max tokens + optional thinking budget).
    /// Invariant: if thinking is enabled, budget < `max_tokens`.
    output_limits: OutputLimits,
    /// Whether prompt caching is enabled (for Claude).
    cache_enabled: bool,
    /// `OpenAI` request defaults (reasoning/verbosity/truncation).
    openai_options: OpenAIRequestOptions,
    /// Provider-specific system prompts.
    /// The correct prompt is selected at streaming time based on the active provider.
    system_prompts: Option<SystemPrompts>,
    /// Cached context usage status (invalidated when history/model changes).
    cached_usage_status: Option<ContextUsageStatus>,
    /// Pending user message awaiting stream completion.
    ///
    /// When a user message is queued, we store its ID and original text here.
    /// If the stream fails with no content, we rollback the message from history
    /// and restore this text to the input box for easy retry.
    pending_user_message: Option<(MessageId, String)>,
    /// Tool definitions to send with each request.
    tool_definitions: Vec<ToolDefinition>,
    /// Tool registry for executors.
    tool_registry: std::sync::Arc<tools::ToolRegistry>,
    /// Tool settings derived from config.
    tool_settings: tools::ToolSettings,
    /// Tool journal for crash recovery.
    tool_journal: ToolJournal,
    /// File hash cache for tool safety checks.
    tool_file_cache: std::sync::Arc<tokio::sync::Mutex<tools::ToolFileCache>>,
    /// Tool iterations used in the current user turn.
    tool_iterations: u32,
    /// Whether we've already warned about a failed history load.
    history_load_warning_shown: bool,
    /// Whether we've already warned about autosave failures.
    autosave_warning_shown: bool,
    /// Active Gemini cache (if caching enabled and cache created).
    /// Uses Arc<Mutex> because cache is created/updated inside async streaming tasks.
    gemini_cache: std::sync::Arc<tokio::sync::Mutex<Option<GeminiCache>>>,
    /// Gemini cache configuration.
    gemini_cache_config: GeminiCacheConfig,
    /// The Librarian for fact extraction and retrieval (Context Infinity).
    /// Uses Arc<Mutex> because it's accessed from async tasks for extraction.
    /// None if context_infinity is disabled or no Gemini API key.
    librarian: Option<std::sync::Arc<tokio::sync::Mutex<Librarian>>>,
}

impl App {
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Check if screen mode toggle was requested and clear the flag.
    pub fn take_toggle_screen_mode(&mut self) -> bool {
        std::mem::take(&mut self.view.toggle_screen_mode)
    }

    /// Request toggling between fullscreen and inline UI modes.
    pub fn request_toggle_screen_mode(&mut self) {
        self.view.toggle_screen_mode = true;
    }

    /// Check if a transcript clear was requested and clear the flag.
    pub fn take_clear_transcript(&mut self) -> bool {
        std::mem::take(&mut self.view.clear_transcript)
    }

    pub fn ui_options(&self) -> UiOptions {
        self.view.ui_options
    }

    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    pub fn model(&self) -> &str {
        self.model.as_str()
    }

    pub fn tick_count(&self) -> usize {
        self.tick
    }

    pub fn history(&self) -> &forge_context::FullHistory {
        self.context_manager.history()
    }

    pub fn streaming(&self) -> Option<&StreamingMessage> {
        match &self.state {
            OperationState::Streaming(active) => Some(&active.message),
            _ => None,
        }
    }

    pub fn tool_loop_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            OperationState::ToolLoop(state) => Some(&state.batch.calls),
            _ => None,
        }
    }

    pub fn tool_loop_execute_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            OperationState::ToolLoop(state) => Some(&state.batch.execute_now),
            _ => None,
        }
    }

    pub fn tool_loop_results(&self) -> Option<&[ToolResult]> {
        match &self.state {
            OperationState::ToolLoop(state) => Some(&state.batch.results),
            _ => None,
        }
    }

    pub fn tool_loop_current_call_id(&self) -> Option<&str> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::Executing(exec) => exec.current_call.as_ref().map(|c| c.id.as_str()),
                ToolLoopPhase::AwaitingApproval(_) => None,
            },
            _ => None,
        }
    }

    pub fn tool_loop_output_lines(&self) -> Option<&[String]> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::Executing(exec) => Some(&exec.output_lines),
                ToolLoopPhase::AwaitingApproval(_) => None,
            },
            _ => None,
        }
    }

    /// Drain the list of files accessed during tool execution.
    ///
    /// This clears the file cache and returns the paths of all accessed files.
    /// Used for linking extracted facts to their source files.
    #[allow(dead_code)] // WIP: will be wired up for librarian extraction
    fn drain_accessed_files(&self) -> Vec<String> {
        if let Ok(mut cache) = self.tool_file_cache.try_lock() {
            let paths: Vec<String> = cache
                .keys()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            cache.clear();
            paths
        } else {
            // Cache is locked by tool execution - skip this time
            tracing::debug!("File cache locked, skipping source tracking");
            Vec::new()
        }
    }

    pub fn tool_approval_requests(&self) -> Option<&[tools::ConfirmationRequest]> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(&approval.requests),
                ToolLoopPhase::Executing(_) => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_selected(&self) -> Option<&[bool]> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(&approval.selected),
                ToolLoopPhase::Executing(_) => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_cursor(&self) -> Option<usize> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(approval.cursor),
                ToolLoopPhase::Executing(_) => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_expanded(&self) -> Option<usize> {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => approval.expanded,
                ToolLoopPhase::Executing(_) => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_deny_confirm(&self) -> bool {
        match &self.state {
            OperationState::ToolLoop(state) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => approval.deny_confirm,
                ToolLoopPhase::Executing(_) => false,
            },
            _ => false,
        }
    }

    pub fn tool_recovery_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            OperationState::ToolRecovery(state) => Some(&state.batch.calls),
            _ => None,
        }
    }

    pub fn tool_recovery_results(&self) -> Option<&[ToolResult]> {
        match &self.state {
            OperationState::ToolRecovery(state) => Some(&state.batch.results),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        // Check if there are no History items (real conversation content)
        // Local items (notifications) don't count towards "empty"
        !self
            .display
            .iter()
            .any(|item| matches!(item, DisplayItem::History(_)))
            && !matches!(
                self.state,
                OperationState::Streaming(_)
                    | OperationState::ToolLoop(_)
                    | OperationState::ToolRecovery(_)
            )
    }

    pub fn display_items(&self) -> &[DisplayItem] {
        &self.display
    }

    /// Version counter for display changes - used for render caching.
    pub fn display_version(&self) -> usize {
        self.display_version
    }

    pub fn has_api_key(&self, provider: Provider) -> bool {
        self.api_keys.contains_key(&provider)
    }

    /// Get the current API key for the selected provider
    pub fn current_api_key(&self) -> Option<&String> {
        self.api_keys.get(&self.model.provider())
    }

    /// Whether we're currently streaming a response.
    pub fn is_loading(&self) -> bool {
        self.busy_reason().is_some()
    }

    /// Returns a description of why the app is busy, or None if idle.
    ///
    /// This centralizes busy-state checks to ensure consistency across
    /// `start_streaming`, `start_summarization`, and UI queries.
    fn busy_reason(&self) -> Option<&'static str> {
        match &self.state {
            OperationState::Idle => None,
            OperationState::Streaming(_) => Some("streaming a response"),
            OperationState::ToolLoop(_) => Some("tool execution in progress"),
            OperationState::ToolRecovery(_) => Some("tool recovery pending"),
            OperationState::Summarizing(_)
            | OperationState::SummarizingWithQueued(_)
            | OperationState::SummarizationRetry(_)
            | OperationState::SummarizationRetryWithQueued(_) => Some("summarization in progress"),
        }
    }

    /// Get context usage statistics for the UI.
    /// Uses cached value when available to avoid recomputing every frame.
    pub fn context_usage_status(&mut self) -> ContextUsageStatus {
        if let Some(cached) = &self.cached_usage_status {
            return cached.clone();
        }
        let status = self.context_manager.usage_status();
        self.cached_usage_status = Some(status.clone());
        status
    }

    /// Invalidate cached context usage status.
    /// Call this when history, model, or output limits change.
    fn invalidate_usage_cache(&mut self) {
        self.cached_usage_status = None;
    }

    pub fn context_infinity_enabled(&self) -> bool {
        self.context_infinity
    }

    #[allow(clippy::unused_self)] // Kept as method for API consistency
    fn idle_state(&self) -> OperationState {
        OperationState::Idle
    }

    fn replace_with_idle(&mut self) -> OperationState {
        std::mem::replace(&mut self.state, OperationState::Idle)
    }

    fn build_basic_api_messages(&mut self) -> Vec<Message> {
        let budget = self
            .context_manager
            .current_limits()
            .effective_input_budget();
        let entries = self.context_manager.history().entries();
        if entries.is_empty() {
            return Vec::new();
        }

        let mut selected_rev: Vec<Message> = Vec::new();
        let mut tokens_used = 0u32;
        let mut truncated = false;
        let mut oversize = false;

        for entry in entries.iter().rev() {
            let tokens = entry.token_count();
            if tokens_used + tokens <= budget {
                selected_rev.push(entry.message().clone());
                tokens_used += tokens;
            } else if selected_rev.is_empty() {
                selected_rev.push(entry.message().clone());
                oversize = true;
                break;
            } else {
                truncated = true;
                break;
            }
        }

        if truncated {
            self.push_notification(
                "ContextInfinity disabled: truncating history to fit model budget",
            );
        } else if oversize {
            self.push_notification("ContextInfinity disabled: last message exceeds model budget");
        }

        selected_rev.reverse();
        selected_rev
    }

    fn clamp_output_limits_to_model(&mut self) {
        let model_max_output = self.context_manager.current_limits().max_output();
        let current = self.output_limits;

        if current.max_output_tokens() <= model_max_output {
            return;
        }

        let clamped = if let Some(budget) = current.thinking_budget() {
            if budget < model_max_output {
                OutputLimits::with_thinking(model_max_output, budget)
                    .unwrap_or(OutputLimits::new(model_max_output))
            } else {
                OutputLimits::new(model_max_output)
            }
        } else {
            OutputLimits::new(model_max_output)
        };

        let warning = if current.has_thinking() && !clamped.has_thinking() {
            format!(
                "Clamped max_output_tokens {} → {} for {}; disabled thinking budget",
                current.max_output_tokens(),
                clamped.max_output_tokens(),
                self.model
            )
        } else {
            format!(
                "Clamped max_output_tokens {} → {} for {}",
                current.max_output_tokens(),
                clamped.max_output_tokens(),
                self.model
            )
        };
        tracing::warn!("{warning}");

        self.output_limits = clamped;
        // Sync to context manager for accurate budget calculation
        self.context_manager
            .set_output_limit(clamped.max_output_tokens());
    }

    /// Switch to a different provider
    pub fn set_provider(&mut self, provider: Provider) {
        self.model = provider.default_model();
        if self.context_infinity_enabled() {
            // Notify context manager of model change for adaptive context
            self.handle_context_adaptation();
        } else {
            self.context_manager
                .set_model_without_adaptation(self.model.as_str());
        }

        self.clamp_output_limits_to_model();
    }

    /// Set a specific model (called from :model command).
    pub fn set_model(&mut self, model: ModelName) {
        self.model = model;
        if self.context_infinity_enabled() {
            self.handle_context_adaptation();
        } else {
            self.context_manager
                .set_model_without_adaptation(self.model.as_str());
        }

        self.clamp_output_limits_to_model();
    }

    /// Handle context adaptation after a model switch.
    ///
    /// This method is called after `set_model()` or `set_provider()` to handle
    /// the context adaptation result:
    /// - If shrinking with `needs_summarization`, starts background summarization
    /// - If expanding, attempts to restore previously summarized messages
    fn handle_context_adaptation(&mut self) {
        let adaptation = self.context_manager.switch_model(self.model.as_str());
        self.invalidate_usage_cache();

        match adaptation {
            ContextAdaptation::NoChange
            | ContextAdaptation::Shrinking {
                needs_summarization: false,
                ..
            } => {
                // No action needed
            }
            ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_summarization: true,
            } => {
                self.push_notification(format!(
                    "Context budget shrank {}k → {}k; summarizing...",
                    old_budget / 1000,
                    new_budget / 1000
                ));
                self.start_summarization();
            }
            ContextAdaptation::Expanding {
                old_budget,
                new_budget,
                can_restore,
            } => {
                if can_restore > 0 {
                    let restored = self.context_manager.try_restore_messages();
                    if restored > 0 {
                        self.push_notification(format!(
                            "Context budget expanded {}k → {}k; restored {} messages",
                            old_budget / 1000,
                            new_budget / 1000,
                            restored
                        ));
                    }
                }
            }
        }
    }

    /// Increment animation tick and poll background tasks.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.poll_summarization();
        self.poll_summarization_retry();
        self.poll_tool_loop();
    }

    /// Get elapsed time since last frame and update timing.
    pub fn frame_elapsed(&mut self) -> Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.view.last_frame);
        self.view.last_frame = now;
        elapsed
    }

    /// Get mutable reference to modal effect for UI processing.
    pub fn modal_effect_mut(&mut self) -> Option<&mut ModalEffect> {
        self.view.modal_effect.as_mut()
    }

    pub fn clear_modal_effect(&mut self) {
        self.view.modal_effect = None;
    }

    pub fn input_mode(&self) -> InputMode {
        self.input.mode()
    }

    pub fn enter_insert_mode_at_end(&mut self) {
        self.input.draft_mut().move_cursor_end();
        self.enter_insert_mode();
    }

    pub fn enter_insert_mode_with_clear(&mut self) {
        self.input.draft_mut().clear();
        self.enter_insert_mode();
    }

    pub fn enter_normal_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_normal();
        self.view.modal_effect = None;
    }

    pub fn enter_insert_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_insert();
    }

    pub fn enter_command_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_command();
    }

    pub fn enter_model_select_mode(&mut self) {
        let index = PredefinedModel::all()
            .iter()
            .position(|m| m.to_model_name() == self.model)
            .unwrap_or(0);
        self.input = std::mem::take(&mut self.input).into_model_select(index);
        if self.view.ui_options.reduced_motion {
            self.view.modal_effect = None;
        } else {
            self.view.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
            self.view.last_frame = Instant::now();
        }
    }

    pub fn model_select_index(&self) -> Option<usize> {
        self.input.model_select_index()
    }

    fn model_select_max_index() -> usize {
        PredefinedModel::all().len().saturating_sub(1)
    }

    fn trigger_model_select_shake(&mut self) {
        if self.view.ui_options.reduced_motion {
            return;
        }
        self.view.modal_effect = Some(ModalEffect::shake(Duration::from_millis(360)));
        self.view.last_frame = Instant::now();
    }

    pub fn model_select_move_up(&mut self) {
        if let InputState::ModelSelect { selected, .. } = &mut self.input
            && *selected > 0
        {
            *selected -= 1;
        }
    }

    pub fn model_select_move_down(&mut self) {
        if let InputState::ModelSelect { selected, .. } = &mut self.input {
            let max_index = Self::model_select_max_index();
            if *selected < max_index {
                *selected += 1;
            }
        }
    }

    pub fn model_select_set_index(&mut self, index: usize) {
        if let InputState::ModelSelect { selected, .. } = &mut self.input {
            let max_index = Self::model_select_max_index();
            *selected = index.min(max_index);
        }
    }

    /// Select the current model and return to normal mode.
    pub fn model_select_confirm(&mut self) {
        let Some(index) = self.model_select_index() else {
            return;
        };
        let models = PredefinedModel::all();
        let Some(predefined) = models.get(index) else {
            self.trigger_model_select_shake();
            return;
        };
        let model = predefined.to_model_name();
        self.set_model(model);
        self.push_notification(format!("Model set to: {}", predefined.display_name()));
        self.enter_normal_mode();
    }

    pub fn tool_approval_move_up(&mut self) {
        if let OperationState::ToolLoop(state) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
            && approval.cursor > 0
        {
            approval.cursor -= 1;
            approval.deny_confirm = false;
            approval.expanded = None;
        }
    }

    pub fn tool_approval_move_down(&mut self) {
        if let OperationState::ToolLoop(state) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
        {
            // Allow cursor to move to Submit (N) and Deny (N+1) buttons
            let max_cursor = approval.requests.len() + 1;
            if approval.cursor < max_cursor {
                approval.cursor += 1;
            }
            approval.deny_confirm = false;
            approval.expanded = None;
        }
    }

    pub fn tool_approval_toggle(&mut self) {
        if let OperationState::ToolLoop(state) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
            && approval.cursor < approval.selected.len()
        {
            approval.selected[approval.cursor] = !approval.selected[approval.cursor];
            approval.deny_confirm = false;
        }
    }

    pub fn tool_approval_toggle_details(&mut self) {
        if let OperationState::ToolLoop(state) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
            && approval.cursor < approval.requests.len()
        {
            if approval.expanded == Some(approval.cursor) {
                approval.expanded = None;
            } else {
                approval.expanded = Some(approval.cursor);
            }
            approval.deny_confirm = false;
        }
    }

    pub fn tool_approval_approve_all(&mut self) {
        self.resolve_tool_approval(tools::ApprovalDecision::ApproveAll);
    }

    pub fn tool_approval_deny_all(&mut self) {
        self.tool_approval_request_deny_all();
    }

    /// Handle Enter key on approval prompt - action depends on cursor position:
    /// - On tool item: toggle selection
    /// - On Submit button: confirm selected
    /// - On Deny All button: deny all
    pub fn tool_approval_activate(&mut self) {
        // Determine cursor position relative to tool count
        let (cursor, num_tools) = if let OperationState::ToolLoop(state) = &self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &state.phase
        {
            (approval.cursor, approval.requests.len())
        } else {
            return;
        };

        match cursor.cmp(&num_tools) {
            std::cmp::Ordering::Less => self.tool_approval_toggle(),
            std::cmp::Ordering::Equal => self.tool_approval_confirm_selected(),
            std::cmp::Ordering::Greater => self.tool_approval_request_deny_all(),
        }
    }

    pub fn tool_approval_confirm_selected(&mut self) {
        let ids = if let OperationState::ToolLoop(state) = &self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &state.phase
        {
            approval
                .requests
                .iter()
                .zip(approval.selected.iter())
                .filter(|(_, selected)| **selected)
                .map(|(req, _)| req.tool_call_id.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        if ids.is_empty() {
            self.resolve_tool_approval(tools::ApprovalDecision::DenyAll);
        } else {
            self.resolve_tool_approval(tools::ApprovalDecision::ApproveSelected(ids));
        }
    }

    pub fn tool_approval_request_deny_all(&mut self) {
        let mut should_deny = false;
        if let OperationState::ToolLoop(state) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
        {
            let deny_cursor = approval.requests.len() + 1;
            approval.cursor = deny_cursor;
            if approval.deny_confirm {
                should_deny = true;
            } else {
                approval.deny_confirm = true;
            }
        }

        if should_deny {
            self.resolve_tool_approval(tools::ApprovalDecision::DenyAll);
        }
    }

    pub fn tool_recovery_resume(&mut self) {
        self.resolve_tool_recovery(ToolRecoveryDecision::Resume);
    }

    pub fn tool_recovery_discard(&mut self) {
        self.resolve_tool_recovery(ToolRecoveryDecision::Discard);
    }

    pub fn draft_text(&self) -> &str {
        self.input.draft().text()
    }

    pub fn draft_cursor(&self) -> usize {
        self.input.draft().cursor()
    }

    pub fn draft_cursor_byte_index(&self) -> usize {
        self.input.draft().byte_index()
    }

    pub fn command_text(&self) -> Option<&str> {
        self.input.command()
    }

    pub fn command_cursor(&self) -> Option<usize> {
        self.input.command_cursor()
    }

    pub fn command_cursor_byte_index(&self) -> Option<usize> {
        self.input.command_cursor_byte_index()
    }

    pub fn update_scroll_max(&mut self, max: u16) {
        self.view.scroll_max = max;

        if let ScrollState::Manual { offset_from_top } = self.view.scroll
            && offset_from_top >= max
        {
            self.view.scroll = ScrollState::AutoBottom;
        }
    }

    pub fn scroll_offset_from_top(&self) -> u16 {
        match self.view.scroll {
            ScrollState::AutoBottom => self.view.scroll_max,
            ScrollState::Manual { offset_from_top } => offset_from_top.min(self.view.scroll_max),
        }
    }

    /// Scroll up in message view.
    pub fn scroll_up(&mut self) {
        self.view.scroll = match self.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.view.scroll_max.saturating_sub(3),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(3),
            },
        };
    }

    /// Scroll up by a page.
    pub fn scroll_page_up(&mut self) {
        let delta = 10;
        self.view.scroll = match self.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.view.scroll_max.saturating_sub(delta),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(delta),
            },
        };
    }

    /// Scroll down in message view.
    pub fn scroll_down(&mut self) {
        let ScrollState::Manual { offset_from_top } = self.view.scroll else {
            return;
        };

        let new_offset = offset_from_top.saturating_add(3);
        if new_offset >= self.view.scroll_max {
            self.view.scroll = ScrollState::AutoBottom;
        } else {
            self.view.scroll = ScrollState::Manual {
                offset_from_top: new_offset,
            };
        }
    }

    /// Scroll down by a page.
    pub fn scroll_page_down(&mut self) {
        let ScrollState::Manual { offset_from_top } = self.view.scroll else {
            return;
        };

        let delta = 10;
        let new_offset = offset_from_top.saturating_add(delta);
        if new_offset >= self.view.scroll_max {
            self.view.scroll = ScrollState::AutoBottom;
        } else {
            self.view.scroll = ScrollState::Manual {
                offset_from_top: new_offset,
            };
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.view.scroll = ScrollState::Manual { offset_from_top: 0 };
    }

    /// Jump to bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.view.scroll = ScrollState::AutoBottom;
    }

    /// Scroll up by 20% of total scrollable content.
    pub fn scroll_up_chunk(&mut self) {
        let delta = (self.view.scroll_max / 5).max(1);
        self.view.scroll = match self.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.view.scroll_max.saturating_sub(delta),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(delta),
            },
        };
    }
}

#[cfg(test)]
mod tests;

//! Core engine for Forge - state machine and orchestration.
//!
//! This crate contains the App state machine without TUI dependencies.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

mod ui;
use ui::InputState;
pub use ui::{
    ChangeKind, DisplayItem, DraftInput, FilesPanelState, InputHistory, InputMode, ModalEffect,
    ModalEffectKind, PanelEffect, PanelEffectKind, PredefinedModel, ScrollState, UiOptions,
    ViewState,
};

pub use forge_context::{
    ActiveJournal, ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus,
    ExtractionResult, Fact, FactType, FullHistory, Librarian, MessageId, ModelLimits,
    ModelLimitsSource, ModelRegistry, PendingSummarization, PreparedContext, RecoveredStream,
    RecoveredToolBatch, RetrievalResult, StreamJournal, SummarizationNeeded, SummarizationScope,
    TokenCounter, ToolBatchId, ToolJournal, generate_summary, retrieve_relevant,
    summarization_model,
};
pub use forge_providers::{self, ApiConfig, gemini::GeminiCache, gemini::GeminiCacheConfig};
pub use forge_types::{
    ApiKey, CacheHint, CacheableMessage, EmptyStringError, Message, ModelName, ModelNameKind,
    NonEmptyStaticStr, NonEmptyString, OpenAIReasoningEffort, OpenAIReasoningSummary,
    OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation, OutputLimits, Provider,
    StreamEvent, StreamFinishReason, ToolCall, ToolDefinition, ToolResult, sanitize_terminal_text,
};

mod config;
pub use config::{AppConfig, ForgeConfig};

mod checkpoints;
mod tools;

mod commands;
mod errors;
mod init;
mod input_modes;
mod persistence;
mod security;
mod session_state;
pub use session_state::SessionChangeLog;
mod state;
mod streaming;
mod summarization;
mod tool_loop;
mod util;

pub use input_modes::{
    CommandMode, CommandToken, EnteredCommand, InsertMode, InsertToken, QueuedUserMessage,
};

pub use commands::{CommandSpec, command_help_summary, command_specs};

pub(crate) use persistence::{ABORTED_JOURNAL_BADGE, EMPTY_RESPONSE_BADGE};

pub(crate) use init::{
    DEFAULT_TOOL_CAPACITY_BYTES, TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

/// Maximum number of stream events to process per UI tick.
pub const DEFAULT_STREAM_EVENT_BUDGET: usize = 512;

/// Result of diff generation for panel display.
#[derive(Debug, Clone)]
pub enum FileDiff {
    /// Unified diff between baseline and current.
    Diff(String),
    /// File was created (no baseline) - show full content as additions.
    Created(String),
    /// File no longer exists on disk.
    Deleted,
    /// Binary file - show size only.
    Binary(usize),
    /// Error reading file.
    Error(String),
}

pub use state::SummarizationTask;

use state::{
    ActiveStream, DataDir, OperationState, SummarizationRetry, SummarizationRetryState,
    SummarizationStart, SummarizationState, ToolLoopPhase, ToolRecoveryDecision,
};

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
    /// Provider thinking/reasoning deltas (if captured).
    thinking: String,
    /// Whether we should capture thinking deltas at all (default: false).
    ///
    /// This keeps the default behavior (discard thinking) while allowing
    /// opt-in UI rendering without forcing a breaking change in the rest of
    /// the engine.
    capture_thinking: bool,
    receiver: mpsc::Receiver<StreamEvent>,
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
        Self::new_with_thinking_capture(model, receiver, max_tool_args_bytes, false)
    }

    /// Construct a streaming message, optionally capturing provider thinking deltas.
    ///
    /// When `capture_thinking` is `false`, thinking events are discarded (current default).
    #[must_use]
    pub fn new_with_thinking_capture(
        model: ModelName,
        receiver: mpsc::Receiver<StreamEvent>,
        max_tool_args_bytes: usize,
        capture_thinking: bool,
    ) -> Self {
        Self {
            model,
            content: String::new(),
            thinking: String::new(),
            capture_thinking,
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

    /// Provider thinking/reasoning content received during streaming (if captured).
    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
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
            StreamEvent::ThinkingDelta(thinking) => {
                if self.capture_thinking {
                    self.thinking.push_str(&thinking);
                }
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

pub(crate) const MAX_SUMMARIZATION_ATTEMPTS: u8 = 5;
pub(crate) const SUMMARIZATION_RETRY_BASE_MS: u64 = 500;
pub(crate) const SUMMARIZATION_RETRY_MAX_MS: u64 = 8000;
pub(crate) const SUMMARIZATION_RETRY_JITTER_MS: u64 = 200;

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
    /// `OpenAI` request defaults (reasoning/summary/verbosity/truncation).
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
    /// Checkpoints for rewind (per-turn conversation checkpoints + tool-edit snapshots).
    checkpoints: checkpoints::CheckpointStore,
    /// Tool iterations used in the current user turn.
    tool_iterations: u32,
    /// Whether we've already warned about a failed history load.
    history_load_warning_shown: bool,
    /// Whether we've already warned about autosave failures.
    autosave_warning_shown: bool,
    /// Active Gemini cache (if caching enabled and cache created).
    /// Uses Arc<Mutex> because cache is created/updated inside async streaming tasks.
    gemini_cache: std::sync::Arc<tokio::sync::Mutex<Option<GeminiCache>>>,
    /// Whether Gemini thinking mode is enabled via config.
    gemini_thinking_enabled: bool,
    /// Gemini cache configuration.
    gemini_cache_config: GeminiCacheConfig,
    /// The Librarian for fact extraction and retrieval (Context Infinity).
    /// Uses Arc<Mutex> because it's accessed from async tasks for extraction.
    /// None if context_infinity is disabled or no Gemini API key.
    librarian: Option<std::sync::Arc<tokio::sync::Mutex<Librarian>>>,
    /// Input history for prompt and command recall.
    input_history: ui::InputHistory,
    /// Counter for debouncing session autosave (incremented each tick).
    session_save_counter: u32,
    /// Session-wide log of files created and modified.
    session_changes: SessionChangeLog,
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

    /// Toggle visibility of the files panel.
    pub fn toggle_files_panel(&mut self) {
        let panel = &mut self.view.files_panel;
        if self.view.ui_options.reduced_motion {
            self.view.files_panel_effect = None;
            panel.visible = !panel.visible;
            if !panel.visible {
                // Reset state when hiding
                panel.expanded = None;
                panel.diff_scroll = 0;
            }
            return;
        }

        if panel.visible {
            self.view.files_panel_effect =
                Some(PanelEffect::slide_out_right(Duration::from_millis(180)));
            self.view.last_frame = Instant::now();
        } else {
            panel.visible = true;
            self.view.files_panel_effect =
                Some(PanelEffect::slide_in_right(Duration::from_millis(180)));
            self.view.last_frame = Instant::now();
        }
    }

    /// Check if the files panel is visible.
    pub fn files_panel_visible(&self) -> bool {
        self.view.files_panel.visible
    }

    /// Get mutable reference to files panel effect for UI processing.
    pub fn files_panel_effect_mut(&mut self) -> Option<&mut PanelEffect> {
        self.view.files_panel_effect.as_mut()
    }

    pub fn clear_files_panel_effect(&mut self) {
        self.view.files_panel_effect = None;
    }

    pub fn finish_files_panel_effect(&mut self) {
        if let Some(effect) = &self.view.files_panel_effect
            && effect.kind() == PanelEffectKind::SlideOutRight
        {
            let panel = &mut self.view.files_panel;
            panel.visible = false;
            // Reset state when hiding
            panel.expanded = None;
            panel.diff_scroll = 0;
        }
        self.view.files_panel_effect = None;
    }

    /// Get the session-wide file change log.
    pub fn session_changes(&self) -> &SessionChangeLog {
        &self.session_changes
    }

    /// Get ordered list of changed files: modified first (alphabetical), then created.
    /// Filters out files that no longer exist on disk.
    pub fn ordered_files(&self) -> Vec<(std::path::PathBuf, ChangeKind)> {
        let changes = &self.session_changes;
        changes
            .modified
            .iter()
            .map(|p| (p.clone(), ChangeKind::Modified))
            .chain(
                changes
                    .created
                    .iter()
                    .map(|p| (p.clone(), ChangeKind::Created)),
            )
            .filter(|(p, _)| p.exists())
            .collect()
    }

    fn files_panel_count(&self) -> usize {
        self.ordered_files().len()
    }

    /// Cycle to the next file in the panel (wrapping).
    pub fn files_panel_next(&mut self) {
        let count = self.files_panel_count();
        if count == 0 {
            return;
        }
        let new_selected = (self.view.files_panel.selected + 1) % count;
        let expanded_path = self
            .ordered_files()
            .get(new_selected)
            .map(|(p, _)| p.clone());

        let panel = &mut self.view.files_panel;
        panel.selected = new_selected;
        panel.diff_scroll = 0;
        panel.expanded = expanded_path;
    }

    /// Cycle to the previous file in the panel (wrapping).
    pub fn files_panel_prev(&mut self) {
        let count = self.files_panel_count();
        if count == 0 {
            return;
        }
        let new_selected = if self.view.files_panel.selected == 0 {
            count - 1
        } else {
            self.view.files_panel.selected - 1
        };
        let expanded_path = self
            .ordered_files()
            .get(new_selected)
            .map(|(p, _)| p.clone());

        let panel = &mut self.view.files_panel;
        panel.selected = new_selected;
        panel.diff_scroll = 0;
        panel.expanded = expanded_path;
    }

    /// Check if a diff is currently expanded.
    pub fn files_panel_expanded(&self) -> bool {
        self.view.files_panel.expanded.is_some()
    }

    /// Get the current files panel state.
    pub fn files_panel_state(&self) -> &FilesPanelState {
        &self.view.files_panel
    }

    /// Collapse the expanded diff.
    pub fn files_panel_collapse(&mut self) {
        self.view.files_panel.expanded = None;
        self.view.files_panel.diff_scroll = 0;
    }

    /// Sync the files panel selection index with the expanded path.
    ///
    /// Called after files are added/removed to ensure the selected index
    /// correctly corresponds to the expanded file path.
    pub fn files_panel_sync_selection(&mut self) {
        let panel = &mut self.view.files_panel;
        let files = self
            .session_changes
            .modified
            .iter()
            .cloned()
            .chain(self.session_changes.created.iter().cloned())
            .filter(|p| p.exists())
            .collect::<Vec<_>>();

        if let Some(expanded_path) = &panel.expanded {
            // Find the new index for the expanded path
            if let Some(new_idx) = files.iter().position(|p| p == expanded_path) {
                panel.selected = new_idx;
            } else {
                // Expanded file no longer in list - collapse and reset
                panel.expanded = None;
                panel.diff_scroll = 0;
                panel.selected = panel.selected.min(files.len().saturating_sub(1));
            }
        } else {
            // No expansion - just clamp selected to valid range
            panel.selected = panel.selected.min(files.len().saturating_sub(1));
        }
    }

    /// Scroll the diff view down.
    pub fn files_panel_scroll_diff_down(&mut self) {
        self.view.files_panel.diff_scroll += 10;
    }

    /// Scroll the diff view up.
    pub fn files_panel_scroll_diff_up(&mut self) {
        self.view.files_panel.diff_scroll = self.view.files_panel.diff_scroll.saturating_sub(10);
    }

    /// Generate diff for the currently expanded file in the panel.
    pub fn files_panel_diff(&self) -> Option<FileDiff> {
        let path = self.view.files_panel.expanded.as_ref()?;

        let current = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some(FileDiff::Deleted);
            }
            Err(e) => return Some(FileDiff::Error(e.to_string())),
        };

        // Check for binary content (contains null bytes in first 8KB)
        if current.iter().take(8192).any(|&b| b == 0) {
            return Some(FileDiff::Binary(current.len()));
        }

        let baseline = self
            .checkpoints
            .find_baseline_for_file(path)
            .and_then(|proof| self.checkpoints.baseline_content(proof, path));

        if let Some(old_bytes) = baseline {
            let diff = tools::builtins::format_unified_diff(
                &path.to_string_lossy(),
                old_bytes,
                &current,
                true,
            );
            Some(FileDiff::Diff(diff))
        } else {
            let content = String::from_utf8_lossy(&current);
            let lines: Vec<_> = content.lines().map(|l| format!("+{l}")).collect();
            Some(FileDiff::Created(lines.join("\n")))
        }
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

    // ========================================================================
    // Tool loop state helpers (private, inline)
    // ========================================================================

    #[inline]
    fn tool_loop_state(&self) -> Option<&state::ToolLoopState> {
        match &self.state {
            OperationState::ToolLoop(state) => Some(state.as_ref()),
            _ => None,
        }
    }

    #[inline]
    fn tool_loop_state_mut(&mut self) -> Option<&mut state::ToolLoopState> {
        match &mut self.state {
            OperationState::ToolLoop(state) => Some(state.as_mut()),
            _ => None,
        }
    }

    #[inline]
    fn tool_approval_ref(&self) -> Option<&state::ApprovalState> {
        match &self.tool_loop_state()?.phase {
            ToolLoopPhase::AwaitingApproval(approval) => Some(approval),
            ToolLoopPhase::Executing(_) => None,
        }
    }

    #[inline]
    fn tool_approval_mut(&mut self) -> Option<&mut state::ApprovalState> {
        match &mut self.tool_loop_state_mut()?.phase {
            ToolLoopPhase::AwaitingApproval(approval) => Some(approval),
            ToolLoopPhase::Executing(_) => None,
        }
    }

    #[inline]
    fn tool_exec_ref(&self) -> Option<&state::ActiveToolExecution> {
        match &self.tool_loop_state()?.phase {
            ToolLoopPhase::Executing(exec) => Some(exec),
            ToolLoopPhase::AwaitingApproval(_) => None,
        }
    }

    // ========================================================================
    // Tool loop public accessors
    // ========================================================================

    pub fn tool_loop_calls(&self) -> Option<&[ToolCall]> {
        Some(&self.tool_loop_state()?.batch.calls)
    }

    pub fn tool_loop_execute_calls(&self) -> Option<&[ToolCall]> {
        Some(&self.tool_loop_state()?.batch.execute_now)
    }

    pub fn tool_loop_results(&self) -> Option<&[ToolResult]> {
        Some(&self.tool_loop_state()?.batch.results)
    }

    pub fn tool_loop_current_call_id(&self) -> Option<&str> {
        self.tool_exec_ref()?
            .current_call
            .as_ref()
            .map(|c| c.id.as_str())
    }

    pub fn tool_loop_output_lines(&self) -> Option<&[String]> {
        Some(&self.tool_exec_ref()?.output_lines)
    }

    pub fn tool_approval_requests(&self) -> Option<&[tools::ConfirmationRequest]> {
        Some(&self.tool_approval_ref()?.requests)
    }

    pub fn tool_approval_selected(&self) -> Option<&[bool]> {
        Some(&self.tool_approval_ref()?.selected)
    }

    pub fn tool_approval_cursor(&self) -> Option<usize> {
        Some(self.tool_approval_ref()?.cursor)
    }

    pub fn tool_approval_expanded(&self) -> Option<usize> {
        self.tool_approval_ref()?.expanded
    }

    pub fn tool_approval_deny_confirm(&self) -> bool {
        self.tool_approval_ref()
            .is_some_and(|approval| approval.deny_confirm)
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
            OperationState::Summarizing(_) | OperationState::SummarizationRetry(_) => {
                Some("summarization in progress")
            }
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

        // Debounced session autosave (~3 seconds at 100ms poll interval)
        self.session_save_counter += 1;
        if self.session_save_counter >= 30 {
            self.session_save_counter = 0;
            self.autosave_session();
        }
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
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        if approval.cursor == 0 {
            return;
        }
        approval.cursor -= 1;
        approval.deny_confirm = false;
        approval.expanded = None;
    }

    pub fn tool_approval_move_down(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        // Allow cursor to move to Submit (N) and Deny (N+1) buttons
        let max_cursor = approval.requests.len() + 1;
        if approval.cursor < max_cursor {
            approval.cursor += 1;
        }
        approval.deny_confirm = false;
        approval.expanded = None;
    }

    pub fn tool_approval_toggle(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        if approval.cursor >= approval.selected.len() {
            return;
        }
        approval.selected[approval.cursor] = !approval.selected[approval.cursor];
        approval.deny_confirm = false;
    }

    pub fn tool_approval_toggle_details(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        if approval.cursor >= approval.requests.len() {
            return;
        }
        if approval.expanded == Some(approval.cursor) {
            approval.expanded = None;
        } else {
            approval.expanded = Some(approval.cursor);
        }
        approval.deny_confirm = false;
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
        let Some(approval) = self.tool_approval_ref() else {
            return;
        };
        let cursor = approval.cursor;
        let num_tools = approval.requests.len();

        match cursor.cmp(&num_tools) {
            std::cmp::Ordering::Less => self.tool_approval_toggle(),
            std::cmp::Ordering::Equal => self.tool_approval_confirm_selected(),
            std::cmp::Ordering::Greater => self.tool_approval_request_deny_all(),
        }
    }

    pub fn tool_approval_confirm_selected(&mut self) {
        let ids = self
            .tool_approval_ref()
            .map(|approval| {
                approval
                    .requests
                    .iter()
                    .zip(approval.selected.iter())
                    .filter(|(_, selected)| **selected)
                    .map(|(req, _)| req.tool_call_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if ids.is_empty() {
            self.resolve_tool_approval(tools::ApprovalDecision::DenyAll);
        } else {
            self.resolve_tool_approval(tools::ApprovalDecision::ApproveSelected(ids));
        }
    }

    pub fn tool_approval_request_deny_all(&mut self) {
        let should_deny = if let Some(approval) = self.tool_approval_mut() {
            let deny_cursor = approval.requests.len() + 1;
            approval.cursor = deny_cursor;
            if approval.deny_confirm {
                true
            } else {
                approval.deny_confirm = true;
                false
            }
        } else {
            false
        };

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

    /// Navigate to previous (older) prompt in Insert mode.
    ///
    /// On first call, stashes the current draft and shows the most recent prompt.
    /// Subsequent calls show progressively older prompts.
    pub fn navigate_history_up(&mut self) {
        if let InputState::Insert(ref mut draft) = self.input
            && let Some(text) = self.input_history.navigate_prompt_up(draft.text())
        {
            draft.set_text(text.to_owned());
        }
    }

    /// Navigate to next (newer) prompt in Insert mode.
    ///
    /// When at the newest entry, restores the stashed draft.
    pub fn navigate_history_down(&mut self) {
        if let InputState::Insert(ref mut draft) = self.input
            && let Some(text) = self.input_history.navigate_prompt_down()
        {
            draft.set_text(text.to_owned());
        }
    }

    /// Navigate to previous (older) command in Command mode.
    pub fn navigate_command_history_up(&mut self) {
        if let InputState::Command { command, .. } = &mut self.input
            && let Some(text) = self.input_history.navigate_command_up(command.text())
        {
            command.set_text(text.to_owned());
        }
    }

    /// Navigate to next (newer) command in Command mode.
    pub fn navigate_command_history_down(&mut self) {
        if let InputState::Command { command, .. } = &mut self.input
            && let Some(text) = self.input_history.navigate_command_down()
        {
            command.set_text(text.to_owned());
        }
    }

    /// Record a submitted prompt to history.
    pub(crate) fn record_prompt(&mut self, text: &str) {
        self.input_history.push_prompt(text.to_owned());
        self.input_history.reset_navigation();
    }

    /// Record an executed command to history.
    pub(crate) fn record_command(&mut self, text: &str) {
        self.input_history.push_command(text.to_owned());
        self.input_history.reset_navigation();
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

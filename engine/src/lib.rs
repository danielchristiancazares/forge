//! Core engine for Forge - state machine and orchestration.
//!
//! This crate contains the `App` state machine without TUI dependencies, providing:
//!
//! - **Application state**: The [`App`] struct manages all runtime state
//! - **Input modes**: Vim-style modal editing (Normal, Insert, Command, Model)
//! - **Streaming**: LLM response streaming with tool call handling
//! - **Tool execution**: Approval flow, crash recovery, and result journaling
//! - **Context management**: Token budgeting, distillation triggers
//! - **Persistence**: Session state, history, and checkpoint recovery
//!
//! # Architecture
//!
//! The engine uses dual state machines:
//!
//! 1. **Input state** (`InputState`): Controls which input mode is active
//! 2. **Operation state** (`OperationState`): Tracks async operations (streaming, distillation)
//!
//! The TUI layer (`forge_tui`) reads state from `App` and forwards input back to it.
//! No rendering logic lives in this crate.
//!
//! # Type-Driven Design
//!
//! Operations requiring proof of state use token types:
//!
//! - [`InsertToken`]: Proof that we're in Insert mode
//! - [`CommandToken`]: Proof that we're in Command mode
//! - [`QueuedUserMessage`]: Proof that a message is validated and ready to send
//! - [`PreparedContext`]: Proof that context was built within token budget

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

mod ui;
use ui::InputState;
pub use ui::{
    ChangeKind, DisplayItem, DraftInput, FileEntry, FilePickerState, FilesPanelState, InputHistory,
    InputMode, ModalEffect, ModalEffectKind, PanelEffect, PanelEffectKind, PredefinedModel,
    ScrollState, UiOptions, ViewState, find_match_positions,
};

pub use forge_context::{
    ActiveJournal, BeginSessionError, ContextAdaptation, ContextBuildError, ContextManager,
    ContextUsageStatus, DistillationNeeded, DistillationPlanError, DistillationScope,
    ExtractionResult, Fact, FactType, FullHistory, Librarian, MessageId, ModelLimits,
    ModelLimitsSource, ModelRegistry, PendingDistillation, PreparedContext, RecoveredStream,
    RecoveredToolBatch, RetrievalResult, StreamJournal, TokenCounter, ToolBatchId, ToolJournal,
    distillation_model, generate_distillation, retrieve_relevant,
};
pub use forge_providers::{self, ApiConfig, gemini::GeminiCache, gemini::GeminiCacheConfig};
pub use forge_types::{
    ApiKey, ApiUsage, CacheHint, CacheableMessage, EmptyStringError, Message, ModelName,
    NonEmptyStaticStr, NonEmptyString, OpenAIReasoningEffort, OpenAIReasoningItem,
    OpenAIReasoningSummary, OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation,
    OutputLimits, Provider, SecretString, StreamEvent, StreamFinishReason, ThinkingReplayState,
    ThinkingState, ThoughtSignature, ThoughtSignatureState, ToolCall, ToolDefinition, ToolResult,
    sanitize_terminal_text,
};

mod config;
pub use config::{AppConfig, ForgeConfig};

mod checkpoints;

pub use forge_tools as tools;

mod commands;
mod errors;
mod init;
mod input_modes;
mod notifications;
mod persistence;
mod security;
pub use security::sanitize_display_text;
mod session_state;
pub use session_state::SessionChangeLog;
mod distillation;
mod lsp_integration;
mod state;
mod streaming;
mod tool_loop;
mod util;

pub use input_modes::{
    CommandMode, CommandToken, EnteredCommand, InsertMode, InsertToken, QueuedUserMessage,
};

pub use commands::{CommandSpec, command_specs};

pub use notifications::SystemNotification;

pub(crate) use persistence::{ABORTED_JOURNAL_BADGE, EMPTY_RESPONSE_BADGE};

pub(crate) use init::{
    DEFAULT_TOOL_CAPACITY_BYTES, TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

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

/// Aggregated API usage for a user turn (may include multiple API calls).
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnUsage {
    /// Number of API calls made during this turn.
    pub api_calls: u32,
    /// Total usage aggregated across all API calls.
    pub total: ApiUsage,
    /// Usage from the most recent API call (for display).
    pub last_call: ApiUsage,
}

impl TurnUsage {
    pub fn record_call(&mut self, usage: ApiUsage) {
        self.api_calls = self.api_calls.saturating_add(1);
        self.total.merge(&usage);
        self.last_call = usage;
    }
}

pub use state::DistillationTask;

use state::{
    ActiveStream, DataDir, DistillationStart, DistillationState, OperationState, ToolLoopPhase,
    ToolRecoveryDecision,
};

#[derive(Debug, Clone)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments_json: String,
    thought_signature: ThoughtSignatureState,
    args_exceeded: bool,
}

#[derive(Debug)]
struct ParsedToolCalls {
    calls: Vec<ToolCall>,
    pre_resolved: Vec<ToolResult>,
}

/// An in-flight streaming response from an LLM.
///
/// This type uses the typestate pattern: its existence proves streaming is active.
/// When the stream completes, the `StreamingMessage` is consumed to produce a
/// complete assistant [`Message`].
///
/// # Tool Call Accumulation
///
/// As `ToolCallStart` and `ToolCallDelta` events arrive, tool calls are accumulated
/// in `ToolCallAccumulator` structs. Arguments are streamed as JSON strings and
/// parsed only when the stream completes.
///
/// Thinking/reasoning deltas are always captured for retroactive UI visibility toggle.
#[derive(Debug)]
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    /// Provider thinking/reasoning deltas.
    thinking: String,
    /// Provider-specific replay state for thinking blocks.
    thinking_replay: ThinkingReplayState,
    receiver: mpsc::Receiver<StreamEvent>,
    tool_calls: Vec<ToolCallAccumulator>,
    max_tool_args_bytes: usize,
    /// API-reported token usage accumulated during streaming.
    usage: ApiUsage,
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
            thinking: String::new(),
            thinking_replay: ThinkingReplayState::Unsigned,
            receiver,
            tool_calls: Vec::new(),
            max_tool_args_bytes,
            usage: ApiUsage::default(),
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

    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    #[must_use]
    pub fn thinking_replay_state(&self) -> &ThinkingReplayState {
        &self.thinking_replay
    }

    /// API-reported token usage accumulated during streaming.
    #[must_use]
    pub fn usage(&self) -> ApiUsage {
        self.usage
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
                self.thinking.push_str(&thinking);
                None
            }
            StreamEvent::ThinkingSignature(signature_delta) => {
                match &mut self.thinking_replay {
                    ThinkingReplayState::Unsigned => {
                        self.thinking_replay = ThinkingReplayState::ClaudeSigned {
                            signature: ThoughtSignature::new(signature_delta),
                        };
                    }
                    ThinkingReplayState::ClaudeSigned { signature } => {
                        signature.push_str(&signature_delta);
                    }
                    ThinkingReplayState::OpenAIReasoning { .. } => {}
                }
                None
            }
            StreamEvent::OpenAIReasoningDone {
                id,
                encrypted_content,
            } => {
                let item = OpenAIReasoningItem {
                    id,
                    encrypted_content,
                };
                match &mut self.thinking_replay {
                    ThinkingReplayState::OpenAIReasoning { items } => items.push(item),
                    _ => {
                        self.thinking_replay =
                            ThinkingReplayState::OpenAIReasoning { items: vec![item] };
                    }
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
            StreamEvent::Usage(usage) => {
                self.usage.merge(&usage);
                None
            }
            StreamEvent::Done => Some(StreamFinishReason::Done),
            StreamEvent::Error(err) => Some(StreamFinishReason::Error(err)),
        }
    }

    #[must_use]
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    pub(crate) fn take_tool_calls(&mut self) -> ParsedToolCalls {
        let mut calls = Vec::new();
        let mut pre_resolved = Vec::new();

        for acc in self.tool_calls.drain(..) {
            let build_call =
                |id: String,
                 name: String,
                 arguments: serde_json::Value,
                 thought_signature: ThoughtSignatureState| {
                    match thought_signature {
                        ThoughtSignatureState::Signed(signature) => {
                            ToolCall::new_signed(id, name, arguments, signature)
                        }
                        ThoughtSignatureState::Unsigned => ToolCall::new(id, name, arguments),
                    }
                };
            if acc.args_exceeded {
                pre_resolved.push(ToolResult::error(
                    acc.id.clone(),
                    acc.name.clone(),
                    "Tool arguments exceeded maximum size",
                ));
                calls.push(build_call(
                    acc.id,
                    acc.name,
                    serde_json::Value::Object(serde_json::Map::new()),
                    acc.thought_signature,
                ));
                continue;
            }

            if acc.arguments_json.trim().is_empty() {
                calls.push(build_call(
                    acc.id,
                    acc.name,
                    serde_json::Value::Object(serde_json::Map::new()),
                    acc.thought_signature,
                ));
                continue;
            }

            match serde_json::from_str(&acc.arguments_json) {
                Ok(arguments) => calls.push(build_call(
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
                    calls.push(build_call(
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

    pub fn into_message(self) -> Result<Message, forge_types::EmptyStringError> {
        // Persisted assistant content is untrusted external text; sanitize before it can
        // reach history/context storage to prevent terminal injection, invisible prompt
        // injection, and secret leaks.
        let sanitized = crate::security::sanitize_display_text(&self.content);
        let content = NonEmptyString::new(sanitized)?;
        Ok(Message::assistant(self.model, content))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemPrompts {
    /// Claude-specific prompt.
    pub claude: &'static str,
    /// OpenAI-specific prompt.
    pub openai: &'static str,
    /// Gemini-specific prompt.
    pub gemini: &'static str,
}

impl SystemPrompts {
    #[must_use]
    pub fn get(&self, provider: Provider) -> &'static str {
        match provider {
            Provider::Claude => self.claude,
            Provider::OpenAI => self.openai,
            Provider::Gemini => self.gemini,
        }
    }
}

pub struct App {
    input: InputState,
    display: Vec<DisplayItem>,
    /// Version counter for display changes - incremented when display items change.
    /// Used by TUI to cache rendered output and avoid rebuilding every frame.
    display_version: usize,
    should_quit: bool,
    /// View state for rendering (scroll, status, modal effects).
    view: ViewState,
    api_keys: HashMap<Provider, SecretString>,
    model: ModelName,
    tick: usize,
    data_dir: DataDir,
    context_manager: ContextManager,
    stream_journal: StreamJournal,
    state: OperationState,
    /// Whether memory (automatic distillation) is enabled.
    /// This is determined at init from config/env and does not change during runtime.
    memory_enabled: bool,
    /// Validated output limits (max tokens + explicit thinking state).
    /// Invariant: if thinking is enabled, budget < `max_tokens`.
    output_limits: OutputLimits,
    /// Whether prompt caching is enabled (for Claude).
    cache_enabled: bool,
    /// `OpenAI` request defaults (reasoning/summary/verbosity/truncation).
    openai_options: OpenAIRequestOptions,
    openai_reasoning_effort_explicit: bool,
    /// Provider-specific system prompts.
    /// The correct prompt is selected at streaming time based on the active provider.
    system_prompts: SystemPrompts,
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
    /// Tool names hidden from UI rendering (source-of-truth: `ToolExecutor::is_hidden()`).
    hidden_tools: HashSet<String>,
    /// Tool registry for executors.
    tool_registry: std::sync::Arc<tools::ToolRegistry>,
    /// Tool settings derived from config.
    tool_settings: tools::ToolSettings,
    /// Tool journal for crash recovery.
    tool_journal: ToolJournal,
    /// When set, tool execution is disabled for safety due to tool journal errors.
    ///
    /// This is a session-scoped safety latch: once tools are disabled, we pre-resolve
    /// tool calls to errors rather than executing them. This avoids running tools when
    /// crash-consistency guarantees cannot be upheld.
    tool_journal_disabled_reason: Option<String>,
    /// Pending stream journal step ID that needs commit+prune cleanup.
    ///
    /// Best-effort retries run during `tick()` when idle. If this remains uncleared,
    /// `StreamJournal::begin_session()` will refuse to start new streams.
    pending_stream_cleanup: Option<forge_context::StepId>,
    pending_stream_cleanup_failures: u8,
    /// Pending tool journal batch ID that needs pruning cleanup.
    ///
    /// Best-effort retries run during `tick()` when idle. If this remains uncleared,
    /// `ToolJournal::begin_*batch()` will refuse to start new batches.
    pending_tool_cleanup: Option<ToolBatchId>,
    pending_tool_cleanup_failures: u8,
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
    /// Uses `Arc<Mutex>` because cache is created/updated inside async streaming tasks.
    gemini_cache: std::sync::Arc<tokio::sync::Mutex<Option<GeminiCache>>>,
    /// Whether Gemini thinking mode is enabled via config.
    gemini_thinking_enabled: bool,
    anthropic_thinking_mode: config::AnthropicThinkingMode,
    /// Anthropic effort level for Opus 4.6+ ("low", "medium", "high", "max").
    anthropic_thinking_effort: config::AnthropicEffort,
    /// Gemini cache configuration.
    gemini_cache_config: GeminiCacheConfig,
    /// The Librarian for fact extraction and retrieval (Context Infinity).
    /// Uses `Arc<Mutex>` because it's accessed from async tasks for extraction.
    /// None if context_infinity is disabled or no Gemini API key.
    librarian: Option<std::sync::Arc<tokio::sync::Mutex<Librarian>>>,
    /// Input history for prompt and command recall.
    input_history: ui::InputHistory,
    /// Wall-clock timestamp for animation tick cadence (spinner ~10Hz).
    last_ui_tick: Instant,
    /// Wall-clock timestamp for session autosave cadence (~3s).
    last_session_autosave: Instant,
    /// Earliest time to attempt journal cleanup retries.
    next_journal_cleanup_attempt: Instant,
    /// Session-wide log of files created and modified.
    session_changes: SessionChangeLog,
    /// File picker state for "@" reference feature.
    file_picker: ui::FilePickerState,
    /// API usage for the current user turn (reset when turn completes).
    turn_usage: Option<TurnUsage>,
    /// API usage from the last completed turn (for status bar display).
    last_turn_usage: Option<TurnUsage>,
    /// Queue of pending system notifications to inject into next API request.
    notification_queue: notifications::NotificationQueue,
    /// LSP config, consumed on first start. `None` once started or if LSP is disabled.
    lsp_config: Option<forge_lsp::LspConfig>,
    /// LSP client manager. Populated lazily on first tool batch via `LspManager::start()`.
    /// `Arc<Mutex<Option<>>>` so the spawned startup task can populate it.
    lsp: std::sync::Arc<tokio::sync::Mutex<Option<forge_lsp::LspManager>>>,
    /// Cached diagnostics snapshot for UI display and agent feedback.
    lsp_snapshot: forge_lsp::DiagnosticsSnapshot,
    /// Pending diagnostics check: edited files + deadline for deferred error injection.
    pending_diag_check: Option<(Vec<std::path::PathBuf>, Instant)>,
}

impl App {
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Check if a transcript clear was requested and clear the flag.
    pub fn take_clear_transcript(&mut self) -> bool {
        std::mem::take(&mut self.view.clear_transcript)
    }

    pub fn ui_options(&self) -> UiOptions {
        self.view.ui_options
    }

    /// Toggle visibility of thinking/reasoning content in the UI.
    pub fn toggle_thinking(&mut self) {
        self.view.ui_options.show_thinking = !self.view.ui_options.show_thinking;
    }

    /// Toggle visibility of the files panel.
    pub fn toggle_files_panel(&mut self) {
        let panel = &mut self.view.files_panel;
        if self.view.ui_options.reduced_motion {
            self.view.files_panel_effect = None;
            panel.visible = !panel.visible;
            if !panel.visible {
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

    /// Close the files panel (no-op if already hidden).
    pub fn close_files_panel(&mut self) {
        if !self.view.files_panel.visible {
            return;
        }
        if self.view.ui_options.reduced_motion {
            self.view.files_panel_effect = None;
            let panel = &mut self.view.files_panel;
            panel.visible = false;
            panel.expanded = None;
            panel.diff_scroll = 0;
            return;
        }
        self.view.files_panel_effect =
            Some(PanelEffect::slide_out_right(Duration::from_millis(180)));
        self.view.last_frame = Instant::now();
    }

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
            panel.expanded = None;
            panel.diff_scroll = 0;
        }
        self.view.files_panel_effect = None;
    }

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

    pub fn files_panel_expanded(&self) -> bool {
        self.view.files_panel.expanded.is_some()
    }

    pub fn files_panel_state(&self) -> &FilesPanelState {
        &self.view.files_panel
    }

    /// Collapse the expanded diff.
    pub fn files_panel_collapse(&mut self) {
        self.view.files_panel.expanded = None;
        self.view.files_panel.diff_scroll = 0;
    }

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
            if let Some(new_idx) = files.iter().position(|p| p == expanded_path) {
                panel.selected = new_idx;
            } else {
                panel.expanded = None;
                panel.diff_scroll = 0;
                panel.selected = panel.selected.min(files.len().saturating_sub(1));
            }
        } else {
            panel.selected = panel.selected.min(files.len().saturating_sub(1));
        }
    }

    /// Scroll the diff view down.
    pub fn files_panel_scroll_diff_down(&mut self) {
        self.view.files_panel.diff_scroll += 10;
    }

    pub fn files_panel_scroll_diff_up(&mut self) {
        self.view.files_panel.diff_scroll = self.view.files_panel.diff_scroll.saturating_sub(10);
    }

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

    pub(crate) fn openai_options_for_model(&self, model: &ModelName) -> OpenAIRequestOptions {
        if model.provider() != Provider::OpenAI {
            return self.openai_options;
        }

        if model
            .as_str()
            .trim()
            .to_ascii_lowercase()
            .starts_with("gpt-5.2-pro")
            && !self.openai_reasoning_effort_explicit
            && self.openai_options.reasoning_effort() != OpenAIReasoningEffort::XHigh
        {
            return OpenAIRequestOptions::new(
                OpenAIReasoningEffort::XHigh,
                self.openai_options.reasoning_summary(),
                self.openai_options.verbosity(),
                self.openai_options.truncation(),
            );
        }

        self.openai_options
    }

    pub fn tick_count(&self) -> usize {
        self.tick
    }

    pub fn history(&self) -> &forge_context::FullHistory {
        self.context_manager.history()
    }

    pub fn streaming(&self) -> Option<&StreamingMessage> {
        match &self.state {
            OperationState::Streaming(active) => Some(active.message()),
            _ => None,
        }
    }

    // Tool loop state helpers (private, inline)

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
            ToolLoopPhase::Processing(_) | ToolLoopPhase::Executing(_) => None,
        }
    }

    #[inline]
    fn tool_approval_mut(&mut self) -> Option<&mut state::ApprovalState> {
        match &mut self.tool_loop_state_mut()?.phase {
            ToolLoopPhase::AwaitingApproval(approval) => Some(approval),
            ToolLoopPhase::Processing(_) | ToolLoopPhase::Executing(_) => None,
        }
    }

    #[inline]
    fn tool_exec_ref(&self) -> Option<&tool_loop::ActiveExecution> {
        match &self.tool_loop_state()?.phase {
            ToolLoopPhase::Executing(exec) => Some(exec),
            ToolLoopPhase::AwaitingApproval(_) | ToolLoopPhase::Processing(_) => None,
        }
    }

    // Tool loop public accessors

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
        Some(self.tool_exec_ref()?.spawned.call().id.as_str())
    }

    pub fn tool_loop_output_lines(&self) -> Option<&[String]> {
        let exec = self.tool_exec_ref()?;
        let current_id = exec.spawned.call().id.as_str();
        exec.output_lines.get(current_id).map(Vec::as_slice)
    }

    pub fn tool_loop_output_lines_for(&self, tool_call_id: &str) -> Option<&[String]> {
        self.tool_exec_ref()?
            .output_lines
            .get(tool_call_id)
            .map(Vec::as_slice)
    }

    pub fn tool_approval_requests(&self) -> Option<&[tools::ConfirmationRequest]> {
        Some(&self.tool_approval_ref()?.data().requests)
    }

    pub fn tool_approval_selected(&self) -> Option<&[bool]> {
        Some(&self.tool_approval_ref()?.data().selected)
    }

    pub fn tool_approval_cursor(&self) -> Option<usize> {
        Some(self.tool_approval_ref()?.data().cursor)
    }

    pub fn tool_approval_expanded(&self) -> Option<usize> {
        self.tool_approval_ref()?.data().expanded
    }

    pub fn tool_approval_scroll_offset(&self) -> usize {
        self.tool_approval_ref()
            .map(|s| s.data().scroll_offset)
            .unwrap_or(0)
    }

    pub fn tool_approval_deny_confirm(&self) -> bool {
        self.tool_approval_ref()
            .is_some_and(state::ApprovalState::is_confirming_deny)
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
                    | OperationState::RecoveryBlocked(_)
            )
    }

    pub fn display_items(&self) -> &[DisplayItem] {
        &self.display
    }

    /// Whether the named tool should be hidden from UI rendering.
    pub fn is_tool_hidden(&self, name: &str) -> bool {
        self.hidden_tools.contains(name)
    }

    /// Version counter for display changes - used for render caching.
    pub fn display_version(&self) -> usize {
        self.display_version
    }

    pub fn has_api_key(&self, provider: Provider) -> bool {
        self.api_keys.contains_key(&provider)
    }

    /// Get the current API key for the selected provider.
    pub fn current_api_key(&self) -> Option<&SecretString> {
        self.api_keys.get(&self.model.provider())
    }

    /// Whether we're currently streaming a response.
    pub fn is_loading(&self) -> bool {
        self.busy_reason().is_some()
    }

    /// If present, tools are disabled for safety due to a tool journal error.
    pub fn tool_journal_disabled_reason(&self) -> Option<&str> {
        self.tool_journal_disabled_reason.as_deref()
    }

    /// Human-readable reason for a recovery block (if recovery is blocked).
    pub fn recovery_blocked_reason(&self) -> Option<String> {
        match &self.state {
            OperationState::RecoveryBlocked(state) => Some(state.reason.message()),
            _ => None,
        }
    }

    /// Returns a description of why the app is busy, or None if idle.
    ///
    /// This centralizes busy-state checks to ensure consistency across
    /// `start_streaming`, `start_distillation`, and UI queries.
    fn busy_reason(&self) -> Option<&'static str> {
        match &self.state {
            OperationState::Idle | OperationState::ToolsDisabled(_) => None,
            OperationState::Streaming(_) => Some("streaming a response"),
            OperationState::ToolLoop(_) => Some("tool execution in progress"),
            OperationState::ToolRecovery(_) => Some("tool recovery pending"),
            OperationState::RecoveryBlocked(_) => Some("recovery blocked"),
            OperationState::Distilling(_) => Some("distillation in progress"),
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

    pub fn memory_enabled(&self) -> bool {
        self.memory_enabled
    }

    /// Queue a system notification to be injected into the next API request.
    ///
    /// Notifications are injected as assistant messages, which cannot be forged
    /// by user input, providing a secure channel for system-level communication.
    pub fn queue_notification(&mut self, notification: notifications::SystemNotification) {
        self.notification_queue.push(notification);
    }

    /// API usage from the last completed turn (for status bar display).
    pub fn last_turn_usage(&self) -> Option<&TurnUsage> {
        self.last_turn_usage.as_ref()
    }

    #[allow(clippy::unused_self)] // Kept as method for API consistency
    fn idle_state(&self) -> OperationState {
        if self.tool_journal_disabled_reason.is_some() {
            OperationState::ToolsDisabled(state::ToolsDisabledState)
        } else {
            OperationState::Idle
        }
    }

    fn replace_with_idle(&mut self) -> OperationState {
        let idle = self.idle_state();
        std::mem::replace(&mut self.state, idle)
    }

    fn build_basic_api_messages(&mut self, reserved_overhead: u32) -> Vec<Message> {
        let budget = self
            .context_manager
            .current_limits()
            .effective_input_budget()
            .saturating_sub(reserved_overhead);
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

        let clamped = match current.thinking() {
            ThinkingState::Enabled(budget) => {
                let budget_tokens = budget.as_u32();
                if budget_tokens < model_max_output {
                    OutputLimits::with_thinking(model_max_output, budget_tokens)
                        .unwrap_or(OutputLimits::new(model_max_output))
                } else {
                    OutputLimits::new(model_max_output)
                }
            }
            ThinkingState::Disabled => OutputLimits::new(model_max_output),
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

    /// Set a specific model (called from :model command).
    ///
    /// Persists the model to `~/.forge/config.toml` for future sessions.
    pub fn set_model(&mut self, model: ModelName) {
        self.model = model.clone();
        if self.memory_enabled() {
            self.handle_context_adaptation();
        } else {
            self.context_manager
                .set_model_without_adaptation(self.model.clone());
        }

        self.clamp_output_limits_to_model();

        if let Err(e) = config::ForgeConfig::persist_model(model.as_str()) {
            tracing::warn!("Failed to persist model to config: {e}");
        }
    }

    /// Handle context adaptation after a model switch.
    ///
    /// This method is called after `set_model()` to handle the context adaptation result:
    /// - If shrinking with `needs_distillation`, starts background distillation
    /// - If expanding, attempts to restore previously distilled messages
    fn handle_context_adaptation(&mut self) {
        let adaptation = self.context_manager.switch_model(self.model.clone());
        self.invalidate_usage_cache();

        match adaptation {
            ContextAdaptation::NoChange
            | ContextAdaptation::Shrinking {
                needs_distillation: false,
                ..
            } => {}
            ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_distillation: true,
            } => {
                self.push_notification(format!(
                    "Context budget shrank {}k → {}k; distilling...",
                    old_budget / 1000,
                    new_budget / 1000
                ));
                self.start_distillation();
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

    /// Poll background tasks and update wall-clock based timers.
    pub fn tick(&mut self) {
        self.poll_distillation();
        self.poll_tool_loop();
        self.poll_lsp_events();
        self.poll_journal_cleanup();

        let now = Instant::now();

        // Preserve prior spinner cadence (~10Hz), independent of render FPS.
        if now.duration_since(self.last_ui_tick) >= Duration::from_millis(100) {
            self.last_ui_tick = now;
            self.tick = self.tick.wrapping_add(1);
        }

        // Preserve prior autosave cadence (~3s), independent of render FPS.
        if now.duration_since(self.last_session_autosave) >= Duration::from_secs(3) {
            self.last_session_autosave = now;
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

    // File select mode (@ reference feature)

    /// Enter file select mode, scanning files from the current directory.
    pub fn enter_file_select_mode(&mut self) {
        if !self.file_picker.is_scanned() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            self.file_picker.scan_files(&cwd);
        }
        self.input = std::mem::take(&mut self.input).into_file_select();
        if self.view.ui_options.reduced_motion {
            self.view.modal_effect = None;
        } else {
            self.view.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
            self.view.last_frame = Instant::now();
        }
    }

    /// Get the current file select filter text.
    pub fn file_select_filter(&self) -> Option<&str> {
        self.input.file_select_filter()
    }

    /// Get the current file select index.
    pub fn file_select_index(&self) -> Option<usize> {
        self.input.file_select_index()
    }

    /// Get filtered files for display.
    pub fn file_select_files(&self) -> Vec<&ui::FileEntry> {
        self.file_picker.filtered_files()
    }

    /// Get the file picker state for rendering.
    pub fn file_picker(&self) -> &ui::FilePickerState {
        &self.file_picker
    }

    /// Move file selection up.
    pub fn file_select_move_up(&mut self) {
        if let InputState::FileSelect { selected, .. } = &mut self.input
            && *selected > 0
        {
            *selected -= 1;
        }
    }

    /// Move file selection down.
    pub fn file_select_move_down(&mut self) {
        if let InputState::FileSelect { selected, .. } = &mut self.input {
            let max_index = self.file_picker.filtered_count().saturating_sub(1);
            if *selected < max_index {
                *selected += 1;
            }
        }
    }

    /// Update the file select filter and refresh filtered results.
    pub fn file_select_update_filter(&mut self) {
        let filter = self.input.file_select_filter().unwrap_or("").to_string();
        self.file_picker.update_filter(&filter);
        // Reset selection to 0 when filter changes
        if let InputState::FileSelect { selected, .. } = &mut self.input {
            *selected = 0;
        }
    }

    /// Push a character to the file select filter.
    pub fn file_select_push_char(&mut self, c: char) {
        if let Some(filter) = self.input.file_select_filter_mut() {
            filter.enter_char(c);
        }
        self.file_select_update_filter();
    }

    /// Delete a character from the file select filter (backspace).
    pub fn file_select_backspace(&mut self) {
        if let Some(filter) = self.input.file_select_filter_mut() {
            filter.delete_char();
        }
        self.file_select_update_filter();
    }

    /// Confirm file selection - insert the selected file path into the draft.
    pub fn file_select_confirm(&mut self) {
        let Some(index) = self.file_select_index() else {
            self.enter_insert_mode();
            return;
        };

        if let Some(entry) = self.file_picker.get_selected(index) {
            let path = entry.display.clone();
            // Insert the file path at cursor position in the draft
            self.input.draft_mut().enter_text(&path);
        }

        self.enter_insert_mode();
    }

    /// Cancel file selection and return to insert mode.
    pub fn file_select_cancel(&mut self) {
        self.enter_insert_mode();
    }

    pub fn tool_approval_move_up(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        // data_mut() auto-cancels deny confirmation
        let data = approval.data_mut();
        if data.cursor == 0 {
            return;
        }
        data.cursor -= 1;
        data.expanded = None;
    }

    pub fn tool_approval_move_down(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        // data_mut() auto-cancels deny confirmation
        let data = approval.data_mut();
        // Allow cursor to move to Submit (N) and Deny (N+1) buttons
        let max_cursor = data.requests.len() + 1;
        if data.cursor < max_cursor {
            data.cursor += 1;
        }
        data.expanded = None;
    }

    pub fn tool_approval_toggle(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        // data_mut() auto-cancels deny confirmation
        let data = approval.data_mut();
        if data.cursor >= data.selected.len() {
            return;
        }
        data.selected[data.cursor] = !data.selected[data.cursor];
    }

    pub fn tool_approval_toggle_details(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        // data_mut() auto-cancels deny confirmation
        let data = approval.data_mut();
        if data.cursor >= data.requests.len() {
            return;
        }
        if data.expanded == Some(data.cursor) {
            data.expanded = None;
        } else {
            data.expanded = Some(data.cursor);
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
        let Some(approval) = self.tool_approval_ref() else {
            return;
        };
        let data = approval.data();
        let cursor = data.cursor;
        let num_tools = data.requests.len();

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
                let data = approval.data();
                data.requests
                    .iter()
                    .zip(data.selected.iter())
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
            // Check if already confirming BEFORE any mutations
            if approval.is_confirming_deny() {
                // Second 'd' press - execute denial
                true
            } else {
                // First 'd' press - move cursor and enter confirmation
                let deny_cursor = approval.data().requests.len() + 1;
                approval.data_mut().cursor = deny_cursor;
                approval.enter_deny_confirmation();
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

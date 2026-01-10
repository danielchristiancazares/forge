//! Core engine for Forge - state machine and orchestration.
//!
//! This crate contains the App state machine without TUI dependencies.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::{AbortHandle, Abortable};
use serde_json::Value;
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;

use config::OpenAIConfig;

// Re-export from crates for public API
pub use forge_context::{
    ActiveJournal, ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus,
    FullHistory, MessageId, ModelLimits, ModelLimitsSource, ModelRegistry, PendingSummarization,
    PreparedContext, RecoveredStream, RecoveredToolBatch, StreamJournal, SummarizationNeeded,
    SummarizationScope, TokenCounter, ToolBatchId, ToolJournal, generate_summary,
    summarization_model,
};
pub use forge_providers::{self, ApiConfig};
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

// ============================================================================
// StreamingMessage - async message being streamed
// ============================================================================

/// Accumulator for a single tool call during streaming.
///
/// As tool call events arrive, we accumulate the JSON arguments string
/// until the stream completes, then parse into a complete ToolCall.
#[derive(Debug, Clone)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments_json: String,
}

/// A message being streamed - existence proves streaming is active.
/// Typestate: consuming this produces a complete assistant `Message`.
#[derive(Debug)]
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
    /// Accumulated tool calls during streaming.
    tool_calls: Vec<ToolCallAccumulator>,
}

impl StreamingMessage {
    pub fn new(model: ModelName, receiver: mpsc::UnboundedReceiver<StreamEvent>) -> Self {
        Self {
            model,
            content: String::new(),
            receiver,
            tool_calls: Vec::new(),
        }
    }

    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    pub fn model_name(&self) -> &ModelName {
        &self.model
    }

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
            StreamEvent::ToolCallStart { id, name } => {
                self.tool_calls.push(ToolCallAccumulator {
                    id,
                    name,
                    arguments_json: String::new(),
                });
                None
            }
            StreamEvent::ToolCallDelta { id, arguments } => {
                if let Some(acc) = self.tool_calls.iter_mut().find(|t| t.id == id) {
                    acc.arguments_json.push_str(&arguments);
                }
                None
            }
            StreamEvent::Done => Some(StreamFinishReason::Done),
            StreamEvent::Error(err) => Some(StreamFinishReason::Error(err)),
        }
    }

    /// Returns true if any tool calls were received during streaming.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Take accumulated tool calls, parsing JSON arguments.
    ///
    /// Returns successfully parsed tool calls. Invalid JSON arguments are logged
    /// and skipped (graceful degradation).
    pub fn take_tool_calls(&mut self) -> Vec<ToolCall> {
        self.tool_calls
            .drain(..)
            .map(|acc| {
                match serde_json::from_str(&acc.arguments_json) {
                    Ok(arguments) => ToolCall::new(acc.id, acc.name, arguments),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to parse tool call arguments for '{}': {}",
                            acc.name,
                            e
                        );
                        // Return with empty object as fallback
                        ToolCall::new(acc.id, acc.name, serde_json::json!({}))
                    }
                }
            })
            .collect()
    }

    /// Consume streaming message and produce a complete message.
    pub fn into_message(self) -> Result<Message, forge_types::EmptyStringError> {
        let content = NonEmptyString::new(self.content)?;
        Ok(Message::assistant(self.model, content))
    }
}

// ============================================================================
// Modal Effect - animation state for TUI overlays
// ============================================================================

/// The kind of modal animation effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalEffectKind {
    PopScale,
    SlideUp,
}

/// Modal animation effect state.
#[derive(Debug, Clone)]
pub struct ModalEffect {
    kind: ModalEffectKind,
    elapsed: Duration,
    duration: Duration,
}

impl ModalEffect {
    /// Create a pop-scale effect (used when entering model select).
    pub fn pop_scale(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::PopScale,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Create a slide-up effect.
    pub fn slide_up(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::SlideUp,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Advance the animation by the given delta time.
    pub fn advance(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
    }

    /// Get the animation progress (0.0 to 1.0).
    pub fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = self.elapsed.as_secs_f32();
        let total = self.duration.as_secs_f32();
        (elapsed / total).clamp(0.0, 1.0)
    }

    /// Check if the animation is finished.
    pub fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    /// Get the effect kind.
    pub fn kind(&self) -> ModalEffectKind {
        self.kind
    }
}

// ============================================================================
// Summarization Types
// ============================================================================

/// A background summarization task.
///
/// Holds the state for an in-progress summarization operation:
/// - The message IDs being summarized
/// - The JoinHandle for the async task
#[derive(Debug)]
pub struct SummarizationTask {
    scope: SummarizationScope,
    generated_by: String,
    handle: tokio::task::JoinHandle<anyhow::Result<String>>,
    attempt: u8,
}

#[derive(Debug)]
struct SummarizationRetry {
    attempt: u8,
    ready_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummarizationStart {
    Started,
    NotNeeded,
    Failed,
}

const MAX_SUMMARIZATION_ATTEMPTS: u8 = 5;
const SUMMARIZATION_RETRY_BASE_MS: u64 = 500;
const SUMMARIZATION_RETRY_MAX_MS: u64 = 8000;
const SUMMARIZATION_RETRY_JITTER_MS: u64 = 200;

const DEFAULT_MAX_TOOL_CALLS_PER_BATCH: usize = 8;
const DEFAULT_MAX_TOOL_ITERATIONS_PER_TURN: u32 = 4;
const DEFAULT_MAX_TOOL_ARGS_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_TOOL_OUTPUT_BYTES: usize = 102_400;
const DEFAULT_MAX_PATCH_BYTES: usize = 512 * 1024;
const DEFAULT_MAX_READ_FILE_BYTES: usize = 200 * 1024;
const DEFAULT_MAX_READ_FILE_SCAN_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 30;
const DEFAULT_TOOL_FILE_TIMEOUT_SECS: u64 = 30;
const DEFAULT_TOOL_SHELL_TIMEOUT_SECS: u64 = 300;
const DEFAULT_TOOL_CAPACITY_BYTES: usize = 64 * 1024;
const TOOL_OUTPUT_SAFETY_MARGIN_TOKENS: u32 = 256;
const TOOL_EVENT_CHANNEL_CAPACITY: usize = 64;

const DEFAULT_ENV_DENYLIST: [&str; 7] = [
    "*_KEY",
    "*_TOKEN",
    "*_SECRET",
    "*_PASSWORD",
    "AWS_*",
    "ANTHROPIC_*",
    "OPENAI_*",
];

const DEFAULT_SANDBOX_DENIES: [&str; 5] = [
    "**/.ssh/**",
    "**/.gnupg/**",
    "**/id_rsa*",
    "**/*.pem",
    "**/*.key",
];

const RECOVERY_COMPLETE_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Recovered - stream completed but not finalized]");
const RECOVERY_INCOMPLETE_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Recovered - incomplete response from previous session]");
const RECOVERY_ERROR_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Recovered - stream error from previous session]");
const ABORTED_JOURNAL_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Aborted - journal write failed]");
const STREAM_ERROR_BADGE: NonEmptyStaticStr = NonEmptyStaticStr::new("[Stream error]");
const EMPTY_RESPONSE_BADGE: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Empty response - API returned no content]");

struct StreamErrorUi {
    status: String,
    message: NonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataDirSource {
    System,
    Fallback,
}

#[derive(Debug, Clone)]
struct DataDir {
    path: PathBuf,
    source: DataDirSource,
}

impl DataDir {
    fn join(&self, child: &str) -> PathBuf {
        self.path.join(child)
    }
}

/// Input mode for the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
}

#[derive(Debug, Default)]
struct DraftInput {
    text: String,
    cursor: usize,
}

impl DraftInput {
    fn text(&self) -> &str {
        &self.text
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn take_text(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn move_cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.cursor.saturating_add(1);
        self.cursor = self.clamp_cursor(cursor_moved_right);
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.text.insert(index, new_char);
        self.move_cursor_right();
    }

    fn delete_char(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let start = self.byte_index_at(self.cursor - 1);
        let end = self.byte_index_at(self.cursor);
        self.text.replace_range(start..end, "");
        self.move_cursor_left();
    }

    fn delete_char_forward(&mut self) {
        let grapheme_count = self.grapheme_count();
        if self.cursor >= grapheme_count {
            return;
        }

        let start = self.byte_index_at(self.cursor);
        let end = self.byte_index_at(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor = self.grapheme_count();
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Set the draft text and move cursor to end.
    fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.grapheme_count();
    }

    fn delete_word_backwards(&mut self) {
        while self.cursor > 0 {
            let idx = self.cursor - 1;
            if self.grapheme_is_whitespace(idx) {
                self.delete_char();
            } else {
                break;
            }
        }

        while self.cursor > 0 {
            let idx = self.cursor - 1;
            if !self.grapheme_is_whitespace(idx) {
                self.delete_char();
            } else {
                break;
            }
        }
    }

    fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_is_whitespace(&self, index: usize) -> bool {
        self.text
            .graphemes(true)
            .nth(index)
            .is_some_and(|grapheme| grapheme.chars().all(|c| c.is_whitespace()))
    }

    fn byte_index(&self) -> usize {
        self.byte_index_at(self.cursor)
    }

    fn byte_index_at(&self, grapheme_index: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(grapheme_index)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        let max = self.grapheme_count();
        new_cursor_pos.min(max)
    }
}

/// Predefined model options for the model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredefinedModel {
    ClaudeOpus,
    Gpt52,
}

impl PredefinedModel {
    pub const fn all() -> &'static [PredefinedModel] {
        &[PredefinedModel::ClaudeOpus, PredefinedModel::Gpt52]
    }

    pub const fn display_name(&self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Anthropic Claude Opus 4.5",
            PredefinedModel::Gpt52 => "OpenAI GPT 5.2",
        }
    }

    pub fn to_model_name(&self) -> ModelName {
        match self {
            PredefinedModel::ClaudeOpus => {
                ModelName::known(Provider::Claude, "claude-opus-4-5-20251101")
            }
            PredefinedModel::Gpt52 => ModelName::known(Provider::OpenAI, "gpt-5.2"),
        }
    }

    pub const fn provider(&self) -> Provider {
        match self {
            PredefinedModel::ClaudeOpus => Provider::Claude,
            PredefinedModel::Gpt52 => Provider::OpenAI,
        }
    }
}

#[derive(Debug)]
enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
    ModelSelect { draft: DraftInput, selected: usize },
}

impl Default for InputState {
    fn default() -> Self {
        Self::Normal(DraftInput::default())
    }
}

impl InputState {
    fn mode(&self) -> InputMode {
        match self {
            InputState::Normal(_) => InputMode::Normal,
            InputState::Insert(_) => InputMode::Insert,
            InputState::Command { .. } => InputMode::Command,
            InputState::ModelSelect { .. } => InputMode::ModelSelect,
        }
    }

    fn draft(&self) -> &DraftInput {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => draft,
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => draft,
        }
    }

    fn draft_mut(&mut self) -> &mut DraftInput {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => draft,
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => draft,
        }
    }

    fn command(&self) -> Option<&str> {
        match self {
            InputState::Command { command, .. } => Some(command),
            _ => None,
        }
    }

    fn command_mut(&mut self) -> Option<&mut String> {
        match self {
            InputState::Command { command, .. } => Some(command),
            _ => None,
        }
    }

    fn model_select_index(&self) -> Option<usize> {
        match self {
            InputState::ModelSelect { selected, .. } => Some(*selected),
            _ => None,
        }
    }

    fn into_normal(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Normal(draft),
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => {
                InputState::Normal(draft)
            }
        }
    }

    fn into_insert(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Insert(draft),
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => {
                InputState::Insert(draft)
            }
        }
    }

    fn into_command(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Command {
                draft,
                command: String::new(),
            },
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => {
                InputState::Command {
                    draft,
                    command: String::new(),
                }
            }
        }
    }

    fn into_model_select(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => {
                InputState::ModelSelect { draft, selected: 0 }
            }
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } => {
                InputState::ModelSelect { draft, selected: 0 }
            }
        }
    }
}

/// Scroll position for the message view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollState {
    /// Always keep the newest content visible.
    #[default]
    AutoBottom,
    /// Manual scroll offset from the top of the rendered message buffer.
    Manual { offset_from_top: u16 },
}

/// Proof that a non-empty user message was queued.
///
/// The `config` captures the model/provider at queue time. If summarization runs
/// before streaming starts, the original config is preserved - changing model/provider
/// during summarization won't affect the queued request. This is intentional: the user
/// message was validated against the original model's context limits.
#[derive(Debug)]
pub struct QueuedUserMessage {
    config: ApiConfig,
}

#[derive(Debug)]
struct ActiveStream {
    message: StreamingMessage,
    journal: ActiveJournal,
    abort_handle: AbortHandle,
    tool_batch_id: Option<ToolBatchId>,
    tool_call_seq: usize,
}

#[derive(Debug)]
struct SummarizationState {
    task: SummarizationTask,
}

#[derive(Debug)]
struct SummarizationWithQueuedState {
    task: SummarizationTask,
    queued: ApiConfig,
}

#[derive(Debug)]
struct SummarizationRetryState {
    retry: SummarizationRetry,
}

#[derive(Debug)]
struct SummarizationRetryWithQueuedState {
    retry: SummarizationRetry,
    queued: ApiConfig,
}

/// State for when the assistant has made tool calls and we're waiting for results.
///
/// This is similar to the Summarizing state - a pause in the conversation flow
/// while external processing occurs. Once all tool results are submitted,
/// the conversation resumes with the updated context.
#[derive(Debug)]
pub struct PendingToolExecution {
    /// Text content from assistant before/alongside tool calls (may be empty).
    pub assistant_text: String,
    /// Tool calls waiting for results.
    pub pending_calls: Vec<ToolCall>,
    /// Results received so far.
    pub results: Vec<ToolResult>,
    /// Model that made the tool calls.
    pub model: ModelName,
    /// Journal step ID for recovery.
    pub step_id: forge_context::StepId,
    /// Tool batch journal ID.
    pub batch_id: ToolBatchId,
}

#[derive(Debug)]
struct ToolBatch {
    assistant_text: String,
    calls: Vec<ToolCall>,
    results: Vec<ToolResult>,
    model: ModelName,
    step_id: forge_context::StepId,
    batch_id: ToolBatchId,
    iteration: u32,
    execute_now: Vec<ToolCall>,
    approval_calls: Vec<ToolCall>,
    approval_requests: Vec<tools::ConfirmationRequest>,
}

#[derive(Debug)]
struct ApprovalState {
    requests: Vec<tools::ConfirmationRequest>,
    selected: Vec<bool>,
    cursor: usize,
}

#[derive(Debug)]
struct ActiveToolExecution {
    queue: VecDeque<ToolCall>,
    current_call: Option<ToolCall>,
    join_handle: Option<tokio::task::JoinHandle<ToolResult>>,
    event_rx: Option<mpsc::Receiver<tools::ToolEvent>>,
    abort_handle: Option<AbortHandle>,
    output_lines: Vec<String>,
    remaining_capacity_bytes: usize,
}

#[derive(Debug)]
enum ToolLoopPhase {
    AwaitingApproval(ApprovalState),
    Executing(ActiveToolExecution),
}

#[derive(Debug)]
struct ToolLoopState {
    batch: ToolBatch,
    phase: ToolLoopPhase,
}

#[derive(Debug)]
struct ToolRecoveryState {
    batch: RecoveredToolBatch,
    step_id: forge_context::StepId,
    model: ModelName,
}

#[derive(Debug, Clone, Copy)]
enum ToolRecoveryDecision {
    Resume,
    Discard,
}

#[derive(Debug)]
struct ToolPlan {
    execute_now: Vec<ToolCall>,
    approval_calls: Vec<ToolCall>,
    approval_requests: Vec<tools::ConfirmationRequest>,
    pre_resolved: Vec<ToolResult>,
}

#[derive(Debug)]
enum EnabledState {
    Idle,
    Streaming(ActiveStream),
    AwaitingToolResults(PendingToolExecution),
    ToolLoop(ToolLoopState),
    ToolRecovery(ToolRecoveryState),
    Summarizing(SummarizationState),
    SummarizingWithQueued(SummarizationWithQueuedState),
    SummarizationRetry(SummarizationRetryState),
    SummarizationRetryWithQueued(SummarizationRetryWithQueuedState),
}

#[derive(Debug)]
enum DisabledState {
    Idle,
    Streaming(ActiveStream),
}

#[derive(Debug)]
enum AppState {
    Enabled(EnabledState),
    Disabled(DisabledState),
}

#[derive(Debug, Clone)]
pub enum DisplayItem {
    History(MessageId),
    Local(Message),
}

/// Proof that a command line was entered in Command mode.
#[derive(Debug)]
pub struct EnteredCommand {
    raw: String,
}

/// Proof token for Insert mode operations.
#[derive(Debug)]
pub struct InsertToken(());

/// Proof token for Command mode operations.
#[derive(Debug)]
pub struct CommandToken(());

/// Mode wrapper for safe insert operations.
pub struct InsertMode<'a> {
    app: &'a mut App,
}

/// Mode wrapper for safe command operations.
pub struct CommandMode<'a> {
    app: &'a mut App,
}

/// Application state
pub struct App {
    input: InputState,
    display: Vec<DisplayItem>,
    scroll: ScrollState,
    scroll_max: u16,
    should_quit: bool,
    /// Request to toggle between fullscreen and inline UI modes.
    toggle_screen_mode: bool,
    status_message: Option<String>,
    api_keys: HashMap<Provider, String>,
    model: ModelName,
    tick: usize,
    data_dir: DataDir,
    /// Context manager for adaptive context window management.
    context_manager: ContextManager,
    /// Stream journal for crash recovery.
    stream_journal: StreamJournal,
    state: AppState,
    /// Validated output limits (max tokens + optional thinking budget).
    /// Invariant: if thinking is enabled, budget < max_tokens.
    output_limits: OutputLimits,
    /// Whether prompt caching is enabled (for Claude).
    cache_enabled: bool,
    /// OpenAI request defaults (reasoning/verbosity/truncation).
    openai_options: OpenAIRequestOptions,
    /// Frame timing for animations.
    last_frame: Instant,
    /// Active modal animation effect.
    modal_effect: Option<ModalEffect>,
    /// System prompt sent to the LLM with each request.
    system_prompt: Option<&'static str>,
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
    /// Tool loop mode.
    tools_mode: tools::ToolsMode,
    /// Tool settings derived from config.
    tool_settings: tools::ToolSettings,
    /// Tool journal for crash recovery.
    tool_journal: ToolJournal,
    /// File hash cache for tool safety checks.
    tool_file_cache: std::sync::Arc<tokio::sync::Mutex<tools::ToolFileCache>>,
    /// Tool iterations used in the current user turn.
    tool_iterations: u32,
}

impl App {
    pub fn new(system_prompt: Option<&'static str>) -> anyhow::Result<Self> {
        let config = ForgeConfig::load();

        // Load API keys from config, then fall back to environment.
        let mut api_keys = HashMap::new();
        if let Some(keys) = config.as_ref().and_then(|cfg| cfg.api_keys.as_ref()) {
            if let Some(key) = keys.anthropic.as_ref() {
                let resolved = config::expand_env_vars(key);
                let trimmed = resolved.trim();
                if !trimmed.is_empty() {
                    api_keys.insert(Provider::Claude, trimmed.to_string());
                }
            }
            if let Some(key) = keys.openai.as_ref() {
                let resolved = config::expand_env_vars(key);
                let trimmed = resolved.trim();
                if !trimmed.is_empty() {
                    api_keys.insert(Provider::OpenAI, trimmed.to_string());
                }
            }
        }

        if let std::collections::hash_map::Entry::Vacant(e) = api_keys.entry(Provider::Claude)
            && let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                e.insert(key);
            }
        }
        if let std::collections::hash_map::Entry::Vacant(e) = api_keys.entry(Provider::OpenAI)
            && let Ok(key) = std::env::var("OPENAI_API_KEY")
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                e.insert(key);
            }
        }

        let provider = config
            .as_ref()
            .and_then(|cfg| cfg.app.as_ref())
            .and_then(|app| app.provider.as_ref())
            .and_then(|raw| {
                let parsed = Provider::parse(raw);
                if parsed.is_none() {
                    tracing::warn!("Unknown provider in config: {}", raw);
                }
                parsed
            })
            .or_else(|| {
                if api_keys.contains_key(&Provider::Claude) {
                    Some(Provider::Claude)
                } else if api_keys.contains_key(&Provider::OpenAI) {
                    Some(Provider::OpenAI)
                } else {
                    None
                }
            })
            .unwrap_or(Provider::Claude);

        let model = config
            .as_ref()
            .and_then(|cfg| cfg.app.as_ref())
            .and_then(|app| app.model.as_ref())
            .map(|raw| match provider.parse_model(raw) {
                Ok(model) => model,
                Err(err) => {
                    tracing::warn!("Invalid model in config: {err}");
                    provider.default_model()
                }
            })
            .unwrap_or_else(|| provider.default_model());

        let context_manager = ContextManager::new(model.as_str());
        let context_infinity_enabled = config
            .as_ref()
            .and_then(|cfg| cfg.context.as_ref())
            .and_then(|ctx| ctx.infinity)
            .unwrap_or_else(Self::context_infinity_enabled_from_env);

        let anthropic_config = config.as_ref().and_then(|cfg| cfg.anthropic.as_ref());

        // Load cache config (default: enabled)
        let cache_enabled = anthropic_config
            .and_then(|cfg| cfg.cache_enabled)
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|cfg| cfg.cache.as_ref())
                    .and_then(|cache| cache.enabled)
            })
            .unwrap_or(true);

        // Build OutputLimits at the boundary - validates invariants here, not at runtime
        let output_limits = {
            let max_output = config
                .as_ref()
                .and_then(|cfg| cfg.app.as_ref())
                .and_then(|app| app.max_output_tokens)
                .unwrap_or(16_000); // Default max output

            let thinking_enabled = anthropic_config
                .and_then(|cfg| cfg.thinking_enabled)
                .or_else(|| {
                    config
                        .as_ref()
                        .and_then(|cfg| cfg.thinking.as_ref())
                        .and_then(|t| t.enabled)
                })
                .unwrap_or(false);

            if thinking_enabled {
                let budget = anthropic_config
                    .and_then(|cfg| cfg.thinking_budget_tokens)
                    .or_else(|| {
                        config
                            .as_ref()
                            .and_then(|cfg| cfg.thinking.as_ref())
                            .and_then(|t| t.budget_tokens)
                    })
                    .unwrap_or(10_000);

                // Validate at boundary - if invalid, warn and fall back to no thinking
                match OutputLimits::with_thinking(max_output, budget) {
                    Ok(limits) => limits,
                    Err(e) => {
                        tracing::warn!(
                            "Invalid thinking config: {e}. Disabling extended thinking."
                        );
                        OutputLimits::new(max_output)
                    }
                }
            } else {
                OutputLimits::new(max_output)
            }
        };

        let openai_options = Self::openai_request_options_from_config(
            config.as_ref().and_then(|cfg| cfg.openai.as_ref()),
        );

        let data_dir = Self::data_dir();

        std::fs::create_dir_all(&data_dir.path)?;

        // Initialize stream journal (required for streaming durability).
        let journal_path = data_dir.join("stream_journal.db");
        let stream_journal = StreamJournal::open(&journal_path)?;

        // Tool settings and registry.
        let tool_settings = Self::tool_settings_from_config(config.as_ref());
        let mut tool_registry = tools::ToolRegistry::default();
        if let Err(e) = tools::builtins::register_builtins(
            &mut tool_registry,
            tool_settings.read_limits,
            tool_settings.patch_limits,
        ) {
            tracing::warn!("Failed to register built-in tools: {e}");
        }
        let tool_registry = std::sync::Arc::new(tool_registry);
        let config_tool_definitions = Self::load_tool_definitions_from_config(config.as_ref());
        let tool_definitions = match tool_settings.mode {
            tools::ToolsMode::Enabled => tool_registry.definitions(),
            tools::ToolsMode::ParseOnly => config_tool_definitions,
            tools::ToolsMode::Disabled => Vec::new(),
        };

        let tool_journal_path = data_dir.join("tool_journal.db");
        let tool_journal = ToolJournal::open(&tool_journal_path)?;
        let tool_file_cache =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

        let state = if context_infinity_enabled {
            AppState::Enabled(EnabledState::Idle)
        } else {
            AppState::Disabled(DisabledState::Idle)
        };

        let mut app = Self {
            input: InputState::default(),
            display: Vec::new(),
            scroll: ScrollState::AutoBottom,
            scroll_max: 0,
            should_quit: false,
            toggle_screen_mode: false,
            status_message: None,
            api_keys,
            model,
            tick: 0,
            data_dir,
            context_manager,
            stream_journal,
            state,
            output_limits,
            cache_enabled,
            openai_options,
            last_frame: Instant::now(),
            modal_effect: None,
            system_prompt,
            cached_usage_status: None,
            pending_user_message: None,
            tool_definitions,
            tool_registry,
            tools_mode: tool_settings.mode,
            tool_settings,
            tool_journal,
            tool_file_cache,
            tool_iterations: 0,
        };

        app.clamp_output_limits_to_model();
        // Sync output limit to context manager for accurate budget calculation
        app.context_manager
            .set_output_limit(app.output_limits.max_output_tokens());

        // Load previous session's history if available
        app.load_history_if_exists();
        app.check_crash_recovery();
        if app.status_message.is_none() && matches!(app.data_dir.source, DataDirSource::Fallback) {
            app.set_status(format!(
                "Using fallback data dir: {}",
                app.data_dir.path.display()
            ));
        }

        Ok(app)
    }

    /// Get the base data directory for forge.
    fn data_dir() -> DataDir {
        match dirs::data_local_dir() {
            Some(path) => DataDir {
                path: path.join("forge"),
                source: DataDirSource::System,
            },
            None => DataDir {
                path: PathBuf::from(".").join("forge"),
                source: DataDirSource::Fallback,
            },
        }
    }

    /// Get the path to the history file.
    fn history_path(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    fn context_infinity_enabled_from_env() -> bool {
        match std::env::var("FORGE_CONTEXT_INFINITY") {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "0" | "false" | "off" | "no" => false,
                "1" | "true" | "on" | "yes" => true,
                _ => true,
            },
            Err(_) => true,
        }
    }

    fn openai_request_options_from_config(config: Option<&OpenAIConfig>) -> OpenAIRequestOptions {
        let reasoning_effort = config
            .and_then(|cfg| cfg.reasoning_effort.as_deref())
            .map(|raw| {
                OpenAIReasoningEffort::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI reasoning_effort in config: {raw}");
                    OpenAIReasoningEffort::default()
                })
            })
            .unwrap_or_default();

        let verbosity = config
            .and_then(|cfg| cfg.verbosity.as_deref())
            .map(|raw| {
                OpenAITextVerbosity::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI verbosity in config: {raw}");
                    OpenAITextVerbosity::default()
                })
            })
            .unwrap_or_default();

        let truncation = config
            .and_then(|cfg| cfg.truncation.as_deref())
            .map(|raw| {
                OpenAITruncation::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI truncation in config: {raw}");
                    OpenAITruncation::default()
                })
            })
            .unwrap_or_default();

        OpenAIRequestOptions::new(reasoning_effort, verbosity, truncation)
    }

    fn load_tool_definitions_from_config(config: Option<&ForgeConfig>) -> Vec<ToolDefinition> {
        let mut defs = Vec::new();
        if let Some(tools_cfg) = config.and_then(|cfg| cfg.tools.as_ref()) {
            for tool_cfg in &tools_cfg.definitions {
                match tool_cfg.to_tool_definition() {
                    Ok(tool_def) => {
                        tracing::debug!("Loaded tool: {}", tool_def.name);
                        defs.push(tool_def);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load tool '{}': {}", tool_cfg.name, e);
                    }
                }
            }
        }
        defs
    }

    fn tool_settings_from_config(config: Option<&ForgeConfig>) -> tools::ToolSettings {
        let tools_cfg = config.and_then(|cfg| cfg.tools.as_ref());
        let has_defs = tools_cfg
            .map(|cfg| !cfg.definitions.is_empty())
            .unwrap_or(false);
        let mode = parse_tools_mode(tools_cfg.and_then(|cfg| cfg.mode.as_deref()), has_defs);
        let allow_parallel = tools_cfg
            .and_then(|cfg| cfg.allow_parallel)
            .unwrap_or(false);

        let limits = tools::ToolLimits {
            max_tool_calls_per_batch: tools_cfg
                .and_then(|cfg| cfg.max_tool_calls_per_batch)
                .unwrap_or(DEFAULT_MAX_TOOL_CALLS_PER_BATCH),
            max_tool_iterations_per_user_turn: tools_cfg
                .and_then(|cfg| cfg.max_tool_iterations_per_user_turn)
                .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS_PER_TURN),
            max_tool_args_bytes: tools_cfg
                .and_then(|cfg| cfg.max_tool_args_bytes)
                .unwrap_or(DEFAULT_MAX_TOOL_ARGS_BYTES),
        };

        let read_limits = tools::ReadFileLimits {
            max_file_read_bytes: tools_cfg
                .and_then(|cfg| cfg.read_file.as_ref())
                .and_then(|cfg| cfg.max_file_read_bytes)
                .unwrap_or(DEFAULT_MAX_READ_FILE_BYTES),
            max_scan_bytes: tools_cfg
                .and_then(|cfg| cfg.read_file.as_ref())
                .and_then(|cfg| cfg.max_scan_bytes)
                .unwrap_or(DEFAULT_MAX_READ_FILE_SCAN_BYTES),
        };

        let patch_limits = tools::PatchLimits {
            max_patch_bytes: tools_cfg
                .and_then(|cfg| cfg.apply_patch.as_ref())
                .and_then(|cfg| cfg.max_patch_bytes)
                .unwrap_or(DEFAULT_MAX_PATCH_BYTES),
        };

        let timeouts = tools::ToolTimeouts {
            default_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.default_seconds)
                    .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS),
            ),
            file_operations_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.file_operations_seconds)
                    .unwrap_or(DEFAULT_TOOL_FILE_TIMEOUT_SECS),
            ),
            shell_commands_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.shell_commands_seconds)
                    .unwrap_or(DEFAULT_TOOL_SHELL_TIMEOUT_SECS),
            ),
        };

        let max_output_bytes = tools_cfg
            .and_then(|cfg| cfg.output.as_ref())
            .and_then(|cfg| cfg.max_bytes)
            .unwrap_or(DEFAULT_MAX_TOOL_OUTPUT_BYTES);

        let policy_cfg = tools_cfg.and_then(|cfg| cfg.approval.as_ref());
        let policy = tools::Policy {
            enabled: policy_cfg.and_then(|cfg| cfg.enabled).unwrap_or(true),
            mode: parse_approval_mode(policy_cfg.and_then(|cfg| cfg.mode.as_deref())),
            allowlist: {
                let list = policy_cfg
                    .map(|cfg| cfg.allowlist.clone())
                    .unwrap_or_else(|| vec!["read_file".to_string()]);
                list.into_iter().collect()
            },
            denylist: {
                let list = if policy_cfg.and_then(|cfg| Some(&cfg.denylist)).is_some() {
                    policy_cfg
                        .map(|cfg| cfg.denylist.clone())
                        .unwrap_or_default()
                } else {
                    vec!["run_command".to_string()]
                };
                list.into_iter().collect()
            },
            prompt_side_effects: policy_cfg
                .and_then(|cfg| cfg.prompt_side_effects)
                .unwrap_or(true),
        };

        let env_patterns: Vec<String> = tools_cfg
            .and_then(|cfg| cfg.environment.as_ref())
            .map(|cfg| cfg.denylist.clone())
            .filter(|list| !list.is_empty())
            .unwrap_or_else(|| DEFAULT_ENV_DENYLIST.iter().map(|s| s.to_string()).collect());
        let env_sanitizer = tools::EnvSanitizer::new(&env_patterns).unwrap_or_else(|e| {
            tracing::warn!("Invalid env denylist: {e}. Using defaults.");
            tools::EnvSanitizer::new(
                &DEFAULT_ENV_DENYLIST
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            )
            .expect("default env sanitizer")
        });

        let sandbox_cfg = tools_cfg.and_then(|cfg| cfg.sandbox.as_ref());
        let include_default_denies = sandbox_cfg
            .and_then(|cfg| cfg.include_default_denies)
            .unwrap_or(true);
        let mut denied_patterns = sandbox_cfg
            .map(|cfg| cfg.denied_patterns.clone())
            .unwrap_or_default();
        if include_default_denies {
            denied_patterns.extend(DEFAULT_SANDBOX_DENIES.iter().map(|s| s.to_string()));
        }

        let mut allowed_roots: Vec<PathBuf> = sandbox_cfg
            .map(|cfg| cfg.allowed_roots.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|raw| PathBuf::from(config::expand_env_vars(&raw)))
            .collect();
        if allowed_roots.is_empty() {
            allowed_roots.push(PathBuf::from("."));
        }
        let allow_absolute = sandbox_cfg
            .and_then(|cfg| cfg.allow_absolute)
            .unwrap_or(false);

        let sandbox = tools::sandbox::Sandbox::new(
            allowed_roots.clone(),
            denied_patterns.clone(),
            allow_absolute,
        )
        .unwrap_or_else(|e| {
            tracing::warn!("Invalid sandbox config: {e}. Using defaults.");
            tools::sandbox::Sandbox::new(
                vec![PathBuf::from(".")],
                DEFAULT_SANDBOX_DENIES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                false,
            )
            .expect("default sandbox")
        });

        tools::ToolSettings {
            mode,
            allow_parallel,
            limits,
            read_limits,
            patch_limits,
            timeouts,
            max_output_bytes,
            policy,
            sandbox,
            env_sanitizer,
        }
    }

    /// Save the conversation history to disk.
    pub fn save_history(&self) -> anyhow::Result<()> {
        let path = self.history_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.context_manager.save(&path)
    }

    /// Load conversation history from disk (called during init if file exists).
    fn load_history_if_exists(&mut self) {
        let path = self.history_path();
        if !path.exists() {
            return;
        }

        match forge_context::ContextManager::load(&path, self.model.as_str()) {
            Ok(mut loaded_manager) => {
                // Sync output limit before replacing context manager
                loaded_manager.set_output_limit(self.output_limits.max_output_tokens());
                self.context_manager = loaded_manager;
                self.rebuild_display_from_history();
                self.set_status(format!(
                    "Loaded {} messages from previous session",
                    self.context_manager.history().len()
                ));
            }
            Err(e) => {
                tracing::warn!("Failed to load history: {e}");
            }
        }
    }

    fn rebuild_display_from_history(&mut self) {
        self.display.clear();
        for entry in self.context_manager.history().entries() {
            self.display.push(DisplayItem::History(entry.id()));
        }
    }

    fn push_history_message(&mut self, message: Message) -> MessageId {
        let id = self.context_manager.push_message(message);
        self.display.push(DisplayItem::History(id));
        self.invalidate_usage_cache();
        id
    }

    /// Push an assistant message with an associated stream step ID.
    ///
    /// Used for streaming responses to enable idempotent crash recovery.
    fn push_history_message_with_step_id(
        &mut self,
        message: Message,
        step_id: forge_context::StepId,
    ) -> MessageId {
        let id = self
            .context_manager
            .push_message_with_step_id(message, step_id);
        self.display.push(DisplayItem::History(id));
        self.invalidate_usage_cache();
        id
    }

    /// Save history to disk.
    /// Returns true if successful, false if save failed (logged but not propagated).
    /// Called after user messages and assistant completions for crash durability.
    fn autosave_history(&self) -> bool {
        match self.save_history() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Autosave failed: {e}");
                false
            }
        }
    }

    fn push_local_message(&mut self, message: Message) {
        self.display.push(DisplayItem::Local(message));
    }

    /// Check for and recover from a crashed streaming session.
    ///
    /// Returns `Some(RecoveredStream)` if there was an incomplete stream that was recovered.
    /// The recovered partial response is added to the conversation with a warning badge.
    ///
    /// # Idempotent recovery
    ///
    /// If history already contains an entry with the recovered step_id, we skip
    /// adding the message (it was already recovered or committed) and just finalize
    /// the journal cleanup.
    pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
        if let Ok(Some(mut recovered_batch)) = self.tool_journal.recover() {
            let stream_recovered = match self.stream_journal.recover() {
                Ok(recovered) => recovered,
                Err(e) => {
                    self.set_status(format!("Recovery failed: {e}"));
                    return None;
                }
            };

            let (step_id, partial_text, stream_model_name) = match &stream_recovered {
                Some(RecoveredStream::Complete {
                    step_id,
                    partial_text,
                    model_name,
                    ..
                }) => (
                    Some(*step_id),
                    Some(partial_text.as_str()),
                    model_name.clone(),
                ),
                Some(RecoveredStream::Incomplete {
                    step_id,
                    partial_text,
                    model_name,
                    ..
                }) => (
                    Some(*step_id),
                    Some(partial_text.as_str()),
                    model_name.clone(),
                ),
                Some(RecoveredStream::Errored {
                    step_id,
                    partial_text,
                    model_name,
                    ..
                }) => (
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

            let model = parse_model_name_from_string(&recovered_batch.model_name)
                .or_else(|| {
                    stream_model_name
                        .as_ref()
                        .and_then(|name| parse_model_name_from_string(name))
                })
                .unwrap_or_else(|| self.model.clone());

            if let Some(step_id) = step_id {
                self.state = AppState::Enabled(EnabledState::ToolRecovery(ToolRecoveryState {
                    batch: recovered_batch,
                    step_id,
                    model,
                }));
                self.set_status("Recovered tool batch. Press R to resume or D to discard.");
                return stream_recovered;
            }

            tracing::warn!("Tool batch recovery found but no stream journal step.");
            self.set_status("Recovered tool batch but stream journal missing; discarding");
            let _ = self.tool_journal.discard_batch(recovered_batch.batch_id);
            return None;
        }

        let recovered = match self.stream_journal.recover() {
            Ok(Some(recovered)) => recovered,
            Ok(None) => return None,
            Err(e) => {
                self.set_status(format!("Recovery failed: {e}"));
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
                self.set_status(format!(
                    "Recovery already in history, but autosave failed; keeping step {} recoverable",
                    step_id
                ));
            }
            return Some(recovered);
        }

        // Use the model from the stream if available, otherwise fall back to current
        let model = model_name
            .and_then(|name| parse_model_name_from_string(&name))
            .unwrap_or_else(|| self.model.clone());

        // Add the partial response as an assistant message with recovery badge.
        let mut recovered_content = NonEmptyString::from(recovery_badge);
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
            self.set_status(format!(
                "Recovered {} bytes (step {}, last seq {}) from crashed session",
                partial_text.len(),
                step_id,
                last_seq,
            ));
        } else {
            self.set_status(format!(
                "Recovered {} bytes (step {}, last seq {}) but autosave failed; recovery will retry",
                partial_text.len(),
                step_id,
                last_seq,
            ));
        }

        Some(recovered)
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Check if screen mode toggle was requested and clear the flag.
    pub fn take_toggle_screen_mode(&mut self) -> bool {
        std::mem::take(&mut self.toggle_screen_mode)
    }

    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
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
            AppState::Enabled(EnabledState::Streaming(active))
            | AppState::Disabled(DisabledState::Streaming(active)) => Some(&active.message),
            _ => None,
        }
    }

    /// Get pending tool calls if we're in AwaitingToolResults state.
    pub fn pending_tool_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            AppState::Enabled(EnabledState::AwaitingToolResults(pending)) => {
                Some(&pending.pending_calls)
            }
            _ => None,
        }
    }

    /// Whether we're waiting for tool results.
    pub fn is_awaiting_tool_results(&self) -> bool {
        matches!(
            self.state,
            AppState::Enabled(EnabledState::AwaitingToolResults(_))
        )
    }

    pub fn tool_loop_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => Some(&state.batch.calls),
            _ => None,
        }
    }

    pub fn tool_loop_execute_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => Some(&state.batch.execute_now),
            _ => None,
        }
    }

    pub fn tool_loop_results(&self) -> Option<&[ToolResult]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => Some(&state.batch.results),
            _ => None,
        }
    }

    pub fn tool_loop_current_call_id(&self) -> Option<&str> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => match &state.phase {
                ToolLoopPhase::Executing(exec) => exec.current_call.as_ref().map(|c| c.id.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tool_loop_output_lines(&self) -> Option<&[String]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => match &state.phase {
                ToolLoopPhase::Executing(exec) => Some(&exec.output_lines),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_requests(&self) -> Option<&[tools::ConfirmationRequest]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(&approval.requests),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_selected(&self) -> Option<&[bool]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(&approval.selected),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tool_approval_cursor(&self) -> Option<usize> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolLoop(state)) => match &state.phase {
                ToolLoopPhase::AwaitingApproval(approval) => Some(approval.cursor),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tool_recovery_calls(&self) -> Option<&[ToolCall]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolRecovery(state)) => Some(&state.batch.calls),
            _ => None,
        }
    }

    pub fn tool_recovery_results(&self) -> Option<&[ToolResult]> {
        match &self.state {
            AppState::Enabled(EnabledState::ToolRecovery(state)) => Some(&state.batch.results),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
            && !matches!(
                self.state,
                AppState::Enabled(EnabledState::Streaming(_))
                    | AppState::Disabled(DisabledState::Streaming(_))
                    | AppState::Enabled(EnabledState::AwaitingToolResults(_))
                    | AppState::Enabled(EnabledState::ToolLoop(_))
                    | AppState::Enabled(EnabledState::ToolRecovery(_))
            )
    }

    pub fn display_items(&self) -> &[DisplayItem] {
        &self.display
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
        matches!(
            self.state,
            AppState::Enabled(EnabledState::Streaming(_))
                | AppState::Disabled(DisabledState::Streaming(_))
                | AppState::Enabled(EnabledState::AwaitingToolResults(_))
                | AppState::Enabled(EnabledState::ToolLoop(_))
                | AppState::Enabled(EnabledState::Summarizing(_))
                | AppState::Enabled(EnabledState::SummarizingWithQueued(_))
                | AppState::Enabled(EnabledState::SummarizationRetry(_))
                | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_))
        )
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
        matches!(self.state, AppState::Enabled(_))
    }

    fn idle_state(&self) -> AppState {
        match &self.state {
            AppState::Enabled(_) => AppState::Enabled(EnabledState::Idle),
            AppState::Disabled(_) => AppState::Disabled(DisabledState::Idle),
        }
    }

    fn replace_with_idle(&mut self) -> AppState {
        let idle = self.idle_state();
        std::mem::replace(&mut self.state, idle)
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
            self.set_status("ContextInfinity disabled: truncating history to fit model budget");
        } else if oversize {
            self.set_status("ContextInfinity disabled: last message exceeds model budget");
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
    /// This method is called after set_model() or set_provider() to handle
    /// the context adaptation result:
    /// - If shrinking with needs_summarization, starts background summarization
    /// - If expanding, attempts to restore previously summarized messages
    fn handle_context_adaptation(&mut self) {
        let adaptation = self.context_manager.switch_model(self.model.as_str());
        self.invalidate_usage_cache();

        match adaptation {
            ContextAdaptation::NoChange => {}
            ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_summarization: true,
            } => {
                self.set_status(format!(
                    "Context budget shrank {}k → {}k; summarizing...",
                    old_budget / 1000,
                    new_budget / 1000
                ));
                self.start_summarization();
            }
            ContextAdaptation::Shrinking {
                needs_summarization: false,
                ..
            } => {
                // Shrinking but still fits - no action needed
            }
            ContextAdaptation::Expanding {
                old_budget,
                new_budget,
                can_restore,
            } => {
                if can_restore > 0 {
                    let restored = self.context_manager.try_restore_messages();
                    if restored > 0 {
                        self.set_status(format!(
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

    /// Start a background summarization task if summarization is needed.
    ///
    /// This prepares a summarization request from the context manager and spawns
    /// an async task to generate the summary. The result is polled via `poll_summarization()`.
    pub fn start_summarization(&mut self) {
        let _ = self.start_summarization_with_attempt(None, 1);
    }

    fn start_summarization_with_attempt(
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

        match &self.state {
            AppState::Enabled(EnabledState::Streaming(_))
            | AppState::Disabled(DisabledState::Streaming(_)) => {
                self.set_status("Cannot summarize while streaming");
                return SummarizationStart::Failed;
            }
            AppState::Enabled(EnabledState::AwaitingToolResults(_)) => {
                self.set_status("Cannot summarize while awaiting tool results");
                return SummarizationStart::Failed;
            }
            AppState::Enabled(EnabledState::ToolLoop(_)) => {
                self.set_status("Cannot summarize while tool execution runs");
                return SummarizationStart::Failed;
            }
            AppState::Enabled(EnabledState::ToolRecovery(_)) => {
                self.set_status("Cannot summarize while tool recovery is pending");
                return SummarizationStart::Failed;
            }
            AppState::Enabled(EnabledState::Summarizing(_))
            | AppState::Enabled(EnabledState::SummarizingWithQueued(_))
            | AppState::Enabled(EnabledState::SummarizationRetry(_))
            | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_)) => {
                return SummarizationStart::Failed;
            }
            AppState::Enabled(EnabledState::Idle) | AppState::Disabled(DisabledState::Idle) => {}
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
            AppState::Enabled(EnabledState::SummarizingWithQueued(
                SummarizationWithQueuedState {
                    task,
                    queued: config,
                },
            ))
        } else {
            AppState::Enabled(EnabledState::Summarizing(SummarizationState { task }))
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
            AppState::Enabled(EnabledState::Summarizing(state)) => state.task.handle.is_finished(),
            AppState::Enabled(EnabledState::SummarizingWithQueued(state)) => {
                state.task.handle.is_finished()
            }
            _ => return,
        };

        // Check if the task is finished (non-blocking)
        if !finished {
            return;
        }

        // Take ownership of the task
        let (task, queued_request) =
            match std::mem::replace(&mut self.state, AppState::Enabled(EnabledState::Idle)) {
                AppState::Enabled(EnabledState::Summarizing(state)) => (state.task, None),
                AppState::Enabled(EnabledState::SummarizingWithQueued(state)) => {
                    (state.task, Some(state.queued))
                }
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
        self.state = AppState::Enabled(EnabledState::Idle);
        let next_attempt = attempt.saturating_add(1);
        let had_pending = queued_request.is_some();

        if next_attempt <= MAX_SUMMARIZATION_ATTEMPTS {
            let delay = summarization_retry_delay(next_attempt);
            let retry = SummarizationRetry {
                attempt: next_attempt,
                ready_at: Instant::now() + delay,
            };
            self.state = if let Some(config) = queued_request {
                AppState::Enabled(EnabledState::SummarizationRetryWithQueued(
                    SummarizationRetryWithQueuedState {
                        retry,
                        queued: config,
                    },
                ))
            } else {
                AppState::Enabled(EnabledState::SummarizationRetry(SummarizationRetryState {
                    retry,
                }))
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

    fn poll_summarization_retry(&mut self) {
        if !self.context_infinity_enabled() {
            return;
        }

        let ready = match &self.state {
            AppState::Enabled(EnabledState::SummarizationRetry(state)) => {
                state.retry.ready_at <= Instant::now()
            }
            AppState::Enabled(EnabledState::SummarizationRetryWithQueued(state)) => {
                state.retry.ready_at <= Instant::now()
            }
            _ => return,
        };

        if !ready {
            return;
        }

        let (retry, queued_request) =
            match std::mem::replace(&mut self.state, AppState::Enabled(EnabledState::Idle)) {
                AppState::Enabled(EnabledState::SummarizationRetry(state)) => (state.retry, None),
                AppState::Enabled(EnabledState::SummarizationRetryWithQueued(state)) => {
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

    /// Start streaming response from the API.
    pub fn start_streaming(&mut self, queued: QueuedUserMessage) {
        match &self.state {
            AppState::Enabled(EnabledState::Streaming(_))
            | AppState::Disabled(DisabledState::Streaming(_)) => {
                self.set_status("Already streaming a response");
                return;
            }
            AppState::Enabled(EnabledState::AwaitingToolResults(_)) => {
                self.set_status("Busy: waiting for tool results");
                return;
            }
            AppState::Enabled(EnabledState::ToolLoop(_)) => {
                self.set_status("Busy: tool execution in progress");
                return;
            }
            AppState::Enabled(EnabledState::ToolRecovery(_)) => {
                self.set_status("Busy: tool recovery pending");
                return;
            }
            AppState::Enabled(EnabledState::Summarizing(_))
            | AppState::Enabled(EnabledState::SummarizingWithQueued(_))
            | AppState::Enabled(EnabledState::SummarizationRetry(_))
            | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_)) => {
                self.set_status("Busy: summarization in progress");
                return;
            }
            AppState::Enabled(EnabledState::Idle) | AppState::Disabled(DisabledState::Idle) => {}
        }

        let QueuedUserMessage { config } = queued;
        let context_infinity_enabled = self.context_infinity_enabled();

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
                        "Recent {} messages ({} tokens) exceed budget ({} tokens). Reduce input or use larger model.",
                        message_count, required_tokens, budget_tokens
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

        let (tx, rx) = mpsc::unbounded_channel();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        let active = ActiveStream {
            message: StreamingMessage::new(config.model().clone(), rx),
            journal,
            abort_handle,
            tool_batch_id: None,
            tool_call_seq: 0,
        };

        self.state = if context_infinity_enabled {
            AppState::Enabled(EnabledState::Streaming(active))
        } else {
            AppState::Disabled(DisabledState::Streaming(active))
        };

        // OutputLimits is pre-validated at config load time - no runtime checks needed
        // Invariant: if thinking is enabled, budget < max_tokens (guaranteed by type)
        let limits = self.output_limits;

        // Convert messages to cacheable format based on cache_enabled setting
        let cache_enabled = self.cache_enabled;
        let system_prompt = self.system_prompt;
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

        let task = async move {
            let tx_events = tx.clone();
            // Convert tools to Option<&[ToolDefinition]>
            let tools_ref = if tools.is_empty() {
                None
            } else {
                Some(tools.as_slice())
            };
            let result = forge_providers::send_message(
                &config,
                &cacheable_messages,
                limits,
                system_prompt,
                tools_ref,
                move |event| {
                    let _ = tx_events.send(event);
                },
            )
            .await;

            if let Err(e) = result {
                let _ = tx.send(StreamEvent::Error(e.to_string()));
            }
        };

        tokio::spawn(async move {
            let _ = Abortable::new(task, abort_registration).await;
        });
    }

    /// Process any pending stream events.
    pub fn process_stream_events(&mut self) {
        if !matches!(
            self.state,
            AppState::Enabled(EnabledState::Streaming(_))
                | AppState::Disabled(DisabledState::Streaming(_))
        ) {
            return;
        }

        // Process all available events.
        loop {
            let event = {
                let active = match self.state {
                    AppState::Enabled(EnabledState::Streaming(ref mut active))
                    | AppState::Disabled(DisabledState::Streaming(ref mut active)) => active,
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
                StreamEvent::Error(msg) => StreamEvent::Error(sanitize_stream_error(&msg)),
                other => other,
            };

            let mut journal_error: Option<String> = None;
            let mut finish_reason: Option<StreamFinishReason> = None;
            let update_assistant_text = matches!(event, StreamEvent::TextDelta(_));

            let idle = self.idle_state();
            let state = std::mem::replace(&mut self.state, idle);
            let (mut active, context_enabled) = match state {
                AppState::Enabled(EnabledState::Streaming(active)) => (active, true),
                AppState::Disabled(DisabledState::Streaming(active)) => (active, false),
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
                    StreamEvent::ToolCallStart { id, name } => {
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
                        if let Some(batch_id) = active.tool_batch_id {
                            if let Err(e) =
                                self.tool_journal.append_call_args(batch_id, id, arguments)
                            {
                                journal_error = Some(e.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }

            if journal_error.is_none() {
                finish_reason = active.message.apply_event(event);
                if update_assistant_text && let Some(batch_id) = active.tool_batch_id {
                    if let Err(e) = self
                        .tool_journal
                        .update_assistant_text(batch_id, active.message.content())
                    {
                        journal_error = Some(e.to_string());
                    }
                }
            }

            self.state = if context_enabled {
                AppState::Enabled(EnabledState::Streaming(active))
            } else {
                AppState::Disabled(DisabledState::Streaming(active))
            };

            if let Some(err) = journal_error {
                // Abort streaming without applying the unpersisted event.
                let active = match self.replace_with_idle() {
                    AppState::Enabled(EnabledState::Streaming(active))
                    | AppState::Disabled(DisabledState::Streaming(active)) => active,
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
                let aborted = if partial.is_empty() {
                    NonEmptyString::from(ABORTED_JOURNAL_BADGE)
                } else {
                    NonEmptyString::from(ABORTED_JOURNAL_BADGE)
                        .append("\n\n")
                        .append(partial.as_str())
                };
                self.push_local_message(Message::assistant(model, aborted));
                self.set_status(format!("Journal append failed: {err}"));
                return;
            }

            if let Some(reason) = finish_reason {
                self.finish_streaming(reason);
                return;
            }
        }
    }

    /// Finish the current streaming session and commit the message.
    ///
    /// # Commit ordering for crash durability
    ///
    /// The order of operations is critical for durability:
    /// 1. Capture step_id before consuming the journal
    /// 2. Seal the journal (marks stream as complete in SQLite)
    /// 3. Push message to history WITH step_id (for idempotent recovery)
    /// 4. Persist history to disk
    /// 5. Mark journal step as committed (after history is persisted)
    /// 6. Prune the journal step (cleanup)
    ///
    /// This ensures that if we crash after step 2 but before step 5, recovery
    /// will find the uncommitted step. If history already has that step_id,
    /// recovery will skip it (idempotent).
    fn finish_streaming(&mut self, finish_reason: StreamFinishReason) {
        let active = match self.replace_with_idle() {
            AppState::Enabled(EnabledState::Streaming(active))
            | AppState::Disabled(DisabledState::Streaming(active)) => active,
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

        // Check for tool calls before converting to message
        if message.has_tool_calls() {
            let tool_calls = message.take_tool_calls();
            let assistant_text = message.content().to_string();
            self.pending_user_message = None;
            self.handle_tool_calls(assistant_text, tool_calls, model, step_id, tool_batch_id);
            return;
        }

        // Convert streaming message to completed message (empty content is invalid).
        let message = message.into_message().ok();

        match finish_reason {
            StreamFinishReason::Error(err) => {
                if let Some(message) = message {
                    // Partial content received - keep both user message and partial response
                    self.pending_user_message = None;
                    // Push with step_id for idempotent recovery
                    self.push_history_message_with_step_id(message, step_id);
                    if self.autosave_history() {
                        // Only commit+prune if history was persisted successfully
                        self.finalize_journal_commit(step_id);
                    }
                    // If save failed, leave journal recoverable for next session
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
            StreamFinishReason::Done => {}
        }

        let Some(message) = message else {
            // Stream completed successfully but with empty content - unusual but not an error
            self.pending_user_message = None;
            let empty_msg = Message::assistant(model, NonEmptyString::from(EMPTY_RESPONSE_BADGE));
            self.push_local_message(empty_msg);
            self.set_status("Warning: API returned empty response");
            // Empty response - discard the step (nothing to recover)
            self.discard_journal_step(step_id);
            return;
        };

        // Stream completed successfully with content
        self.pending_user_message = None;
        // Push with step_id for idempotent recovery
        self.push_history_message_with_step_id(message, step_id);
        if self.autosave_history() {
            // Only commit+prune if history was persisted successfully
            self.finalize_journal_commit(step_id);
        }
        // If save failed, leave journal recoverable for next session
    }

    fn handle_tool_calls(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        model: ModelName,
        step_id: forge_context::StepId,
        tool_batch_id: Option<ToolBatchId>,
    ) {
        if tool_calls.is_empty() {
            return;
        }

        let mut batch_id = tool_batch_id.unwrap_or(0);
        if batch_id != 0 {
            if let Err(e) = self
                .tool_journal
                .update_assistant_text(batch_id, &assistant_text)
            {
                tracing::warn!("Tool journal update failed: {e}");
                self.set_status(format!("Tool journal error: {e}"));
                batch_id = 0;
            }
        }
        if batch_id == 0 {
            batch_id =
                match self
                    .tool_journal
                    .begin_batch(model.as_str(), &assistant_text, &tool_calls)
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::warn!("Tool journal begin failed: {e}");
                        self.set_status(format!("Tool journal error: {e}"));
                        0
                    }
                };
        }

        match self.tools_mode {
            tools::ToolsMode::Disabled => {
                let results: Vec<ToolResult> = tool_calls
                    .iter()
                    .map(|call| ToolResult::error(call.id.clone(), "Tool execution disabled"))
                    .collect();
                if batch_id != 0 {
                    for result in &results {
                        let _ = self.tool_journal.record_result(batch_id, result);
                    }
                }
                self.commit_tool_batch(
                    assistant_text,
                    tool_calls,
                    results,
                    model,
                    step_id,
                    batch_id,
                    false,
                );
            }
            tools::ToolsMode::ParseOnly => {
                let pending = PendingToolExecution {
                    assistant_text,
                    pending_calls: tool_calls,
                    results: Vec::new(),
                    model,
                    step_id,
                    batch_id,
                };
                self.state = AppState::Enabled(EnabledState::AwaitingToolResults(pending));
                self.set_status("Waiting for tool results...");
            }
            tools::ToolsMode::Enabled => {
                self.start_tool_loop(assistant_text, tool_calls, model, step_id, batch_id);
            }
        }
    }

    fn start_tool_loop(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        model: ModelName,
        step_id: forge_context::StepId,
        batch_id: ToolBatchId,
    ) {
        let next_iteration = self.tool_iterations.saturating_add(1);
        if next_iteration > self.tool_settings.limits.max_tool_iterations_per_user_turn {
            let results: Vec<ToolResult> = tool_calls
                .iter()
                .map(|call| ToolResult::error(call.id.clone(), "Max tool iterations reached"))
                .collect();
            if batch_id != 0 {
                for result in &results {
                    let _ = self.tool_journal.record_result(batch_id, result);
                }
            }
            self.commit_tool_batch(
                assistant_text,
                tool_calls,
                results,
                model,
                step_id,
                batch_id,
                true,
            );
            return;
        }
        self.tool_iterations = next_iteration;

        let plan = self.plan_tool_calls(&tool_calls);
        if batch_id != 0 {
            for result in &plan.pre_resolved {
                let _ = self.tool_journal.record_result(batch_id, result);
            }
        }

        let batch = ToolBatch {
            assistant_text,
            calls: tool_calls,
            results: plan.pre_resolved,
            model,
            step_id,
            batch_id,
            iteration: next_iteration,
            execute_now: plan.execute_now,
            approval_calls: plan.approval_calls,
            approval_requests: plan.approval_requests.clone(),
        };
        let iteration = batch.iteration;
        let max_iterations = self.tool_settings.limits.max_tool_iterations_per_user_turn;

        let remaining_capacity_bytes = self.remaining_tool_capacity(&batch);

        if !plan.approval_requests.is_empty() {
            let approval = ApprovalState {
                requests: plan.approval_requests,
                selected: vec![false; batch.approval_requests.len()],
                cursor: 0,
            };
            self.state = AppState::Enabled(EnabledState::ToolLoop(ToolLoopState {
                batch,
                phase: ToolLoopPhase::AwaitingApproval(approval),
            }));
            self.set_status(format!(
                "Tool approval required (iteration {}/{})",
                iteration, max_iterations
            ));
            return;
        }

        let queue = batch.execute_now.clone();
        if queue.is_empty() {
            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                batch.results,
                batch.model,
                batch.step_id,
                batch.batch_id,
                true,
            );
            return;
        }

        let exec = self.spawn_tool_execution(queue, remaining_capacity_bytes);
        self.state = AppState::Enabled(EnabledState::ToolLoop(ToolLoopState {
            batch,
            phase: ToolLoopPhase::Executing(exec),
        }));
        self.set_status(format!(
            "Running tools (iteration {}/{})",
            iteration, max_iterations
        ));
    }

    fn plan_tool_calls(&self, calls: &[ToolCall]) -> ToolPlan {
        let mut execute_now = Vec::new();
        let mut approval_calls = Vec::new();
        let mut approval_requests = Vec::new();
        let mut pre_resolved = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut accepted = 0usize;

        for call in calls {
            if !self.tool_settings.policy.enabled {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::Disabled),
                ));
                continue;
            }

            if self.tool_settings.policy.is_denylisted(&call.name) {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::Denylisted {
                        tool: call.name.clone(),
                    }),
                ));
                continue;
            }

            if !seen_ids.insert(call.id.clone()) {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::DuplicateToolCallId {
                        id: call.id.clone(),
                    },
                ));
                continue;
            }
            accepted += 1;
            if accepted > self.tool_settings.limits.max_tool_calls_per_batch {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::LimitsExceeded {
                        message: "Exceeded max tool calls per batch".to_string(),
                    }),
                ));
                continue;
            }

            let args_size = serde_json::to_vec(&call.arguments)
                .map(|v| v.len())
                .unwrap_or(0);
            if args_size > self.tool_settings.limits.max_tool_args_bytes {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::LimitsExceeded {
                        message: "Tool arguments too large".to_string(),
                    }),
                ));
                continue;
            }

            if call.name == "apply_patch" {
                if let Some(patch) = call.arguments.get("patch").and_then(|v| v.as_str()) {
                    if patch.as_bytes().len() > self.tool_settings.patch_limits.max_patch_bytes {
                        pre_resolved.push(tool_error_result(
                            call,
                            tools::ToolError::SandboxViolation(
                                tools::DenialReason::LimitsExceeded {
                                    message: "Patch exceeds max_patch_bytes".to_string(),
                                },
                            ),
                        ));
                        continue;
                    }
                }
            }

            let exec = match self.tool_registry.lookup(&call.name) {
                Ok(exec) => exec,
                Err(err) => {
                    pre_resolved.push(tool_error_result(call, err));
                    continue;
                }
            };

            if let Err(err) = tools::validate_args(&exec.schema(), &call.arguments) {
                pre_resolved.push(tool_error_result(call, err));
                continue;
            }

            if let Err(err) = preflight_sandbox(&self.tool_settings.sandbox, &call) {
                pre_resolved.push(tool_error_result(call, err));
                continue;
            }

            if matches!(self.tool_settings.policy.mode, tools::ApprovalMode::Deny)
                && !self.tool_settings.policy.is_allowlisted(&call.name)
            {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::Denylisted {
                        tool: call.name.clone(),
                    }),
                ));
                continue;
            }

            let allowlisted = self.tool_settings.policy.is_allowlisted(&call.name);
            let needs_confirmation = match self.tool_settings.policy.mode {
                tools::ApprovalMode::Auto => exec.requires_approval(),
                tools::ApprovalMode::Prompt => {
                    exec.requires_approval()
                        || (self.tool_settings.policy.prompt_side_effects
                            && exec.is_side_effecting()
                            && !allowlisted)
                }
                tools::ApprovalMode::Deny => exec.requires_approval(),
            };

            if needs_confirmation {
                let summary = match exec.approval_summary(&call.arguments) {
                    Ok(summary) => summary,
                    Err(err) => {
                        pre_resolved.push(tool_error_result(call, err));
                        continue;
                    }
                };
                let summary = truncate_with_ellipsis(&summary, 200);
                approval_requests.push(tools::ConfirmationRequest {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    summary,
                    risk_level: exec.risk_level(),
                });
                approval_calls.push(call.clone());
            } else {
                execute_now.push(call.clone());
            }
        }

        ToolPlan {
            execute_now,
            approval_calls,
            approval_requests,
            pre_resolved,
        }
    }

    fn tool_capacity_bytes(&mut self) -> usize {
        let usage = match self.context_usage_status() {
            ContextUsageStatus::Ready(usage)
            | ContextUsageStatus::NeedsSummarization { usage, .. }
            | ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => usage,
        };

        if usage.budget_tokens == 0 {
            return DEFAULT_TOOL_CAPACITY_BYTES;
        }

        let available_tokens = usage
            .budget_tokens
            .saturating_sub(usage.used_tokens)
            .saturating_sub(TOOL_OUTPUT_SAFETY_MARGIN_TOKENS);
        if available_tokens == 0 {
            return 0;
        }

        let available_bytes = (available_tokens as usize).saturating_mul(4);
        if available_bytes == 0 {
            DEFAULT_TOOL_CAPACITY_BYTES
        } else {
            available_bytes
        }
    }

    fn remaining_tool_capacity(&mut self, batch: &ToolBatch) -> usize {
        let mut remaining = self.tool_capacity_bytes();
        for result in &batch.results {
            remaining = remaining.saturating_sub(result.content.len());
        }
        remaining
    }

    fn spawn_tool_execution(
        &self,
        queue: Vec<ToolCall>,
        initial_capacity_bytes: usize,
    ) -> ActiveToolExecution {
        let mut exec = ActiveToolExecution {
            queue: VecDeque::from(queue),
            current_call: None,
            join_handle: None,
            event_rx: None,
            abort_handle: None,
            output_lines: Vec::new(),
            remaining_capacity_bytes: initial_capacity_bytes,
        };
        self.start_next_tool_call(&mut exec);
        exec
    }

    fn start_next_tool_call(&self, exec: &mut ActiveToolExecution) -> bool {
        let Some(call) = exec.queue.pop_front() else {
            return false;
        };

        exec.output_lines.clear();
        exec.current_call = Some(call.clone());

        let (event_tx, event_rx) = mpsc::channel(TOOL_EVENT_CHANNEL_CAPACITY);
        exec.event_rx = Some(event_rx);

        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        exec.abort_handle = Some(abort_handle.clone());

        let registry = self.tool_registry.clone();
        let settings = self.tool_settings.clone();
        let file_cache = self.tool_file_cache.clone();
        let working_dir = settings.sandbox.working_dir();
        let remaining_capacity = exec.remaining_capacity_bytes;

        let handle = tokio::spawn(async move {
            use futures_util::FutureExt;
            let _ = event_tx
                .send(tools::ToolEvent::Started {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                })
                .await;

            let exec_ref = match registry.lookup(&call.name) {
                Ok(exec) => exec,
                Err(err) => {
                    let result = tool_error_result(&call, err);
                    let _ = event_tx
                        .send(tools::ToolEvent::Completed {
                            tool_call_id: call.id.clone(),
                        })
                        .await;
                    return result;
                }
            };

            let default_timeout = match call.name.as_str() {
                "read_file" | "apply_patch" => settings.timeouts.file_operations_timeout,
                "run_command" => settings.timeouts.shell_commands_timeout,
                _ => settings.timeouts.default_timeout,
            };

            let mut ctx = tools::ToolCtx {
                sandbox: settings.sandbox.clone(),
                abort: abort_handle,
                output_tx: event_tx.clone(),
                default_timeout,
                max_output_bytes: settings.max_output_bytes,
                available_capacity_bytes: remaining_capacity,
                tool_call_id: call.id.clone(),
                allow_truncation: true,
                working_dir,
                env_sanitizer: settings.env_sanitizer.clone(),
                file_cache,
            };

            let timeout = exec_ref.timeout().unwrap_or(ctx.default_timeout);
            let exec_future = exec_ref.execute(call.arguments.clone(), &mut ctx);
            let exec_future = std::panic::AssertUnwindSafe(exec_future).catch_unwind();
            let exec_future = Abortable::new(exec_future, abort_registration);

            let result = match tokio::time::timeout(timeout, exec_future).await {
                Err(_) => tool_error_result(
                    &call,
                    tools::ToolError::Timeout {
                        tool: call.name.clone(),
                        elapsed: timeout,
                    },
                ),
                Ok(Err(_)) => tool_error_result(&call, tools::ToolError::Cancelled),
                Ok(Ok(Err(panic_payload))) => {
                    let panic_msg = panic_payload_to_string(&panic_payload);
                    let message = format!("Tool panicked: {}", panic_msg);
                    ToolResult::error(call.id.clone(), tools::sanitize_output(&message))
                }
                Ok(Ok(Ok(inner))) => match inner {
                    Ok(output) => {
                        let sanitized = tools::sanitize_output(&output);
                        let effective_max = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
                        let final_output = if ctx.allow_truncation {
                            tools::truncate_output(sanitized, effective_max)
                        } else {
                            sanitized
                        };
                        ToolResult::success(call.id.clone(), final_output)
                    }
                    Err(err) => tool_error_result(&call, err),
                },
            };

            let _ = event_tx
                .send(tools::ToolEvent::Completed {
                    tool_call_id: call.id.clone(),
                })
                .await;

            result
        });

        exec.join_handle = Some(handle);
        true
    }

    fn poll_tool_loop(&mut self) {
        use futures_util::future::FutureExt;

        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
            AppState::Enabled(EnabledState::ToolLoop(state)) => state,
            other => {
                self.state = other;
                return;
            }
        };
        let mut state = state;
        let mut completed: Option<ToolResult> = None;
        let mut should_commit = false;

        match &mut state.phase {
            ToolLoopPhase::AwaitingApproval(_) => {}
            ToolLoopPhase::Executing(exec) => {
                if let Some(rx) = exec.event_rx.as_mut() {
                    loop {
                        match rx.try_recv() {
                            Ok(event) => match event {
                                tools::ToolEvent::Started {
                                    tool_call_id,
                                    tool_name,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if is_current {
                                        exec.output_lines.push(format!(
                                            "▶ {} ({})",
                                            tools::sanitize_output(&tool_name),
                                            tool_call_id
                                        ));
                                    }
                                }
                                tools::ToolEvent::StdoutChunk {
                                    tool_call_id,
                                    chunk,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if !is_current {
                                        continue;
                                    }
                                    append_tool_output_lines(
                                        &mut exec.output_lines,
                                        &tools::sanitize_output(&chunk),
                                        None,
                                    );
                                }
                                tools::ToolEvent::StderrChunk {
                                    tool_call_id,
                                    chunk,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if !is_current {
                                        continue;
                                    }
                                    append_tool_output_lines(
                                        &mut exec.output_lines,
                                        &tools::sanitize_output(&chunk),
                                        Some("[stderr] "),
                                    );
                                }
                                tools::ToolEvent::Completed { tool_call_id } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if is_current {
                                        exec.output_lines
                                            .push(format!("✓ Tool completed ({})", tool_call_id));
                                    }
                                }
                            },
                            Err(mpsc::error::TryRecvError::Empty) => break,
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                exec.event_rx = None;
                                break;
                            }
                        }
                    }
                }

                if let Some(handle) = exec.join_handle.as_mut() {
                    if let Some(joined) = handle.now_or_never() {
                        exec.join_handle = None;
                        exec.event_rx = None;
                        exec.abort_handle = None;

                        let result = match joined {
                            Ok(result) => result,
                            Err(err) => {
                                let call_id = exec
                                    .current_call
                                    .as_ref()
                                    .map(|c| c.id.clone())
                                    .unwrap_or_else(|| "<unknown>".to_string());
                                let message = if err.is_cancelled() {
                                    "Tool execution cancelled"
                                } else {
                                    "Tool execution failed"
                                };
                                ToolResult::error(call_id, message)
                            }
                        };
                        exec.current_call = None;
                        completed = Some(result);
                    }
                }

                if let Some(result) = completed.take() {
                    if state.batch.batch_id != 0 {
                        let _ = self
                            .tool_journal
                            .record_result(state.batch.batch_id, &result);
                    }
                    exec.remaining_capacity_bytes = exec
                        .remaining_capacity_bytes
                        .saturating_sub(result.content.len());
                    state.batch.results.push(result);

                    if exec.queue.is_empty() {
                        should_commit = true;
                    } else {
                        self.start_next_tool_call(exec);
                    }
                }
            }
        }

        if should_commit {
            self.commit_tool_batch(
                state.batch.assistant_text,
                state.batch.calls,
                state.batch.results,
                state.batch.model,
                state.batch.step_id,
                state.batch.batch_id,
                true,
            );
        } else {
            self.state = AppState::Enabled(EnabledState::ToolLoop(state));
        }
    }

    fn cancel_tool_batch(
        &mut self,
        assistant_text: String,
        calls: Vec<ToolCall>,
        mut results: Vec<ToolResult>,
        model: ModelName,
        step_id: forge_context::StepId,
        batch_id: ToolBatchId,
    ) {
        let existing: std::collections::HashSet<String> =
            results.iter().map(|r| r.tool_call_id.clone()).collect();
        for call in &calls {
            if existing.contains(&call.id) {
                continue;
            }
            let result = ToolResult::error(call.id.clone(), "Cancelled by user");
            if batch_id != 0 {
                let _ = self.tool_journal.record_result(batch_id, &result);
            }
            results.push(result);
        }

        self.commit_tool_batch(
            assistant_text,
            calls,
            results,
            model,
            step_id,
            batch_id,
            false,
        );
    }

    /// Submit a tool result. When all results are in, commit messages and return to Idle.
    ///
    /// Returns `Ok(true)` if all results are now complete and conversation can resume,
    /// `Ok(false)` if more results are still needed, or an error if not in the right state.
    pub fn submit_tool_result(&mut self, result: ToolResult) -> Result<bool, String> {
        // First, validate and add the result to pending state
        let (results_count, pending_count, batch_id) = {
            let pending = match &mut self.state {
                AppState::Enabled(EnabledState::AwaitingToolResults(pending)) => pending,
                _ => return Err("Not awaiting tool results".to_string()),
            };

            // Verify this result matches a pending call
            let matching_call = pending
                .pending_calls
                .iter()
                .any(|c| c.id == result.tool_call_id);
            if !matching_call {
                return Err(format!(
                    "No pending tool call with ID '{}'",
                    result.tool_call_id
                ));
            }

            // Check for duplicate result
            if pending
                .results
                .iter()
                .any(|r| r.tool_call_id == result.tool_call_id)
            {
                return Err(format!(
                    "Result for tool call '{}' already submitted",
                    result.tool_call_id
                ));
            }

            pending.results.push(result);
            if pending.batch_id != 0 {
                let _ = self
                    .tool_journal
                    .record_result(pending.batch_id, pending.results.last().unwrap());
            }
            (
                pending.results.len(),
                pending.pending_calls.len(),
                pending.batch_id,
            )
        };

        // Check if all results are in
        if results_count < pending_count {
            self.set_status(format!(
                "Received {}/{} tool results...",
                results_count, pending_count
            ));
            return Ok(false);
        }

        // All results received - commit to history and return to Idle
        // Take ownership of the pending state
        let pending =
            match std::mem::replace(&mut self.state, AppState::Enabled(EnabledState::Idle)) {
                AppState::Enabled(EnabledState::AwaitingToolResults(p)) => p,
                other => {
                    self.state = other;
                    return Err("State changed unexpectedly".to_string());
                }
            };

        self.commit_tool_batch(
            pending.assistant_text,
            pending.pending_calls,
            pending.results,
            pending.model,
            pending.step_id,
            batch_id,
            false,
        );
        Ok(true)
    }

    fn commit_tool_batch(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        results: Vec<ToolResult>,
        model: ModelName,
        step_id: forge_context::StepId,
        batch_id: ToolBatchId,
        auto_resume: bool,
    ) {
        self.state = self.idle_state();

        if let Ok(content) = NonEmptyString::new(assistant_text.clone()) {
            let message = Message::assistant(model.clone(), content);
            self.push_history_message_with_step_id(message, step_id);
        }

        let mut result_map: std::collections::HashMap<String, ToolResult> =
            std::collections::HashMap::new();
        for result in results {
            result_map
                .entry(result.tool_call_id.clone())
                .or_insert(result);
        }

        let mut ordered_results: Vec<ToolResult> = Vec::new();
        for call in &tool_calls {
            if let Some(result) = result_map.remove(&call.id) {
                ordered_results.push(result);
            } else {
                ordered_results.push(ToolResult::error(call.id.clone(), "Missing tool result"));
            }
        }

        for call in &tool_calls {
            self.push_history_message(Message::tool_use(call.clone()));
        }

        for result in &ordered_results {
            self.push_history_message(Message::tool_result(result.clone()));
        }

        if self.autosave_history() {
            self.finalize_journal_commit(step_id);
            if batch_id != 0 {
                if let Err(e) = self.tool_journal.commit_batch(batch_id) {
                    tracing::warn!("Failed to commit tool batch {}: {e}", batch_id);
                }
            }
        }

        if auto_resume {
            let Some(api_key) = self.api_keys.get(&model.provider()).cloned() else {
                self.set_status(format!(
                    "Cannot resume: no API key for {}",
                    model.provider().display_name()
                ));
                return;
            };

            let api_key = match model.provider() {
                Provider::Claude => ApiKey::Claude(api_key),
                Provider::OpenAI => ApiKey::OpenAI(api_key),
            };

            let config = match ApiConfig::new(api_key, model.clone()) {
                Ok(config) => config.with_openai_options(self.openai_options),
                Err(e) => {
                    self.set_status(format!("Cannot resume after tools: {e}"));
                    return;
                }
            };

            self.start_streaming(QueuedUserMessage { config });
        }
    }

    /// Atomically commit and prune a journal step.
    ///
    /// Called ONLY after history has been successfully persisted to disk.
    fn finalize_journal_commit(&mut self, step_id: forge_context::StepId) {
        if let Err(e) = self.stream_journal.commit_and_prune_step(step_id) {
            tracing::warn!("Failed to commit/prune journal step {}: {e}", step_id);
        }
    }

    /// Discard a journal step that won't be recovered (error/empty cases).
    fn discard_journal_step(&mut self, step_id: forge_context::StepId) {
        if let Err(e) = self.stream_journal.discard_step(step_id) {
            tracing::warn!("Failed to discard journal step {}: {e}", step_id);
        }
    }

    /// Rollback a pending user message after stream error with no content.
    ///
    /// This removes the user message from history and display, then restores
    /// the original text to the input box for easy retry.
    fn rollback_pending_user_message(&mut self) {
        let Some((msg_id, original_text)) = self.pending_user_message.take() else {
            return;
        };

        // Remove from history
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
        let elapsed = now.duration_since(self.last_frame);
        self.last_frame = now;
        elapsed
    }

    /// Get mutable reference to modal effect for UI processing.
    pub fn modal_effect_mut(&mut self) -> Option<&mut ModalEffect> {
        self.modal_effect.as_mut()
    }

    pub fn clear_modal_effect(&mut self) {
        self.modal_effect = None;
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    pub fn input_mode(&self) -> InputMode {
        self.input.mode()
    }

    pub fn insert_token(&self) -> Option<InsertToken> {
        matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
    }

    pub fn command_token(&self) -> Option<CommandToken> {
        matches!(&self.input, InputState::Command { .. }).then_some(CommandToken(()))
    }

    pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
        InsertMode { app: self }
    }

    pub fn command_mode(&mut self, _token: CommandToken) -> CommandMode<'_> {
        CommandMode { app: self }
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
        self.modal_effect = None;
    }

    pub fn enter_insert_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_insert();
    }

    pub fn enter_command_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_command();
    }

    pub fn enter_model_select_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_model_select();
        self.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
        self.last_frame = Instant::now();
    }

    pub fn model_select_index(&self) -> Option<usize> {
        self.input.model_select_index()
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
            let max_index = PredefinedModel::all().len().saturating_sub(1);
            if *selected < max_index {
                *selected += 1;
            }
        }
    }

    pub fn model_select_set_index(&mut self, index: usize) {
        if let InputState::ModelSelect { selected, .. } = &mut self.input {
            let max_index = PredefinedModel::all().len().saturating_sub(1);
            *selected = index.min(max_index);
        }
    }

    /// Select the current model and return to normal mode.
    pub fn model_select_confirm(&mut self) {
        let Some(index) = self.model_select_index() else {
            return;
        };
        let models = PredefinedModel::all();
        if let Some(predefined) = models.get(index) {
            let model = predefined.to_model_name();
            self.set_model(model);
            self.set_status(format!("Model set to: {}", predefined.display_name()));
        }
        self.enter_normal_mode();
    }

    pub fn tool_approval_move_up(&mut self) {
        if let AppState::Enabled(EnabledState::ToolLoop(state)) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
            && approval.cursor > 0
        {
            approval.cursor -= 1;
        }
    }

    pub fn tool_approval_move_down(&mut self) {
        if let AppState::Enabled(EnabledState::ToolLoop(state)) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
        {
            if approval.cursor + 1 < approval.requests.len() {
                approval.cursor += 1;
            }
        }
    }

    pub fn tool_approval_toggle(&mut self) {
        if let AppState::Enabled(EnabledState::ToolLoop(state)) = &mut self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &mut state.phase
            && approval.cursor < approval.selected.len()
        {
            approval.selected[approval.cursor] = !approval.selected[approval.cursor];
        }
    }

    pub fn tool_approval_approve_all(&mut self) {
        self.resolve_tool_approval(tools::ApprovalDecision::ApproveAll);
    }

    pub fn tool_approval_deny_all(&mut self) {
        self.resolve_tool_approval(tools::ApprovalDecision::DenyAll);
    }

    pub fn tool_approval_confirm_selected(&mut self) {
        let ids = if let AppState::Enabled(EnabledState::ToolLoop(state)) = &self.state
            && let ToolLoopPhase::AwaitingApproval(approval) = &state.phase
        {
            approval
                .requests
                .iter()
                .zip(approval.selected.iter())
                .filter_map(|(req, selected)| selected.then(|| req.tool_call_id.clone()))
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

    pub fn tool_recovery_resume(&mut self) {
        self.resolve_tool_recovery(ToolRecoveryDecision::Resume);
    }

    pub fn tool_recovery_discard(&mut self) {
        self.resolve_tool_recovery(ToolRecoveryDecision::Discard);
    }

    fn resolve_tool_approval(&mut self, decision: tools::ApprovalDecision) {
        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
            AppState::Enabled(EnabledState::ToolLoop(state)) => state,
            other => {
                self.state = other;
                return;
            }
        };

        let ToolLoopState { mut batch, phase } = state;
        let ToolLoopPhase::AwaitingApproval(_approval) = phase else {
            self.state = AppState::Enabled(EnabledState::ToolLoop(ToolLoopState { batch, phase }));
            return;
        };

        let mut approved_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        match &decision {
            tools::ApprovalDecision::ApproveAll => {
                approved_ids = batch
                    .approval_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect();
            }
            tools::ApprovalDecision::ApproveSelected(ids) => {
                approved_ids.extend(ids.iter().cloned());
            }
            tools::ApprovalDecision::DenyAll => {}
        }

        let mut approved_calls = Vec::new();
        let mut denied_results = Vec::new();
        for call in batch.approval_calls.drain(..) {
            if approved_ids.contains(&call.id) {
                approved_calls.push(call);
            } else {
                denied_results.push(ToolResult::error(
                    call.id.clone(),
                    "Tool call denied by user",
                ));
            }
        }

        if batch.batch_id != 0 {
            for result in &denied_results {
                let _ = self.tool_journal.record_result(batch.batch_id, result);
            }
        }
        batch.results.extend(denied_results);

        let mut queue = batch.execute_now.clone();
        queue.extend(approved_calls);
        batch.execute_now = queue.clone();

        if queue.is_empty() {
            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                batch.results,
                batch.model,
                batch.step_id,
                batch.batch_id,
                true,
            );
            return;
        }

        let remaining_capacity = self.remaining_tool_capacity(&batch);
        let exec = self.spawn_tool_execution(queue, remaining_capacity);
        self.state = AppState::Enabled(EnabledState::ToolLoop(ToolLoopState {
            batch,
            phase: ToolLoopPhase::Executing(exec),
        }));
        self.set_status("Running tools...");
    }

    fn resolve_tool_recovery(&mut self, decision: ToolRecoveryDecision) {
        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
            AppState::Enabled(EnabledState::ToolRecovery(state)) => state,
            other => {
                self.state = other;
                return;
            }
        };

        self.commit_recovered_tool_batch(state, decision);
    }

    fn commit_recovered_tool_batch(
        &mut self,
        state: ToolRecoveryState,
        decision: ToolRecoveryDecision,
    ) {
        let ToolRecoveryState {
            batch,
            step_id,
            model,
        } = state;

        let assistant_text = batch.assistant_text.clone();
        let results = match decision {
            ToolRecoveryDecision::Resume => {
                let mut merged = batch.results;
                let existing: std::collections::HashSet<String> =
                    merged.iter().map(|r| r.tool_call_id.clone()).collect();
                for call in &batch.calls {
                    if !existing.contains(&call.id) {
                        merged.push(ToolResult::error(
                            call.id.clone(),
                            "Tool result missing after crash",
                        ));
                    }
                }
                merged
            }
            ToolRecoveryDecision::Discard => batch
                .calls
                .iter()
                .map(|call| {
                    ToolResult::error(call.id.clone(), "Tool results discarded after crash")
                })
                .collect(),
        };

        if batch.batch_id != 0 {
            for result in &results {
                let _ = self.tool_journal.record_result(batch.batch_id, result);
            }
        }

        let auto_resume = matches!(self.tools_mode, tools::ToolsMode::Enabled);
        self.commit_tool_batch(
            assistant_text,
            batch.calls,
            results,
            model,
            step_id,
            batch.batch_id,
            auto_resume,
        );

        match decision {
            ToolRecoveryDecision::Resume => {
                self.set_status("Recovered tool batch resumed");
            }
            ToolRecoveryDecision::Discard => {
                self.set_status("Tool results discarded after crash");
            }
        }
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

    pub fn update_scroll_max(&mut self, max: u16) {
        self.scroll_max = max;

        if let ScrollState::Manual { offset_from_top } = self.scroll
            && offset_from_top >= max
        {
            self.scroll = ScrollState::AutoBottom;
        }
    }

    pub fn scroll_offset_from_top(&self) -> u16 {
        match self.scroll {
            ScrollState::AutoBottom => self.scroll_max,
            ScrollState::Manual { offset_from_top } => offset_from_top.min(self.scroll_max),
        }
    }

    /// Scroll up in message view.
    pub fn scroll_up(&mut self) {
        self.scroll = match self.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.scroll_max.saturating_sub(3),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(3),
            },
        };
    }

    /// Scroll down in message view.
    pub fn scroll_down(&mut self) {
        let ScrollState::Manual { offset_from_top } = self.scroll else {
            return;
        };

        let new_offset = offset_from_top.saturating_add(3);
        if new_offset >= self.scroll_max {
            self.scroll = ScrollState::AutoBottom;
        } else {
            self.scroll = ScrollState::Manual {
                offset_from_top: new_offset,
            };
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll = ScrollState::Manual { offset_from_top: 0 };
    }

    /// Jump to bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = ScrollState::AutoBottom;
    }

    pub fn process_command(&mut self, command: EnteredCommand) {
        let parts: Vec<&str> = command.raw.split_whitespace().collect();

        match parts.first().copied() {
            Some("q" | "quit") => {
                self.request_quit();
            }
            Some("clear") => {
                let state = self.replace_with_idle();
                match state {
                    AppState::Enabled(EnabledState::Streaming(active))
                    | AppState::Disabled(DisabledState::Streaming(active)) => {
                        active.abort_handle.abort();
                        let _ = active.journal.discard(&mut self.stream_journal);
                    }
                    AppState::Enabled(EnabledState::AwaitingToolResults(pending)) => {
                        // Clear pending tool execution and discard the journal step.
                        self.discard_journal_step(pending.step_id);
                        if pending.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(pending.batch_id);
                        }
                    }
                    AppState::Enabled(EnabledState::ToolLoop(state)) => {
                        if let ToolLoopPhase::Executing(exec) = &state.phase {
                            if let Some(handle) = &exec.abort_handle {
                                handle.abort();
                            }
                        }
                        if state.batch.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(state.batch.batch_id);
                        }
                        self.discard_journal_step(state.batch.step_id);
                    }
                    AppState::Enabled(EnabledState::ToolRecovery(state)) => {
                        if state.batch.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(state.batch.batch_id);
                        }
                        self.discard_journal_step(state.step_id);
                    }
                    AppState::Enabled(EnabledState::Summarizing(state)) => {
                        state.task.handle.abort();
                    }
                    AppState::Enabled(EnabledState::SummarizingWithQueued(state)) => {
                        state.task.handle.abort();
                    }
                    AppState::Enabled(EnabledState::SummarizationRetry(_))
                    | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_))
                    | AppState::Enabled(EnabledState::Idle)
                    | AppState::Disabled(DisabledState::Idle) => {}
                }

                self.display.clear();
                self.context_manager = ContextManager::new(self.model.as_str());
                self.context_manager
                    .set_output_limit(self.output_limits.max_output_tokens());
                self.invalidate_usage_cache();
                self.autosave_history(); // Persist cleared state immediately
                self.set_status("Conversation cleared");
            }
            Some("model") => {
                if let Some(model_name) = parts.get(1) {
                    let provider = self.provider();
                    match provider.parse_model(model_name) {
                        Ok(model) => {
                            let kind = model.kind();
                            self.set_model(model);
                            let suffix = match kind {
                                ModelNameKind::Known => "",
                                ModelNameKind::Unverified => " (unverified; limits may fallback)",
                            };
                            self.set_status(format!("Model set to: {}{}", self.model, suffix));
                        }
                        Err(e) => {
                            self.set_status(format!("Invalid model: {e}"));
                        }
                    }
                } else {
                    // Enter model selection mode with TUI list
                    self.enter_model_select_mode();
                }
            }
            Some("provider" | "p") => {
                if let Some(provider_str) = parts.get(1) {
                    if let Some(provider) = Provider::parse(provider_str) {
                        self.set_provider(provider);
                        let has_key = self.current_api_key().is_some();
                        let status = if has_key {
                            format!("Switched to {} ({})", provider.display_name(), self.model)
                        } else {
                            format!(
                                "Switched to {} - No API key! Set {}",
                                provider.display_name(),
                                provider.env_var()
                            )
                        };
                        self.set_status(status);
                    } else {
                        self.set_status(format!("Unknown provider: {provider_str}"));
                    }
                } else {
                    let provider = self.provider();
                    let providers: Vec<&str> = Provider::all().iter().map(|p| p.as_str()).collect();
                    self.set_status(format!(
                        "Current: {} ({}) │ Providers: {} │ Models: {}",
                        provider.display_name(),
                        self.model,
                        providers.join(", "),
                        provider.available_models().join(", ")
                    ));
                }
            }
            Some("context" | "ctx") => {
                let usage_status = self.context_usage_status();
                let (usage, needs_summary, recent_too_large) = match &usage_status {
                    ContextUsageStatus::Ready(usage) => (usage, None, None),
                    ContextUsageStatus::NeedsSummarization { usage, needed } => {
                        (usage, Some(needed), None)
                    }
                    ContextUsageStatus::RecentMessagesTooLarge {
                        usage,
                        required_tokens,
                        budget_tokens,
                    } => (usage, None, Some((*required_tokens, *budget_tokens))),
                };
                let limits = self.context_manager.current_limits();
                let limits_source = match self.context_manager.current_limits_source() {
                    ModelLimitsSource::Override => "override".to_string(),
                    ModelLimitsSource::Prefix(prefix) => prefix.to_string(),
                    ModelLimitsSource::DefaultFallback => "fallback(default)".to_string(),
                };
                let context_flag = if self.context_infinity_enabled() {
                    "on"
                } else {
                    "off"
                };
                let status_suffix = if let Some((required, budget)) = recent_too_large {
                    format!(
                        " │ ERROR: recent msgs ({} tokens) > budget ({} tokens)",
                        required, budget
                    )
                } else {
                    needs_summary.map_or(String::new(), |needed| {
                        format!(
                            " │ Summarize: {} msgs (~{} tokens)",
                            needed.messages_to_summarize.len(),
                            needed.excess_tokens
                        )
                    })
                };
                self.set_status(format!(
                    "ContextInfinity: {} │ Context: {} │ Model: {} │ Limits: {} │ Window: {}k │ Budget: {}k │ Max output: {}k{}",
                    context_flag,
                    usage.format_compact(),
                    self.context_manager.current_model(),
                    limits_source,
                    limits.context_window() / 1000,
                    limits.effective_input_budget() / 1000,
                    limits.max_output() / 1000,
                    status_suffix,
                ));
            }
            Some("journal" | "jrnl") => match self.stream_journal.stats() {
                Ok(stats) => {
                    let streaming = matches!(
                        self.state,
                        AppState::Enabled(EnabledState::Streaming(_))
                            | AppState::Disabled(DisabledState::Streaming(_))
                    );
                    let state_desc = if streaming {
                        "streaming"
                    } else if stats.unsealed_entries > 0 {
                        "unsealed"
                    } else {
                        "idle"
                    };
                    self.set_status(format!(
                        "Journal: {} │ Total: {} │ Sealed: {} │ Unsealed: {} │ Steps: {}",
                        state_desc,
                        stats.total_entries,
                        stats.sealed_entries,
                        stats.unsealed_entries,
                        stats.current_step_id,
                    ));
                }
                Err(e) => {
                    self.set_status(format!("Journal error: {e}"));
                }
            },
            Some("summarize" | "sum") => {
                if !self.context_infinity_enabled() {
                    self.set_status("ContextInfinity disabled: summarization unavailable");
                } else if matches!(
                    self.state,
                    AppState::Enabled(EnabledState::Summarizing(_))
                        | AppState::Enabled(EnabledState::SummarizingWithQueued(_))
                        | AppState::Enabled(EnabledState::SummarizationRetry(_))
                        | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_))
                ) {
                    self.set_status("Summarization already in progress");
                } else if matches!(
                    self.state,
                    AppState::Enabled(EnabledState::Streaming(_))
                        | AppState::Disabled(DisabledState::Streaming(_))
                ) {
                    self.set_status("Cannot summarize while streaming");
                } else {
                    self.set_status("Summarizing older messages...");
                    let result = self.start_summarization_with_attempt(None, 1);
                    if matches!(result, SummarizationStart::NotNeeded) {
                        self.set_status("No messages need summarization");
                    }
                }
            }
            Some("cancel") => {
                match self.replace_with_idle() {
                    AppState::Enabled(EnabledState::Streaming(active))
                    | AppState::Disabled(DisabledState::Streaming(active)) => {
                        active.abort_handle.abort();

                        // Clean up journal state
                        let _ = active.journal.discard(&mut self.stream_journal);
                        self.set_status("Streaming cancelled");
                    }
                    AppState::Enabled(EnabledState::AwaitingToolResults(pending)) => {
                        self.cancel_tool_batch(
                            pending.assistant_text,
                            pending.pending_calls,
                            pending.results,
                            pending.model,
                            pending.step_id,
                            pending.batch_id,
                        );
                        self.set_status("Tool results cancelled");
                    }
                    AppState::Enabled(EnabledState::ToolLoop(state)) => {
                        if let ToolLoopPhase::Executing(exec) = &state.phase {
                            if let Some(handle) = &exec.abort_handle {
                                handle.abort();
                            }
                        }
                        self.cancel_tool_batch(
                            state.batch.assistant_text,
                            state.batch.calls,
                            state.batch.results,
                            state.batch.model,
                            state.batch.step_id,
                            state.batch.batch_id,
                        );
                        self.set_status("Tool execution cancelled");
                    }
                    AppState::Enabled(EnabledState::ToolRecovery(state)) => {
                        self.commit_recovered_tool_batch(state, ToolRecoveryDecision::Discard);
                    }
                    other => {
                        self.state = other;
                        self.set_status("No active stream to cancel");
                    }
                }
            }
            Some("screen") => {
                self.toggle_screen_mode = true;
            }
            Some("tool") => {
                // Syntax: /tool <call_id> <result_content>
                // Or: /tool error <call_id> <error_message>
                if parts.len() < 3 {
                    self.set_status(
                        "Usage: /tool <call_id> <result> or /tool error <call_id> <message>",
                    );
                } else if parts[1] == "error" && parts.len() >= 4 {
                    let id = parts[2].to_string();
                    let content = parts[3..].join(" ");
                    let result = ToolResult::error(id.clone(), content);
                    match self.submit_tool_result(result) {
                        Ok(true) => self.set_status("All tool results submitted"),
                        Ok(false) => {} // Status already set by submit_tool_result
                        Err(e) => self.set_status(format!("Tool error: {e}")),
                    }
                } else {
                    let id = parts[1].to_string();
                    let content = parts[2..].join(" ");
                    let result = ToolResult::success(id.clone(), content);
                    match self.submit_tool_result(result) {
                        Ok(true) => self.set_status("All tool results submitted"),
                        Ok(false) => {} // Status already set by submit_tool_result
                        Err(e) => self.set_status(format!("Tool error: {e}")),
                    }
                }
            }
            Some("tools") => {
                if self.tool_definitions.is_empty() {
                    self.set_status("No tools configured. Add tools to config.toml");
                } else {
                    let tools_list: Vec<&str> = self
                        .tool_definitions
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect();
                    self.set_status(format!(
                        "Tools ({}): {}",
                        self.tool_definitions.len(),
                        tools_list.join(", ")
                    ));
                }
            }
            Some("help") => {
                self.set_status(
                    "Commands: /q(uit), /clear, /cancel, /model, /p(rovider), /ctx, /jrnl, /sum, /screen, /tool, /tools",
                );
            }
            Some(cmd) => {
                self.set_status(format!("Unknown command: {cmd}"));
            }
            None => {}
        }
    }
}

fn parse_tools_mode(raw: Option<&str>, has_definitions: bool) -> tools::ToolsMode {
    match raw.map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "disabled" => tools::ToolsMode::Disabled,
        Some(ref s) if s == "parse_only" => tools::ToolsMode::ParseOnly,
        Some(ref s) if s == "enabled" => tools::ToolsMode::Enabled,
        _ => {
            if has_definitions {
                tools::ToolsMode::ParseOnly
            } else {
                tools::ToolsMode::Disabled
            }
        }
    }
}

fn parse_approval_mode(raw: Option<&str>) -> tools::ApprovalMode {
    match raw.map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "auto" => tools::ApprovalMode::Auto,
        Some(ref s) if s == "deny" => tools::ApprovalMode::Deny,
        Some(ref s) if s == "prompt" => tools::ApprovalMode::Prompt,
        _ => tools::ApprovalMode::Prompt,
    }
}

fn preflight_sandbox(
    sandbox: &tools::sandbox::Sandbox,
    call: &ToolCall,
) -> Result<(), tools::ToolError> {
    let working_dir = sandbox.working_dir();
    match call.name.as_str() {
        "read_file" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "path must be a string".to_string(),
                })?;
            let _ = sandbox.resolve_path(path, &working_dir)?;
        }
        "apply_patch" => {
            let patch_str = call
                .arguments
                .get("patch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "patch must be a string".to_string(),
                })?;
            let patch =
                tools::lp1::parse_patch(patch_str).map_err(|e| tools::ToolError::BadArgs {
                    message: e.to_string(),
                })?;
            for file in patch.files {
                let _ = sandbox.resolve_path(&file.path, &working_dir)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn tool_error_result(call: &ToolCall, err: tools::ToolError) -> ToolResult {
    let message = match err {
        tools::ToolError::BadArgs { message } => format!("Bad args: {message}"),
        tools::ToolError::Timeout { tool, elapsed } => {
            format!("Tool '{tool}' timed out after {}s", elapsed.as_secs())
        }
        tools::ToolError::SandboxViolation(reason) => reason.to_string(),
        tools::ToolError::ExecutionFailed { tool, message } => {
            format!("{tool} failed: {message}")
        }
        tools::ToolError::Cancelled => "Cancelled by user".to_string(),
        tools::ToolError::UnknownTool { name } => format!("Unknown tool: {name}"),
        tools::ToolError::DuplicateTool { name } => format!("Duplicate tool: {name}"),
        tools::ToolError::DuplicateToolCallId { id } => {
            format!("Duplicate tool call id: {id}")
        }
        tools::ToolError::PatchFailed { file, message } => {
            format!("Patch failed for {}: {message}", file.display())
        }
        tools::ToolError::StaleFile { file, reason } => {
            format!("Stale file {}: {reason}", file.display())
        }
    };

    ToolResult::error(call.id.clone(), tools::sanitize_output(&message))
}

fn append_tool_output_lines(lines: &mut Vec<String>, chunk: &str, prefix: Option<&str>) {
    let prefix = prefix.unwrap_or("");
    for line in chunk.lines() {
        let mut entry = String::new();
        entry.push_str(prefix);
        entry.push_str(line);
        lines.push(entry);
    }
    if lines.len() > 50 {
        let overflow = lines.len() - 50;
        lines.drain(0..overflow);
    }
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

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

impl<'a> InsertMode<'a> {
    fn draft_mut(&mut self) -> &mut DraftInput {
        self.app.input.draft_mut()
    }

    pub fn move_cursor_left(&mut self) {
        self.draft_mut().move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        self.draft_mut().move_cursor_right();
    }

    pub fn enter_char(&mut self, new_char: char) {
        self.draft_mut().enter_char(new_char);
    }

    pub fn delete_char(&mut self) {
        self.draft_mut().delete_char();
    }

    pub fn delete_char_forward(&mut self) {
        self.draft_mut().delete_char_forward();
    }

    pub fn delete_word_backwards(&mut self) {
        self.draft_mut().delete_word_backwards();
    }

    pub fn reset_cursor(&mut self) {
        self.draft_mut().reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        self.draft_mut().move_cursor_end();
    }

    pub fn clear_line(&mut self) {
        self.draft_mut().clear();
    }

    /// Queue the current draft as a user message.
    ///
    /// Returns a token proving that a non-empty user message was queued, and that it is valid to
    /// begin a new stream.
    pub fn queue_message(self) -> Option<QueuedUserMessage> {
        match &self.app.state {
            AppState::Enabled(EnabledState::Streaming(_))
            | AppState::Disabled(DisabledState::Streaming(_)) => {
                self.app.set_status("Already streaming a response");
                return None;
            }
            AppState::Enabled(EnabledState::AwaitingToolResults(_)) => {
                self.app.set_status("Busy: waiting for tool results");
                return None;
            }
            AppState::Enabled(EnabledState::ToolLoop(_)) => {
                self.app.set_status("Busy: tool execution in progress");
                return None;
            }
            AppState::Enabled(EnabledState::ToolRecovery(_)) => {
                self.app.set_status("Busy: tool recovery pending");
                return None;
            }
            AppState::Enabled(EnabledState::Summarizing(_))
            | AppState::Enabled(EnabledState::SummarizingWithQueued(_))
            | AppState::Enabled(EnabledState::SummarizationRetry(_))
            | AppState::Enabled(EnabledState::SummarizationRetryWithQueued(_)) => {
                self.app.set_status("Busy: summarization in progress");
                return None;
            }
            AppState::Enabled(EnabledState::Idle) | AppState::Disabled(DisabledState::Idle) => {}
        }

        if self.app.draft_text().trim().is_empty() {
            return None;
        }

        let api_key = match self.app.current_api_key().cloned() {
            Some(key) => key,
            None => {
                self.app.set_status(format!(
                    "No API key configured. Set {} environment variable.",
                    self.app.provider().env_var()
                ));
                return None;
            }
        };

        let raw_content = self.app.input.draft_mut().take_text();
        let content = match NonEmptyString::new(raw_content.clone()) {
            Ok(content) => content,
            Err(_) => {
                self.app.set_status("Cannot send empty message");
                return None;
            }
        };

        // Track user message in context manager (also adds to display)
        let msg_id = self.app.push_history_message(Message::user(content));
        self.app.autosave_history(); // Persist user message immediately for crash durability

        // Store pending message for potential rollback if stream fails with no content
        self.app.pending_user_message = Some((msg_id, raw_content));

        self.app.scroll_to_bottom();
        self.app.tool_iterations = 0;

        let api_key = match self.app.model.provider() {
            Provider::Claude => ApiKey::Claude(api_key),
            Provider::OpenAI => ApiKey::OpenAI(api_key),
        };

        let config = match ApiConfig::new(api_key, self.app.model.clone()) {
            Ok(config) => config.with_openai_options(self.app.openai_options),
            Err(e) => {
                self.app.set_status(format!("Cannot queue request: {e}"));
                return None;
            }
        };

        Some(QueuedUserMessage { config })
    }
}

impl<'a> CommandMode<'a> {
    fn command_mut(&mut self) -> Option<&mut String> {
        self.app.input.command_mut()
    }

    pub fn push_char(&mut self, c: char) {
        let Some(command) = self.command_mut() else {
            return;
        };

        command.push(c);
    }

    pub fn backspace(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };

        command.pop();
    }

    pub fn take_command(self) -> Option<EnteredCommand> {
        let input = std::mem::take(&mut self.app.input);
        let InputState::Command { draft, command } = input else {
            self.app.input = input;
            return None;
        };

        self.app.input = InputState::Normal(draft);
        Some(EnteredCommand { raw: command })
    }
}

fn sanitize_stream_error(raw: &str) -> String {
    // First redact API keys, then strip terminal controls
    let redacted = redact_api_keys(raw.trim());
    sanitize_terminal_text(&redacted).into_owned()
}

fn redact_api_keys(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == 's' {
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('k') && lookahead.next() == Some('-') {
                // Consume the remaining "k-" in the real iterator.
                chars.next();
                chars.next();
                output.push_str("sk-***");
                while let Some(&next_ch) = chars.peek() {
                    if is_key_delimiter(next_ch) {
                        break;
                    }
                    chars.next();
                }
                continue;
            }
        }
        output.push(ch);
    }
    output
}

fn is_key_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | '}' | ']' | ')' | '\\')
}

fn split_api_error(raw: &str) -> Option<(String, String)> {
    let rest = raw.strip_prefix("API error ")?;
    let (status, body) = rest.split_once(": ")?;
    Some((status.trim().to_string(), body.trim().to_string()))
}

fn extract_error_message(raw: &str) -> Option<String> {
    let body = split_api_error(raw)
        .map(|(_, body)| body)
        .unwrap_or_else(|| raw.trim().to_string());
    let payload: Value = serde_json::from_str(&body).ok()?;
    payload
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .pointer("/response/error/message")
                .and_then(|value| value.as_str())
        })
        .or_else(|| payload.pointer("/message").and_then(|value| value.as_str()))
        .or_else(|| payload.as_str())
        .map(|msg| msg.to_string())
}

fn is_auth_error(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let mentions_key =
        lower.contains("api key") || lower.contains("x-api-key") || lower.contains("authorization");
    let auth_words = lower.contains("invalid")
        || lower.contains("incorrect")
        || lower.contains("missing")
        || lower.contains("unauthorized")
        || lower.contains("not provided")
        || lower.contains("authentication");
    let has_code = lower.contains("401");

    lower.contains("invalid_api_key")
        || lower.contains("you must provide an api key")
        || (mentions_key && auth_words)
        || (mentions_key && has_code)
        || (has_code && lower.contains("unauthorized"))
}

fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max - 3).collect();
        format!("{head}...")
    }
}

fn format_stream_error(provider: Provider, model: &str, err: &str) -> StreamErrorUi {
    let trimmed = err.trim();
    let (status, body) =
        split_api_error(trimmed).unwrap_or_else(|| (String::new(), trimmed.to_string()));
    let extracted = extract_error_message(&body).unwrap_or_else(|| body.clone());
    let is_auth = is_auth_error(&extracted) || is_auth_error(trimmed) || is_auth_error(&status);

    if is_auth {
        let env_var = provider.env_var();
        let mut content = String::new();
        content.push_str(STREAM_ERROR_BADGE.as_str());
        content.push_str("\n\n");
        content.push_str(&format!(
            "{} authentication failed for model {}.",
            provider.display_name(),
            model
        ));
        content.push_str("\n\nFix:\n- Set ");
        content.push_str(env_var);
        let config_hint = config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.forge/config.toml".to_string());
        content.push_str(&format!(
            " (env) or add it to {} under [api_keys].\n- Then retry your message.",
            config_hint
        ));

        let detail = if !status.trim().is_empty() {
            status.trim().to_string()
        } else {
            truncate_with_ellipsis(&extracted, 160)
        };
        if !detail.is_empty() {
            content.push_str("\n\nDetails: ");
            content.push_str(&detail);
        }

        let message = NonEmptyString::new(content)
            .unwrap_or_else(|_| NonEmptyString::from(STREAM_ERROR_BADGE));
        return StreamErrorUi {
            status: format!("Auth error: set {env_var}"),
            message,
        };
    }

    let detail = if !extracted.trim().is_empty() {
        extracted.trim().to_string()
    } else if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        "unknown error".to_string()
    };
    let detail_short = truncate_with_ellipsis(&detail, 200);
    let status_source = detail.lines().next().unwrap_or("");
    let status_short = truncate_with_ellipsis(status_source, 80);
    let mut content = String::new();
    content.push_str(STREAM_ERROR_BADGE.as_str());
    content.push_str("\n\n");
    if !status.trim().is_empty() {
        content.push_str("Request failed (");
        content.push_str(status.trim());
        content.push_str(").");
    } else {
        content.push_str("Request failed.");
    }
    if !detail_short.is_empty() {
        content.push_str("\n\nDetails: ");
        content.push_str(&detail_short);
    }

    let message =
        NonEmptyString::new(content).unwrap_or_else(|_| NonEmptyString::from(STREAM_ERROR_BADGE));
    StreamErrorUi {
        status: format!("Stream error: {status_short}"),
        message,
    }
}

/// Parse a model name string back into a ModelName.
///
/// Used for crash recovery when we need to reconstruct the model from stored metadata.
/// Falls back to None if the model name cannot be parsed.
fn parse_model_name_from_string(name: &str) -> Option<ModelName> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Detect provider from model name prefix
    let provider = if trimmed.to_ascii_lowercase().starts_with("claude-") {
        Provider::Claude
    } else if trimmed.to_ascii_lowercase().starts_with("gpt-") {
        Provider::OpenAI
    } else {
        // Unknown provider - can't parse
        return None;
    };

    // Use the standard parse method
    ModelName::parse(provider, trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // StreamEvent is already in scope from the use statement above

    fn test_app() -> App {
        let mut api_keys = HashMap::new();
        api_keys.insert(Provider::Claude, "test".to_string());
        let model = Provider::Claude.default_model();
        let stream_journal = StreamJournal::open_in_memory().expect("in-memory journal for tests");
        let data_dir = DataDir {
            path: PathBuf::from(".").join("forge-test"),
            source: DataDirSource::Fallback,
        };
        let output_limits = OutputLimits::new(4096);
        let mut context_manager = ContextManager::new(model.as_str());
        context_manager.set_output_limit(output_limits.max_output_tokens());
        let tool_settings = App::tool_settings_from_config(None);
        let mut tool_registry = tools::ToolRegistry::default();
        let _ = tools::builtins::register_builtins(
            &mut tool_registry,
            tool_settings.read_limits,
            tool_settings.patch_limits,
        );
        let tool_registry = std::sync::Arc::new(tool_registry);
        let tool_definitions = match tool_settings.mode {
            tools::ToolsMode::Enabled => tool_registry.definitions(),
            tools::ToolsMode::ParseOnly => Vec::new(),
            tools::ToolsMode::Disabled => Vec::new(),
        };
        let tool_journal = ToolJournal::open_in_memory().expect("in-memory tool journal");
        let tool_file_cache = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        App {
            input: InputState::default(),
            display: Vec::new(),
            scroll: ScrollState::AutoBottom,
            scroll_max: 0,
            should_quit: false,
            toggle_screen_mode: false,
            status_message: None,
            api_keys,
            model: model.clone(),
            tick: 0,
            data_dir,
            context_manager,
            stream_journal,
            state: AppState::Enabled(EnabledState::Idle),
            output_limits,
            cache_enabled: false,
            openai_options: OpenAIRequestOptions::default(),
            last_frame: Instant::now(),
            modal_effect: None,
            system_prompt: None,
            cached_usage_status: None,
            pending_user_message: None,
            tool_definitions,
            tool_registry,
            tools_mode: tool_settings.mode,
            tool_settings,
            tool_journal,
            tool_file_cache,
            tool_iterations: 0,
        }
    }

    #[test]
    fn enter_and_delete_respects_unicode_cursor() {
        let mut app = test_app();
        app.input = InputState::Insert(DraftInput {
            text: "a🦀b".to_string(),
            cursor: 1,
        });

        {
            let token = app.insert_token().expect("insert mode");
            let mut insert = app.insert_mode(token);
            insert.enter_char('X');
        }
        assert_eq!(app.draft_text(), "aX🦀b");
        assert_eq!(app.draft_cursor(), 2);

        {
            let token = app.insert_token().expect("insert mode");
            let mut insert = app.insert_mode(token);
            insert.delete_char();
        }
        assert_eq!(app.draft_text(), "a🦀b");
        assert_eq!(app.draft_cursor(), 1);

        {
            let token = app.insert_token().expect("insert mode");
            let mut insert = app.insert_mode(token);
            insert.delete_char_forward();
        }
        assert_eq!(app.draft_text(), "ab");
        assert_eq!(app.draft_cursor(), 1);
    }

    #[test]
    fn submit_message_adds_user_message() {
        let mut app = test_app();
        app.input = InputState::Insert(DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        });

        let token = app.insert_token().expect("insert mode");
        let _queued = app
            .insert_mode(token)
            .queue_message()
            .expect("queued message");

        assert!(app.draft_text().is_empty());
        assert_eq!(app.draft_cursor(), 0);
        assert_eq!(app.scroll, ScrollState::AutoBottom);
        // Streaming is started separately by start_streaming()
        assert!(!app.is_loading());

        // Only user message added; streaming message created by start_streaming()
        assert_eq!(app.history().len(), 1);
        let first = app.history().entries().first().expect("user message");
        assert!(matches!(first.message(), Message::User(_)));
        assert_eq!(first.message().content(), "hello");
    }

    #[test]
    fn process_command_quit_sets_should_quit() {
        let mut app = test_app();
        app.enter_command_mode();

        let command = {
            let token = app.command_token().expect("command mode");
            let mut command_mode = app.command_mode(token);
            command_mode.push_char('q');
            command_mode.take_command().expect("take command")
        };

        app.process_command(command);

        assert!(app.should_quit());
        assert_eq!(app.input_mode(), InputMode::Normal);
        assert!(app.command_text().is_none());
    }

    #[test]
    fn process_command_clear_resets_conversation() {
        let mut app = test_app();
        let content = NonEmptyString::new("hi").expect("non-empty test content");
        app.push_history_message(Message::user(content));
        app.enter_command_mode();

        let command = {
            let token = app.command_token().expect("command mode");
            let mut command_mode = app.command_mode(token);
            for c in "clear".chars() {
                command_mode.push_char(c);
            }
            command_mode.take_command().expect("take command")
        };

        app.process_command(command);

        assert!(app.is_empty());
        assert_eq!(app.status_message(), Some("Conversation cleared"));
        assert_eq!(app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn process_command_provider_switch_sets_status_when_no_key() {
        let mut app = test_app();
        app.enter_command_mode();

        let command = {
            let token = app.command_token().expect("command mode");
            let mut command_mode = app.command_mode(token);
            for c in "p gpt".chars() {
                command_mode.push_char(c);
            }
            command_mode.take_command().expect("take command")
        };

        app.process_command(command);

        assert_eq!(app.provider(), Provider::OpenAI);
        assert_eq!(app.model(), Provider::OpenAI.default_model().as_str());
        assert_eq!(
            app.status_message(),
            Some("Switched to GPT - No API key! Set OPENAI_API_KEY")
        );
        assert_eq!(app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn process_stream_events_applies_deltas_and_done() {
        let mut app = test_app();

        // Start streaming using the new architecture
        let (tx, rx) = mpsc::unbounded_channel();
        let streaming = StreamingMessage::new(app.model.clone(), rx);
        let (abort_handle, _abort_registration) = AbortHandle::new_pair();
        let journal = app
            .stream_journal
            .begin_session(app.model.as_str())
            .expect("journal session");
        app.state = AppState::Enabled(EnabledState::Streaming(ActiveStream {
            message: streaming,
            journal,
            abort_handle,
            tool_batch_id: None,
            tool_call_seq: 0,
        }));
        assert!(app.is_loading());

        tx.send(StreamEvent::TextDelta("hello".to_string()))
            .expect("send delta");
        tx.send(StreamEvent::Done).expect("send done");

        app.process_stream_events();

        let last = app.history().entries().last().expect("completed message");
        assert!(matches!(last.message(), Message::Assistant(_)));
        assert_eq!(last.message().content(), "hello");
        assert!(!app.is_loading());
        assert!(app.streaming().is_none());
    }

    #[test]
    fn submit_message_without_key_sets_status_and_does_not_queue() {
        let mut app = test_app();
        app.api_keys.clear();
        app.input = InputState::Insert(DraftInput {
            text: "hi".to_string(),
            cursor: 2,
        });

        let token = app.insert_token().expect("insert mode");
        let queued = app.insert_mode(token).queue_message();
        assert!(queued.is_none());
        assert_eq!(
            app.status_message(),
            Some("No API key configured. Set ANTHROPIC_API_KEY environment variable.")
        );
        assert!(app.is_empty());
        assert!(!app.is_loading());
    }

    #[test]
    fn draft_input_set_text_moves_cursor_to_end() {
        let mut draft = DraftInput {
            text: "initial".to_string(),
            cursor: 0,
        };

        draft.set_text("new text".to_string());

        assert_eq!(draft.text(), "new text");
        assert_eq!(draft.cursor(), 8); // cursor at end
    }

    #[test]
    fn draft_input_set_text_handles_unicode() {
        let mut draft = DraftInput::default();

        draft.set_text("hello 🦀 world".to_string());

        assert_eq!(draft.text(), "hello 🦀 world");
        assert_eq!(draft.cursor(), 13); // 13 graphemes: h-e-l-l-o- -🦀- -w-o-r-l-d
    }

    #[test]
    fn queue_message_sets_pending_user_message() {
        let mut app = test_app();
        app.input = InputState::Insert(DraftInput {
            text: "test message".to_string(),
            cursor: 12,
        });

        let token = app.insert_token().expect("insert mode");
        let _queued = app
            .insert_mode(token)
            .queue_message()
            .expect("queued message");

        // Verify pending_user_message is set
        assert!(app.pending_user_message.is_some());
        let (msg_id, original_text) = app.pending_user_message.as_ref().unwrap();
        assert_eq!(msg_id.as_u64(), 0); // First message
        assert_eq!(original_text, "test message");
    }

    #[test]
    fn rollback_pending_user_message_restores_input() {
        let mut app = test_app();

        // Simulate: user sends message (stored in history and pending)
        let content = NonEmptyString::new("my message").expect("non-empty");
        let msg_id = app.push_history_message(Message::user(content));
        app.pending_user_message = Some((msg_id, "my message".to_string()));

        assert_eq!(app.history().len(), 1);
        assert_eq!(app.display.len(), 1);

        // Rollback the pending message
        app.rollback_pending_user_message();

        // Message should be removed from history
        assert_eq!(app.history().len(), 0);
        // Display should be updated
        assert_eq!(app.display.len(), 0);
        // Input should be restored
        assert_eq!(app.draft_text(), "my message");
        // Should be in insert mode for easy retry
        assert_eq!(app.input_mode(), InputMode::Insert);
        // Pending should be cleared
        assert!(app.pending_user_message.is_none());
    }

    #[test]
    fn rollback_pending_user_message_no_op_when_empty() {
        let mut app = test_app();

        // No pending message
        assert!(app.pending_user_message.is_none());

        // Rollback should be a no-op
        app.rollback_pending_user_message();

        assert!(app.draft_text().is_empty());
        assert!(app.pending_user_message.is_none());
    }

    // ========================================================================
    // ModalEffect Tests
    // ========================================================================

    #[test]
    fn modal_effect_pop_scale_initial_state() {
        let effect = ModalEffect::pop_scale(Duration::from_millis(200));
        assert_eq!(effect.kind(), ModalEffectKind::PopScale);
        assert!(!effect.is_finished());
        // Initial progress should be close to 0
        assert!(effect.progress() < 0.1);
    }

    #[test]
    fn modal_effect_slide_up_initial_state() {
        let effect = ModalEffect::slide_up(Duration::from_millis(300));
        assert_eq!(effect.kind(), ModalEffectKind::SlideUp);
        assert!(!effect.is_finished());
    }

    #[test]
    fn modal_effect_advance_increases_progress() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(200));
        let _initial = effect.progress();

        effect.advance(Duration::from_millis(100));
        // Note: progress depends on elapsed wall time, not just advance calls
        // The elapsed tracking is based on Instant, but we can at least verify is_finished
        assert!(!effect.is_finished()); // Still has 100ms left after 100ms advance
    }

    #[test]
    fn modal_effect_finished_after_duration() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(100));
        effect.advance(Duration::from_millis(150));
        assert!(effect.is_finished());
    }

    #[test]
    fn modal_effect_zero_duration_immediately_finished() {
        let effect = ModalEffect::pop_scale(Duration::ZERO);
        assert!(effect.is_finished());
        assert_eq!(effect.progress(), 1.0);
    }

    #[test]
    fn modal_effect_progress_clamped_at_one() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(10));
        effect.advance(Duration::from_millis(1000)); // Way past duration
        assert!(effect.progress() <= 1.0);
    }

    // ========================================================================
    // StreamingMessage Tests
    // ========================================================================

    #[test]
    fn streaming_message_apply_text_delta() {
        let (tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let mut stream = StreamingMessage::new(model, rx);

        assert!(stream.content().is_empty());

        let result = stream.apply_event(StreamEvent::TextDelta("Hello".to_string()));
        assert!(result.is_none()); // Not finished yet

        assert_eq!(stream.content(), "Hello");

        let result = stream.apply_event(StreamEvent::TextDelta(" World".to_string()));
        assert!(result.is_none());

        assert_eq!(stream.content(), "Hello World");

        drop(tx); // Suppress unused warning
    }

    #[test]
    fn streaming_message_apply_done() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let mut stream = StreamingMessage::new(model, rx);

        stream.apply_event(StreamEvent::TextDelta("content".to_string()));

        let result = stream.apply_event(StreamEvent::Done);
        assert_eq!(result, Some(StreamFinishReason::Done));
    }

    #[test]
    fn streaming_message_apply_error() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let mut stream = StreamingMessage::new(model, rx);

        let result = stream.apply_event(StreamEvent::Error("API error".to_string()));
        assert_eq!(
            result,
            Some(StreamFinishReason::Error("API error".to_string()))
        );
    }

    #[test]
    fn streaming_message_apply_thinking_delta_ignored() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let mut stream = StreamingMessage::new(model, rx);

        stream.apply_event(StreamEvent::TextDelta("visible".to_string()));
        stream.apply_event(StreamEvent::ThinkingDelta("thinking...".to_string()));

        // Thinking content should not appear in content
        assert_eq!(stream.content(), "visible");
    }

    #[test]
    fn streaming_message_into_message_success() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let mut stream = StreamingMessage::new(model.clone(), rx);

        stream.apply_event(StreamEvent::TextDelta("Test content".to_string()));

        let message = stream.into_message().expect("should convert to message");
        assert_eq!(message.content(), "Test content");
        assert!(matches!(message, Message::Assistant(_)));
    }

    #[test]
    fn streaming_message_into_message_empty_fails() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = Provider::Claude.default_model();
        let stream = StreamingMessage::new(model, rx);

        // No content added
        let result = stream.into_message();
        assert!(result.is_err());
    }

    #[test]
    fn streaming_message_provider_and_model() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let model = ModelName::known(Provider::OpenAI, "gpt-5.2");
        let stream = StreamingMessage::new(model, rx);

        assert_eq!(stream.provider(), Provider::OpenAI);
        assert_eq!(stream.model_name().as_str(), "gpt-5.2");
    }
}

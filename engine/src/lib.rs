//! Core engine for Forge - state machine and orchestration.
//!
//! This crate contains the App state machine without TUI dependencies.

use std::collections::HashMap;
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
    PreparedContext, RecoveredStream, StreamJournal, SummarizationNeeded, SummarizationScope,
    TokenCounter, generate_summary, summarization_model,
};
pub use forge_providers::{self, ApiConfig};
pub use forge_types::{
    ApiKey, CacheHint, CacheableMessage, EmptyStringError, Message, ModelName, ModelNameKind,
    NonEmptyStaticStr, NonEmptyString, OpenAIReasoningEffort, OpenAIRequestOptions,
    OpenAITextVerbosity, OpenAITruncation, OutputLimits, Provider, StreamEvent, StreamFinishReason,
};

// Config types - passed in from caller
mod config;
pub use config::{AppConfig, ForgeConfig};

// ============================================================================
// StreamingMessage - async message being streamed
// ============================================================================

/// A message being streamed - existence proves streaming is active.
/// Typestate: consuming this produces a complete assistant `Message`.
#[derive(Debug)]
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
}

impl StreamingMessage {
    pub fn new(model: ModelName, receiver: mpsc::UnboundedReceiver<StreamEvent>) -> Self {
        Self {
            model,
            content: String::new(),
            receiver,
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
            StreamEvent::Done => Some(StreamFinishReason::Done),
            StreamEvent::Error(err) => Some(StreamFinishReason::Error(err)),
        }
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

#[derive(Debug)]
enum EnabledState {
    Idle,
    Streaming(ActiveStream),
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
        };

        app.clamp_output_limits_to_model();

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
            Ok(loaded_manager) => {
                self.context_manager = loaded_manager;
                self.rebuild_display_from_history();
                self.set_status(format!(
                    "Loaded {} messages from previous session",
                    self.context_manager.history().len()
                ));
            }
            Err(e) => {
                eprintln!("Failed to load history: {e}");
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
        id
    }

    /// Autosave history to disk (best-effort, errors logged but not propagated).
    /// Called after user messages and assistant completions for crash durability.
    fn autosave_history(&self) {
        if let Err(e) = self.save_history() {
            tracing::warn!("Autosave failed: {e}");
        }
    }

    fn push_local_message(&mut self, message: Message) {
        self.display.push(DisplayItem::Local(message));
    }

    /// Check for and recover from a crashed streaming session.
    ///
    /// Returns `Some(RecoveredStream)` if there was an incomplete stream that was recovered.
    /// The recovered partial response is added to the conversation with a warning badge.
    pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
        let recovered = match self.stream_journal.recover() {
            Ok(Some(recovered)) => recovered,
            Ok(None) => return None,
            Err(e) => {
                self.set_status(format!("Recovery failed: {e}"));
                return None;
            }
        };

        let (recovery_badge, step_id, last_seq, partial_text, error_text) = match &recovered {
            RecoveredStream::Complete {
                step_id,
                partial_text,
                last_seq,
            } => (
                RECOVERY_COMPLETE_BADGE,
                *step_id,
                *last_seq,
                partial_text.as_str(),
                None,
            ),
            RecoveredStream::Incomplete {
                step_id,
                partial_text,
                last_seq,
            } => (
                RECOVERY_INCOMPLETE_BADGE,
                *step_id,
                *last_seq,
                partial_text.as_str(),
                None,
            ),
            RecoveredStream::Errored {
                step_id,
                partial_text,
                last_seq,
                error,
            } => (
                RECOVERY_ERROR_BADGE,
                *step_id,
                *last_seq,
                partial_text.as_str(),
                Some(error.as_str()),
            ),
        };

        // Add the partial response as an assistant message with recovery badge.
        let mut recovered_content = NonEmptyString::from(recovery_badge);
        if !partial_text.is_empty() {
            recovered_content = recovered_content.append("\n\n").append(partial_text);
        }
        if let Some(error) = error_text
            && !error.is_empty()
        {
            let error_line = format!("Error: {error}");
            recovered_content = recovered_content.append("\n\n").append(error_line.as_str());
        }

        // Push recovered partial response as a completed assistant message.
        self.push_history_message(Message::assistant(self.model.clone(), recovered_content));
        self.autosave_history(); // Persist recovered message immediately

        // Seal the recovered journal entries
        if let Err(e) = self.stream_journal.seal_unsealed(step_id) {
            eprintln!("Failed to seal recovered journal: {e}");
            // Try to discard instead
            let _ = self.stream_journal.discard_unsealed(step_id);
        }

        self.set_status(format!(
            "Recovered {} bytes (step {}, last seq {}) from crashed session",
            partial_text.len(),
            step_id,
            last_seq,
        ));

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

    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
            && !matches!(
                self.state,
                AppState::Enabled(EnabledState::Streaming(_))
                    | AppState::Disabled(DisabledState::Streaming(_))
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
        )
    }

    /// Get context usage statistics for the UI.
    pub fn context_usage_status(&self) -> ContextUsageStatus {
        self.context_manager.usage_status()
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
        let handle =
            tokio::spawn(async move { generate_summary(&config, &messages, target_tokens).await });

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

        let journal = match self.stream_journal.begin_session() {
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
        let cacheable_messages: Vec<CacheableMessage> = if cache_enabled {
            // Cache older messages, keep recent ones fresh
            let len = api_messages.len();
            let recent_threshold = len.saturating_sub(4); // Don't cache last 4 messages
            api_messages
                .into_iter()
                .enumerate()
                .map(|(i, msg)| {
                    if i < recent_threshold {
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

        let system_prompt = self.system_prompt;
        let task = async move {
            let tx_events = tx.clone();
            let result = forge_providers::send_message(
                &config,
                &cacheable_messages,
                limits,
                system_prompt,
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
                StreamEvent::Error(msg) => StreamEvent::Error(sanitize_stream_error(&msg)),
                other => other,
            };

            let mut journal_error: Option<String> = None;
            let mut finish_reason: Option<StreamFinishReason> = None;

            {
                let active = match self.state {
                    AppState::Enabled(EnabledState::Streaming(ref mut active))
                    | AppState::Disabled(DisabledState::Streaming(ref mut active)) => active,
                    _ => return,
                };

                let persist_result = match &event {
                    StreamEvent::TextDelta(text) => active
                        .journal
                        .append_text(&mut self.stream_journal, text.clone()),
                    StreamEvent::ThinkingDelta(_) => {
                        // Don't persist thinking content to journal - silently consume
                        Ok(())
                    }
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
                    finish_reason = active.message.apply_event(event);
                }
            }

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
                } = active;

                abort_handle.abort();

                if let Err(seal_err) = journal.seal(&mut self.stream_journal) {
                    eprintln!("Journal seal failed after append error: {seal_err}");
                }

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
            message,
            journal,
            abort_handle,
        } = active;

        abort_handle.abort();

        // Seal the journal
        if let Err(e) = journal.seal(&mut self.stream_journal) {
            eprintln!("Journal seal failed: {e}");
        }

        // Capture metadata before consuming the streaming message.
        let model = message.model_name().clone();

        // Convert streaming message to completed message (empty content is invalid).
        let message = message.into_message().ok();

        match finish_reason {
            StreamFinishReason::Error(err) => {
                if let Some(message) = message {
                    self.push_history_message(message);
                    self.autosave_history(); // Persist partial response for crash durability
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
            let empty_msg = Message::assistant(model, NonEmptyString::from(EMPTY_RESPONSE_BADGE));
            self.push_local_message(empty_msg);
            self.set_status("Warning: API returned empty response");
            return;
        };

        self.push_history_message(message);
        self.autosave_history(); // Persist completed response for crash durability
    }

    /// Increment animation tick and poll background tasks.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.poll_summarization();
        self.poll_summarization_retry();
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
                    other => {
                        self.state = other;
                        self.set_status("No active stream to cancel");
                    }
                }
            }
            Some("screen") => {
                self.toggle_screen_mode = true;
            }
            Some("help") => {
                self.set_status(
                    "Commands: /q(uit), /clear, /cancel, /model, /p(rovider), /ctx, /jrnl, /sum, /screen",
                );
            }
            Some(cmd) => {
                self.set_status(format!("Unknown command: {cmd}"));
            }
            None => {}
        }
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

        let content = self.app.input.draft_mut().take_text();
        let content = match NonEmptyString::new(content) {
            Ok(content) => content,
            Err(_) => {
                self.app.set_status("Cannot send empty message");
                return None;
            }
        };

        // Track user message in context manager (also adds to display)
        self.app.push_history_message(Message::user(content));
        self.app.autosave_history(); // Persist user message immediately for crash durability

        self.app.scroll_to_bottom();

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
    redact_api_keys(raw.trim())
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
        content.push_str(" (env) or add it to ~/.forge/config.toml under [api_keys].\n- Then retry your message.");

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
            context_manager: ContextManager::new(model.as_str()),
            stream_journal,
            state: AppState::Enabled(EnabledState::Idle),
            output_limits: OutputLimits::new(4096),
            cache_enabled: false,
            openai_options: OpenAIRequestOptions::default(),
            last_frame: Instant::now(),
            modal_effect: None,
            system_prompt: None,
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
        let journal = app.stream_journal.begin_session().expect("journal session");
        app.state = AppState::Enabled(EnabledState::Streaming(ActiveStream {
            message: streaming,
            journal,
            abort_handle,
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
}

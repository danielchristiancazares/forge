use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc;

use crate::context_infinity::{
    ContextAdaptation, ContextManager, ContextUsage, JournalSession, MessageId, ModelLimitsSource,
    PendingSummarization, RecoveredStream, StreamJournal, SummarizationScope, SummaryId,
    generate_summary, summarization_model,
};
use crate::message::{
    Message, NonEmptyString, StreamDisposition, StreamFinishReason, StreamingMessage,
};
use crate::provider::{ApiConfig, ApiKey, ModelName, ModelNameKind, Provider, StreamEvent};

/// A background summarization task.
///
/// Holds the state for an in-progress summarization operation:
/// - The summary ID that will be used when complete
/// - The message IDs being summarized
/// - The JoinHandle for the async task
#[derive(Debug)]
pub struct SummarizationTask {
    summary_id: SummaryId,
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

/// Input mode for the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
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

        let current_index = self.cursor;
        let from_left_to_current_index = current_index - 1;

        let before_char_to_delete = self.text.chars().take(from_left_to_current_index);
        let after_char_to_delete = self.text.chars().skip(current_index);

        self.text = before_char_to_delete.chain(after_char_to_delete).collect();
        self.move_cursor_left();
    }

    fn delete_char_forward(&mut self) {
        let current_index = self.cursor;
        if current_index >= self.text.chars().count() {
            return;
        }

        let before_char = self.text.chars().take(current_index);
        let after_char = self.text.chars().skip(current_index + 1);

        self.text = before_char.chain(after_char).collect();
    }

    fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn delete_word_backwards(&mut self) {
        while self.cursor > 0 {
            let idx = self.cursor - 1;
            let ch = self.text.chars().nth(idx);
            if ch.is_some_and(|c| c.is_whitespace()) {
                self.delete_char();
            } else {
                break;
            }
        }

        while self.cursor > 0 {
            let idx = self.cursor - 1;
            let ch = self.text.chars().nth(idx);
            if ch.is_some_and(|c| !c.is_whitespace()) {
                self.delete_char();
            } else {
                break;
            }
        }
    }

    fn byte_index(&self) -> usize {
        self.text
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.cursor)
            .unwrap_or(self.text.len())
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.text.chars().count())
    }
}

#[derive(Debug)]
enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
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
        }
    }

    fn draft(&self) -> &DraftInput {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => draft,
            InputState::Command { draft, .. } => draft,
        }
    }

    fn draft_mut(&mut self) -> &mut DraftInput {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => draft,
            InputState::Command { draft, .. } => draft,
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

    fn into_normal(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Normal(draft),
            InputState::Command { draft, .. } => InputState::Normal(draft),
        }
    }

    fn into_insert(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Insert(draft),
            InputState::Command { draft, .. } => InputState::Insert(draft),
        }
    }

    fn into_command(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Command {
                draft,
                command: String::new(),
            },
            InputState::Command { draft, .. } => InputState::Command {
                draft,
                command: String::new(),
            },
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
#[derive(Debug)]
pub struct QueuedUserMessage {
    config: ApiConfig,
}

#[derive(Debug)]
struct ActiveStream {
    message: StreamingMessage,
    journal: JournalSession,
    abort_handle: AbortHandle,
}

#[derive(Debug)]
struct SummarizationState {
    task: SummarizationTask,
    queued_request: Option<ApiConfig>,
}

#[derive(Debug)]
struct SummarizationRetryState {
    retry: SummarizationRetry,
    queued_request: Option<ApiConfig>,
}

#[derive(Debug, Default)]
enum AppState {
    #[default]
    Idle,
    Streaming(ActiveStream),
    Summarizing(SummarizationState),
    SummarizationRetry(SummarizationRetryState),
}

#[derive(Debug, Clone)]
pub(crate) enum DisplayItem {
    History(MessageId),
    Local(Message),
}

/// Proof that a command line was entered in Command mode.
#[derive(Debug)]
pub(crate) struct EnteredCommand {
    raw: String,
}

#[derive(Debug)]
pub(crate) struct InsertToken(());

#[derive(Debug)]
pub(crate) struct CommandToken(());

pub(crate) struct InsertMode<'a> {
    app: &'a mut App,
}

pub(crate) struct CommandMode<'a> {
    app: &'a mut App,
}

/// Application state
pub struct App {
    input: InputState,
    display: Vec<DisplayItem>,
    scroll: ScrollState,
    scroll_max: u16,
    should_quit: bool,
    status_message: Option<String>,
    api_keys: HashMap<Provider, String>,
    model: ModelName,
    tick: usize,
    /// Whether ContextInfinity (summarization) is enabled.
    context_infinity_enabled: bool,
    /// Context manager for adaptive context window management.
    context_manager: ContextManager,
    /// Stream journal for crash recovery.
    stream_journal: StreamJournal,
    state: AppState,
}

impl App {
    pub fn new() -> anyhow::Result<Self> {
        // Try to load API keys from environment
        let mut api_keys = HashMap::new();
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            api_keys.insert(Provider::Claude, key);
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            api_keys.insert(Provider::OpenAI, key);
        }

        // Default to Claude if available, otherwise OpenAI
        let provider = if api_keys.contains_key(&Provider::Claude) {
            Provider::Claude
        } else if api_keys.contains_key(&Provider::OpenAI) {
            Provider::OpenAI
        } else {
            Provider::Claude // Default even without key
        };

        let model = provider.default_model();
        let context_manager = ContextManager::new(model.as_str());
        let context_infinity_enabled = Self::context_infinity_enabled_from_env();

        // Initialize stream journal (required for streaming durability).
        let journal_path = Self::journal_path();
        let stream_journal = StreamJournal::open(&journal_path)?;

        let mut app = Self {
            input: InputState::default(),
            display: Vec::new(),
            scroll: ScrollState::AutoBottom,
            scroll_max: 0,
            should_quit: false,
            status_message: None,
            api_keys,
            model,
            tick: 0,
            context_infinity_enabled,
            context_manager,
            stream_journal,
            state: AppState::Idle,
        };

        // Load previous session's history if available
        app.load_history_if_exists();
        app.check_crash_recovery();

        Ok(app)
    }

    /// Get the base data directory for forge.
    fn data_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("forge")
    }

    /// Get the path to the stream journal database.
    fn journal_path() -> PathBuf {
        Self::data_dir().join("stream_journal.db")
    }

    /// Get the path to the history file.
    fn history_path() -> PathBuf {
        Self::data_dir().join("history.json")
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

    /// Save the conversation history to disk.
    pub fn save_history(&self) -> anyhow::Result<()> {
        let path = Self::history_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.context_manager.save(&path)
    }

    /// Load conversation history from disk (called during init if file exists).
    fn load_history_if_exists(&mut self) {
        let path = Self::history_path();
        if !path.exists() {
            return;
        }

        match crate::context_infinity::ContextManager::load(&path, self.model.as_str()) {
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

    fn push_local_message(&mut self, message: Message) {
        self.display.push(DisplayItem::Local(message));
    }

    /// Check for and recover from a crashed streaming session.
    ///
    /// Returns `Some(RecoveredStream)` if there was an incomplete stream that was recovered.
    /// The recovered partial response is added to the conversation with a warning badge.
    pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
        let recovered = self.stream_journal.recover()?;

        let (recovery_badge, step_id, last_seq, partial_text) = match &recovered {
            RecoveredStream::Complete {
                step_id,
                partial_text,
                last_seq,
            } => (
                "[Recovered - stream completed but not finalized]",
                *step_id,
                *last_seq,
                partial_text,
            ),
            RecoveredStream::Incomplete {
                step_id,
                partial_text,
                last_seq,
            } => (
                "[Recovered - incomplete response from previous session]",
                *step_id,
                *last_seq,
                partial_text,
            ),
        };

        // Add the partial response as an assistant message with recovery badge.
        let recovered_content = if partial_text.is_empty() {
            recovery_badge.to_string()
        } else {
            format!("{}\n\n{}", recovery_badge, partial_text)
        };
        let recovered_content = NonEmptyString::from_string_or(recovered_content, recovery_badge);

        // Push recovered partial response as a completed assistant message.
        self.push_history_message(Message::assistant(self.model.clone(), recovered_content));

        // Seal the recovered journal entries
        if let Err(e) = self.stream_journal.seal() {
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

    pub fn history(&self) -> &crate::context_infinity::FullHistory {
        self.context_manager.history()
    }

    pub fn streaming(&self) -> Option<&StreamingMessage> {
        match &self.state {
            AppState::Streaming(active) => Some(&active.message),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.display.is_empty() && !matches!(self.state, AppState::Streaming(_))
    }

    pub(crate) fn display_items(&self) -> &[DisplayItem] {
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
        matches!(self.state, AppState::Streaming(_))
    }

    /// Get context usage statistics for the UI.
    pub fn context_usage(&self) -> ContextUsage {
        self.context_manager.usage()
    }

    pub fn context_infinity_enabled(&self) -> bool {
        self.context_infinity_enabled
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

    /// Switch to a different provider
    pub fn set_provider(&mut self, provider: Provider) {
        self.model = provider.default_model();
        if self.context_infinity_enabled {
            // Notify context manager of model change for adaptive context
            self.handle_context_adaptation();
        } else {
            self.context_manager
                .set_model_without_adaptation(self.model.as_str());
        }
    }

    /// Set a specific model (called from :model command).
    pub fn set_model(&mut self, model: ModelName) {
        self.model = model;
        if self.context_infinity_enabled {
            self.handle_context_adaptation();
        } else {
            self.context_manager
                .set_model_without_adaptation(self.model.as_str());
        }
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
        if !self.context_infinity_enabled {
            self.set_status("ContextInfinity disabled: summarization unavailable");
            return SummarizationStart::Failed;
        }
        if attempt > MAX_SUMMARIZATION_ATTEMPTS {
            return SummarizationStart::Failed;
        }

        match self.state {
            AppState::Streaming(_) => {
                self.set_status("Cannot summarize while streaming");
                return SummarizationStart::Failed;
            }
            AppState::Summarizing(_) | AppState::SummarizationRetry(_) => {
                return SummarizationStart::Failed;
            }
            AppState::Idle => {}
        }

        // Try to build working context to see if summarization is needed
        let message_ids = match self.context_manager.build_working_context() {
            Ok(_) => return SummarizationStart::NotNeeded, // No summarization needed
            Err(needed) => needed.messages_to_summarize,
        };

        // Prepare summarization request
        let Some(pending) = self.context_manager.prepare_summarization(&message_ids) else {
            return SummarizationStart::Failed;
        };

        let PendingSummarization {
            summary_id,
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
        // Prefer the queued request's key if present so we don't get stuck.
        let api_key = if let Some(config) = queued_request.as_ref() {
            config.api_key_owned()
        } else {
            match self.current_api_key().cloned() {
                Some(key) => match self.model.provider() {
                    Provider::Claude => ApiKey::Claude(key),
                    Provider::OpenAI => ApiKey::OpenAI(key),
                },
                None => {
                    self.set_status("Cannot summarize: no API key configured");
                    return SummarizationStart::Failed;
                }
            }
        };

        let config = match ApiConfig::new(api_key, self.model.clone()) {
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
            summary_id,
            scope,
            generated_by,
            handle,
            attempt,
        };

        self.state = AppState::Summarizing(SummarizationState {
            task,
            queued_request,
        });
        SummarizationStart::Started
    }

    /// Poll for completed summarization task and apply the result.
    ///
    /// This should be called in the main tick() loop. It checks if the background
    /// summarization task has completed, and if so, applies the result via
    /// `context_manager.complete_summarization()`.
    pub fn poll_summarization(&mut self) {
        use futures_util::future::FutureExt;

        if !self.context_infinity_enabled {
            return;
        }

        let finished = match &self.state {
            AppState::Summarizing(state) => state.task.handle.is_finished(),
            _ => return,
        };

        // Check if the task is finished (non-blocking)
        if !finished {
            return;
        }

        // Take ownership of the task
        let state = match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Summarizing(state) => state,
            other => {
                self.state = other;
                return;
            }
        };

        let SummarizationTask {
            summary_id,
            scope,
            generated_by,
            handle,
            attempt,
        } = state.task;
        let queued_request = state.queued_request;

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
                self.context_manager.complete_summarization(
                    summary_id,
                    scope,
                    summary_text,
                    generated_by,
                );
                self.set_status("Summarization complete");

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
        self.state = AppState::Idle;
        let next_attempt = attempt.saturating_add(1);

        if next_attempt <= MAX_SUMMARIZATION_ATTEMPTS {
            let delay = summarization_retry_delay(next_attempt);
            self.state = AppState::SummarizationRetry(SummarizationRetryState {
                retry: SummarizationRetry {
                    attempt: next_attempt,
                    ready_at: Instant::now() + delay,
                },
                queued_request,
            });
            self.set_status(format!(
                "Summarization failed (attempt {}/{}): {}. Retrying in {}ms...",
                attempt,
                MAX_SUMMARIZATION_ATTEMPTS,
                error,
                delay.as_millis()
            ));
            return;
        }

        let suffix = if queued_request.is_some() {
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
        if !self.context_infinity_enabled {
            return;
        }

        let ready = match &self.state {
            AppState::SummarizationRetry(state) => state.retry.ready_at <= Instant::now(),
            _ => return,
        };

        if !ready {
            return;
        }

        let state = match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::SummarizationRetry(state) => state,
            other => {
                self.state = other;
                return;
            }
        };

        let attempt = state.retry.attempt;
        let had_pending = state.queued_request.is_some();
        let start_result = self.start_summarization_with_attempt(state.queued_request, attempt);

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
        match self.state {
            AppState::Streaming(_) => {
                self.set_status("Already streaming a response");
                return;
            }
            AppState::Summarizing(_) | AppState::SummarizationRetry(_) => {
                self.set_status("Busy: summarization in progress");
                return;
            }
            AppState::Idle => {}
        }

        let QueuedUserMessage { config } = queued;

        let api_messages = if self.context_infinity_enabled {
            match self.context_manager.get_api_messages() {
                Ok(messages) => messages,
                Err(needed) => {
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

        self.state = AppState::Streaming(active);

        let max_output_tokens = self.context_manager.current_limits().max_output();

        let task = async move {
            let tx_events = tx.clone();
            let result = crate::provider::send_message(
                &config,
                &api_messages,
                max_output_tokens,
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
        if !matches!(self.state, AppState::Streaming(_)) {
            return;
        }

        // Process all available events.
        loop {
            let event = {
                let active = match self.state {
                    AppState::Streaming(ref mut active) => active,
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

            let mut journal_error: Option<String> = None;
            let mut finished = false;

            {
                let active = match self.state {
                    AppState::Streaming(ref mut active) => active,
                    _ => return,
                };

                let delta = match &event {
                    StreamEvent::TextDelta(text) => active.journal.next_text(text.clone()),
                    StreamEvent::Done => active.journal.next_done(),
                    StreamEvent::Error(msg) => active.journal.next_error(msg.clone()),
                };

                // Persist BEFORE display.
                if let Err(e) = self.stream_journal.append_delta(delta) {
                    journal_error = Some(e.to_string());
                }

                if journal_error.is_none() {
                    finished = active.message.apply_event(event) == StreamDisposition::Finished;
                }
            }

            if let Some(err) = journal_error {
                // Abort streaming without applying the unpersisted event.
                let active = match std::mem::replace(&mut self.state, AppState::Idle) {
                    AppState::Streaming(active) => active,
                    other => {
                        self.state = other;
                        return;
                    }
                };

                active.abort_handle.abort();

                if let Err(seal_err) = self.stream_journal.seal() {
                    eprintln!("Journal seal failed after append error: {seal_err}");
                }

                let model = active.message.model_name().clone();
                let partial = active.message.content().to_string();
                let aborted = NonEmptyString::from_string_or(
                    format!("[Aborted - journal write failed]\n\n{partial}"),
                    "[Aborted - journal write failed]",
                );
                self.push_local_message(Message::assistant(model, aborted));
                self.set_status(format!("Journal append failed: {err}"));
                return;
            }

            if finished {
                self.finish_streaming();
                return;
            }
        }
    }

    /// Finish the current streaming session and commit the message.
    fn finish_streaming(&mut self) {
        let active = match std::mem::replace(&mut self.state, AppState::Idle) {
            AppState::Streaming(active) => active,
            other => {
                self.state = other;
                return;
            }
        };

        active.abort_handle.abort();

        // Seal the journal
        if let Err(e) = self.stream_journal.seal() {
            eprintln!("Journal seal failed: {e}");
        }

        // Capture metadata and finish reason before consuming the streaming message.
        let model = active.message.model_name().clone();
        let finish_reason = active.message.finish_reason().cloned();

        // Convert streaming message to completed message (empty content is invalid).
        let message = active.message.into_message().ok();

        match finish_reason {
            Some(StreamFinishReason::Error(err)) => {
                if let Some(message) = message {
                    self.push_history_message(message);
                }
                let system_msg = Message::system(NonEmptyString::from_string_or(
                    format!("[Stream error]\n\n{err}"),
                    "[Stream error]",
                ));
                self.push_local_message(system_msg);
                self.set_status(format!("Stream error: {err}"));
                return;
            }
            Some(StreamFinishReason::Done) | None => {}
        }

        let Some(message) = message else {
            let empty_msg = Message::assistant(
                model,
                NonEmptyString::from_static("[Empty response - API returned no content]"),
            );
            self.push_local_message(empty_msg);
            self.set_status("Warning: API returned empty response");
            return;
        };

        self.push_history_message(message);
    }

    /// Increment animation tick and poll background tasks.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.poll_summarization();
        self.poll_summarization_retry();
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

    pub(crate) fn insert_token(&self) -> Option<InsertToken> {
        matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
    }

    pub(crate) fn command_token(&self) -> Option<CommandToken> {
        matches!(&self.input, InputState::Command { .. }).then_some(CommandToken(()))
    }

    pub(crate) fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
        InsertMode { app: self }
    }

    pub(crate) fn command_mode(&mut self, _token: CommandToken) -> CommandMode<'_> {
        CommandMode { app: self }
    }

    pub(crate) fn enter_insert_mode_at_end(&mut self) {
        self.input.draft_mut().move_cursor_end();
        self.enter_insert_mode();
    }

    pub(crate) fn enter_insert_mode_with_clear(&mut self) {
        self.input.draft_mut().clear();
        self.enter_insert_mode();
    }

    pub fn enter_normal_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_normal();
    }

    pub fn enter_insert_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_insert();
    }

    pub fn enter_command_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_command();
    }

    pub fn draft_text(&self) -> &str {
        self.input.draft().text()
    }

    pub fn draft_cursor(&self) -> usize {
        self.input.draft().cursor()
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

    pub(crate) fn process_command(&mut self, command: EnteredCommand) {
        let parts: Vec<&str> = command.raw.split_whitespace().collect();

        match parts.first().copied() {
            Some("q" | "quit") => {
                self.request_quit();
            }
            Some("clear") => {
                match std::mem::replace(&mut self.state, AppState::Idle) {
                    AppState::Streaming(active) => {
                        active.abort_handle.abort();

                        let _ = self
                            .stream_journal
                            .discard_unsealed(active.journal.step_id());
                    }
                    AppState::Summarizing(state) => {
                        state.task.handle.abort();
                    }
                    AppState::SummarizationRetry(_) | AppState::Idle => {}
                }

                self.display.clear();
                self.context_manager = ContextManager::new(self.model.as_str());
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
                    let usage = self.context_usage();
                    self.set_status(format!(
                        "Model: {} │ Context: {}",
                        self.model,
                        usage.format_compact()
                    ));
                }
            }
            Some("provider" | "p") => {
                if let Some(provider_str) = parts.get(1) {
                    if let Some(provider) = Provider::from_str(provider_str) {
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
                let usage = self.context_usage();
                let limits = self.context_manager.current_limits();
                let limits_source = match self.context_manager.current_limits_source() {
                    ModelLimitsSource::Override => "override".to_string(),
                    ModelLimitsSource::Prefix(prefix) => prefix.to_string(),
                    ModelLimitsSource::DefaultFallback => "fallback(default)".to_string(),
                };
                let context_flag = if self.context_infinity_enabled {
                    "on"
                } else {
                    "off"
                };
                self.set_status(format!(
                    "ContextInfinity: {} │ Context: {} │ Model: {} │ Limits: {} │ Window: {}k │ Budget: {}k │ Max output: {}k",
                    context_flag,
                    usage.format_compact(),
                    self.context_manager.current_model(),
                    limits_source,
                    limits.context_window() / 1000,
                    limits.effective_input_budget() / 1000,
                    limits.max_output() / 1000,
                ));
            }
            Some("journal" | "jrnl") => match self.stream_journal.stats() {
                Ok(stats) => {
                    let state_desc = match self.stream_journal.state() {
                        crate::context_infinity::JournalState::Empty => "idle",
                        crate::context_infinity::JournalState::Streaming { .. } => "streaming",
                        crate::context_infinity::JournalState::Sealed { .. } => "sealed",
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
                if !self.context_infinity_enabled {
                    self.set_status("ContextInfinity disabled: summarization unavailable");
                } else if matches!(
                    self.state,
                    AppState::Summarizing(_) | AppState::SummarizationRetry(_)
                ) {
                    self.set_status("Summarization already in progress");
                } else if matches!(self.state, AppState::Streaming(_)) {
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
                match std::mem::replace(&mut self.state, AppState::Idle) {
                    AppState::Streaming(active) => {
                        active.abort_handle.abort();

                        // Clean up journal state
                        let _ = self
                            .stream_journal
                            .discard_unsealed(active.journal.step_id());
                        self.set_status("Streaming cancelled");
                    }
                    other => {
                        self.state = other;
                        self.set_status("No active stream to cancel");
                    }
                }
            }
            Some("help") => {
                self.set_status(
                    "Commands: :q(uit), :clear, :cancel, :model, :p(rovider), :ctx, :jrnl, :sum",
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
        match self.app.state {
            AppState::Streaming(_) => {
                self.app.set_status("Already streaming a response");
                return None;
            }
            AppState::Summarizing(_) | AppState::SummarizationRetry(_) => {
                self.app.set_status("Busy: summarization in progress");
                return None;
            }
            AppState::Idle => {}
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

        self.app.scroll_to_bottom();

        let api_key = match self.app.model.provider() {
            Provider::Claude => ApiKey::Claude(api_key),
            Provider::OpenAI => ApiKey::OpenAI(api_key),
        };

        let config = match ApiConfig::new(api_key, self.app.model.clone()) {
            Ok(config) => config,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::provider::StreamEvent;

    fn test_app() -> App {
        let mut api_keys = HashMap::new();
        api_keys.insert(Provider::Claude, "test".to_string());
        let model = Provider::Claude.default_model();
        let stream_journal = StreamJournal::open_in_memory().expect("in-memory journal for tests");

        App {
            input: InputState::default(),
            display: Vec::new(),
            scroll: ScrollState::AutoBottom,
            scroll_max: 0,
            should_quit: false,
            status_message: None,
            api_keys,
            model: model.clone(),
            tick: 0,
            context_infinity_enabled: true,
            context_manager: ContextManager::new(model.as_str()),
            stream_journal,
            state: AppState::Idle,
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
        app.state = AppState::Streaming(ActiveStream {
            message: streaming,
            journal,
            abort_handle,
        });
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

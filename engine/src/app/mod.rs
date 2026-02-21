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
//! Operations requiring proof of state use structural types and borrow-scoped guards:
//!
//! - [`InsertMode`]/[`CommandMode`]: Borrow-scoped access to mode-specific operations
//! - [`QueuedUserMessage`]: Proof that a message is validated and ready to send
//! - [`PreparedContext`]: Proof that context was built within token budget

use std::cmp::Ordering as CmpOrdering;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::mem;
use std::panic::Location;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{Mutex, mpsc};

use crate::ui::InputState;
use crate::ui::{
    ChangeKind, DetailView, DisplayItem, DisplayLog, DraftInput, FileScanState, FilesPanelState,
    InputMode, ModalEffect, ModalState, PanelEffect, PanelEffectKind, PanelState, PredefinedModel,
    ScrollState, SettingsCategory, SettingsFilterMode, SettingsSurface, ShowThinking,
    TranscriptRenderAction, UiOptions, ViewState,
};

use forge_context::{
    ContextAdaptation, ContextManager, ContextPreparation, ContextShrinkRequirement,
    ContextUsageStatus, FullHistory, Librarian, ModelLimitsSource, StreamJournal, TokenCounter,
    ToolJournal, distillation_model, generate_distillation,
};
use forge_lsp::{DiagnosticsSnapshot, LspManager};
use forge_providers::{self, ApiConfig, gemini::GeminiCache, gemini::GeminiCacheConfig};
use forge_types::ui::{AsciiOnly, HighContrast, ReducedMotion};
use forge_types::{
    ApiUsage, CacheableMessage, EmptyStringError, LspConfig, Message, MessageId, ModelName,
    NonEmptyString, OpenAIReasoningEffort, OpenAIReasoningItem, OpenAIRequestOptions, OutputLimits,
    PlanState, Provider, SecretString, StepId, StreamEvent, StreamFinishReason,
    ThinkingReplayState, ThinkingState, ThoughtSignature, ThoughtSignatureState, ToolBatchId,
    ToolCall, ToolDefinition, ToolResult, plan::ActiveStepQuery, sanitize_terminal_text,
};
use forge_utils::format_unified_diff;

use crate::tools;
use crate::{EnvironmentContext, SessionChangeLog};

// Module aliases so app submodules can keep using `super::state`, `super::ui`, etc.
use crate::config;
use crate::notifications;
use crate::notifications::{NotificationQueue, SystemNotification};
use crate::operation;
use crate::security;
use crate::state;
use crate::state::{ApprovalExpanded, ApprovalSelection, JournalCleanup};
use crate::ui;

pub(crate) mod checkpoints;
pub(crate) mod commands;
mod distillation;
pub(crate) mod init;
pub(crate) mod input_modes;
mod lsp_integration;
mod persistence;
mod plan;
pub(crate) mod streaming;
mod tool_gate;
pub(crate) mod tool_loop;

pub use commands::{CommandSpec, command_specs};
pub use input_modes::{
    CommandMode, CommandModeAccess, EnteredCommand, InsertMode, InsertModeAccess,
    QueueMessageResult, QueuedUserMessage,
};

pub(crate) use persistence::{ABORTED_JOURNAL_BADGE, EMPTY_RESPONSE_BADGE};

pub(crate) use init::{
    DEFAULT_TOOL_CAPACITY_BYTES, TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

pub(crate) use input_modes::TurnContext;
pub(crate) use tool_loop::{ActiveExecution, ToolQueue};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandInputAccess<'a> {
    Active {
        text: &'a str,
        cursor: usize,
        cursor_byte_index: usize,
    },
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSelectAccess {
    Active { selected_index: usize },
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSelectAccess<'a> {
    Active {
        filter: &'a str,
        selected_index: usize,
    },
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAccess<'a> {
    Active {
        surface: SettingsSurface,
        filter_text: &'a str,
        filter_mode: SettingsFilterMode,
        detail_view: DetailView,
        selected_index: usize,
    },
    Inactive,
}

pub enum StreamingAccess<'a> {
    Active(&'a StreamingMessage),
    Inactive,
}

pub enum ToolLoopExecution<'a> {
    Idle,
    Active { current_call_id: &'a str },
}

pub enum ToolLoopAccess<'a> {
    Active {
        calls: &'a [ToolCall],
        execute_calls: &'a [ToolCall],
        results: &'a [ToolResult],
        output_lines: &'a HashMap<String, Vec<String>>,
        execution: ToolLoopExecution<'a>,
    },
    Inactive,
}

pub enum ToolApprovalAccess<'a> {
    Selecting {
        requests: &'a [tools::ConfirmationRequest],
        selected: &'a [ApprovalSelection],
        cursor: usize,
        expanded: ApprovalExpanded,
        scroll_offset: usize,
    },
    ConfirmingDeny {
        requests: &'a [tools::ConfirmationRequest],
        selected: &'a [ApprovalSelection],
        cursor: usize,
        expanded: ApprovalExpanded,
        scroll_offset: usize,
    },
    Inactive,
}

pub enum PlanApprovalAccess {
    Active {
        kind: &'static str,
        rendered: String,
    },
    Inactive,
}

pub enum ToolRecoveryAccess<'a> {
    Active {
        calls: &'a [ToolCall],
        results: &'a [ToolResult],
    },
    Inactive,
}

enum OperationTake<T> {
    Taken(T),
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyState {
    Idle,
    StreamingResponse,
    ToolExecution,
    PlanApproval,
    ToolRecovery,
    RecoveryBlocked,
    Distillation,
}

impl BusyState {
    fn reason(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::StreamingResponse => "streaming a response",
            Self::ToolExecution => "tool execution in progress",
            Self::PlanApproval => "plan approval pending",
            Self::ToolRecovery => "tool recovery pending",
            Self::RecoveryBlocked => "recovery blocked",
            Self::Distillation => "distillation in progress",
        }
    }
}

enum ToolJournalGate<'a> {
    Enabled,
    Disabled { reason: &'a str },
}

enum RecoveryGate {
    Clear,
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnConfig {
    pub(crate) model: ModelName,
    pub(crate) tool_approval_mode: tools::ApprovalMode,
    pub(crate) context_memory_enabled: bool,
    pub(crate) ui_options: UiOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnConfigStaging {
    pub(crate) active: TurnConfig,
    pub(crate) staged: TurnConfig,
}

#[derive(Debug, Clone)]
pub(crate) enum LibrarianState {
    Disabled,
    Enabled(Arc<Mutex<Librarian>>),
}

impl LibrarianState {
    fn to_tool_handle(&self) -> tools::ToolCtxLibrarian {
        match self {
            Self::Enabled(arc) => tools::ToolCtxLibrarian::Enabled(arc.clone()),
            Self::Disabled => tools::ToolCtxLibrarian::Disabled,
        }
    }
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

/// API usage from the last completed turn (replaces `Option<TurnUsage>`).
#[derive(Debug, Clone, Copy, Default)]
pub enum CompletedTurnUsage {
    #[default]
    NoTurnCompleted,
    Available(TurnUsage),
}

#[derive(Debug, Clone)]
pub(crate) struct TurnOrigin {
    pub(crate) message_id: MessageId,
    pub(crate) original_draft: String,
    pub(crate) consumed_agents_md: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) enum TurnRollback {
    #[default]
    Committed,
    Pending(TurnOrigin),
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum ActiveTurnUsage {
    #[default]
    Inactive,
    Accumulating(TurnUsage),
}

impl ActiveTurnUsage {
    /// Accumulate usage from an API call. Auto-initializes on first call.
    fn record_call(&mut self, usage: ApiUsage) {
        match self {
            Self::Inactive => {
                let mut turn = TurnUsage::default();
                turn.record_call(usage);
                *self = Self::Accumulating(turn);
            }
            Self::Accumulating(acc) => acc.record_call(usage),
        }
    }

    /// Transfer into completed form. Resets self to Inactive.
    fn finalize(&mut self) -> CompletedTurnUsage {
        match mem::take(self) {
            Self::Inactive => CompletedTurnUsage::NoTurnCompleted,
            Self::Accumulating(u) => CompletedTurnUsage::Available(u),
        }
    }

    /// Reset without producing a value (abort/cancel paths).
    fn reset(&mut self) {
        *self = Self::Inactive;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub active_profile: String,
    pub session_config_hash: String,
    pub mode: String,
    pub active_model: String,
    pub provider: Provider,
    pub provider_status: String,
    pub context_used_tokens: u32,
    pub context_budget_tokens: u32,
    pub distill_threshold_tokens: u32,
    pub auto_attached: Vec<String>,
    pub rate_limit_state: String,
    pub last_api_call: String,
    pub last_error: Option<String>,
    pub session_overrides: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveLayerValue {
    pub layer: &'static str,
    pub value: String,
    pub is_winner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveSetting {
    pub setting: &'static str,
    pub layers: Vec<ResolveLayerValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveCascade {
    pub settings: Vec<ResolveSetting>,
    pub session_config_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationFinding {
    pub title: String,
    pub detail: String,
    pub fix_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub errors: Vec<ValidationFinding>,
    pub warnings: Vec<ValidationFinding>,
    pub healthy: Vec<ValidationFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppearanceEditorSnapshot {
    pub draft: UiOptions,
    pub selected: usize,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEditorSnapshot {
    pub draft: ModelName,
    pub selected: usize,
    pub dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextEditorSnapshot {
    pub draft_memory_enabled: bool,
    pub selected: usize,
    pub dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolsEditorSnapshot {
    pub draft_approval_mode: &'static str,
    pub selected: usize,
    pub dirty: bool,
}

use crate::state::{
    ActiveStream, DataDir, DistillationStart, DistillationState, DistillationTask, OperationEdge,
    OperationState, OperationTag, ToolLoopPhase, ToolRecoveryDecision,
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
pub(crate) struct ParsedToolCalls {
    calls: Vec<ToolCall>,
    pre_resolved: Vec<ToolResult>,
}

pub(crate) mod settings;
pub(crate) use settings::*;

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
    /// `OpenAI` response ID for `previous_response_id` chaining (Pro models).
    openai_response_id: Option<String>,
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
            openai_response_id: None,
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

    #[must_use]
    pub fn openai_response_id(&self) -> Option<&str> {
        self.openai_response_id.as_deref()
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
            StreamEvent::ResponseId(id) => {
                self.openai_response_id = Some(id);
                None
            }
            StreamEvent::ThinkingSignature(signature_delta) => {
                match &mut self.thinking_replay {
                    ThinkingReplayState::Unsigned | ThinkingReplayState::Unknown => {
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
                summary,
                encrypted_content,
            } => {
                if let Ok(item) = OpenAIReasoningItem::try_new(id, summary, encrypted_content) {
                    match &mut self.thinking_replay {
                        ThinkingReplayState::OpenAIReasoning { items } => items.push(item),
                        _ => {
                            self.thinking_replay =
                                ThinkingReplayState::OpenAIReasoning { items: vec![item] };
                        }
                    }
                } else {
                    tracing::warn!("Skipping invalid OpenAI reasoning replay item");
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

    pub fn into_message(self) -> Result<Message, EmptyStringError> {
        // Persisted assistant content is untrusted external text; sanitize before it can
        // reach history/context storage to prevent terminal injection, invisible prompt
        // injection, and secret leaks.
        let sanitized = security::sanitize_display_text(&self.content);
        let content = NonEmptyString::new(sanitized)?;
        Ok(Message::assistant(self.model, content, SystemTime::now()))
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

#[derive(Clone)]
pub(crate) struct ProviderRuntimeState {
    /// `OpenAI` request defaults (reasoning/summary/verbosity/truncation).
    openai_options: OpenAIRequestOptions,
    openai_reasoning_effort_explicit: bool,
    /// Active Gemini cache (if caching enabled and cache created).
    /// Uses `Arc<Mutex>` because cache is created/updated inside async streaming tasks.
    gemini_cache: Arc<Mutex<Option<GeminiCache>>>,
    /// Whether Gemini thinking mode is enabled via config.
    gemini_thinking_enabled: bool,
    anthropic_thinking_mode: config::AnthropicThinkingMode,
    /// Anthropic effort level for Opus 4.6+ ("low", "medium", "high", "max").
    anthropic_thinking_effort: config::AnthropicEffort,
    /// Gemini cache configuration.
    gemini_cache_config: GeminiCacheConfig,
    /// Last `OpenAI` response ID for Pro model `previous_response_id` chaining.
    openai_previous_response_id: Option<String>,
}

struct LspRuntimeState {
    /// LSP config, consumed on first start. `None` once started or if LSP is disabled.
    config: Option<LspConfig>,
    /// LSP client manager. Populated lazily on first tool batch via `LspManager::start()`.
    /// `Arc<Mutex<Option<>>>` so the spawned startup task can populate it.
    manager: Arc<Mutex<Option<LspManager>>>,
    /// Cached diagnostics snapshot for UI display and agent feedback.
    snapshot: DiagnosticsSnapshot,
    /// Pending diagnostics check: edited files + deadline for deferred error injection.
    pending_diag_check: Option<(Vec<PathBuf>, Instant)>,
}

struct AppUi {
    input: InputState,
    display: DisplayLog,
    should_quit: bool,
    /// View state for rendering (scroll, status, modal effects).
    view: ViewState,
    /// Mutually-exclusive settings detail editor state.
    settings_editor: SettingsEditorState,
    /// Input history for prompt and command recall.
    input_history: ui::InputHistory,
    /// Wall-clock timestamp for animation tick cadence (spinner ~10Hz).
    last_ui_tick: Instant,
    /// File picker state for "@" reference feature.
    file_picker: ui::FilePickerState,
}

struct AppCore {
    turn_config: TurnConfigStaging,
    model: ModelName,
    context_manager: ContextManager,
    state: OperationState,
    /// Whether memory (automatic distillation) is enabled.
    /// This is determined at init from config/env and does not change during runtime.
    memory_enabled: bool,
    /// Validated output limits (max tokens + explicit thinking state).
    /// Invariant: if thinking is enabled, budget < `max_tokens`.
    /// May be clamped below `configured_output_limits` for smaller models.
    output_limits: OutputLimits,
    /// Output limits as configured at init time (before model-specific clamping).
    /// Used to restore thinking budget when switching back to a larger model.
    configured_output_limits: OutputLimits,
    /// Whether prompt caching is enabled (for Claude).
    cache_enabled: bool,
    /// Provider-specific system prompts.
    /// The correct prompt is selected at streaming time based on the active provider.
    system_prompts: SystemPrompts,
    /// Runtime environment context gathered at startup.
    /// Used to inject date, platform, cwd, and git status into system prompts.
    environment: EnvironmentContext,
    /// Turn rollback state: tracks whether the current user message can be undone.
    ///
    /// When `Pending`, the user message can be rolled back from history on stream
    /// failure. When `Committed`, the message is finalized and cannot be undone.
    turn_rollback: TurnRollback,
    /// Tool definitions to send with each request.
    tool_definitions: Vec<ToolDefinition>,
    /// Tool names hidden from UI rendering (source-of-truth: `ToolExecutor::is_hidden()`).
    hidden_tools: HashSet<String>,
    /// Session-scoped tool execution gate.
    ///
    /// When disabled, tool execution is fail-closed due to tool journal health errors.
    tool_gate: ToolGate,
    /// Checkpoints for rewind (per-turn conversation checkpoints + tool-edit snapshots).
    checkpoints: checkpoints::CheckpointStore,
    /// Tool iterations used in the current user turn.
    tool_iterations: u32,
    /// Session-wide log of files created and modified.
    session_changes: SessionChangeLog,
    /// API usage for the current user turn (reset when turn completes).
    turn_usage: ActiveTurnUsage,
    /// API usage from the last completed turn (for status bar display).
    last_turn_usage: CompletedTurnUsage,
    /// Queue of pending system notifications to inject into next API request.
    notification_queue: NotificationQueue,
    /// Plan lifecycle state (Inactive / Proposed / Active).
    plan_state: PlanState,
}

struct AppRuntime {
    api_keys: HashMap<Provider, SecretString>,
    /// Path to the config file for persist operations.
    config_path: PathBuf,
    tick: usize,
    data_dir: DataDir,
    stream_journal: StreamJournal,
    provider_runtime: ProviderRuntimeState,
    /// Tool registry for executors.
    tool_registry: Arc<tools::ToolRegistry>,
    /// Tool settings derived from config.
    tool_settings: tools::ToolSettings,
    /// Tool journal for crash recovery.
    tool_journal: ToolJournal,
    /// Stream journal step that needs commit+prune cleanup.
    ///
    /// Best-effort retries run during `tick()` when idle. If this remains uncleared,
    /// `StreamJournal::begin_session()` will refuse to start new streams.
    stream_cleanup: JournalCleanup<StepId>,
    /// Tool journal batch that needs pruning cleanup.
    ///
    /// Best-effort retries run during `tick()` when idle. If this remains uncleared,
    /// `ToolJournal::begin_*batch()` will refuse to start new batches.
    tool_cleanup: JournalCleanup<ToolBatchId>,
    /// File hash cache for tool safety checks.
    tool_file_cache: Arc<Mutex<tools::ToolFileCache>>,
    /// Whether we've already warned about a failed history load.
    history_load_warning_shown: bool,
    /// Whether we've already warned about autosave failures.
    autosave_warning_shown: bool,
    /// The Librarian for fact extraction and retrieval (Long-term Memory).
    /// Uses `Arc<Mutex>` because it's accessed from async tasks for extraction.
    librarian: LibrarianState,
    /// Wall-clock timestamp for session autosave cadence (~3s).
    last_session_autosave: Instant,
    /// Earliest time to attempt journal cleanup retries.
    next_journal_cleanup_attempt: Instant,
    lsp_runtime: LspRuntimeState,
}

pub struct App {
    ui: AppUi,
    core: AppCore,
    runtime: AppRuntime,
}

use tool_gate::ToolGate;

impl App {
    pub fn should_quit(&self) -> bool {
        self.ui.should_quit
    }

    pub fn request_quit(&mut self) {
        self.ui.should_quit = true;
    }

    pub fn take_transcript_render_action(&mut self) -> TranscriptRenderAction {
        mem::replace(
            &mut self.ui.view.transcript_action,
            TranscriptRenderAction::Preserve,
        )
    }

    pub fn ui_options(&self) -> UiOptions {
        self.ui.view.ui_options
    }

    #[must_use]
    pub fn settings_usable_model_count(&self) -> usize {
        PredefinedModel::all()
            .iter()
            .filter(|model| self.has_api_key(model.provider()))
            .count()
    }

    #[must_use]
    pub fn settings_configured_model(&self) -> &ModelName {
        &self.core.turn_config.staged.model
    }

    #[must_use]
    pub fn settings_configured_tool_approval_mode(&self) -> tools::ApprovalMode {
        self.core.turn_config.staged.tool_approval_mode
    }

    #[must_use]
    pub fn settings_configured_tool_approval_mode_label(&self) -> &'static str {
        approval_mode_display(self.core.turn_config.staged.tool_approval_mode)
    }

    #[must_use]
    pub fn settings_configured_context_memory_enabled(&self) -> bool {
        self.core.turn_config.staged.context_memory_enabled
    }

    #[must_use]
    pub fn settings_configured_ui_options(&self) -> UiOptions {
        self.core.turn_config.staged.ui_options
    }

    #[must_use]
    pub fn settings_staged_model(&self) -> &ModelName {
        &self.core.turn_config.staged.model
    }

    #[must_use]
    pub fn settings_staged_tool_approval_mode(&self) -> tools::ApprovalMode {
        self.core.turn_config.staged.tool_approval_mode
    }

    #[must_use]
    pub fn settings_staged_context_memory_enabled(&self) -> bool {
        self.core.turn_config.staged.context_memory_enabled
    }

    #[must_use]
    pub fn settings_staged_ui_options(&self) -> UiOptions {
        self.core.turn_config.staged.ui_options
    }

    #[must_use]
    pub fn settings_pending_model_apply_next_turn(&self) -> bool {
        self.core.turn_config.staged.model != self.core.turn_config.active.model
    }

    #[must_use]
    pub fn settings_pending_tools_apply_next_turn(&self) -> bool {
        self.core.turn_config.staged.tool_approval_mode
            != self.core.turn_config.active.tool_approval_mode
    }

    #[must_use]
    pub fn settings_pending_ui_apply_next_turn(&self) -> bool {
        self.core.turn_config.staged.ui_options != self.core.turn_config.active.ui_options
    }

    #[must_use]
    pub fn settings_pending_context_apply_next_turn(&self) -> bool {
        self.core.turn_config.staged.context_memory_enabled
            != self.core.turn_config.active.context_memory_enabled
    }

    #[must_use]
    pub fn settings_pending_apply_next_turn(&self) -> bool {
        self.core.turn_config.staged != self.core.turn_config.active
    }

    #[must_use]
    pub fn settings_model_editor_snapshot(&self) -> Option<ModelEditorSnapshot> {
        match &self.ui.settings_editor {
            SettingsEditorState::Model(editor) => Some(ModelEditorSnapshot {
                draft: editor.draft().clone(),
                selected: editor.selected(),
                dirty: editor.is_unsaved(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn settings_context_editor_snapshot(&self) -> Option<ContextEditorSnapshot> {
        match &self.ui.settings_editor {
            SettingsEditorState::Context(editor) => Some(ContextEditorSnapshot {
                draft_memory_enabled: editor.draft().as_bool(),
                selected: editor.selected(),
                dirty: editor.is_unsaved(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn settings_tools_editor_snapshot(&self) -> Option<ToolsEditorSnapshot> {
        match &self.ui.settings_editor {
            SettingsEditorState::Tools(editor) => Some(ToolsEditorSnapshot {
                draft_approval_mode: approval_mode_display(*editor.draft()),
                selected: editor.selected(),
                dirty: editor.is_unsaved(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn settings_appearance_editor_snapshot(&self) -> Option<AppearanceEditorSnapshot> {
        match &self.ui.settings_editor {
            SettingsEditorState::Appearance(editor) => Some(AppearanceEditorSnapshot {
                draft: *editor.draft(),
                selected: editor.selected(),
                dirty: editor.is_unsaved(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn settings_has_unsaved_edits(&self) -> bool {
        match &self.ui.settings_editor {
            SettingsEditorState::Model(e) => e.is_unsaved(),
            SettingsEditorState::Tools(e) => e.is_unsaved(),
            SettingsEditorState::Context(e) => e.is_unsaved(),
            SettingsEditorState::Appearance(e) => e.is_unsaved(),
            SettingsEditorState::Inactive => false,
        }
    }

    pub(crate) fn apply_pending_turn_settings(&mut self) {
        let staged = self.core.turn_config.staged.clone();
        self.runtime.tool_settings.policy.mode = staged.tool_approval_mode;
        self.set_context_memory_enabled_internal(staged.context_memory_enabled, false);
        self.set_active_model(staged.model.clone());
        self.ui.view.ui_options = staged.ui_options;
        if matches!(staged.ui_options.reduced_motion, ReducedMotion::Enabled) {
            self.ui.view.modal_effect = ModalState::Inactive;
            self.ui.view.files_panel_effect = PanelState::Inactive;
        }
        self.core.turn_config.active = staged;
    }

    /// Toggle visibility of thinking/reasoning content in the UI.
    pub fn toggle_thinking(&mut self) {
        self.ui.view.ui_options.show_thinking =
            if matches!(self.ui.view.ui_options.show_thinking, ShowThinking::Enabled) {
                ShowThinking::Disabled
            } else {
                ShowThinking::Enabled
            };
    }

    /// Toggle visibility of the files panel.
    pub fn toggle_files_panel(&mut self) {
        let is_visible = matches!(self.ui.view.files_panel, FilesPanelState::Visible(_));
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            self.ui.view.files_panel_effect = PanelState::Inactive;
            if is_visible {
                self.ui.view.files_panel = FilesPanelState::Hidden;
            } else {
                self.ui.view.files_panel = FilesPanelState::Visible(ui::ActiveFilesPanel {
                    selected: 0,
                    expansion: ui::DiffExpansion::Collapsed,
                    diff_scroll: 0,
                });
                self.files_panel_sync_selection();
            }
            return;
        }

        if is_visible {
            self.ui.view.files_panel_effect =
                PanelState::Active(PanelEffect::slide_out_right(Duration::from_millis(180)));
            self.ui.view.last_frame = Instant::now();
        } else {
            self.ui.view.files_panel = FilesPanelState::Visible(ui::ActiveFilesPanel {
                selected: 0,
                expansion: ui::DiffExpansion::Collapsed,
                diff_scroll: 0,
            });
            self.files_panel_sync_selection();
            self.ui.view.files_panel_effect =
                PanelState::Active(PanelEffect::slide_in_right(Duration::from_millis(180)));
            self.ui.view.last_frame = Instant::now();
        }
    }

    /// Close the files panel (no-op if already hidden).
    pub fn close_files_panel(&mut self) {
        if matches!(self.ui.view.files_panel, FilesPanelState::Hidden) {
            return;
        }
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            self.ui.view.files_panel_effect = PanelState::Inactive;
            self.ui.view.files_panel = FilesPanelState::Hidden;
            return;
        }
        self.ui.view.files_panel_effect =
            PanelState::Active(PanelEffect::slide_out_right(Duration::from_millis(180)));
        self.ui.view.last_frame = Instant::now();
    }

    pub fn files_panel_visible(&self) -> bool {
        matches!(self.ui.view.files_panel, FilesPanelState::Visible(_))
    }

    pub fn files_panel_effect_mut(&mut self) -> Option<&mut PanelEffect> {
        match &mut self.ui.view.files_panel_effect {
            PanelState::Active(e) => Some(e),
            PanelState::Inactive => None,
        }
    }

    pub fn clear_files_panel_effect(&mut self) {
        self.ui.view.files_panel_effect = PanelState::Inactive;
    }

    pub fn finish_files_panel_effect(&mut self) {
        if let PanelState::Active(effect) = &self.ui.view.files_panel_effect
            && effect.kind() == PanelEffectKind::SlideOutRight
        {
            self.ui.view.files_panel = FilesPanelState::Hidden;
        }
        self.ui.view.files_panel_effect = PanelState::Inactive;
    }

    pub fn session_changes(&self) -> &SessionChangeLog {
        &self.core.session_changes
    }

    /// Filters out files that no longer exist on disk.
    pub fn ordered_files(&self) -> Vec<(PathBuf, ChangeKind)> {
        let changes = &self.core.session_changes;
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
        let files = self.ordered_files();
        if let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel {
            active.selected = (active.selected + 1) % count;
            active.diff_scroll = 0;
            if matches!(active.expansion, ui::DiffExpansion::Expanded(_)) {
                if let Some((path, _)) = files.get(active.selected) {
                    active.expansion = ui::DiffExpansion::Expanded(path.clone());
                } else {
                    active.expansion = ui::DiffExpansion::Collapsed;
                }
            }
        }
    }

    /// Cycle to the previous file in the panel (wrapping).
    pub fn files_panel_prev(&mut self) {
        let count = self.files_panel_count();
        if count == 0 {
            return;
        }
        let files = self.ordered_files();
        if let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel {
            active.selected = if active.selected == 0 {
                count - 1
            } else {
                active.selected - 1
            };
            active.diff_scroll = 0;
            if matches!(active.expansion, ui::DiffExpansion::Expanded(_)) {
                if let Some((path, _)) = files.get(active.selected) {
                    active.expansion = ui::DiffExpansion::Expanded(path.clone());
                } else {
                    active.expansion = ui::DiffExpansion::Collapsed;
                }
            }
        }
    }

    pub fn files_panel_expanded(&self) -> bool {
        match &self.ui.view.files_panel {
            FilesPanelState::Visible(active) => {
                matches!(active.expansion, ui::DiffExpansion::Expanded(_))
            }
            FilesPanelState::Hidden => false,
        }
    }

    pub fn files_panel_state(&self) -> &FilesPanelState {
        &self.ui.view.files_panel
    }

    /// Collapse the expanded diff.
    pub fn files_panel_collapse(&mut self) {
        if let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel {
            active.expansion = ui::DiffExpansion::Collapsed;
            active.diff_scroll = 0;
        }
    }

    pub fn files_panel_sync_selection(&mut self) {
        let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel else {
            return;
        };
        let files = self
            .core
            .session_changes
            .modified
            .iter()
            .cloned()
            .chain(self.core.session_changes.created.iter().cloned())
            .filter(|p| p.exists())
            .collect::<Vec<_>>();

        if let ui::DiffExpansion::Expanded(expanded_path) = &active.expansion {
            if let Some(new_idx) = files.iter().position(|p| p == expanded_path) {
                active.selected = new_idx;
            } else {
                active.expansion = ui::DiffExpansion::Collapsed;
                active.diff_scroll = 0;
                active.selected = active.selected.min(files.len().saturating_sub(1));
            }
        } else {
            active.selected = active.selected.min(files.len().saturating_sub(1));
        }
    }

    /// Scroll the diff view down.
    pub fn files_panel_scroll_diff_down(&mut self) {
        if let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel {
            active.diff_scroll += 10;
        }
    }

    pub fn files_panel_scroll_diff_up(&mut self) {
        if let FilesPanelState::Visible(ref mut active) = self.ui.view.files_panel {
            active.diff_scroll = active.diff_scroll.saturating_sub(10);
        }
    }

    pub fn files_panel_diff(&self) -> Option<FileDiff> {
        let path = match &self.ui.view.files_panel {
            FilesPanelState::Visible(active) => match &active.expansion {
                ui::DiffExpansion::Expanded(p) => p,
                ui::DiffExpansion::Collapsed => return None,
            },
            FilesPanelState::Hidden => return None,
        };

        let current = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Some(FileDiff::Deleted);
            }
            Err(e) => return Some(FileDiff::Error(e.to_string())),
        };

        // Check for binary content (contains null bytes in first 8KB)
        if current.iter().take(8192).any(|&b| b == 0) {
            return Some(FileDiff::Binary(current.len()));
        }

        let baseline = self.core.checkpoints.find_baseline_for_file(path);
        match baseline {
            checkpoints::FileBaselineLookup::Found(proof) => {
                match self.core.checkpoints.baseline_content(proof, path) {
                    checkpoints::BaselineContentLookup::Existed(old_bytes) => {
                        let diff =
                            format_unified_diff(&path.to_string_lossy(), old_bytes, &current, true);
                        Some(FileDiff::Diff(diff))
                    }
                    checkpoints::BaselineContentLookup::MissingAtCheckpoint
                    | checkpoints::BaselineContentLookup::MissingBaseline => {
                        let content = String::from_utf8_lossy(&current);
                        let lines: Vec<_> = content.lines().map(|l| format!("+{l}")).collect();
                        Some(FileDiff::Created(lines.join("\n")))
                    }
                }
            }
            checkpoints::FileBaselineLookup::Missing => {
                let content = String::from_utf8_lossy(&current);
                let lines: Vec<_> = content.lines().map(|l| format!("+{l}")).collect();
                Some(FileDiff::Created(lines.join("\n")))
            }
        }
    }

    pub fn provider(&self) -> Provider {
        self.core.model.provider()
    }

    pub fn model(&self) -> &str {
        self.core.model.as_str()
    }

    pub fn model_display_name(&self) -> &'static str {
        self.core.model.short_display_name()
    }

    pub(crate) fn openai_options_for_model(&self, model: &ModelName) -> OpenAIRequestOptions {
        if model.provider() != Provider::OpenAI {
            return self.runtime.provider_runtime.openai_options;
        }

        if model.as_str() == "gpt-5.2-pro"
            && !self
                .runtime
                .provider_runtime
                .openai_reasoning_effort_explicit
            && self
                .runtime
                .provider_runtime
                .openai_options
                .reasoning_effort()
                != OpenAIReasoningEffort::XHigh
        {
            return OpenAIRequestOptions::new(
                OpenAIReasoningEffort::XHigh,
                self.runtime
                    .provider_runtime
                    .openai_options
                    .reasoning_summary(),
                self.runtime.provider_runtime.openai_options.verbosity(),
                self.runtime.provider_runtime.openai_options.truncation(),
            );
        }

        self.runtime.provider_runtime.openai_options
    }

    pub fn tick_count(&self) -> usize {
        self.runtime.tick
    }

    pub fn history(&self) -> &FullHistory {
        self.core.context_manager.history()
    }

    #[inline]
    fn operation_state(&self) -> &OperationState {
        &self.core.state
    }

    pub fn streaming(&self) -> Option<&StreamingMessage> {
        match self.operation_state() {
            OperationState::Streaming(active) => Some(active.message()),
            _ => None,
        }
    }

    pub fn streaming_access(&self) -> StreamingAccess<'_> {
        match self.operation_state() {
            OperationState::Streaming(active) => StreamingAccess::Active(active.message()),
            _ => StreamingAccess::Inactive,
        }
    }

    // Tool loop state helpers (private, inline)

    #[inline]
    fn tool_loop_state(&self) -> Option<&state::ToolLoopState> {
        match self.operation_state() {
            OperationState::ToolLoop(state) => Some(state.as_ref()),
            _ => None,
        }
    }

    #[inline]
    fn tool_loop_state_mut(&mut self) -> Option<&mut state::ToolLoopState> {
        match &mut self.core.state {
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

    #[inline]
    fn plan_approval_state(&self) -> Option<&state::PlanApprovalState> {
        match self.operation_state() {
            OperationState::PlanApproval(state) => Some(state),
            _ => None,
        }
    }

    #[inline]
    fn tool_recovery_state(&self) -> Option<&state::ToolRecoveryState> {
        match self.operation_state() {
            OperationState::ToolRecovery(state) => Some(state),
            _ => None,
        }
    }

    #[inline]
    fn recovery_blocked_state(&self) -> Option<&state::RecoveryBlockedState> {
        match self.operation_state() {
            OperationState::RecoveryBlocked(state) => Some(state),
            _ => None,
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

    pub fn tool_approval_selected(&self) -> Option<&[ApprovalSelection]> {
        Some(&self.tool_approval_ref()?.data().selected)
    }

    pub fn tool_approval_cursor(&self) -> Option<usize> {
        Some(self.tool_approval_ref()?.data().cursor)
    }

    pub fn tool_approval_expanded(&self) -> ApprovalExpanded {
        self.tool_approval_ref()
            .map(|s| s.data().expanded)
            .unwrap_or(ApprovalExpanded::Collapsed)
    }

    pub fn tool_approval_scroll_offset(&self) -> usize {
        self.tool_approval_ref()
            .map_or(0, |s| s.data().scroll_offset)
    }

    pub fn tool_approval_deny_confirm(&self) -> bool {
        self.tool_approval_ref()
            .is_some_and(state::ApprovalState::is_confirming_deny)
    }

    pub fn plan_state(&self) -> &PlanState {
        &self.core.plan_state
    }

    // Plan approval accessors

    pub fn plan_approval_kind(&self) -> Option<&'static str> {
        match &self.plan_approval_state()?.kind {
            state::PlanApprovalKind::Create => Some("create"),
            state::PlanApprovalKind::Edit { .. } => Some("edit"),
        }
    }

    pub fn plan_approval_rendered(&self) -> Option<String> {
        self.plan_approval_state()?;
        match &self.core.plan_state {
            PlanState::Inactive => None,
            PlanState::Proposed(plan) | PlanState::Active(plan) => Some(plan.render()),
        }
    }

    pub fn plan_status_line(&self) -> Option<String> {
        match &self.core.plan_state {
            PlanState::Active(plan) => {
                if let ActiveStepQuery::Active(active) = plan.active_step() {
                    let phase_name = plan.phases()[active.phase_index()].name();
                    Some(format!(
                        "Plan: {phase_name} — {}",
                        active.step().description()
                    ))
                } else {
                    Some("Plan: Active (no current step)".to_string())
                }
            }
            _ => None,
        }
    }

    pub fn plan_approval_approve(&mut self) {
        self.resolve_plan_approval(operation::PlanApprovalDecision::Approve);
    }

    pub fn plan_approval_reject(&mut self) {
        self.resolve_plan_approval(operation::PlanApprovalDecision::Reject);
    }

    pub fn tool_recovery_calls(&self) -> Option<&[ToolCall]> {
        Some(&self.tool_recovery_state()?.batch.calls)
    }

    pub fn tool_recovery_results(&self) -> Option<&[ToolResult]> {
        Some(&self.tool_recovery_state()?.batch.results)
    }

    pub fn tool_loop_access(&self) -> ToolLoopAccess<'_> {
        static EMPTY_OUTPUT: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(HashMap::new);
        let Some(tl) = self.tool_loop_state() else {
            return ToolLoopAccess::Inactive;
        };
        let execution = match &tl.phase {
            ToolLoopPhase::Executing(exec) => ToolLoopExecution::Active {
                current_call_id: exec.spawned.call().id.as_str(),
            },
            _ => ToolLoopExecution::Idle,
        };
        ToolLoopAccess::Active {
            calls: &tl.batch.calls,
            execute_calls: &tl.batch.execute_now,
            results: &tl.batch.results,
            output_lines: match &tl.phase {
                ToolLoopPhase::Executing(exec) => &exec.output_lines,
                _ => &EMPTY_OUTPUT,
            },
            execution,
        }
    }

    pub fn tool_approval_access(&self) -> ToolApprovalAccess<'_> {
        let Some(approval) = self.tool_approval_ref() else {
            return ToolApprovalAccess::Inactive;
        };
        let data = approval.data();
        match approval {
            state::ApprovalState::Selecting(_) => ToolApprovalAccess::Selecting {
                requests: &data.requests,
                selected: &data.selected,
                cursor: data.cursor,
                expanded: data.expanded,
                scroll_offset: data.scroll_offset,
            },
            state::ApprovalState::ConfirmingDeny(_) => ToolApprovalAccess::ConfirmingDeny {
                requests: &data.requests,
                selected: &data.selected,
                cursor: data.cursor,
                expanded: data.expanded,
                scroll_offset: data.scroll_offset,
            },
        }
    }

    pub fn plan_approval_access(&self) -> PlanApprovalAccess {
        let Some(pa) = self.plan_approval_state() else {
            return PlanApprovalAccess::Inactive;
        };
        let kind = match &pa.kind {
            state::PlanApprovalKind::Create => "create",
            state::PlanApprovalKind::Edit { .. } => "edit",
        };
        let rendered = match &self.core.plan_state {
            PlanState::Proposed(plan) | PlanState::Active(plan) => plan.render(),
            PlanState::Inactive => String::new(),
        };
        PlanApprovalAccess::Active { kind, rendered }
    }

    pub fn tool_recovery_access(&self) -> ToolRecoveryAccess<'_> {
        let Some(tr) = self.tool_recovery_state() else {
            return ToolRecoveryAccess::Inactive;
        };
        ToolRecoveryAccess::Active {
            calls: &tr.batch.calls,
            results: &tr.batch.results,
        }
    }

    pub fn is_empty(&self) -> bool {
        // Check if there are no History items (real conversation content)
        // Local items (notifications) don't count towards "empty"
        !self
            .ui
            .display
            .iter()
            .any(|item| matches!(item, DisplayItem::History(_)))
            && !matches!(
                self.core.state,
                OperationState::Streaming(_)
                    | OperationState::ToolLoop(_)
                    | OperationState::PlanApproval(_)
                    | OperationState::ToolRecovery(_)
                    | OperationState::RecoveryBlocked(_)
            )
    }

    pub fn display_items(&self) -> &[DisplayItem] {
        self.ui.display.items()
    }

    pub fn is_tool_hidden(&self, name: &str) -> bool {
        self.core.hidden_tools.contains(name)
    }

    /// Version counter for display changes - used for render caching.
    pub fn display_version(&self) -> usize {
        self.ui.display.revision()
    }

    pub fn has_api_key(&self, provider: Provider) -> bool {
        self.runtime.api_keys.contains_key(&provider)
    }

    pub fn current_api_key(&self) -> Option<&SecretString> {
        self.runtime.api_keys.get(&self.core.model.provider())
    }

    pub fn is_loading(&self) -> bool {
        match self.busy_state() {
            BusyState::Idle => false,
            BusyState::StreamingResponse
            | BusyState::ToolExecution
            | BusyState::PlanApproval
            | BusyState::ToolRecovery
            | BusyState::RecoveryBlocked
            | BusyState::Distillation => true,
        }
    }

    fn push_settings_next_turn_guardrail(&mut self) {
        match self.busy_state() {
            BusyState::Idle => {}
            busy => {
                let reason = busy.reason();
                self.push_notification(format!(
                    "Settings edits apply on the next turn. Active turn remains unchanged while {reason}."
                ));
            }
        }
    }

    fn push_settings_unsaved_edits_notification(&mut self) {
        self.push_notification(
            "Unsaved settings changes. Press s to save or r to revert before leaving.",
        );
    }

    pub fn settings_open_resolve_surface(&mut self) {
        if self.settings_has_unsaved_edits() {
            self.push_settings_unsaved_edits_notification();
            return;
        }
        self.enter_resolve_mode();
    }

    fn settings_category_for_resolve_setting(setting: &str) -> Option<SettingsCategory> {
        match setting {
            "Model" => Some(SettingsCategory::Models),
            "Context Limit" | "Context Memory" => Some(SettingsCategory::Context),
            "Tool Approval Mode" => Some(SettingsCategory::Tools),
            "UI Defaults" => Some(SettingsCategory::Appearance),
            _ => None,
        }
    }

    pub fn settings_resolve_move_up(&mut self) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Resolve,
                ..
            }
        ) {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access()
            && modal.selected > 0
        {
            modal.selected -= 1;
        }
    }

    pub fn settings_resolve_move_down(&mut self) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Resolve,
                ..
            }
        ) {
            return;
        }
        let len = self.resolve_cascade().settings.len();
        if len == 0 {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access()
            && modal.selected + 1 < len
        {
            modal.selected += 1;
        }
    }

    pub fn settings_resolve_activate_selected(&mut self) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Resolve,
                ..
            }
        ) {
            return;
        }
        if self.settings_has_unsaved_edits() {
            self.push_settings_unsaved_edits_notification();
            return;
        }
        let selected = match self.settings_access() {
            SettingsAccess::Active { selected_index, .. } => selected_index,
            SettingsAccess::Inactive => return,
        };
        let Some(setting) = self
            .resolve_cascade()
            .settings
            .get(selected)
            .map(|setting| setting.setting)
        else {
            return;
        };
        let Some(category) = Self::settings_category_for_resolve_setting(setting) else {
            return;
        };
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.surface = SettingsSurface::Root;
            modal.filter = DraftInput::default();
            modal.filter_mode = SettingsFilterMode::Browsing;
            modal.detail_view = DetailView::Hidden;
            if let Some(index) = SettingsCategory::ALL
                .iter()
                .position(|candidate| *candidate == category)
            {
                modal.selected = index;
            }
        }
        self.open_settings_detail(category);
    }

    /// Tool journal safety-gate status.
    fn tool_journal_gate(&self) -> ToolJournalGate<'_> {
        match self.core.tool_gate.status() {
            tool_gate::ToolGateStatus::Enabled => ToolJournalGate::Enabled,
            tool_gate::ToolGateStatus::Disabled { reason } => ToolJournalGate::Disabled { reason },
        }
    }

    /// Recovery-gate status.
    fn recovery_gate(&self) -> RecoveryGate {
        match self.recovery_blocked_state() {
            Some(state) => RecoveryGate::Blocked {
                reason: state.reason.message(),
            },
            None => RecoveryGate::Clear,
        }
    }

    /// Centralizes busy-state checks to ensure consistency across
    /// `start_streaming`, `start_distillation`, and UI queries.
    fn busy_state(&self) -> BusyState {
        match self.operation_state() {
            OperationState::Idle => BusyState::Idle,
            OperationState::Streaming(_) => BusyState::StreamingResponse,
            OperationState::ToolLoop(_) => BusyState::ToolExecution,
            OperationState::PlanApproval(_) => BusyState::PlanApproval,
            OperationState::ToolRecovery(_) => BusyState::ToolRecovery,
            OperationState::RecoveryBlocked(_) => BusyState::RecoveryBlocked,
            OperationState::Distilling(_) => BusyState::Distillation,
        }
    }

    pub fn context_usage_status(&mut self) -> ContextUsageStatus {
        self.core.context_manager.usage_status()
    }

    pub fn memory_enabled(&self) -> bool {
        self.core.memory_enabled
    }

    /// Queue a system notification to be injected into the next API request.
    ///
    /// Notifications are injected as assistant messages, which cannot be forged
    /// by user input, providing a secure channel for system-level communication.
    pub fn queue_notification(&mut self, notification: SystemNotification) {
        self.core.notification_queue.push(notification);
    }

    /// API usage from the last completed turn (for status bar display).
    pub fn last_turn_usage(&self) -> &CompletedTurnUsage {
        &self.core.last_turn_usage
    }
    #[allow(clippy::unused_self)]
    fn idle_state(&self) -> OperationState {
        OperationState::Idle
    }

    fn op_take(&mut self) -> OperationState {
        let idle = self.idle_state();
        mem::replace(&mut self.core.state, idle)
    }

    fn op_take_streaming(&mut self) -> OperationTake<ActiveStream> {
        match self.op_take() {
            OperationState::Streaming(active) => OperationTake::Taken(active),
            other => {
                self.op_restore(other);
                OperationTake::Skipped
            }
        }
    }

    fn op_take_distilling(&mut self) -> OperationTake<DistillationState> {
        match self.op_take() {
            OperationState::Distilling(state) => OperationTake::Taken(state),
            other => {
                self.op_restore(other);
                OperationTake::Skipped
            }
        }
    }

    fn op_take_plan_approval(&mut self) -> OperationTake<Box<state::PlanApprovalState>> {
        match self.op_take() {
            OperationState::PlanApproval(state) => OperationTake::Taken(state),
            other => {
                self.op_restore(other);
                OperationTake::Skipped
            }
        }
    }

    fn op_restore_streaming(&mut self, active: ActiveStream) {
        self.op_restore(OperationState::Streaming(active));
    }

    fn op_restore_distilling(&mut self, state: DistillationState) {
        self.op_restore(OperationState::Distilling(state));
    }

    /// Emit an operation edge without changing `OperationState`.
    ///
    /// Used for lifecycle edges that should remain centrally observable even when Rust move/borrow
    /// rules force us into "take + restore" implementation patterns.
    #[track_caller]
    fn op_edge(&mut self, edge: OperationEdge) {
        let state = self.core.state.tag();
        let loc = Location::caller();
        let legal = Self::op_is_legal_transition(state, edge, state);
        if !legal {
            tracing::warn!(
                state = ?state,
                edge = edge.as_str(),
                file = loc.file(),
                line = loc.line(),
                column = loc.column(),
                "Illegal Operation edge",
            );
            debug_assert!(
                legal,
                "Illegal Operation edge: {state:?} --{edge:?}--> {state:?} at {}:{}:{}",
                loc.file(),
                loc.line(),
                loc.column()
            );
        }

        tracing::debug!(
            state = ?state,
            edge = edge.as_str(),
            file = loc.file(),
            line = loc.line(),
            column = loc.column(),
            "Operation edge",
        );

        self.op_apply_edge_effects(state, edge, state);
    }

    #[cfg(test)]
    fn op_transition_edge(from: OperationTag, to: OperationTag) -> Option<OperationEdge> {
        operation::transition_edge(from, to)
    }

    fn op_transition_receipt(
        from: OperationTag,
        to: OperationTag,
    ) -> Option<operation::TransitionReceipt> {
        operation::transition_receipt(from, to)
    }

    fn op_is_legal_transition(from: OperationTag, edge: OperationEdge, to: OperationTag) -> bool {
        operation::is_legal_transition(from, edge, to)
    }

    /// Authoritative `OperationState` transition point.
    ///
    /// Phase-0 of the OperationState transition overhaul:
    /// - stop scattering `self.core.state = ...` across the codebase,
    /// - log variant-level edges once,
    /// - provide a single hook for future cross-cutting effects (metrics, UI sync, etc).
    #[track_caller]
    fn op_transition(&mut self, next: OperationState) {
        let from = self.core.state.tag();
        let to = next.tag();
        let receipt = Self::op_transition_receipt(from, to);
        if let Some(receipt) = receipt {
            let loc = Location::caller();
            let legal = operation::receipt_is_legal(receipt);
            if !legal {
                tracing::warn!(
                    from = ?from,
                    to = ?to,
                    edge = receipt.edge().as_str(),
                    file = loc.file(),
                    line = loc.line(),
                    column = loc.column(),
                    "Illegal OperationState transition",
                );
                debug_assert!(
                    legal,
                    "Illegal OperationState transition: {from:?} --{:?}--> {to:?} at {}:{}:{}",
                    receipt.edge(),
                    loc.file(),
                    loc.line(),
                    loc.column()
                );
            }
        }
        if from != to {
            let loc = Location::caller();
            tracing::debug!(
                from = ?from,
                to = ?to,
                file = loc.file(),
                line = loc.line(),
                column = loc.column(),
                "OperationState transition",
            );
        }
        if let Some(receipt) = receipt {
            self.op_apply_edge_effects(receipt.from(), receipt.edge(), receipt.to());
        }
        self.core.state = next;
    }

    /// Like [`Self::op_transition`], but forces the `from` tag.
    ///
    /// Useful when a method temporarily takes `self.core.state` (via `mem::replace`) and
    /// needs to emit a semantically correct edge that would otherwise read as `Idle -> X`.
    #[track_caller]
    fn op_transition_from(&mut self, from: OperationTag, next: OperationState) {
        let to = next.tag();
        let receipt = Self::op_transition_receipt(from, to);
        if let Some(receipt) = receipt {
            let loc = Location::caller();
            let legal = operation::receipt_is_legal(receipt);
            if !legal {
                tracing::warn!(
                    from = ?from,
                    to = ?to,
                    edge = receipt.edge().as_str(),
                    file = loc.file(),
                    line = loc.line(),
                    column = loc.column(),
                    "Illegal OperationState transition",
                );
                debug_assert!(
                    legal,
                    "Illegal OperationState transition: {from:?} --{:?}--> {to:?} at {}:{}:{}",
                    receipt.edge(),
                    loc.file(),
                    loc.line(),
                    loc.column()
                );
            }
        }
        if from != to {
            let loc = Location::caller();
            tracing::debug!(
                from = ?from,
                to = ?to,
                file = loc.file(),
                line = loc.line(),
                column = loc.column(),
                "OperationState transition",
            );
        }
        if let Some(receipt) = receipt {
            self.op_apply_edge_effects(receipt.from(), receipt.edge(), receipt.to());
        }
        self.core.state = next;
    }

    /// Internal state write used for "take + restore" patterns.
    ///
    /// This intentionally does not emit a transition edge, because callers use
    /// temporary `Idle` slots to satisfy Rust move/borrow rules.
    /// Logging those internal hops would drown out real lifecycle edges.
    fn op_restore(&mut self, next: OperationState) {
        self.core.state = next;
    }

    #[allow(clippy::unused_self)]
    fn op_apply_edge_effects(
        &mut self,
        _from: OperationTag,
        _edge: OperationEdge,
        _to: OperationTag,
    ) {
    }

    fn build_basic_api_messages(&mut self, reserved_overhead: u32) -> Vec<Message> {
        let budget = self
            .core
            .context_manager
            .current_limits()
            .effective_input_budget()
            .saturating_sub(reserved_overhead);
        let entries = self.core.context_manager.history().entries();
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

    fn reconcile_output_limits_with_model(&mut self) {
        let model_max_output = self.core.context_manager.current_limits().max_output();
        let current = self.core.output_limits;

        // Try to restore configured limits if the new model has enough headroom.
        // This handles the case where thinking was dropped for a smaller model
        // and needs to be restored when switching back to a larger one.
        let target = if current.max_output_tokens() <= model_max_output {
            let configured = self.core.configured_output_limits;
            match configured.thinking() {
                ThinkingState::Enabled(budget) if !current.has_thinking() => {
                    let budget_tokens = budget.as_u32();
                    if budget_tokens < model_max_output {
                        let restored = OutputLimits::with_thinking(model_max_output, budget_tokens)
                            .unwrap_or(OutputLimits::new(model_max_output));
                        tracing::info!(
                            "Restored thinking budget ({budget_tokens}) for {}",
                            self.core.model
                        );
                        restored
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        } else {
            // Model is smaller — clamp down.
            match current.thinking() {
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
            }
        };

        if target != current {
            if current.has_thinking() && !target.has_thinking() {
                tracing::warn!(
                    "Clamped max_output_tokens {} → {} for {}; disabled thinking budget",
                    current.max_output_tokens(),
                    target.max_output_tokens(),
                    self.core.model
                );
            } else if current.max_output_tokens() != target.max_output_tokens() {
                tracing::warn!(
                    "Adjusted max_output_tokens {} → {} for {}",
                    current.max_output_tokens(),
                    target.max_output_tokens(),
                    self.core.model
                );
            }

            self.core.output_limits = target;
            self.core
                .context_manager
                .set_output_limit(target.max_output_tokens());
        }
    }

    fn set_active_model(&mut self, model: ModelName) {
        self.core.model = model;
        if self.memory_enabled() {
            self.handle_context_adaptation();
        } else {
            self.core
                .context_manager
                .set_model_without_adaptation(self.core.model.clone());
        }
        self.reconcile_output_limits_with_model();
    }

    fn set_model_internal(&mut self, model: ModelName, persist: bool) {
        self.runtime.provider_runtime.openai_previous_response_id = None;
        self.set_active_model(model.clone());
        self.core.turn_config.active.model = model.clone();
        self.core.turn_config.staged.model = model.clone();
        if persist
            && let Err(e) =
                config::ForgeConfig::persist_model(&self.runtime.config_path, model.as_str())
        {
            tracing::warn!("Failed to persist model to config: {e}");
        }
    }

    /// Persists the model to `~/.forge/config.toml` for future sessions.
    pub fn set_model(&mut self, model: ModelName) {
        self.set_model_internal(model, true);
    }

    fn set_context_memory_enabled_internal(&mut self, enabled: bool, persist: bool) {
        self.core.memory_enabled = enabled;
        self.core.context_manager.invalidate_usage_cache();
        if persist
            && let Err(err) = config::ForgeConfig::persist_context_settings(
                &self.runtime.config_path,
                config::ContextSettings { memory: enabled },
            )
        {
            tracing::warn!("Failed to persist context settings: {err}");
        }
    }

    /// Handle context adaptation after a model switch.
    ///
    /// This method is called after `set_model()` to handle the context adaptation result:
    /// - If shrinking with `needs_distillation`, starts background distillation
    /// - If expanding, attempts to restore previously distilled messages
    fn handle_context_adaptation(&mut self) {
        let adaptation = self
            .core
            .context_manager
            .switch_model(self.core.model.clone());

        match adaptation {
            ContextAdaptation::NoChange
            | ContextAdaptation::Shrinking {
                requirement: ContextShrinkRequirement::Ready,
                ..
            } => {}
            ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                requirement: ContextShrinkRequirement::NeedsDistillation,
            } => {
                self.push_notification(format!(
                    "Context budget shrank {}k → {}k; compacting...",
                    old_budget / 1000,
                    new_budget / 1000
                ));
                self.start_distillation();
            }
            ContextAdaptation::Expanding {
                old_budget,
                new_budget,
            } => {
                self.push_notification(format!(
                    "Context budget expanded {}k → {}k",
                    old_budget / 1000,
                    new_budget / 1000
                ));
            }
        }
    }

    pub fn tick(&mut self) {
        self.poll_distillation();
        self.poll_tool_loop();
        self.poll_lsp_events();
        self.poll_journal_cleanup();

        let now = Instant::now();

        // Preserve prior spinner cadence (~10Hz), independent of render FPS.
        if now.duration_since(self.ui.last_ui_tick) >= Duration::from_millis(100) {
            self.ui.last_ui_tick = now;
            self.runtime.tick = self.runtime.tick.wrapping_add(1);
        }

        // Preserve prior autosave cadence (~3s), independent of render FPS.
        if now.duration_since(self.runtime.last_session_autosave) >= Duration::from_secs(3) {
            self.runtime.last_session_autosave = now;
            self.autosave_session();
        }
    }

    pub fn frame_elapsed(&mut self) -> Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.ui.view.last_frame);
        self.ui.view.last_frame = now;
        elapsed
    }

    pub fn modal_effect_mut(&mut self) -> Option<&mut ModalEffect> {
        match &mut self.ui.view.modal_effect {
            ModalState::Active(e) => Some(e),
            ModalState::Inactive => None,
        }
    }

    pub fn clear_modal_effect(&mut self) {
        self.ui.view.modal_effect = ModalState::Inactive;
    }

    pub fn input_mode(&self) -> InputMode {
        self.ui.input.mode()
    }

    pub fn enter_insert_mode_at_end(&mut self) {
        self.ui.input.draft_mut().move_cursor_end();
        self.enter_insert_mode();
    }

    pub fn enter_insert_mode_with_clear(&mut self) {
        self.ui.input.draft_mut().clear();
        self.enter_insert_mode();
    }

    pub fn enter_normal_mode(&mut self) {
        self.ui.input = mem::take(&mut self.ui.input).into_normal();
        self.reset_settings_detail_editor();
        self.ui.view.modal_effect = ModalState::Inactive;
    }

    pub fn enter_insert_mode(&mut self) {
        self.ui.input = mem::take(&mut self.ui.input).into_insert();
    }

    pub fn enter_command_mode(&mut self) {
        self.ui.input = mem::take(&mut self.ui.input).into_command();
    }

    fn enter_settings_surface(&mut self, surface: SettingsSurface) {
        let current = mem::take(&mut self.ui.input);
        self.ui.input = if surface == SettingsSurface::Root {
            current.into_settings()
        } else {
            current.into_settings_surface(surface)
        };
        self.reset_settings_detail_editor();
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            self.ui.view.modal_effect = ModalState::Inactive;
        } else {
            self.ui.view.modal_effect =
                ModalState::Active(ModalEffect::pop_scale(Duration::from_millis(700)));
            self.ui.view.last_frame = Instant::now();
        }
    }

    pub fn enter_settings_mode(&mut self) {
        self.enter_settings_surface(SettingsSurface::Root);
    }

    pub fn enter_runtime_mode(&mut self) {
        self.enter_settings_surface(SettingsSurface::Runtime);
    }

    pub fn enter_resolve_mode(&mut self) {
        self.enter_settings_surface(SettingsSurface::Resolve);
    }

    pub fn enter_validate_mode(&mut self) {
        self.enter_settings_surface(SettingsSurface::Validate);
    }

    #[must_use]
    pub fn settings_access(&self) -> SettingsAccess<'_> {
        match self.ui.input.settings_modal_ref() {
            ui::SettingsModalRef::Active(modal) => SettingsAccess::Active {
                surface: modal.surface,
                filter_text: modal.filter.text(),
                filter_mode: modal.filter_mode,
                detail_view: modal.detail_view,
                selected_index: modal.selected,
            },
            ui::SettingsModalRef::Inactive => SettingsAccess::Inactive,
        }
    }

    #[must_use]
    pub fn settings_categories(&self) -> Vec<SettingsCategory> {
        match self.settings_access() {
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                filter_text,
                ..
            } => SettingsCategory::filtered(filter_text),
            SettingsAccess::Active { .. } | SettingsAccess::Inactive => Vec::new(),
        }
    }

    fn open_settings_detail(&mut self, category: SettingsCategory) {
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.detail_view = DetailView::Visible(category);
        }
        self.ui.settings_editor = SettingsEditorState::Inactive;
        match category {
            SettingsCategory::Models => {
                self.ui.settings_editor = SettingsEditorState::Model(EditorState::from_model(
                    self.core.turn_config.staged.model.clone(),
                ));
            }
            SettingsCategory::Tools => {
                self.ui.settings_editor = SettingsEditorState::Tools(EditorState::new(
                    self.core.turn_config.staged.tool_approval_mode,
                    0,
                ));
            }
            SettingsCategory::Context => {
                self.ui.settings_editor = SettingsEditorState::Context(EditorState::new(
                    MemoryState::from_bool(self.core.turn_config.staged.context_memory_enabled),
                    0,
                ));
            }
            SettingsCategory::Appearance => {
                self.ui.settings_editor = SettingsEditorState::Appearance(EditorState::new(
                    self.core.turn_config.staged.ui_options,
                    0,
                ));
            }
            _ => {}
        }
    }

    fn reset_settings_detail_editor(&mut self) {
        self.ui.settings_editor = SettingsEditorState::Inactive;
    }

    #[must_use]
    pub fn tool_definition_count(&self) -> usize {
        self.core.tool_definitions.len()
    }

    pub fn settings_move_up(&mut self) {
        let selected = match self.settings_access() {
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                selected_index,
                ..
            } => selected_index,
            SettingsAccess::Active { .. } | SettingsAccess::Inactive => return,
        };
        if selected == 0 {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.selected -= 1;
        }
    }

    pub fn settings_move_down(&mut self) {
        let selected = match self.settings_access() {
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                selected_index,
                ..
            } => selected_index,
            SettingsAccess::Inactive | SettingsAccess::Active { .. } => return,
        };
        let len = self.settings_categories().len();
        if len == 0 || selected + 1 >= len {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.selected += 1;
        }
    }

    pub fn settings_start_filter(&mut self) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                ..
            }
        ) {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.filter_mode = SettingsFilterMode::Filtering;
        }
    }

    pub fn settings_stop_filter(&mut self) {
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.filter_mode = SettingsFilterMode::Browsing;
        }
    }

    pub fn settings_filter_push_char(&mut self, c: char) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                ..
            }
        ) {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.filter.enter_char(c);
        }
        self.settings_clamp_selection();
    }

    pub fn settings_filter_backspace(&mut self) {
        if !matches!(
            self.settings_access(),
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                ..
            }
        ) {
            return;
        }
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            modal.filter.delete_char();
        }
        self.settings_clamp_selection();
    }

    pub fn settings_detail_move_up(&mut self) {
        match &mut self.ui.settings_editor {
            SettingsEditorState::Model(editor) if editor.selected() > 0 => {
                let new = editor.selected() - 1;
                editor.set_selected(new);
            }
            SettingsEditorState::Tools(editor) if editor.selected() > 0 => {
                let new = editor.selected() - 1;
                editor.set_selected(new);
            }
            SettingsEditorState::Context(editor) if editor.selected() > 0 => {
                let new = editor.selected() - 1;
                editor.set_selected(new);
            }
            SettingsEditorState::Appearance(editor) if editor.selected() > 0 => {
                let new = editor.selected() - 1;
                editor.set_selected(new);
            }
            _ => {}
        }
    }

    pub fn settings_detail_move_down(&mut self) {
        match &mut self.ui.settings_editor {
            SettingsEditorState::Model(editor) => {
                let max_index = EditorState::<ModelName>::max_model_index();
                if editor.selected() < max_index {
                    let new = editor.selected() + 1;
                    editor.set_selected(new);
                }
            }
            SettingsEditorState::Tools(editor) => {
                if editor.selected() + 1 < TOOLS_SETTINGS_COUNT {
                    let new = editor.selected() + 1;
                    editor.set_selected(new);
                }
            }
            SettingsEditorState::Context(editor) => {
                if editor.selected() + 1 < CONTEXT_SETTINGS_COUNT {
                    let new = editor.selected() + 1;
                    editor.set_selected(new);
                }
            }
            SettingsEditorState::Appearance(editor) => {
                if editor.selected() + 1 < APPEARANCE_SETTINGS_COUNT {
                    let new = editor.selected() + 1;
                    editor.set_selected(new);
                }
            }
            SettingsEditorState::Inactive => {}
        }
    }

    pub fn settings_detail_toggle_selected(&mut self) {
        match &mut self.ui.settings_editor {
            SettingsEditorState::Model(editor) => {
                editor.update_draft_from_selected();
            }
            SettingsEditorState::Tools(editor) => {
                editor.cycle_selected();
            }
            SettingsEditorState::Context(editor) => {
                editor.cycle_selected();
            }
            SettingsEditorState::Appearance(editor) => {
                editor.toggle_selected();
            }
            SettingsEditorState::Inactive => {}
        }
    }

    pub fn settings_revert_edits(&mut self) {
        let defaults = self.core.turn_config.staged.ui_options;
        match &mut self.ui.settings_editor {
            SettingsEditorState::Model(editor) => {
                let baseline = editor.baseline().clone();
                *editor = EditorState::new(baseline.clone(), 0);
                editor.sync_selected_to_draft();
            }
            SettingsEditorState::Tools(editor) => {
                let baseline = *editor.baseline();
                *editor = EditorState::new(baseline, editor.selected());
            }
            SettingsEditorState::Context(editor) => {
                let baseline = *editor.baseline();
                *editor = EditorState::new(baseline, editor.selected());
            }
            SettingsEditorState::Appearance(editor) => {
                let selected = editor.selected();
                *editor = EditorState::with_draft(*editor.baseline(), defaults, selected);
            }
            SettingsEditorState::Inactive => {}
        }
    }

    pub fn settings_save_edits(&mut self) {
        if let SettingsEditorState::Model(editor) = &self.ui.settings_editor {
            if !editor.is_unsaved() {
                self.push_notification("No settings changes to save.");
                return;
            }
            let draft = editor.draft().clone();
            let draft_provider = draft.provider();
            if let Err(err) =
                config::ForgeConfig::persist_model(&self.runtime.config_path, draft.as_str())
            {
                tracing::warn!("Failed to persist model setting: {err}");
                self.push_notification(format!("Failed to save settings: {err}"));
                return;
            }
            self.core.turn_config.staged.model = draft.clone();
            if let SettingsEditorState::Model(editor) = &mut self.ui.settings_editor {
                *editor = EditorState::new(draft, editor.selected());
                editor.sync_selected_to_draft();
            }
            self.push_notification("Model saved. Changes apply on the next turn.");
            if !self.has_api_key(draft_provider) {
                self.push_notification(format!(
                    "{} API key is missing. Set {} before the next turn.",
                    draft_provider.display_name(),
                    draft_provider.env_var()
                ));
            }
            self.push_settings_next_turn_guardrail();
            return;
        }

        if let SettingsEditorState::Tools(editor) = &self.ui.settings_editor {
            if !editor.is_unsaved() {
                self.push_notification("No settings changes to save.");
                return;
            }
            let draft_approval_mode = *editor.draft();
            let settings = config::ToolApprovalSettings {
                mode: approval_mode_config_value(draft_approval_mode).to_string(),
            };
            if let Err(err) = config::ForgeConfig::persist_tool_approval_settings(
                &self.runtime.config_path,
                &settings,
            ) {
                tracing::warn!("Failed to persist tool approval setting: {err}");
                self.push_notification(format!("Failed to save settings: {err}"));
                return;
            }
            self.core.turn_config.staged.tool_approval_mode = draft_approval_mode;
            if let SettingsEditorState::Tools(editor) = &mut self.ui.settings_editor {
                *editor = EditorState::new(draft_approval_mode, editor.selected());
            }
            self.push_notification("Tool defaults saved. Changes apply on the next turn.");
            self.push_settings_next_turn_guardrail();
            return;
        }

        if let SettingsEditorState::Context(editor) = &self.ui.settings_editor {
            if !editor.is_unsaved() {
                self.push_notification("No settings changes to save.");
                return;
            }
            let draft = editor.draft().as_bool();
            if let Err(err) = config::ForgeConfig::persist_context_settings(
                &self.runtime.config_path,
                config::ContextSettings { memory: draft },
            ) {
                tracing::warn!("Failed to persist context setting: {err}");
                self.push_notification(format!("Failed to save settings: {err}"));
                return;
            }
            self.core.turn_config.staged.context_memory_enabled = draft;
            if let SettingsEditorState::Context(editor) = &mut self.ui.settings_editor {
                *editor = EditorState::new(MemoryState::from_bool(draft), editor.selected());
            }
            self.push_notification("Context defaults saved. Changes apply on the next turn.");
            self.push_settings_next_turn_guardrail();
            return;
        }

        let SettingsEditorState::Appearance(editor) = &self.ui.settings_editor else {
            return;
        };
        if !editor.is_unsaved() {
            self.push_notification("No settings changes to save.");
            return;
        }

        let draft = *editor.draft();
        let persist = config::AppUiSettings {
            ascii_only: matches!(draft.ascii_only, AsciiOnly::Enabled),
            high_contrast: matches!(draft.high_contrast, HighContrast::Enabled),
            reduced_motion: matches!(draft.reduced_motion, ReducedMotion::Enabled),
            show_thinking: matches!(draft.show_thinking, ShowThinking::Enabled),
        };
        if let Err(err) =
            config::ForgeConfig::persist_ui_settings(&self.runtime.config_path, persist)
        {
            tracing::warn!("Failed to persist UI settings: {err}");
            self.push_notification(format!("Failed to save settings: {err}"));
            return;
        }

        self.core.turn_config.staged.ui_options = draft;
        if let SettingsEditorState::Appearance(editor) = &mut self.ui.settings_editor {
            *editor = EditorState::new(draft, editor.selected());
        }
        self.push_notification("Settings saved. Changes apply on the next turn.");
        self.push_settings_next_turn_guardrail();
    }

    pub fn settings_activate(&mut self) {
        let (filter_mode, detail_view, selected) = match self.settings_access() {
            SettingsAccess::Active {
                surface: SettingsSurface::Root,
                filter_mode,
                detail_view,
                selected_index,
                ..
            } => (filter_mode, detail_view, selected_index),
            SettingsAccess::Active { .. } | SettingsAccess::Inactive => return,
        };

        if matches!(filter_mode, SettingsFilterMode::Filtering) {
            self.settings_stop_filter();
            return;
        }

        if matches!(detail_view, DetailView::Visible(_)) {
            return;
        }

        let categories = self.settings_categories();
        let Some(category) = categories.get(selected).copied() else {
            return;
        };

        self.open_settings_detail(category);
    }

    pub fn settings_close_or_exit(&mut self) {
        let (filter_mode, detail_view) = match self.settings_access() {
            SettingsAccess::Active {
                filter_mode,
                detail_view,
                ..
            } => (filter_mode, detail_view),
            SettingsAccess::Inactive => return,
        };

        if matches!(filter_mode, SettingsFilterMode::Filtering) {
            self.settings_stop_filter();
            return;
        }

        if matches!(detail_view, DetailView::Visible(_)) {
            if self.settings_has_unsaved_edits() {
                self.push_settings_unsaved_edits_notification();
                return;
            }
            if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
                modal.detail_view = DetailView::Hidden;
            }
            self.reset_settings_detail_editor();
            return;
        }

        self.enter_normal_mode();
    }

    fn settings_clamp_selection(&mut self) {
        let len = self.settings_categories().len();
        if let ui::SettingsModalMut::Active(modal) = self.ui.input.settings_modal_mut_access() {
            if len == 0 {
                modal.selected = 0;
            } else if modal.selected >= len {
                modal.selected = len - 1;
            }
        }
    }

    #[must_use]
    pub fn session_config_hash(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.core.model.as_str().hash(&mut hasher);
        self.core
            .turn_config
            .active
            .model
            .as_str()
            .hash(&mut hasher);
        self.provider().as_str().hash(&mut hasher);
        self.core.memory_enabled.hash(&mut hasher);
        self.core.cache_enabled.hash(&mut hasher);
        self.core
            .output_limits
            .max_output_tokens()
            .hash(&mut hasher);
        match self.core.output_limits.thinking() {
            ThinkingState::Disabled => {
                0u32.hash(&mut hasher);
            }
            ThinkingState::Enabled(budget) => {
                budget.as_u32().hash(&mut hasher);
            }
        }
        self.runtime
            .provider_runtime
            .openai_options
            .reasoning_effort()
            .as_str()
            .hash(&mut hasher);
        self.runtime
            .provider_runtime
            .openai_options
            .reasoning_summary()
            .as_str()
            .hash(&mut hasher);
        self.runtime
            .provider_runtime
            .openai_options
            .verbosity()
            .as_str()
            .hash(&mut hasher);
        self.runtime
            .provider_runtime
            .openai_options
            .truncation()
            .as_str()
            .hash(&mut hasher);
        for provider in Provider::all() {
            provider.as_str().hash(&mut hasher);
            self.has_api_key(*provider).hash(&mut hasher);
        }
        self.runtime
            .tool_settings
            .limits
            .max_tool_calls_per_batch
            .hash(&mut hasher);
        self.runtime
            .tool_settings
            .limits
            .max_tool_iterations_per_user_turn
            .hash(&mut hasher);
        self.runtime
            .tool_settings
            .limits
            .max_tool_args_bytes
            .hash(&mut hasher);
        self.runtime
            .tool_settings
            .max_output_bytes
            .hash(&mut hasher);
        self.runtime
            .tool_settings
            .policy
            .allowlist
            .len()
            .hash(&mut hasher);
        self.runtime
            .tool_settings
            .policy
            .denylist
            .len()
            .hash(&mut hasher);
        match self.runtime.tool_settings.policy.mode {
            tools::ApprovalMode::Permissive => "permissive",
            tools::ApprovalMode::Default => "default",
            tools::ApprovalMode::Strict => "strict",
        }
        .hash(&mut hasher);
        let hash = hasher.finish();
        format!("{hash:07x}")
    }

    pub fn runtime_snapshot(&mut self) -> RuntimeSnapshot {
        let usage_status = self.context_usage_status();
        let usage = match usage_status {
            ContextUsageStatus::Ready(usage)
            | ContextUsageStatus::NeedsDistillation { usage, .. }
            | ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => usage,
        };
        let distill_threshold_tokens = usage.budget_tokens().saturating_mul(8) / 10;
        let provider = self.provider();
        let provider_status = if self.has_api_key(provider) {
            "configured".to_string()
        } else {
            "missing_api_key".to_string()
        };
        let mode = if matches!(self.input_mode(), InputMode::Insert) {
            "chat".to_string()
        } else {
            "code".to_string()
        };
        let last_error = match self.tool_journal_gate() {
            ToolJournalGate::Disabled { reason } => Some(reason.to_string()),
            ToolJournalGate::Enabled => match self.recovery_gate() {
                RecoveryGate::Blocked { reason } => Some(reason),
                RecoveryGate::Clear => None,
            },
        };
        let rate_limit_state = if self.is_loading() {
            "busy".to_string()
        } else {
            "healthy".to_string()
        };
        let last_api_call = match self.last_turn_usage() {
            CompletedTurnUsage::Available(_) => "recent_success".to_string(),
            CompletedTurnUsage::NoTurnCompleted if self.is_loading() => "in_progress".to_string(),
            CompletedTurnUsage::NoTurnCompleted => "none".to_string(),
        };
        let mut session_overrides = Vec::new();
        if self.settings_pending_model_apply_next_turn() {
            session_overrides.push("pending model change: next turn".to_string());
        }
        if self.settings_configured_tool_approval_mode() != tools::ApprovalMode::Default {
            session_overrides.push(format!(
                "tool approval mode: {}",
                approval_mode_display(self.settings_configured_tool_approval_mode())
            ));
        }
        if self.settings_pending_tools_apply_next_turn() {
            session_overrides.push("pending tool defaults: next turn".to_string());
        }
        if !self.settings_configured_context_memory_enabled() {
            session_overrides.push("context memory default: off".to_string());
        }
        if self.settings_pending_context_apply_next_turn() {
            session_overrides.push(format!(
                "pending context memory: {} (next turn)",
                on_off(self.core.turn_config.staged.context_memory_enabled)
            ));
        }
        if self.settings_configured_ui_options() != UiOptions::default() {
            session_overrides.push(format!(
                "ui defaults: {}",
                ui_options_display(self.settings_configured_ui_options())
            ));
        }
        if self.settings_pending_ui_apply_next_turn() {
            session_overrides.push(format!(
                "pending ui defaults: {} (next turn)",
                ui_options_display(self.core.turn_config.staged.ui_options)
            ));
        }

        RuntimeSnapshot {
            active_profile: "default".to_string(),
            session_config_hash: self.session_config_hash(),
            mode,
            active_model: self.model().to_string(),
            provider,
            provider_status,
            context_used_tokens: usage.used_tokens(),
            context_budget_tokens: usage.budget_tokens(),
            distill_threshold_tokens,
            auto_attached: vec!["AGENTS.md".to_string()],
            rate_limit_state,
            last_api_call,
            last_error,
            session_overrides,
        }
    }

    #[must_use]
    pub fn resolve_cascade(&self) -> ResolveCascade {
        let mut settings = Vec::new();
        let model_session_is_winner = self.settings_pending_model_apply_next_turn();
        let model_session_value = if model_session_is_winner {
            self.core.turn_config.staged.model.to_string()
        } else {
            "unset".to_string()
        };
        settings.push(ResolveSetting {
            setting: "Model",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: self.core.turn_config.active.model.to_string(),
                    is_winner: !model_session_is_winner,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: model_session_value,
                    is_winner: model_session_is_winner,
                },
            ],
        });

        settings.push(ResolveSetting {
            setting: "Temperature",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: "provider-default".to_string(),
                    is_winner: true,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: "unset".to_string(),
                    is_winner: false,
                },
            ],
        });

        let context_limit = self
            .core
            .context_manager
            .current_limits()
            .effective_input_budget();
        settings.push(ResolveSetting {
            setting: "Context Limit",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: context_limit.to_string(),
                    is_winner: true,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: "unset".to_string(),
                    is_winner: false,
                },
            ],
        });

        let tools_pending = self.settings_pending_tools_apply_next_turn();
        settings.push(ResolveSetting {
            setting: "Tool Approval Mode",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: approval_mode_display(self.core.turn_config.active.tool_approval_mode)
                        .to_string(),
                    is_winner: !tools_pending,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: if tools_pending {
                        approval_mode_display(self.core.turn_config.staged.tool_approval_mode)
                            .to_string()
                    } else {
                        "unset".to_string()
                    },
                    is_winner: tools_pending,
                },
            ],
        });

        let context_pending = self.settings_pending_context_apply_next_turn();
        settings.push(ResolveSetting {
            setting: "Context Memory",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: on_off(self.core.turn_config.active.context_memory_enabled).to_string(),
                    is_winner: !context_pending,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: if context_pending {
                        on_off(self.core.turn_config.staged.context_memory_enabled).to_string()
                    } else {
                        "unset".to_string()
                    },
                    is_winner: context_pending,
                },
            ],
        });

        let ui_pending = self.settings_pending_ui_apply_next_turn();
        settings.push(ResolveSetting {
            setting: "UI Defaults",
            layers: vec![
                ResolveLayerValue {
                    layer: "Global",
                    value: ui_options_display(self.core.turn_config.active.ui_options),
                    is_winner: !ui_pending,
                },
                ResolveLayerValue {
                    layer: "Project",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Profile",
                    value: "unset".to_string(),
                    is_winner: false,
                },
                ResolveLayerValue {
                    layer: "Session",
                    value: if ui_pending {
                        ui_options_display(self.core.turn_config.staged.ui_options)
                    } else {
                        "unset".to_string()
                    },
                    is_winner: ui_pending,
                },
            ],
        });

        ResolveCascade {
            settings,
            session_config_hash: self.session_config_hash(),
        }
    }

    #[must_use]
    pub fn validate_config(&self) -> ValidationReport {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut healthy = Vec::new();

        for provider in Provider::all() {
            if self.has_api_key(*provider) {
                healthy.push(ValidationFinding {
                    title: format!("{} API key configured", provider.display_name()),
                    detail: format!(
                        "{} is available for model selection.",
                        provider.display_name()
                    ),
                    fix_path: "Settings > Providers".to_string(),
                });
            } else {
                warnings.push(ValidationFinding {
                    title: format!("{} API key missing", provider.display_name()),
                    detail: format!(
                        "{} models will be blocked until a key is configured.",
                        provider.display_name()
                    ),
                    fix_path: format!("Settings > Providers > {}", provider.display_name()),
                });
            }
        }

        if self.current_api_key().is_some() {
            healthy.push(ValidationFinding {
                title: "Active model provider is configured".to_string(),
                detail: format!("{} can run immediately.", self.model()),
                fix_path: "Settings > Models".to_string(),
            });
        } else {
            errors.push(ValidationFinding {
                title: "Active model provider is not configured".to_string(),
                detail: format!(
                    "Model '{}' requires {} API key.",
                    self.model(),
                    self.provider().env_var()
                ),
                fix_path: "Settings > Providers".to_string(),
            });
        }

        match self.tool_journal_gate() {
            ToolJournalGate::Disabled { .. } => {
                warnings.push(ValidationFinding {
                    title: "Tool journal safety latch is active".to_string(),
                    detail: "Tool execution is disabled for crash-consistency safety.".to_string(),
                    fix_path: "Run /clear to reset journal state".to_string(),
                });
            }
            ToolJournalGate::Enabled => {
                healthy.push(ValidationFinding {
                    title: "Tool journal safety latch is clear".to_string(),
                    detail: "Tool execution is available.".to_string(),
                    fix_path: "Settings > Tools".to_string(),
                });
            }
        }

        ValidationReport {
            errors,
            warnings,
            healthy,
        }
    }

    pub fn enter_model_select_mode(&mut self) {
        let index = PredefinedModel::all()
            .iter()
            .position(|m| m.to_model_name() == self.core.model)
            .unwrap_or(0);
        self.ui.input = mem::take(&mut self.ui.input).into_model_select(index);
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            self.ui.view.modal_effect = ModalState::Inactive;
        } else {
            self.ui.view.modal_effect =
                ModalState::Active(ModalEffect::pop_scale(Duration::from_millis(700)));
            self.ui.view.last_frame = Instant::now();
        }
    }

    pub fn model_select_access(&self) -> ModelSelectAccess {
        match self.ui.input.model_select_ref() {
            ui::ModelSelectRef::Active { selected } => ModelSelectAccess::Active {
                selected_index: selected,
            },
            ui::ModelSelectRef::Inactive => ModelSelectAccess::Inactive,
        }
    }

    fn model_select_max_index() -> usize {
        PredefinedModel::all().len().saturating_sub(1)
    }

    fn trigger_model_select_shake(&mut self) {
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            return;
        }
        self.ui.view.modal_effect =
            ModalState::Active(ModalEffect::shake(Duration::from_millis(360)));
        self.ui.view.last_frame = Instant::now();
    }

    pub fn model_select_move_up(&mut self) {
        if let ui::ModelSelectMut::Active { selected } = self.ui.input.model_select_mut_access()
            && *selected > 0
        {
            *selected -= 1;
        }
    }

    pub fn model_select_move_down(&mut self) {
        if let ui::ModelSelectMut::Active { selected } = self.ui.input.model_select_mut_access() {
            let max_index = Self::model_select_max_index();
            if *selected < max_index {
                *selected += 1;
            }
        }
    }

    pub fn model_select_set_index(&mut self, index: usize) {
        if let ui::ModelSelectMut::Active { selected } = self.ui.input.model_select_mut_access() {
            let max_index = Self::model_select_max_index();
            *selected = index.min(max_index);
        }
    }

    /// Select the current model and return to normal mode.
    pub fn model_select_confirm(&mut self) {
        let ModelSelectAccess::Active {
            selected_index: index,
        } = self.model_select_access()
        else {
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
        if matches!(self.ui.file_picker.scan_state(), FileScanState::Unscanned) {
            let root = self.runtime.tool_settings.sandbox.working_dir();
            self.ui
                .file_picker
                .scan_files(&root, &self.runtime.tool_settings.sandbox);
        }
        self.ui.input = mem::take(&mut self.ui.input).into_file_select();
        if matches!(
            self.ui.view.ui_options.reduced_motion,
            ReducedMotion::Enabled
        ) {
            self.ui.view.modal_effect = ModalState::Inactive;
        } else {
            self.ui.view.modal_effect =
                ModalState::Active(ModalEffect::pop_scale(Duration::from_millis(700)));
            self.ui.view.last_frame = Instant::now();
        }
    }

    pub fn file_select_access(&self) -> FileSelectAccess<'_> {
        match self.ui.input.file_select_ref() {
            ui::FileSelectRef::Active { filter, selected } => FileSelectAccess::Active {
                filter: filter.text(),
                selected_index: selected,
            },
            ui::FileSelectRef::Inactive => FileSelectAccess::Inactive,
        }
    }

    pub fn file_select_files(&self) -> Vec<&ui::FileEntry> {
        self.ui.file_picker.filtered_files()
    }

    pub fn file_picker(&self) -> &ui::FilePickerState {
        &self.ui.file_picker
    }

    pub fn file_select_move_up(&mut self) {
        if let ui::FileSelectMut::Active { selected, .. } = self.ui.input.file_select_mut_access()
            && *selected > 0
        {
            *selected -= 1;
        }
    }

    pub fn file_select_move_down(&mut self) {
        if let ui::FileSelectMut::Active { selected, .. } = self.ui.input.file_select_mut_access() {
            let max_index = self.ui.file_picker.filtered_count().saturating_sub(1);
            if *selected < max_index {
                *selected += 1;
            }
        }
    }

    pub fn file_select_update_filter(&mut self) {
        let filter = match self.ui.input.file_select_ref() {
            ui::FileSelectRef::Active { filter, .. } => filter.text().to_string(),
            ui::FileSelectRef::Inactive => String::new(),
        };
        self.ui.file_picker.update_filter(&filter);
        if let ui::FileSelectMut::Active { selected, .. } = self.ui.input.file_select_mut_access() {
            *selected = 0;
        }
    }

    pub fn file_select_push_char(&mut self, c: char) {
        if let ui::FileSelectMut::Active { filter, .. } = self.ui.input.file_select_mut_access() {
            filter.enter_char(c);
        }
        self.file_select_update_filter();
    }

    pub fn file_select_backspace(&mut self) {
        if let ui::FileSelectMut::Active { filter, .. } = self.ui.input.file_select_mut_access() {
            filter.delete_char();
        }
        self.file_select_update_filter();
    }

    /// Confirm file selection - insert the selected file path into the draft.
    pub fn file_select_confirm(&mut self) {
        let FileSelectAccess::Active {
            selected_index: index,
            ..
        } = self.file_select_access()
        else {
            self.enter_insert_mode();
            return;
        };

        if let Some(entry) = self.ui.file_picker.get_selected(index) {
            let path = if entry.display.chars().any(char::is_whitespace) {
                let escaped = entry.display.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            } else {
                entry.display.clone()
            };
            // Insert the file path at cursor position in the draft
            self.ui.input.draft_mut().enter_text(&path);
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
        let data = approval.selecting_data_mut();
        if data.cursor == 0 {
            return;
        }
        data.cursor -= 1;
        data.expanded = ApprovalExpanded::Collapsed;
    }

    pub fn tool_approval_move_down(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        let data = approval.selecting_data_mut();
        let max_cursor = data.requests.len() + 1;
        if data.cursor < max_cursor {
            data.cursor += 1;
        }
        data.expanded = ApprovalExpanded::Collapsed;
    }

    pub fn tool_approval_toggle(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        let data = approval.selecting_data_mut();
        if data.cursor >= data.selected.len() {
            return;
        }
        data.selected[data.cursor].toggle();
    }

    pub fn tool_approval_toggle_details(&mut self) {
        let Some(approval) = self.tool_approval_mut() else {
            return;
        };
        let data = approval.selecting_data_mut();
        if data.cursor >= data.requests.len() {
            return;
        }
        data.expanded = match data.expanded {
            ApprovalExpanded::Expanded(idx) if idx == data.cursor => ApprovalExpanded::Collapsed,
            _ => ApprovalExpanded::Expanded(data.cursor),
        };
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
            CmpOrdering::Less => self.tool_approval_toggle(),
            CmpOrdering::Equal => self.tool_approval_confirm_selected(),
            CmpOrdering::Greater => self.tool_approval_request_deny_all(),
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
                    .filter(|(_, selected)| (**selected).is_approved())
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
            if approval.is_confirming_deny() {
                true
            } else {
                let deny_cursor = approval.data().requests.len() + 1;
                approval.selecting_data_mut().cursor = deny_cursor;
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
        self.ui.input.draft().text()
    }

    pub fn draft_cursor(&self) -> usize {
        self.ui.input.draft().cursor()
    }

    pub fn draft_cursor_byte_index(&self) -> usize {
        self.ui.input.draft().byte_index()
    }

    pub fn command_input_access(&self) -> CommandInputAccess<'_> {
        match self.ui.input.command_ref() {
            ui::CommandDraftRef::Active(command) => CommandInputAccess::Active {
                text: command.text(),
                cursor: command.cursor(),
                cursor_byte_index: command.byte_index(),
            },
            ui::CommandDraftRef::Inactive => CommandInputAccess::Inactive,
        }
    }

    /// Navigate to previous (older) prompt in Insert mode.
    ///
    /// On first call, stashes the current draft and shows the most recent prompt.
    /// Subsequent calls show progressively older prompts.
    pub fn navigate_history_up(&mut self) {
        if let ui::InsertDraftMut::Active(draft) = self.ui.input.insert_mut_access() {
            match self.ui.input_history.navigate_prompt_up(draft.text()) {
                ui::NavOutcome::Moved(text) => draft.set_text(text),
                ui::NavOutcome::AtBoundary => {}
            }
        }
    }

    /// Navigate to next (newer) prompt in Insert mode.
    ///
    /// When at the newest entry, restores the stashed draft.
    pub fn navigate_history_down(&mut self) {
        if let ui::InsertDraftMut::Active(draft) = self.ui.input.insert_mut_access() {
            match self.ui.input_history.navigate_prompt_down() {
                ui::NavOutcome::Moved(text) => draft.set_text(text),
                ui::NavOutcome::AtBoundary => {}
            }
        }
    }

    /// Navigate to previous (older) command in Command mode.
    pub fn navigate_command_history_up(&mut self) {
        if let ui::CommandDraftMut::Active(command) = self.ui.input.command_mut_access() {
            match self.ui.input_history.navigate_command_up(command.text()) {
                ui::NavOutcome::Moved(text) => command.set_text(text),
                ui::NavOutcome::AtBoundary => {}
            }
        }
    }

    /// Navigate to next (newer) command in Command mode.
    pub fn navigate_command_history_down(&mut self) {
        if let ui::CommandDraftMut::Active(command) = self.ui.input.command_mut_access() {
            match self.ui.input_history.navigate_command_down() {
                ui::NavOutcome::Moved(text) => command.set_text(text),
                ui::NavOutcome::AtBoundary => {}
            }
        }
    }

    /// Record a submitted prompt to history.
    pub(crate) fn record_prompt(&mut self, text: &str) {
        if let Ok(ne) = NonEmptyString::new(text) {
            self.ui.input_history.push_prompt(ne);
        }
        self.ui.input_history.reset_navigation();
    }

    /// Record an executed command to history.
    pub(crate) fn record_command(&mut self, text: &str) {
        if let Ok(ne) = NonEmptyString::new(text) {
            self.ui.input_history.push_command(ne);
        }
        self.ui.input_history.reset_navigation();
    }

    pub fn update_scroll_max(&mut self, max: u16) {
        self.ui.view.scroll_max = max;

        if let ScrollState::Manual { offset_from_top } = self.ui.view.scroll
            && offset_from_top >= max
        {
            self.ui.view.scroll = ScrollState::AutoBottom;
        }
    }

    pub fn scroll_offset_from_top(&self) -> u16 {
        match self.ui.view.scroll {
            ScrollState::AutoBottom => self.ui.view.scroll_max,
            ScrollState::Manual { offset_from_top } => offset_from_top.min(self.ui.view.scroll_max),
        }
    }

    pub fn scroll_up(&mut self) {
        self.ui.view.scroll = match self.ui.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.ui.view.scroll_max.saturating_sub(3),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(3),
            },
        };
    }

    /// Scroll up by a page.
    pub fn scroll_page_up(&mut self) {
        let delta = 10;
        self.ui.view.scroll = match self.ui.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.ui.view.scroll_max.saturating_sub(delta),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(delta),
            },
        };
    }

    pub fn scroll_down(&mut self) {
        let ScrollState::Manual { offset_from_top } = self.ui.view.scroll else {
            return;
        };

        let new_offset = offset_from_top.saturating_add(3);
        if new_offset >= self.ui.view.scroll_max {
            self.ui.view.scroll = ScrollState::AutoBottom;
        } else {
            self.ui.view.scroll = ScrollState::Manual {
                offset_from_top: new_offset,
            };
        }
    }

    /// Scroll down by a page.
    pub fn scroll_page_down(&mut self) {
        let ScrollState::Manual { offset_from_top } = self.ui.view.scroll else {
            return;
        };

        let delta = 10;
        let new_offset = offset_from_top.saturating_add(delta);
        if new_offset >= self.ui.view.scroll_max {
            self.ui.view.scroll = ScrollState::AutoBottom;
        } else {
            self.ui.view.scroll = ScrollState::Manual {
                offset_from_top: new_offset,
            };
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.ui.view.scroll = ScrollState::Manual { offset_from_top: 0 };
    }

    pub fn scroll_to_bottom(&mut self) {
        self.ui.view.scroll = ScrollState::AutoBottom;
    }

    /// Scroll up by 20% of total scrollable content.
    pub fn scroll_up_chunk(&mut self) {
        let delta = (self.ui.view.scroll_max / 5).max(1);
        self.ui.view.scroll = match self.ui.view.scroll {
            ScrollState::AutoBottom => ScrollState::Manual {
                offset_from_top: self.ui.view.scroll_max.saturating_sub(delta),
            },
            ScrollState::Manual { offset_from_top } => ScrollState::Manual {
                offset_from_top: offset_from_top.saturating_sub(delta),
            },
        };
    }
}

#[cfg(test)]
mod tests;

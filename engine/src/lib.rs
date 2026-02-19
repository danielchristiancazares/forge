//! The `forge_engine` crate provides the core building blocks for the Forge AI application.
//!
//! This crate is designed to be used by the Forge AI TUI and CLI.

mod ui;

pub use ui::{
    ChangeKind, DisplayItem, DraftInput, FileEntry, FilePickerState, FilesPanelState, FocusState,
    InputHistory, InputMode, InputState, ModalEffect, ModalEffectKind, PanelEffect,
    PanelEffectKind, PredefinedModel, ScrollState, SettingsCategory, SettingsModalState,
    SettingsSurface, UiOptions, ViewMode, ViewState, find_match_positions,
};

mod config;
pub use config::{AppConfig, ForgeConfig};

mod errors;
mod notifications;
mod security;
pub use security::sanitize_display_text;

mod session_state;
pub use session_state::SessionChangeLog;

mod environment;
pub use environment::{EnvironmentContext, assemble_prompt};

pub use notifications::SystemNotification;

mod state;
pub use state::DistillationTask;
pub use state::{ApprovalExpanded, ApprovalSelection};

mod util;

mod core;
mod runtime;

mod app;

// Public app facade.
pub use app::{
    App, AppearanceEditorSnapshot, CommandMode, CommandModeAccess, CommandSpec,
    ContextEditorSnapshot, DEFAULT_STREAM_EVENT_BUDGET, EnteredCommand, FileDiff, InsertMode,
    InsertModeAccess, ModelEditorSnapshot, QueueMessageResult, QueuedUserMessage, ResolveCascade,
    ResolveLayerValue, ResolveSetting, RuntimeSnapshot, StreamingMessage, SystemPrompts,
    ToolsEditorSnapshot, TurnUsage, ValidationFinding, ValidationReport, command_specs,
};

// Crate-internal cross-layer glue.
pub(crate) use app::{ActiveExecution, ToolQueue, TurnContext};

pub use forge_context::{
    ActiveJournal, BeginSessionError, CompactionPlan, ContextAdaptation, ContextBuildError,
    ContextManager, ContextUsageStatus, Fact, FactType, FullHistory, Librarian, MessageId,
    ModelLimits, ModelLimitsSource, ModelRegistry, PreparedContext, RecoveredStream,
    RecoveredToolBatch, StreamJournal, TokenCounter, ToolBatchId, ToolJournal, distillation_model,
    generate_distillation,
};

pub use forge_tools as tools;

pub use forge_providers::{self, ApiConfig, gemini::GeminiCache, gemini::GeminiCacheConfig};

pub use forge_types::{
    ApiKey, CacheHint, CacheableMessage, Message, ModelName, NonEmptyString, OpenAIReasoningEffort,
    OpenAIReasoningSummary, OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation,
    OutputLimits, PlanState, Provider, SecretString, StreamEvent, StreamFinishReason,
    ThinkingReplayState, ToolCall, ToolDefinition, ToolResult, sanitize_terminal_text,
};

pub use forge_config::{self, ConfigError};

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

// Modules moved to forge-core; re-exported for engine-internal use.
pub(crate) use forge_core::errors;
pub(crate) use forge_core::notifications;
pub(crate) use forge_core::thinking;

// Module-level aliases for engine-internal callers.
pub(crate) mod security {
    pub use forge_core::sanitize_display_text;
    pub use forge_utils::security::sanitize_stream_error;
}

pub(crate) mod util {
    pub use forge_core::{parse_model_name_from_string, wrap_api_key};
    pub use forge_types::truncate_with_ellipsis;
}

pub use forge_core::{
    DisplayLog, EnvironmentContext, NotificationQueue, SystemNotification, assemble_prompt,
    parse_model_name_from_string, sanitize_display_text, wrap_api_key,
};

mod session_state;
pub use session_state::SessionChangeLog;

mod state;
pub use state::DistillationTask;
pub use state::{ApprovalExpanded, ApprovalSelection};

mod app;

// Public app facade.
pub use app::{
    App, AppearanceEditorSnapshot, CommandInputAccess, CommandMode, CommandModeAccess, CommandSpec,
    ContextEditorSnapshot, DEFAULT_STREAM_EVENT_BUDGET, EnteredCommand, FileDiff, FileSelectAccess,
    InsertMode, InsertModeAccess, ModelEditorSnapshot, ModelSelectAccess, QueueMessageResult,
    QueuedUserMessage, ResolveCascade, ResolveLayerValue, ResolveSetting, RuntimeSnapshot,
    SettingsAccess, StreamingMessage, SystemPrompts, ToolsEditorSnapshot, TurnUsage,
    ValidationFinding, ValidationReport, command_specs,
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

//! Unit tests for the engine crate.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;
use forge_context::StepId;
use futures_util::future::AbortHandle;
use serde_json::json;
use tempfile::tempdir;

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::init::DEFAULT_MAX_TOOL_ARGS_BYTES;
use super::{
    App, AppearanceEditorSnapshot, CommandInputAccess, ContextEditorSnapshot, ModelEditorSnapshot,
    ProviderRuntimeState, QueueMessageResult, QueuedUserMessage, SettingsAccess, StreamingMessage,
    SystemPrompts, ToolsEditorSnapshot,
};
use crate::EnvironmentContext;
use crate::state::{
    ActiveStream, DataDir, DataDirSource, DistillationStart, DistillationState, DistillationTask,
    OperationState, ToolLoopPhase,
};
use crate::tools;
use crate::ui::{
    DisplayItem, DraftInput, InputMode, InputState, PredefinedModel, ScrollState, SettingsCategory,
    SettingsSurface, UiOptions, ViewState,
};
use forge_context::{ContextManager, StreamJournal, ToolJournal};
use forge_providers::ApiConfig;
use forge_types::{
    ApiKey, Message, ModelName, NonEmptyString, OpenAIReasoningEffort, OpenAIRequestOptions,
    OutputLimits, PlanState, Provider, SecretString, StreamEvent, StreamFinishReason,
    ThinkingReplayState, ThoughtSignatureState, ToolCall, ToolResult,
};

/// Test system prompts for unit tests.
const TEST_SYSTEM_PROMPTS: SystemPrompts = SystemPrompts {
    claude: "You are a helpful assistant.",
    openai: "You are a helpful assistant.",
    gemini: "You are a helpful assistant.",
};

fn test_app() -> App {
    let mut api_keys = HashMap::new();
    api_keys.insert(Provider::Claude, SecretString::new("test".to_string()));
    let model = Provider::Claude.default_model();
    let stream_journal = StreamJournal::open_in_memory().expect("in-memory journal for tests");
    let data_dir_path = tempdir().expect("temp data dir for tests").keep();
    let data_dir = DataDir {
        path: data_dir_path,
        source: DataDirSource::System,
    };
    let output_limits = OutputLimits::new(4096);
    let mut context_manager = ContextManager::new(model.clone());
    context_manager.set_output_limit(output_limits.max_output_tokens());
    let tool_settings = App::tool_settings_from_config(None);
    let (tool_registry, tool_definitions, hidden_tools) = App::build_tool_registry(&tool_settings);
    let tool_journal = ToolJournal::open_in_memory().expect("in-memory tool journal");
    let tool_file_cache = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    super::init::build_app(super::init::AppBuildParts {
        view: ViewState::default(),
        configured_model: model.clone(),
        configured_tool_approval_mode: tools::ApprovalMode::Default,
        configured_context_memory_enabled: true,
        configured_ui_options: UiOptions::default(),
        api_keys,
        config_path: tempdir()
            .expect("temp config dir for tests")
            .keep()
            .join("config.toml"),
        model: model.clone(),
        data_dir,
        context_manager,
        stream_journal,
        memory_enabled: true,
        output_limits,
        configured_output_limits: output_limits,
        cache_enabled: false,
        provider_runtime: ProviderRuntimeState {
            openai_options: OpenAIRequestOptions::default(),
            openai_reasoning_effort_explicit: false,
            gemini_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            gemini_thinking_enabled: false,
            anthropic_thinking_mode: crate::config::AnthropicThinkingMode::default(),
            anthropic_thinking_effort: crate::config::AnthropicEffort::default(),
            gemini_cache_config: crate::GeminiCacheConfig::default(),
            openai_previous_response_id: None,
        },
        system_prompts: TEST_SYSTEM_PROMPTS,
        environment: EnvironmentContext::gather_without_agents_md(),
        tool_definitions,
        hidden_tools,
        tool_registry,
        tool_settings,
        tool_journal,
        tool_file_cache,
        librarian: None,
        lsp_config: None,
    })
}

fn expect_queued_message(result: QueueMessageResult) -> QueuedUserMessage {
    match result {
        QueueMessageResult::Queued(queued) => queued,
        QueueMessageResult::Skipped => panic!("queued message"),
    }
}

fn insert_mode(app: &mut App) -> super::InsertMode<'_> {
    match app.insert_mode_mut() {
        super::InsertModeAccess::InInsert(mode) => mode,
        super::InsertModeAccess::NotInsert => panic!("insert mode"),
    }
}

fn command_mode(app: &mut App) -> super::CommandMode<'_> {
    match app.command_mode_mut() {
        super::CommandModeAccess::InCommand(mode) => mode,
        super::CommandModeAccess::NotCommand => panic!("command mode"),
    }
}

fn last_notification(app: &App) -> Option<&str> {
    for item in app.ui.display.iter().rev() {
        if let DisplayItem::Local(msg) = item
            && msg.role_str() == "system"
        {
            return Some(msg.content());
        }
    }
    None
}

fn set_streaming_state(app: &mut App) {
    let (_tx, rx) = mpsc::channel(1);
    let streaming = StreamingMessage::new(
        app.core.model.clone(),
        rx,
        app.runtime.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .runtime
        .stream_journal
        .begin_session(app.core.model.as_str())
        .expect("journal session");
    app.core.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: super::TurnContext::new_for_tests(),
    });
}

fn settings_surface(app: &App) -> Option<SettingsSurface> {
    match app.settings_access() {
        SettingsAccess::Active { surface, .. } => Some(surface),
        SettingsAccess::Inactive => None,
    }
}

fn settings_filter_text(app: &App) -> Option<&str> {
    match app.settings_access() {
        SettingsAccess::Active { filter_text, .. } => Some(filter_text),
        SettingsAccess::Inactive => None,
    }
}

fn settings_filter_active(app: &App) -> bool {
    match app.settings_access() {
        SettingsAccess::Active { filter_active, .. } => filter_active,
        SettingsAccess::Inactive => false,
    }
}

fn settings_detail_view(app: &App) -> Option<SettingsCategory> {
    match app.settings_access() {
        SettingsAccess::Active { detail_view, .. } => detail_view,
        SettingsAccess::Inactive => None,
    }
}

fn settings_selected_index(app: &App) -> Option<usize> {
    match app.settings_access() {
        SettingsAccess::Active { selected_index, .. } => Some(selected_index),
        SettingsAccess::Inactive => None,
    }
}

#[test]
fn openai_options_default_upgrade_for_gpt_52_pro() {
    let app = test_app();
    let pro_model = ModelName::from_predefined(PredefinedModel::Gpt52Pro);
    let base_model = ModelName::from_predefined(PredefinedModel::Gpt52);

    let pro_options = app.openai_options_for_model(&pro_model);
    let base_options = app.openai_options_for_model(&base_model);

    assert_eq!(pro_options.reasoning_effort(), OpenAIReasoningEffort::XHigh);
    assert_eq!(base_options.reasoning_effort(), OpenAIReasoningEffort::High);
}

#[test]
fn openai_options_respects_explicit_effort_for_gpt_52_pro() {
    let mut app = test_app();
    app.runtime
        .provider_runtime
        .openai_reasoning_effort_explicit = true;
    app.runtime.provider_runtime.openai_options = OpenAIRequestOptions::new(
        OpenAIReasoningEffort::Medium,
        app.runtime
            .provider_runtime
            .openai_options
            .reasoning_summary(),
        app.runtime.provider_runtime.openai_options.verbosity(),
        app.runtime.provider_runtime.openai_options.truncation(),
    );
    let pro_model = ModelName::from_predefined(PredefinedModel::Gpt52Pro);
    let pro_options = app.openai_options_for_model(&pro_model);
    assert_eq!(
        pro_options.reasoning_effort(),
        OpenAIReasoningEffort::Medium
    );
}

#[derive(Debug)]
struct MockTool {
    name: &'static str,
    requires_approval: bool,
    log: Arc<Mutex<Vec<String>>>,
}

impl MockTool {
    fn new(name: &'static str, requires_approval: bool, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            requires_approval,
            log,
        }
    }
}

impl tools::ToolExecutor for MockTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Mock tool for ordering tests"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn approval_summary(&self, _args: &serde_json::Value) -> Result<String, tools::ToolError> {
        let name = self.name;
        Ok(format!("Run {name}"))
    }

    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a mut tools::ToolCtx,
    ) -> tools::ToolFut<'a> {
        let log = Arc::clone(&self.log);
        let name = self.name.to_string();
        Box::pin(async move {
            log.lock().expect("log lock").push(name);
            Ok("ok".to_string())
        })
    }
}

async fn drive_tool_loop_to_idle(app: &mut App) {
    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    loop {
        if matches!(app.core.state, OperationState::Idle) {
            break;
        }
        match app.core.state {
            OperationState::ToolLoop(_) => {
                app.poll_tool_loop();
                tokio::task::yield_now().await;
            }
            _ => panic!("unexpected state while driving tool loop"),
        }
        assert!(
            start.elapsed() <= timeout,
            "tool loop did not finish before timeout"
        );
    }
}

#[test]
fn enter_and_delete_respects_unicode_cursor() {
    let mut app = test_app();
    app.ui.input = InputState::Insert(DraftInput::new("a🦀b".to_string(), 1));

    {
        let mut insert = insert_mode(&mut app);
        insert.enter_char('X');
    }
    assert_eq!(app.draft_text(), "aX🦀b");
    assert_eq!(app.draft_cursor(), 2);

    {
        let mut insert = insert_mode(&mut app);
        insert.delete_char();
    }
    assert_eq!(app.draft_text(), "a🦀b");
    assert_eq!(app.draft_cursor(), 1);

    {
        let mut insert = insert_mode(&mut app);
        insert.delete_char_forward();
    }
    assert_eq!(app.draft_text(), "ab");
    assert_eq!(app.draft_cursor(), 1);
}

#[test]
fn submit_message_adds_user_message() {
    let mut app = test_app();
    app.ui.input = InputState::Insert(DraftInput::new("hello".to_string(), 5));

    let queued = insert_mode(&mut app).queue_message();
    let _ = expect_queued_message(queued);

    assert!(app.draft_text().is_empty());
    assert_eq!(app.draft_cursor(), 0);
    assert_eq!(app.ui.view.scroll, ScrollState::AutoBottom);
    assert!(!app.is_loading());

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
        let mut command_mode = command_mode(&mut app);
        command_mode.push_char('q');
        command_mode.take_command()
    };

    app.process_command(command);

    assert!(app.should_quit());
    assert_eq!(app.input_mode(), InputMode::Normal);
    assert!(matches!(
        app.command_input_access(),
        CommandInputAccess::Inactive
    ));
}

#[test]
fn process_command_clear_resets_conversation() {
    let mut app = test_app();
    let content = NonEmptyString::new("hi").expect("non-empty test content");
    app.push_history_message(Message::user(content, SystemTime::now()));
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "clear".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert!(app.is_empty());
    assert!(app.display_items().is_empty());
    assert_eq!(app.input_mode(), InputMode::Normal);
}

#[test]
fn process_command_clear_requests_transcript_clear() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "clear".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert!(app.take_clear_transcript());
    assert!(!app.take_clear_transcript());
}

#[test]
fn process_command_settings_opens_settings_modal() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "settings".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(settings_filter_text(&app), Some(""));
    assert!(!settings_filter_active(&app));
    assert_eq!(settings_selected_index(&app), Some(0));
    assert_eq!(settings_detail_view(&app), None);
    assert_eq!(settings_surface(&app), Some(SettingsSurface::Root));
}

#[test]
fn settings_access_is_inactive_outside_settings_mode() {
    let app = test_app();
    assert!(matches!(app.settings_access(), SettingsAccess::Inactive));
}

#[test]
fn settings_access_reports_root_modal_state() {
    let mut app = test_app();
    app.enter_settings_mode();
    assert!(matches!(
        app.settings_access(),
        SettingsAccess::Active {
            surface: SettingsSurface::Root,
            filter_text: "",
            filter_active: false,
            detail_view: None,
            selected_index: 0,
        }
    ));
}

#[test]
fn process_command_settings_shows_guardrail_when_busy() {
    let mut app = test_app();
    set_streaming_state(&mut app);
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "settings".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert_eq!(
        last_notification(&app),
        Some(
            "Settings edits apply on the next turn. Active turn remains unchanged while streaming a response."
        )
    );
}

#[test]
fn process_command_runtime_opens_runtime_panel() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "runtime".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(settings_surface(&app), Some(SettingsSurface::Runtime));
}

#[test]
fn process_command_resolve_opens_resolve_panel() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "resolve".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(settings_surface(&app), Some(SettingsSurface::Resolve));
}

#[test]
fn process_command_validate_opens_validation_panel() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let mut command_mode = command_mode(&mut app);
        for c in "validate".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command()
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(settings_surface(&app), Some(SettingsSurface::Validate));
}

#[test]
fn settings_resolve_activate_selected_jumps_to_target_detail() {
    let mut app = test_app();
    app.enter_resolve_mode();
    for _ in 0..3 {
        app.settings_resolve_move_down();
    }

    app.settings_resolve_activate_selected();

    assert_eq!(settings_surface(&app), Some(SettingsSurface::Root));
    assert_eq!(settings_detail_view(&app), Some(SettingsCategory::Tools));
}

#[test]
fn settings_resolve_move_down_clamps_to_last_setting() {
    let mut app = test_app();
    app.enter_resolve_mode();
    for _ in 0..20 {
        app.settings_resolve_move_down();
    }

    let last_index = app.resolve_cascade().settings.len().saturating_sub(1);
    assert_eq!(settings_selected_index(&app), Some(last_index));
}

#[test]
fn runtime_snapshot_lists_pending_next_turn_settings() {
    let mut app = test_app();
    app.core.pending_turn_model = Some(ModelName::from_predefined(PredefinedModel::Gpt52Pro));
    app.core.pending_turn_context_memory_enabled = Some(false);
    app.core.pending_turn_ui_options = Some(UiOptions {
        ascii_only: true,
        high_contrast: true,
        reduced_motion: false,
        show_thinking: false,
    });

    let snapshot = app.runtime_snapshot();

    assert!(
        snapshot
            .session_overrides
            .iter()
            .any(|item| item == "pending model change: next turn")
    );
    assert!(
        snapshot
            .session_overrides
            .iter()
            .any(|item| item == "pending context memory: off (next turn)")
    );
    assert!(
        snapshot
            .session_overrides
            .iter()
            .any(|item| item == "pending ui defaults: ascii_only, high_contrast (next turn)")
    );
}

#[test]
fn resolve_cascade_includes_context_memory_and_ui_defaults_layers() {
    let mut app = test_app();
    app.core.pending_turn_context_memory_enabled = Some(false);
    app.core.pending_turn_ui_options = Some(UiOptions {
        ascii_only: false,
        high_contrast: true,
        reduced_motion: true,
        show_thinking: false,
    });

    let cascade = app.resolve_cascade();

    let context_memory = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "Context Memory")
        .expect("context memory setting");
    let context_session_layer = context_memory
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("session context layer");
    assert_eq!(context_session_layer.value, "off");
    assert!(context_session_layer.is_winner);

    let ui_defaults = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "UI Defaults")
        .expect("ui defaults setting");
    let ui_session_layer = ui_defaults
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("session ui layer");
    assert_eq!(ui_session_layer.value, "high_contrast, reduced_motion");
    assert!(ui_session_layer.is_winner);
}

#[test]
fn resolve_cascade_uses_pending_model_for_session_layer() {
    let mut app = test_app();
    app.core.pending_turn_model = Some(ModelName::from_predefined(PredefinedModel::Gpt52Pro));

    let cascade = app.resolve_cascade();

    let model = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "Model")
        .expect("model setting");
    let global = model
        .layers
        .iter()
        .find(|layer| layer.layer == "Global")
        .expect("global layer");
    let session = model
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("session layer");
    assert_eq!(session.value, "gpt-5.2-pro");
    assert!(session.is_winner);
    assert!(!global.is_winner);
}

fn open_appearance_settings(app: &mut App) {
    app.enter_settings_mode();
    for _ in 0..SettingsCategory::ALL.len().saturating_sub(1) {
        app.settings_move_down();
    }
    app.settings_activate();
}

fn open_models_settings(app: &mut App) {
    app.enter_settings_mode();
    app.settings_move_down();
    app.settings_activate();
}

fn open_context_settings(app: &mut App) {
    app.enter_settings_mode();
    for _ in 0..2 {
        app.settings_move_down();
    }
    app.settings_activate();
}

fn open_tools_settings(app: &mut App) {
    app.enter_settings_mode();
    for _ in 0..3 {
        app.settings_move_down();
    }
    app.settings_activate();
}

#[test]
fn settings_close_or_exit_blocks_with_unsaved_detail_changes() {
    let mut app = test_app();
    open_models_settings(&mut app);

    app.settings_detail_move_down();
    app.settings_detail_toggle_selected();
    app.settings_close_or_exit();

    assert_eq!(settings_detail_view(&app), Some(SettingsCategory::Models));
    assert_eq!(
        last_notification(&app),
        Some("Unsaved settings changes. Press s to save or r to revert before leaving.")
    );
}

#[test]
fn settings_close_or_exit_allows_leaving_detail_after_revert() {
    let mut app = test_app();
    open_models_settings(&mut app);

    app.settings_detail_move_down();
    app.settings_detail_toggle_selected();
    app.settings_revert_edits();
    app.settings_close_or_exit();

    assert_eq!(settings_detail_view(&app), None);
    assert_eq!(app.input_mode(), InputMode::Settings);
}

#[test]
fn settings_usable_model_count_reflects_configured_providers() {
    let mut app = test_app();
    assert_eq!(app.settings_usable_model_count(), 3);

    app.runtime
        .api_keys
        .insert(Provider::OpenAI, SecretString::new("openai".to_string()));
    assert_eq!(app.settings_usable_model_count(), 5);

    app.runtime
        .api_keys
        .insert(Provider::Gemini, SecretString::new("gemini".to_string()));
    assert_eq!(app.settings_usable_model_count(), 7);
}

#[test]
fn settings_activate_models_initializes_editor_snapshot() {
    let mut app = test_app();

    open_models_settings(&mut app);

    assert_eq!(settings_detail_view(&app), Some(SettingsCategory::Models));
    assert_eq!(
        app.settings_model_editor_snapshot(),
        Some(ModelEditorSnapshot {
            draft: app.settings_configured_model().clone(),
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_models_select_and_revert_updates_dirty_state() {
    let mut app = test_app();
    open_models_settings(&mut app);

    app.settings_detail_move_down();
    app.settings_detail_toggle_selected();

    let selected_model = ModelName::from_predefined(PredefinedModel::ClaudeSonnet);
    assert_eq!(
        app.settings_model_editor_snapshot(),
        Some(ModelEditorSnapshot {
            draft: selected_model,
            selected: 1,
            dirty: true,
        })
    );

    app.settings_revert_edits();
    assert_eq!(
        app.settings_model_editor_snapshot(),
        Some(ModelEditorSnapshot {
            draft: app.settings_configured_model().clone(),
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_models_save_warns_when_provider_key_is_missing() {
    let mut app = test_app();
    open_models_settings(&mut app);

    app.settings_detail_move_down();
    app.settings_detail_move_down();
    app.settings_detail_move_down();
    app.settings_detail_toggle_selected();
    app.settings_save_edits();

    assert_eq!(
        app.settings_configured_model(),
        &ModelName::from_predefined(PredefinedModel::Gpt52Pro)
    );
    assert!(app.settings_pending_model_apply_next_turn());
    assert_eq!(
        last_notification(&app),
        Some("GPT API key is missing. Set OPENAI_API_KEY before the next turn.")
    );
}

#[test]
fn settings_activate_tools_initializes_editor_snapshot() {
    let mut app = test_app();

    open_tools_settings(&mut app);

    assert_eq!(settings_detail_view(&app), Some(SettingsCategory::Tools));
    assert_eq!(
        app.settings_tools_editor_snapshot(),
        Some(ToolsEditorSnapshot {
            draft_approval_mode: "default",
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_tools_cycle_and_revert_updates_dirty_state() {
    let mut app = test_app();
    open_tools_settings(&mut app);

    app.settings_detail_toggle_selected();
    assert_eq!(
        app.settings_tools_editor_snapshot(),
        Some(ToolsEditorSnapshot {
            draft_approval_mode: "strict",
            selected: 0,
            dirty: true,
        })
    );

    app.settings_revert_edits();
    assert_eq!(
        app.settings_tools_editor_snapshot(),
        Some(ToolsEditorSnapshot {
            draft_approval_mode: "default",
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_tools_save_shows_guardrail_when_busy() {
    let mut app = test_app();
    set_streaming_state(&mut app);
    open_tools_settings(&mut app);

    app.settings_detail_toggle_selected();
    app.settings_save_edits();

    assert!(app.settings_pending_tools_apply_next_turn());
    assert_eq!(
        last_notification(&app),
        Some(
            "Settings edits apply on the next turn. Active turn remains unchanged while streaming a response."
        )
    );
}

#[test]
fn settings_activate_context_initializes_editor_snapshot() {
    let mut app = test_app();

    open_context_settings(&mut app);

    assert_eq!(settings_detail_view(&app), Some(SettingsCategory::Context));
    assert_eq!(
        app.settings_context_editor_snapshot(),
        Some(ContextEditorSnapshot {
            draft_memory_enabled: true,
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_context_toggle_and_revert_updates_dirty_state() {
    let mut app = test_app();
    open_context_settings(&mut app);

    app.settings_detail_toggle_selected();
    assert_eq!(
        app.settings_context_editor_snapshot(),
        Some(ContextEditorSnapshot {
            draft_memory_enabled: false,
            selected: 0,
            dirty: true,
        })
    );

    app.settings_revert_edits();
    assert_eq!(
        app.settings_context_editor_snapshot(),
        Some(ContextEditorSnapshot {
            draft_memory_enabled: true,
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_activate_appearance_initializes_editor_snapshot() {
    let mut app = test_app();

    open_appearance_settings(&mut app);

    assert_eq!(
        settings_detail_view(&app),
        Some(SettingsCategory::Appearance)
    );
    assert_eq!(
        app.settings_appearance_editor_snapshot(),
        Some(AppearanceEditorSnapshot {
            draft: UiOptions::default(),
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_appearance_toggle_and_revert_updates_dirty_state() {
    let mut app = test_app();
    open_appearance_settings(&mut app);

    app.settings_detail_toggle_selected();
    assert_eq!(
        app.settings_appearance_editor_snapshot(),
        Some(AppearanceEditorSnapshot {
            draft: UiOptions {
                ascii_only: true,
                ..UiOptions::default()
            },
            selected: 0,
            dirty: true,
        })
    );

    app.settings_revert_edits();
    assert_eq!(
        app.settings_appearance_editor_snapshot(),
        Some(AppearanceEditorSnapshot {
            draft: UiOptions::default(),
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn apply_pending_turn_settings_consumes_staged_defaults() {
    let mut app = test_app();
    let pending = UiOptions {
        ascii_only: true,
        high_contrast: true,
        reduced_motion: true,
        show_thinking: true,
    };
    app.core.pending_turn_ui_options = Some(pending);

    app.apply_pending_turn_settings();

    assert_eq!(app.ui_options(), pending);
    assert!(!app.settings_pending_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_model_before_request_config() {
    let mut app = test_app();
    let pending_model = ModelName::from_predefined(PredefinedModel::ClaudeHaiku);
    app.core.pending_turn_model = Some(pending_model.clone());
    app.ui.input = InputState::Insert(DraftInput::new("pending model".to_string(), 13));

    let queued = insert_mode(&mut app).queue_message();
    let queued = expect_queued_message(queued);

    assert_eq!(queued.config.model(), &pending_model);
    assert_eq!(app.model(), pending_model.as_str());
    assert!(!app.settings_pending_model_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_context_before_request_config() {
    let mut app = test_app();
    app.core.pending_turn_context_memory_enabled = Some(false);
    app.ui.input = InputState::Insert(DraftInput::new("pending context".to_string(), 15));

    let queued = insert_mode(&mut app).queue_message();
    let _ = expect_queued_message(queued);

    assert!(!app.memory_enabled());
    assert!(!app.settings_pending_context_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_tools_before_request_config() {
    let mut app = test_app();
    app.core.pending_turn_tool_approval_mode = Some(tools::ApprovalMode::Strict);
    app.ui.input = InputState::Insert(DraftInput::new("pending tools".to_string(), 13));

    let queued = insert_mode(&mut app).queue_message();
    let _ = expect_queued_message(queued);

    assert_eq!(
        app.runtime.tool_settings.policy.mode,
        tools::ApprovalMode::Strict
    );
    assert!(!app.settings_pending_tools_apply_next_turn());
}

#[test]
fn session_config_hash_is_stable_without_changes() {
    let app = test_app();
    let first = app.session_config_hash();
    let second = app.session_config_hash();
    assert_eq!(first, second);
}

#[test]
fn validation_findings_include_fix_paths() {
    let app = test_app();
    let report = app.validate_config();
    for finding in report
        .errors
        .iter()
        .chain(report.warnings.iter())
        .chain(report.healthy.iter())
    {
        assert!(!finding.fix_path.trim().is_empty());
    }
}

#[test]
fn process_stream_events_applies_deltas_and_done() {
    let mut app = test_app();

    let (tx, rx) = mpsc::channel(1024);
    let streaming = StreamingMessage::new(
        app.core.model.clone(),
        rx,
        app.runtime.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .runtime
        .stream_journal
        .begin_session(app.core.model.as_str())
        .expect("journal session");
    app.core.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: super::TurnContext::new_for_tests(),
    });
    assert!(app.is_loading());

    tx.try_send(StreamEvent::TextDelta("hello".to_string()))
        .expect("send delta");
    tx.try_send(StreamEvent::Done).expect("send done");

    app.process_stream_events();

    let last = app.history().entries().last().expect("completed message");
    assert!(matches!(last.message(), Message::Assistant(_)));
    assert_eq!(last.message().content(), "hello");
    assert!(!app.is_loading());
    assert!(app.streaming().is_none());
}

#[test]
fn process_stream_events_persists_thinking_signature_when_hidden() {
    let mut app = test_app();

    let (tx, rx) = mpsc::channel(1024);
    let streaming = StreamingMessage::new(
        app.core.model.clone(),
        rx,
        app.runtime.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .runtime
        .stream_journal
        .begin_session(app.core.model.as_str())
        .expect("journal session");
    app.core.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: super::TurnContext::new_for_tests(),
    });

    tx.try_send(StreamEvent::ThinkingSignature("sig".to_string()))
        .expect("send signature");
    tx.try_send(StreamEvent::TextDelta("hello".to_string()))
        .expect("send delta");
    tx.try_send(StreamEvent::Done).expect("send done");

    app.process_stream_events();

    let entries = app.history().entries();
    assert_eq!(entries.len(), 2);
    let thinking_entry = entries.first().expect("thinking message");
    let assistant_entry = entries.last().expect("assistant message");

    let Message::Thinking(thinking) = thinking_entry.message() else {
        panic!("expected thinking message first");
    };
    assert_eq!(thinking.content(), "[Thinking hidden]");
    assert!(matches!(
        thinking.replay_state(),
        ThinkingReplayState::ClaudeSigned { signature } if signature.as_str() == "sig"
    ));

    assert!(matches!(assistant_entry.message(), Message::Assistant(_)));
    assert_eq!(assistant_entry.message().content(), "hello");
}

#[test]
fn process_stream_events_respects_budget() {
    let mut app = test_app();

    let (tx, rx) = mpsc::channel(1024);
    let streaming = StreamingMessage::new(
        app.core.model.clone(),
        rx,
        app.runtime.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .runtime
        .stream_journal
        .begin_session(app.core.model.as_str())
        .expect("journal session");
    app.core.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: super::TurnContext::new_for_tests(),
    });

    for _ in 0..10_000 {
        let _ = tx.try_send(StreamEvent::TextDelta("x".to_string()));
    }

    app.process_stream_events();

    let content_len = app.streaming().expect("still streaming").content().len();
    assert_eq!(content_len, 64);
    assert!(content_len < 10_000);
}

#[test]
fn process_stream_events_starts_tool_journal_on_first_tool_call() {
    let mut app = test_app();

    let (tx, rx) = mpsc::channel(1024);
    let streaming = StreamingMessage::new(
        app.core.model.clone(),
        rx,
        app.runtime.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .runtime
        .stream_journal
        .begin_session(app.core.model.as_str())
        .expect("journal session");
    app.core.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: super::TurnContext::new_for_tests(),
    });

    tx.try_send(StreamEvent::ToolCallStart {
        id: "call-1".to_string(),
        name: "Read".to_string(),
        thought_signature: ThoughtSignatureState::Unsigned,
    })
    .expect("send tool start");
    tx.try_send(StreamEvent::ToolCallDelta {
        id: "call-1".to_string(),
        arguments: r#"{"path":"foo.txt"}"#.to_string(),
    })
    .expect("send tool args");

    app.process_stream_events();

    let tool_batch_id = match &app.core.state {
        OperationState::Streaming(ActiveStream::Journaled { tool_batch_id, .. }) => *tool_batch_id,
        _ => panic!("expected journaled stream"),
    };

    let recovered = app
        .runtime
        .tool_journal
        .recover()
        .expect("recover tool batch")
        .expect("pending tool batch");
    assert_eq!(recovered.batch_id, tool_batch_id);
    assert_eq!(recovered.model_name, app.core.model.as_str());
    assert_eq!(recovered.calls.len(), 1);
    let call = &recovered.calls[0];
    assert_eq!(call.id, "call-1");
    assert_eq!(call.name, "Read");
    assert_eq!(call.arguments["path"], "foo.txt");
    assert!(recovered.results.is_empty());
}

#[test]
fn submit_message_without_key_sets_status_and_does_not_queue() {
    let mut app = test_app();
    app.runtime.api_keys.clear();
    app.ui.input = InputState::Insert(DraftInput::new("hi".to_string(), 2));

    let queued = insert_mode(&mut app).queue_message();
    assert!(matches!(queued, QueueMessageResult::Skipped));
    assert_eq!(
        last_notification(&app),
        Some("No API key configured. Set ANTHROPIC_API_KEY environment variable.")
    );
    assert!(app.is_empty());
    assert!(!app.is_loading());
}

#[test]
fn queue_message_sets_pending_user_message() {
    let mut app = test_app();
    app.ui.input = InputState::Insert(DraftInput::new("test message".to_string(), 12));

    let queued = insert_mode(&mut app).queue_message();
    let _ = expect_queued_message(queued);

    assert!(app.core.pending_user_message.is_some());
    let (msg_id, original_text, _agents_md) = app.core.pending_user_message.as_ref().unwrap();
    assert_eq!(msg_id.as_u64(), 0);
    assert_eq!(original_text, "test message");
}

#[tokio::test]
async fn distillation_not_needed_starts_queued_request() {
    let mut app = test_app();
    app.ui.input = InputState::Insert(DraftInput::new("queued".to_string(), 6));

    let queued = insert_mode(&mut app).queue_message();
    let queued = expect_queued_message(queued);

    let result = app.try_start_distillation(Some(queued));

    assert_eq!(result, DistillationStart::NotNeeded);
    assert!(matches!(app.core.state, OperationState::Streaming(_)));
}

#[test]
fn rollback_pending_user_message_restores_input() {
    let mut app = test_app();

    let content = NonEmptyString::new("my message").expect("non-empty");
    let msg_id = app.push_history_message(Message::user(content, SystemTime::now()));
    app.core.pending_user_message = Some((msg_id, "my message".to_string(), String::new()));

    assert_eq!(app.history().len(), 1);
    assert_eq!(app.ui.display.len(), 1);

    app.rollback_pending_user_message();

    assert_eq!(app.history().len(), 0);
    assert_eq!(app.ui.display.len(), 0);
    assert_eq!(app.draft_text(), "my message");
    assert_eq!(app.input_mode(), InputMode::Insert);
    assert!(app.core.pending_user_message.is_none());
}

#[test]
fn rollback_pending_user_message_no_op_when_empty() {
    let mut app = test_app();

    assert!(app.core.pending_user_message.is_none());

    app.rollback_pending_user_message();

    assert!(app.draft_text().is_empty());
    assert!(app.core.pending_user_message.is_none());
}

#[test]
fn streaming_message_apply_text_delta() {
    let (tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    assert!(stream.content().is_empty());

    let result = stream.apply_event(StreamEvent::TextDelta("Hello".to_string()));
    assert!(result.is_none());

    assert_eq!(stream.content(), "Hello");

    let result = stream.apply_event(StreamEvent::TextDelta(" World".to_string()));
    assert!(result.is_none());

    assert_eq!(stream.content(), "Hello World");

    drop(tx); // Suppress unused warning
}

#[test]
fn streaming_message_apply_done() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("content".to_string()));

    let result = stream.apply_event(StreamEvent::Done);
    assert_eq!(result, Some(StreamFinishReason::Done));
}

#[test]
fn streaming_message_apply_error() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    let result = stream.apply_event(StreamEvent::Error("API error".to_string()));
    assert_eq!(
        result,
        Some(StreamFinishReason::Error("API error".to_string()))
    );
}

#[test]
fn streaming_message_apply_thinking_delta_ignored() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("visible".to_string()));
    stream.apply_event(StreamEvent::ThinkingDelta("thinking...".to_string()));

    assert_eq!(stream.content(), "visible");
}

#[test]
fn streaming_message_apply_thinking_delta_captured_when_enabled() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("visible".to_string()));
    stream.apply_event(StreamEvent::ThinkingDelta("thinking...".to_string()));

    assert_eq!(stream.content(), "visible");
    assert_eq!(stream.thinking(), "thinking...");
}

#[test]
fn streaming_message_into_message_success() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model.clone(), rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("Test content".to_string()));

    let message = stream.into_message().expect("should convert to message");
    assert_eq!(message.content(), "Test content");
    assert!(matches!(message, Message::Assistant(_)));
}

#[test]
fn streaming_message_into_message_empty_fails() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    let result = stream.into_message();
    assert!(result.is_err());
}

#[test]
fn streaming_message_provider_and_model() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = ModelName::from_predefined(PredefinedModel::Gpt52);
    let stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    assert_eq!(stream.provider(), Provider::OpenAI);
    assert_eq!(stream.model_name().as_str(), "gpt-5.2");
}

#[test]
fn tool_call_args_overflow_pre_resolved_error() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, 4);

    stream.apply_event(StreamEvent::ToolCallStart {
        id: "call-1".to_string(),
        name: "Run".to_string(),
        thought_signature: ThoughtSignatureState::Unsigned,
    });
    stream.apply_event(StreamEvent::ToolCallDelta {
        id: "call-1".to_string(),
        arguments: "12345".to_string(),
    });

    let parsed = stream.take_tool_calls();
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.pre_resolved.len(), 1);
    let result = &parsed.pre_resolved[0];
    assert_eq!(result.tool_call_id, "call-1");
    assert!(result.is_error);
    assert_eq!(result.content, "Tool arguments exceeded maximum size");
    assert_eq!(parsed.calls[0].arguments, json!({}));
}

#[test]
fn tool_call_args_json_escaping_is_deserialized_with_serde() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::ToolCallStart {
        id: "call-1".to_string(),
        name: "Run".to_string(),
        thought_signature: ThoughtSignatureState::Unsigned,
    });
    stream.apply_event(StreamEvent::ToolCallDelta {
        id: "call-1".to_string(),
        arguments: r#"{"url":"https:\/\/example.com\/a","text":"caf\u00e9","emoji":"\ud83d\ude80","quote":"He said \"hi\""}"#.to_string(),
    });

    let parsed = stream.take_tool_calls();
    assert_eq!(parsed.calls.len(), 1);
    assert!(parsed.pre_resolved.is_empty());
    assert_eq!(parsed.calls[0].arguments["url"], "https://example.com/a");
    assert_eq!(parsed.calls[0].arguments["text"], "caf\u{00e9}");
    assert_eq!(parsed.calls[0].arguments["emoji"], "\u{1F680}");
    assert_eq!(parsed.calls[0].arguments["quote"], "He said \"hi\"");
}

#[test]
fn tool_call_args_escaped_sequences_across_deltas_are_deserialized() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::ToolCallStart {
        id: "call-1".to_string(),
        name: "Run".to_string(),
        thought_signature: ThoughtSignatureState::Unsigned,
    });
    stream.apply_event(StreamEvent::ToolCallDelta {
        id: "call-1".to_string(),
        arguments: r#"{"unicode":"\u26"#.to_string(),
    });
    stream.apply_event(StreamEvent::ToolCallDelta {
        id: "call-1".to_string(),
        arguments: r#"3A","path":"https:\/\/example.com\/x","msg":"slash\/ok"}"#.to_string(),
    });

    let parsed = stream.take_tool_calls();
    assert_eq!(parsed.calls.len(), 1);
    assert!(parsed.pre_resolved.is_empty());
    assert_eq!(parsed.calls[0].arguments["unicode"], "\u{263A}");
    assert_eq!(parsed.calls[0].arguments["path"], "https://example.com/x");
    assert_eq!(parsed.calls[0].arguments["msg"], "slash/ok");
}

#[tokio::test]
async fn tool_loop_awaiting_approval_then_deny_all_commits() {
    let mut app = test_app();
    app.runtime.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "Edit",
        json!({
            "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n"
        }),
    );
    let thinking =
        crate::thinking::ThinkingPayload::Provided(forge_types::ThinkingMessage::with_signature(
            app.core.model.clone(),
            NonEmptyString::new("thinking").expect("non-empty"),
            "sig".to_string(),
            SystemTime::now(),
        ));
    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });

    match &app.core.state {
        OperationState::ToolLoop(state) => match state.phase {
            ToolLoopPhase::AwaitingApproval(ref approval) => {
                assert_eq!(approval.data().requests.len(), 1);
            }
            ToolLoopPhase::Processing(_) | ToolLoopPhase::Executing(_) => {
                panic!("expected awaiting approval")
            }
        },
        _ => panic!("expected tool loop state"),
    }

    app.resolve_tool_approval(tools::ApprovalDecision::DenyAll);

    assert!(matches!(app.core.state, OperationState::Idle));
    let entries = app.history().entries();
    let thinking_entry = entries.first().expect("thinking message");
    let Message::Thinking(thinking) = thinking_entry.message() else {
        panic!("expected thinking message first");
    };
    assert!(matches!(
        thinking.replay_state(),
        ThinkingReplayState::ClaudeSigned { signature } if signature.as_str() == "sig"
    ));
    let last = app.history().entries().last().expect("tool result");
    assert!(matches!(last.message(), Message::ToolResult(_)));
    assert_eq!(last.message().content(), "Tool call denied by user");
}

#[tokio::test]
async fn run_approval_request_captures_reason_without_changing_summary() {
    let mut app = test_app();
    app.runtime.api_keys.clear();
    app.runtime.tool_settings.policy.mode = tools::ApprovalMode::Default;
    app.runtime.tool_settings.policy.denylist.remove("Run");

    let call = ToolCall::new(
        "call-1",
        "Run",
        json!({
            "command": "echo hello",
            "reason": "Need to verify the local build toolchain."
        }),
    );
    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking: crate::thinking::ThinkingPayload::NotProvided,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });

    let requests = app
        .tool_approval_requests()
        .expect("approval requests should be present");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].summary, "Run command: echo hello".to_string());
    assert_eq!(
        requests[0].reason,
        Some("Need to verify the local build toolchain.".to_string())
    );
}

#[tokio::test]
async fn tool_loop_preserves_order_after_approval() {
    let mut app = test_app();
    app.runtime.api_keys.clear();

    let log = Arc::new(Mutex::new(Vec::new()));
    let registry =
        std::sync::Arc::get_mut(&mut app.runtime.tool_registry).expect("unique registry");
    registry
        .register(Box::new(MockTool::new("mock_a", true, Arc::clone(&log))))
        .expect("register mock_a");
    registry
        .register(Box::new(MockTool::new("mock_b", false, Arc::clone(&log))))
        .expect("register mock_b");
    app.core.tool_definitions = app.runtime.tool_registry.definitions();

    let calls = vec![
        ToolCall::new("call-a", "mock_a", json!({})),
        ToolCall::new("call-b", "mock_b", json!({})),
    ];
    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking: crate::thinking::ThinkingPayload::NotProvided,
        calls,
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });

    match &app.core.state {
        OperationState::ToolLoop(state) => match state.phase {
            ToolLoopPhase::AwaitingApproval(_) => {}
            ToolLoopPhase::Processing(_) | ToolLoopPhase::Executing(_) => {
                panic!("expected awaiting approval")
            }
        },
        _ => panic!("expected tool loop state"),
    }

    assert!(log.lock().expect("log lock").is_empty());

    app.resolve_tool_approval(tools::ApprovalDecision::ApproveAll);
    drive_tool_loop_to_idle(&mut app).await;

    let order = log.lock().expect("log lock").clone();
    assert_eq!(order, vec!["mock_a".to_string(), "mock_b".to_string()]);
}

#[tokio::test]
async fn tool_loop_write_then_read_same_batch() {
    let mut app = test_app();
    app.runtime.tool_settings.policy.mode = tools::ApprovalMode::Permissive;
    app.runtime.api_keys.clear();

    let temp_dir = tempdir().expect("temp dir");
    app.runtime.tool_settings.sandbox =
        tools::sandbox::Sandbox::new(vec![temp_dir.path().to_path_buf()], Vec::new(), false)
            .expect("sandbox");

    let calls = vec![
        ToolCall::new(
            "call-write",
            "Write",
            json!({ "path": "test.txt", "content": "hello" }),
        ),
        ToolCall::new("call-read", "Read", json!({ "path": "test.txt" })),
    ];

    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking: crate::thinking::ThinkingPayload::NotProvided,
        calls,
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });

    // In Permissive mode, neither write_file nor read_file requires approval,
    // so the batch should execute directly without awaiting approval.
    drive_tool_loop_to_idle(&mut app).await;

    let results: Vec<&ToolResult> = app
        .history()
        .entries()
        .iter()
        .filter_map(|entry| match entry.message() {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();

    let write_result = results
        .iter()
        .find(|result| result.tool_call_id == "call-write")
        .expect("write result");
    assert!(!write_result.is_error);
    assert!(write_result.content.contains("Created"));

    let read_result = results
        .iter()
        .find(|result| result.tool_call_id == "call-read")
        .expect("read result");
    assert!(!read_result.is_error);
    assert_eq!(read_result.content, "1| hello");
}

#[tokio::test]
async fn compaction_failure_goes_to_idle_no_retry() {
    let mut app = test_app();

    let config =
        ApiConfig::new(ApiKey::claude("test"), app.core.model.clone()).expect("api config");
    let handle = tokio::spawn(async { Err(anyhow!("boom")) });

    let task = DistillationTask {
        generated_by: "test".to_string(),
        handle,
    };
    app.core.state = OperationState::Distilling(DistillationState::CompletedWithQueued {
        task,
        message: QueuedUserMessage {
            config: config.clone(),
            turn: super::TurnContext::new_for_tests(),
        },
    });

    tokio::task::yield_now().await;
    app.poll_distillation();

    // With transport-layer retries handling transient failures, compaction errors
    // go directly to Idle state (no engine-level retry).
    assert!(matches!(app.core.state, OperationState::Idle));
}

#[test]
fn tool_loop_max_iterations_short_circuits() {
    let mut app = test_app();
    app.core.tool_iterations = app
        .runtime
        .tool_settings
        .limits
        .max_tool_iterations_per_user_turn;
    app.runtime.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "Edit",
        json!({ "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n" }),
    );
    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking: crate::thinking::ThinkingPayload::NotProvided,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });

    assert!(matches!(app.core.state, OperationState::Idle));
    let last = app.history().entries().last().expect("tool result");
    assert!(matches!(last.message(), Message::ToolResult(_)));
    assert_eq!(last.message().content(), "Max tool iterations reached");
}

fn plan_create_call() -> ToolCall {
    ToolCall::new(
        "call-plan-1",
        "Plan",
        json!({
            "subcommand": "create",
            "phases": [
                {
                    "name": "Phase 1",
                    "steps": [
                        { "description": "Step A" },
                        { "description": "Step B" }
                    ]
                }
            ]
        }),
    )
}

fn plan_edit_call() -> ToolCall {
    ToolCall::new(
        "call-plan-edit",
        "Plan",
        json!({
            "subcommand": "edit",
            "justification": "Adding a step",
            "edit_op": {
                "type": "add_step",
                "phase_index": 0,
                "step": { "description": "Step C" }
            }
        }),
    )
}

fn submit_plan_call(app: &mut App, call: ToolCall) {
    app.handle_tool_calls(crate::state::ToolLoopIngress {
        assistant_text: "assistant".to_string(),
        thinking: crate::thinking::ThinkingPayload::NotProvided,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.core.model.clone(),
        step_id: StepId::new(1),
        tool_journal: crate::state::ToolJournalBatch::Absent,
        turn: super::TurnContext::new_for_tests(),
    });
}

#[test]
fn plan_create_enters_plan_approval_state() {
    let mut app = test_app();
    submit_plan_call(&mut app, plan_create_call());

    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));
    assert!(matches!(app.core.plan_state, PlanState::Proposed(_)));
    assert_eq!(app.plan_approval_kind(), Some("create"));
    assert!(app.plan_approval_rendered().is_some());
}

#[test]
fn plan_create_approve_activates_plan() {
    let mut app = test_app();
    app.runtime.api_keys.clear();
    submit_plan_call(&mut app, plan_create_call());
    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));

    app.plan_approval_approve();

    assert!(matches!(app.core.plan_state, PlanState::Active(_)));
    let plan = match &app.core.plan_state {
        PlanState::Active(plan) => plan,
        _ => panic!("expected active plan"),
    };
    assert!(matches!(
        plan.active_step(),
        forge_types::plan::ActiveStepQuery::Active(_)
    ));
}

#[test]
fn plan_create_reject_returns_inactive() {
    let mut app = test_app();
    app.runtime.api_keys.clear();
    submit_plan_call(&mut app, plan_create_call());
    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));

    app.plan_approval_reject();

    assert!(matches!(app.core.plan_state, PlanState::Inactive));
}

#[test]
fn plan_edit_approve_applies_edit() {
    let mut app = test_app();
    app.runtime.api_keys.clear();

    let plan = forge_types::Plan::from_input(vec![forge_types::PhaseInput {
        name: "Phase 1".to_string(),
        steps: vec![
            forge_types::StepInput {
                description: "Step A".to_string(),
                depends_on: vec![],
            },
            forge_types::StepInput {
                description: "Step B".to_string(),
                depends_on: vec![],
            },
        ],
    }])
    .unwrap();
    let mut plan = plan;
    forge_types::plan::editor::activate_step(&mut plan, forge_types::PlanStepId::new(1)).unwrap();
    app.core.plan_state = PlanState::Active(plan);

    submit_plan_call(&mut app, plan_edit_call());
    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));
    assert_eq!(app.plan_approval_kind(), Some("edit"));

    let step_count_before_approve = match &app.core.plan_state {
        PlanState::Active(plan) => plan.step_count(),
        _ => panic!("expected active plan"),
    };
    app.plan_approval_approve();

    assert!(matches!(app.core.plan_state, PlanState::Active(_)));
    assert_eq!(
        match &app.core.plan_state {
            PlanState::Active(plan) => plan.step_count(),
            _ => panic!("expected active plan"),
        },
        step_count_before_approve + 1
    );
}

#[test]
fn plan_edit_reject_reverts() {
    let mut app = test_app();
    app.runtime.api_keys.clear();

    let plan = forge_types::Plan::from_input(vec![forge_types::PhaseInput {
        name: "Phase 1".to_string(),
        steps: vec![
            forge_types::StepInput {
                description: "Step A".to_string(),
                depends_on: vec![],
            },
            forge_types::StepInput {
                description: "Step B".to_string(),
                depends_on: vec![],
            },
        ],
    }])
    .unwrap();
    let mut plan = plan;
    forge_types::plan::editor::activate_step(&mut plan, forge_types::PlanStepId::new(1)).unwrap();
    let original_step_count = plan.step_count();
    app.core.plan_state = PlanState::Active(plan);

    submit_plan_call(&mut app, plan_edit_call());
    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));

    app.plan_approval_reject();

    assert!(matches!(app.core.plan_state, PlanState::Active(_)));
    assert_eq!(
        match &app.core.plan_state {
            PlanState::Active(plan) => plan.step_count(),
            _ => panic!("expected active plan"),
        },
        original_step_count
    );
}

#[test]
fn plan_approval_cancel_reverts() {
    let mut app = test_app();
    submit_plan_call(&mut app, plan_create_call());
    assert!(matches!(app.core.state, OperationState::PlanApproval(_)));

    app.cancel_active_operation();

    assert!(matches!(app.core.plan_state, PlanState::Inactive));
}

#[test]
fn plan_advance_creates_checkpoint() {
    let mut app = test_app();

    // Set up an active plan with step 1 Active.
    let mut plan = forge_types::Plan::from_input(vec![forge_types::PhaseInput {
        name: "Phase 1".to_string(),
        steps: vec![
            forge_types::StepInput {
                description: "Step A".to_string(),
                depends_on: vec![],
            },
            forge_types::StepInput {
                description: "Step B".to_string(),
                depends_on: vec![],
            },
        ],
    }])
    .unwrap();
    forge_types::plan::editor::activate_step(&mut plan, forge_types::PlanStepId::new(1)).unwrap();
    app.core.plan_state = PlanState::Active(plan);

    let step_id = forge_types::PlanStepId::new(1);
    let advance_call = ToolCall::new(
        "call-advance",
        "Plan",
        json!({
            "subcommand": "advance",
            "step_id": 1,
            "outcome": "Done"
        }),
    );

    let resolution = app.resolve_plan_tool_calls(&[advance_call], Vec::new());
    assert_eq!(resolution.pre_resolved.len(), 1);
    assert!(!resolution.pre_resolved[0].is_error);

    let summaries = app.core.checkpoints.summaries();
    assert!(
        summaries
            .iter()
            .any(|s| s.kind == super::checkpoints::CheckpointKind::PlanStep(step_id)),
        "Expected a PlanStep checkpoint for step {step_id:?}"
    );
}

#[test]
fn plan_skip_creates_checkpoint() {
    let mut app = test_app();

    // Set up an active plan with step 1 Active.
    let mut plan = forge_types::Plan::from_input(vec![forge_types::PhaseInput {
        name: "Phase 1".to_string(),
        steps: vec![
            forge_types::StepInput {
                description: "Step A".to_string(),
                depends_on: vec![],
            },
            forge_types::StepInput {
                description: "Step B".to_string(),
                depends_on: vec![],
            },
        ],
    }])
    .unwrap();
    forge_types::plan::editor::activate_step(&mut plan, forge_types::PlanStepId::new(1)).unwrap();
    app.core.plan_state = PlanState::Active(plan);

    let step_id = forge_types::PlanStepId::new(1);
    let skip_call = ToolCall::new(
        "call-skip",
        "Plan",
        json!({
            "subcommand": "skip",
            "step_id": 1,
            "reason": "Not needed"
        }),
    );

    let resolution = app.resolve_plan_tool_calls(&[skip_call], Vec::new());
    assert_eq!(resolution.pre_resolved.len(), 1);
    assert!(!resolution.pre_resolved[0].is_error);

    let summaries = app.core.checkpoints.summaries();
    assert!(
        summaries
            .iter()
            .any(|s| s.kind == super::checkpoints::CheckpointKind::PlanStep(step_id)),
        "Expected a PlanStep checkpoint for step {step_id:?}"
    );
}

// ─── Phase 0: Guardrail tests ───────────────────────────────────────────────

/// Exhaustive truth table for all `(OperationTag, OperationTag)` → `OperationEdge` mappings.
///
/// When the state machine changes (e.g. new edges for ToolRecovery / RecoveryBlocked in Phase 2),
/// this test **must** be updated. That's the point: it's a tripwire.
#[test]
fn operation_transition_edge_matrix_is_exhaustive() {
    use crate::state::{OperationEdge, OperationTag};
    use OperationEdge::{
        EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch,
        ResolvePlanApproval, StartDistillation, StartStreaming,
    };
    use OperationTag::{
        Distilling, Idle, PlanApproval, RecoveryBlocked, Streaming, ToolLoop, ToolRecovery,
    };

    let all_tags = [
        Idle,
        Streaming,
        ToolLoop,
        PlanApproval,
        ToolRecovery,
        RecoveryBlocked,
        Distilling,
    ];

    // Legal transitions: (from, to) → expected edge
    let legal: Vec<(OperationTag, OperationTag, OperationEdge)> = vec![
        (Idle, Streaming, StartStreaming),
        (Idle, Distilling, StartDistillation),
        (Idle, Idle, FinishToolBatch),
        (Streaming, PlanApproval, EnterToolLoopAwaitingApproval),
        (Streaming, ToolLoop, EnterToolLoopExecuting),
        (PlanApproval, ToolLoop, ResolvePlanApproval),
        (PlanApproval, Idle, FinishToolBatch),
        (ToolLoop, Idle, FinishToolBatch),
    ];

    // Verify every legal entry returns the correct edge.
    for (from, to, expected) in &legal {
        assert_eq!(
            App::op_transition_edge(*from, *to),
            Some(*expected),
            "Expected ({from:?}, {to:?}) → {expected:?}"
        );
    }

    // Every other (from, to) pair must return None.
    let is_legal = |from: OperationTag, to: OperationTag| -> bool {
        legal.iter().any(|(f, t, _)| *f == from && *t == to)
    };

    for from in &all_tags {
        for to in &all_tags {
            if !is_legal(*from, *to) {
                assert_eq!(
                    App::op_transition_edge(*from, *to),
                    None,
                    "({from:?}, {to:?}) should have no named edge"
                );
            }
        }
    }
}

/// Cross-check: every legal `(from, to, edge)` triple passes `op_is_legal_transition`.
#[test]
fn every_transition_edge_passes_legality_check() {
    use crate::state::{OperationEdge, OperationTag};
    use OperationEdge::{
        EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch, FinishTurn,
        ResolvePlanApproval, StartDistillation, StartStreaming,
    };
    use OperationTag::{Distilling, Idle, PlanApproval, Streaming, ToolLoop};

    let cases: Vec<(OperationTag, OperationEdge, OperationTag)> = vec![
        (Idle, StartStreaming, Streaming),
        (Idle, StartDistillation, Distilling),
        (Idle, FinishToolBatch, Idle),
        (Streaming, EnterToolLoopAwaitingApproval, PlanApproval),
        (Streaming, EnterToolLoopExecuting, ToolLoop),
        (PlanApproval, ResolvePlanApproval, ToolLoop),
        (PlanApproval, FinishToolBatch, Idle),
        (ToolLoop, FinishToolBatch, Idle),
        // FinishTurn is a same-state edge, not produced by op_transition_edge
        (Idle, FinishTurn, Idle),
    ];

    for (from, edge, to) in &cases {
        assert!(
            App::op_is_legal_transition(*from, *edge, *to),
            "({from:?}, {edge:?}, {to:?}) should be legal"
        );
    }
}

/// Spot-check that impossible transitions are rejected by `op_is_legal_transition`.
#[test]
fn illegal_edges_are_rejected() {
    use crate::state::{OperationEdge, OperationTag};

    let cases: Vec<(OperationTag, OperationEdge, OperationTag)> = vec![
        // Can't start streaming while streaming
        (
            OperationTag::Streaming,
            OperationEdge::StartStreaming,
            OperationTag::Streaming,
        ),
        // Can't distill from tool loop
        (
            OperationTag::ToolLoop,
            OperationEdge::StartDistillation,
            OperationTag::Distilling,
        ),
        // Can't resolve plan approval from idle
        (
            OperationTag::Idle,
            OperationEdge::ResolvePlanApproval,
            OperationTag::ToolLoop,
        ),
        // Distilling can't use FinishToolBatch
        (
            OperationTag::Distilling,
            OperationEdge::FinishToolBatch,
            OperationTag::Idle,
        ),
    ];

    for (from, edge, to) in &cases {
        assert!(
            !App::op_is_legal_transition(*from, *edge, *to),
            "({from:?}, {edge:?}, {to:?}) should be illegal"
        );
    }
}

/// Tool gate disabled → tool calls pre-resolved to errors, app returns to Idle.
#[tokio::test]
async fn tool_gate_disabled_blocks_tool_execution() {
    let mut app = test_app();
    app.runtime.api_keys.clear();
    app.core.tool_gate.disable("test reason");

    set_streaming_state(&mut app);
    assert!(matches!(app.core.state, OperationState::Streaming(_)));

    // Feed a tool call into the streaming message
    if let OperationState::Streaming(ref mut active) = app.core.state {
        active
            .message_mut()
            .apply_event(StreamEvent::ToolCallStart {
                id: "call-gate".to_string(),
                name: "Read".to_string(),
                thought_signature: ThoughtSignatureState::Unsigned,
            });
        active
            .message_mut()
            .apply_event(StreamEvent::ToolCallDelta {
                id: "call-gate".to_string(),
                arguments: r#"{"path":"foo.txt"}"#.to_string(),
            });
    }

    app.finish_streaming(StreamFinishReason::Done);

    // Tool gate disabled → tool calls pre-resolved to errors, returns to Idle
    assert!(
        matches!(app.core.state, OperationState::Idle),
        "Expected Idle after tool gate blocked execution, got {:?}",
        app.core.state.tag()
    );

    // Verify the tool result was an error
    let results: Vec<&ToolResult> = app
        .history()
        .entries()
        .iter()
        .filter_map(|entry| match entry.message() {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();

    assert!(!results.is_empty(), "Expected at least one tool result");
    assert!(
        results.iter().all(|r| r.is_error),
        "All tool results should be errors"
    );
}

/// `commit_tool_batch` requires `JournalStatus` proof — structural enforcement via type system.
#[test]
fn commit_tool_batch_requires_journal_proof() {
    let state_src = include_str!("../state.rs");

    // JournalStatus struct has private fields (no `pub` on inner)
    assert!(
        state_src.contains("pub(crate) struct JournalStatus(ToolBatchId)"),
        "JournalStatus should have a private inner field"
    );

    // Only constructor is `new`
    let new_count = state_src.matches("fn new(id: ToolBatchId) -> Self").count();
    assert_eq!(
        new_count, 1,
        "JournalStatus should have exactly one constructor"
    );

    // commit_tool_batch_without_journal exists as a separate path
    let tool_loop_src = include_str!("tool_loop.rs");
    assert!(
        tool_loop_src.contains("fn commit_tool_batch_without_journal"),
        "commit_tool_batch_without_journal should exist as explicit fallback"
    );
    assert!(
        tool_loop_src.contains("fn commit_tool_batch("),
        "commit_tool_batch should exist with journal_status parameter"
    );
}

/// Source guardrail: `self.state =` only in `app/mod.rs` (op_transition, op_transition_from, op_restore).
///
/// The search needle is built at runtime to prevent this test from self-matching
/// when scanning its own source file.
#[test]
fn self_state_assignment_only_in_authorized_locations() {
    let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    // Runtime-constructed needle avoids self-matching when this file is scanned.
    let needle = ["self", ".core.state", " ="].concat();

    for entry in std::fs::read_dir(&app_dir).expect("read app dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let filename = path.file_name().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path).expect("read source file");

        let count = source
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("//")
                    || trimmed.starts_with("///")
                    || trimmed.starts_with('*')
                {
                    return false;
                }
                trimmed.contains(&needle)
            })
            .count();

        match filename {
            "mod.rs" => {
                assert_eq!(
                    count, 3,
                    "mod.rs: expected 3 state-assignment sites \
                     (op_transition, op_transition_from, op_restore), found {count}"
                );
            }
            _ => {
                assert_eq!(
                    count, 0,
                    "{filename}: expected 0 state-assignment sites, found {count}"
                );
            }
        }
    }
}

/// Source guardrail: `replace_with_idle` usage baseline across `app/` files.
///
/// The search needle is built at runtime to prevent this test from self-matching.
#[test]
fn replace_with_idle_usage_baseline() {
    let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    let needle = ["replace", "_with_idle("].concat();

    for entry in std::fs::read_dir(&app_dir).expect("read app dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let filename = path.file_name().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path).expect("read source file");

        let count = source.matches(&*needle).count();

        match filename {
            "mod.rs" => {
                assert_eq!(
                    count, 1,
                    "mod.rs: expected 1 replace-with-idle (definition), found {count}"
                );
            }
            "commands.rs" => {
                assert_eq!(
                    count, 2,
                    "commands.rs: expected 2 replace-with-idle (cancel + /clear), found {count}"
                );
            }
            "streaming.rs" => {
                assert_eq!(
                    count, 2,
                    "streaming.rs: expected 2 replace-with-idle \
                     (journal abort + finish_streaming), found {count}"
                );
            }
            _ => {
                assert_eq!(
                    count, 0,
                    "{filename}: expected 0 replace-with-idle, found {count}"
                );
            }
        }
    }
}

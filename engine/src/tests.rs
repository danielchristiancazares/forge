//! Unit tests for the engine crate.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use forge_context::StepId;
use futures_util::future::AbortHandle;
use serde_json::json;
use tempfile::tempdir;

use super::*;
use crate::init::DEFAULT_MAX_TOOL_ARGS_BYTES;
use crate::state::DataDirSource;
use crate::ui::DraftInput;

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
    let mut tool_registry = tools::ToolRegistry::default();
    let _ = tools::builtins::register_builtins(
        &mut tool_registry,
        tool_settings.read_limits,
        tool_settings.patch_limits,
        tool_settings.search.clone(),
        tool_settings.webfetch.clone(),
        tool_settings.shell.clone(),
        tool_settings.run_policy,
    );
    let tool_registry = std::sync::Arc::new(tool_registry);
    let tool_definitions = tool_registry.definitions();
    let hidden_tools: std::collections::HashSet<String> = tool_definitions
        .iter()
        .filter(|d| d.hidden)
        .map(|d| d.name.clone())
        .collect();
    let tool_journal = ToolJournal::open_in_memory().expect("in-memory tool journal");
    let tool_file_cache = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    App {
        input: InputState::default(),
        display: Vec::new(),
        display_version: 0,
        should_quit: false,
        view: ViewState::default(),
        configured_model: model.clone(),
        configured_chat_model_override: None,
        configured_code_model_override: None,
        configured_tool_approval_mode: tools::ApprovalMode::Default,
        configured_context_memory_enabled: true,
        configured_ui_options: UiOptions::default(),
        pending_turn_model: None,
        pending_turn_chat_model_override: None,
        pending_turn_code_model_override: None,
        pending_turn_tool_approval_mode: None,
        pending_turn_context_memory_enabled: None,
        pending_turn_ui_options: None,
        settings_model_editor: None,
        settings_model_overrides_editor: None,
        settings_tools_editor: None,
        settings_context_editor: None,
        settings_appearance_editor: None,
        api_keys,
        model: model.clone(),
        tick: 0,
        data_dir,
        context_manager,
        stream_journal,
        state: OperationState::Idle,
        memory_enabled: true,
        output_limits,
        cache_enabled: false,
        openai_options: OpenAIRequestOptions::default(),
        openai_reasoning_effort_explicit: false,
        system_prompts: TEST_SYSTEM_PROMPTS,
        environment: EnvironmentContext::gather(),
        cached_usage_status: None,
        pending_user_message: None,
        tool_definitions,
        hidden_tools,
        tool_registry,
        tool_settings,
        tool_journal,
        tool_journal_disabled_reason: None,
        pending_stream_cleanup: None,
        pending_stream_cleanup_failures: 0,
        pending_tool_cleanup: None,
        pending_tool_cleanup_failures: 0,
        tool_file_cache,
        checkpoints: crate::checkpoints::CheckpointStore::default(),
        tool_iterations: 0,
        history_load_warning_shown: false,
        autosave_warning_shown: false,
        gemini_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        gemini_thinking_enabled: false,
        anthropic_thinking_mode: crate::config::AnthropicThinkingMode::default(),
        anthropic_thinking_effort: crate::config::AnthropicEffort::default(),
        gemini_cache_config: crate::GeminiCacheConfig::default(),
        librarian: None, // No Gemini API key in tests
        input_history: crate::ui::InputHistory::default(),
        last_ui_tick: Instant::now(),
        last_session_autosave: Instant::now(),
        next_journal_cleanup_attempt: Instant::now(),
        session_changes: crate::session_state::SessionChangeLog::default(),
        file_picker: crate::ui::FilePickerState::new(),
        turn_usage: None,
        last_turn_usage: None,
        notification_queue: crate::notifications::NotificationQueue::new(),
        lsp_config: None,
        lsp: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        lsp_snapshot: forge_lsp::DiagnosticsSnapshot::default(),
        pending_diag_check: None,
    }
}

/// Get the last notification text from the display (for test assertions).
fn last_notification(app: &App) -> Option<&str> {
    for item in app.display.iter().rev() {
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
        app.model.clone(),
        rx,
        app.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .stream_journal
        .begin_session(app.model.as_str())
        .expect("journal session");
    app.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: crate::input_modes::TurnContext::new_for_tests(),
    });
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
    app.openai_reasoning_effort_explicit = true;
    app.openai_options = OpenAIRequestOptions::new(
        OpenAIReasoningEffort::Medium,
        app.openai_options.reasoning_summary(),
        app.openai_options.verbosity(),
        app.openai_options.truncation(),
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
        if matches!(app.state, OperationState::Idle) {
            break;
        }
        match app.state {
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
    assert_eq!(app.view.scroll, ScrollState::AutoBottom);
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
    assert_eq!(last_notification(&app), Some("Conversation cleared"));
    assert_eq!(app.input_mode(), InputMode::Normal);
}

#[test]
fn process_command_clear_requests_transcript_clear() {
    let mut app = test_app();
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

    assert!(app.take_clear_transcript());
    assert!(!app.take_clear_transcript());
}

#[test]
fn process_command_settings_opens_settings_modal() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let token = app.command_token().expect("command mode");
        let mut command_mode = app.command_mode(token);
        for c in "settings".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command().expect("take command")
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(app.settings_filter_text(), Some(""));
    assert!(!app.settings_filter_active());
    assert_eq!(app.settings_selected_index(), Some(0));
    assert_eq!(app.settings_detail_view(), None);
    assert_eq!(app.settings_surface(), Some(SettingsSurface::Root));
}

#[test]
fn process_command_settings_shows_guardrail_when_busy() {
    let mut app = test_app();
    set_streaming_state(&mut app);
    app.enter_command_mode();

    let command = {
        let token = app.command_token().expect("command mode");
        let mut command_mode = app.command_mode(token);
        for c in "settings".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command().expect("take command")
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
        let token = app.command_token().expect("command mode");
        let mut command_mode = app.command_mode(token);
        for c in "runtime".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command().expect("take command")
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(app.settings_surface(), Some(SettingsSurface::Runtime));
}

#[test]
fn process_command_resolve_opens_resolve_panel() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let token = app.command_token().expect("command mode");
        let mut command_mode = app.command_mode(token);
        for c in "resolve".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command().expect("take command")
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(app.settings_surface(), Some(SettingsSurface::Resolve));
}

#[test]
fn process_command_validate_opens_validation_panel() {
    let mut app = test_app();
    app.enter_command_mode();

    let command = {
        let token = app.command_token().expect("command mode");
        let mut command_mode = app.command_mode(token);
        for c in "validate".chars() {
            command_mode.push_char(c);
        }
        command_mode.take_command().expect("take command")
    };

    app.process_command(command);

    assert_eq!(app.input_mode(), InputMode::Settings);
    assert_eq!(app.settings_surface(), Some(SettingsSurface::Validate));
}

#[test]
fn settings_open_resolve_surface_from_model_overrides_detail() {
    let mut app = test_app();
    open_model_overrides_settings(&mut app);

    app.settings_open_resolve_surface();

    assert_eq!(app.settings_surface(), Some(SettingsSurface::Resolve));
    assert_eq!(app.settings_detail_view(), None);
}

#[test]
fn settings_open_resolve_surface_blocks_when_detail_has_unsaved_edits() {
    let mut app = test_app();
    open_model_overrides_settings(&mut app);
    app.settings_detail_toggle_selected();

    app.settings_open_resolve_surface();

    assert_eq!(app.settings_surface(), Some(SettingsSurface::Root));
    assert_eq!(
        app.settings_detail_view(),
        Some(SettingsCategory::ModelOverrides)
    );
    assert_eq!(
        last_notification(&app),
        Some("Unsaved settings changes. Press s to save or r to revert before leaving.")
    );
}

#[test]
fn runtime_snapshot_lists_pending_next_turn_settings() {
    let mut app = test_app();
    app.pending_turn_model = Some(ModelName::from_predefined(PredefinedModel::Gpt52Pro));
    app.pending_turn_context_memory_enabled = Some(false);
    app.pending_turn_ui_options = Some(UiOptions {
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
            .any(|item| item == "pending default model: gpt-5.2-pro (next turn)")
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
    app.pending_turn_context_memory_enabled = Some(false);
    app.pending_turn_ui_options = Some(UiOptions {
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
fn resolve_cascade_uses_pending_default_model_for_session_layer() {
    let mut app = test_app();
    app.pending_turn_model = Some(ModelName::from_predefined(PredefinedModel::Gpt52Pro));

    let cascade = app.resolve_cascade();

    let chat_model = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "Chat Model")
        .expect("chat model setting");
    let chat_global = chat_model
        .layers
        .iter()
        .find(|layer| layer.layer == "Global")
        .expect("chat global layer");
    let chat_session = chat_model
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("chat session layer");
    assert_eq!(chat_session.value, "gpt-5.2-pro");
    assert!(chat_session.is_winner);
    assert!(!chat_global.is_winner);

    let code_model = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "Code Model")
        .expect("code model setting");
    let code_global = code_model
        .layers
        .iter()
        .find(|layer| layer.layer == "Global")
        .expect("code global layer");
    let code_session = code_model
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("code session layer");
    assert_eq!(code_session.value, "gpt-5.2-pro");
    assert!(code_session.is_winner);
    assert!(!code_global.is_winner);
}

#[test]
fn resolve_cascade_preserves_model_override_winner_over_pending_default() {
    let mut app = test_app();
    app.pending_turn_model = Some(ModelName::from_predefined(PredefinedModel::Gpt52Pro));
    app.configured_chat_model_override =
        Some(ModelName::from_predefined(PredefinedModel::ClaudeHaiku));

    let cascade = app.resolve_cascade();

    let chat_model = cascade
        .settings
        .iter()
        .find(|setting| setting.setting == "Chat Model")
        .expect("chat model setting");
    let chat_session = chat_model
        .layers
        .iter()
        .find(|layer| layer.layer == "Session")
        .expect("chat session layer");
    assert!(chat_session.value.starts_with("claude-haiku-4-5"));
    assert!(chat_session.is_winner);
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

fn open_model_overrides_settings(app: &mut App) {
    app.enter_settings_mode();
    app.settings_move_down();
    app.settings_move_down();
    app.settings_activate();
}

fn open_context_settings(app: &mut App) {
    app.enter_settings_mode();
    for _ in 0..3 {
        app.settings_move_down();
    }
    app.settings_activate();
}

fn open_tools_settings(app: &mut App) {
    app.enter_settings_mode();
    for _ in 0..4 {
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

    assert_eq!(app.settings_detail_view(), Some(SettingsCategory::Models));
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

    assert_eq!(app.settings_detail_view(), None);
    assert_eq!(app.input_mode(), InputMode::Settings);
}

#[test]
fn settings_usable_model_count_reflects_configured_providers() {
    let mut app = test_app();
    assert_eq!(app.settings_usable_model_count(), 2);

    app.api_keys
        .insert(Provider::OpenAI, SecretString::new("openai".to_string()));
    assert_eq!(app.settings_usable_model_count(), 4);

    app.api_keys
        .insert(Provider::Gemini, SecretString::new("gemini".to_string()));
    assert_eq!(app.settings_usable_model_count(), 6);
}

#[test]
fn settings_activate_models_initializes_editor_snapshot() {
    let mut app = test_app();

    open_models_settings(&mut app);

    assert_eq!(app.settings_detail_view(), Some(SettingsCategory::Models));
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

    let selected_model = ModelName::from_predefined(PredefinedModel::ClaudeHaiku);
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
fn settings_activate_model_overrides_initializes_editor_snapshot() {
    let mut app = test_app();

    open_model_overrides_settings(&mut app);

    assert_eq!(
        app.settings_detail_view(),
        Some(SettingsCategory::ModelOverrides)
    );
    assert_eq!(
        app.settings_model_overrides_editor_snapshot(),
        Some(ModelOverridesEditorSnapshot {
            draft_chat_model: None,
            draft_code_model: None,
            selected: 0,
            dirty: false,
        })
    );
}

#[test]
fn settings_model_overrides_cycle_and_revert_updates_dirty_state() {
    let mut app = test_app();
    open_model_overrides_settings(&mut app);

    app.settings_detail_toggle_selected();
    app.settings_detail_move_down();
    app.settings_detail_toggle_selected();
    assert_eq!(
        app.settings_model_overrides_editor_snapshot(),
        Some(ModelOverridesEditorSnapshot {
            draft_chat_model: Some(ModelName::from_predefined(PredefinedModel::ClaudeOpus)),
            draft_code_model: Some(ModelName::from_predefined(PredefinedModel::ClaudeOpus)),
            selected: 1,
            dirty: true,
        })
    );

    app.settings_revert_edits();
    assert_eq!(
        app.settings_model_overrides_editor_snapshot(),
        Some(ModelOverridesEditorSnapshot {
            draft_chat_model: None,
            draft_code_model: None,
            selected: 1,
            dirty: false,
        })
    );
}

#[test]
fn settings_activate_tools_initializes_editor_snapshot() {
    let mut app = test_app();

    open_tools_settings(&mut app);

    assert_eq!(app.settings_detail_view(), Some(SettingsCategory::Tools));
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

    assert_eq!(app.settings_detail_view(), Some(SettingsCategory::Context));
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
        app.settings_detail_view(),
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
    app.pending_turn_ui_options = Some(pending);

    app.apply_pending_turn_settings();

    assert_eq!(app.ui_options(), pending);
    assert!(!app.settings_pending_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_model_before_request_config() {
    let mut app = test_app();
    let pending_model = ModelName::from_predefined(PredefinedModel::ClaudeHaiku);
    app.pending_turn_model = Some(pending_model.clone());
    app.input = InputState::Insert(DraftInput {
        text: "pending model".to_string(),
        cursor: 13,
    });

    let token = app.insert_token().expect("insert mode");
    let queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    assert_eq!(queued.config.model(), &pending_model);
    assert_eq!(app.model(), pending_model.as_str());
    assert!(!app.settings_pending_model_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_context_before_request_config() {
    let mut app = test_app();
    app.pending_turn_context_memory_enabled = Some(false);
    app.input = InputState::Insert(DraftInput {
        text: "pending context".to_string(),
        cursor: 15,
    });

    let token = app.insert_token().expect("insert mode");
    let _queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    assert!(!app.memory_enabled());
    assert!(!app.settings_pending_context_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_model_override_before_request_config() {
    let mut app = test_app();
    let pending_model = ModelName::from_predefined(PredefinedModel::ClaudeHaiku);
    app.pending_turn_chat_model_override =
        Some(PendingModelOverride::Explicit(pending_model.clone()));
    app.input = InputState::Insert(DraftInput {
        text: "pending override".to_string(),
        cursor: 16,
    });

    let token = app.insert_token().expect("insert mode");
    let queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    assert_eq!(queued.config.model(), &pending_model);
    assert_eq!(app.model(), pending_model.as_str());
    assert!(!app.settings_pending_model_overrides_apply_next_turn());
}

#[test]
fn queue_message_applies_pending_tools_before_request_config() {
    let mut app = test_app();
    app.pending_turn_tool_approval_mode = Some(tools::ApprovalMode::Strict);
    app.input = InputState::Insert(DraftInput {
        text: "pending tools".to_string(),
        cursor: 13,
    });

    let token = app.insert_token().expect("insert mode");
    let _queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    assert_eq!(app.tool_settings.policy.mode, tools::ApprovalMode::Strict);
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
        app.model.clone(),
        rx,
        app.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .stream_journal
        .begin_session(app.model.as_str())
        .expect("journal session");
    app.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: crate::input_modes::TurnContext::new_for_tests(),
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
        app.model.clone(),
        rx,
        app.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .stream_journal
        .begin_session(app.model.as_str())
        .expect("journal session");
    app.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: crate::input_modes::TurnContext::new_for_tests(),
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
        app.model.clone(),
        rx,
        app.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .stream_journal
        .begin_session(app.model.as_str())
        .expect("journal session");
    app.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: crate::input_modes::TurnContext::new_for_tests(),
    });

    for _ in 0..10_000 {
        let _ = tx.try_send(StreamEvent::TextDelta("x".to_string()));
    }

    app.process_stream_events();

    let content_len = app.streaming().expect("still streaming").content().len();
    assert_eq!(content_len, DEFAULT_STREAM_EVENT_BUDGET);
    assert!(content_len < 10_000);
}

#[test]
fn process_stream_events_starts_tool_journal_on_first_tool_call() {
    let mut app = test_app();

    let (tx, rx) = mpsc::channel(1024);
    let streaming = StreamingMessage::new(
        app.model.clone(),
        rx,
        app.tool_settings.limits.max_tool_args_bytes,
    );
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let journal = app
        .stream_journal
        .begin_session(app.model.as_str())
        .expect("journal session");
    app.state = OperationState::Streaming(ActiveStream::Transient {
        message: streaming,
        journal,
        abort_handle,
        tool_call_seq: 0,
        tool_args_journal_bytes: std::collections::HashMap::new(),
        turn: crate::input_modes::TurnContext::new_for_tests(),
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

    let tool_batch_id = match &app.state {
        OperationState::Streaming(ActiveStream::Journaled { tool_batch_id, .. }) => *tool_batch_id,
        _ => panic!("expected journaled stream"),
    };

    let recovered = app
        .tool_journal
        .recover()
        .expect("recover tool batch")
        .expect("pending tool batch");
    assert_eq!(recovered.batch_id, tool_batch_id);
    assert_eq!(recovered.model_name, app.model.as_str());
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
    app.api_keys.clear();
    app.input = InputState::Insert(DraftInput {
        text: "hi".to_string(),
        cursor: 2,
    });

    let token = app.insert_token().expect("insert mode");
    let queued = app.insert_mode(token).queue_message();
    assert!(queued.is_none());
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
    app.input = InputState::Insert(DraftInput {
        text: "test message".to_string(),
        cursor: 12,
    });

    let token = app.insert_token().expect("insert mode");
    let _queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    assert!(app.pending_user_message.is_some());
    let (msg_id, original_text) = app.pending_user_message.as_ref().unwrap();
    assert_eq!(msg_id.as_u64(), 0);
    assert_eq!(original_text, "test message");
}

#[tokio::test]
async fn distillation_not_needed_starts_queued_request() {
    let mut app = test_app();
    app.input = InputState::Insert(DraftInput {
        text: "queued".to_string(),
        cursor: 6,
    });

    let token = app.insert_token().expect("insert mode");
    let queued = app
        .insert_mode(token)
        .queue_message()
        .expect("queued message");

    let result = app.try_start_distillation(Some(queued));

    assert_eq!(result, DistillationStart::NotNeeded);
    assert!(matches!(app.state, OperationState::Streaming(_)));
}

#[test]
fn rollback_pending_user_message_restores_input() {
    let mut app = test_app();

    let content = NonEmptyString::new("my message").expect("non-empty");
    let msg_id = app.push_history_message(Message::user(content));
    app.pending_user_message = Some((msg_id, "my message".to_string()));

    assert_eq!(app.history().len(), 1);
    assert_eq!(app.display.len(), 1);

    app.rollback_pending_user_message();

    assert_eq!(app.history().len(), 0);
    assert_eq!(app.display.len(), 0);
    assert_eq!(app.draft_text(), "my message");
    assert_eq!(app.input_mode(), InputMode::Insert);
    assert!(app.pending_user_message.is_none());
}

#[test]
fn rollback_pending_user_message_no_op_when_empty() {
    let mut app = test_app();

    assert!(app.pending_user_message.is_none());

    app.rollback_pending_user_message();

    assert!(app.draft_text().is_empty());
    assert!(app.pending_user_message.is_none());
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
    app.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "Edit",
        json!({
            "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n"
        }),
    );
    let thinking = Message::thinking_with_signature(
        app.model.clone(),
        NonEmptyString::new("thinking").expect("non-empty"),
        "sig".to_string(),
    );
    app.handle_tool_calls(crate::state::ToolLoopInput {
        assistant_text: "assistant".to_string(),
        thinking_message: Some(thinking),
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.model.clone(),
        step_id: StepId::new(1),
        tool_batch_id: None,
        turn: crate::input_modes::TurnContext::new_for_tests(),
    });

    match &app.state {
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

    assert!(matches!(app.state, OperationState::Idle));
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
    app.api_keys.clear();
    app.tool_settings.policy.mode = tools::ApprovalMode::Default;
    app.tool_settings.policy.denylist.remove("Run");

    let call = ToolCall::new(
        "call-1",
        "Run",
        json!({
            "command": "echo hello",
            "reason": "Need to verify the local build toolchain."
        }),
    );
    app.handle_tool_calls(crate::state::ToolLoopInput {
        assistant_text: "assistant".to_string(),
        thinking_message: None,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.model.clone(),
        step_id: StepId::new(1),
        tool_batch_id: None,
        turn: crate::input_modes::TurnContext::new_for_tests(),
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
    app.api_keys.clear();

    let log = Arc::new(Mutex::new(Vec::new()));
    let registry = std::sync::Arc::get_mut(&mut app.tool_registry).expect("unique registry");
    registry
        .register(Box::new(MockTool::new("mock_a", true, Arc::clone(&log))))
        .expect("register mock_a");
    registry
        .register(Box::new(MockTool::new("mock_b", false, Arc::clone(&log))))
        .expect("register mock_b");
    app.tool_definitions = app.tool_registry.definitions();

    let calls = vec![
        ToolCall::new("call-a", "mock_a", json!({})),
        ToolCall::new("call-b", "mock_b", json!({})),
    ];
    app.handle_tool_calls(crate::state::ToolLoopInput {
        assistant_text: "assistant".to_string(),
        thinking_message: None,
        calls,
        pre_resolved: Vec::new(),
        model: app.model.clone(),
        step_id: StepId::new(1),
        tool_batch_id: None,
        turn: crate::input_modes::TurnContext::new_for_tests(),
    });

    match &app.state {
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
    app.tool_settings.policy.mode = tools::ApprovalMode::Permissive;
    app.api_keys.clear();

    let temp_dir = tempdir().expect("temp dir");
    app.tool_settings.sandbox =
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

    app.handle_tool_calls(crate::state::ToolLoopInput {
        assistant_text: "assistant".to_string(),
        thinking_message: None,
        calls,
        pre_resolved: Vec::new(),
        model: app.model.clone(),
        step_id: StepId::new(1),
        tool_batch_id: None,
        turn: crate::input_modes::TurnContext::new_for_tests(),
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

    let config = ApiConfig::new(ApiKey::claude("test"), app.model.clone()).expect("api config");
    let handle = tokio::spawn(async { Err(anyhow!("boom")) });

    let task = DistillationTask {
        generated_by: "test".to_string(),
        handle,
    };
    app.state = OperationState::Distilling(DistillationState::CompletedWithQueued {
        task,
        message: QueuedUserMessage {
            config: config.clone(),
            turn: crate::input_modes::TurnContext::new_for_tests(),
        },
    });

    tokio::task::yield_now().await;
    app.poll_distillation();

    // With transport-layer retries handling transient failures, compaction errors
    // go directly to Idle state (no engine-level retry).
    assert!(matches!(app.state, OperationState::Idle));
}

#[test]
fn tool_loop_max_iterations_short_circuits() {
    let mut app = test_app();
    app.tool_iterations = app.tool_settings.limits.max_tool_iterations_per_user_turn;
    app.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "Edit",
        json!({ "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n" }),
    );
    app.handle_tool_calls(crate::state::ToolLoopInput {
        assistant_text: "assistant".to_string(),
        thinking_message: None,
        calls: vec![call],
        pre_resolved: Vec::new(),
        model: app.model.clone(),
        step_id: StepId::new(1),
        tool_batch_id: None,
        turn: crate::input_modes::TurnContext::new_for_tests(),
    });

    assert!(matches!(app.state, OperationState::Idle));
    let last = app.history().entries().last().expect("tool result");
    assert!(matches!(last.message(), Message::ToolResult(_)));
    assert_eq!(last.message().content(), "Max tool iterations reached");
}

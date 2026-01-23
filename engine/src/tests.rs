//! Unit tests for the engine crate.

use std::path::PathBuf;
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
        tool_settings.search.clone(),
        tool_settings.webfetch.clone(),
        tool_settings.shell.clone(),
    );
    let tool_registry = std::sync::Arc::new(tool_registry);
    let tool_definitions = tool_registry.definitions();
    let tool_journal = ToolJournal::open_in_memory().expect("in-memory tool journal");
    let tool_file_cache = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    App {
        input: InputState::default(),
        display: Vec::new(),
        display_version: 0,
        should_quit: false,
        view: ViewState::default(),
        api_keys,
        model: model.clone(),
        tick: 0,
        data_dir,
        context_manager,
        stream_journal,
        state: OperationState::Idle,
        context_infinity: true,
        output_limits,
        cache_enabled: false,
        openai_options: OpenAIRequestOptions::default(),
        system_prompts: None,
        cached_usage_status: None,
        pending_user_message: None,
        tool_definitions,
        tool_registry,
        tool_settings,
        tool_journal,
        tool_file_cache,
        tool_iterations: 0,
        history_load_warning_shown: false,
        autosave_warning_shown: false,
        gemini_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        gemini_cache_config: crate::GeminiCacheConfig::default(),
        librarian: None, // No Gemini API key in tests
    }
}

/// Get the last notification text from the display (for test assertions).
fn last_notification(app: &App) -> Option<&str> {
    for item in app.display.iter().rev() {
        if let DisplayItem::Local(msg) = item {
            if msg.role_str() == "system" {
                return Some(msg.content());
            }
        }
    }
    None
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

    fn is_side_effecting(&self) -> bool {
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
        last_notification(&app),
        Some("Switched to GPT - No API key! Set OPENAI_API_KEY")
    );
    assert_eq!(app.input_mode(), InputMode::Normal);
}

#[test]
fn process_stream_events_applies_deltas_and_done() {
    let mut app = test_app();

    // Start streaming using the new architecture
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
    app.state = OperationState::Streaming(ActiveStream {
        message: streaming,
        journal,
        abort_handle,
        tool_batch_id: None,
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
    app.state = OperationState::Streaming(ActiveStream {
        message: streaming,
        journal,
        abort_handle,
        tool_batch_id: None,
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

    // Verify pending_user_message is set
    assert!(app.pending_user_message.is_some());
    let (msg_id, original_text) = app.pending_user_message.as_ref().unwrap();
    assert_eq!(msg_id.as_u64(), 0); // First message
    assert_eq!(original_text, "test message");
}

#[tokio::test]
async fn summarization_not_needed_starts_queued_request() {
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

    let result = app.start_summarization_with_attempt(Some(queued), 1);

    assert_eq!(result, SummarizationStart::NotNeeded);
    assert!(matches!(app.state, OperationState::Streaming(_)));
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
// StreamingMessage Tests
// ========================================================================

#[test]
fn streaming_message_apply_text_delta() {
    let (tx, rx) = mpsc::channel(1024);
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

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

    // Thinking content should not appear in content
    assert_eq!(stream.content(), "visible");
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

    // No content added
    let result = stream.into_message();
    assert!(result.is_err());
}

#[test]
fn streaming_message_provider_and_model() {
    let (_tx, rx) = mpsc::channel(1024);
    let model = ModelName::known(Provider::OpenAI, "gpt-5.2");
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
        name: "run_command".to_string(),
        thought_signature: None,
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

#[tokio::test]
async fn tool_loop_awaiting_approval_then_deny_all_commits() {
    let mut app = test_app();
    app.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "apply_patch",
        json!({
            "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n"
        }),
    );
    app.handle_tool_calls(
        "assistant".to_string(),
        vec![call],
        Vec::new(),
        app.model.clone(),
        StepId::new(1),
        None,
        crate::input_modes::TurnContext::new_for_tests(),
    );

    match &app.state {
        OperationState::ToolLoop(state) => match state.phase {
            ToolLoopPhase::AwaitingApproval(ref approval) => {
                assert_eq!(approval.requests.len(), 1);
            }
            ToolLoopPhase::Executing(_) => panic!("expected awaiting approval"),
        },
        _ => panic!("expected tool loop state"),
    }

    app.resolve_tool_approval(tools::ApprovalDecision::DenyAll);

    assert!(matches!(app.state, OperationState::Idle));
    let last = app.history().entries().last().expect("tool result");
    assert!(matches!(last.message(), Message::ToolResult(_)));
    assert_eq!(last.message().content(), "Tool call denied by user");
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
    app.handle_tool_calls(
        "assistant".to_string(),
        calls,
        Vec::new(),
        app.model.clone(),
        StepId::new(1),
        None,
        crate::input_modes::TurnContext::new_for_tests(),
    );

    match &app.state {
        OperationState::ToolLoop(state) => match state.phase {
            ToolLoopPhase::AwaitingApproval(_) => {}
            ToolLoopPhase::Executing(_) => panic!("expected awaiting approval"),
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
            "write_file",
            json!({ "path": "test.txt", "content": "hello" }),
        ),
        ToolCall::new("call-read", "read_file", json!({ "path": "test.txt" })),
    ];

    app.handle_tool_calls(
        "assistant".to_string(),
        calls,
        Vec::new(),
        app.model.clone(),
        StepId::new(1),
        None,
        crate::input_modes::TurnContext::new_for_tests(),
    );

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
    assert_eq!(read_result.content, "hello");
}

#[tokio::test]
async fn summarization_failure_sets_retry_with_queued_request() {
    let mut app = test_app();

    let content = NonEmptyString::new("alpha").expect("non-empty");
    let msg_id = app.push_history_message(Message::user(content));
    let pending = app
        .context_manager
        .prepare_summarization(&[msg_id])
        .expect("pending summarization");

    let config =
        ApiConfig::new(ApiKey::Claude("test".to_string()), app.model.clone()).expect("api config");
    let handle = tokio::spawn(async { Err(anyhow!("boom")) });

    let task = SummarizationTask {
        scope: pending.scope,
        generated_by: "test".to_string(),
        handle,
        attempt: 1,
    };
    app.state = OperationState::SummarizingWithQueued(SummarizationWithQueuedState {
        task,
        queued: QueuedUserMessage {
            config: config.clone(),
            turn: crate::input_modes::TurnContext::new_for_tests(),
        },
    });

    let before = Instant::now();
    tokio::task::yield_now().await;
    app.poll_summarization();

    match &app.state {
        OperationState::SummarizationRetryWithQueued(state) => {
            assert_eq!(state.retry.attempt, 2);
            assert!(state.retry.ready_at >= before);
            assert_eq!(
                state.queued.config.model().as_str(),
                config.model().as_str()
            );
        }
        _ => panic!("expected retry with queued"),
    }
}

#[test]
fn tool_loop_max_iterations_short_circuits() {
    let mut app = test_app();
    app.tool_iterations = app.tool_settings.limits.max_tool_iterations_per_user_turn;
    app.api_keys.clear();

    let call = ToolCall::new(
        "call-1",
        "apply_patch",
        json!({ "patch": "LP1\nF foo.txt\nT\nhello\n.\nEND\n" }),
    );
    app.handle_tool_calls(
        "assistant".to_string(),
        vec![call],
        Vec::new(),
        app.model.clone(),
        StepId::new(1),
        None,
        crate::input_modes::TurnContext::new_for_tests(),
    );

    assert!(matches!(app.state, OperationState::Idle));
    let last = app.history().entries().last().expect("tool result");
    assert!(matches!(last.message(), Message::ToolResult(_)));
    assert_eq!(last.message().content(), "Max tool iterations reached");
}

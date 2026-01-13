//! Unit tests for the engine crate.

use std::path::PathBuf;

use futures_util::future::AbortHandle;

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
        history_load_warning_shown: false,
        autosave_warning_shown: false,
        empty_send_warning_shown: false,
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
    assert_eq!(app.status_message(), Some("Conversation cleared"));
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
// StreamingMessage Tests
// ========================================================================

#[test]
fn streaming_message_apply_text_delta() {
    let (tx, rx) = mpsc::unbounded_channel();
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
    let (_tx, rx) = mpsc::unbounded_channel();
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("content".to_string()));

    let result = stream.apply_event(StreamEvent::Done);
    assert_eq!(result, Some(StreamFinishReason::Done));
}

#[test]
fn streaming_message_apply_error() {
    let (_tx, rx) = mpsc::unbounded_channel();
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
    let (_tx, rx) = mpsc::unbounded_channel();
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("visible".to_string()));
    stream.apply_event(StreamEvent::ThinkingDelta("thinking...".to_string()));

    // Thinking content should not appear in content
    assert_eq!(stream.content(), "visible");
}

#[test]
fn streaming_message_into_message_success() {
    let (_tx, rx) = mpsc::unbounded_channel();
    let model = Provider::Claude.default_model();
    let mut stream = StreamingMessage::new(model.clone(), rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    stream.apply_event(StreamEvent::TextDelta("Test content".to_string()));

    let message = stream.into_message().expect("should convert to message");
    assert_eq!(message.content(), "Test content");
    assert!(matches!(message, Message::Assistant(_)));
}

#[test]
fn streaming_message_into_message_empty_fails() {
    let (_tx, rx) = mpsc::unbounded_channel();
    let model = Provider::Claude.default_model();
    let stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    // No content added
    let result = stream.into_message();
    assert!(result.is_err());
}

#[test]
fn streaming_message_provider_and_model() {
    let (_tx, rx) = mpsc::unbounded_channel();
    let model = ModelName::known(Provider::OpenAI, "gpt-5.2");
    let stream = StreamingMessage::new(model, rx, DEFAULT_MAX_TOOL_ARGS_BYTES);

    assert_eq!(stream.provider(), Provider::OpenAI);
    assert_eq!(stream.model_name().as_str(), "gpt-5.2");
}

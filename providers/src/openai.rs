use crate::{
    ApiConfig, ApiResponse, ApiUsage, CacheableMessage, Message, OutputLimits, Result,
    SseParseAction, SseParser, StreamEvent, ThoughtSignatureState, ToolDefinition, handle_response,
    http_client, mpsc, process_sse_stream,
    retry::{RetryConfig, send_with_retry},
};
use forge_types::OpenAIReasoningSummary;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const API_URL: &str = "https://api.openai.com/v1/responses";

// ========================================================================
// OpenAI SSE Parser
// ========================================================================

use crate::sse_types::openai as typed;

#[derive(Default)]
struct OpenAIParser {
    /// Track which item_ids have received text deltas (for fallback on .done)
    text_delta_seen: HashSet<String>,
    /// Track which item_ids have received reasoning summary deltas
    reasoning_delta_seen: HashSet<String>,
    /// Track whether the last reasoning summary part for an item ended with a newline
    reasoning_part_last_newline: HashMap<String, bool>,
    /// Map item_id -> call_id for tool calls
    item_to_call: HashMap<String, String>,
    /// Track which call_ids have received argument deltas
    call_has_delta: HashSet<String>,
}

impl OpenAIParser {
    /// Resolve call_id from item_id or direct call_id.
    fn resolve_call_id(&self, item_id: Option<&str>, call_id: Option<&str>) -> Option<String> {
        if let Some(call_id) = call_id {
            return Some(call_id.to_string());
        }
        if let Some(item_id) = item_id {
            if let Some(mapped) = self.item_to_call.get(item_id) {
                return Some(mapped.clone());
            }
            return Some(item_id.to_string());
        }
        None
    }
}

impl SseParser for OpenAIParser {
    fn parse(&mut self, json: &Value) -> SseParseAction {
        // Deserialize into typed event - forward compatible via Unknown variant
        let event: typed::Event = match serde_json::from_value(json.clone()) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("Failed to parse OpenAI SSE event: {e}");
                return SseParseAction::Continue;
            }
        };

        let mut events = Vec::new();

        match event {
            typed::Event::OutputItemAdded { item_id, item } => {
                if let Some(typed::OutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                }) = item
                {
                    // Resolve call_id: prefer call_id, fall back to id
                    let resolved_call_id = call_id.or(id.clone());
                    let Some(call_id) = resolved_call_id.filter(|s| !s.trim().is_empty()) else {
                        return SseParseAction::Error("OpenAI tool call missing id".to_string());
                    };
                    let Some(name) = name.filter(|s| !s.trim().is_empty()) else {
                        return SseParseAction::Error("OpenAI tool call missing name".to_string());
                    };

                    // Track item_id -> call_id mapping for later deltas
                    if let Some(ref item_id) = item_id {
                        self.item_to_call.insert(item_id.clone(), call_id.clone());
                    }
                    if let Some(ref id) = id {
                        self.item_to_call.insert(id.clone(), call_id.clone());
                    }

                    events.push(StreamEvent::ToolCallStart {
                        id: call_id.clone(),
                        name,
                        thought_signature: ThoughtSignatureState::Unsigned,
                    });

                    // Emit initial arguments if present
                    if let Some(args) = arguments.filter(|s| !s.is_empty()) {
                        events.push(StreamEvent::ToolCallDelta {
                            id: call_id.clone(),
                            arguments: args,
                        });
                        self.call_has_delta.insert(call_id);
                    }
                }
            }

            typed::Event::OutputTextDelta { item_id, delta }
            | typed::Event::RefusalDelta { item_id, delta } => {
                if let Some(delta) = delta {
                    if let Some(item_id) = item_id {
                        self.text_delta_seen.insert(item_id);
                    }
                    events.push(StreamEvent::TextDelta(delta));
                }
            }

            typed::Event::OutputTextDone { item_id, text } => {
                // Only emit fallback if no deltas were seen for this item
                let saw_delta = item_id
                    .as_ref()
                    .is_some_and(|id| self.text_delta_seen.contains(id));
                if !saw_delta && let Some(text) = text {
                    events.push(StreamEvent::TextDelta(text));
                }
            }

            typed::Event::ReasoningSummaryDelta { item_id, delta } => {
                if let Some(delta) = delta {
                    if let Some(item_id) = item_id {
                        self.reasoning_delta_seen.insert(item_id);
                    }
                    events.push(StreamEvent::ThinkingDelta(delta));
                }
            }

            typed::Event::ReasoningSummaryDone { item_id, text } => {
                // Only emit fallback if no deltas were seen for this item
                let saw_delta = item_id
                    .as_ref()
                    .is_some_and(|id| self.reasoning_delta_seen.contains(id));
                if !saw_delta && let Some(text) = text {
                    events.push(StreamEvent::ThinkingDelta(text));
                }
            }

            typed::Event::ReasoningSummaryPartAdded { item_id, part } => {
                if let Some(part) = part
                    && let Some(text) = part.text
                {
                    if let Some(ref item_id) = item_id {
                        self.reasoning_delta_seen.insert(item_id.clone());
                        let mut summary = text;
                        if let Some(ended_with_newline) =
                            self.reasoning_part_last_newline.get(item_id)
                        {
                            let starts_with_newline =
                                summary.starts_with('\n') || summary.starts_with('\r');
                            if !*ended_with_newline && !starts_with_newline {
                                summary.insert(0, '\n');
                            }
                        }
                        let ends_with_newline = summary.ends_with('\n') || summary.ends_with('\r');
                        self.reasoning_part_last_newline
                            .insert(item_id.clone(), ends_with_newline);
                        events.push(StreamEvent::ThinkingDelta(summary));
                    } else {
                        events.push(StreamEvent::ThinkingDelta(text));
                    }
                }
            }

            typed::Event::FunctionCallArgumentsDelta {
                item_id,
                call_id,
                delta,
            } => {
                let resolved = self.resolve_call_id(item_id.as_deref(), call_id.as_deref());
                if let Some(delta) = delta {
                    let Some(call_id) = resolved else {
                        return SseParseAction::Error(
                            "OpenAI tool call delta missing id".to_string(),
                        );
                    };
                    events.push(StreamEvent::ToolCallDelta {
                        id: call_id.clone(),
                        arguments: delta,
                    });
                    self.call_has_delta.insert(call_id);
                }
            }

            typed::Event::FunctionCallArgumentsDone {
                item_id,
                call_id,
                arguments,
            } => {
                let resolved = self.resolve_call_id(item_id.as_deref(), call_id.as_deref());
                if let Some(arguments) = arguments {
                    let Some(call_id) = resolved else {
                        return SseParseAction::Error(
                            "OpenAI tool call args missing id".to_string(),
                        );
                    };
                    // Only emit if no deltas were seen for this call
                    if !self.call_has_delta.contains(&call_id) && !arguments.is_empty() {
                        events.push(StreamEvent::ToolCallDelta {
                            id: call_id.clone(),
                            arguments,
                        });
                    }
                    self.call_has_delta.insert(call_id);
                }
            }

            typed::Event::Completed { response } => {
                if let Some(response) = response
                    && let Some(usage) = response.usage
                {
                    let cached_tokens = usage.input_tokens_details.map_or(0, |d| d.cached_tokens);
                    events.push(StreamEvent::Usage(ApiUsage {
                        input_tokens: usage.input_tokens,
                        cache_read_tokens: cached_tokens,
                        cache_creation_tokens: 0, // OpenAI doesn't report this
                        output_tokens: usage.output_tokens,
                    }));
                }
                events.push(StreamEvent::Done);
                return SseParseAction::Emit(events);
            }

            typed::Event::Incomplete { response } => {
                let reason = response
                    .and_then(|r| r.incomplete_details)
                    .and_then(|d| d.reason)
                    .unwrap_or_else(|| "Response incomplete".to_string());
                return SseParseAction::Error(reason);
            }

            typed::Event::Failed { response, error } => {
                let message = error
                    .and_then(|e| e.message)
                    .or_else(|| response.and_then(|r| r.error).and_then(|e| e.message))
                    .unwrap_or_else(|| "Response failed".to_string());
                return SseParseAction::Error(message);
            }

            typed::Event::Error { error } => {
                let message = error
                    .and_then(|e| e.message)
                    .unwrap_or_else(|| "Unknown error".to_string());
                return SseParseAction::Error(message);
            }

            typed::Event::Unknown => {}
        }

        if events.is_empty() {
            SseParseAction::Continue
        } else {
            SseParseAction::Emit(events)
        }
    }

    fn provider_name(&self) -> &'static str {
        "OpenAI"
    }
}

/// Map message role to `OpenAI` Responses API role.
///
/// Per the `OpenAI` Model Spec, the authority hierarchy is:
///   Root > System > Developer > User > Guideline
///
/// "System" level is reserved for `OpenAI`'s own runtime injections.
/// API developers operate at "Developer" level, so `Message::System`
/// maps to "developer" role, not "system".
fn openai_role(msg: &Message) -> &'static str {
    match msg {
        Message::System(_) => "developer",
        Message::User(_) | Message::ToolResult(_) => "user",
        Message::Assistant(_) | Message::Thinking(_) | Message::ToolUse(_) => "assistant",
    }
}

fn build_request_body(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
) -> Value {
    let mut input_items: Vec<Value> = Vec::new();
    for cacheable in messages {
        let msg = &cacheable.message;
        match msg {
            Message::ToolUse(call) => {
                let args_json =
                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string());
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": args_json,
                }));
            }
            Message::ToolResult(result) => {
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": result.tool_call_id,
                    "output": result.content,
                }));
            }
            Message::Thinking(_) => {
                // Thinking is not sent back to the API - stored for UI only.
            }
            _ => {
                input_items.push(json!({
                    "role": openai_role(msg),
                    "content": msg.content(),
                }));
            }
        }
    }

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), json!(config.model().as_str()));
    body.insert("input".to_string(), Value::Array(input_items));
    body.insert(
        "max_output_tokens".to_string(),
        json!(limits.max_output_tokens()),
    );
    body.insert("stream".to_string(), json!(true));

    if let Some(prompt) = system_prompt
        && !prompt.trim().is_empty()
    {
        body.insert("instructions".to_string(), json!(prompt));
    }

    if let Some(tools) = tools
        && !tools.is_empty()
    {
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        body.insert("tools".to_string(), Value::Array(tool_defs));
    }

    let options = config.openai_options();
    body.insert(
        "truncation".to_string(),
        json!(options.truncation().as_str()),
    );

    let model = config.model().as_str();
    if model.starts_with("gpt-5") {
        let mut reasoning = serde_json::Map::new();
        reasoning.insert(
            "effort".to_string(),
            json!(options.reasoning_effort().as_str()),
        );
        if options.reasoning_summary() != OpenAIReasoningSummary::Disabled {
            reasoning.insert(
                "summary".to_string(),
                json!(options.reasoning_summary().as_str()),
            );
        }
        body.insert("reasoning".to_string(), Value::Object(reasoning));
        body.insert(
            "text".to_string(),
            json!({ "verbosity": options.verbosity().as_str() }),
        );
    }

    Value::Object(body)
}

pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let client = http_client();
    let retry_config = RetryConfig::default();

    let body = build_request_body(config, messages, limits, system_prompt, tools);

    let auth_header = format!("Bearer {}", config.api_key());
    let body_json = body.clone();

    // Wrap request with retry logic (REQ-4)
    // Streaming requests omit X-Stainless-Timeout (no total timeout)
    let outcome = send_with_retry(
        || {
            client
                .post(API_URL)
                .header("Authorization", &auth_header)
                .header("content-type", "application/json")
                .json(&body_json)
        },
        None, // No timeout header for streaming
        &retry_config,
    )
    .await;

    let response = match handle_response(outcome, &tx).await? {
        ApiResponse::Success(resp) => resp,
        ApiResponse::StreamTerminated => return Ok(()),
    };

    let mut parser = OpenAIParser::default();
    process_sse_stream(response, &mut parser, &tx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SseParser;
    use forge_types::NonEmptyString;
    use forge_types::{
        ApiKey, OpenAIReasoningEffort, OpenAIReasoningSummary, OpenAIRequestOptions,
        OpenAITextVerbosity, OpenAITruncation, Provider,
    };
    use serde_json::json;

    fn collect_events(json: Value, parser: &mut OpenAIParser) -> Vec<StreamEvent> {
        match parser.parse(&json) {
            super::SseParseAction::Emit(events) => events,
            _ => Vec::new(),
        }
    }

    #[test]
    fn maps_system_message_to_developer_role() {
        let key = ApiKey::openai("test");
        let model = Provider::OpenAI.default_model();
        let config = ApiConfig::new(key, model).unwrap();

        let messages = vec![
            CacheableMessage::plain(Message::system(NonEmptyString::new("Distillate").unwrap())),
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
        ];

        let body = build_request_body(&config, &messages, OutputLimits::new(1024), None, None);

        let input = body.get("input").unwrap().as_array().unwrap();
        assert_eq!(input.len(), 2);
        // Message::System maps to "developer" per OpenAI Model Spec hierarchy
        assert_eq!(input[0]["role"].as_str(), Some("developer"));
        assert_eq!(input[0]["content"].as_str(), Some("Distillate"));
        assert_eq!(input[1]["role"].as_str(), Some("user"));
    }

    #[test]
    fn preserves_explicit_system_prompt() {
        let key = ApiKey::openai("test");
        let model = Provider::OpenAI.default_model();
        let config = ApiConfig::new(key, model).unwrap();

        let messages = vec![CacheableMessage::plain(Message::system(
            NonEmptyString::new("Distillate").unwrap(),
        ))];

        let body = build_request_body(
            &config,
            &messages,
            OutputLimits::new(1024),
            Some("prompt"),
            None,
        );

        assert_eq!(body.get("instructions").unwrap().as_str(), Some("prompt"));
    }

    #[test]
    fn includes_reasoning_summary_when_configured() {
        let key = ApiKey::openai("test");
        let model = Provider::OpenAI.default_model();
        let options = OpenAIRequestOptions::new(
            OpenAIReasoningEffort::Low,
            OpenAIReasoningSummary::Auto,
            OpenAITextVerbosity::High,
            OpenAITruncation::Auto,
        );
        let config = ApiConfig::new(key, model)
            .unwrap()
            .with_openai_options(options);

        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(&config, &messages, OutputLimits::new(1024), None, None);

        let reasoning = body.get("reasoning").unwrap();
        assert_eq!(reasoning["summary"].as_str(), Some("auto"));
    }

    #[test]
    fn omits_reasoning_summary_by_default() {
        let key = ApiKey::openai("test");
        let model = Provider::OpenAI.default_model();
        let config = ApiConfig::new(key, model).unwrap();

        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(&config, &messages, OutputLimits::new(1024), None, None);

        let reasoning = body.get("reasoning").unwrap();
        assert!(reasoning.get("summary").is_none());
    }

    #[test]
    fn emits_tool_call_start_and_args_from_output_item() {
        let mut state = OpenAIParser::default();
        let events = collect_events(
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "item_1",
                    "call_id": "call_1",
                    "name": "Read",
                    "arguments": "{\"path\":\"foo\"}"
                }
            }),
            &mut state,
        );

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallStart { id, name, .. }
                if id == "call_1" && name == "Read"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallDelta { id, arguments }
                if id == "call_1" && arguments == "{\"path\":\"foo\"}"
        ));
    }

    #[test]
    fn maps_argument_deltas_to_call_id_from_item() {
        let mut state = OpenAIParser::default();
        let _ = collect_events(
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "item_1",
                    "call_id": "call_1",
                    "name": "Read"
                }
            }),
            &mut state,
        );

        let events = collect_events(
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "item_1",
                "delta": "{\"path\":\"bar\"}"
            }),
            &mut state,
        );

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallDelta { id, arguments }
                if id == "call_1" && arguments == "{\"path\":\"bar\"}"
        ));
    }

    #[test]
    fn arguments_done_emits_only_when_no_prior_delta() {
        let mut state = OpenAIParser::default();
        let _ = collect_events(
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "item_1",
                    "call_id": "call_1",
                    "name": "Read"
                }
            }),
            &mut state,
        );

        let _ = collect_events(
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "item_1",
                "delta": "{\"path\":\"bar\"}"
            }),
            &mut state,
        );

        let events = collect_events(
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": "item_1",
                "arguments": "{\"path\":\"bar\"}"
            }),
            &mut state,
        );
        assert!(events.is_empty());

        let mut fresh = OpenAIParser::default();
        let _ = collect_events(
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "item_2",
                    "call_id": "call_2",
                    "name": "Read"
                }
            }),
            &mut fresh,
        );
        let events = collect_events(
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": "item_2",
                "arguments": "{\"path\":\"baz\"}"
            }),
            &mut fresh,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallDelta { id, arguments }
                if id == "call_2" && arguments == "{\"path\":\"baz\"}"
        ));
    }

    #[test]
    fn emits_reasoning_summary_delta_as_thinking() {
        let mut state = OpenAIParser::default();
        let events = collect_events(
            json!({
                "type": "response.reasoning_summary_text.delta",
                "delta": "brief summary"
            }),
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ThinkingDelta(text) if text == "brief summary"
        ));
    }

    #[test]
    fn inserts_newline_between_reasoning_summary_parts() {
        let mut state = OpenAIParser::default();
        let _ = collect_events(
            json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": "item_1",
                "part": { "text": "First section." }
            }),
            &mut state,
        );
        let events = collect_events(
            json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": "item_1",
                "part": { "text": "Second section." }
            }),
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ThinkingDelta(text) if text == "\nSecond section."
        ));
    }

    #[test]
    fn emits_reasoning_summary_done_when_no_delta() {
        let mut state = OpenAIParser::default();
        let events = collect_events(
            json!({
                "type": "response.reasoning_summary_text.done",
                "text": "Summary text"
            }),
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ThinkingDelta(text) if text == "Summary text"
        ));
    }

    #[test]
    fn response_completed_emits_usage() {
        let mut state = OpenAIParser::default();
        let action = state.parse(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 1234,
                    "output_tokens": 567,
                    "total_tokens": 1801,
                    "input_tokens_details": { "cached_tokens": 100 },
                    "output_tokens_details": { "reasoning_tokens": 50 }
                }
            }
        }));
        match action {
            super::SseParseAction::Emit(events) => {
                // Should emit Usage followed by Done
                assert_eq!(events.len(), 2);
                match &events[0] {
                    StreamEvent::Usage(usage) => {
                        assert_eq!(usage.input_tokens, 1234);
                        assert_eq!(usage.output_tokens, 567);
                        assert_eq!(usage.cache_read_tokens, 100);
                        assert_eq!(usage.cache_creation_tokens, 0);
                    }
                    _ => panic!("Expected Usage event first"),
                }
                assert!(matches!(&events[1], StreamEvent::Done));
            }
            _ => panic!("Expected Emit action"),
        }
    }

    #[test]
    fn response_completed_without_usage_returns_done() {
        let mut state = OpenAIParser::default();
        let action = state.parse(&json!({
            "type": "response.completed",
            "response": {}
        }));
        // Now emits Done as an event (not SseParseAction::Done) for consistency
        match action {
            super::SseParseAction::Emit(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], StreamEvent::Done));
            }
            _ => panic!("Expected Emit action with Done event"),
        }
    }
}

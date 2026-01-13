//! LLM provider clients with streaming support.
//!
//! This crate handles HTTP communication with Claude and `OpenAI` APIs,
//! including SSE streaming and error handling.

// Pedantic lint configuration - these are intentional design choices
#![allow(clippy::missing_errors_doc)] // Result-returning functions are self-explanatory
#![allow(clippy::missing_panics_doc)] // Panics are documented in assertions

use anyhow::Result;
use forge_types::{
    ApiKey, CacheHint, CacheableMessage, Message, ModelName, OpenAIRequestOptions, OutputLimits,
    Provider, StreamEvent, ToolDefinition,
};
use std::sync::OnceLock;
use std::time::Duration;

// Re-export types that callers need
pub use forge_types;

// ============================================================================
// Shared HTTP Client
// ============================================================================

/// Connection timeout for API requests.
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// Max idle time between SSE chunks before aborting.
const STREAM_IDLE_TIMEOUT_SECS: u64 = 60;

/// Maximum bytes for SSE buffer before aborting (4 MiB).
/// Prevents memory exhaustion from malicious/misbehaving servers.
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Maximum bytes for error body reads (32 KiB).
/// Prevents memory spikes from large error responses.
const MAX_ERROR_BODY_BYTES: usize = 32 * 1024;

/// Shared HTTP client for all provider requests.
///
/// This client is configured with:
/// - Connection timeout: 30 seconds
/// - No read/total timeout (SSE streams can run for extended periods)
/// - Redirects disabled (API endpoints should never redirect)
/// - HTTPS only
///
/// For synchronous requests needing a timeout (like summarization),
/// use [`http_client_with_timeout`] instead.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .build()
            .expect("build shared HTTP client")
    })
}

/// HTTP client with a total request timeout for synchronous operations.
///
/// Use this for non-streaming requests like summarization where you want
/// to bound the total request time.
#[must_use]
pub fn http_client_with_timeout(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .expect("build HTTP client with timeout")
}

fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n");
    let crlf = buffer.windows(4).position(|w| w == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a <= b { (a, 2) } else { (b, 4) }),
        (Some(a), None) => Some((a, 2)),
        (None, Some(b)) => Some((b, 4)),
        (None, None) => None,
    }
}

fn drain_next_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let (pos, delim_len) = find_sse_event_boundary(buffer)?;
    let event = buffer[..pos].to_vec();
    buffer.drain(..pos + delim_len);
    Some(event)
}

fn extract_sse_data(event: &str) -> Option<String> {
    let mut data = String::new();
    let mut found = false;

    for line in event.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);

        if let Some(mut rest) = line.strip_prefix("data:") {
            if let Some(stripped) = rest.strip_prefix(' ') {
                rest = stripped;
            }

            if found {
                data.push('\n');
            }
            data.push_str(rest);
            found = true;
        }
    }

    if found { Some(data) } else { None }
}

/// Read an HTTP error response body with size limits.
/// Prevents memory exhaustion from large error payloads.
async fn read_capped_error_body(response: reqwest::Response) -> String {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ERROR_BODY_BYTES {
            body.truncate(MAX_ERROR_BODY_BYTES);
            let text = String::from_utf8_lossy(&body);
            return format!("{text}...(truncated)");
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

// ============================================================================
// API Configuration
// ============================================================================

/// Configuration for API requests.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}

impl ApiConfig {
    pub fn new(api_key: ApiKey, model: ModelName) -> Result<Self, ApiConfigError> {
        let key_provider = api_key.provider();
        let model_provider = model.provider();
        if key_provider != model_provider {
            return Err(ApiConfigError::ProviderMismatch {
                key: key_provider,
                model: model_provider,
            });
        }

        Ok(Self {
            api_key,
            model,
            openai_options: OpenAIRequestOptions::default(),
        })
    }

    #[must_use]
    pub fn with_openai_options(mut self, options: OpenAIRequestOptions) -> Self {
        self.openai_options = options;
        self
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        self.api_key.provider()
    }

    #[must_use]
    pub fn api_key(&self) -> &str {
        self.api_key.as_str()
    }

    #[must_use]
    pub fn api_key_owned(&self) -> ApiKey {
        self.api_key.clone()
    }

    #[must_use]
    pub fn model(&self) -> &ModelName {
        &self.model
    }

    #[must_use]
    pub fn openai_options(&self) -> OpenAIRequestOptions {
        self.openai_options
    }
}

// ============================================================================
// Streaming API
// ============================================================================

/// Send a chat request and stream the response.
///
/// # Arguments
/// * `config` - API configuration (key, model, options)
/// * `messages` - Conversation history
/// * `limits` - Output token limits (with optional thinking budget)
/// * `system_prompt` - Optional system prompt to inject
/// * `tools` - Optional list of tool definitions for function calling
/// * `on_event` - Callback for streaming events
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()> {
    match config.provider() {
        Provider::Claude => {
            claude::send_message(config, messages, limits, system_prompt, tools, on_event).await
        }
        Provider::OpenAI => {
            openai::send_message(config, messages, limits, system_prompt, tools, on_event).await
        }
    }
}

/// Claude/Anthropic API implementation.
pub mod claude {
    use super::{
        ApiConfig, CacheHint, CacheableMessage, Duration, MAX_SSE_BUFFER_BYTES, Message,
        OutputLimits, Result, STREAM_IDLE_TIMEOUT_SECS, StreamEvent, ToolDefinition,
        drain_next_sse_event, extract_sse_data, http_client, read_capped_error_body,
    };
    use serde_json::json;

    const API_URL: &str = "https://api.anthropic.com/v1/messages";

    /// Build a content block with optional `cache_control`.
    fn content_block(text: &str, cache_hint: CacheHint) -> serde_json::Value {
        match cache_hint {
            CacheHint::None => json!({
                "type": "text",
                "text": text
            }),
            CacheHint::Ephemeral => json!({
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" }
            }),
        }
    }

    fn build_request_body(
        model: &str,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
    ) -> serde_json::Value {
        let mut system_blocks: Vec<serde_json::Value> = Vec::new();
        let mut api_messages: Vec<serde_json::Value> = Vec::new();

        if let Some(prompt) = system_prompt
            && !prompt.trim().is_empty()
        {
            system_blocks.push(content_block(prompt, CacheHint::Ephemeral));
        }

        for cacheable in messages {
            let msg = &cacheable.message;
            let hint = cacheable.cache_hint;

            match msg {
                Message::System(_) => {
                    system_blocks.push(content_block(msg.content(), hint));
                }
                Message::User(_) => {
                    api_messages.push(json!({
                        "role": "user",
                        "content": [content_block(msg.content(), hint)]
                    }));
                }
                Message::Assistant(_) => {
                    // Assistant messages sent as strings, not content blocks.
                    // cache_control can't be applied to assistant messages anyway.
                    api_messages.push(json!({
                        "role": "assistant",
                        "content": msg.content()
                    }));
                }
                Message::ToolUse(call) => {
                    // Tool use is sent as an assistant message with tool_use content block
                    api_messages.push(json!({
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.arguments
                        }]
                    }));
                }
                Message::ToolResult(result) => {
                    // Tool result is sent as a user message with tool_result content block
                    api_messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": result.tool_call_id,
                            "content": result.content,
                            "is_error": result.is_error
                        }]
                    }));
                }
            }
        }

        // Check if history contains Assistant or ToolUse messages.
        // When thinking is enabled, the API requires assistant messages to start with
        // thinking/redacted_thinking blocks. Since we don't store thinking content,
        // we must disable thinking when replaying conversations with assistant messages.
        let has_assistant_history = messages
            .iter()
            .any(|m| matches!(m.message, Message::Assistant(_) | Message::ToolUse(_)));

        let mut body = serde_json::Map::new();
        body.insert("model".into(), json!(model));
        body.insert("max_tokens".into(), json!(limits.max_output_tokens()));
        body.insert("stream".into(), json!(true));
        body.insert("messages".into(), json!(api_messages));

        if !system_blocks.is_empty() {
            body.insert("system".into(), json!(system_blocks));
        }

        // Add tool definitions if provided
        if let Some(tools) = tools
            && !tools.is_empty()
        {
            let tool_schemas: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters
                    })
                })
                .collect();
            body.insert("tools".into(), json!(tool_schemas));
        }

        // Only enable thinking for fresh conversations without assistant history.
        // Conversations with tool use require consistent thinking blocks which we don't store.
        if let Some(budget) = limits.thinking_budget()
            && !has_assistant_history
        {
            body.insert(
                "thinking".into(),
                json!({
                    "type": "enabled",
                    "budget_tokens": budget
                }),
            );
        }

        serde_json::Value::Object(body)
    }

    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        use futures_util::StreamExt;

        let client = http_client();

        let body = build_request_body(
            config.model().as_str(),
            messages,
            limits,
            system_prompt,
            tools,
        );

        let response = client
            .post(API_URL)
            .header("x-api-key", config.api_key())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = read_capped_error_body(response).await;
            on_event(StreamEvent::Error(format!(
                "API error {status}: {error_text}"
            )));
            return Ok(());
        }

        // Process SSE stream
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let saw_done = false;
        // Track current tool call ID for streaming tool arguments
        let mut current_tool_id: Option<String> = None;

        loop {
            let Ok(next) =
                tokio::time::timeout(Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS), stream.next())
                    .await
            else {
                on_event(StreamEvent::Error("Stream idle timeout".to_string()));
                return Ok(());
            };
            let Some(chunk) = next else { break };
            let chunk = chunk?;
            buffer.extend_from_slice(&chunk);

            // Security: prevent unbounded buffer growth
            if buffer.len() > MAX_SSE_BUFFER_BYTES {
                on_event(StreamEvent::Error(
                    "SSE buffer exceeded maximum size (4 MiB)".to_string(),
                ));
                return Ok(());
            }

            while let Some(event) = drain_next_sse_event(&mut buffer) {
                if event.is_empty() {
                    continue;
                }

                let Ok(event) = std::str::from_utf8(&event) else {
                    on_event(StreamEvent::Error(
                        "Received invalid UTF-8 from SSE stream".to_string(),
                    ));
                    return Ok(());
                };

                if let Some(data) = extract_sse_data(event) {
                    if data == "[DONE]" {
                        on_event(StreamEvent::Done);
                        return Ok(());
                    }

                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                        // Handle content_block_start for tool_use
                        if json["type"] == "content_block_start"
                            && let Some(block) = json.get("content_block")
                            && block["type"] == "tool_use"
                        {
                            let id = block["id"].as_str().unwrap_or("").to_string();
                            let name = block["name"].as_str().unwrap_or("").to_string();
                            current_tool_id = Some(id.clone());
                            on_event(StreamEvent::ToolCallStart { id, name });
                        }

                        if json["type"] == "content_block_delta" {
                            if let Some(delta_type) = json["delta"]["type"].as_str() {
                                match delta_type {
                                    "text_delta" => {
                                        if let Some(text) = json["delta"]["text"].as_str() {
                                            on_event(StreamEvent::TextDelta(text.to_string()));
                                        }
                                    }
                                    "thinking_delta" => {
                                        if let Some(thinking) = json["delta"]["thinking"].as_str() {
                                            on_event(StreamEvent::ThinkingDelta(
                                                thinking.to_string(),
                                            ));
                                        }
                                    }
                                    "input_json_delta" => {
                                        // Tool arguments streaming
                                        if let Some(json_chunk) =
                                            json["delta"]["partial_json"].as_str()
                                            && let Some(ref id) = current_tool_id
                                        {
                                            on_event(StreamEvent::ToolCallDelta {
                                                id: id.clone(),
                                                arguments: json_chunk.to_string(),
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            } else if let Some(text) = json["delta"]["text"].as_str() {
                                on_event(StreamEvent::TextDelta(text.to_string()));
                            }
                        }

                        // Reset tool ID when content block ends
                        if json["type"] == "content_block_stop" {
                            current_tool_id = None;
                        }

                        if json["type"] == "message_stop" {
                            on_event(StreamEvent::Done);
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Detect premature EOF (connection closed before message_stop)
        if !saw_done {
            on_event(StreamEvent::Error(
                "Connection closed before stream completed".to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use forge_types::NonEmptyString;
        use forge_types::Provider;

        #[test]
        fn hoists_system_messages_into_system_blocks() {
            let model = Provider::Claude.default_model();
            let limits = OutputLimits::new(1024);

            let messages = vec![
                CacheableMessage::plain(Message::system(NonEmptyString::new("summary").unwrap())),
                CacheableMessage::plain(Message::try_user("hi").unwrap()),
            ];

            let body = build_request_body(model.as_str(), &messages, limits, None, None);

            let system = body.get("system").unwrap().as_array().unwrap();
            assert_eq!(system.len(), 1);
            assert_eq!(system[0]["text"].as_str(), Some("summary"));

            let msgs = body.get("messages").unwrap().as_array().unwrap();
            assert_eq!(msgs.len(), 1);
            assert_eq!(msgs[0]["role"].as_str(), Some("user"));
        }

        #[test]
        fn system_prompt_precedes_system_messages() {
            let model = Provider::Claude.default_model();
            let limits = OutputLimits::new(1024);

            let messages = vec![CacheableMessage::plain(Message::system(
                NonEmptyString::new("summary").unwrap(),
            ))];

            let body = build_request_body(model.as_str(), &messages, limits, Some("prompt"), None);

            let system = body.get("system").unwrap().as_array().unwrap();
            assert_eq!(system.len(), 2);
            assert_eq!(system[0]["text"].as_str(), Some("prompt"));
            assert_eq!(
                system[0]["cache_control"]["type"].as_str(),
                Some("ephemeral")
            );
            assert_eq!(system[1]["text"].as_str(), Some("summary"));
        }
    }
}

/// `OpenAI` API implementation.
pub mod openai {
    use super::{
        ApiConfig, CacheableMessage, Duration, MAX_SSE_BUFFER_BYTES, Message, OutputLimits, Result,
        STREAM_IDLE_TIMEOUT_SECS, StreamEvent, ToolDefinition, drain_next_sse_event,
        extract_sse_data, http_client, read_capped_error_body,
    };
    use serde_json::{Value, json};
    use std::collections::{HashMap, HashSet};

    const API_URL: &str = "https://api.openai.com/v1/responses";

    fn extract_error_message(payload: &Value) -> Option<String> {
        payload
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                payload
                    .get("response")
                    .and_then(|response| response.get("error"))
                    .and_then(|error| error.get("message"))
                    .and_then(|value| value.as_str())
            })
            .map(std::string::ToString::to_string)
    }

    fn extract_incomplete_reason(payload: &Value) -> Option<String> {
        payload
            .get("response")
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| details.get("reason"))
            .and_then(|value| value.as_str())
            .map(std::string::ToString::to_string)
    }

    fn resolve_call_id(
        item_id: Option<&str>,
        call_id: Option<&str>,
        item_to_call: &HashMap<String, String>,
    ) -> Option<String> {
        if let Some(call_id) = call_id {
            return Some(call_id.to_string());
        }
        if let Some(item_id) = item_id {
            if let Some(mapped) = item_to_call.get(item_id) {
                return Some(mapped.clone());
            }
            return Some(item_id.to_string());
        }
        None
    }

    #[derive(Debug, Default)]
    struct OpenAIStreamState {
        saw_text_delta: bool,
        item_to_call: HashMap<String, String>,
        call_has_delta: HashSet<String>,
    }

    enum OpenAIStreamAction {
        Continue,
        Stop,
    }

    fn handle_openai_stream_event<F>(
        json: &Value,
        state: &mut OpenAIStreamState,
        on_event: &mut F,
    ) -> OpenAIStreamAction
    where
        F: FnMut(StreamEvent),
    {
        match json["type"].as_str().unwrap_or("") {
            "response.output_item.added" => {
                let item = json.get("item").or_else(|| json.get("output_item"));
                if let Some(item) = item
                    && item.get("type").and_then(|value| value.as_str()) == Some("function_call")
                {
                    let item_id = item.get("id").and_then(|v| v.as_str());
                    let call_id = item.get("call_id").and_then(|v| v.as_str()).or(item_id);
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(call_id) = call_id {
                        let call_id = call_id.to_string();
                        if let Some(item_id) = item_id {
                            state
                                .item_to_call
                                .insert(item_id.to_string(), call_id.clone());
                        }
                        on_event(StreamEvent::ToolCallStart {
                            id: call_id.clone(),
                            name: name.to_string(),
                        });
                        if let Some(arguments) = item.get("arguments").and_then(|v| v.as_str())
                            && !arguments.is_empty()
                        {
                            on_event(StreamEvent::ToolCallDelta {
                                id: call_id.clone(),
                                arguments: arguments.to_string(),
                            });
                            state.call_has_delta.insert(call_id);
                        }
                    }
                }
                OpenAIStreamAction::Continue
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(delta) = json["delta"].as_str() {
                    state.saw_text_delta = true;
                    on_event(StreamEvent::TextDelta(delta.to_string()));
                }
                OpenAIStreamAction::Continue
            }
            "response.output_text.done" => {
                if !state.saw_text_delta
                    && let Some(text) = json["text"].as_str()
                {
                    on_event(StreamEvent::TextDelta(text.to_string()));
                }
                OpenAIStreamAction::Continue
            }
            "response.function_call_arguments.delta" => {
                let item_id = json.get("item_id").and_then(|v| v.as_str());
                let call_id = json.get("call_id").and_then(|v| v.as_str());
                let resolved = resolve_call_id(item_id, call_id, &state.item_to_call);
                if let Some(delta) = json.get("delta").and_then(|v| v.as_str())
                    && let Some(call_id) = resolved
                {
                    on_event(StreamEvent::ToolCallDelta {
                        id: call_id.clone(),
                        arguments: delta.to_string(),
                    });
                    state.call_has_delta.insert(call_id);
                }
                OpenAIStreamAction::Continue
            }
            "response.function_call_arguments.done" => {
                let item_id = json.get("item_id").and_then(|v| v.as_str());
                let call_id = json.get("call_id").and_then(|v| v.as_str());
                let resolved = resolve_call_id(item_id, call_id, &state.item_to_call);
                if let Some(arguments) = json.get("arguments").and_then(|v| v.as_str())
                    && let Some(call_id) = resolved
                {
                    if !state.call_has_delta.contains(&call_id) && !arguments.is_empty() {
                        on_event(StreamEvent::ToolCallDelta {
                            id: call_id.clone(),
                            arguments: arguments.to_string(),
                        });
                    }
                    state.call_has_delta.insert(call_id);
                }
                OpenAIStreamAction::Continue
            }
            "response.completed" => {
                on_event(StreamEvent::Done);
                OpenAIStreamAction::Stop
            }
            "response.incomplete" => {
                let reason = extract_incomplete_reason(json)
                    .unwrap_or_else(|| "Response incomplete".to_string());
                on_event(StreamEvent::Error(reason));
                OpenAIStreamAction::Stop
            }
            "response.failed" | "error" => {
                let message =
                    extract_error_message(json).unwrap_or_else(|| "Response failed".to_string());
                on_event(StreamEvent::Error(message));
                OpenAIStreamAction::Stop
            }
            _ => OpenAIStreamAction::Continue,
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
            Message::Assistant(_) | Message::ToolUse(_) => "assistant",
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
            body.insert(
                "reasoning".to_string(),
                json!({ "effort": options.reasoning_effort().as_str() }),
            );
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
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        use futures_util::StreamExt;

        let client = http_client();

        let body = build_request_body(config, messages, limits, system_prompt, tools);

        let response = client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", config.api_key()))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = read_capped_error_body(response).await;
            on_event(StreamEvent::Error(format!(
                "API error {status}: {error_text}"
            )));
            return Ok(());
        }

        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let saw_done = false;
        let mut state = OpenAIStreamState::default();
        let mut emit = |event| on_event(event);

        loop {
            let Ok(next) =
                tokio::time::timeout(Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS), stream.next())
                    .await
            else {
                on_event(StreamEvent::Error("Stream idle timeout".to_string()));
                return Ok(());
            };
            let Some(chunk) = next else { break };
            let chunk = chunk?;
            buffer.extend_from_slice(&chunk);

            // Security: prevent unbounded buffer growth
            if buffer.len() > MAX_SSE_BUFFER_BYTES {
                on_event(StreamEvent::Error(
                    "SSE buffer exceeded maximum size (4 MiB)".to_string(),
                ));
                return Ok(());
            }

            while let Some(event) = drain_next_sse_event(&mut buffer) {
                if event.is_empty() {
                    continue;
                }

                let Ok(event) = std::str::from_utf8(&event) else {
                    on_event(StreamEvent::Error(
                        "Received invalid UTF-8 from SSE stream".to_string(),
                    ));
                    return Ok(());
                };

                if let Some(data) = extract_sse_data(event) {
                    if data == "[DONE]" {
                        on_event(StreamEvent::Done);
                        return Ok(());
                    }

                    if let Ok(json) = serde_json::from_str::<Value>(&data)
                        && matches!(
                            handle_openai_stream_event(&json, &mut state, &mut emit),
                            OpenAIStreamAction::Stop
                        )
                    {
                        return Ok(());
                    }
                }
            }
        }

        // Detect premature EOF (connection closed before response.completed)
        if !saw_done {
            on_event(StreamEvent::Error(
                "Connection closed before stream completed".to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use forge_types::NonEmptyString;
        use forge_types::{ApiKey, Provider};
        use serde_json::json;

        fn collect_events(json: Value, state: &mut OpenAIStreamState) -> Vec<StreamEvent> {
            let mut events = Vec::new();
            let mut emit = |event| events.push(event);
            let _ = handle_openai_stream_event(&json, state, &mut emit);
            events
        }

        #[test]
        fn maps_system_message_to_developer_role() {
            let key = ApiKey::OpenAI("test".to_string());
            let model = Provider::OpenAI.default_model();
            let config = ApiConfig::new(key, model).unwrap();

            let messages = vec![
                CacheableMessage::plain(Message::system(NonEmptyString::new("summary").unwrap())),
                CacheableMessage::plain(Message::try_user("hi").unwrap()),
            ];

            let body = build_request_body(&config, &messages, OutputLimits::new(1024), None, None);

            let input = body.get("input").unwrap().as_array().unwrap();
            assert_eq!(input.len(), 2);
            // Message::System maps to "developer" per OpenAI Model Spec hierarchy
            assert_eq!(input[0]["role"].as_str(), Some("developer"));
            assert_eq!(input[0]["content"].as_str(), Some("summary"));
            assert_eq!(input[1]["role"].as_str(), Some("user"));
        }

        #[test]
        fn preserves_explicit_system_prompt() {
            let key = ApiKey::OpenAI("test".to_string());
            let model = Provider::OpenAI.default_model();
            let config = ApiConfig::new(key, model).unwrap();

            let messages = vec![CacheableMessage::plain(Message::system(
                NonEmptyString::new("summary").unwrap(),
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
        fn emits_tool_call_start_and_args_from_output_item() {
            let mut state = OpenAIStreamState::default();
            let events = collect_events(
                json!({
                    "type": "response.output_item.added",
                    "item": {
                        "type": "function_call",
                        "id": "item_1",
                        "call_id": "call_1",
                        "name": "read_file",
                        "arguments": "{\"path\":\"foo\"}"
                    }
                }),
                &mut state,
            );

            assert_eq!(events.len(), 2);
            assert!(matches!(
                &events[0],
                StreamEvent::ToolCallStart { id, name }
                    if id == "call_1" && name == "read_file"
            ));
            assert!(matches!(
                &events[1],
                StreamEvent::ToolCallDelta { id, arguments }
                    if id == "call_1" && arguments == "{\"path\":\"foo\"}"
            ));
        }

        #[test]
        fn maps_argument_deltas_to_call_id_from_item() {
            let mut state = OpenAIStreamState::default();
            let _ = collect_events(
                json!({
                    "type": "response.output_item.added",
                    "item": {
                        "type": "function_call",
                        "id": "item_1",
                        "call_id": "call_1",
                        "name": "read_file"
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
            let mut state = OpenAIStreamState::default();
            let _ = collect_events(
                json!({
                    "type": "response.output_item.added",
                    "item": {
                        "type": "function_call",
                        "id": "item_1",
                        "call_id": "call_1",
                        "name": "read_file"
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

            let mut fresh = OpenAIStreamState::default();
            let _ = collect_events(
                json!({
                    "type": "response.output_item.added",
                    "item": {
                        "type": "function_call",
                        "id": "item_2",
                        "call_id": "call_2",
                        "name": "read_file"
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::Provider;

    #[test]
    fn api_config_rejects_mismatched_provider() {
        let key = ApiKey::Claude("test".to_string());
        let model = Provider::OpenAI.default_model();
        let result = ApiConfig::new(key, model);
        assert!(result.is_err());
    }

    #[test]
    fn api_config_accepts_matching_provider() {
        let key = ApiKey::Claude("test".to_string());
        let model = Provider::Claude.default_model();
        let result = ApiConfig::new(key, model);
        assert!(result.is_ok());
    }

    // ========================================================================
    // SSE Parsing Tests
    // ========================================================================

    mod sse_boundary {
        use super::*;

        #[test]
        fn finds_lf_boundary() {
            let buffer = b"data: hello\n\ndata: world";
            let result = find_sse_event_boundary(buffer);
            assert_eq!(result, Some((11, 2))); // Position of first \n\n, delimiter len 2
        }

        #[test]
        fn finds_crlf_boundary() {
            let buffer = b"data: hello\r\n\r\ndata: world";
            let result = find_sse_event_boundary(buffer);
            assert_eq!(result, Some((11, 4))); // Position of first \r\n\r\n, delimiter len 4
        }

        #[test]
        fn prefers_earlier_lf_over_crlf() {
            // LF boundary comes first
            let buffer = b"data: a\n\ndata: b\r\n\r\n";
            let result = find_sse_event_boundary(buffer);
            assert_eq!(result, Some((7, 2)));
        }

        #[test]
        fn prefers_earlier_crlf_over_lf() {
            // CRLF boundary comes first
            let buffer = b"data: a\r\n\r\ndata: b\n\n";
            let result = find_sse_event_boundary(buffer);
            assert_eq!(result, Some((7, 4)));
        }

        #[test]
        fn returns_none_when_no_boundary() {
            let buffer = b"data: incomplete event\n";
            assert_eq!(find_sse_event_boundary(buffer), None);
        }

        #[test]
        fn returns_none_for_empty_buffer() {
            assert_eq!(find_sse_event_boundary(b""), None);
        }

        #[test]
        fn finds_boundary_at_start() {
            let buffer = b"\n\nrest";
            assert_eq!(find_sse_event_boundary(buffer), Some((0, 2)));
        }
    }

    mod sse_drain {
        use super::*;

        #[test]
        fn drains_single_event() {
            let mut buffer = b"data: hello\n\ndata: world\n\n".to_vec();
            let event = drain_next_sse_event(&mut buffer);
            assert_eq!(event, Some(b"data: hello".to_vec()));
            assert_eq!(buffer, b"data: world\n\n");
        }

        #[test]
        fn drains_multiple_events_sequentially() {
            let mut buffer = b"event: a\n\nevent: b\n\nevent: c\n\n".to_vec();

            let e1 = drain_next_sse_event(&mut buffer);
            assert_eq!(e1, Some(b"event: a".to_vec()));

            let e2 = drain_next_sse_event(&mut buffer);
            assert_eq!(e2, Some(b"event: b".to_vec()));

            let e3 = drain_next_sse_event(&mut buffer);
            assert_eq!(e3, Some(b"event: c".to_vec()));

            let e4 = drain_next_sse_event(&mut buffer);
            assert_eq!(e4, None);
        }

        #[test]
        fn returns_none_for_incomplete_event() {
            let mut buffer = b"data: incomplete".to_vec();
            assert_eq!(drain_next_sse_event(&mut buffer), None);
            // Buffer should remain unchanged
            assert_eq!(buffer, b"data: incomplete");
        }

        #[test]
        fn handles_empty_event() {
            let mut buffer = b"\n\ndata: after\n\n".to_vec();
            let event = drain_next_sse_event(&mut buffer);
            assert_eq!(event, Some(b"".to_vec())); // Empty event
            assert_eq!(buffer, b"data: after\n\n");
        }

        #[test]
        fn handles_crlf_events() {
            let mut buffer = b"data: crlf\r\n\r\nrest".to_vec();
            let event = drain_next_sse_event(&mut buffer);
            assert_eq!(event, Some(b"data: crlf".to_vec()));
            assert_eq!(buffer, b"rest");
        }
    }

    mod sse_extract {
        use super::*;

        #[test]
        fn extracts_single_data_line() {
            let event = "data: hello";
            assert_eq!(extract_sse_data(event), Some("hello".to_string()));
        }

        #[test]
        fn extracts_data_without_space() {
            let event = "data:hello";
            assert_eq!(extract_sse_data(event), Some("hello".to_string()));
        }

        #[test]
        fn extracts_multiline_data() {
            let event = "data: line1\ndata: line2\ndata: line3";
            assert_eq!(
                extract_sse_data(event),
                Some("line1\nline2\nline3".to_string())
            );
        }

        #[test]
        fn ignores_non_data_lines() {
            let event = "event: message\nid: 123\ndata: actual_data\nretry: 1000";
            assert_eq!(extract_sse_data(event), Some("actual_data".to_string()));
        }

        #[test]
        fn returns_none_for_no_data() {
            let event = "event: ping\nid: 456";
            assert_eq!(extract_sse_data(event), None);
        }

        #[test]
        fn handles_empty_data() {
            let event = "data: ";
            assert_eq!(extract_sse_data(event), Some("".to_string()));
        }

        #[test]
        fn handles_data_with_colons() {
            let event = "data: {\"key\": \"value\"}";
            assert_eq!(
                extract_sse_data(event),
                Some("{\"key\": \"value\"}".to_string())
            );
        }

        #[test]
        fn strips_carriage_return_suffix() {
            let event = "data: windows\r";
            assert_eq!(extract_sse_data(event), Some("windows".to_string()));
        }

        #[test]
        fn handles_mixed_line_endings() {
            let event = "data: line1\r\ndata: line2\ndata: line3\r";
            assert_eq!(
                extract_sse_data(event),
                Some("line1\nline2\nline3".to_string())
            );
        }

        #[test]
        fn extracts_done_marker() {
            let event = "data: [DONE]";
            assert_eq!(extract_sse_data(event), Some("[DONE]".to_string()));
        }
    }
}

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
use tokio::sync::mpsc;

// Re-export types that callers need
pub use forge_types;

const CONNECT_TIMEOUT_SECS: u64 = 30;
/// Max idle time between SSE chunks before aborting.
const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 60;

/// Maximum bytes for SSE buffer before aborting (4 MiB).
/// Prevents memory exhaustion from malicious/misbehaving servers.
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Maximum consecutive SSE parse failures before aborting.
const MAX_SSE_PARSE_ERRORS: usize = 3;

/// Maximum characters of bad SSE payload to include in logs.
const MAX_SSE_PARSE_ERROR_PREVIEW: usize = 160;

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
            .unwrap_or_else(|e| {
                // This should never fail in practice (only fails if TLS backend unavailable),
                // but if it does, log and fall back to a default client rather than panicking.
                tracing::error!("Failed to build custom HTTP client: {e}. Using default.");
                reqwest::Client::new()
            })
    })
}

/// HTTP client with a total request timeout for synchronous operations.
///
/// Use this for non-streaming requests like summarization where you want
/// to bound the total request time.
///
/// Returns `Err` if the client cannot be built (e.g., TLS backend unavailable).
pub fn http_client_with_timeout(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
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

enum SseParseAction {
    /// Continue processing, no event to emit
    Continue,
    /// Emit these events and continue
    Emit(Vec<StreamEvent>),
    /// Stream is done (message_stop, response.completed, finishReason=STOP)
    Done,
    /// Fatal error, stop processing
    Error(String),
}

trait SseParser {
    /// Parse a JSON payload and return the action to take.
    fn parse(&mut self, json: &serde_json::Value) -> SseParseAction;

    /// Provider name for error logging.
    fn provider_name(&self) -> &'static str;
}

/// Process an SSE stream using a provider-specific parser.
///
/// This handles the common SSE processing logic:
/// - Timeout handling for idle streams
/// - Buffer management with size limits
/// - UTF-8 validation
/// - Event boundary detection
/// - `[DONE]` marker handling
/// - Parse error tracking with threshold
fn stream_idle_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        let timeout = std::env::var("FORGE_STREAM_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT_SECS);
        Duration::from_secs(timeout)
    })
}

async fn send_event(tx: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> bool {
    tx.send(event).await.is_ok()
}

async fn process_sse_stream<P: SseParser>(
    response: reqwest::Response,
    parser: &mut P,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<()> {
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut parse_errors = 0usize;

    loop {
        let Ok(next) = tokio::time::timeout(stream_idle_timeout(), stream.next()).await else {
            let _ = send_event(tx, StreamEvent::Error("Stream idle timeout".to_string())).await;
            return Ok(());
        };

        let Some(chunk) = next else { break };
        let chunk = chunk?;
        buffer.extend_from_slice(&chunk);

        // Security: prevent unbounded buffer growth
        if buffer.len() > MAX_SSE_BUFFER_BYTES {
            let _ = send_event(
                tx,
                StreamEvent::Error("SSE buffer exceeded maximum size (4 MiB)".to_string()),
            )
            .await;
            return Ok(());
        }

        while let Some(event) = drain_next_sse_event(&mut buffer) {
            if event.is_empty() {
                continue;
            }

            let Ok(event) = std::str::from_utf8(&event) else {
                let _ = send_event(
                    tx,
                    StreamEvent::Error("Received invalid UTF-8 from SSE stream".to_string()),
                )
                .await;
                return Ok(());
            };

            let Some(data) = extract_sse_data(event) else {
                continue;
            };

            if data == "[DONE]" {
                let _ = send_event(tx, StreamEvent::Done).await;
                return Ok(());
            }

            match serde_json::from_str::<serde_json::Value>(&data) {
                Ok(json) => {
                    parse_errors = 0;
                    match parser.parse(&json) {
                        SseParseAction::Continue => {}
                        SseParseAction::Emit(events) => {
                            for event in events {
                                if !send_event(tx, event).await {
                                    return Ok(());
                                }
                            }
                        }
                        SseParseAction::Done => {
                            let _ = send_event(tx, StreamEvent::Done).await;
                            return Ok(());
                        }
                        SseParseAction::Error(msg) => {
                            let _ = send_event(tx, StreamEvent::Error(msg)).await;
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    parse_errors = parse_errors.saturating_add(1);
                    let preview: String = data.chars().take(MAX_SSE_PARSE_ERROR_PREVIEW).collect();
                    tracing::warn!(
                        %e,
                        preview = %preview,
                        provider = parser.provider_name(),
                        "Invalid SSE JSON payload"
                    );
                    if parse_errors >= MAX_SSE_PARSE_ERRORS {
                        let _ = send_event(
                            tx,
                            StreamEvent::Error(format!("Invalid stream payload: {e}")),
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
        }
    }

    // Premature EOF: connection closed without completion signal
    let _ = send_event(
        tx,
        StreamEvent::Error("Connection closed before stream completed".to_string()),
    )
    .await;
    Ok(())
}

/// Read an HTTP error response body with size limits.
/// Prevents memory exhaustion from large error payloads.
pub async fn read_capped_error_body(response: reqwest::Response) -> String {
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

/// Send a chat request and stream the response.
///
/// # Arguments
/// * `config` - API configuration (key, model, options)
/// * `messages` - Conversation history
/// * `limits` - Output token limits (with optional thinking budget)
/// * `system_prompt` - Optional system prompt to inject
/// * `tools` - Optional list of tool definitions for function calling
/// * `gemini_cache` - Optional Gemini cache reference (ignored for other providers)
/// * `tx` - Channel sender for streaming events
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    gemini_cache: Option<&gemini::GeminiCache>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    match config.provider() {
        Provider::Claude => {
            claude::send_message(config, messages, limits, system_prompt, tools, tx).await
        }
        Provider::OpenAI => {
            openai::send_message(config, messages, limits, system_prompt, tools, tx).await
        }
        Provider::Gemini => {
            gemini::send_message(
                config,
                messages,
                limits,
                system_prompt,
                tools,
                gemini_cache,
                tx,
            )
            .await
        }
    }
}

/// Claude/Anthropic API implementation.
pub mod claude {
    use super::{
        ApiConfig, CacheHint, CacheableMessage, Message, OutputLimits, Result, SseParseAction,
        SseParser, StreamEvent, ToolDefinition, http_client, mpsc, process_sse_stream,
        read_capped_error_body, send_event,
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

    // ========================================================================
    // Claude SSE Parser
    // ========================================================================

    #[derive(Default)]
    struct ClaudeParser {
        /// Current tool call ID for streaming tool arguments
        current_tool_id: Option<String>,
    }

    impl SseParser for ClaudeParser {
        fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
            let mut events = Vec::new();

            // Handle content_block_start for tool_use
            if json["type"] == "content_block_start"
                && let Some(block) = json.get("content_block")
                && block["type"] == "tool_use"
            {
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();
                self.current_tool_id = Some(id.clone());
                events.push(StreamEvent::ToolCallStart {
                    id,
                    name,
                    thought_signature: None,
                });
            }

            if json["type"] == "content_block_delta" {
                if let Some(delta_type) = json["delta"]["type"].as_str() {
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) = json["delta"]["text"].as_str() {
                                events.push(StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                        "thinking_delta" => {
                            if let Some(thinking) = json["delta"]["thinking"].as_str() {
                                events.push(StreamEvent::ThinkingDelta(thinking.to_string()));
                            }
                        }
                        "input_json_delta" => {
                            if let Some(json_chunk) = json["delta"]["partial_json"].as_str()
                                && let Some(ref id) = self.current_tool_id
                            {
                                events.push(StreamEvent::ToolCallDelta {
                                    id: id.clone(),
                                    arguments: json_chunk.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                } else if let Some(text) = json["delta"]["text"].as_str() {
                    events.push(StreamEvent::TextDelta(text.to_string()));
                }
            }

            // Reset tool ID when content block ends
            if json["type"] == "content_block_stop" {
                self.current_tool_id = None;
            }

            if json["type"] == "message_stop" {
                return SseParseAction::Done;
            }

            if events.is_empty() {
                SseParseAction::Continue
            } else {
                SseParseAction::Emit(events)
            }
        }

        fn provider_name(&self) -> &'static str {
            "Claude"
        }
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
            let _ = send_event(
                &tx,
                StreamEvent::Error(format!("API error {status}: {error_text}")),
            )
            .await;
            return Ok(());
        }

        let mut parser = ClaudeParser::default();
        process_sse_stream(response, &mut parser, &tx).await
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
        ApiConfig, CacheableMessage, Message, OutputLimits, Result, SseParseAction, SseParser,
        StreamEvent, ToolDefinition, http_client, mpsc, process_sse_stream, read_capped_error_body,
        send_event,
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

    // ========================================================================
    // OpenAI SSE Parser
    // ========================================================================

    #[derive(Default)]
    struct OpenAIParser {
        saw_text_delta: bool,
        item_to_call: HashMap<String, String>,
        call_has_delta: HashSet<String>,
    }

    impl SseParser for OpenAIParser {
        fn parse(&mut self, json: &Value) -> SseParseAction {
            let mut events = Vec::new();

            match json["type"].as_str().unwrap_or("") {
                "response.output_item.added" => {
                    let item = json.get("item").or_else(|| json.get("output_item"));
                    if let Some(item) = item
                        && item.get("type").and_then(|value| value.as_str())
                            == Some("function_call")
                    {
                        let item_id = item.get("id").and_then(|v| v.as_str());
                        let call_id = item.get("call_id").and_then(|v| v.as_str()).or(item_id);
                        let name = item.get("name").and_then(|v| v.as_str());
                        let Some(call_id) = call_id else {
                            return SseParseAction::Error(
                                "OpenAI tool call missing id".to_string(),
                            );
                        };
                        let Some(name) = name.filter(|value| !value.trim().is_empty()) else {
                            return SseParseAction::Error(
                                "OpenAI tool call missing name".to_string(),
                            );
                        };
                        let call_id = call_id.to_string();
                        if call_id.trim().is_empty() {
                            return SseParseAction::Error(
                                "OpenAI tool call missing id".to_string(),
                            );
                        }
                        if let Some(item_id) = item_id {
                            self.item_to_call
                                .insert(item_id.to_string(), call_id.clone());
                        }
                        events.push(StreamEvent::ToolCallStart {
                            id: call_id.clone(),
                            name: name.to_string(),
                            thought_signature: None,
                        });
                        if let Some(arguments) = item.get("arguments").and_then(|v| v.as_str())
                            && !arguments.is_empty()
                        {
                            events.push(StreamEvent::ToolCallDelta {
                                id: call_id.clone(),
                                arguments: arguments.to_string(),
                            });
                            self.call_has_delta.insert(call_id);
                        }
                    }
                }
                "response.output_text.delta" | "response.refusal.delta" => {
                    if let Some(delta) = json["delta"].as_str() {
                        self.saw_text_delta = true;
                        events.push(StreamEvent::TextDelta(delta.to_string()));
                    }
                }
                "response.output_text.done" => {
                    if !self.saw_text_delta
                        && let Some(text) = json["text"].as_str()
                    {
                        events.push(StreamEvent::TextDelta(text.to_string()));
                    }
                }
                "response.function_call_arguments.delta" => {
                    let item_id = json.get("item_id").and_then(|v| v.as_str());
                    let call_id = json.get("call_id").and_then(|v| v.as_str());
                    let resolved = resolve_call_id(item_id, call_id, &self.item_to_call);
                    if let Some(delta) = json.get("delta").and_then(|v| v.as_str())
                        && let Some(call_id) = resolved
                    {
                        events.push(StreamEvent::ToolCallDelta {
                            id: call_id.clone(),
                            arguments: delta.to_string(),
                        });
                        self.call_has_delta.insert(call_id);
                    } else if json.get("delta").and_then(|v| v.as_str()).is_some() {
                        return SseParseAction::Error(
                            "OpenAI tool call delta missing id".to_string(),
                        );
                    }
                }
                "response.function_call_arguments.done" => {
                    let item_id = json.get("item_id").and_then(|v| v.as_str());
                    let call_id = json.get("call_id").and_then(|v| v.as_str());
                    let resolved = resolve_call_id(item_id, call_id, &self.item_to_call);
                    if let Some(arguments) = json.get("arguments").and_then(|v| v.as_str())
                        && let Some(call_id) = resolved
                    {
                        if !self.call_has_delta.contains(&call_id) && !arguments.is_empty() {
                            events.push(StreamEvent::ToolCallDelta {
                                id: call_id.clone(),
                                arguments: arguments.to_string(),
                            });
                        }
                        self.call_has_delta.insert(call_id);
                    } else if json.get("arguments").and_then(|v| v.as_str()).is_some() {
                        return SseParseAction::Error(
                            "OpenAI tool call args missing id".to_string(),
                        );
                    }
                }
                "response.completed" => {
                    return SseParseAction::Done;
                }
                "response.incomplete" => {
                    let reason = extract_incomplete_reason(json)
                        .unwrap_or_else(|| "Response incomplete".to_string());
                    return SseParseAction::Error(reason);
                }
                "response.failed" | "error" => {
                    let message = extract_error_message(json)
                        .unwrap_or_else(|| "Response failed".to_string());
                    return SseParseAction::Error(message);
                }
                _ => {}
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
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
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
            let _ = send_event(
                &tx,
                StreamEvent::Error(format!("API error {status}: {error_text}")),
            )
            .await;
            return Ok(());
        }

        let mut parser = OpenAIParser::default();
        process_sse_stream(response, &mut parser, &tx).await
    }

    #[cfg(test)]
    mod tests {
        use super::SseParser;
        use super::*;
        use forge_types::NonEmptyString;
        use forge_types::{ApiKey, Provider};
        use serde_json::json;

        fn collect_events(json: Value, parser: &mut OpenAIParser) -> Vec<StreamEvent> {
            match parser.parse(&json) {
                super::SseParseAction::Emit(events) => events,
                _ => Vec::new(),
            }
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
            let mut state = OpenAIParser::default();
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
                StreamEvent::ToolCallStart { id, name, .. }
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
            let mut state = OpenAIParser::default();
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
            let mut state = OpenAIParser::default();
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

            let mut fresh = OpenAIParser::default();
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

/// Google Gemini API implementation.
pub mod gemini {
    use super::{
        ApiConfig, CacheableMessage, Message, OutputLimits, Result, SseParseAction, SseParser,
        StreamEvent, ToolDefinition, http_client, mpsc, process_sse_stream, read_capped_error_body,
        send_event,
    };
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use uuid::Uuid;

    const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

    // ============================================================================
    // Context Caching Types
    // ============================================================================

    /// Active Gemini cache reference.
    ///
    /// Gemini uses explicit caching where a cache object is created via API
    /// and then referenced in subsequent requests via `cachedContent` field.
    #[derive(Debug, Clone)]
    pub struct GeminiCache {
        /// Cache name returned by API (e.g., "cachedContents/abc123")
        pub name: String,
        /// When this cache expires (UTC)
        pub expire_time: DateTime<Utc>,
        /// Hash of cached system prompt (for detecting changes)
        pub system_prompt_hash: u64,
    }

    impl GeminiCache {
        /// Check if this cache has expired.
        #[must_use]
        pub fn is_expired(&self) -> bool {
            Utc::now() >= self.expire_time
        }

        /// Check if this cache matches the given system prompt.
        #[must_use]
        pub fn matches_prompt(&self, prompt: &str) -> bool {
            hash_prompt(prompt) == self.system_prompt_hash
        }
    }

    /// Configuration for Gemini caching.
    #[derive(Debug, Clone, Default)]
    pub struct GeminiCacheConfig {
        /// Whether caching is enabled
        pub enabled: bool,
        /// TTL in seconds for cached content (default: 3600 = 1 hour)
        pub ttl_seconds: u32,
    }

    /// Hash a system prompt for comparison.
    fn hash_prompt(prompt: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        prompt.hash(&mut hasher);
        hasher.finish()
    }

    /// Check if a prompt is large enough to cache.
    ///
    /// Gemini requires minimum token counts:
    /// - Gemini 3 Pro: 4,096 tokens
    /// - Gemini Flash models: 1,024 tokens
    fn should_cache_prompt(prompt: &str, model: &str) -> bool {
        let min_tokens = if model.contains("flash") { 1024 } else { 4096 };
        // Rough estimate: 1 token ≈ 4 characters
        prompt.len() / 4 >= min_tokens
    }

    /// Create a cached content object with the system prompt.
    ///
    /// This calls the Gemini cachedContents API to create a persistent cache
    /// that can be referenced in subsequent requests.
    ///
    /// # Note
    /// The cachedContents endpoint uses camelCase (unlike generateContent
    /// which mixes snake_case and camelCase).
    pub async fn create_cache(
        api_key: &str,
        model: &str,
        system_prompt: &str,
        ttl_seconds: u32,
    ) -> Result<GeminiCache> {
        // Check if prompt meets minimum token threshold
        if !should_cache_prompt(system_prompt, model) {
            anyhow::bail!(
                "System prompt too short for caching (minimum ~4096 tokens for Pro models)"
            );
        }

        let url = format!("{API_BASE}/cachedContents");

        // NOTE: cachedContents endpoint uses camelCase throughout
        let body = json!({
            "model": format!("models/{}", model),
            "systemInstruction": {
                "parts": [{ "text": system_prompt }]
            },
            "ttl": format!("{}s", ttl_seconds)
        });

        let response = http_client()
            .post(&url)
            .header("x-goog-api-key", api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = read_capped_error_body(response).await;
            anyhow::bail!("Failed to create cache: {status} - {error_text}");
        }

        let data: Value = response.json().await?;

        // Parse the response
        let name = data["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' in cache response"))?
            .to_string();

        let expire_time_str = data["expireTime"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'expireTime' in cache response"))?;

        let expire_time = DateTime::parse_from_rfc3339(expire_time_str)
            .map_err(|e| anyhow::anyhow!("Invalid expireTime format: {e}"))?
            .with_timezone(&Utc);

        tracing::info!("Created Gemini cache: {name} (expires: {expire_time})");

        Ok(GeminiCache {
            name,
            expire_time,
            system_prompt_hash: hash_prompt(system_prompt),
        })
    }

    /// Build a content part for Gemini API.
    fn text_part(text: &str) -> Value {
        json!({ "text": text })
    }

    fn remove_additional_properties(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("additionalProperties");
                for value in map.values_mut() {
                    remove_additional_properties(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    remove_additional_properties(value);
                }
            }
            _ => {}
        }
    }

    /// Build the request body for Gemini API.
    ///
    /// Note: Gemini API uses mixed casing:
    /// - `system_instruction` (snake_case)
    /// - `generationConfig` (camelCase)
    /// - `contents`, `tools` (lowercase)
    /// - `cachedContent` (camelCase) for cache references
    fn build_request_body(
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
        thinking_enabled: bool,
        cache: Option<&GeminiCache>,
    ) -> Value {
        let mut contents: Vec<Value> = Vec::new();

        let mut index = 0;
        while index < messages.len() {
            let msg = &messages[index].message;
            match msg {
                Message::System(_) => {
                    // System messages go into contents as user messages for Gemini
                    // (main system prompt uses top-level system_instruction)
                    contents.push(json!({
                        "role": "user",
                        "parts": [text_part(msg.content())]
                    }));
                    index += 1;
                }
                Message::User(_) => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [text_part(msg.content())]
                    }));
                    index += 1;
                }
                Message::Assistant(_) => {
                    contents.push(json!({
                        "role": "model",
                        "parts": [text_part(msg.content())]
                    }));
                    index += 1;
                }
                Message::ToolUse(_) => {
                    // Group consecutive tool calls into a single model content entry.
                    let mut parts: Vec<Value> = Vec::new();
                    while index < messages.len() {
                        match &messages[index].message {
                            Message::ToolUse(call) => {
                                let mut part = serde_json::Map::new();
                                part.insert(
                                    "functionCall".into(),
                                    json!({
                                        "name": call.name,
                                        "args": call.arguments
                                    }),
                                );
                                if let Some(signature) = call.thought_signature.as_ref() {
                                    part.insert("thoughtSignature".into(), json!(signature));
                                }
                                parts.push(Value::Object(part));
                                index += 1;
                            }
                            _ => break,
                        }
                    }
                    contents.push(json!({
                        "role": "model",
                        "parts": parts
                    }));
                }
                Message::ToolResult(_) => {
                    // Group consecutive tool results into a single user content entry.
                    let mut parts: Vec<Value> = Vec::new();
                    while index < messages.len() {
                        match &messages[index].message {
                            Message::ToolResult(result) => {
                                parts.push(json!({
                                    "functionResponse": {
                                        "name": result.tool_name.clone(),
                                        "response": {
                                            "result": result.content
                                        }
                                    }
                                }));
                                index += 1;
                            }
                            _ => break,
                        }
                    }
                    contents.push(json!({
                        "role": "user",
                        "parts": parts
                    }));
                }
            }
        }

        let mut body = serde_json::Map::new();
        body.insert("contents".into(), json!(contents));

        // If cache is provided, reference it instead of inline system_instruction
        // (the system prompt is already in the cache)
        if let Some(cache) = cache {
            body.insert("cachedContent".into(), json!(cache.name));
        } else if let Some(prompt) = system_prompt
            && !prompt.trim().is_empty()
        {
            // System instruction uses snake_case
            body.insert(
                "system_instruction".into(),
                json!({
                    "parts": [text_part(prompt)]
                }),
            );
        }

        // Generation config uses camelCase
        let mut gen_config = serde_json::Map::new();
        gen_config.insert("maxOutputTokens".into(), json!(limits.max_output_tokens()));
        gen_config.insert("temperature".into(), json!(1.0));

        // Add thinking config if enabled (Gemini 3 Pro uses thinkingLevel)
        if thinking_enabled {
            gen_config.insert(
                "thinkingConfig".into(),
                json!({
                    "thinkingLevel": "high",
                    "includeThoughts": true
                }),
            );
        }

        body.insert("generationConfig".into(), Value::Object(gen_config));

        // Add tool definitions if provided
        if let Some(tools) = tools
            && !tools.is_empty()
        {
            let function_declarations: Vec<Value> = tools
                .iter()
                .map(|t| {
                    let mut parameters = t.parameters.clone();
                    remove_additional_properties(&mut parameters);
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": parameters
                    })
                })
                .collect();
            body.insert(
                "tools".into(),
                json!([{
                    "functionDeclarations": function_declarations
                }]),
            );
        }

        Value::Object(body)
    }

    // ========================================================================
    // Gemini SSE Parser
    // ========================================================================

    /// Map Gemini finishReason to SseParseAction.
    fn handle_finish_reason(reason: &str) -> Option<SseParseAction> {
        match reason {
            "STOP" | "MAX_TOKENS" => Some(SseParseAction::Done),
            "SAFETY" => Some(SseParseAction::Error(
                "Content filtered by safety settings".to_string(),
            )),
            "RECITATION" => Some(SseParseAction::Error(
                "Response blocked: recitation".to_string(),
            )),
            "LANGUAGE" => Some(SseParseAction::Error("Unsupported language".to_string())),
            "BLOCKLIST" => Some(SseParseAction::Error(
                "Content contains blocked terms".to_string(),
            )),
            "PROHIBITED_CONTENT" => Some(SseParseAction::Error(
                "Prohibited content detected".to_string(),
            )),
            "SPII" => Some(SseParseAction::Error("Sensitive PII detected".to_string())),
            "MALFORMED_FUNCTION_CALL" => Some(SseParseAction::Error(
                "Invalid function call generated".to_string(),
            )),
            "MISSING_THOUGHT_SIGNATURE" => Some(SseParseAction::Error(
                "Missing thought signature in request".to_string(),
            )),
            "TOO_MANY_TOOL_CALLS" => Some(SseParseAction::Error(
                "Too many consecutive tool calls".to_string(),
            )),
            "UNEXPECTED_TOOL_CALL" => Some(SseParseAction::Error(
                "Tool call but no tools enabled".to_string(),
            )),
            "OTHER" => Some(SseParseAction::Error(
                "Generation stopped: unknown reason".to_string(),
            )),
            _ => None, // Unknown reason, continue processing
        }
    }

    /// Parser state for Gemini SSE streams.
    #[derive(Default)]
    struct GeminiParser;

    impl SseParser for GeminiParser {
        fn parse(&mut self, json: &Value) -> SseParseAction {
            // Check for error response
            if let Some(error) = json.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                return SseParseAction::Error(message.to_string());
            }

            let mut events = Vec::new();

            // Process candidates
            if let Some(candidates) = json.get("candidates").and_then(|v| v.as_array()) {
                for candidate in candidates {
                    // Check finish reason
                    if let Some(reason) = candidate.get("finishReason").and_then(|v| v.as_str())
                        && let Some(action) = handle_finish_reason(reason)
                    {
                        return action;
                    }

                    // Process content parts
                    if let Some(content) = candidate.get("content")
                        && let Some(parts) = content.get("parts").and_then(|v| v.as_array())
                    {
                        for part in parts {
                            // Check for thinking content
                            let is_thought =
                                part.get("thought").and_then(Value::as_bool) == Some(true);

                            // Text content
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                if is_thought {
                                    events.push(StreamEvent::ThinkingDelta(text.to_string()));
                                } else {
                                    events.push(StreamEvent::TextDelta(text.to_string()));
                                }
                            }

                            // Function call
                            if let Some(func_call) = part.get("functionCall") {
                                let name = func_call
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let args = func_call.get("args").cloned().unwrap_or(json!({}));

                                // Generate UUID for tool call ID (Gemini doesn't provide one)
                                let id = format!("call_{}", Uuid::new_v4());

                                events.push(StreamEvent::ToolCallStart {
                                    id: id.clone(),
                                    name,
                                    thought_signature: None,
                                });

                                // Send arguments as a single delta
                                if let Ok(args_str) = serde_json::to_string(&args) {
                                    events.push(StreamEvent::ToolCallDelta {
                                        id,
                                        arguments: args_str,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            if events.is_empty() {
                SseParseAction::Continue
            } else {
                SseParseAction::Emit(events)
            }
        }

        fn provider_name(&self) -> &'static str {
            "Gemini"
        }
    }

    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
        cache: Option<&GeminiCache>,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let client = http_client();
        let model = config.model().as_str();
        let url = format!("{API_BASE}/models/{model}:streamGenerateContent?alt=sse");

        // Check if thinking is enabled based on limits (temporary - will use config later)
        let thinking_enabled = limits.thinking_budget().is_some();

        let body = build_request_body(
            messages,
            limits,
            system_prompt,
            tools,
            thinking_enabled,
            cache,
        );

        let response = client
            .post(&url)
            .header("x-goog-api-key", config.api_key())
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = read_capped_error_body(response).await;
            let _ = send_event(
                &tx,
                StreamEvent::Error(format!("API error {status}: {error_text}")),
            )
            .await;
            return Ok(());
        }

        let mut parser = GeminiParser;
        process_sse_stream(response, &mut parser, &tx).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn contains_additional_properties(value: &Value) -> bool {
            match value {
                Value::Object(map) => {
                    map.contains_key("additionalProperties")
                        || map.values().any(contains_additional_properties)
                }
                Value::Array(values) => values.iter().any(contains_additional_properties),
                _ => false,
            }
        }

        #[test]
        fn builds_request_with_system_instruction() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(
                &messages,
                limits,
                Some("You are helpful"),
                None,
                false,
                None,
            );

            assert!(body.get("system_instruction").is_some());
            let sys = body.get("system_instruction").unwrap();
            assert_eq!(sys["parts"][0]["text"], "You are helpful");
        }

        #[test]
        fn builds_request_with_generation_config() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(8192);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let gen_config = body.get("generationConfig").unwrap();
            assert_eq!(gen_config["maxOutputTokens"], 8192);
            assert_eq!(gen_config["temperature"], 1.0);
        }

        #[test]
        fn builds_request_with_thinking_config() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(8192);

            let body = build_request_body(&messages, limits, None, None, true, None);

            let gen_config = body.get("generationConfig").unwrap();
            let thinking = gen_config.get("thinkingConfig").unwrap();
            assert_eq!(thinking["thinkingLevel"], "high");
            assert_eq!(thinking["includeThoughts"], true);
        }

        #[test]
        fn builds_request_with_tools() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(4096);

            let tools = vec![forge_types::ToolDefinition::new(
                "get_weather",
                "Get weather for a location",
                json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string" }
                    }
                }),
            )];

            let body = build_request_body(&messages, limits, None, Some(&tools), false, None);

            let tools_json = body.get("tools").unwrap();
            let decls = &tools_json[0]["functionDeclarations"];
            assert_eq!(decls[0]["name"], "get_weather");
        }

        #[test]
        fn strips_additional_properties_from_tool_schemas() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(4096);

            let tools = vec![forge_types::ToolDefinition::new(
                "complex_tool",
                "Tool with nested schema",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "location": { "type": "string" },
                        "options": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "unit": { "type": "string" }
                            }
                        },
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "value": { "type": "string" }
                                }
                            }
                        }
                    }
                }),
            )];

            let body = build_request_body(&messages, limits, None, Some(&tools), false, None);

            let params = &body["tools"][0]["functionDeclarations"][0]["parameters"];
            assert!(!contains_additional_properties(params));
        }

        #[test]
        fn maps_tool_use_to_function_call() {
            let call = forge_types::ToolCall::new("call_123", "read_file", json!({"path": "foo"}));
            let messages = vec![CacheableMessage::plain(Message::tool_use(call))];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents[0]["role"], "model");
            let func_call = &contents[0]["parts"][0]["functionCall"];
            assert_eq!(func_call["name"], "read_file");
        }

        #[test]
        fn groups_tool_calls_and_preserves_thought_signature() {
            let call = forge_types::ToolCall::new_with_thought_signature(
                "call_1",
                "read_file",
                json!({"path": "foo"}),
                Some("sig_1".to_string()),
            );
            let second = forge_types::ToolCall::new("call_2", "list_dir", json!({}));
            let messages = vec![
                CacheableMessage::plain(Message::tool_use(call)),
                CacheableMessage::plain(Message::tool_use(second)),
            ];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents.len(), 1);
            let parts = contents[0]["parts"].as_array().unwrap();
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0]["thoughtSignature"], "sig_1");
            assert!(parts[1].get("thoughtSignature").is_none());
        }

        #[test]
        fn groups_tool_results_into_single_user_message() {
            let result_a =
                forge_types::ToolResult::success("call_1", "read_file", "file contents here");
            let result_b =
                forge_types::ToolResult::success("call_2", "list_dir", "dir contents here");
            let messages = vec![
                CacheableMessage::plain(Message::tool_result(result_a)),
                CacheableMessage::plain(Message::tool_result(result_b)),
            ];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents.len(), 1);
            assert_eq!(contents[0]["role"], "user");
            let parts = contents[0]["parts"].as_array().unwrap();
            assert_eq!(parts.len(), 2);
            // Gemini uses tool_name for functionResponse.name
            assert_eq!(parts[0]["functionResponse"]["name"], "read_file");
            assert_eq!(parts[1]["functionResponse"]["name"], "list_dir");
        }

        #[test]
        fn maps_tool_result_to_function_response() {
            let result =
                forge_types::ToolResult::success("call_1", "read_file", "file contents here");
            let messages = vec![CacheableMessage::plain(Message::tool_result(result))];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents[0]["role"], "user");
            let func_resp = &contents[0]["parts"][0]["functionResponse"];
            // Gemini uses tool_name for functionResponse.name
            assert_eq!(func_resp["name"], "read_file");
        }

        #[test]
        fn handle_finish_reason_stop() {
            let action = handle_finish_reason("STOP");
            assert!(matches!(action, Some(SseParseAction::Done)));
        }

        #[test]
        fn handle_finish_reason_safety() {
            let action = handle_finish_reason("SAFETY");
            assert!(matches!(action, Some(SseParseAction::Error(_))));
        }

        #[test]
        fn handle_finish_reason_unknown() {
            let action = handle_finish_reason("UNKNOWN_REASON");
            assert!(action.is_none());
        }

        #[test]
        fn builds_request_with_cache_reference() {
            use chrono::TimeZone;

            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::new(4096);

            let cache = GeminiCache {
                name: "cachedContents/abc123".to_string(),
                expire_time: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
                system_prompt_hash: 12345,
            };

            let body = build_request_body(
                &messages,
                limits,
                Some("You are helpful"), // Should be ignored when cache present
                None,
                false,
                Some(&cache),
            );

            // Should have cachedContent reference
            assert_eq!(body.get("cachedContent").unwrap(), "cachedContents/abc123");

            // Should NOT have system_instruction (it's in the cache)
            assert!(body.get("system_instruction").is_none());
        }

        #[test]
        fn cache_expiry_check() {
            use chrono::TimeZone;

            // Expired cache
            let expired = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                system_prompt_hash: 0,
            };
            assert!(expired.is_expired());

            // Future cache
            let future = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
                system_prompt_hash: 0,
            };
            assert!(!future.is_expired());
        }

        #[test]
        fn cache_prompt_matching() {
            let prompt = "You are a helpful assistant.";
            let hash = hash_prompt(prompt);

            let cache = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc::now(),
                system_prompt_hash: hash,
            };

            assert!(cache.matches_prompt(prompt));
            assert!(!cache.matches_prompt("Different prompt"));
        }

        #[test]
        fn should_cache_large_prompt() {
            // 4096 tokens * 4 chars/token = 16384 chars minimum
            let small_prompt = "A".repeat(1000);
            let large_prompt = "A".repeat(20000);

            assert!(!should_cache_prompt(&small_prompt, "gemini-3-pro"));
            assert!(should_cache_prompt(&large_prompt, "gemini-3-pro"));

            // Flash models have lower threshold
            let medium_prompt = "A".repeat(5000);
            assert!(should_cache_prompt(&medium_prompt, "gemini-3-flash"));
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
            assert_eq!(extract_sse_data(event), Some(String::new()));
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

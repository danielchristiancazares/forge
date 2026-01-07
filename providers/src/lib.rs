//! LLM provider clients with streaming support.
//!
//! This crate handles HTTP communication with Claude and OpenAI APIs,
//! including SSE streaming and error handling.

use anyhow::Result;
use forge_types::{
    ApiKey, CacheHint, CacheableMessage, Message, ModelName, OpenAIRequestOptions, OutputLimits,
    Provider, StreamEvent,
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
pub fn http_client_with_timeout(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(timeout_secs))
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
            return format!("{}...(truncated)", text);
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

    pub fn with_openai_options(mut self, options: OpenAIRequestOptions) -> Self {
        self.openai_options = options;
        self
    }

    pub fn provider(&self) -> Provider {
        self.api_key.provider()
    }

    pub fn api_key(&self) -> &str {
        self.api_key.as_str()
    }

    pub fn api_key_owned(&self) -> ApiKey {
        self.api_key.clone()
    }

    pub fn model(&self) -> &ModelName {
        &self.model
    }

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
/// * `on_event` - Callback for streaming events
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()> {
    match config.provider() {
        Provider::Claude => {
            claude::send_message(config, messages, limits, system_prompt, on_event).await
        }
        Provider::OpenAI => {
            openai::send_message(config, messages, limits, system_prompt, on_event).await
        }
    }
}

/// Claude/Anthropic API implementation.
pub mod claude {
    use super::*;
    use serde_json::json;

    const API_URL: &str = "https://api.anthropic.com/v1/messages";

    /// Build a content block with optional cache_control.
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
                    // This means cache_control can't be applied to them - Anthropic's
                    // API only supports cache hints on content blocks. This is acceptable
                    // because caching is most valuable for system prompts and early user
                    // messages that remain stable across turns.
                    api_messages.push(json!({
                        "role": "assistant",
                        "content": msg.content()
                    }));
                }
            }
        }

        let mut body = serde_json::Map::new();
        body.insert("model".into(), json!(model));
        body.insert("max_tokens".into(), json!(limits.max_output_tokens()));
        body.insert("stream".into(), json!(true));
        body.insert("messages".into(), json!(api_messages));

        if !system_blocks.is_empty() {
            body.insert("system".into(), json!(system_blocks));
        }

        if let Some(budget) = limits.thinking_budget() {
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
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        let client = http_client();

        let body = build_request_body(config.model().as_str(), messages, limits, system_prompt);

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
                "API error {}: {}",
                status, error_text
            )));
            return Ok(());
        }

        // Process SSE stream
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut saw_done = false;

        while let Some(chunk) = stream.next().await {
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

                let event = match std::str::from_utf8(&event) {
                    Ok(event) => event,
                    Err(_) => {
                        on_event(StreamEvent::Error(
                            "Received invalid UTF-8 from SSE stream".to_string(),
                        ));
                        return Ok(());
                    }
                };

                if let Some(data) = extract_sse_data(event) {
                    if data == "[DONE]" {
                        saw_done = true;
                        on_event(StreamEvent::Done);
                        return Ok(());
                    }

                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
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
                                    _ => {}
                                }
                            } else if let Some(text) = json["delta"]["text"].as_str() {
                                on_event(StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                        if json["type"] == "message_stop" {
                            saw_done = true;
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

        #[test]
        fn hoists_system_messages_into_system_blocks() {
            let model = Provider::Claude.default_model();
            let limits = OutputLimits::new(1024);

            let messages = vec![
                CacheableMessage::plain(Message::system(NonEmptyString::new("summary").unwrap())),
                CacheableMessage::plain(Message::try_user("hi").unwrap()),
            ];

            let body = build_request_body(model.as_str(), &messages, limits, None);

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

            let body = build_request_body(model.as_str(), &messages, limits, Some("prompt"));

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

/// OpenAI API implementation.
pub mod openai {
    use super::*;
    use serde_json::{Value, json};

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
            .map(|s| s.to_string())
    }

    fn extract_incomplete_reason(payload: &Value) -> Option<String> {
        payload
            .get("response")
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| details.get("reason"))
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
    }

    /// Map message role to OpenAI Responses API role.
    ///
    /// Per the OpenAI Model Spec, the authority hierarchy is:
    ///   Root > System > Developer > User > Guideline
    ///
    /// "System" level is reserved for OpenAI's own runtime injections.
    /// API developers operate at "Developer" level, so Message::System
    /// maps to "developer" role, not "system".
    fn openai_role(msg: &Message) -> &'static str {
        match msg {
            Message::System(_) => "developer",
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
        }
    }

    fn build_request_body(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
    ) -> Value {
        let mut input_items: Vec<Value> = Vec::new();
        for cacheable in messages {
            let msg = &cacheable.message;
            input_items.push(json!({
                "role": openai_role(msg),
                "content": msg.content(),
            }));
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
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        let client = http_client();

        let body = build_request_body(config, messages, limits, system_prompt);

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
                "API error {}: {}",
                status, error_text
            )));
            return Ok(());
        }

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut saw_delta = false;
        let mut saw_done = false;

        while let Some(chunk) = stream.next().await {
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

                let event = match std::str::from_utf8(&event) {
                    Ok(event) => event,
                    Err(_) => {
                        on_event(StreamEvent::Error(
                            "Received invalid UTF-8 from SSE stream".to_string(),
                        ));
                        return Ok(());
                    }
                };

                if let Some(data) = extract_sse_data(event) {
                    if data == "[DONE]" {
                        saw_done = true;
                        on_event(StreamEvent::Done);
                        return Ok(());
                    }

                    if let Ok(json) = serde_json::from_str::<Value>(&data) {
                        match json["type"].as_str().unwrap_or("") {
                            "response.output_text.delta" => {
                                if let Some(delta) = json["delta"].as_str() {
                                    saw_delta = true;
                                    on_event(StreamEvent::TextDelta(delta.to_string()));
                                }
                            }
                            "response.refusal.delta" => {
                                if let Some(delta) = json["delta"].as_str() {
                                    saw_delta = true;
                                    on_event(StreamEvent::TextDelta(delta.to_string()));
                                }
                            }
                            "response.output_text.done" => {
                                if !saw_delta && let Some(text) = json["text"].as_str() {
                                    on_event(StreamEvent::TextDelta(text.to_string()));
                                }
                            }
                            "response.completed" => {
                                saw_done = true;
                                on_event(StreamEvent::Done);
                                return Ok(());
                            }
                            "response.incomplete" => {
                                let reason = extract_incomplete_reason(&json)
                                    .unwrap_or_else(|| "Response incomplete".to_string());
                                on_event(StreamEvent::Error(reason));
                                return Ok(());
                            }
                            "response.failed" | "error" => {
                                let message = extract_error_message(&json)
                                    .unwrap_or_else(|| "Response failed".to_string());
                                on_event(StreamEvent::Error(message));
                                return Ok(());
                            }
                            _ => {}
                        }
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

        #[test]
        fn maps_system_message_to_developer_role() {
            let key = ApiKey::OpenAI("test".to_string());
            let model = Provider::OpenAI.default_model();
            let config = ApiConfig::new(key, model).unwrap();

            let messages = vec![
                CacheableMessage::plain(Message::system(NonEmptyString::new("summary").unwrap())),
                CacheableMessage::plain(Message::try_user("hi").unwrap()),
            ];

            let body = build_request_body(&config, &messages, OutputLimits::new(1024), None);

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

            let body =
                build_request_body(&config, &messages, OutputLimits::new(1024), Some("prompt"));

            assert_eq!(body.get("instructions").unwrap().as_str(), Some("prompt"));
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
}

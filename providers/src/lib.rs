//! LLM provider clients with unified streaming support.
//!
//! This crate handles HTTP communication with Claude, OpenAI, and Gemini APIs,
//! providing a unified streaming interface that abstracts provider differences
//! while preserving provider-specific features.
//!
//! # Architecture
//!
//! The crate is organized around a provider dispatch pattern:
//!
//! - [`send_message`] - Unified entry point that dispatches to provider-specific implementations
//! - [`claude`] - Anthropic Claude API client (Messages API)
//! - [`openai`] - OpenAI API client (Responses API for GPT-5.x)
//! - [`gemini`] - Google Gemini API client (GenerateContent API)
//!
//! All providers emit events through a [`tokio::sync::mpsc::Sender<StreamEvent>`]
//! channel, allowing the caller to process streaming content as it arrives.
//!
//! # Configuration
//!
//! Use [`ApiConfig`] to bundle API credentials and model selection. The constructor
//! validates that the API key and model belong to the same provider, making
//! provider mismatch errors impossible at runtime.
//!
//! # Streaming Events
//!
//! All providers normalize their responses to [`StreamEvent`][forge_types::StreamEvent]:
//!
//! | Event | Description |
//! |-------|-------------|
//! | `TextDelta` | Incremental text content from the model |
//! | `ThinkingDelta` | Extended thinking/reasoning content |
//! | `ToolCallStart` | Beginning of a tool/function call |
//! | `ToolCallDelta` | Incremental tool call arguments (JSON) |
//! | `Usage` | Token consumption metrics |
//! | `Done` | Stream completed successfully |
//! | `Error` | Stream terminated with an error |
//!
//! # Error Handling
//!
//! Errors during streaming are delivered as `StreamEvent::Error` events rather than
//! `Result::Err` returns. This allows partial responses to be captured before an error
//! occurs. Only unrecoverable failures like network errors return `Err`.

// Pedantic lint configuration - these are intentional design choices
#![allow(clippy::missing_errors_doc)] // Result-returning functions are self-explanatory
#![allow(clippy::missing_panics_doc)] // Panics are documented in assertions

pub mod retry;
pub mod sse_types;

use anyhow::Result;
use forge_types::{
    ApiKey, ApiUsage, CacheHint, CacheableMessage, Message, ModelName, OpenAIRequestOptions,
    OutputLimits, Provider, StreamEvent, ToolDefinition,
};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

pub use forge_types;

const CONNECT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 60;

// REQ-1: TCP Keepalive (from Anthropic Python SDK)
// Note: reqwest only exposes tcp_keepalive (idle time); interval/retries use platform defaults.
const TCP_KEEPALIVE_SECS: u64 = 60;

// REQ-2: Connection pool settings (from httpx defaults)
const POOL_MAX_IDLE_PER_HOST: usize = 100;
const POOL_IDLE_TIMEOUT_SECS: u64 = 90;

const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

const MAX_SSE_PARSE_ERRORS: usize = 3;

const MAX_SSE_PARSE_ERROR_PREVIEW: usize = 160;

const MAX_ERROR_BODY_BYTES: usize = 32 * 1024;

/// Shared HTTP client for all provider requests.
///
/// This client is configured with:
/// - Connection timeout: 30 seconds
/// - No read/total timeout (SSE streams can run for extended periods)
/// - Redirects disabled (API endpoints should never redirect)
/// - HTTPS only
/// - TCP keepalive (REQ-1): idle 60s, interval 60s, count 5
/// - Connection pool (REQ-2): 100 per-host, 90s idle timeout
/// - Platform headers (REQ-6): X-Stainless-Lang, OS, Arch
///
/// For synchronous requests needing a timeout (like distillation),
/// use [`http_client_with_timeout`] instead.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        base_client_builder().build().unwrap_or_else(|e| {
            // This should never fail in practice (only fails if TLS backend unavailable),
            // but if it does, log and fall back to a default client rather than panicking.
            tracing::error!("Failed to build custom HTTP client: {e}. Using default.");
            reqwest::Client::new()
        })
    })
}

/// Base client builder with shared configuration.
///
/// Applies:
/// - REQ-1: TCP keepalive settings
/// - REQ-2: Connection pool limits
/// - REQ-6: Platform headers (X-Stainless-*)
fn base_client_builder() -> reqwest::ClientBuilder {
    use reqwest::header::{HeaderMap, HeaderValue};

    let mut default_headers = HeaderMap::new();
    // REQ-6: Platform headers
    default_headers.insert("X-Stainless-Lang", HeaderValue::from_static("rust"));
    default_headers.insert(
        "X-Stainless-OS",
        HeaderValue::from_static(std::env::consts::OS),
    );
    default_headers.insert(
        "X-Stainless-Arch",
        HeaderValue::from_static(std::env::consts::ARCH),
    );

    reqwest::Client::builder()
        // Basic settings
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        // REQ-1: TCP keepalive
        .tcp_keepalive(Some(Duration::from_secs(TCP_KEEPALIVE_SECS)))
        // REQ-2: Connection pool
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(Some(Duration::from_secs(POOL_IDLE_TIMEOUT_SECS)))
        // REQ-6: Default headers
        .default_headers(default_headers)
}

/// HTTP client with a total request timeout for synchronous operations.
///
/// Use this for non-streaming requests like distillation where you want
/// to bound the total request time.
///
/// Inherits all base client settings (TCP keepalive, pool limits, platform headers).
///
/// Returns `Err` if the client cannot be built (e.g., TLS backend unavailable).
pub fn http_client_with_timeout(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error> {
    base_client_builder()
        .timeout(Duration::from_secs(timeout_secs))
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

/// Provider-specific SSE event parser.
///
/// Each provider implements this trait to parse their JSON event payloads
/// into unified [`StreamEvent`]s. The shared [`process_sse_stream`] function
/// handles common SSE logic (buffering, timeouts, error tracking) while
/// delegating JSON interpretation to the provider-specific parser.
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
                                let is_terminal =
                                    matches!(&event, StreamEvent::Done | StreamEvent::Error(_));
                                if !send_event(tx, event).await {
                                    return Ok(());
                                }
                                if is_terminal {
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

/// Configuration for API requests, bundling credentials and model selection.
///
/// This type enforces provider consistency at construction time: you cannot
/// create an `ApiConfig` with a Claude API key and an OpenAI model. This makes
/// provider mismatch errors impossible at runtime.
///
/// # Builder Pattern
///
/// Use the `with_*` methods to configure provider-specific options:
///
/// ```ignore
/// let config = ApiConfig::new(api_key, model)?
///     .with_openai_options(OpenAIRequestOptions::default())
///     .with_gemini_thinking_enabled(true);
/// ```
#[derive(Debug, Clone)]
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
    gemini_thinking_enabled: bool,
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
            gemini_thinking_enabled: false,
        })
    }

    #[must_use]
    pub fn with_openai_options(mut self, options: OpenAIRequestOptions) -> Self {
        self.openai_options = options;
        self
    }

    #[must_use]
    pub fn with_gemini_thinking_enabled(mut self, enabled: bool) -> Self {
        self.gemini_thinking_enabled = enabled;
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

    #[must_use]
    pub const fn gemini_thinking_enabled(&self) -> bool {
        self.gemini_thinking_enabled
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

/// Anthropic Claude API implementation.
///
/// Communicates with `https://api.anthropic.com/v1/messages` using SSE streaming.
///
/// # Features
///
/// - Extended thinking mode via `OutputLimits::with_thinking()`
/// - Ephemeral caching via `CacheHint::Ephemeral` on message content
/// - Tool calling with `tool_use` content blocks
///
/// # Thinking Mode Constraints
///
/// Thinking is automatically disabled when the conversation history contains
/// `Message::Assistant` or `Message::ToolUse` messages. This is because Claude's
/// API requires assistant messages to start with thinking/redacted_thinking blocks
/// when thinking is enabled, but Forge doesn't store thinking content in history.
pub mod claude {
    use super::{
        ApiConfig, ApiUsage, CacheHint, CacheableMessage, Message, OutputLimits, Result,
        SseParseAction, SseParser, StreamEvent, ToolDefinition, http_client, mpsc,
        process_sse_stream, read_capped_error_body,
        retry::{RetryConfig, RetryOutcome, send_with_retry},
        send_event,
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

        // Track pending assistant content blocks for grouping
        let mut pending_assistant_content: Vec<serde_json::Value> = Vec::new();

        // Helper to flush pending assistant content into a message
        let flush_assistant =
            |content: &mut Vec<serde_json::Value>, messages: &mut Vec<serde_json::Value>| {
                if !content.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": std::mem::take(content)
                    }));
                }
            };

        for cacheable in messages {
            let msg = &cacheable.message;
            let hint = cacheable.cache_hint;

            match msg {
                Message::System(_) => {
                    system_blocks.push(content_block(msg.content(), hint));
                }
                Message::User(_) => {
                    // Flush any pending assistant content before user message
                    flush_assistant(&mut pending_assistant_content, &mut api_messages);
                    api_messages.push(json!({
                        "role": "user",
                        "content": [content_block(msg.content(), hint)]
                    }));
                }
                Message::Assistant(_) => {
                    // Add text content block to pending assistant content
                    pending_assistant_content.push(json!({
                        "type": "text",
                        "text": msg.content()
                    }));
                }
                Message::ToolUse(call) => {
                    // Add tool_use content block to pending assistant content
                    pending_assistant_content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments
                    }));
                }
                Message::ToolResult(result) => {
                    // Flush any pending assistant content before tool result (user role)
                    flush_assistant(&mut pending_assistant_content, &mut api_messages);
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
                Message::Thinking(thinking) => {
                    // Send thinking as redacted_thinking if we have the signature
                    if let Some(signature) = thinking.signature() {
                        pending_assistant_content.push(json!({
                            "type": "redacted_thinking",
                            "data": signature
                        }));
                    }
                    // If no signature, skip (legacy thinking without signature)
                }
            }
        }

        // Flush any remaining assistant content
        flush_assistant(&mut pending_assistant_content, &mut api_messages);

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

        // Enable thinking if configured. We now properly replay thinking with signatures
        // via redacted_thinking blocks, so this works across multi-turn conversations.
        if let Some(budget) = limits.thinking_budget() {
            body.insert(
                "thinking".into(),
                json!({
                    "type": "enabled",
                    "budget_tokens": budget
                }),
            );

            // Add context_management to preserve all thinking blocks for cache efficiency.
            // Opus 4.5 preserves by default, but this is harmless there and essential for
            // Sonnet 4.5 / Haiku 4.5 where thinking blocks are stripped by default.
            body.insert(
                "context_management".into(),
                json!({
                    "edits": [{
                        "type": "clear_thinking_20251015",
                        "keep": "all"
                    }]
                }),
            );
        }

        serde_json::Value::Object(body)
    }

    // ========================================================================
    // Claude SSE Parser
    // ========================================================================

    use crate::sse_types::claude as typed;

    #[derive(Default)]
    struct ClaudeParser {
        /// Current tool call ID for streaming tool arguments
        current_tool_id: Option<String>,
    }

    impl SseParser for ClaudeParser {
        fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
            // Deserialize into typed event - forward compatible via Unknown variant
            let event: typed::Event = match serde_json::from_value(json.clone()) {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("Failed to parse Claude SSE event: {e}");
                    return SseParseAction::Continue;
                }
            };

            let mut events = Vec::new();

            match event {
                typed::Event::MessageStart { message } => {
                    if let Some(usage) = message.usage {
                        events.push(StreamEvent::Usage(ApiUsage {
                            input_tokens: usage.total_input_tokens(),
                            cache_read_tokens: usage.cache_read_input_tokens,
                            cache_creation_tokens: usage.cache_creation_input_tokens,
                            output_tokens: 0,
                        }));
                    }
                }

                typed::Event::MessageDelta { usage } => {
                    if let Some(usage) = usage
                        && usage.output_tokens > 0
                    {
                        events.push(StreamEvent::Usage(ApiUsage {
                            input_tokens: 0,
                            cache_read_tokens: 0,
                            cache_creation_tokens: 0,
                            output_tokens: usage.output_tokens,
                        }));
                    }
                }

                typed::Event::ContentBlockStart { content_block, .. } => {
                    if let typed::ContentBlock::ToolUse { id, name } = content_block {
                        if id.is_empty() {
                            return SseParseAction::Error(
                                "Claude tool call missing id".to_string(),
                            );
                        }
                        if name.is_empty() {
                            return SseParseAction::Error(
                                "Claude tool call missing name".to_string(),
                            );
                        }
                        self.current_tool_id = Some(id.clone());
                        events.push(StreamEvent::ToolCallStart {
                            id,
                            name,
                            thought_signature: None,
                        });
                    }
                }

                typed::Event::ContentBlockDelta { delta, .. } => match delta {
                    typed::Delta::TextDelta { text } => {
                        events.push(StreamEvent::TextDelta(text));
                    }
                    typed::Delta::ThinkingDelta { thinking } => {
                        events.push(StreamEvent::ThinkingDelta(thinking));
                    }
                    typed::Delta::SignatureDelta { signature } => {
                        events.push(StreamEvent::ThinkingSignature(signature));
                    }
                    typed::Delta::InputJsonDelta { partial_json } => {
                        if let Some(ref id) = self.current_tool_id {
                            events.push(StreamEvent::ToolCallDelta {
                                id: id.clone(),
                                arguments: partial_json,
                            });
                        }
                    }
                    typed::Delta::Unknown => {}
                },

                typed::Event::ContentBlockStop { .. } => {
                    self.current_tool_id = None;
                }

                typed::Event::MessageStop => {
                    return SseParseAction::Done;
                }

                typed::Event::Ping | typed::Event::Unknown => {}
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
        let retry_config = RetryConfig::default();

        let body = build_request_body(
            config.model().as_str(),
            messages,
            limits,
            system_prompt,
            tools,
        );

        let api_key = config.api_key().to_string();
        let body_json = body.clone();

        // Wrap request with retry logic (REQ-4)
        // Streaming requests omit X-Stainless-Timeout (no total timeout)
        let outcome = send_with_retry(
            || {
                client
                    .post(API_URL)
                    .header("x-api-key", &api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header(
                        "anthropic-beta",
                        "interleaved-thinking-2025-05-14,context-management-2025-06-27",
                    )
                    .header("content-type", "application/json")
                    .json(&body_json)
            },
            None, // No timeout header for streaming
            &retry_config,
        )
        .await;

        let response = match outcome {
            RetryOutcome::Success(resp) | RetryOutcome::HttpError(resp) => resp,
            RetryOutcome::ConnectionError { attempts, source } => {
                let _ = send_event(
                    &tx,
                    StreamEvent::Error(format!(
                        "Request failed after {attempts} attempts: {source}"
                    )),
                )
                .await;
                return Ok(());
            }
            RetryOutcome::NonRetryable(e) => {
                let _ = send_event(&tx, StreamEvent::Error(format!("Request failed: {e}"))).await;
                return Ok(());
            }
        };

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
                CacheableMessage::plain(Message::system(
                    NonEmptyString::new("Distillate").unwrap(),
                )),
                CacheableMessage::plain(Message::try_user("hi").unwrap()),
            ];

            let body = build_request_body(model.as_str(), &messages, limits, None, None);

            let system = body.get("system").unwrap().as_array().unwrap();
            assert_eq!(system.len(), 1);
            assert_eq!(system[0]["text"].as_str(), Some("Distillate"));

            let msgs = body.get("messages").unwrap().as_array().unwrap();
            assert_eq!(msgs.len(), 1);
            assert_eq!(msgs[0]["role"].as_str(), Some("user"));
        }

        #[test]
        fn system_prompt_precedes_system_messages() {
            let model = Provider::Claude.default_model();
            let limits = OutputLimits::new(1024);

            let messages = vec![CacheableMessage::plain(Message::system(
                NonEmptyString::new("Distillate").unwrap(),
            ))];

            let body = build_request_body(model.as_str(), &messages, limits, Some("prompt"), None);

            let system = body.get("system").unwrap().as_array().unwrap();
            assert_eq!(system.len(), 2);
            assert_eq!(system[0]["text"].as_str(), Some("prompt"));
            assert_eq!(
                system[0]["cache_control"]["type"].as_str(),
                Some("ephemeral")
            );
            assert_eq!(system[1]["text"].as_str(), Some("Distillate"));
        }

        #[test]
        fn claude_parser_emits_usage_on_message_start() {
            let mut parser = ClaudeParser::default();
            // Anthropic reports: input_tokens (non-cached) + cache_read + cache_creation
            // Total input should be the sum: 1234 + 1000 + 50 = 2284
            let json: serde_json::Value = serde_json::json!({
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 1234,
                        "cache_read_input_tokens": 1000,
                        "cache_creation_input_tokens": 50
                    }
                }
            });

            let action = parser.parse(&json);
            match action {
                SseParseAction::Emit(events) => {
                    assert_eq!(events.len(), 1);
                    match &events[0] {
                        StreamEvent::Usage(usage) => {
                            // Total = non_cached + cache_read + cache_creation
                            assert_eq!(usage.input_tokens, 2284);
                            assert_eq!(usage.cache_read_tokens, 1000);
                            assert_eq!(usage.cache_creation_tokens, 50);
                            assert_eq!(usage.output_tokens, 0);
                        }
                        _ => panic!("Expected Usage event"),
                    }
                }
                _ => panic!("Expected Emit action"),
            }
        }

        #[test]
        fn claude_parser_emits_usage_on_message_delta() {
            let mut parser = ClaudeParser::default();
            let json: serde_json::Value = serde_json::json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": "end_turn"
                },
                "usage": {
                    "output_tokens": 567
                }
            });

            let action = parser.parse(&json);
            match action {
                SseParseAction::Emit(events) => {
                    assert_eq!(events.len(), 1);
                    match &events[0] {
                        StreamEvent::Usage(usage) => {
                            assert_eq!(usage.input_tokens, 0);
                            assert_eq!(usage.cache_read_tokens, 0);
                            assert_eq!(usage.cache_creation_tokens, 0);
                            assert_eq!(usage.output_tokens, 567);
                        }
                        _ => panic!("Expected Usage event"),
                    }
                }
                _ => panic!("Expected Emit action"),
            }
        }
    }
}

/// OpenAI API implementation using the Responses API.
///
/// Communicates with `https://api.openai.com/v1/responses` for GPT-5.x models.
/// This uses the newer Responses API (not Chat Completions) which supports
/// advanced reasoning features.
///
/// # Features
///
/// - Reasoning effort control (`none`, `low`, `medium`, `high`, `xhigh`)
/// - Reasoning summaries (emitted as `ThinkingDelta` events)
/// - Text verbosity control
/// - Automatic server-side prefix caching
///
/// # Role Mapping
///
/// Per the OpenAI Model Spec authority hierarchy, `Message::System` maps to
/// the `"developer"` role (not `"system"`, which is reserved for OpenAI runtime).
pub mod openai {
    use super::{
        ApiConfig, ApiUsage, CacheableMessage, Message, OutputLimits, Result, SseParseAction,
        SseParser, StreamEvent, ToolDefinition, http_client, mpsc, process_sse_stream,
        read_capped_error_body,
        retry::{RetryConfig, RetryOutcome, send_with_retry},
        send_event,
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
                        let Some(call_id) = resolved_call_id.filter(|s| !s.trim().is_empty())
                        else {
                            return SseParseAction::Error(
                                "OpenAI tool call missing id".to_string(),
                            );
                        };
                        let Some(name) = name.filter(|s| !s.trim().is_empty()) else {
                            return SseParseAction::Error(
                                "OpenAI tool call missing name".to_string(),
                            );
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
                            thought_signature: None,
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
                            let ends_with_newline =
                                summary.ends_with('\n') || summary.ends_with('\r');
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
                        let cached_tokens =
                            usage.input_tokens_details.map_or(0, |d| d.cached_tokens);
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
            if options.reasoning_summary() != OpenAIReasoningSummary::None {
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

        let response = match outcome {
            RetryOutcome::Success(resp) | RetryOutcome::HttpError(resp) => resp,
            RetryOutcome::ConnectionError { attempts, source } => {
                let _ = send_event(
                    &tx,
                    StreamEvent::Error(format!(
                        "Request failed after {attempts} attempts: {source}"
                    )),
                )
                .await;
                return Ok(());
            }
            RetryOutcome::NonRetryable(e) => {
                let _ = send_event(&tx, StreamEvent::Error(format!("Request failed: {e}"))).await;
                return Ok(());
            }
        };

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
            let key = ApiKey::OpenAI("test".to_string());
            let model = Provider::OpenAI.default_model();
            let config = ApiConfig::new(key, model).unwrap();

            let messages = vec![
                CacheableMessage::plain(Message::system(
                    NonEmptyString::new("Distillate").unwrap(),
                )),
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
            let key = ApiKey::OpenAI("test".to_string());
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
            let key = ApiKey::OpenAI("test".to_string());
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
            let key = ApiKey::OpenAI("test".to_string());
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
}

/// Google Gemini API implementation.
///
/// Communicates with `https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent`.
///
/// # Features
///
/// - Thinking mode via `thinkingConfig` with `thinkingLevel: "high"`
/// - Explicit context caching via the `cachedContents` API
/// - Thought signatures for tool calls when thinking mode is enabled
///
/// # Message Grouping
///
/// Gemini requires consecutive tool calls and tool results to be grouped:
/// - Multiple consecutive `Message::ToolUse` become a single `model` content entry
/// - Multiple consecutive `Message::ToolResult` become a single `user` content entry
///
/// # Schema Sanitization
///
/// The `additionalProperties` field is recursively removed from tool parameter
/// schemas, as Gemini doesn't support it.
pub mod gemini {
    use super::{
        ApiConfig, CacheableMessage, Message, OutputLimits, Result, SseParseAction, SseParser,
        StreamEvent, ToolDefinition, http_client, mpsc, process_sse_stream, read_capped_error_body,
        retry::{RetryConfig, RetryOutcome, send_with_retry},
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
        /// Hash of cached tool definitions (for detecting changes)
        pub tools_hash: u64,
    }

    impl GeminiCache {
        /// Check if this cache has expired.
        #[must_use]
        pub fn is_expired(&self) -> bool {
            Utc::now() >= self.expire_time
        }

        /// Check if this cache matches the given system prompt and tools.
        #[must_use]
        pub fn matches_config(&self, prompt: &str, tools: Option<&[ToolDefinition]>) -> bool {
            hash_prompt(prompt) == self.system_prompt_hash && hash_tools(tools) == self.tools_hash
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

    /// Hash tool definitions for cache comparison.
    fn hash_tools(tools: Option<&[ToolDefinition]>) -> u64 {
        let mut hasher = DefaultHasher::new();
        if let Some(tools) = tools {
            for tool in tools {
                tool.name.hash(&mut hasher);
                tool.description.hash(&mut hasher);
                // Hash the JSON representation of parameters for stability
                tool.parameters.to_string().hash(&mut hasher);
            }
        }
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

    /// Create a cached content object with the system prompt and tools.
    ///
    /// This calls the Gemini cachedContents API to create a persistent cache
    /// that can be referenced in subsequent requests.
    ///
    /// # Note
    /// The cachedContents endpoint uses camelCase (unlike generateContent
    /// which mixes snake_case and camelCase).
    ///
    /// When using cached content, `systemInstruction`, `tools`, and `toolConfig`
    /// must be part of the cache - they cannot be specified in GenerateContent.
    pub async fn create_cache(
        api_key: &str,
        model: &str,
        system_prompt: &str,
        tools: Option<&[ToolDefinition]>,
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
        let mut body = json!({
            "model": format!("models/{}", model),
            "systemInstruction": {
                "parts": [{ "text": system_prompt }]
            },
            "ttl": format!("{}s", ttl_seconds)
        });

        // Include tools in cache if provided (required by Gemini API when using cached content)
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
            body["tools"] = json!([{
                "functionDeclarations": function_declarations
            }]);
        }

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
            tools_hash: hash_tools(tools),
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
                Message::Thinking(_) => {
                    // Thinking is not sent back to the API - stored for UI only.
                    index += 1;
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

        // Add tool definitions if provided, but NOT when using cached content
        // (tools must be part of the cache per Gemini API requirements)
        if cache.is_none()
            && let Some(tools) = tools
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

    use crate::sse_types::gemini as typed;

    /// Parser state for Gemini SSE streams.
    #[derive(Default)]
    struct GeminiParser;

    impl SseParser for GeminiParser {
        fn parse(&mut self, json: &Value) -> SseParseAction {
            // Deserialize into typed response
            let response: typed::Response = match serde_json::from_value(json.clone()) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("Failed to parse Gemini SSE response: {e}");
                    return SseParseAction::Continue;
                }
            };

            // Check for error response
            if let Some(error) = response.error {
                return SseParseAction::Error(error.message_or_default().to_string());
            }

            let mut events = Vec::new();
            let mut finish_action: Option<SseParseAction> = None;

            // Process candidates
            if let Some(candidates) = response.candidates {
                for candidate in candidates {
                    // Process content parts FIRST (before checking finish reason)
                    // This ensures we don't drop final content when finishReason is present
                    if let Some(content) = candidate.content
                        && let Some(parts) = content.parts
                    {
                        for part in parts {
                            // Text content
                            if let Some(text) = part.text {
                                if part.thought {
                                    events.push(StreamEvent::ThinkingDelta(text));
                                } else {
                                    events.push(StreamEvent::TextDelta(text));
                                }
                            }

                            // Function call
                            if let Some(func_call) = part.function_call {
                                let name = func_call.name.unwrap_or_default();

                                // Generate UUID for tool call ID (Gemini doesn't provide one)
                                let id = format!("call_{}", Uuid::new_v4());

                                events.push(StreamEvent::ToolCallStart {
                                    id: id.clone(),
                                    name,
                                    thought_signature: part.thought_signature,
                                });

                                // Send arguments as a single delta
                                let args = func_call.args.unwrap_or(json!({}));
                                if let Ok(args_str) = serde_json::to_string(&args) {
                                    events.push(StreamEvent::ToolCallDelta {
                                        id,
                                        arguments: args_str,
                                    });
                                }
                            }
                        }
                    }

                    // Check finish reason AFTER processing content
                    if let Some(reason_str) = candidate.finish_reason {
                        let reason = typed::FinishReason::parse(&reason_str);
                        if reason.is_success() {
                            finish_action = Some(SseParseAction::Done);
                        } else if let Some(msg) = reason.error_message() {
                            finish_action = Some(SseParseAction::Error(msg.to_string()));
                        }
                    }
                }
            }

            // If we have a finish action, emit any accumulated events first, then signal completion
            if let Some(action) = finish_action {
                if events.is_empty() {
                    return action;
                }
                // Emit events and signal done/error based on finish reason
                match action {
                    SseParseAction::Done => events.push(StreamEvent::Done),
                    SseParseAction::Error(msg) => events.push(StreamEvent::Error(msg)),
                    _ => {}
                }
                return SseParseAction::Emit(events);
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
        let retry_config = RetryConfig::default();
        let model = config.model().as_str();
        let url = format!("{API_BASE}/models/{model}:streamGenerateContent?alt=sse");

        let thinking_enabled = config.gemini_thinking_enabled();

        let body = build_request_body(
            messages,
            limits,
            system_prompt,
            tools,
            thinking_enabled,
            cache,
        );

        let api_key = config.api_key().to_string();
        let body_json = body.clone();

        // Wrap request with retry logic (REQ-4)
        // Streaming requests omit X-Stainless-Timeout (no total timeout)
        let outcome = send_with_retry(
            || {
                client
                    .post(&url)
                    .header("x-goog-api-key", &api_key)
                    .header("content-type", "application/json")
                    .json(&body_json)
            },
            None, // No timeout header for streaming
            &retry_config,
        )
        .await;

        let response = match outcome {
            RetryOutcome::Success(resp) | RetryOutcome::HttpError(resp) => resp,
            RetryOutcome::ConnectionError { attempts, source } => {
                let _ = send_event(
                    &tx,
                    StreamEvent::Error(format!(
                        "Request failed after {attempts} attempts: {source}"
                    )),
                )
                .await;
                return Ok(());
            }
            RetryOutcome::NonRetryable(e) => {
                let _ = send_event(&tx, StreamEvent::Error(format!("Request failed: {e}"))).await;
                return Ok(());
            }
        };

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
        use forge_types::{ApiKey, Provider};

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
        fn gemini_thinking_flag_controls_request() {
            let messages = vec![CacheableMessage::plain(Message::try_user("hello").unwrap())];
            let limits = OutputLimits::with_thinking(8192, 2048).unwrap();

            let config = ApiConfig::new(
                ApiKey::Gemini("test".to_string()),
                Provider::Gemini.default_model(),
            )
            .unwrap();

            let body = build_request_body(
                &messages,
                limits,
                None,
                None,
                config.gemini_thinking_enabled(),
                None,
            );
            let gen_config = body.get("generationConfig").unwrap();
            assert!(gen_config.get("thinkingConfig").is_none());

            let config = config.with_gemini_thinking_enabled(true);
            let body = build_request_body(
                &messages,
                limits,
                None,
                None,
                config.gemini_thinking_enabled(),
                None,
            );
            let gen_config = body.get("generationConfig").unwrap();
            let thinking = gen_config.get("thinkingConfig").unwrap();
            assert_eq!(thinking["thinkingLevel"], "high");
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
            let call = forge_types::ToolCall::new("call_123", "Read", json!({"path": "foo"}));
            let messages = vec![CacheableMessage::plain(Message::tool_use(call))];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents[0]["role"], "model");
            let func_call = &contents[0]["parts"][0]["functionCall"];
            assert_eq!(func_call["name"], "Read");
        }

        #[test]
        fn groups_tool_calls_and_preserves_thought_signature() {
            let call = forge_types::ToolCall::new_with_thought_signature(
                "call_1",
                "Read",
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
            let result_a = forge_types::ToolResult::success("call_1", "Read", "file contents here");
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
            assert_eq!(parts[0]["functionResponse"]["name"], "Read");
            assert_eq!(parts[1]["functionResponse"]["name"], "list_dir");
        }

        #[test]
        fn maps_tool_result_to_function_response() {
            let result = forge_types::ToolResult::success("call_1", "Read", "file contents here");
            let messages = vec![CacheableMessage::plain(Message::tool_result(result))];
            let limits = OutputLimits::new(4096);

            let body = build_request_body(&messages, limits, None, None, false, None);

            let contents = body.get("contents").unwrap().as_array().unwrap();
            assert_eq!(contents[0]["role"], "user");
            let func_resp = &contents[0]["parts"][0]["functionResponse"];
            // Gemini uses tool_name for functionResponse.name
            assert_eq!(func_resp["name"], "Read");
        }

        #[test]
        fn finish_reason_stop_is_success() {
            let reason = typed::FinishReason::parse("STOP");
            assert!(reason.is_success());
            assert!(reason.error_message().is_none());
        }

        #[test]
        fn finish_reason_safety_is_error() {
            let reason = typed::FinishReason::parse("SAFETY");
            assert!(!reason.is_success());
            assert!(reason.error_message().is_some());
        }

        #[test]
        fn finish_reason_unknown_continues() {
            let reason = typed::FinishReason::parse("UNKNOWN_REASON");
            // Unknown reasons should continue processing (not error)
            assert!(reason.error_message().is_none());
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
                tools_hash: 0,
            };

            // Create some tools to verify they're NOT included when cache is present
            let tools = vec![forge_types::ToolDefinition::new(
                "test_tool".to_string(),
                "A test tool".to_string(),
                serde_json::json!({"type": "object"}),
            )];

            let body = build_request_body(
                &messages,
                limits,
                Some("You are helpful"), // Should be ignored when cache present
                Some(&tools),            // Should be ignored when cache present
                false,
                Some(&cache),
            );

            // Should have cachedContent reference
            assert_eq!(body.get("cachedContent").unwrap(), "cachedContents/abc123");

            // Should NOT have system_instruction (it's in the cache)
            assert!(body.get("system_instruction").is_none());

            // Should NOT have tools (they're in the cache)
            assert!(body.get("tools").is_none());
        }

        #[test]
        fn cache_expiry_check() {
            use chrono::TimeZone;

            // Expired cache
            let expired = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                system_prompt_hash: 0,
                tools_hash: 0,
            };
            assert!(expired.is_expired());

            // Future cache
            let future = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
                system_prompt_hash: 0,
                tools_hash: 0,
            };
            assert!(!future.is_expired());
        }

        #[test]
        fn cache_config_matching() {
            let prompt = "You are a helpful assistant.";
            let tools = vec![forge_types::ToolDefinition::new(
                "test_tool".to_string(),
                "A test tool".to_string(),
                serde_json::json!({"type": "object"}),
            )];

            let cache = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc::now(),
                system_prompt_hash: hash_prompt(prompt),
                tools_hash: hash_tools(Some(&tools)),
            };

            // Matches when both prompt and tools match
            assert!(cache.matches_config(prompt, Some(&tools)));

            // Doesn't match with different prompt
            assert!(!cache.matches_config("Different prompt", Some(&tools)));

            // Doesn't match with different tools
            let different_tools = vec![forge_types::ToolDefinition::new(
                "other_tool".to_string(),
                "Another tool".to_string(),
                serde_json::json!({"type": "object"}),
            )];
            assert!(!cache.matches_config(prompt, Some(&different_tools)));

            // Doesn't match with no tools when cache has tools
            assert!(!cache.matches_config(prompt, None));
        }

        #[test]
        fn cache_config_matching_no_tools() {
            let prompt = "You are a helpful assistant.";

            let cache = GeminiCache {
                name: "test".to_string(),
                expire_time: Utc::now(),
                system_prompt_hash: hash_prompt(prompt),
                tools_hash: hash_tools(None),
            };

            // Matches when both prompt matches and no tools
            assert!(cache.matches_config(prompt, None));

            // Doesn't match when tools are provided but cache has none
            let tools = vec![forge_types::ToolDefinition::new(
                "test_tool".to_string(),
                "A test tool".to_string(),
                serde_json::json!({"type": "object"}),
            )];
            assert!(!cache.matches_config(prompt, Some(&tools)));
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

        #[test]
        fn parser_extracts_thought_signature_from_function_call() {
            let mut parser = GeminiParser;

            // Simulate Gemini response with thinking mode enabled
            let response = json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "Read",
                                "args": {"path": "test.rs"}
                            },
                            "thoughtSignature": "abc123signature"
                        }]
                    }
                }]
            });

            let action = parser.parse(&response);

            match action {
                SseParseAction::Emit(events) => {
                    assert!(!events.is_empty());
                    match &events[0] {
                        StreamEvent::ToolCallStart {
                            id: _,
                            name,
                            thought_signature,
                        } => {
                            assert_eq!(name, "Read");
                            assert_eq!(thought_signature.as_deref(), Some("abc123signature"));
                        }
                        _ => panic!("Expected ToolCallStart event"),
                    }
                }
                _ => panic!("Expected Emit action"),
            }
        }

        #[test]
        fn parser_handles_function_call_without_thought_signature() {
            let mut parser = GeminiParser;

            // Simulate Gemini response without thinking mode
            let response = json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "list_dir",
                                "args": {}
                            }
                        }]
                    }
                }]
            });

            let action = parser.parse(&response);

            match action {
                SseParseAction::Emit(events) => {
                    assert!(!events.is_empty());
                    match &events[0] {
                        StreamEvent::ToolCallStart {
                            id: _,
                            name,
                            thought_signature,
                        } => {
                            assert_eq!(name, "list_dir");
                            assert!(thought_signature.is_none());
                        }
                        _ => panic!("Expected ToolCallStart event"),
                    }
                }
                _ => panic!("Expected Emit action"),
            }
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

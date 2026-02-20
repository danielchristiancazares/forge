//! LLM provider clients with unified streaming support.
//!
//! # Architecture
//!
//! The crate is organized around a provider dispatch pattern:
//!
//! - [`send_message`] - Unified entry point that dispatches to provider-specific implementations
//! - [`claude`] - Anthropic Claude API client (Messages API)
//! - [`openai`] - OpenAI API client (Responses API; GPT-5 options when applicable)
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
//! All providers normalize their responses to [`StreamEvent`]:
//!
//! | Event | Description |
//! |-------|-------------|
//! | `TextDelta` | Incremental text content from the model |
//! | `ThinkingDelta` | Extended thinking/reasoning content |
//! | `ThinkingSignature` | Provider thinking signature for replay/verification (provider-specific) |
//! | `ToolCallStart` | Beginning of a tool/function call |
//! | `ToolCallDelta` | Incremental tool call arguments (JSON) |
//! | `Usage` | Token consumption metrics |
//! | `Done` | Stream completed successfully |
//! | `Error` | Stream terminated with an error |
//!
//! # Error Handling
//!
//! Most provider/API errors during streaming are delivered as `StreamEvent::Error` events
//! rather than `Result::Err` returns, allowing partial output to be captured before the
//! error occurs. Low-level failures that prevent reading the HTTP response stream (e.g.
//! mid-stream I/O errors) may still return `Err`.

pub mod retry;
pub mod sse_types;

pub(crate) use anyhow::Result;
pub(crate) use forge_types::{
    ApiKey, ApiUsage, CacheHint, CacheableMessage, Message, ModelName, OpenAIRequestOptions,
    OutputLimits, Provider, StreamEvent, ThinkingReplayState, ThoughtSignature,
    ThoughtSignatureState, ToolDefinition,
};
use std::sync::OnceLock;
use std::time::Duration;
pub(crate) use tokio::sync::mpsc;

pub use forge_types;

/// Canonical Anthropic Messages API endpoint.
pub const CLAUDE_MESSAGES_API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Canonical OpenAI Responses API endpoint.
pub const OPENAI_RESPONSES_API_URL: &str = "https://api.openai.com/v1/responses";
/// Canonical Gemini API base URL.
pub const GEMINI_API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

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

const MAX_ERROR_BODY_BYTES: usize = 32 * 1024;

pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        base_client_builder().build().unwrap_or_else(|e| {
            tracing::error!(
                "Failed to build hardened HTTP client: {e}. Attempting minimal hardened fallback."
            );
            reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("Minimal hardened HTTP client must build; cannot proceed without TLS")
        })
    })
}

/// REQ-6: Platform headers (X-Stainless-*)
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

#[derive(Debug)]
pub(crate) enum SseParseAction {
    /// Continue processing, no event to emit
    Continue,
    /// Emit these events and continue
    Emit(Vec<StreamEvent>),
    /// Stream is done (message_stop, response.completed, finishReason=STOP)
    Done,
    Error(String),
}

pub(crate) trait SseParser {
    fn parse(&mut self, json: &serde_json::Value) -> SseParseAction;
    fn provider_name(&self) -> &'static str;
}

pub(crate) fn stream_idle_timeout() -> Duration {
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

pub(crate) async fn send_event(tx: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> bool {
    tx.send(event).await.is_ok()
}

pub(crate) fn parse_sse_payload<T>(
    json: &serde_json::Value,
    provider_name: &'static str,
) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    match serde_json::from_value(json.clone()) {
        Ok(event) => Some(event),
        Err(e) => {
            tracing::warn!(%e, provider = provider_name, "Failed to parse SSE event");
            None
        }
    }
}

pub(crate) fn emit_or_continue(events: Vec<StreamEvent>) -> SseParseAction {
    if events.is_empty() {
        SseParseAction::Continue
    } else {
        SseParseAction::Emit(events)
    }
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
pub(crate) async fn process_sse_stream<P: SseParser>(
    response: reqwest::Response,
    parser: &mut P,
    tx: &mpsc::Sender<StreamEvent>,
    idle_timeout: Duration,
) -> Result<()> {
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut parse_errors = 0usize;

    loop {
        let Ok(next) = tokio::time::timeout(idle_timeout, stream.next()).await else {
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
                    tracing::warn!(
                        %e,
                        payload_bytes = data.len(),
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

#[derive(Debug)]
pub(crate) enum ApiResponse {
    Success(reqwest::Response),
    StreamTerminated,
}

pub(crate) async fn handle_response(
    outcome: retry::RetryOutcome,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<ApiResponse> {
    let response = match outcome {
        retry::RetryOutcome::Success(resp) | retry::RetryOutcome::HttpError(resp) => resp,
        retry::RetryOutcome::ConnectionError { attempts, source } => {
            let _ = send_event(
                tx,
                StreamEvent::Error(format!(
                    "Request failed after {attempts} attempts: {source}"
                )),
            )
            .await;
            return Ok(ApiResponse::StreamTerminated);
        }
        retry::RetryOutcome::NonRetryable(e) => {
            let _ = send_event(tx, StreamEvent::Error(format!("Request failed: {e}"))).await;
            return Ok(ApiResponse::StreamTerminated);
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let error_text = read_capped_error_body(response).await;
        let _ = send_event(
            tx,
            StreamEvent::Error(format!("API error {status}: {error_text}")),
        )
        .await;
        return Ok(ApiResponse::StreamTerminated);
    }

    Ok(ApiResponse::Success(response))
}

pub(crate) async fn send_retried_sse_request<P, F>(
    build_request: F,
    retry_config: &retry::RetryConfig,
    tx: &mpsc::Sender<StreamEvent>,
    parser: &mut P,
    idle_timeout: Duration,
) -> Result<()>
where
    P: SseParser,
    F: Fn() -> reqwest::RequestBuilder,
{
    let response = match send_retried_request(build_request, retry_config, tx).await? {
        ApiResponse::Success(resp) => resp,
        ApiResponse::StreamTerminated => return Ok(()),
    };

    process_sse_stream(response, parser, tx, idle_timeout).await
}

pub(crate) async fn send_retried_request<F>(
    build_request: F,
    retry_config: &retry::RetryConfig,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<ApiResponse>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let outcome = retry::send_with_retry(build_request, None, retry_config).await;
    handle_response(outcome, tx).await
}

/// Provider + model configuration with provider-specific tuning knobs.
///
/// The constructor enforces that the API key and model belong to the same provider.
///
/// ```rust
/// use forge_providers::ApiConfig;
/// use forge_types::{ApiKey, OpenAIRequestOptions, Provider};
///
/// let config = ApiConfig::new(ApiKey::openai("test"), Provider::OpenAI.default_model())
///     .unwrap()
///     .with_openai_options(OpenAIRequestOptions::default())
///     .with_gemini_thinking_enabled(true);
/// # let _ = config;
/// ```
#[derive(Debug, Clone)]
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
    gemini_thinking_enabled: bool,
    anthropic_thinking_mode: &'static str,
    anthropic_thinking_effort: &'static str,
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
            anthropic_thinking_mode: "adaptive",
            anthropic_thinking_effort: "max",
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
    pub fn with_anthropic_thinking(mut self, mode: &'static str, effort: &'static str) -> Self {
        self.anthropic_thinking_mode = mode;
        self.anthropic_thinking_effort = effort;
        self
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        self.api_key.provider()
    }

    #[must_use]
    pub fn api_key(&self) -> &str {
        self.api_key.expose_secret()
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

    #[must_use]
    pub const fn anthropic_thinking_mode(&self) -> &str {
        self.anthropic_thinking_mode
    }

    #[must_use]
    pub const fn anthropic_thinking_effort(&self) -> &str {
        self.anthropic_thinking_effort
    }
}

pub struct SendMessageRequest<'a> {
    pub config: &'a ApiConfig,
    pub messages: &'a [CacheableMessage],
    pub limits: OutputLimits,
    pub system_prompt: Option<&'a str>,
    pub tools: Option<&'a [ToolDefinition]>,
    pub system_cache_hint: CacheHint,
    pub cache_last_tool: bool,
    pub gemini_cache: Option<&'a gemini::GeminiCache>,
    pub tx: mpsc::Sender<StreamEvent>,
    /// OpenAI `previous_response_id` for stateful Pro model chaining.
    pub previous_response_id: Option<&'a str>,
}

pub async fn send_message(request: SendMessageRequest<'_>) -> Result<()> {
    match request.config.provider() {
        Provider::Claude => claude::send_message(&request).await,
        Provider::OpenAI => openai::send_message(&request).await,
        Provider::Gemini => gemini::send_message(&request).await,
    }
}

/// tool calling with `tool_use` content blocks.
///
/// # Thinking Mode Constraints
///
/// Thinking is automatically disabled when the conversation history contains
/// `Message::Assistant` or `Message::ToolUse` messages. This is because Claude's
/// API requires assistant messages to start with thinking/redacted_thinking blocks
/// when thinking is enabled, but Forge doesn't store thinking content in history.
pub mod claude;

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
pub mod openai;

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
pub mod gemini;

#[cfg(test)]
mod tests {
    use super::{
        ApiConfig, ApiKey, drain_next_sse_event, extract_sse_data, find_sse_event_boundary,
    };
    use forge_types::Provider;

    #[test]
    fn api_config_rejects_mismatched_provider() {
        let key = ApiKey::claude("test");
        let model = Provider::OpenAI.default_model();
        let result = ApiConfig::new(key, model);
        assert!(result.is_err());
    }

    #[test]
    fn api_config_accepts_matching_provider() {
        let key = ApiKey::claude("test");
        let model = Provider::Claude.default_model();
        let result = ApiConfig::new(key, model);
        assert!(result.is_ok());
    }

    mod sse_boundary {
        use super::find_sse_event_boundary;

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
        use super::drain_next_sse_event;

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
        use super::extract_sse_data;

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

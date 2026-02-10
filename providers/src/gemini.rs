use crate::{
    ApiConfig, ApiResponse, CacheableMessage, Message, OutputLimits, Result, SseParseAction,
    SseParser, StreamEvent, ThoughtSignature, ThoughtSignatureState, ToolDefinition,
    handle_response, http_client, http_client_with_timeout, mpsc, process_sse_stream,
    read_capped_error_body,
    retry::{RetryConfig, send_with_retry},
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
        anyhow::bail!("System prompt too short for caching (minimum ~4096 tokens for Pro models)");
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

    let client = http_client_with_timeout(120).map_err(|e| anyhow::anyhow!("HTTP client: {e}"))?;
    let response = client
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
                            if let ThoughtSignatureState::Signed(signature) =
                                &call.thought_signature
                            {
                                part.insert("thoughtSignature".into(), json!(signature.as_str()));
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
                tracing::warn!(%e, "Failed to parse Gemini SSE event");
                return SseParseAction::Continue;
            }
        };

        // Check for error response
        if let Some(error) = response.error {
            return SseParseAction::Error(error.message_or_default().to_string());
        }

        let mut events = Vec::new();
        let mut finish_action: Option<SseParseAction> = None;

        if let Some(usage) = response.usage_metadata {
            events.push(StreamEvent::Usage(crate::ApiUsage {
                input_tokens: usage.prompt_token_count,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                output_tokens: usage.candidates_token_count,
            }));
        }

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
                            if name.is_empty() {
                                tracing::warn!("Gemini function call with empty name, skipping");
                                continue;
                            }

                            // Generate UUID for tool call ID (Gemini doesn't provide one)
                            let id = format!("call_{}", Uuid::new_v4());

                            let thought_signature = match part.thought_signature {
                                Some(signature) if !signature.is_empty() => {
                                    ThoughtSignatureState::Signed(ThoughtSignature::new(signature))
                                }
                                _ => ThoughtSignatureState::Unsigned,
                            };
                            events.push(StreamEvent::ToolCallStart {
                                id: id.clone(),
                                name,
                                thought_signature,
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

    let response = match handle_response(outcome, &tx).await? {
        ApiResponse::Success(resp) => resp,
        ApiResponse::StreamTerminated => return Ok(()),
    };

    let mut parser = GeminiParser;
    process_sse_stream(response, &mut parser, &tx, crate::stream_idle_timeout()).await
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

        let config =
            ApiConfig::new(ApiKey::gemini("test"), Provider::Gemini.default_model()).unwrap();

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
        let call = forge_types::ToolCall::new_signed(
            "call_1",
            "Read",
            json!({"path": "foo"}),
            forge_types::ThoughtSignature::new("sig_1"),
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
        let result_b = forge_types::ToolResult::success("call_2", "list_dir", "dir contents here");
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

        assert_eq!(body.get("cachedContent").unwrap(), "cachedContents/abc123");

        assert!(body.get("system_instruction").is_none());

        assert!(body.get("tools").is_none());
    }

    #[test]
    fn cache_expiry_check() {
        use chrono::TimeZone;

        let expired = GeminiCache {
            name: "test".to_string(),
            expire_time: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            system_prompt_hash: 0,
            tools_hash: 0,
        };
        assert!(expired.is_expired());

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

        assert!(cache.matches_config(prompt, Some(&tools)));

        assert!(!cache.matches_config("Different prompt", Some(&tools)));

        let different_tools = vec![forge_types::ToolDefinition::new(
            "other_tool".to_string(),
            "Another tool".to_string(),
            serde_json::json!({"type": "object"}),
        )];
        assert!(!cache.matches_config(prompt, Some(&different_tools)));

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
                        assert!(matches!(
                            thought_signature,
                            ThoughtSignatureState::Signed(signature)
                                if signature.as_str() == "abc123signature"
                        ));
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
                        assert!(matches!(thought_signature, ThoughtSignatureState::Unsigned));
                    }
                    _ => panic!("Expected ToolCallStart event"),
                }
            }
            _ => panic!("Expected Emit action"),
        }
    }
}

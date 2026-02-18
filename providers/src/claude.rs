use crate::{
    ApiUsage, CacheHint, CacheableMessage, Message, OutputLimits, Result, SendMessageRequest,
    SseParseAction, SseParser, StreamEvent, ThinkingReplayState, ThoughtSignatureState,
    ToolDefinition, emit_or_continue, http_client, parse_sse_payload, retry::RetryConfig,
    send_retried_sse_request,
};
use forge_types::ThinkingState;
use serde_json::json;

const API_URL: &str = crate::CLAUDE_MESSAGES_API_URL;

fn is_opus_4_6_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("claude-opus-4-6")
}

fn anthropic_beta_header(model: &str, limits: OutputLimits) -> Option<&'static str> {
    if is_opus_4_6_model(model) {
        // NOTE: Server-side compaction (compact-2026-01-12) is intentionally NOT
        // enabled here. Forge maintains its own client-side conversation history
        // and does not reconcile it after server compaction, causing an infinite
        // loop: full history → compaction → tool calls → full history → compaction.
        // Use Forge's client-side distillation instead.
        return Some("context-1m-2025-08-07");
    }

    if limits.has_thinking() {
        Some("interleaved-thinking-2025-05-14,context-management-2025-06-27")
    } else {
        None
    }
}

fn apply_ephemeral_cache_control(block: &mut serde_json::Value) {
    block["cache_control"] = json!({ "type": "ephemeral", "ttl": "1h" });
}

fn content_block(text: &str, cache_hint: CacheHint) -> serde_json::Value {
    let mut block = json!({
        "type": "text",
        "text": text
    });
    if matches!(cache_hint, CacheHint::Ephemeral) {
        apply_ephemeral_cache_control(&mut block);
    }
    block
}

struct ClaudeRequestBodyInput<'a> {
    model: &'a str,
    messages: &'a [CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&'a str>,
    system_cache_hint: CacheHint,
    tools: Option<&'a [ToolDefinition]>,
    cache_last_tool: bool,
    thinking_mode: &'a str,
    thinking_effort: &'a str,
}

fn build_request_body(input: ClaudeRequestBodyInput<'_>) -> serde_json::Value {
    let ClaudeRequestBodyInput {
        model,
        messages,
        limits,
        system_prompt,
        system_cache_hint,
        tools,
        cache_last_tool,
        thinking_mode,
        thinking_effort,
    } = input;

    let mut system_blocks: Vec<serde_json::Value> = Vec::new();
    let mut api_messages: Vec<serde_json::Value> = Vec::new();

    if let Some(prompt) = system_prompt
        && !prompt.trim().is_empty()
    {
        system_blocks.push(content_block(prompt, system_cache_hint));
    }

    // Track pending assistant content blocks for grouping
    let mut pending_assistant_content: Vec<serde_json::Value> = Vec::new();
    let mut pending_assistant_cached = false;

    // Helper to flush pending assistant content into a message.
    // If any message in the group had CacheHint::Ephemeral, cache_control
    // is placed on the last content block (covering the entire prefix through
    // this assistant turn).
    let flush_assistant = |content: &mut Vec<serde_json::Value>,
                           cached: &mut bool,
                           messages: &mut Vec<serde_json::Value>| {
        if !content.is_empty() {
            if *cached && let Some(last) = content.last_mut() {
                apply_ephemeral_cache_control(last);
            }
            messages.push(json!({
                "role": "assistant",
                "content": std::mem::take(content)
            }));
            *cached = false;
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
                flush_assistant(
                    &mut pending_assistant_content,
                    &mut pending_assistant_cached,
                    &mut api_messages,
                );
                api_messages.push(json!({
                    "role": "user",
                    "content": [content_block(msg.content(), hint)]
                }));
            }
            Message::Assistant(_) => {
                // Add text content block to pending assistant content
                if matches!(hint, CacheHint::Ephemeral) {
                    pending_assistant_cached = true;
                }
                pending_assistant_content.push(json!({
                    "type": "text",
                    "text": msg.content()
                }));
            }
            Message::ToolUse(call) => {
                // Add tool_use content block to pending assistant content
                if matches!(hint, CacheHint::Ephemeral) {
                    pending_assistant_cached = true;
                }
                pending_assistant_content.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments
                }));
            }
            Message::ToolResult(result) => {
                // Flush any pending assistant content before tool result (user role)
                flush_assistant(
                    &mut pending_assistant_content,
                    &mut pending_assistant_cached,
                    &mut api_messages,
                );
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": result.tool_call_id,
                    "content": result.content,
                    "is_error": result.is_error
                });
                if matches!(hint, CacheHint::Ephemeral) {
                    apply_ephemeral_cache_control(&mut block);
                }
                api_messages.push(json!({
                    "role": "user",
                    "content": [block]
                }));
            }
            Message::Thinking(thinking) => {
                if let ThinkingReplayState::ClaudeSigned { signature } = thinking.replay_state() {
                    pending_assistant_content.push(json!({
                        "type": "redacted_thinking",
                        "data": signature.as_str()
                    }));
                }
            }
        }
    }

    // Flush any remaining assistant content
    flush_assistant(
        &mut pending_assistant_content,
        &mut pending_assistant_cached,
        &mut api_messages,
    );

    if is_opus_4_6_model(model)
        && matches!(
            api_messages.last(),
            Some(last) if last
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("assistant")
        )
    {
        api_messages.pop();
        tracing::warn!(
            "Dropped trailing assistant prefill for Opus 4.6 compatibility (Anthropic no longer accepts assistant-prefilled final turns)"
        );
    }

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
        let mut tool_schemas: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                })
            })
            .collect();
        if cache_last_tool && let Some(last) = tool_schemas.last_mut() {
            apply_ephemeral_cache_control(last);
        }
        body.insert("tools".into(), json!(tool_schemas));
    }

    // Opus 4.6 thinking mode and effort are configurable via [anthropic] config.
    if is_opus_4_6_model(model) {
        let mut thinking_obj = json!({"type": thinking_mode});
        // When mode is "enabled", also emit budget_tokens from OutputLimits
        if thinking_mode == "enabled"
            && let ThinkingState::Enabled(budget) = limits.thinking()
        {
            thinking_obj["budget_tokens"] = json!(budget.as_u32());
        }
        body.insert("thinking".into(), thinking_obj);
        if thinking_mode != "disabled" {
            body.insert(
                "output_config".into(),
                json!({
                    "effort": thinking_effort
                }),
            );
        }
    // Legacy thinking mode for pre-4.6 models.
    } else if let ThinkingState::Enabled(budget) = limits.thinking() {
        body.insert(
            "thinking".into(),
            json!({
                "type": "enabled",
                "budget_tokens": budget.as_u32()
            }),
        );

        // Add context_management to preserve all thinking blocks for cache efficiency.
        // Essential for Haiku 4.5 where thinking blocks are stripped by default.
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

use crate::sse_types::claude as typed;

#[derive(Default)]
struct ClaudeParser {
    /// Current tool call ID for streaming tool arguments
    current_tool_id: Option<String>,
    /// Server-side compaction is in progress — the API will send a new
    /// `message_start` after the current `message_stop`, so we must NOT
    /// treat that `message_stop` as end-of-stream.
    compacting: bool,
}

impl SseParser for ClaudeParser {
    fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
        // Deserialize into typed event - forward compatible via Unknown variant
        let Some(event) = parse_sse_payload::<typed::Event>(json, "Claude") else {
            return SseParseAction::Continue;
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

            typed::Event::MessageDelta { delta, usage } => {
                if let Some(typed::MessageDeltaInfo {
                    stop_reason: Some(typed::StopReason::Compaction),
                }) = delta
                {
                    tracing::info!("Server-side compaction triggered by Anthropic API");
                    self.compacting = true;
                }

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
                        return SseParseAction::Error("Claude tool call missing id".to_string());
                    }
                    if name.is_empty() {
                        return SseParseAction::Error("Claude tool call missing name".to_string());
                    }
                    self.current_tool_id = Some(id.clone());
                    events.push(StreamEvent::ToolCallStart {
                        id,
                        name,
                        thought_signature: ThoughtSignatureState::Unsigned,
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
                if self.compacting {
                    // Compaction: the API will immediately start a new message
                    // stream with the compacted context. Keep reading.
                    self.compacting = false;
                    // Fall through to Continue — do NOT signal Done.
                } else {
                    return SseParseAction::Done;
                }
            }

            typed::Event::Error { error } => {
                let msg = if error.message.is_empty() {
                    format!("Claude stream error: {}", error.error_type)
                } else {
                    error.message
                };
                return SseParseAction::Error(msg);
            }

            typed::Event::Ping | typed::Event::Unknown => {}
        }

        emit_or_continue(events)
    }

    fn provider_name(&self) -> &'static str {
        "Claude"
    }
}

pub async fn send_message(request: &SendMessageRequest<'_>) -> Result<()> {
    let client = http_client();
    let retry_config = RetryConfig::default();
    let config = request.config;

    let body = build_request_body(ClaudeRequestBodyInput {
        model: config.model().as_str(),
        messages: request.messages,
        limits: request.limits,
        system_prompt: request.system_prompt,
        system_cache_hint: request.system_cache_hint,
        tools: request.tools,
        cache_last_tool: request.cache_last_tool,
        thinking_mode: config.anthropic_thinking_mode(),
        thinking_effort: config.anthropic_thinking_effort(),
    });
    let beta_header = anthropic_beta_header(config.model().as_str(), request.limits);

    let api_key = config.api_key().to_string();
    let body_json = body;

    let mut parser = ClaudeParser::default();
    send_retried_sse_request(
        || {
            let mut request = client
                .post(API_URL)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json");

            if let Some(beta) = beta_header {
                request = request.header("anthropic-beta", beta);
            }

            request.json(&body_json)
        },
        &retry_config,
        &request.tx,
        &mut parser,
        crate::stream_idle_timeout(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        CacheHint, CacheableMessage, ClaudeParser, ClaudeRequestBodyInput, Message, OutputLimits,
        SseParseAction, SseParser, StreamEvent, ToolDefinition, anthropic_beta_header,
        build_request_body, json,
    };
    use forge_types::Provider;
    use forge_types::{ModelName, NonEmptyString};

    #[test]
    fn hoists_system_messages_into_system_blocks() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(1024);

        let messages = vec![
            CacheableMessage::plain(Message::system(NonEmptyString::new("Distillate").unwrap())),
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

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

        // With Default hint, system prompt should NOT have cache_control
        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: Some("prompt"),
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        let system = body.get("system").unwrap().as_array().unwrap();
        assert_eq!(system.len(), 2);
        assert_eq!(system[0]["text"].as_str(), Some("prompt"));
        assert!(system[0].get("cache_control").is_none());
        assert_eq!(system[1]["text"].as_str(), Some("Distillate"));
    }

    #[test]
    fn system_prompt_cached_when_hint_ephemeral() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(1024);
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: Some("prompt"),
            system_cache_hint: CacheHint::Ephemeral,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        let system = body.get("system").unwrap().as_array().unwrap();
        assert_eq!(
            system[0]["cache_control"]["type"].as_str(),
            Some("ephemeral")
        );
        assert_eq!(system[0]["cache_control"]["ttl"].as_str(), Some("1h"));
    }

    #[test]
    fn tool_schema_cache_control_with_ttl() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(1024);
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];
        let tools = vec![
            ToolDefinition::new("tool_a", "desc a", json!({"type": "object"})),
            ToolDefinition::new("tool_b", "desc b", json!({"type": "object"})),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: Some(&tools),
            cache_last_tool: true,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        let api_tools = body["tools"].as_array().unwrap();
        assert_eq!(api_tools.len(), 2);
        // Only last tool has cache_control
        assert!(api_tools[0].get("cache_control").is_none());
        assert_eq!(
            api_tools[1]["cache_control"]["type"].as_str(),
            Some("ephemeral")
        );
        assert_eq!(api_tools[1]["cache_control"]["ttl"].as_str(), Some("1h"));
    }

    #[test]
    fn message_cache_control_has_ttl() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(1024);
        let messages = vec![
            CacheableMessage::cached(Message::try_user("cached msg").unwrap()),
            CacheableMessage::plain(Message::try_user("plain msg").unwrap()),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        let api_messages = body["messages"].as_array().unwrap();
        let cached_content = api_messages[0]["content"][0].clone();
        assert_eq!(
            cached_content["cache_control"]["type"].as_str(),
            Some("ephemeral")
        );
        assert_eq!(cached_content["cache_control"]["ttl"].as_str(), Some("1h"));

        let plain_content = api_messages[1]["content"][0].clone();
        assert!(plain_content.get("cache_control").is_none());
    }

    #[test]
    fn opus_4_6_default_adaptive_and_max_effort() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::with_thinking(16_000, 4096).unwrap();
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        assert_eq!(body["thinking"]["type"].as_str(), Some("adaptive"));
        assert_eq!(body["output_config"]["effort"].as_str(), Some("max"));
        // Server-side compaction is disabled (conflicts with client-side distillation)
        assert!(body.get("context_management").is_none());
    }

    #[test]
    fn opus_4_6_uses_adaptive_even_when_thinking_not_requested() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(16_000);
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        assert_eq!(body["thinking"]["type"].as_str(), Some("adaptive"));
        assert_eq!(body["output_config"]["effort"].as_str(), Some("max"));
    }

    #[test]
    fn opus_4_6_disabled_thinking_mode() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(16_000);
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "disabled",
            thinking_effort: "max",
        });

        assert_eq!(body["thinking"]["type"].as_str(), Some("disabled"));
        // No effort when thinking is disabled
        assert!(body.get("output_config").is_none());
        // Server-side compaction is disabled (conflicts with client-side distillation)
        assert!(body.get("context_management").is_none());
    }

    #[test]
    fn opus_4_6_enabled_thinking_with_budget() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::with_thinking(16_000, 4096).unwrap();
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "enabled",
            thinking_effort: "high",
        });

        assert_eq!(body["thinking"]["type"].as_str(), Some("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(4096));
        assert_eq!(body["output_config"]["effort"].as_str(), Some("high"));
    }

    #[test]
    fn opus_4_6_effort_levels() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(16_000);
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        for effort in &["low", "medium", "high", "max"] {
            let body = build_request_body(ClaudeRequestBodyInput {
                model: model.as_str(),
                messages: &messages,
                limits,
                system_prompt: None,
                system_cache_hint: CacheHint::Default,
                tools: None,
                cache_last_tool: false,
                thinking_mode: "adaptive",
                thinking_effort: effort,
            });
            assert_eq!(body["output_config"]["effort"].as_str(), Some(*effort));
        }
    }

    #[test]
    fn non_opus_4_6_thinking_keeps_budget_tokens() {
        let model: ModelName = Provider::Claude
            .parse_model("claude-haiku-4-5-20251001")
            .unwrap();
        let limits = OutputLimits::with_thinking(16_000, 4096).unwrap();
        let messages = vec![CacheableMessage::plain(Message::try_user("hi").unwrap())];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });

        assert_eq!(body["thinking"]["type"].as_str(), Some("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(4096));
        assert_eq!(
            body["context_management"]["edits"][0]["type"].as_str(),
            Some("clear_thinking_20251015")
        );
    }

    #[test]
    fn opus_4_6_drops_trailing_assistant_prefill() {
        let model = Provider::Claude.default_model();
        let limits = OutputLimits::new(1024);
        let messages = vec![
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
            CacheableMessage::plain(Message::assistant(
                model.clone(),
                NonEmptyString::new("prefill").unwrap(),
            )),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });
        let request_messages = body["messages"].as_array().unwrap();

        assert_eq!(request_messages.len(), 1);
        assert_eq!(request_messages[0]["role"].as_str(), Some("user"));
    }

    #[test]
    fn anthropic_beta_header_sets_context_1m_for_opus_4_6() {
        let limits = OutputLimits::with_thinking(16_000, 4096).unwrap();
        assert_eq!(
            anthropic_beta_header("claude-opus-4-6", limits),
            Some("context-1m-2025-08-07")
        );
    }

    #[test]
    fn anthropic_beta_header_kept_for_legacy_thinking_models() {
        let limits = OutputLimits::with_thinking(16_000, 4096).unwrap();
        assert_eq!(
            anthropic_beta_header("claude-haiku-4-5-20251001", limits),
            Some("interleaved-thinking-2025-05-14,context-management-2025-06-27")
        );
    }

    #[test]
    fn tool_result_cache_hint_adds_cache_control() {
        let model = Provider::Claude.default_model();
        let result = forge_types::ToolResult::success("call_1", "Read", "file contents");
        let limits = OutputLimits::new(1024);
        let messages = vec![
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
            CacheableMessage::plain(Message::assistant(
                model.clone(),
                NonEmptyString::new("I'll read that file").unwrap(),
            )),
            CacheableMessage::cached(Message::tool_result(result)),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });
        let api_messages = body["messages"].as_array().unwrap();

        // tool_result is the last user-role message
        let tool_msg = &api_messages[api_messages.len() - 1];
        let block = &tool_msg["content"][0];
        assert_eq!(block["type"].as_str(), Some("tool_result"));
        assert_eq!(block["cache_control"]["type"].as_str(), Some("ephemeral"));
    }

    #[test]
    fn tool_result_plain_has_no_cache_control() {
        let model = Provider::Claude.default_model();
        let result = forge_types::ToolResult::success("call_1", "Read", "file contents");
        let limits = OutputLimits::new(1024);
        let messages = vec![
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
            CacheableMessage::plain(Message::tool_result(result)),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });
        let api_messages = body["messages"].as_array().unwrap();
        let tool_msg = &api_messages[api_messages.len() - 1];
        let block = &tool_msg["content"][0];
        assert_eq!(block["type"].as_str(), Some("tool_result"));
        assert!(block.get("cache_control").is_none());
    }

    #[test]
    fn assistant_group_cache_hint_on_last_block() {
        let model = Provider::Claude.default_model();
        let tool_call = forge_types::ToolCall {
            id: "call_1".to_string(),
            name: "Read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test"}),
            thought_signature: forge_types::ThoughtSignatureState::Unsigned,
        };
        let limits = OutputLimits::new(1024);
        let messages = vec![
            CacheableMessage::plain(Message::try_user("hi").unwrap()),
            // Assistant text + tool_use grouped into one API message.
            // Mark the tool_use as cached — cache_control goes on last block.
            CacheableMessage::plain(Message::assistant(
                model.clone(),
                NonEmptyString::new("Let me read that").unwrap(),
            )),
            CacheableMessage::cached(Message::tool_use(tool_call)),
            // ToolResult triggers flush of the assistant group above.
            CacheableMessage::plain(Message::tool_result(forge_types::ToolResult::success(
                "call_1", "Read", "contents",
            ))),
        ];

        let body = build_request_body(ClaudeRequestBodyInput {
            model: model.as_str(),
            messages: &messages,
            limits,
            system_prompt: None,
            system_cache_hint: CacheHint::Default,
            tools: None,
            cache_last_tool: false,
            thinking_mode: "adaptive",
            thinking_effort: "max",
        });
        let api_messages = body["messages"].as_array().unwrap();

        // Find the assistant message (should have text + tool_use blocks)
        let assistant_msg = api_messages
            .iter()
            .find(|m| m["role"].as_str() == Some("assistant"))
            .unwrap();
        let content = assistant_msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        // First block (text) should NOT have cache_control
        assert!(content[0].get("cache_control").is_none());
        // Last block (tool_use) SHOULD have cache_control
        assert_eq!(
            content[1]["cache_control"]["type"].as_str(),
            Some("ephemeral")
        );
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

    #[test]
    fn claude_parser_continues_through_compaction() {
        let mut parser = ClaudeParser::default();

        // Phase 1: Pre-compaction content
        let text_event = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "before compaction" }
        });
        match parser.parse(&text_event) {
            SseParseAction::Emit(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::TextDelta(t) if t == "before compaction")
                );
            }
            _ => panic!("Expected Emit for pre-compaction text"),
        }

        // Phase 2: Server signals compaction via message_delta
        let compaction_delta = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "compaction" },
            "usage": { "output_tokens": 42 }
        });
        match parser.parse(&compaction_delta) {
            SseParseAction::Emit(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], StreamEvent::Usage(u) if u.output_tokens == 42));
            }
            _ => panic!("Expected Emit with usage for compaction delta"),
        }
        assert!(parser.compacting, "compacting flag should be set");

        // Phase 3: message_stop during compaction should NOT end the stream
        let message_stop = serde_json::json!({ "type": "message_stop" });
        match parser.parse(&message_stop) {
            SseParseAction::Continue => {}
            SseParseAction::Done => {
                panic!("message_stop during compaction must not signal Done")
            }
            other => panic!("Expected Continue, got {other:?}"),
        }
        assert!(
            !parser.compacting,
            "compacting flag should be cleared after message_stop"
        );

        // Phase 4: New message_start from compacted continuation
        let new_start = serde_json::json!({
            "type": "message_start",
            "message": {
                "usage": {
                    "input_tokens": 500,
                    "cache_read_input_tokens": 200,
                    "cache_creation_input_tokens": 0
                }
            }
        });
        match parser.parse(&new_start) {
            SseParseAction::Emit(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], StreamEvent::Usage(u) if u.input_tokens == 700));
            }
            _ => panic!("Expected Emit for post-compaction message_start"),
        }

        // Phase 5: Post-compaction content continues normally
        let post_text = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "after compaction" }
        });
        match parser.parse(&post_text) {
            SseParseAction::Emit(events) => {
                assert_eq!(events.len(), 1);
                assert!(matches!(&events[0], StreamEvent::TextDelta(t) if t == "after compaction"));
            }
            _ => panic!("Expected Emit for post-compaction text"),
        }

        // Phase 6: Normal message_stop after compaction ends the stream
        match parser.parse(&message_stop) {
            SseParseAction::Done => {}
            _ => panic!("Final message_stop should signal Done"),
        }
    }

    #[test]
    fn claude_parser_normal_stop_without_compaction() {
        let mut parser = ClaudeParser::default();

        let end_turn_delta = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": 100 }
        });
        parser.parse(&end_turn_delta);

        let message_stop = serde_json::json!({ "type": "message_stop" });
        match parser.parse(&message_stop) {
            SseParseAction::Done => {}
            _ => panic!("Normal message_stop should signal Done"),
        }
    }
}

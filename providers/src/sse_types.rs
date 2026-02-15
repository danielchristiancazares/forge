//! Typed SSE event structures for provider responses.
//!
//! These types enable compile-time validation of provider JSON responses.
//! Parse errors happen at the serde boundary, not scattered through parsing logic.
//!
//! # Design
//!
//! Each provider module defines:
//! - An event enum tagged by the `type` field
//! - Supporting structs for nested data
//! - `#[serde(default)]` for optional fields with sensible defaults
//!
//! This eliminates:
//! - Stringly-typed JSON key access
//! - Runtime `unwrap_or()` sentinel values
//! - Deep `if let Some(...)` chains

pub mod claude {
    use serde::Deserialize;

    /// Top-level Claude SSE event, tagged by `type` field.
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum Event {
        MessageStart {
            message: MessageInfo,
        },
        MessageDelta {
            delta: Option<MessageDeltaInfo>,
            usage: Option<OutputUsage>,
        },
        ContentBlockStart {
            index: u32,
            content_block: ContentBlock,
        },
        ContentBlockDelta {
            index: u32,
            delta: Delta,
        },
        ContentBlockStop {
            index: u32,
        },
        MessageStop,
        /// Ping events (keepalive)
        Ping,
        Error {
            error: ErrorInfo,
        },
        /// Unknown event type - allows forward compatibility
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    pub struct ErrorInfo {
        #[serde(default, rename = "type")]
        pub error_type: String,
        #[serde(default)]
        pub message: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct MessageInfo {
        pub usage: Option<InputUsage>,
    }

    /// Input token usage from message_start.
    ///
    /// Note: Anthropic's `input_tokens` is non-cached tokens only.
    /// Total input = input_tokens + cache_read + cache_creation
    #[derive(Debug, Deserialize, Default)]
    pub struct InputUsage {
        #[serde(default)]
        pub input_tokens: u32,
        #[serde(default)]
        pub cache_read_input_tokens: u32,
        #[serde(default)]
        pub cache_creation_input_tokens: u32,
    }

    impl InputUsage {
        /// Total input tokens including cached.
        #[must_use]
        pub fn total_input_tokens(&self) -> u32 {
            self.input_tokens
                .saturating_add(self.cache_read_input_tokens)
                .saturating_add(self.cache_creation_input_tokens)
        }
    }

    /// Output token usage from message_delta.
    #[derive(Debug, Deserialize, Default)]
    pub struct OutputUsage {
        #[serde(default)]
        pub output_tokens: u32,
    }

    /// Stop reason from message_delta's `delta` field.
    #[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
    #[serde(rename_all = "snake_case")]
    pub enum StopReason {
        EndTurn,
        MaxTokens,
        StopSequence,
        ToolUse,
        Compaction,
        #[serde(other)]
        Unknown,
    }

    /// The `delta` object inside a `message_delta` event.
    #[derive(Debug, Deserialize)]
    pub struct MessageDeltaInfo {
        #[serde(default)]
        pub stop_reason: Option<StopReason>,
    }

    /// Content block in content_block_start.
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum ContentBlock {
        Text {
            text: String,
        },
        ToolUse {
            id: String,
            name: String,
        },
        Thinking {
            thinking: String,
        },
        /// Unknown block type - forward compatibility
        #[serde(other)]
        Unknown,
    }

    /// Delta in content_block_delta.
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum Delta {
        TextDelta {
            text: String,
        },
        ThinkingDelta {
            thinking: String,
        },
        SignatureDelta {
            signature: String,
        },
        InputJsonDelta {
            partial_json: String,
        },
        /// Unknown delta type - forward compatibility
        #[serde(other)]
        Unknown,
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn deserialize_message_start() {
            let json = r#"{
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 100,
                        "cache_read_input_tokens": 50,
                        "cache_creation_input_tokens": 25
                    }
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::MessageStart { message } => {
                    let usage = message.usage.unwrap();
                    assert_eq!(usage.input_tokens, 100);
                    assert_eq!(usage.cache_read_input_tokens, 50);
                    assert_eq!(usage.total_input_tokens(), 175);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_content_block_start_tool_use() {
            let json = r#"{
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "Read"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::ContentBlockStart { content_block, .. } => match content_block {
                    ContentBlock::ToolUse { id, name } => {
                        assert_eq!(id, "toolu_123");
                        assert_eq!(name, "Read");
                    }
                    _ => panic!("wrong block type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_text_delta() {
            let json = r#"{
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "text_delta",
                    "text": "Hello"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::ContentBlockDelta { delta, .. } => match delta {
                    Delta::TextDelta { text } => assert_eq!(text, "Hello"),
                    _ => panic!("wrong delta type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_thinking_delta() {
            let json = r#"{
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "thinking_delta",
                    "thinking": "Let me think..."
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::ContentBlockDelta { delta, .. } => match delta {
                    Delta::ThinkingDelta { thinking } => assert_eq!(thinking, "Let me think..."),
                    _ => panic!("wrong delta type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_input_json_delta() {
            let json = r#"{
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"path\":"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::ContentBlockDelta { delta, .. } => match delta {
                    Delta::InputJsonDelta { partial_json } => {
                        assert_eq!(partial_json, "{\"path\":");
                    }
                    _ => panic!("wrong delta type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_message_stop() {
            let json = r#"{"type": "message_stop"}"#;
            let event: Event = serde_json::from_str(json).unwrap();
            assert!(matches!(event, Event::MessageStop));
        }

        #[test]
        fn unknown_event_type_deserializes() {
            let json = r#"{"type": "future_event", "data": 123}"#;
            let event: Event = serde_json::from_str(json).unwrap();
            assert!(matches!(event, Event::Unknown));
        }

        #[test]
        fn missing_usage_fields_default_to_zero() {
            let json = r#"{
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 100
                    }
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::MessageStart { message } => {
                    let usage = message.usage.unwrap();
                    assert_eq!(usage.input_tokens, 100);
                    assert_eq!(usage.cache_read_input_tokens, 0);
                    assert_eq!(usage.cache_creation_input_tokens, 0);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_message_delta_with_compaction_stop_reason() {
            let json = r#"{
                "type": "message_delta",
                "delta": {"stop_reason": "compaction"},
                "usage": {"output_tokens": 42}
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::MessageDelta { delta, usage } => {
                    let info = delta.unwrap();
                    assert_eq!(info.stop_reason, Some(StopReason::Compaction));
                    assert_eq!(usage.unwrap().output_tokens, 42);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_message_delta_with_end_turn_stop_reason() {
            let json = r#"{
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 100}
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::MessageDelta { delta, usage } => {
                    let info = delta.unwrap();
                    assert_eq!(info.stop_reason, Some(StopReason::EndTurn));
                    assert_eq!(usage.unwrap().output_tokens, 100);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_message_delta_unknown_stop_reason() {
            let json = r#"{
                "type": "message_delta",
                "delta": {"stop_reason": "future_reason"},
                "usage": {"output_tokens": 0}
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::MessageDelta { delta, .. } => {
                    let info = delta.unwrap();
                    assert_eq!(info.stop_reason, Some(StopReason::Unknown));
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_error_event() {
            let json = r#"{
                "type": "error",
                "error": {"type": "overloaded_error", "message": "Overloaded"}
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::Error { error } => {
                    assert_eq!(error.error_type, "overloaded_error");
                    assert_eq!(error.message, "Overloaded");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn error_event_does_not_become_unknown() {
            let json = r#"{
                "type": "error",
                "error": {"type": "api_error", "message": "Internal server error"}
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            assert!(!matches!(event, Event::Unknown));
        }
    }
}

pub mod openai {
    use serde::Deserialize;

    /// Top-level OpenAI SSE event, tagged by `type` field.
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum Event {
        #[serde(rename = "response.created")]
        Created {
            response: Option<ResponseInfo>,
            sequence_number: Option<u64>,
        },
        #[serde(rename = "response.in_progress")]
        InProgress {
            response: Option<ResponseInfo>,
            sequence_number: Option<u64>,
        },
        #[serde(rename = "response.output_item.added")]
        OutputItemAdded {
            item_id: Option<String>,
            #[serde(alias = "output_item")]
            item: Option<OutputItem>,
        },
        #[serde(rename = "response.output_item.done")]
        OutputItemDone {
            #[serde(alias = "output_item")]
            item: Option<OutputItem>,
        },
        #[serde(rename = "response.output_text.delta")]
        OutputTextDelta {
            item_id: Option<String>,
            delta: Option<String>,
        },
        #[serde(rename = "response.output_text.done")]
        OutputTextDone {
            item_id: Option<String>,
            text: Option<String>,
        },
        #[serde(rename = "response.refusal.delta")]
        RefusalDelta {
            item_id: Option<String>,
            delta: Option<String>,
        },
        #[serde(rename = "response.reasoning_summary_text.delta")]
        ReasoningSummaryDelta {
            item_id: Option<String>,
            delta: Option<String>,
        },
        #[serde(rename = "response.reasoning_summary_text.done")]
        ReasoningSummaryDone {
            item_id: Option<String>,
            text: Option<String>,
        },
        #[serde(rename = "response.reasoning_summary_part.added")]
        ReasoningSummaryPartAdded {
            item_id: Option<String>,
            part: Option<ReasoningPart>,
        },
        #[serde(rename = "response.reasoning_summary_part.done")]
        ReasoningSummaryPartDone {
            item_id: Option<String>,
            part: Option<ReasoningPart>,
        },
        #[serde(rename = "response.function_call_arguments.delta")]
        FunctionCallArgumentsDelta {
            item_id: Option<String>,
            call_id: Option<String>,
            delta: Option<String>,
        },
        #[serde(rename = "response.function_call_arguments.done")]
        FunctionCallArgumentsDone {
            item_id: Option<String>,
            call_id: Option<String>,
            arguments: Option<String>,
        },
        #[serde(rename = "response.completed")]
        Completed { response: Option<ResponseInfo> },
        #[serde(rename = "response.incomplete")]
        Incomplete { response: Option<ResponseInfo> },
        #[serde(rename = "response.failed")]
        Failed {
            response: Option<ResponseInfo>,
            error: Option<ErrorInfo>,
        },
        #[serde(rename = "error")]
        Error { error: Option<ErrorInfo> },
        /// Unknown event type - forward compatibility
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    pub enum OutputItem {
        #[serde(rename = "function_call")]
        FunctionCall {
            id: Option<String>,
            call_id: Option<String>,
            name: Option<String>,
            arguments: Option<String>,
        },
        #[serde(rename = "message")]
        Message { content: Option<Vec<ContentPart>> },
        #[serde(rename = "reasoning")]
        Reasoning {
            id: Option<String>,
            #[serde(default)]
            summary: Vec<ReasoningPart>,
            encrypted_content: Option<String>,
        },
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    pub struct ContentPart {
        #[serde(rename = "type")]
        pub content_type: Option<String>,
        pub text: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ReasoningPart {
        #[serde(rename = "type")]
        pub part_type: Option<String>,
        pub text: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ResponseInfo {
        pub id: Option<String>,
        pub usage: Option<Usage>,
        pub error: Option<ErrorInfo>,
        pub incomplete_details: Option<IncompleteDetails>,
    }

    #[derive(Debug, Deserialize, Default)]
    pub struct Usage {
        #[serde(default)]
        pub input_tokens: u32,
        #[serde(default)]
        pub output_tokens: u32,
        pub input_tokens_details: Option<InputTokensDetails>,
    }

    #[derive(Debug, Deserialize, Default)]
    pub struct InputTokensDetails {
        #[serde(default)]
        pub cached_tokens: u32,
    }

    #[derive(Debug, Deserialize)]
    pub struct ErrorInfo {
        pub message: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct IncompleteDetails {
        pub reason: Option<String>,
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn deserialize_output_item_function_call() {
            let json = r#"{
                "type": "response.output_item.added",
                "item_id": "item_1",
                "item": {
                    "type": "function_call",
                    "id": "item_1",
                    "call_id": "call_1",
                    "name": "Read",
                    "arguments": "{\"path\":\"foo\"}"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::OutputItemAdded { item, .. } => match item.unwrap() {
                    OutputItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        assert_eq!(call_id.unwrap(), "call_1");
                        assert_eq!(name.unwrap(), "Read");
                        assert_eq!(arguments.unwrap(), "{\"path\":\"foo\"}");
                    }
                    _ => panic!("wrong item type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_text_delta() {
            let json = r#"{
                "type": "response.output_text.delta",
                "item_id": "item_1",
                "delta": "Hello"
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::OutputTextDelta { delta, .. } => {
                    assert_eq!(delta.unwrap(), "Hello");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_completed_with_usage() {
            let json = r#"{
                "type": "response.completed",
                "response": {
                    "usage": {
                        "input_tokens": 1234,
                        "output_tokens": 567,
                        "input_tokens_details": {
                            "cached_tokens": 100
                        }
                    }
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::Completed { response } => {
                    let usage = response.unwrap().usage.unwrap();
                    assert_eq!(usage.input_tokens, 1234);
                    assert_eq!(usage.output_tokens, 567);
                    assert_eq!(usage.input_tokens_details.unwrap().cached_tokens, 100);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_error() {
            let json = r#"{
                "type": "error",
                "error": {
                    "message": "Something went wrong"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::Error { error } => {
                    assert_eq!(error.unwrap().message.unwrap(), "Something went wrong");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_response_created() {
            let json = r#"{
                "type": "response.created",
                "response": {
                    "id": "resp_abc123",
                    "usage": null
                },
                "sequence_number": 0
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::Created {
                    response,
                    sequence_number,
                } => {
                    assert_eq!(response.unwrap().id.unwrap(), "resp_abc123");
                    assert_eq!(sequence_number.unwrap(), 0);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn unknown_event_deserializes() {
            let json = r#"{"type": "response.future_event", "data": 123}"#;
            let event: Event = serde_json::from_str(json).unwrap();
            assert!(matches!(event, Event::Unknown));
        }

        #[test]
        fn deserialize_output_item_done_reasoning() {
            let json = r#"{
                "type": "response.output_item.done",
                "item": {
                    "type": "reasoning",
                    "id": "rs_abc",
                    "summary": [
                        {
                            "type": "summary_text",
                            "text": "reasoning summary"
                        }
                    ],
                    "encrypted_content": "encrypted_data_here"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::OutputItemDone { item } => match item.unwrap() {
                    OutputItem::Reasoning {
                        id,
                        summary,
                        encrypted_content,
                    } => {
                        assert_eq!(id.unwrap(), "rs_abc");
                        assert_eq!(summary.len(), 1);
                        assert_eq!(summary[0].part_type.as_deref(), Some("summary_text"));
                        assert_eq!(summary[0].text.as_deref(), Some("reasoning summary"));
                        assert_eq!(encrypted_content.unwrap(), "encrypted_data_here");
                    }
                    _ => panic!("wrong item type"),
                },
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn deserialize_reasoning_summary_part_done() {
            let json = r#"{
                "type": "response.reasoning_summary_part.done",
                "item_id": "item_1",
                "part": {
                    "type": "summary_text",
                    "text": "final summary part"
                }
            }"#;
            let event: Event = serde_json::from_str(json).unwrap();
            match event {
                Event::ReasoningSummaryPartDone { item_id, part } => {
                    assert_eq!(item_id.as_deref(), Some("item_1"));
                    let part = part.expect("part");
                    assert_eq!(part.part_type.as_deref(), Some("summary_text"));
                    assert_eq!(part.text.as_deref(), Some("final summary part"));
                }
                _ => panic!("wrong event type"),
            }
        }
    }
}

pub mod gemini {
    use serde::Deserialize;

    /// Top-level Gemini SSE response.
    ///
    /// Gemini doesn't use event types like Claude/OpenAI. Instead, each
    /// SSE chunk is a complete response object with candidates.
    /// Token usage data returned by Gemini API.
    #[derive(Debug, Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    pub struct UsageMetadata {
        #[serde(default)]
        pub prompt_token_count: u32,
        #[serde(default)]
        pub candidates_token_count: u32,
        #[serde(default)]
        pub total_token_count: u32,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Response {
        pub candidates: Option<Vec<Candidate>>,
        pub error: Option<ErrorInfo>,
        pub usage_metadata: Option<UsageMetadata>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Candidate {
        pub content: Option<Content>,
        pub finish_reason: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Content {
        pub parts: Option<Vec<Part>>,
    }

    /// A content part in a Gemini response.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Part {
        /// Text content (mutually exclusive with function_call)
        pub text: Option<String>,
        /// Whether this is thinking content
        #[serde(default)]
        pub thought: bool,
        /// Function call (mutually exclusive with text)
        pub function_call: Option<FunctionCall>,
        /// Thought signature for function calls in thinking mode
        pub thought_signature: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct FunctionCall {
        pub name: Option<String>,
        pub args: Option<serde_json::Value>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ErrorInfo {
        pub message: Option<String>,
        pub code: Option<i32>,
    }

    impl ErrorInfo {
        #[must_use]
        pub fn message_or_default(&self) -> &str {
            self.message.as_deref().unwrap_or("Unknown error")
        }
    }

    /// Known Gemini finish reasons.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FinishReason {
        Stop,
        MaxTokens,
        Safety,
        Recitation,
        Language,
        Blocklist,
        ProhibitedContent,
        Spii,
        MalformedFunctionCall,
        MissingThoughtSignature,
        TooManyToolCalls,
        UnexpectedToolCall,
        Other,
        Unknown,
    }

    impl FinishReason {
        #[must_use]
        pub fn parse(s: &str) -> Self {
            match s {
                "STOP" => Self::Stop,
                "MAX_TOKENS" => Self::MaxTokens,
                "SAFETY" => Self::Safety,
                "RECITATION" => Self::Recitation,
                "LANGUAGE" => Self::Language,
                "BLOCKLIST" => Self::Blocklist,
                "PROHIBITED_CONTENT" => Self::ProhibitedContent,
                "SPII" => Self::Spii,
                "MALFORMED_FUNCTION_CALL" => Self::MalformedFunctionCall,
                "MISSING_THOUGHT_SIGNATURE" => Self::MissingThoughtSignature,
                "TOO_MANY_TOOL_CALLS" => Self::TooManyToolCalls,
                "UNEXPECTED_TOOL_CALL" => Self::UnexpectedToolCall,
                "OTHER" => Self::Other,
                _ => Self::Unknown,
            }
        }

        /// Returns error message if this is an error reason, None if success.
        #[must_use]
        pub fn error_message(self) -> Option<&'static str> {
            match self {
                Self::Stop | Self::MaxTokens | Self::Unknown => None,
                Self::Safety => Some("Content filtered by safety settings"),
                Self::Recitation => Some("Response blocked: recitation"),
                Self::Language => Some("Unsupported language"),
                Self::Blocklist => Some("Content contains blocked terms"),
                Self::ProhibitedContent => Some("Prohibited content detected"),
                Self::Spii => Some("Sensitive PII detected"),
                Self::MalformedFunctionCall => Some("Invalid function call generated"),
                Self::MissingThoughtSignature => Some("Missing thought signature in request"),
                Self::TooManyToolCalls => Some("Too many consecutive tool calls"),
                Self::UnexpectedToolCall => Some("Tool call but no tools enabled"),
                Self::Other => Some("Generation stopped: unknown reason"),
            }
        }

        #[must_use]
        pub fn is_success(self) -> bool {
            matches!(self, Self::Stop | Self::MaxTokens)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn deserialize_text_response() {
            let json = r#"{
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Hello"}]
                    }
                }]
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let candidates = resp.candidates.unwrap();
            let parts = candidates[0]
                .content
                .as_ref()
                .unwrap()
                .parts
                .as_ref()
                .unwrap();
            assert_eq!(parts[0].text.as_deref().unwrap(), "Hello");
        }

        #[test]
        fn deserialize_thinking_response() {
            let json = r#"{
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Let me think...", "thought": true}]
                    }
                }]
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let candidates = resp.candidates.unwrap();
            let parts = candidates[0]
                .content
                .as_ref()
                .unwrap()
                .parts
                .as_ref()
                .unwrap();
            assert!(parts[0].thought);
            assert_eq!(parts[0].text.as_deref().unwrap(), "Let me think...");
        }

        #[test]
        fn deserialize_function_call() {
            let json = r#"{
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "Read",
                                "args": {"path": "foo"}
                            },
                            "thoughtSignature": "sig_123"
                        }]
                    }
                }]
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let candidates = resp.candidates.unwrap();
            let parts = candidates[0]
                .content
                .as_ref()
                .unwrap()
                .parts
                .as_ref()
                .unwrap();
            let fc = parts[0].function_call.as_ref().unwrap();
            assert_eq!(fc.name.as_deref().unwrap(), "Read");
            assert_eq!(parts[0].thought_signature.as_deref().unwrap(), "sig_123");
        }

        #[test]
        fn deserialize_finish_reason() {
            let json = r#"{
                "candidates": [{
                    "content": {"parts": [{"text": "Done"}]},
                    "finishReason": "STOP"
                }]
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let candidates = resp.candidates.unwrap();
            let reason = candidates[0].finish_reason.as_deref().unwrap();
            assert_eq!(FinishReason::parse(reason), FinishReason::Stop);
        }

        #[test]
        fn deserialize_error_response() {
            let json = r#"{
                "error": {
                    "message": "API key invalid",
                    "code": 401
                }
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let error = resp.error.unwrap();
            assert_eq!(error.message.unwrap(), "API key invalid");
            assert_eq!(error.code.unwrap(), 401);
        }

        #[test]
        fn finish_reason_error_messages() {
            assert!(FinishReason::Stop.error_message().is_none());
            assert!(FinishReason::MaxTokens.error_message().is_none());
            assert!(FinishReason::Safety.error_message().is_some());
        }

        #[test]
        fn deserialize_usage_metadata() {
            let json = r#"{
                "candidates": [{"content": {"parts": [{"text": "Hello"}]}}],
                "usageMetadata": {
                    "promptTokenCount": 100,
                    "candidatesTokenCount": 50,
                    "totalTokenCount": 150
                }
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            let meta = resp.usage_metadata.unwrap();
            assert_eq!(meta.prompt_token_count, 100);
            assert_eq!(meta.candidates_token_count, 50);
            assert_eq!(meta.total_token_count, 150);
        }

        #[test]
        fn missing_usage_metadata_defaults() {
            let json = r#"{
                "candidates": [{"content": {"parts": [{"text": "Hello"}]}}]
            }"#;
            let resp: Response = serde_json::from_str(json).unwrap();
            assert!(resp.usage_metadata.is_none());
        }
    }
}

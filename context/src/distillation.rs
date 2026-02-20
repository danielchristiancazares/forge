//! Conversation distillation via LLM.
//!
//! This module provides functionality to distill conversation segments
//! into compact distillates using a cheaper/faster LLM model. Distillation
//! preserves key facts, decisions, and context while reducing token count.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::json;

use forge_providers::{
    ApiConfig, CLAUDE_MESSAGES_API_URL, GEMINI_API_BASE_URL, OPENAI_RESPONSES_API_URL,
    http_client_with_timeout, read_capped_error_body,
    retry::{RetryConfig, RetryOutcome, send_with_retry},
};
use forge_types::{InternalModel, Message, Provider, ToolResultOutcome};

use super::token_counter::TokenCounter;

/// Models used for distillation (cheaper/faster than main models).
const CLAUDE_DISTILLATION_MODEL: InternalModel = InternalModel::ClaudeDistiller;
const OPENAI_DISTILLATION_MODEL: InternalModel = InternalModel::OpenAIDistiller;
/// Gemini 3.1 Pro Preview - use the same model for now (no cheaper variant available yet).
const GEMINI_DISTILLATION_MODEL: InternalModel = InternalModel::GeminiDistiller;

/// Context limits for distiller models (conservative to leave room for output + overhead).
/// Claude Haiku 4.5 has 200k context, we use 190k to leave room for output and system prompt.
const CLAUDE_DISTILLER_INPUT_LIMIT: u32 = 190_000;
/// GPT-5-nano has 400k context, we use 380k to leave room for output and system prompt.
const OPENAI_DISTILLER_INPUT_LIMIT: u32 = 380_000;
/// Gemini 3 Pro has 1M context, we use 950k to leave room for output and system prompt.
const GEMINI_DISTILLER_INPUT_LIMIT: u32 = 950_000;

/// Per-provider max output tokens for distillation.
/// Each value reflects the model's actual max output token limit.
fn max_distillation_tokens(provider: Provider) -> u32 {
    match provider {
        Provider::Claude => 64_000,  // Haiku 4.5: 64k max output
        Provider::OpenAI => 128_000, // GPT-5-nano: 128k max output
        Provider::Gemini => 65_536,  // Gemini 3 Pro: 65k max output
    }
}
const DISTILLATION_TIMEOUT_SECS: u64 = 600;

async fn retry_outcome_to_json(
    outcome: RetryOutcome,
    provider_label: &'static str,
    action: &'static str,
) -> Result<serde_json::Value> {
    let response = match outcome {
        RetryOutcome::Success(resp) => resp,
        RetryOutcome::HttpError(resp) => {
            let status = resp.status();
            let error_text = read_capped_error_body(resp).await;
            return Err(anyhow!("{provider_label} API error {status}: {error_text}"));
        }
        RetryOutcome::ConnectionError { attempts, source } => {
            return Err(anyhow!(
                "{provider_label} {action} failed after {attempts} attempts: {source}"
            ));
        }
        RetryOutcome::NonRetryable(e) => {
            return Err(anyhow!("{provider_label} {action} failed: {e}"));
        }
    };

    let json: serde_json::Value = response.json().await?;
    Ok(json)
}

/// Distillation prompt template loaded from context/assets/distillation.md
const DISTILLATION_PROMPT_TEMPLATE: &str = include_str!("../assets/distillation.md");

/// # Returns
/// A tuple of (`system_instruction`, `user_prompt`) for the API call.
/// The user prompt contains the full template with conversation log and file list inlined.
pub fn build_distillation_prompt(messages: &[Message]) -> (String, String) {
    let system_instruction = DISTILLATION_PROMPT_TEMPLATE.to_string();

    let mut conversation_log = String::new();
    let mut file_paths = BTreeSet::new();

    for (i, message) in messages.iter().enumerate() {
        let role = match message {
            Message::System(_) => "System",
            Message::User(_) => "User",
            Message::Assistant(_) => "Assistant",
            Message::Thinking(_) => continue,
            Message::ToolUse(call) => {
                extract_file_paths(&call.arguments, &mut file_paths);
                let _ = write!(
                    conversation_log,
                    "[{i}] Assistant (Tool Call: {}): {}\n\n",
                    call.name,
                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                );
                continue;
            }
            Message::ToolResult(result) => {
                let status = match result.outcome() {
                    ToolResultOutcome::Error => "Error",
                    ToolResultOutcome::Success => "Result",
                };
                let _ = write!(
                    conversation_log,
                    "[{i}] Tool {status}: {}\n\n",
                    result.content
                );
                continue;
            }
        };
        let _ = write!(conversation_log, "[{i}] {role}: {}\n\n", message.content());
    }

    let file_list = if file_paths.is_empty() {
        "(none detected)".to_string()
    } else {
        file_paths.into_iter().collect::<Vec<_>>().join("\n")
    };

    let user_message = format!("Conversation:\n{conversation_log}\nActive files:\n{file_list}");

    (system_instruction, user_message)
}

/// Extract file paths from tool call arguments.
fn extract_file_paths(args: &serde_json::Value, paths: &mut BTreeSet<String>) {
    if let Some(obj) = args.as_object() {
        for key in ["path", "file_path", "source", "destination"] {
            if let Some(serde_json::Value::String(p)) = obj.get(key)
                && !p.is_empty()
            {
                paths.insert(p.clone());
            }
        }
        // Glob/Search results sometimes have paths in nested arrays
        if let Some(serde_json::Value::Array(arr)) = obj.get("paths") {
            for item in arr {
                if let Some(p) = item.as_str() {
                    paths.insert(p.to_string());
                }
            }
        }
    }
}

pub fn distiller_input_limit(provider: Provider) -> u32 {
    match provider {
        Provider::Claude => CLAUDE_DISTILLER_INPUT_LIMIT,
        Provider::OpenAI => OPENAI_DISTILLER_INPUT_LIMIT,
        Provider::Gemini => GEMINI_DISTILLER_INPUT_LIMIT,
    }
}

/// Estimate the token count for a text string using the tokenizer.
fn count_tokens(counter: &TokenCounter, text: &str) -> u32 {
    counter.count_str(text)
}

/// Distill conversation messages into a compact summary using an LLM.
///
/// This function calls a cheaper/faster model to generate the distillation,
/// using the API key from the provided config but overriding the model.
///
/// # Errors
/// Returns an error if:
/// - The input messages exceed the distiller model's context limit
/// - The API call fails
/// - The response cannot be parsed
pub async fn generate_distillation(
    config: &ApiConfig,
    counter: &TokenCounter,
    messages: &[Message],
) -> Result<String> {
    if messages.is_empty() {
        return Ok(String::new());
    }

    let (system_instruction, conversation_text) = build_distillation_prompt(messages);

    // Validate that input doesn't exceed distiller model's context limit
    let estimated_input =
        count_tokens(counter, &system_instruction) + count_tokens(counter, &conversation_text);
    let input_limit = distiller_input_limit(config.provider());

    if estimated_input > input_limit {
        return Err(anyhow!(
            "Distillation scope too large: ~{} tokens exceeds {} model limit of {} tokens. \
             Consider distilling fewer messages at a time.",
            estimated_input,
            distillation_model(config.provider()),
            input_limit
        ));
    }

    let max_tokens = max_distillation_tokens(config.provider());

    match config.provider() {
        Provider::Claude => {
            generate_distillation_claude(
                config.api_key(),
                &system_instruction,
                &conversation_text,
                max_tokens,
            )
            .await
        }
        Provider::OpenAI => {
            generate_distillation_openai(
                config.api_key(),
                &system_instruction,
                &conversation_text,
                max_tokens,
            )
            .await
        }
        Provider::Gemini => {
            generate_distillation_gemini(
                config.api_key(),
                &system_instruction,
                &conversation_text,
                max_tokens,
            )
            .await
        }
    }
}

/// Generate distillation using Claude API (non-streaming).
async fn generate_distillation_claude(
    api_key: &str,
    system_instruction: &str,
    conversation_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = http_client_with_timeout(DISTILLATION_TIMEOUT_SECS)?;
    let retry_config = RetryConfig::default();
    let timeout = Duration::from_secs(DISTILLATION_TIMEOUT_SECS);

    let body = json!({
        "model": CLAUDE_DISTILLATION_MODEL.model_id(),
        "max_tokens": max_tokens,
        "stream": false,
        "system": system_instruction,
        "messages": [
            {
                "role": "user",
                "content": format!("Please distill the following conversation:\n\n{}", conversation_text)
            }
        ]
    });

    let api_key_str = api_key.to_string();
    let body_json = body.clone();

    // Wrap request with retry logic (REQ-4)
    let outcome = send_with_retry(
        || {
            client
                .post(CLAUDE_MESSAGES_API_URL)
                .header("x-api-key", &api_key_str)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body_json)
        },
        Some(timeout),
        &retry_config,
    )
    .await;

    let json = retry_outcome_to_json(outcome, "Claude", "distillation").await?;

    let distillation = json["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract distillation from Claude response: {json:?}"))?;

    Ok(distillation.to_string())
}

/// Generate distillation using `OpenAI` Responses API (non-streaming).
///
/// Uses the Responses API for consistency with the main provider module.
/// Request format: `{ model, instructions, input, max_output_tokens, stream: false }`
/// Response format: `{ output: [{ type: "message", content: [{ type: "output_text", text }] }] }`
async fn generate_distillation_openai(
    api_key: &str,
    system_instruction: &str,
    conversation_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = http_client_with_timeout(DISTILLATION_TIMEOUT_SECS)?;
    let retry_config = RetryConfig::default();
    let timeout = Duration::from_secs(DISTILLATION_TIMEOUT_SECS);

    // Responses API uses `input` array and `instructions` for system prompt
    let body = json!({
        "model": OPENAI_DISTILLATION_MODEL.model_id(),
        "stream": false,
        "max_output_tokens": max_tokens,
        "instructions": system_instruction,
        "input": [
            {
                "role": "user",
                "content": format!("Please distill the following conversation:\n\n{}", conversation_text)
            }
        ]
    });

    let auth_header = format!("Bearer {api_key}");
    let body_json = body.clone();

    // Wrap request with retry logic (REQ-4)
    let outcome = send_with_retry(
        || {
            client
                .post(OPENAI_RESPONSES_API_URL)
                .header("Authorization", &auth_header)
                .header("content-type", "application/json")
                .json(&body_json)
        },
        Some(timeout),
        &retry_config,
    )
    .await;

    let json = retry_outcome_to_json(outcome, "OpenAI", "distillation").await?;

    // Extract text from OpenAI Responses API format:
    // { "output": [{ "type": "reasoning", ... }, { "type": "message", "content": [{ "type": "output_text", "text": "..." }] }] }
    // Skip reasoning items and find the first message item.
    let distillation = json["output"]
        .as_array()
        .and_then(|arr| arr.iter().find(|item| item["type"] == "message"))
        .and_then(|item| item["content"].as_array())
        .and_then(|content| content.first())
        .and_then(|block| block["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract distillation from OpenAI response: {json:?}"))?;

    Ok(distillation.to_string())
}

/// Generate distillation using Gemini API (non-streaming).
///
/// Uses the `generateContent` endpoint without streaming.
/// Request format uses Gemini's unique structure with top-level `system_instruction`.
async fn generate_distillation_gemini(
    api_key: &str,
    system_instruction: &str,
    conversation_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = http_client_with_timeout(DISTILLATION_TIMEOUT_SECS)?;
    let retry_config = RetryConfig::default();
    let timeout = Duration::from_secs(DISTILLATION_TIMEOUT_SECS);

    // Gemini uses top-level system_instruction and mixed casing
    let body = json!({
        "system_instruction": {
            "parts": [{ "text": system_instruction }]
        },
        "contents": [
            {
                "role": "user",
                "parts": [{
                    "text": format!("Please distill the following conversation:\n\n{}", conversation_text)
                }]
            }
        ],
        "generationConfig": {
            "maxOutputTokens": max_tokens,
            "temperature": 1.0
        }
    });

    let url = format!(
        "{GEMINI_API_BASE_URL}/models/{}:generateContent",
        GEMINI_DISTILLATION_MODEL.model_id()
    );

    let api_key_str = api_key.to_string();
    let body_json = body.clone();

    // Wrap request with retry logic (REQ-4)
    let outcome = send_with_retry(
        || {
            client
                .post(&url)
                .header("x-goog-api-key", &api_key_str)
                .header("content-type", "application/json")
                .json(&body_json)
        },
        Some(timeout),
        &retry_config,
    )
    .await;

    let json = retry_outcome_to_json(outcome, "Gemini", "distillation").await?;

    // Extract text from Gemini's response format:
    // { "candidates": [{ "content": { "parts": [{ "text": "..." }] } }] }
    let distillation = json["candidates"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|candidate| candidate["content"]["parts"].as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract distillation from Gemini response: {json:?}"))?;

    Ok(distillation.to_string())
}

#[must_use]
pub fn distillation_model(provider: Provider) -> &'static str {
    distillation_internal_model(provider).model_id()
}

const fn distillation_internal_model(provider: Provider) -> InternalModel {
    match provider {
        Provider::Claude => CLAUDE_DISTILLATION_MODEL,
        Provider::OpenAI => OPENAI_DISTILLATION_MODEL,
        Provider::Gemini => GEMINI_DISTILLATION_MODEL,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiConfig, CLAUDE_DISTILLATION_MODEL, CLAUDE_DISTILLER_INPUT_LIMIT,
        OPENAI_DISTILLATION_MODEL, OPENAI_DISTILLER_INPUT_LIMIT, TokenCounter,
        build_distillation_prompt, distillation_model, distiller_input_limit,
        generate_distillation,
    };
    use std::time::SystemTime;

    use forge_types::{Message, Provider};

    #[test]
    fn test_distillation_model_selection() {
        assert_eq!(
            distillation_model(Provider::Claude),
            CLAUDE_DISTILLATION_MODEL.model_id()
        );
        assert_eq!(
            distillation_model(Provider::OpenAI),
            OPENAI_DISTILLATION_MODEL.model_id()
        );
    }

    #[test]
    fn test_build_prompt_preserves_message_order() {
        let messages = vec![
            Message::try_user("First message", SystemTime::now()).expect("non-empty test message"),
            Message::try_user("Second message", SystemTime::now()).expect("non-empty test message"),
            Message::try_user("Third message", SystemTime::now()).expect("non-empty test message"),
        ];

        let (_, prompt) = build_distillation_prompt(&messages);

        let first_pos = prompt.find("First message").unwrap();
        let second_pos = prompt.find("Second message").unwrap();
        let third_pos = prompt.find("Third message").unwrap();

        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[tokio::test]
    async fn test_generate_distillation_empty_messages() {
        use forge_types::ApiKey;

        let model = Provider::Claude
            .parse_model("claude-opus-4-6")
            .expect("parse model");
        let config = ApiConfig::new(ApiKey::claude("fake-key"), model).expect("config");
        let counter = TokenCounter::new();

        let messages: Vec<Message> = vec![];
        let result = generate_distillation(&config, &counter, &messages).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_distiller_input_limit_claude() {
        let limit = distiller_input_limit(Provider::Claude);
        assert_eq!(limit, CLAUDE_DISTILLER_INPUT_LIMIT);
        assert_eq!(limit, 190_000);
    }

    #[test]
    fn test_distiller_input_limit_openai() {
        let limit = distiller_input_limit(Provider::OpenAI);
        assert_eq!(limit, OPENAI_DISTILLER_INPUT_LIMIT);
        assert_eq!(limit, 380_000);
    }

    #[test]
    fn test_build_prompt_with_system_message() {
        use forge_types::NonEmptyString;

        let messages = vec![Message::system(
            NonEmptyString::new("You are a helpful assistant.").expect("msg"),
            SystemTime::now(),
        )];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("System:"));
        assert!(prompt.contains("You are a helpful assistant."));
    }

    #[test]
    fn test_build_prompt_with_assistant_message() {
        use forge_types::NonEmptyString;

        let messages = vec![Message::assistant(
            Provider::Claude.default_model(),
            NonEmptyString::new("Hello! How can I help?").expect("msg"),
            SystemTime::now(),
        )];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("Assistant:"));
        assert!(prompt.contains("Hello! How can I help?"));
    }

    #[test]
    fn test_build_prompt_with_tool_use() {
        use forge_types::ToolCall;
        use serde_json::json;

        let tool_call = ToolCall::new("call_123", "Read", json!({"path": "/tmp/test.txt"}));
        let messages = vec![Message::ToolUse(tool_call)];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("Tool Call: Read"));
        assert!(prompt.contains("/tmp/test.txt"));
    }

    #[test]
    fn test_build_prompt_with_tool_use_extracts_file_paths() {
        use forge_types::ToolCall;
        use serde_json::json;

        let messages = vec![
            Message::ToolUse(ToolCall::new("c1", "Read", json!({"path": "src/main.rs"}))),
            Message::ToolUse(ToolCall::new(
                "c2",
                "Edit",
                json!({"file_path": "src/lib.rs"}),
            )),
        ];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("src/main.rs"));
        assert!(!prompt.contains("(none detected)"));
    }

    #[test]
    fn test_build_prompt_with_tool_result_success() {
        use forge_types::ToolResult;

        let result = ToolResult {
            tool_call_id: "call_123".to_string(),
            tool_name: "Read".to_string(),
            content: "File contents here".to_string(),
            outcome: forge_types::ToolResultOutcome::Success,
        };
        let messages = vec![Message::ToolResult(result)];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("Tool Result:"));
        assert!(prompt.contains("File contents here"));
    }

    #[test]
    fn test_build_prompt_with_tool_result_error() {
        use forge_types::ToolResult;

        let result = ToolResult {
            tool_call_id: "call_456".to_string(),
            tool_name: "Read".to_string(),
            content: "File not found".to_string(),
            outcome: forge_types::ToolResultOutcome::Error,
        };
        let messages = vec![Message::ToolResult(result)];

        let (_, prompt) = build_distillation_prompt(&messages);
        assert!(prompt.contains("Tool Error:"));
        assert!(prompt.contains("File not found"));
    }
}

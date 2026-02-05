//! Conversation distillation via LLM.
//!
//! This module provides functionality to distill conversation segments
//! into compact distillates using a cheaper/faster LLM model. Distillation
//! preserves key facts, decisions, and context while reducing token count.

use std::fmt::Write;

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::json;

use forge_providers::{
    ApiConfig, http_client_with_timeout, read_capped_error_body,
    retry::{RetryConfig, RetryOutcome, send_with_retry},
};
use forge_types::{Message, Provider};

use super::MessageId;
use super::token_counter::TokenCounter;

/// Models used for distillation (cheaper/faster than main models).
const CLAUDE_DISTILLATION_MODEL: &str = "claude-haiku-4-5";
const OPENAI_DISTILLATION_MODEL: &str = "gpt-5-nano";
/// Gemini 3 Pro Preview - use the same model for now (no cheaper variant available yet).
const GEMINI_DISTILLATION_MODEL: &str = "gemini-3-pro-preview";

/// Context limits for distiller models (conservative to leave room for output + overhead).
/// Claude Haiku 4.5 has 200k context, we use 190k to leave room for output and system prompt.
const CLAUDE_DISTILLER_INPUT_LIMIT: u32 = 190_000;
/// GPT-5-nano has 400k context, we use 380k to leave room for output and system prompt.
const OPENAI_DISTILLER_INPUT_LIMIT: u32 = 380_000;
/// Gemini 3 Pro has 1M context, we use 950k to leave room for output and system prompt.
const GEMINI_DISTILLER_INPUT_LIMIT: u32 = 950_000;

const MIN_DISTILLATION_TOKENS: u32 = 64;
const MAX_DISTILLATION_TOKENS: u32 = 2048;
const DISTILLATION_TIMEOUT_SECS: u64 = 60;

/// API endpoints.
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Using Responses API for consistency with main provider (providers/src/lib.rs).
const OPENAI_API_URL: &str = "https://api.openai.com/v1/responses";
/// Gemini API endpoint (non-streaming).
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Build a distillation prompt for a slice of messages.
///
/// The prompt instructs the LLM to:
/// - Preserve key facts, decisions, and important context
/// - Maintain chronological flow of the conversation
/// - Stay within the target token count
/// - Use clear, concise language
///
/// # Arguments
/// * `messages` - Slice of (`MessageId`, Message) tuples to distill
/// * `target_tokens` - Target token count for the distillation
///
/// # Returns
/// A tuple of (`system_instruction`, `conversation_text`) for the API call.
pub fn build_distillation_prompt(
    messages: &[(MessageId, Message)],
    target_tokens: u32,
) -> (String, String) {
    let system_instruction = format!(
        r#"You are a conversation distiller. Your task is to create a concise distillate of the following conversation.

REQUIREMENTS:
1. Preserve all key facts, decisions, and important context
2. Maintain the chronological flow of topics discussed
3. Keep the Distillate under approximately {target_tokens} tokens
4. Use clear, direct language
5. Preserve any code snippets, file paths, or technical details that are essential
6. Note any unresolved questions or pending actions
7. Format as a coherent narrative, not bullet points

OUTPUT FORMAT:
Write the Distillate as a continuous narrative that captures the essence of the conversation. Start directly with the content - do not include preamble like "This conversation..." or "Distillate:"."#
    );

    let mut conversation_text = String::new();
    for (id, message) in messages {
        let role = match message {
            Message::System(_) => "System",
            Message::User(_) => "User",
            Message::Assistant(_) => "Assistant",
            Message::Thinking(_) => {
                // Skip thinking content in distillations - it's internal reasoning
                continue;
            }
            Message::ToolUse(call) => {
                let _ = write!(
                    conversation_text,
                    "[Message {}] Assistant (Tool Call: {}): {}\n\n",
                    id.as_u64(),
                    call.name,
                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                );
                continue;
            }
            Message::ToolResult(result) => {
                let status = if result.is_error { "Error" } else { "Result" };
                let _ = write!(
                    conversation_text,
                    "[Message {}] Tool {}: {}\n\n",
                    id.as_u64(),
                    status,
                    result.content
                );
                continue;
            }
        };
        let _ = write!(
            conversation_text,
            "[Message {}] {}: {}\n\n",
            id.as_u64(),
            role,
            message.content()
        );
    }

    (system_instruction, conversation_text)
}

/// Get the distiller model's input token limit for a provider.
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

/// Distill conversation messages into a compact Distillate using an LLM.
///
/// This function calls a cheaper/faster model to generate the distillation,
/// using the API key from the provided config but overriding the model.
///
/// # Arguments
/// * `config` - API configuration (provides the API key and determines provider)
/// * `counter` - Token counter for accurate token estimation
/// * `messages` - Slice of (`MessageId`, Message) tuples to distill
/// * `target_tokens` - Target token count for the distillation
///
/// # Returns
/// The generated distillation text.
///
/// # Errors
/// Returns an error if:
/// - The input messages exceed the distiller model's context limit
/// - The API call fails
/// - The response cannot be parsed
pub async fn generate_distillation(
    config: &ApiConfig,
    counter: &TokenCounter,
    messages: &[(MessageId, Message)],
    target_tokens: u32,
) -> Result<String> {
    if messages.is_empty() {
        return Ok(String::new());
    }

    let (system_instruction, conversation_text) =
        build_distillation_prompt(messages, target_tokens);

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

    let max_tokens = target_tokens.clamp(MIN_DISTILLATION_TOKENS, MAX_DISTILLATION_TOKENS);

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
        "model": CLAUDE_DISTILLATION_MODEL,
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
                .post(CLAUDE_API_URL)
                .header("x-api-key", &api_key_str)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body_json)
        },
        Some(timeout),
        &retry_config,
    )
    .await;

    let response = match outcome {
        RetryOutcome::Success(resp) => resp,
        RetryOutcome::HttpError(resp) => {
            let status = resp.status();
            let error_text = read_capped_error_body(resp).await;
            return Err(anyhow!("Claude API error {status}: {error_text}"));
        }
        RetryOutcome::ConnectionError { attempts, source } => {
            return Err(anyhow!(
                "Claude distillation failed after {attempts} attempts: {source}"
            ));
        }
        RetryOutcome::NonRetryable(e) => {
            return Err(anyhow!("Claude distillation failed: {e}"));
        }
    };

    let json: serde_json::Value = response.json().await?;

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
        "model": OPENAI_DISTILLATION_MODEL,
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
                .post(OPENAI_API_URL)
                .header("Authorization", &auth_header)
                .header("content-type", "application/json")
                .json(&body_json)
        },
        Some(timeout),
        &retry_config,
    )
    .await;

    let response = match outcome {
        RetryOutcome::Success(resp) => resp,
        RetryOutcome::HttpError(resp) => {
            let status = resp.status();
            let error_text = read_capped_error_body(resp).await;
            return Err(anyhow!("OpenAI API error {status}: {error_text}"));
        }
        RetryOutcome::ConnectionError { attempts, source } => {
            return Err(anyhow!(
                "OpenAI distillation failed after {attempts} attempts: {source}"
            ));
        }
        RetryOutcome::NonRetryable(e) => {
            return Err(anyhow!("OpenAI distillation failed: {e}"));
        }
    };

    let json: serde_json::Value = response.json().await?;

    // Extract text from OpenAI Responses API format:
    // { "output": [{ "type": "message", "content": [{ "type": "output_text", "text": "..." }] }] }
    let distillation = json["output"]
        .as_array()
        .and_then(|arr| arr.first())
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

    let url = format!("{GEMINI_API_BASE}/models/{GEMINI_DISTILLATION_MODEL}:generateContent");

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

    let response = match outcome {
        RetryOutcome::Success(resp) => resp,
        RetryOutcome::HttpError(resp) => {
            let status = resp.status();
            let error_text = read_capped_error_body(resp).await;
            return Err(anyhow!("Gemini API error {status}: {error_text}"));
        }
        RetryOutcome::ConnectionError { attempts, source } => {
            return Err(anyhow!(
                "Gemini distillation failed after {attempts} attempts: {source}"
            ));
        }
        RetryOutcome::NonRetryable(e) => {
            return Err(anyhow!("Gemini distillation failed: {e}"));
        }
    };

    let json: serde_json::Value = response.json().await?;

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

/// Get the distillation model name for a given provider.
#[must_use]
pub fn distillation_model(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => CLAUDE_DISTILLATION_MODEL,
        Provider::OpenAI => OPENAI_DISTILLATION_MODEL,
        Provider::Gemini => GEMINI_DISTILLATION_MODEL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_messages() -> Vec<(MessageId, Message)> {
        vec![
            (
                MessageId::new_for_test(0),
                Message::try_user("Hello, can you help me with Rust?")
                    .expect("non-empty test message"),
            ),
            (
                MessageId::new_for_test(1),
                Message::try_user("I need to understand lifetimes")
                    .expect("non-empty test message"),
            ),
        ]
    }

    #[test]
    fn test_build_distillation_prompt_basic() {
        let messages = make_test_messages();
        let (system, conversation) = build_distillation_prompt(&messages, 500);

        assert!(system.contains("distiller"));
        assert!(system.contains("500"));
        assert!(system.contains("Preserve"));
        assert!(system.contains("key facts"));

        assert!(conversation.contains("User:"));
        assert!(conversation.contains("Hello, can you help me with Rust?"));
        assert!(conversation.contains("lifetimes"));
        assert!(conversation.contains("[Message 0]"));
        assert!(conversation.contains("[Message 1]"));
    }

    #[test]
    fn test_build_distillation_prompt_empty() {
        let messages: Vec<(MessageId, Message)> = vec![];
        let (system, conversation) = build_distillation_prompt(&messages, 100);

        // System instruction should still be generated
        assert!(system.contains("distiller"));
        assert!(conversation.is_empty());
    }

    #[test]
    fn test_build_distillation_prompt_target_tokens_in_instruction() {
        let messages = make_test_messages();

        let (system_500, _) = build_distillation_prompt(&messages, 500);
        let (system_1000, _) = build_distillation_prompt(&messages, 1000);

        assert!(system_500.contains("500"));
        assert!(system_1000.contains("1000"));
    }

    #[test]
    fn test_distillation_model_selection() {
        assert_eq!(
            distillation_model(Provider::Claude),
            CLAUDE_DISTILLATION_MODEL
        );
        assert_eq!(
            distillation_model(Provider::OpenAI),
            OPENAI_DISTILLATION_MODEL
        );
    }

    #[test]
    fn test_build_prompt_preserves_message_order() {
        let messages = vec![
            (
                MessageId::new_for_test(5),
                Message::try_user("First message").expect("non-empty test message"),
            ),
            (
                MessageId::new_for_test(10),
                Message::try_user("Second message").expect("non-empty test message"),
            ),
            (
                MessageId::new_for_test(15),
                Message::try_user("Third message").expect("non-empty test message"),
            ),
        ];

        let (_, conversation) = build_distillation_prompt(&messages, 500);

        let first_pos = conversation.find("First message").unwrap();
        let second_pos = conversation.find("Second message").unwrap();
        let third_pos = conversation.find("Third message").unwrap();

        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[tokio::test]
    async fn test_generate_distillation_empty_messages() {
        use forge_types::ApiKey;

        let model = Provider::Claude
            .parse_model("claude-opus-4-6")
            .expect("parse model");
        let config = ApiConfig::new(ApiKey::Claude("fake-key".to_string()), model).expect("config");
        let counter = TokenCounter::new();

        let messages: Vec<(MessageId, Message)> = vec![];
        let result = generate_distillation(&config, &counter, &messages, 500).await;

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

        let messages = vec![(
            MessageId::new_for_test(0),
            Message::system(NonEmptyString::new("You are a helpful assistant.").expect("msg")),
        )];

        let (_, conversation) = build_distillation_prompt(&messages, 500);
        assert!(conversation.contains("System:"));
        assert!(conversation.contains("You are a helpful assistant."));
        assert!(conversation.contains("[Message 0]"));
    }

    #[test]
    fn test_build_prompt_with_assistant_message() {
        use forge_types::NonEmptyString;

        let messages = vec![(
            MessageId::new_for_test(0),
            Message::assistant(
                Provider::Claude.default_model(),
                NonEmptyString::new("Hello! How can I help?").expect("msg"),
            ),
        )];

        let (_, conversation) = build_distillation_prompt(&messages, 500);
        assert!(conversation.contains("Assistant:"));
        assert!(conversation.contains("Hello! How can I help?"));
    }

    #[test]
    fn test_build_prompt_with_tool_use() {
        use forge_types::ToolCall;
        use serde_json::json;

        let tool_call = ToolCall::new("call_123", "Read", json!({"path": "/tmp/test.txt"}));

        let messages = vec![(MessageId::new_for_test(0), Message::ToolUse(tool_call))];

        let (_, conversation) = build_distillation_prompt(&messages, 500);
        assert!(conversation.contains("Tool Call: Read"));
        assert!(conversation.contains("/tmp/test.txt"));
        assert!(conversation.contains("[Message 0]"));
    }

    #[test]
    fn test_build_prompt_with_tool_result_success() {
        use forge_types::ToolResult;

        let result = ToolResult {
            tool_call_id: "call_123".to_string(),
            tool_name: "Read".to_string(),
            content: "File contents here".to_string(),
            is_error: false,
        };

        let messages = vec![(MessageId::new_for_test(0), Message::ToolResult(result))];

        let (_, conversation) = build_distillation_prompt(&messages, 500);
        assert!(conversation.contains("Tool Result:"));
        assert!(conversation.contains("File contents here"));
    }

    #[test]
    fn test_build_prompt_with_tool_result_error() {
        use forge_types::ToolResult;

        let result = ToolResult {
            tool_call_id: "call_456".to_string(),
            tool_name: "Read".to_string(),
            content: "File not found".to_string(),
            is_error: true,
        };

        let messages = vec![(MessageId::new_for_test(0), Message::ToolResult(result))];

        let (_, conversation) = build_distillation_prompt(&messages, 500);
        assert!(conversation.contains("Tool Error:"));
        assert!(conversation.contains("File not found"));
    }

    // Note: Integration tests that actually call the API would go in tests/
    // and would be marked with #[ignore] to avoid running in CI without keys.
}

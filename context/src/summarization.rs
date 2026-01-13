//! Conversation summarization via LLM.
//!
//! This module provides functionality to generate summaries of conversation
//! segments using a cheaper/faster LLM model. Summaries preserve key facts,
//! decisions, and context while reducing token count.

use std::fmt::Write;

use anyhow::{Result, anyhow};
use serde_json::json;

use forge_providers::{ApiConfig, http_client_with_timeout};
use forge_types::{Message, Provider};

use super::MessageId;
use super::token_counter::TokenCounter;

/// Models used for summarization (cheaper/faster than main models).
const CLAUDE_SUMMARIZATION_MODEL: &str = "claude-haiku-4-5";
const OPENAI_SUMMARIZATION_MODEL: &str = "gpt-5-nano";

/// Context limits for summarizer models (conservative to leave room for output + overhead).
/// Claude Haiku 4.5 has 200k context, we use 190k to leave room for output and system prompt.
const CLAUDE_SUMMARIZER_INPUT_LIMIT: u32 = 190_000;
/// GPT-5-nano has 400k context, we use 380k to leave room for output and system prompt.
const OPENAI_SUMMARIZER_INPUT_LIMIT: u32 = 380_000;

const MIN_SUMMARY_TOKENS: u32 = 64;
const MAX_SUMMARY_TOKENS: u32 = 2048;
const SUMMARY_TIMEOUT_SECS: u64 = 60;

/// API endpoints.
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Using Responses API for consistency with main provider (providers/src/lib.rs).
const OPENAI_API_URL: &str = "https://api.openai.com/v1/responses";

/// Build a summarization prompt for a slice of messages.
///
/// The prompt instructs the LLM to:
/// - Preserve key facts, decisions, and important context
/// - Maintain chronological flow of the conversation
/// - Stay within the target token count
/// - Use clear, concise language
///
/// # Arguments
/// * `messages` - Slice of (`MessageId`, Message) tuples to summarize
/// * `target_tokens` - Target token count for the summary
///
/// # Returns
/// A tuple of (`system_instruction`, `conversation_text`) for the API call.
pub fn build_summarization_prompt(
    messages: &[(MessageId, Message)],
    target_tokens: u32,
) -> (String, String) {
    let system_instruction = format!(
        r#"You are a conversation summarizer. Your task is to create a concise summary of the following conversation.

REQUIREMENTS:
1. Preserve all key facts, decisions, and important context
2. Maintain the chronological flow of topics discussed
3. Keep the summary under approximately {target_tokens} tokens
4. Use clear, direct language
5. Preserve any code snippets, file paths, or technical details that are essential
6. Note any unresolved questions or pending actions
7. Format as a coherent narrative, not bullet points

OUTPUT FORMAT:
Write the summary as a continuous narrative that captures the essence of the conversation. Start directly with the content - do not include preamble like "This conversation..." or "Summary:"."#
    );

    let mut conversation_text = String::new();
    for (id, message) in messages {
        let role = match message {
            Message::System(_) => "System",
            Message::User(_) => "User",
            Message::Assistant(_) => "Assistant",
            Message::ToolUse(call) => {
                // Format tool calls with their name for context
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
                // Format tool results with their status
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

/// Get the summarizer model's input token limit for a provider.
pub fn summarizer_input_limit(provider: Provider) -> u32 {
    match provider {
        Provider::Claude => CLAUDE_SUMMARIZER_INPUT_LIMIT,
        Provider::OpenAI => OPENAI_SUMMARIZER_INPUT_LIMIT,
    }
}

/// Estimate the token count for a text string using the tokenizer.
fn count_tokens(counter: &TokenCounter, text: &str) -> u32 {
    counter.count_str(text)
}

/// Generate a summary of conversation messages using an LLM.
///
/// This function calls a cheaper/faster model to generate the summary,
/// using the API key from the provided config but overriding the model.
///
/// # Arguments
/// * `config` - API configuration (provides the API key and determines provider)
/// * `counter` - Token counter for accurate token estimation
/// * `messages` - Slice of (`MessageId`, Message) tuples to summarize
/// * `target_tokens` - Target token count for the summary
///
/// # Returns
/// The generated summary text.
///
/// # Errors
/// Returns an error if:
/// - The input messages exceed the summarizer model's context limit
/// - The API call fails
/// - The response cannot be parsed
pub async fn generate_summary(
    config: &ApiConfig,
    counter: &TokenCounter,
    messages: &[(MessageId, Message)],
    target_tokens: u32,
) -> Result<String> {
    if messages.is_empty() {
        return Ok(String::new());
    }

    let (system_instruction, conversation_text) =
        build_summarization_prompt(messages, target_tokens);

    // Validate that input doesn't exceed summarizer model's context limit
    let estimated_input =
        count_tokens(counter, &system_instruction) + count_tokens(counter, &conversation_text);
    let input_limit = summarizer_input_limit(config.provider());

    if estimated_input > input_limit {
        return Err(anyhow!(
            "Summarization scope too large: ~{} tokens exceeds {} model limit of {} tokens. \
             Consider summarizing fewer messages at a time.",
            estimated_input,
            summarization_model(config.provider()),
            input_limit
        ));
    }

    let max_tokens = target_tokens.clamp(MIN_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS);

    match config.provider() {
        Provider::Claude => {
            generate_summary_claude(
                config.api_key(),
                &system_instruction,
                &conversation_text,
                max_tokens,
            )
            .await
        }
        Provider::OpenAI => {
            generate_summary_openai(
                config.api_key(),
                &system_instruction,
                &conversation_text,
                max_tokens,
            )
            .await
        }
    }
}

/// Generate summary using Claude API (non-streaming).
async fn generate_summary_claude(
    api_key: &str,
    system_instruction: &str,
    conversation_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = http_client_with_timeout(SUMMARY_TIMEOUT_SECS);

    let body = json!({
        "model": CLAUDE_SUMMARIZATION_MODEL,
        "max_tokens": max_tokens,
        "stream": false,
        "system": system_instruction,
        "messages": [
            {
                "role": "user",
                "content": format!("Please summarize the following conversation:\n\n{}", conversation_text)
            }
        ]
    });

    let response = client
        .post(CLAUDE_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read error: {e}>"));
        return Err(anyhow!("Claude API error {status}: {error_text}"));
    }

    let json: serde_json::Value = response.json().await?;

    // Extract text from Claude's response format:
    // { "content": [{ "type": "text", "text": "..." }] }
    let summary = json["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract summary from Claude response: {json:?}"))?;

    Ok(summary.to_string())
}

/// Generate summary using `OpenAI` Responses API (non-streaming).
///
/// Uses the Responses API for consistency with the main provider module.
/// Request format: `{ model, instructions, input, max_output_tokens, stream: false }`
/// Response format: `{ output: [{ type: "message", content: [{ type: "output_text", text }] }] }`
async fn generate_summary_openai(
    api_key: &str,
    system_instruction: &str,
    conversation_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = http_client_with_timeout(SUMMARY_TIMEOUT_SECS);

    // Responses API uses `input` array and `instructions` for system prompt
    let body = json!({
        "model": OPENAI_SUMMARIZATION_MODEL,
        "stream": false,
        "max_output_tokens": max_tokens,
        "instructions": system_instruction,
        "input": [
            {
                "role": "user",
                "content": format!("Please summarize the following conversation:\n\n{}", conversation_text)
            }
        ]
    });

    let response = client
        .post(OPENAI_API_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read error: {e}>"));
        return Err(anyhow!("OpenAI API error {status}: {error_text}"));
    }

    let json: serde_json::Value = response.json().await?;

    // Extract text from OpenAI Responses API format:
    // { "output": [{ "type": "message", "content": [{ "type": "output_text", "text": "..." }] }] }
    let summary = json["output"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item["content"].as_array())
        .and_then(|content| content.first())
        .and_then(|block| block["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract summary from OpenAI response: {json:?}"))?;

    Ok(summary.to_string())
}

/// Get the summarization model name for a given provider.
#[must_use]
pub fn summarization_model(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => CLAUDE_SUMMARIZATION_MODEL,
        Provider::OpenAI => OPENAI_SUMMARIZATION_MODEL,
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
    fn test_build_summarization_prompt_basic() {
        let messages = make_test_messages();
        let (system, conversation) = build_summarization_prompt(&messages, 500);

        // Check system instruction contains key requirements
        assert!(system.contains("summarizer"));
        assert!(system.contains("500"));
        assert!(system.contains("Preserve"));
        assert!(system.contains("key facts"));

        // Check conversation text contains messages
        assert!(conversation.contains("User:"));
        assert!(conversation.contains("Hello, can you help me with Rust?"));
        assert!(conversation.contains("lifetimes"));
        assert!(conversation.contains("[Message 0]"));
        assert!(conversation.contains("[Message 1]"));
    }

    #[test]
    fn test_build_summarization_prompt_empty() {
        let messages: Vec<(MessageId, Message)> = vec![];
        let (system, conversation) = build_summarization_prompt(&messages, 100);

        // System instruction should still be generated
        assert!(system.contains("summarizer"));
        // Conversation should be empty
        assert!(conversation.is_empty());
    }

    #[test]
    fn test_build_summarization_prompt_target_tokens_in_instruction() {
        let messages = make_test_messages();

        let (system_500, _) = build_summarization_prompt(&messages, 500);
        let (system_1000, _) = build_summarization_prompt(&messages, 1000);

        assert!(system_500.contains("500"));
        assert!(system_1000.contains("1000"));
    }

    #[test]
    fn test_summarization_model_selection() {
        assert_eq!(
            summarization_model(Provider::Claude),
            CLAUDE_SUMMARIZATION_MODEL
        );
        assert_eq!(
            summarization_model(Provider::OpenAI),
            OPENAI_SUMMARIZATION_MODEL
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

        let (_, conversation) = build_summarization_prompt(&messages, 500);

        let first_pos = conversation.find("First message").unwrap();
        let second_pos = conversation.find("Second message").unwrap();
        let third_pos = conversation.find("Third message").unwrap();

        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[tokio::test]
    async fn test_generate_summary_empty_messages() {
        use forge_types::ApiKey;

        let model = Provider::Claude
            .parse_model("claude-sonnet-4-20250514")
            .expect("parse model");
        let config = ApiConfig::new(ApiKey::Claude("fake-key".to_string()), model).expect("config");
        let counter = TokenCounter::new();

        let messages: Vec<(MessageId, Message)> = vec![];
        let result = generate_summary(&config, &counter, &messages, 500).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // Note: Integration tests that actually call the API would go in tests/
    // and would be marked with #[ignore] to avoid running in CI without keys.
}

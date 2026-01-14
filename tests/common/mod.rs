//! Shared test utilities and fixtures
//!
//! Common infrastructure for integration tests.

#![allow(dead_code)]

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Start a mock server that simulates the Claude API
pub async fn start_claude_mock() -> MockServer {
    MockServer::start().await
}

/// Start a mock server that simulates the OpenAI API
pub async fn start_openai_mock() -> MockServer {
    MockServer::start().await
}

/// Mount a simple Responses API response (non-streaming)
pub async fn mount_chat_response(server: &MockServer, response_content: &str) {
    let body = serde_json::json!({
        "id": "resp_test",
        "object": "response",
        "created_at": 1234567890,
        "model": "gpt-4o",
        "output": [{
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": response_content
            }]
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 20,
            "total_tokens": 30
        }
    });

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mount a streaming SSE response for Responses API
pub async fn mount_streaming_response(server: &MockServer, chunks: &[&str]) {
    let mut sse_body = String::new();

    for chunk in chunks {
        let data = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": chunk
        });
        sse_body.push_str(&format!("data: {}\n\n", data));
    }

    let done_event = serde_json::json!({
        "type": "response.completed"
    });
    sse_body.push_str(&format!("data: {}\n\n", done_event));

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// Claude API response format
pub async fn mount_claude_response(server: &MockServer, response_content: &str) {
    let body = serde_json::json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "text",
            "text": response_content
        }],
        "model": "claude-sonnet-4-20250514",
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 20
        }
    });

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Macro to skip tests that require network access
#[macro_export]
macro_rules! skip_if_no_network {
    () => {
        if std::env::var("FORGE_TEST_NO_NETWORK").is_ok() {
            eprintln!("Skipping test: FORGE_TEST_NO_NETWORK is set");
            return;
        }
    };
}

/// Macro to skip tests on specific platforms
#[macro_export]
macro_rules! skip_on_windows {
    () => {
        #[cfg(target_os = "windows")]
        {
            eprintln!("Skipping test: not supported on Windows");
            return;
        }
    };
}

# forge-providers

This document provides comprehensive documentation for the `forge-providers` crate - the LLM API client layer for the Forge application. It is intended for developers who want to understand, maintain, or extend the provider functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
| :--- | :--- |
| 1-42 | Header & TOC |
| 43-82 | Overview |
| 83-160 | Architecture Diagram |
| 161-213 | Provider System |
| 214-341 | Type-Driven Design |
| 342-430 | SSE Streaming Infrastructure |
| 431-550 | Claude API Client |
| 551-705 | OpenAI API Client |
| 706-765 | Implementation: Error Handling |
| 766-825 | Gemini API Client |
| 826-874 | Public API Reference |
| 875-1008 | Code Examples |
| 1009-1050 | Error Handling |
| 1051-1100 | Dependencies & Safety |
| 1101-1355 | Extension Guide |
| 1356-1493 | Advanced Code Examples |
| 1494-1525 | Crate Design Summary |

## Table of Contents

1. [Overview](#overview)
2. [Architecture Diagram](#architecture-diagram)
3. [Provider System](#provider-system)
4. [Type-Driven Design](#type-driven-design)
5. [SSE Streaming Infrastructure](#sse-streaming-infrastructure)
6. [Claude API Client](#claude-api-client)
7. [OpenAI API Client](#openai-api-client)
8. [Gemini API Client](#gemini-api-client)
9. [Public API Reference](#public-api-reference)
10. [Code Examples](#code-examples)
11. [Error Handling](#error-handling)
12. [Dependencies & Safety](#dependencies--safety)
13. [Extension Guide](#extension-guide)
14. [Advanced Code Examples](#advanced-code-examples)
15. [Crate Design Summary](#crate-design-summary)

---

## Overview

The `forge-providers` crate is responsible for all HTTP communication with LLM APIs. It provides a unified streaming interface that abstracts away the differences between provider-specific APIs while preserving provider-specific features like Claude's extended thinking and OpenAI's reasoning controls.

### Key Responsibilities

| Responsibility | Description |
| :--- | :--- |
| **HTTP Communication** | Send requests to Claude, OpenAI, and Gemini APIs |
| **SSE Parsing** | Parse Server-Sent Events streams from all three providers |
| **Request Building** | Construct provider-specific request payloads |
| **Event Normalization** | Convert provider-specific events to unified `StreamEvent` |
| **Context Caching** | Manage Gemini explicit context caches for large prompts |
| **Configuration Validation** | Ensure API keys match their intended providers |

### Crate Structure

```text
providers/
├── Cargo.toml          # Crate manifest
└── src/
    └── lib.rs          # All provider implementations
        ├── SSE parsing functions
        ├── ApiConfig struct
        ├── send_message() dispatch
        ├── claude module
        ├── openai module
        └── gemini module
```

### Dependencies

| Crate | Purpose |
| :--- | :--- |
| `forge-types` | Core domain types (Provider, ModelName, Message, etc.) |
| `reqwest` | HTTP client with streaming support |
| `futures-util` | Async stream utilities for SSE processing |
| `serde` / `serde_json` | JSON serialization for API payloads |
| `anyhow` / `thiserror` | Error handling |
| `tracing` | Logging infrastructure |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Engine Layer (caller)                              │
│                                                                              │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                         send_message()                               │   │
│   │                                                                      │   │
│   │   ApiConfig ────────────────────────────────────────────────────►   │   │
│   │   CacheableMessage[] ───────────────────────────────────────────►   │   │
│   │   OutputLimits ─────────────────────────────────────────────────►   │   │
│   │   system_prompt ────────────────────────────────────────────────►   │   │
│   │   on_event callback ────────────────────────────────────────────►   │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      │ match config.provider()
                                      ▼
             ┌────────────────────────┴────────────────────────┬────────────────────────┐
             │                        │                        │                        │
             ▼                        ▼                        ▼                        ▼
┌─────────────────────────┐  ┌─────────────────────────┐  ┌─────────────────────────┐  ┌─────────────────────────┐
│  claude::send_message   │  │  openai::send_message   │  │  gemini::send_message   │  │      YourProvider       │
│                         │  │                         │  │                         │  │        (future)         │
│ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │  └─────────────────────────┘
│ │build_request_body() │ │  │ │build_request_body() │ │  │ │build_request_body() │ │
│ │- System blocks      │ │  │ │- input items        │ │  │ │- system_instruction │ │
│ │- Messages array     │ │  │ │- instructions       │ │  │ │- cachedContent ref │ │
│ │- Cache control      │ │  │ │- reasoning.effort   │ │  └─────────────────────┘ │
│ └─────────────────────┘ │  └─────────────────────┘ │             │             │
│            │            │            │            │             ▼             │
│            ▼            │            ▼            │  ┌─────────────────────┐  │
│ ┌─────────────────────┐ │  ┌─────────────────────┐ │  │    POST to API      │  │
│ │   POST to API       │ │  │   POST to API       │ │  │ googleapis.com      │  │
│ │ api.anthropic.com   │ │  │ api.openai.com      │ │  │ /v1beta/models      │  │
│ │ /v1/messages        │ │  │ /v1/responses       │ │  └─────────────────────┘  │
│ └─────────────────────┘ │  └─────────────────────┘ │             │             │
│            │            │            │            │             ▼             │
│            ▼            │            ▼            │  ┌─────────────────────┐  │
│ ┌─────────────────────┐ │  ┌─────────────────────┐ │  │   SSE Stream Loop   │  │
│ │  SSE Stream Loop    │ │  │  SSE Stream Loop    │ │  │ - drain_next_event  │  │
│ │ - drain_next_event  │ │  │ - drain_next_event  │ │  │ - Gemini deltas    │  │
│ │ - Claude deltas     │ │  │ - OpenAI response   │ │  │ - Emit StreamEvent  │  │
│ │ - Emit StreamEvent  │ │  │ - Emit StreamEvent  │ │  └─────────────────────┘  │
│ └─────────────────────┘ │  └─────────────────────┘ │                           │
└─────────────────────────┘  └─────────────────────────┘                           ┘
             │                        │                        │
             │                        │                        │
             └────────────────────────┴──────────┬─────────────┘
                                                 │
                                                 ▼
                                    ┌─────────────────────────┐
                                    │     on_event(event)     │
                                    │                         │
                                    │  StreamEvent::TextDelta │
                                    │  StreamEvent::ThinkingDelta │
                                    │  StreamEvent::Done      │
                                    │  StreamEvent::Error     │
                                    └─────────────────────────┘
```

---

## Provider System

### Provider Enum (from forge-types)

The `Provider` enum represents supported LLM providers:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Provider {
    #[default]
    Claude,
    OpenAI,
    Gemini,
}
```

Key methods:

| Method | Return Type | Description |
| :--- | :--- | :--- |
| `as_str()` | `&'static str` | Provider identifier ("claude", "openai", "gemini") |
| `display_name()` | `&'static str` | UI display name ("Claude", "GPT", "Gemini") |
| `env_var()` | `&'static str` | Environment variable for API key |
| `default_model()` | `ModelName` | Default model for provider |
| `available_models()` | `&'static [&'static str]` | Known models for provider |
| `parse_model(raw)` | `Result<ModelName, ModelParseError>` | Validate and parse model name |
| `parse(s)` | `Option<Provider>` | Parse provider from string |
| `all()` | `&'static [Provider]` | All available providers |

### Provider Dispatch Pattern

The `send_message` function dispatches to provider-specific implementations based on the `ApiConfig`:

```rust
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    gemini_cache: Option<&gemini::GeminiCache>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()> {
    match config.provider() {
        Provider::Claude => {
            claude::send_message(config, messages, limits, system_prompt, tools, on_event).await
        }
        Provider::OpenAI => {
            openai::send_message(config, messages, limits, system_prompt, tools, on_event).await
        }
        Provider::Gemini => {
            gemini::send_message(config, messages, limits, system_prompt, tools, gemini_cache, on_event).await
        }
    }
}
```

This pattern provides:

- **Unified interface**: Callers use the same function regardless of provider
- **Provider isolation**: Each provider module handles its own API specifics
- **Exhaustive matching**: Adding a new provider requires handling all dispatch points

---

## Type-Driven Design

### ApiConfig - Configuration Container

`ApiConfig` holds validated API configuration that ensures provider/key consistency:

```rust
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
}
```

**Construction validation:**

```rust
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
        Ok(Self { api_key, model, openai_options: Default::default() })
    }
}
```

This design makes it impossible to create an `ApiConfig` with a Claude API key and an OpenAI model.

### ApiKey - Provider-Scoped Keys

```rust
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    Gemini(String),
}

impl ApiKey {
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
            ApiKey::Gemini(_) => Provider::Gemini,
        }
    }
}
```

The `ApiKey` enum prevents the invalid state of using an API key with the wrong provider. The key's provider is encoded in its type, not as a separate field that could become inconsistent.

### ModelName - Provider-Scoped Models

```rust
pub struct ModelName {
    provider: Provider,
    name: Cow<'static, str>,
    kind: ModelNameKind,
}

pub enum ModelNameKind {
    Known,      // Verified model name from available_models()
    Unverified, // User-supplied, potentially unknown model
}
```

**Validation rules by provider:**

| Provider | Prefix Requirement | Example Valid | Example Invalid |
| :--- | :--- | :--- | :--- |
| Claude | Must start with `claude-` | `claude-sonnet-4-5-20250929` | `gpt-5.2` |
| OpenAI | Must start with `gpt-5` | `gpt-5.2`, `gpt-5.2-2025-12-11` | `gpt-4o` |
| Gemini | Must start with `gemini-` | `gemini-3-pro`, `gemini-3-flash` | `claude-3` |

The OpenAI prefix requirement (`gpt-5`) ensures compatibility with the OpenAI Responses API.

### OutputLimits - Validated Token Budgets

```rust
pub struct OutputLimits {
    max_output_tokens: u32,
    thinking_budget: Option<u32>,
}
```

Construction enforces invariants:

- If thinking is enabled, `thinking_budget >= 1024`
- If thinking is enabled, `thinking_budget < max_output_tokens`

```rust
impl OutputLimits {
    pub fn with_thinking(max_output_tokens: u32, thinking_budget: u32) 
        -> Result<Self, OutputLimitsError> 
    {
        if thinking_budget < 1024 {
            return Err(OutputLimitsError::ThinkingBudgetTooSmall);
        }
        if thinking_budget >= max_output_tokens {
            return Err(OutputLimitsError::ThinkingBudgetTooLarge { ... });
        }
        Ok(Self { max_output_tokens, thinking_budget: Some(thinking_budget) })
    }
}
```

### CacheHint - Provider Caching Hints

```rust
pub enum CacheHint {
    None,      // No caching preference
    Ephemeral, // Request caching (Claude-specific)
}
```

Different providers handle caching differently:

- **Claude**: Explicit `cache_control: { type: "ephemeral" }` markers on content blocks
- **OpenAI**: Automatic server-side prefix caching (hints are ignored)
- **Gemini**: Explicit context caching via `cachedContents` API for system prompts > 1024/4096 tokens

---

## SSE Streaming Infrastructure

### Architecture Overview

SSE (Server-Sent Events) streaming is abstracted via a trait-based design that separates:

1. **Shared infrastructure**: Buffer management, timeout handling, UTF-8 validation, `[DONE]` detection
2. **Provider-specific parsing**: JSON event interpretation via `SseParser` trait

```
┌─────────────────────────────────────────────────────────────────┐
│                    process_sse_stream()                          │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Shared: timeout, buffer, UTF-8, [DONE], parse errors     │  │
│  └───────────────────────────────────────────────────────────┘  │
│                            │                                     │
│                            ▼                                     │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │              parser.parse(&json) -> SseParseAction         │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │  │
│  │  │ClaudeParser │  │OpenAIParser │  │GeminiParser │        │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘        │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### SseParser Trait

Each provider implements this trait for JSON event parsing:

```rust
/// Action returned by provider-specific SSE JSON parser.
enum SseParseAction {
    Continue,                  // No event to emit
    Emit(Vec<StreamEvent>),    // Emit these events
    Done,                      // Stream completed
    Error(String),             // Fatal error
}

/// Provider-specific SSE JSON parsing.
trait SseParser {
    fn parse(&mut self, json: &serde_json::Value) -> SseParseAction;
    fn provider_name(&self) -> &'static str;
}
```

### Shared Stream Processor

The `process_sse_stream` function handles all common SSE logic:

```rust
async fn process_sse_stream<P: SseParser>(
    response: reqwest::Response,
    parser: &mut P,
    on_event: impl Fn(StreamEvent),
) -> Result<()>
```

This function manages:

- Stream timeout handling (idle timeout triggers error)
- Buffer management with 4 MiB size limit
- UTF-8 validation
- Event boundary detection (`\n\n` and `\r\n\r\n`)
- `[DONE]` marker handling
- Parse error tracking with threshold (3 consecutive errors = fatal)

### Low-Level SSE Parsing Functions

```rust
/// Find the boundary of the next SSE event in the buffer.
fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)>

/// Drain the next complete SSE event from the buffer.
fn drain_next_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>>

/// Extract the data payload from an SSE event.
fn extract_sse_data(event: &str) -> Option<String>
```

### SSE Event Format

SSE events follow this format:

```
event: message_start
data: {"type":"message_start",...}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"text":"Hello"}}

data: [DONE]
```

The parsing handles:

- Both `\n\n` and `\r\n\r\n` delimiters
- Optional `event:` lines (ignored)
- Multi-line `data:` fields
- The `[DONE]` sentinel

### StreamEvent - Unified Event Type

All providers emit events through a unified `StreamEvent` enum:

```rust
pub enum StreamEvent {
    TextDelta(String),                           // Text content chunk
    ThinkingDelta(String),                       // Claude extended thinking chunk
    ToolCallStart { id: String, name: String },  // Tool call started
    ToolCallDelta { id: String, arguments: String }, // Tool call arguments chunk
    Done,                                        // Stream completed successfully
    Error(String),                               // Stream failed with error
}
```

---

## Claude API Client

### Claude API Client Endpoint

```rust
const API_URL: &str = "https://api.anthropic.com/v1/messages";
```

### Claude API Client Authentication

Claude uses an `x-api-key` header:

```rust
client.post(API_URL)
    .header("x-api-key", config.api_key())
    .header("anthropic-version", "2023-06-01")
    .header("content-type", "application/json")
```

### Claude API Client Request Building

The Claude client transforms `CacheableMessage` arrays into the Anthropic Messages API format:

```rust
fn build_request_body(
    model: &str,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
) -> serde_json::Value
```

**Request structure:**

```json
{
  "model": "claude-sonnet-4-5-20250929",
  "max_tokens": 4096,
  "stream": true,
  "system": [
    { "type": "text", "text": "System prompt", "cache_control": { "type": "ephemeral" } }
  ],
  "messages": [
    { "role": "user", "content": [{ "type": "text", "text": "Hello" }] },
    { "role": "assistant", "content": "Hi there!" }
  ],
  "thinking": { "type": "enabled", "budget_tokens": 4096 }
}
```

**Key transformations:**

| Input | API Format |
|-------|------------|
| System prompt (parameter) | First system block with `cache_control: ephemeral` |
| `Message::System` | Additional system blocks (hoisted from messages) |
| `Message::User` | User message with content blocks |
| `Message::Assistant` | Assistant message as plain string |
| `CacheHint::Ephemeral` | `cache_control: { type: "ephemeral" }` |
| `OutputLimits::thinking_budget` | `thinking: { type: "enabled", budget_tokens: N }` |

**Content blocks:**

```rust
fn content_block(text: &str, cache_hint: CacheHint) -> serde_json::Value {
    match cache_hint {
        CacheHint::None => json!({ "type": "text", "text": text }),
        CacheHint::Ephemeral => json!({
            "type": "text",
            "text": text,
            "cache_control": { "type": "ephemeral" }
        }),
    }
}
```

### Claude API Client Response Parsing

Claude SSE events follow this structure:

| Event Type | Action |
|------------|--------|
| `content_block_delta` with `text_delta` | Emit `StreamEvent::TextDelta` |
| `content_block_delta` with `thinking_delta` | Emit `StreamEvent::ThinkingDelta` |
| `message_stop` | Emit `StreamEvent::Done` |
| `data: [DONE]` | Emit `StreamEvent::Done` |

```rust
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
                    on_event(StreamEvent::ThinkingDelta(thinking.to_string()));
                }
            }
            "input_json_delta" => {
                // Tool arguments streaming
                if let Some(json_chunk) = json["delta"]["partial_json"].as_str()
                    && let Some(ref id) = current_tool_id
                {
                    on_event(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        arguments: json_chunk.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
}
```

---

## OpenAI API Client

### OpenAI API Client Endpoint

```rust
const API_URL: &str = "https://api.openai.com/v1/responses";
```

Note: This uses the OpenAI Responses API (not the Chat Completions API) for GPT-5.2 support.

### OpenAI API Client Authentication

OpenAI uses Bearer token authentication:

```rust
client.post(API_URL)
    .header("Authorization", format!("Bearer {}", config.api_key()))
    .header("content-type", "application/json")
```

### OpenAI API Client Request Building

```rust
fn build_request_body(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
) -> serde_json::Value
```

**Request structure:**

```json
{
  "model": "gpt-5.2",
  "input": [
    { "role": "developer", "content": "Context summary" },
    { "role": "user", "content": "Hello" },
    { "role": "assistant", "content": "Hi there!" }
  ],
  "instructions": "System prompt",
  "max_output_tokens": 4096,
  "stream": true,
  "truncation": "auto",
  "reasoning": { "effort": "high" },
  "text": { "verbosity": "high" }
}
```

**Role mapping:**

Per the OpenAI Model Spec authority hierarchy:

| Message Type | OpenAI Role | Rationale |
| :--- | :--- | :--- |
| `Message::System` | `"developer"` | "System" is reserved for OpenAI runtime injections |
| `Message::User` | `"user"` | Standard user role |
| `Message::Assistant` | `"assistant"` | Standard assistant role |

```rust
fn openai_role(msg: &Message) -> &'static str {
    match msg {
        Message::System(_) => "developer",
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
    }
}
```

**GPT-5.2 specific options:**

For models starting with `gpt-5`, additional parameters are included:

```rust
if model.starts_with("gpt-5") {
    body.insert("reasoning", json!({ "effort": options.reasoning_effort().as_str() }));
    body.insert("text", json!({ "verbosity": options.verbosity().as_str() }));
}
```

### OpenAI Request Options

```rust
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,
    verbosity: OpenAITextVerbosity,
    truncation: OpenAITruncation,
}
```

| Option | Values | Default | Description |
| :--- | :--- | :--- | :--- |
| `reasoning_effort` | none, low, medium, high, xhigh | high | Control reasoning depth |
| `verbosity` | low, medium, high | high | Control output length/structure |
| `truncation` | auto, disabled | auto | Context truncation strategy |

### OpenAI API Client Response Parsing

OpenAI Responses API uses different event types:

| Event Type | Action |
|------------|--------|
| `response.output_text.delta` | Emit `StreamEvent::TextDelta` |
| `response.refusal.delta` | Emit `StreamEvent::TextDelta` (model refused) |
| `response.output_text.done` | Emit remaining text if no deltas received |
| `response.completed` | Emit `StreamEvent::Done` |
| `response.incomplete` | Emit `StreamEvent::Error` with reason |
| `response.failed` | Emit `StreamEvent::Error` with message |
| `error` | Emit `StreamEvent::Error` with message |

```rust
match json["type"].as_str().unwrap_or("") {
    "response.output_text.delta" => {
        if let Some(delta) = json["delta"].as_str() {
            saw_delta = true;
            on_event(StreamEvent::TextDelta(delta.to_string()));
        }
    }
    "response.completed" => {
        on_event(StreamEvent::Done);
        return Ok(());
    }
    "response.incomplete" => {
        let reason = extract_incomplete_reason(&json)
            .unwrap_or_else(|| "Response incomplete".to_string());
        on_event(StreamEvent::Error(reason));
        return Ok(());
    }
    // ...
}
```

**Error extraction:**

```rust
fn extract_error_message(payload: &Value) -> Option<String> {
    // Try error.message first
    payload.get("error")
        .and_then(|error| error.get("message"))
        .and_then(|value| value.as_str())
    // Fall back to response.error.message
        .or_else(|| {
            payload.get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(|value| value.as_str())
        })
        .map(|s| s.to_string())
}
```

---

## Implementation: Error Handling

### ApiConfigError

Configuration validation errors:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}
```

### HTTP Error Handling

Non-2xx responses are converted to `StreamEvent::Error`:

```rust
if !response.status().is_success() {
    let status = response.status();
    let error_text = response
        .text()
        .await
        .unwrap_or_else(|e| format!("<failed to read error body: {e}>"));
    on_event(StreamEvent::Error(format!("API error {}: {}", status, error_text)));
    return Ok(());
}
```

### Stream Error Recovery

UTF-8 decoding errors terminate the stream gracefully:

```rust
let event = match std::str::from_utf8(&event) {
    Ok(event) => event,
    Err(_) => {
        on_event(StreamEvent::Error(
            "Received invalid UTF-8 from SSE stream".to_string(),
        ));
        return Ok(());
    }
};
```

### Error Propagation Pattern

The providers use a callback-based error pattern rather than Result types for streaming errors:

```rust
// Errors during streaming are delivered as events
on_event(StreamEvent::Error(error_message));
return Ok(()); // Function succeeds, error is communicated via callback

// Only return Err for unrecoverable failures (network errors, etc.)
let chunk = chunk?; // This propagates network errors
```

---

## Gemini API Client

### Gemini API Client Endpoint

```rust
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
```

The Gemini client uses the `streamGenerateContent` endpoint with the `alt=sse` parameter.

### Gemini API Client Authentication

Gemini uses the `x-goog-api-key` header:

```rust
client.post(&url)
    .header("x-goog-api-key", config.api_key())
    .header("content-type", "application/json")
```

### Gemini API Client Request Building

The Gemini client transformations:

| Input | Gemini API Format |
| :--- | :--- |
| `system_prompt` | `system_instruction` with content parts |
| `Message::System` | User role message (hoisted if possible) |
| `Message::User` | User role message with parts |
| `Message::Assistant` | Model role message with parts |
| `GeminiCache` | `cachedContent` reference (disables `system_instruction`) |

### Context Caching

Gemini provides explicit context caching for large prompts. This is useful for long-running conversations with large system instructions or reference materials.

1. **Create Cache**: Use `gemini::create_cache` to upload the system prompt and get a cache name.
2. **Reference Cache**: Pass the `GeminiCache` reference in `send_message`.

```rust
pub struct GeminiCache {
    pub name: String,
    pub expire_time: DateTime<Utc>,
    pub system_prompt_hash: u64,
}
```

### Gemini API Client Response Parsing

Gemini SSE events follow the `streamGenerateContent` response format:

| Condition | Action |
| :--- | :--- |
| `candidates[].content.parts[].text` | Emit `StreamEvent::TextDelta` |
| `candidates[].content.parts[].thought` | Emit `StreamEvent::ThinkingDelta` |
| `finishReason` is `STOP` or `MAX_TOKENS` | Emit `StreamEvent::Done` |
| `error` field present | Emit `StreamEvent::Error` |

---

## Public API Reference

### Primary Function

```rust
/// Send a chat request and stream the response.
///
/// # Arguments
/// * `config` - API configuration (key, model, options)
/// * `messages` - Conversation history with cache hints
/// * `limits` - Output token limits (with optional thinking budget)
/// * `system_prompt` - Optional system prompt to inject
/// * `tools` - Optional tool definitions for function calling
/// * `on_event` - Callback for streaming events
///
/// # Returns
/// `Ok(())` on completion (success or error delivered via callback)
/// `Err(...)` only for unrecoverable failures (network errors)
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    gemini_cache: Option<&gemini::GeminiCache>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()>
```

### ApiConfig Methods

| Method | Return Type | Description |
| :--- | :--- | :--- |
| `new(api_key, model)` | `Result<Self, ApiConfigError>` | Create config with validation |
| `with_openai_options(options)` | `Self` | Builder for OpenAI-specific options |
| `provider()` | `Provider` | Get the provider |
| `api_key()` | `&str` | Get the API key string |
| `api_key_owned()` | `ApiKey` | Clone the API key |
| `model()` | `&ModelName` | Get the model name |
| `openai_options()` | `OpenAIRequestOptions` | Get OpenAI options |

### Re-exports

The crate re-exports `forge_types` for caller convenience:

```rust
pub use forge_types;
```

---

## Code Examples

### Basic Streaming Request (Claude)

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};

async fn chat_with_claude() -> anyhow::Result<()> {
    // Create provider-scoped types
    let api_key = ApiKey::Claude(std::env::var("ANTHROPIC_API_KEY")?);
    let model = Provider::Claude.default_model();
    
    // ApiConfig validates provider consistency
    let config = ApiConfig::new(api_key, model)?;
    
    // Build conversation
    let user_msg = Message::try_user("What is the capital of France?")?;
    let messages = vec![CacheableMessage::plain(user_msg)];
    
    // Output limits (no thinking)
    let limits = OutputLimits::new(4096);
    
    // Stream response
    send_message(
        &config,
        &messages,
        limits,
        Some("You are a helpful assistant."),
        None, // No tools
        None, // No gemini cache
        |event| {
            match event {
                StreamEvent::TextDelta(text) => print!("{}", text),
                StreamEvent::Done => println!("\n[Done]"),
                StreamEvent::Error(e) => eprintln!("Error: {}", e),
                _ => {}
            }
        },
    ).await?;
    
    Ok(())
}
```

### OpenAI Reasoning Example

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};

async fn chat_with_gpt5() -> anyhow::Result<()> {
    let api_key = ApiKey::OpenAI(std::env::var("OPENAI_API_KEY")?);
    let model = Provider::OpenAI.parse_model("gpt-5.2")?;
    
    // Configure OpenAI-specific options
    let options = OpenAIRequestOptions::new(
        OpenAIReasoningEffort::High,
        OpenAITextVerbosity::Medium,
        OpenAITruncation::Auto,
    );
    
    let config = ApiConfig::new(api_key, model)?
        .with_openai_options(options);
    
    let messages = vec![
        CacheableMessage::plain(Message::try_user("Explain quantum entanglement.")?)
    ];
    
    send_message(
        &config,
        &messages,
        OutputLimits::new(8192),
        None,
        None,
        None,
        |event| { /* handle events */ },
    ).await
}
```

### Claude Thinking Example

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};

async fn thinking_mode() -> anyhow::Result<()> {
    let config = ApiConfig::new(
        ApiKey::Claude("...".into()),
        Provider::Claude.parse_model("claude-opus-4-5-20251101")?,
    )?;
    
    // Enable thinking with 4096 token budget
    // Total output: 16384, of which up to 4096 can be thinking
    let limits = OutputLimits::with_thinking(16384, 4096)?;
    
    send_message(
        &config,
        &[CacheableMessage::plain(Message::try_user("Solve this step by step...")?)],
        limits,
        None,
        None,
        None,
        |event| {
            match event {
                StreamEvent::ThinkingDelta(thought) => {
                    // Internal reasoning (typically hidden from user)
                    eprint!("{}", thought);
                }
                StreamEvent::TextDelta(text) => {
                    // Final response
                    print!("{}", text);
                }
                _ => {}
            }
        },
    ).await
}
```

### Context Caching Example

```rust
use forge_providers::forge_types::*;

fn prepare_messages(history: Vec<Message>) -> Vec<CacheableMessage> {
    let len = history.len();
    history
        .into_iter()
        .enumerate()
        .map(|(i, msg)| {
            // Cache all but the last message (which is new)
            if i < len - 1 {
                CacheableMessage::cached(msg)
            } else {
                CacheableMessage::plain(msg)
            }
        })
        .collect()
}
```

---

## Error Handling

The crate uses `anyhow::Result` for most operations, with specific error types where meaningful:

| Error Type | Cause |
| :--- | :--- |
| `ApiConfigError::ProviderMismatch` | API key and model belong to different providers |
| HTTP errors (via `reqwest`) | Network failures, timeouts |
| `StreamEvent::Error` | API errors (rate limits, invalid requests, etc.) |

API errors are delivered through the `StreamEvent::Error` variant rather than as `Result::Err`, allowing partial responses to be captured before an error occurs.

---

## Dependencies & Safety

### Dependencies

| Crate | Purpose |
| :--- | :--- |
| `forge-types` | Domain types (Provider, Message, StreamEvent, etc.) |
| `reqwest` | HTTP client with streaming support |
| `futures-util` | `StreamExt` for async byte stream iteration |
| `serde` / `serde_json` | JSON serialization for API payloads |
| `anyhow` | Error handling |
| `thiserror` | Custom error type derivation |
| `tracing` | Structured logging (warnings for unexpected messages) |

### Thread Safety

- `ApiConfig` is `Clone + Send + Sync`
- The `on_event` callback must be `Send + 'static` for cross-task delivery
- A shared static HTTP client (`http_client()`) is reused across all requests for connection pooling efficiency

### Testing

```bash
cargo test -p forge-providers
```

Tests verify:

- `ApiConfig` rejects mismatched provider/key combinations
- `ApiConfig` accepts matching provider/key combinations

---

## Extension Guide

### Adding a New Provider

Adding a new LLM provider requires changes in multiple locations:

**Step 1: Extend Provider enum (in `forge-types/src/lib.rs`)**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Provider {
    #[default]
    Claude,
    OpenAI,
    YourProvider,  // New provider
}

impl Provider {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAI => "openai",
            Provider::Gemini => "gemini",
            Provider::YourProvider => "your_provider",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "GPT",
            Provider::Gemini => "Gemini",
            Provider::YourProvider => "YourProvider",
        }
    }

    pub fn env_var(&self) -> &'static str {
        match self {
            // ...existing...
            Provider::YourProvider => "YOUR_PROVIDER_API_KEY",
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            // ...existing...
            Provider::YourProvider => ModelName::known(Self::YourProvider, "your-model-v1"),
        }
    }

    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            // ...existing...
            Provider::YourProvider => &["your-model-v1", "your-model-v2"],
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Provider::Claude),
            "openai" | "gpt" | "chatgpt" => Some(Provider::OpenAI),
            "gemini" | "google" => Some(Provider::Gemini),
            "yourprovider" | "yp" => Some(Provider::YourProvider),
            _ => None,
        }
    }
}
```

**Step 2: Extend ApiKey enum (in `forge-types/src/lib.rs`)**

```rust
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    YourProvider(String),
}

impl ApiKey {
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
            ApiKey::YourProvider(_) => Provider::YourProvider,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ApiKey::Claude(key) | ApiKey::OpenAI(key) | ApiKey::Gemini(key) | ApiKey::YourProvider(key) => key,
        }
    }
}
```

**Step 3: Add ModelParseError variant (if needed)**

```rust
#[derive(Debug, Error)]
pub enum ModelParseError {
    #[error("model name cannot be empty")]
    Empty,
    #[error("Claude model must start with claude- (got {0})")]
    ClaudePrefix(String),
    #[error("OpenAI model must start with gpt-5 (got {0})")]
    OpenAIMinimum(String),
    #[error("YourProvider model must start with your- (got {0})")]
    YourProviderPrefix(String),
}
```

**Step 4: Add provider module (in `providers/src/lib.rs`)**

```rust
/// YourProvider API implementation.
pub mod your_provider {
    use super::{
        ApiConfig, CacheableMessage, Message, OutputLimits, Result,
        SseParseAction, SseParser, StreamEvent, ToolDefinition,
        http_client, process_sse_stream, read_capped_error_body,
    };
    use serde_json::json;

    const API_URL: &str = "https://api.yourprovider.com/v1/chat";

    fn build_request_body(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
    ) -> serde_json::Value {
        // Convert messages to provider-specific format
        let mut api_messages = Vec::new();

        // Add system prompt if provided
        if let Some(prompt) = system_prompt {
            api_messages.push(json!({
                "role": "system",
                "content": prompt
            }));
        }

        // Add conversation messages
        for cacheable in messages {
            let msg = &cacheable.message;
            api_messages.push(json!({
                "role": msg.role_str(),
                "content": msg.content()
            }));
        }

        json!({
            "model": config.model().as_str(),
            "messages": api_messages,
            "max_tokens": limits.max_output_tokens(),
            "stream": true
        })
    }

    // ========================================================================
    // YourProvider SSE Parser
    // ========================================================================

    /// Parser state for YourProvider SSE streams.
    #[derive(Default)]
    struct YourProviderParser;

    impl SseParser for YourProviderParser {
        fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
            // Check for completion
            if json["choices"][0]["finish_reason"].as_str() == Some("stop") {
                return SseParseAction::Done;
            }

            // Extract text delta
            if let Some(text) = json["choices"][0]["delta"]["content"].as_str() {
                return SseParseAction::Emit(vec![StreamEvent::TextDelta(text.to_string())]);
            }

            SseParseAction::Continue
        }

        fn provider_name(&self) -> &'static str {
            "YourProvider"
        }
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
            on_event(StreamEvent::Error(format!("API error {}: {}", status, error_text)));
            return Ok(());
        }

        // Use shared SSE stream processor with provider-specific parser
        let mut parser = YourProviderParser;
        process_sse_stream(response, &mut parser, on_event).await
    }
}
```

**Step 5: Update send_message dispatch**

```rust
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
        Provider::YourProvider => {
            your_provider::send_message(config, messages, limits, system_prompt, on_event).await
        }
    }
}
```

**Step 6: Update engine configuration (in `forge-engine/src/config.rs`)**

Add API key loading for the new provider in the config structure and `App::new()`.

### Adding Provider-Specific Features

To add provider-specific request options (like OpenAI's reasoning controls):

**Step 1: Define options struct (in `forge-types/src/lib.rs`)**

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct YourProviderOptions {
    pub feature_a: bool,
    pub feature_b: YourProviderFeatureB,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum YourProviderFeatureB {
    #[default]
    Low,
    Medium,
    High,
}
```

**Step 2: Add to ApiConfig**

```rust
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
    your_provider_options: YourProviderOptions,  // New
}

impl ApiConfig {
    pub fn with_your_provider_options(mut self, options: YourProviderOptions) -> Self {
        self.your_provider_options = options;
        self
    }
}
```

**Step 3: Use in request building**

```rust
fn build_request_body(config: &ApiConfig, ...) -> serde_json::Value {
    let mut body = json!({ ... });
    
    let options = config.your_provider_options();
    if options.feature_a {
        body["feature_a"] = json!(true);
    }
    body["feature_b"] = json!(options.feature_b.as_str());
    
    body
}
```

---

## Advanced Code Examples

### Basic Streaming Request

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};

async fn chat() -> anyhow::Result<()> {
    let api_key = ApiKey::Claude(std::env::var("ANTHROPIC_API_KEY")?);
    let model = Provider::Claude.default_model();
    let config = ApiConfig::new(api_key, model)?;
    
    let messages = vec![
        CacheableMessage::plain(Message::try_user("Hello!")?),
    ];
    
    send_message(
        &config,
        &messages,
        OutputLimits::new(4096),
        Some("You are a helpful assistant."),
        None, // No tools
        None, // No gemini cache
        |event| match event {
            StreamEvent::TextDelta(text) => print!("{}", text),
            StreamEvent::Done => println!("\n[Complete]"),
            StreamEvent::Error(e) => eprintln!("Error: {}", e),
            _ => {}
        },
    ).await
}
```

### Extended Thinking Mode (Claude)

```rust
let limits = OutputLimits::with_thinking(16384, 4096)?;

send_message(
    &config,
    &messages,
    limits,
    None,
    None,
    None,
    |event| match event {
        StreamEvent::ThinkingDelta(thought) => {
            // Internal reasoning (typically hidden)
            eprint!("[thinking] {}", thought);
        }
        StreamEvent::TextDelta(text) => print!("{}", text),
        _ => {}
    },
).await?;
```

### OpenAI with Reasoning Options

```rust
let api_key = ApiKey::OpenAI(std::env::var("OPENAI_API_KEY")?);
let model = Provider::OpenAI.parse_model("gpt-5.2")?;

let options = OpenAIRequestOptions::new(
    OpenAIReasoningEffort::High,
    OpenAITextVerbosity::Medium,
    OpenAITruncation::Auto,
);

let config = ApiConfig::new(api_key, model)?
    .with_openai_options(options);
```

### Tool Calling Example

```rust
use serde_json::json;

let read_file_tool = ToolDefinition::new(
    "read_file",
    "Read the contents of a file",
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "The file path to read"
            }
        },
        "required": ["path"]
    }),
);

let mut tool_call_id = String::new();
let mut tool_arguments = String::new();

send_message(
    &config,
    &messages,
    OutputLimits::new(4096),
    None,
    Some(&[read_file_tool]),
    None,
    |event| match event {
        StreamEvent::ToolCallStart { id, name } => {
            tool_call_id = id;
            println!("Tool call: {}", name);
        }
        StreamEvent::ToolCallDelta { id, arguments } => {
            if id == tool_call_id {
                tool_arguments.push_str(&arguments);
            }
        }
        StreamEvent::Done => {
            // Parse tool_arguments as JSON and execute
        }
        _ => {}
    },
).await?;
```

### Cache Hints Example

```rust
fn prepare_messages(history: Vec<Message>) -> Vec<CacheableMessage> {
    let len = history.len();
    history
        .into_iter()
        .enumerate()
        .map(|(i, msg)| {
            // Cache all but the last message (which is new)
            if i < len - 1 {
                CacheableMessage::cached(msg)
            } else {
                CacheableMessage::plain(msg)
            }
        })
        .collect()
}
```

---

## Crate Design Summary

The `forge-providers` crate provides a robust, type-safe interface for LLM API communication:

### Architectural Strengths

| Strength | Implementation |
| :--- | :--- |
| **Provider isolation** | Each provider in its own module with specific API handling |
| **Type safety** | `ApiConfig` validation prevents provider/key mismatches |
| **Unified interface** | Single `send_message` function for all providers |
| **Streaming abstraction** | Common `StreamEvent` type normalizes provider differences |
| **Shared infrastructure** | SSE parsing code reused across providers |

### Key Design Patterns

| Pattern | Purpose | Example |
| :--- | :--- | :--- |
| Provider Dispatch | Route to correct implementation | `match config.provider()` |
| Type-Encoded Provider | Prevent key/model mismatches | `ApiKey::Claude(...)` |
| Callback-Based Streaming | Deliver events without blocking | `on_event(StreamEvent::TextDelta(...))` |
| Builder Pattern | Construct configs fluently | `ApiConfig::new(...).with_openai_options(...)` |

### Quick Reference

| Provider | API Endpoint | Auth Header | Key Event Types |
| :--- | :--- | :--- | :--- |
| Claude | `api.anthropic.com/v1/messages` | `x-api-key` | `content_block_delta`, `message_stop` |
| OpenAI | `api.openai.com/v1/responses` | `Authorization: Bearer` | `response.output_text.delta`, `response.completed` |
| Gemini | `googleapis.com/v1beta/models/...` | `x-goog-api-key` | `candidates[].content.parts[]`, `[DONE]` |

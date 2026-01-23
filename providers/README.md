# forge-providers

LLM API client layer for the Forge application. Provides streaming HTTP communication with Claude, OpenAI, and Gemini APIs through a unified interface.

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Provider System](#provider-system)
4. [Type-Driven Design](#type-driven-design)
5. [SSE Streaming Infrastructure](#sse-streaming-infrastructure)
6. [Claude API Client](#claude-api-client)
7. [OpenAI API Client](#openai-api-client)
8. [Gemini API Client](#gemini-api-client)
9. [Public API Reference](#public-api-reference)
10. [Model Limits](#model-limits)
11. [Code Examples](#code-examples)
12. [Error Handling](#error-handling)
13. [Extension Guide](#extension-guide)

---

## Overview

The `forge-providers` crate handles all HTTP communication with LLM APIs. It provides a unified streaming interface that abstracts provider differences while preserving provider-specific features like Claude's extended thinking and OpenAI's reasoning controls.

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
        ├── pub mod claude
        ├── pub mod openai
        └── pub mod gemini
```

### Dependencies

| Crate | Purpose |
| :--- | :--- |
| `forge-types` | Core domain types (Provider, ModelName, Message, etc.) |
| `reqwest` | HTTP client with streaming support |
| `futures-util` | Async stream utilities for SSE processing |
| `tokio` | Async runtime with timeout support |
| `serde` / `serde_json` | JSON serialization for API payloads |
| `chrono` | DateTime handling for Gemini cache expiry |
| `uuid` | Generate tool call IDs for Gemini |
| `anyhow` / `thiserror` | Error handling |
| `tracing` | Structured logging |

---

## Architecture

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
│   │   tools ────────────────────────────────────────────────────────►   │   │
│   │   gemini_cache ─────────────────────────────────────────────────►   │   │
│   │   tx: mpsc::Sender<StreamEvent> ────────────────────────────────►   │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      │ match config.provider()
                                      ▼
             ┌────────────────────────┴────────────────────────┐
             │                        │                        │
             ▼                        ▼                        ▼
┌─────────────────────────┐  ┌─────────────────────────┐  ┌─────────────────────────┐
│  claude::send_message   │  │  openai::send_message   │  │  gemini::send_message   │
│                         │  │                         │  │                         │
│ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │
│ │build_request_body() │ │  │ │build_request_body() │ │  │ │build_request_body() │ │
│ │- System blocks      │ │  │ │- input items        │ │  │ │- system_instruction │ │
│ │- Messages array     │ │  │ │- instructions       │ │  │ │- contents array     │ │
│ │- Cache control      │ │  │ │- reasoning.effort   │ │  │ │- cachedContent ref  │ │
│ │- Thinking config    │ │  │ │- text.verbosity     │ │  │ │- thinkingConfig     │ │
│ └─────────────────────┘ │  │ └─────────────────────┘ │  │ └─────────────────────┘ │
│            │            │  │            │            │  │            │            │
│            ▼            │  │            ▼            │  │            ▼            │
│ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │
│ │   POST to API       │ │  │ │   POST to API       │ │  │ │   POST to API       │ │
│ │ api.anthropic.com   │ │  │ │ api.openai.com      │ │  │ │ googleapis.com      │ │
│ │ /v1/messages        │ │  │ │ /v1/responses       │ │  │ │ /v1beta/models/...  │ │
│ └─────────────────────┘ │  │ └─────────────────────┘ │  │ └─────────────────────┘ │
│            │            │  │            │            │  │            │            │
│            ▼            │  │            ▼            │  │            ▼            │
│ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │  │ ┌─────────────────────┐ │
│ │process_sse_stream() │ │  │ │process_sse_stream() │ │  │ │process_sse_stream() │ │
│ │  + ClaudeParser     │ │  │ │  + OpenAIParser     │ │  │ │  + GeminiParser     │ │
│ └─────────────────────┘ │  │ └─────────────────────┘ │  │ └─────────────────────┘ │
└─────────────────────────┘  └─────────────────────────┘  └─────────────────────────┘
             │                        │                        │
             └────────────────────────┴──────────┬─────────────┘
                                                 │
                                                 ▼
                                    ┌─────────────────────────┐
                                    │      tx.send(event)     │
                                    │                         │
                                    │  StreamEvent::TextDelta │
                                    │  StreamEvent::ThinkingDelta │
                                    │  StreamEvent::ToolCallStart │
                                    │  StreamEvent::ToolCallDelta │
                                    │  StreamEvent::Done      │
                                    │  StreamEvent::Error     │
                                    └─────────────────────────┘
```

---

## Provider System

### Provider Enum (from forge-types)

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
| `parse_model(raw)` | `Result<ModelName, ModelParseError>` | Validate and parse model name |

### Provider Dispatch

The `send_message` function dispatches to provider-specific implementations:

```rust
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
        Provider::Claude => claude::send_message(...).await,
        Provider::OpenAI => openai::send_message(...).await,
        Provider::Gemini => gemini::send_message(...).await,
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

Construction validates provider consistency:

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

This makes it impossible to create an `ApiConfig` with a Claude API key and an OpenAI model.

### ApiKey - Provider-Scoped Keys

```rust
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    Gemini(String),
}
```

The key's provider is encoded in its type, preventing the invalid state of using an API key with the wrong provider.

### ModelName - Provider-Scoped Models

```rust
pub struct ModelName {
    provider: Provider,
    name: Cow<'static, str>,
    kind: ModelNameKind,
}
```

Validation rules by provider:

| Provider | Prefix Requirement | Example Valid |
| :--- | :--- | :--- |
| Claude | Must start with `claude-` | `claude-sonnet-4-5-20250929` |
| OpenAI | Must start with `gpt-5` | `gpt-5.2`, `gpt-5.2-2025-12-11` |
| Gemini | Must start with `gemini-` | `gemini-3-pro-preview` |

### OutputLimits - Token Budgets

```rust
pub struct OutputLimits {
    max_output_tokens: u32,
    thinking_budget: Option<u32>,
}
```

Construction enforces invariants:

- If thinking is enabled, `thinking_budget >= 1024`
- If thinking is enabled, `thinking_budget < max_output_tokens`

### CacheHint - Provider Caching

```rust
pub enum CacheHint {
    None,      // No caching preference
    Ephemeral, // Request caching (Claude-specific)
}
```

Different providers handle caching differently:

- **Claude**: Explicit `cache_control: { type: "ephemeral" }` markers on content blocks
- **OpenAI**: Automatic server-side prefix caching (hints are ignored)
- **Gemini**: Explicit context caching via `cachedContents` API for large system prompts

---

## SSE Streaming Infrastructure

### SseParser Trait

Each provider implements this trait for JSON event parsing:

```rust
enum SseParseAction {
    Continue,                  // No event to emit
    Emit(Vec<StreamEvent>),    // Emit these events
    Done,                      // Stream completed
    Error(String),             // Fatal error
}

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
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<()>
```

This function manages:

| Feature | Behavior |
| :--- | :--- |
| Idle timeout | Default 60s, override via `FORGE_STREAM_IDLE_TIMEOUT_SECS` |
| Buffer limit | 4 MiB maximum to prevent memory exhaustion |
| UTF-8 validation | Invalid UTF-8 triggers error event |
| Event boundaries | Handles both `\n\n` and `\r\n\r\n` delimiters |
| `[DONE]` marker | Emits `StreamEvent::Done` |
| Parse errors | 3 consecutive failures trigger abort |

### Low-Level SSE Functions

```rust
/// Find event boundary position and delimiter length
fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)>

/// Drain next complete event from buffer
fn drain_next_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>>

/// Extract data payload from SSE event text
fn extract_sse_data(event: &str) -> Option<String>
```

### StreamEvent - Unified Event Type

```rust
pub enum StreamEvent {
    /// Text content chunk
    TextDelta(String),
    /// Claude extended thinking chunk
    ThinkingDelta(String),
    /// Tool call started
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    /// Tool call arguments chunk
    ToolCallDelta { id: String, arguments: String },
    /// Stream completed successfully
    Done,
    /// Stream failed with error
    Error(String),
}
```

---

## Claude API Client

### Endpoint and Authentication

```rust
const API_URL: &str = "https://api.anthropic.com/v1/messages";

client.post(API_URL)
    .header("x-api-key", config.api_key())
    .header("anthropic-version", "2023-06-01")
    .header("content-type", "application/json")
```

### Request Structure

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
  "tools": [
    { "name": "read_file", "description": "...", "input_schema": {...} }
  ],
  "thinking": { "type": "enabled", "budget_tokens": 4096 }
}
```

### Message Transformations

| Input | API Format |
| :--- | :--- |
| System prompt (parameter) | First system block with `cache_control: ephemeral` |
| `Message::System` | Additional system blocks |
| `Message::User` | User message with content blocks |
| `Message::Assistant` | Assistant message as plain string |
| `Message::ToolUse` | Assistant message with `tool_use` content block |
| `Message::ToolResult` | User message with `tool_result` content block |

### Thinking Mode Constraints

**Important**: Thinking is automatically disabled when the conversation history contains `Message::Assistant` or `Message::ToolUse` messages. This is because Claude's API requires assistant messages to start with thinking/redacted_thinking blocks when thinking is enabled, but Forge doesn't store thinking content in history.

### Response Parsing

| Event Type | Action |
| :--- | :--- |
| `content_block_start` with `tool_use` | Emit `ToolCallStart` |
| `content_block_delta` with `text_delta` | Emit `TextDelta` |
| `content_block_delta` with `thinking_delta` | Emit `ThinkingDelta` |
| `content_block_delta` with `input_json_delta` | Emit `ToolCallDelta` |
| `content_block_stop` | Reset current tool ID |
| `message_stop` | Emit `Done` |

---

## OpenAI API Client

### Endpoint and Authentication

```rust
const API_URL: &str = "https://api.openai.com/v1/responses";

client.post(API_URL)
    .header("Authorization", format!("Bearer {}", config.api_key()))
    .header("content-type", "application/json")
```

Note: This uses the OpenAI Responses API (not Chat Completions) for GPT-5.2 support.

### Request Structure

```json
{
  "model": "gpt-5.2",
  "input": [
    { "role": "developer", "content": "Context summary" },
    { "role": "user", "content": "Hello" },
    { "type": "function_call", "call_id": "...", "name": "...", "arguments": "..." },
    { "type": "function_call_output", "call_id": "...", "output": "..." }
  ],
  "instructions": "System prompt",
  "max_output_tokens": 4096,
  "stream": true,
  "truncation": "auto",
  "reasoning": { "effort": "high" },
  "text": { "verbosity": "high" },
  "tools": [
    { "type": "function", "name": "...", "description": "...", "parameters": {...} }
  ]
}
```

### Role Mapping

Per the OpenAI Model Spec authority hierarchy:

| Message Type | OpenAI Role | Rationale |
| :--- | :--- | :--- |
| `Message::System` | `"developer"` | "System" is reserved for OpenAI runtime |
| `Message::User` | `"user"` | Standard user role |
| `Message::Assistant` | `"assistant"` | Standard assistant role |
| `Message::ToolUse` | `function_call` item | Tool invocation record |
| `Message::ToolResult` | `function_call_output` item | Tool result record |

### GPT-5 Options

For models starting with `gpt-5`, additional parameters are included:

```rust
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,  // none, low, medium, high, xhigh
    verbosity: OpenAITextVerbosity,           // low, medium, high
    truncation: OpenAITruncation,             // auto, disabled
}
```

### Response Parsing

| Event Type | Action |
| :--- | :--- |
| `response.output_item.added` with `function_call` | Emit `ToolCallStart` (and initial args if present) |
| `response.output_text.delta` | Emit `TextDelta` |
| `response.refusal.delta` | Emit `TextDelta` (model refused) |
| `response.function_call_arguments.delta` | Emit `ToolCallDelta` |
| `response.function_call_arguments.done` | Emit `ToolCallDelta` (if no prior deltas) |
| `response.completed` | Emit `Done` |
| `response.incomplete` | Emit `Error` with reason |
| `response.failed` / `error` | Emit `Error` with message |

The parser maintains state to map `item_id` to `call_id` and tracks which calls have received deltas to avoid duplicate emissions.

---

## Gemini API Client

### Endpoint and Authentication

```rust
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

let url = format!("{API_BASE}/models/{model}:streamGenerateContent?alt=sse");

client.post(&url)
    .header("x-goog-api-key", config.api_key())
    .header("content-type", "application/json")
```

### Request Structure

Note: Gemini API uses mixed casing (`system_instruction` snake_case, `generationConfig` camelCase).

```json
{
  "system_instruction": {
    "parts": [{ "text": "System prompt" }]
  },
  "contents": [
    { "role": "user", "parts": [{ "text": "Hello" }] },
    { "role": "model", "parts": [{ "text": "Hi there" }] },
    { "role": "model", "parts": [{ "functionCall": { "name": "...", "args": {...} } }] },
    { "role": "user", "parts": [{ "functionResponse": { "name": "...", "response": {...} } }] }
  ],
  "generationConfig": {
    "maxOutputTokens": 8192,
    "temperature": 1.0,
    "thinkingConfig": {
      "thinkingLevel": "high",
      "includeThoughts": true
    }
  },
  "tools": [{
    "functionDeclarations": [
      { "name": "...", "description": "...", "parameters": {...} }
    ]
  }]
}
```

### Message Grouping

Gemini requires consecutive tool calls and tool results to be grouped:

- Multiple consecutive `Message::ToolUse` become a single `model` content entry with multiple `functionCall` parts
- Multiple consecutive `Message::ToolResult` become a single `user` content entry with multiple `functionResponse` parts

### Thought Signatures

Gemini requires `thoughtSignature` on tool calls when thinking mode was used. This is preserved from `ToolCall.thought_signature`.

### Schema Sanitization

The `additionalProperties` field is recursively removed from tool parameter schemas, as Gemini doesn't support it.

### Context Caching

Gemini supports explicit context caching for large system prompts:

```rust
pub struct GeminiCache {
    pub name: String,                    // "cachedContents/abc123"
    pub expire_time: DateTime<Utc>,
    pub system_prompt_hash: u64,
}

pub async fn create_cache(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    ttl_seconds: u32,
) -> Result<GeminiCache>
```

**Minimum token requirements**:

- Gemini 3 Pro: 4,096 tokens (~16,384 characters)
- Gemini Flash models: 1,024 tokens (~4,096 characters)

When a cache is provided, the request uses `cachedContent` instead of `system_instruction`.

### Response Parsing

| Condition | Action |
| :--- | :--- |
| `candidates[].content.parts[].text` | Emit `TextDelta` |
| `candidates[].content.parts[].thought == true` | Emit `ThinkingDelta` |
| `candidates[].content.parts[].functionCall` | Emit `ToolCallStart` + `ToolCallDelta` |
| `finishReason` is `STOP` or `MAX_TOKENS` | Emit `Done` |
| `finishReason` is `SAFETY`, `RECITATION`, etc. | Emit `Error` |
| `error` field present | Emit `Error` |

Gemini doesn't provide tool call IDs, so the parser generates UUIDs: `call_{uuid}`.

---

## Public API Reference

### Primary Function

```rust
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    gemini_cache: Option<&gemini::GeminiCache>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<()>
```

Returns `Ok(())` on completion. Errors are delivered through `StreamEvent::Error`. Only returns `Err(...)` for unrecoverable failures (network errors).

### ApiConfig Methods

| Method | Return Type | Description |
| :--- | :--- | :--- |
| `new(api_key, model)` | `Result<Self, ApiConfigError>` | Create with validation |
| `with_openai_options(options)` | `Self` | Builder for OpenAI options |
| `provider()` | `Provider` | Get the provider |
| `api_key()` | `&str` | Get the API key string |
| `model()` | `&ModelName` | Get the model name |
| `openai_options()` | `OpenAIRequestOptions` | Get OpenAI options |

### HTTP Client Functions

```rust
/// Shared HTTP client for streaming requests (no total timeout)
pub fn http_client() -> &'static reqwest::Client

/// HTTP client with timeout for synchronous operations
pub fn http_client_with_timeout(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error>

/// Read error body with 32 KiB limit
pub async fn read_capped_error_body(response: reqwest::Response) -> String
```

### Re-exports

```rust
pub use forge_types;
```

---

## Model Limits

Token limits are defined in `forge-context/src/model_limits.rs`:

| Model Prefix | Context Window | Max Output |
| :--- | :--- | :--- |
| `claude-opus-4-5` | 200,000 | 64,000 |
| `claude-sonnet-4-5` | 200,000 | 64,000 |
| `claude-haiku-4-5` | 200,000 | 64,000 |
| `gpt-5.2` | 400,000 | 128,000 |
| `gemini-3-pro` | 1,048,576 | 65,536 |
| `gemini-3-flash` | 1,048,576 | 65,536 |
| Unknown models | 8,192 | 4,096 |

---

## Code Examples

### Basic Streaming Request

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};
use tokio::sync::mpsc;

async fn chat_with_claude() -> anyhow::Result<()> {
    let api_key = ApiKey::Claude(std::env::var("ANTHROPIC_API_KEY")?);
    let model = Provider::Claude.default_model();
    let config = ApiConfig::new(api_key, model)?;

    let messages = vec![
        CacheableMessage::plain(Message::try_user("What is the capital of France?")?)
    ];

    let (tx, mut rx) = mpsc::channel(32);

    // Spawn the streaming request
    let handle = tokio::spawn(async move {
        send_message(
            &config,
            &messages,
            OutputLimits::new(4096),
            Some("You are a helpful assistant."),
            None, // No tools
            None, // No gemini cache
            tx,
        ).await
    });

    // Process events
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta(text) => print!("{}", text),
            StreamEvent::Done => println!("\n[Done]"),
            StreamEvent::Error(e) => eprintln!("Error: {}", e),
            _ => {}
        }
    }

    handle.await??;
    Ok(())
}
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

### Claude Extended Thinking

```rust
let config = ApiConfig::new(
    ApiKey::Claude("...".into()),
    Provider::Claude.parse_model("claude-opus-4-5-20251101")?,
)?;

// Enable thinking with 4096 token budget
let limits = OutputLimits::with_thinking(16384, 4096)?;

let (tx, mut rx) = mpsc::channel(32);

tokio::spawn(async move {
    send_message(&config, &messages, limits, None, None, None, tx).await
});

while let Some(event) = rx.recv().await {
    match event {
        StreamEvent::ThinkingDelta(thought) => {
            // Internal reasoning (typically hidden)
            eprint!("[thinking] {}", thought);
        }
        StreamEvent::TextDelta(text) => print!("{}", text),
        _ => {}
    }
}
```

### Tool Calling

```rust
use serde_json::json;

let read_file_tool = ToolDefinition::new(
    "read_file",
    "Read the contents of a file",
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path" }
        },
        "required": ["path"]
    }),
);

let (tx, mut rx) = mpsc::channel(32);
let mut tool_calls: HashMap<String, (String, String)> = HashMap::new();

tokio::spawn(async move {
    send_message(
        &config,
        &messages,
        OutputLimits::new(4096),
        None,
        Some(&[read_file_tool]),
        None,
        tx,
    ).await
});

while let Some(event) = rx.recv().await {
    match event {
        StreamEvent::ToolCallStart { id, name, .. } => {
            tool_calls.insert(id, (name, String::new()));
        }
        StreamEvent::ToolCallDelta { id, arguments } => {
            if let Some((_, args)) = tool_calls.get_mut(&id) {
                args.push_str(&arguments);
            }
        }
        StreamEvent::Done => {
            for (id, (name, args)) in &tool_calls {
                println!("Tool: {} Args: {}", name, args);
            }
        }
        _ => {}
    }
}
```

### Gemini Context Caching

```rust
use forge_providers::gemini::{create_cache, GeminiCache};

// Create cache for large system prompt (>16K chars for Pro models)
let cache = create_cache(
    &api_key,
    "gemini-3-pro-preview",
    &large_system_prompt,
    3600, // TTL in seconds
).await?;

// Use cache in subsequent requests
send_message(
    &config,
    &messages,
    limits,
    None, // System prompt is in cache
    tools,
    Some(&cache),
    tx,
).await?;

// Check cache validity
if cache.is_expired() || !cache.matches_prompt(&system_prompt) {
    // Recreate cache
}
```

---

## Error Handling

### ApiConfigError

```rust
#[derive(Debug, thiserror::Error)]
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}
```

### Error Propagation Pattern

Streaming errors are delivered as events rather than `Result::Err`:

```rust
// Errors during streaming → delivered via channel
let _ = tx.send(StreamEvent::Error(error_message)).await;
return Ok(()); // Function succeeds, error communicated via channel

// Only return Err for unrecoverable failures
let chunk = chunk?; // Network errors propagate
```

This allows partial responses to be captured before an error occurs.

### Error Body Reading

Error responses are capped at 32 KiB to prevent memory exhaustion:

```rust
pub async fn read_capped_error_body(response: reqwest::Response) -> String
```

---

## Extension Guide

### Adding a New Provider

1. **Extend Provider enum** (in `forge-types/src/lib.rs`):
   - Add variant to `Provider`
   - Implement all `Provider` methods

2. **Extend ApiKey enum** (in `forge-types/src/lib.rs`):
   - Add variant to `ApiKey`
   - Update `provider()` and `as_str()` methods

3. **Add model limits** (in `forge-context/src/model_limits.rs`):
   - Add entry to `KNOWN_MODELS` array

4. **Add provider module** (in `providers/src/lib.rs`):

```rust
pub mod your_provider {
    use super::*;

    const API_URL: &str = "https://api.yourprovider.com/v1/chat";

    #[derive(Default)]
    struct YourProviderParser;

    impl SseParser for YourProviderParser {
        fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
            // Parse provider-specific JSON events
        }
        fn provider_name(&self) -> &'static str { "YourProvider" }
    }

    fn build_request_body(...) -> serde_json::Value {
        // Build provider-specific request
    }

    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let body = build_request_body(...);
        let response = http_client()
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", config.api_key()))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = read_capped_error_body(response).await;
            let _ = tx.send(StreamEvent::Error(format!("API error: {}", error_text))).await;
            return Ok(());
        }

        let mut parser = YourProviderParser;
        process_sse_stream(response, &mut parser, &tx).await
    }
}
```

5. **Update send_message dispatch**:

```rust
match config.provider() {
    // ... existing providers ...
    Provider::YourProvider => your_provider::send_message(...).await,
}
```

---

## Quick Reference

| Provider | Endpoint | Auth Header | Key SSE Events |
| :--- | :--- | :--- | :--- |
| Claude | `api.anthropic.com/v1/messages` | `x-api-key` | `content_block_delta`, `message_stop` |
| OpenAI | `api.openai.com/v1/responses` | `Authorization: Bearer` | `response.output_text.delta`, `response.completed` |
| Gemini | `googleapis.com/v1beta/models/...` | `x-goog-api-key` | `candidates[].content.parts[]` |

| Constant | Value | Purpose |
| :--- | :--- | :--- |
| `CONNECT_TIMEOUT_SECS` | 30 | HTTP connection timeout |
| `DEFAULT_STREAM_IDLE_TIMEOUT_SECS` | 60 | SSE idle timeout |
| `MAX_SSE_BUFFER_BYTES` | 4 MiB | Buffer size limit |
| `MAX_SSE_PARSE_ERRORS` | 3 | Consecutive parse error threshold |
| `MAX_ERROR_BODY_BYTES` | 32 KiB | Error response size limit |

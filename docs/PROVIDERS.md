# Providers Crate Documentation

The `forge-providers` crate handles HTTP communication with LLM provider APIs (Claude/Anthropic and OpenAI). It provides a unified streaming interface that abstracts provider-specific protocol details while maintaining type safety to prevent configuration errors.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Core Types](#core-types)
3. [Provider Dispatch Mechanism](#provider-dispatch-mechanism)
4. [Claude (Anthropic) Implementation](#claude-anthropic-implementation)
5. [OpenAI Implementation](#openai-implementation)
6. [SSE Stream Processing](#sse-stream-processing)
7. [Tool Calling / Function Calling](#tool-calling--function-calling)
8. [Error Handling](#error-handling)
9. [Security Considerations](#security-considerations)
10. [Adding a New Provider](#adding-a-new-provider)
11. [Usage Examples](#usage-examples)

---

## Architecture Overview

### Design Philosophy

The crate follows several key design principles:

1. **Thin Protocol Layer**: Only handles HTTP I/O and protocol translation. Business logic, context management, and UI concerns live elsewhere.
2. **Type-Driven Safety**: Provider/key/model mismatches are caught at construction time, not runtime.
3. **Streaming-First**: All API calls use Server-Sent Events (SSE) for real-time response delivery.
4. **Callback-Based Events**: Streaming events are delivered via a callback function, allowing immediate processing without buffering.

### Module Structure

```
providers/src/lib.rs
├── Shared HTTP Client           # Global reqwest client configuration
├── SSE Parsing Functions        # Event boundary detection and data extraction
├── ApiConfig                    # Configuration container with provider validation
├── send_message()               # Top-level dispatch function
├── claude module                # Anthropic Messages API implementation
│   ├── build_request_body()     # JSON payload construction
│   └── send_message()           # HTTP POST + SSE stream processing
└── openai module                # OpenAI Responses API implementation
    ├── build_request_body()     # JSON payload construction
    ├── handle_openai_stream_event()  # Event type routing
    └── send_message()           # HTTP POST + SSE stream processing
```

### Request Flow Diagram

```
                    ┌─────────────────────────────────────┐
                    │           send_message()            │
                    │  (top-level dispatch function)      │
                    └──────────────┬──────────────────────┘
                                   │
                                   │ config.provider()
                                   │
          ┌────────────────────────┼────────────────────────┐
          │                        │                        │
          ▼                        │                        ▼
┌─────────────────────┐            │            ┌─────────────────────┐
│  Provider::Claude   │            │            │  Provider::OpenAI   │
└──────────┬──────────┘            │            └──────────┬──────────┘
           │                       │                       │
           ▼                       │                       ▼
┌─────────────────────┐            │            ┌─────────────────────┐
│ claude::send_message│            │            │openai::send_message │
│                     │            │            │                     │
│ 1. Build JSON body  │            │            │ 1. Build JSON body  │
│ 2. POST to API      │            │            │ 2. POST to API      │
│ 3. Stream SSE       │            │            │ 3. Stream SSE       │
│ 4. Parse events     │            │            │ 4. Parse events     │
│ 5. Call on_event    │            │            │ 5. Call on_event    │
└─────────────────────┘            │            └─────────────────────┘
                                   │
                    ┌──────────────┴──────────────┐
                    │         StreamEvent         │
                    │  - TextDelta(String)        │
                    │  - ThinkingDelta(String)    │
                    │  - ToolCallStart{id, name}  │
                    │  - ToolCallDelta{id, args}  │
                    │  - Done                     │
                    │  - Error(String)            │
                    └─────────────────────────────┘
```

---

## Core Types

### ApiConfig

Configuration container that enforces provider consistency between API keys and models.

```rust
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
}
```

**Construction**:

```rust
impl ApiConfig {
    /// Creates a new config. Returns error if key and model providers differ.
    pub fn new(api_key: ApiKey, model: ModelName) -> Result<Self, ApiConfigError>;
    
    /// Builder method for OpenAI-specific options.
    pub fn with_openai_options(self, options: OpenAIRequestOptions) -> Self;
}
```

**Accessors**:

| Method | Return Type | Description |
|--------|-------------|-------------|
| `provider()` | `Provider` | The provider (Claude or OpenAI) |
| `api_key()` | `&str` | Raw API key string |
| `api_key_owned()` | `ApiKey` | Cloned provider-scoped key |
| `model()` | `&ModelName` | The model name with provider scope |
| `openai_options()` | `OpenAIRequestOptions` | OpenAI-specific request settings |

**Example**:

```rust
let api_key = ApiKey::Claude(std::env::var("ANTHROPIC_API_KEY")?);
let model = Provider::Claude.parse_model("claude-sonnet-4-5-20250929")?;

// This succeeds because both are Claude
let config = ApiConfig::new(api_key, model)?;

// This would fail at construction time:
// let bad_config = ApiConfig::new(ApiKey::OpenAI("..."), model)?;
// Error: ProviderMismatch { key: OpenAI, model: Claude }
```

### ApiConfigError

```rust
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}
```

### Shared HTTP Client

The crate provides two HTTP client functions:

```rust
/// Shared client for streaming requests (no total timeout).
/// - Connection timeout: 30 seconds
/// - No read/total timeout (SSE streams can run indefinitely)
/// - HTTPS only
/// - Redirects disabled
pub fn http_client() -> &'static reqwest::Client;

/// Client with total request timeout for synchronous operations.
/// Use for non-streaming requests like summarization.
pub fn http_client_with_timeout(timeout_secs: u64) -> reqwest::Client;
```

The shared client is lazily initialized via `OnceLock` and reused for all streaming requests. This avoids the overhead of creating new TCP connections for each API call.

---

## Provider Dispatch Mechanism

### The `send_message` Function

This is the primary entry point for all API requests:

```rust
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()>
```

**Parameters**:

| Parameter | Type | Description |
|-----------|------|-------------|
| `config` | `&ApiConfig` | API key, model name, and provider-specific options |
| `messages` | `&[CacheableMessage]` | Conversation history with optional cache hints |
| `limits` | `OutputLimits` | Maximum output tokens and optional thinking budget |
| `system_prompt` | `Option<&str>` | System instructions injected before conversation |
| `tools` | `Option<&[ToolDefinition]>` | Available tools for function calling |
| `on_event` | `impl Fn(StreamEvent)` | Callback invoked for each streaming event |

**Dispatch Logic**:

```rust
match config.provider() {
    Provider::Claude => {
        claude::send_message(config, messages, limits, system_prompt, tools, on_event).await
    }
    Provider::OpenAI => {
        openai::send_message(config, messages, limits, system_prompt, tools, on_event).await
    }
}
```

The dispatch is exhaustive via Rust's match expression, so adding a new provider variant to the `Provider` enum will cause a compile error until the new match arm is added.

---

## Claude (Anthropic) Implementation

### API Endpoint

```
POST https://api.anthropic.com/v1/messages
```

### Request Headers

```
x-api-key: <API_KEY>
anthropic-version: 2023-06-01
content-type: application/json
```

### Request Body Structure

The `build_request_body` function constructs the JSON payload:

```rust
fn build_request_body(
    model: &str,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
) -> serde_json::Value
```

**Key transformations**:

1. **System Messages Hoisting**: All `Message::System` variants are extracted from the conversation and placed in a top-level `system` array (Anthropic requires this separation).

2. **System Prompt Injection**: If provided, the system prompt is prepended to the `system` array with `cache_control: { type: "ephemeral" }`.

3. **Content Block Format**: User messages use content blocks with optional cache control:
   ```json
   {
     "role": "user",
     "content": [{
       "type": "text",
       "text": "...",
       "cache_control": { "type": "ephemeral" }
     }]
   }
   ```

4. **Assistant Messages**: Sent as plain strings (cache control not supported by Anthropic for assistant content):
   ```json
   {
     "role": "assistant",
     "content": "..."
   }
   ```

5. **Tool Use Messages**: Transformed to `tool_use` content blocks:
   ```json
   {
     "role": "assistant",
     "content": [{
       "type": "tool_use",
       "id": "call_123",
       "name": "read_file",
       "input": {"path": "/foo/bar"}
     }]
   }
   ```

6. **Tool Result Messages**: Transformed to `tool_result` content blocks:
   ```json
   {
     "role": "user",
     "content": [{
       "type": "tool_result",
       "tool_use_id": "call_123",
       "content": "file contents...",
       "is_error": false
     }]
   }
   ```

7. **Tool Definitions**: Converted to Anthropic's tool schema format:
   ```json
   {
     "tools": [{
       "name": "read_file",
       "description": "Read a file from disk",
       "input_schema": { "type": "object", ... }
     }]
   }
   ```

8. **Thinking Mode**: If `limits.thinking_budget()` is `Some`, adds:
   ```json
   {
     "thinking": {
       "type": "enabled",
       "budget_tokens": 4096
     }
   }
   ```

### SSE Event Types (Claude)

| Event Type | Delta Type | Maps To |
|------------|------------|---------|
| `content_block_start` | `tool_use` | `StreamEvent::ToolCallStart` |
| `content_block_delta` | `text_delta` | `StreamEvent::TextDelta` |
| `content_block_delta` | `thinking_delta` | `StreamEvent::ThinkingDelta` |
| `content_block_delta` | `input_json_delta` | `StreamEvent::ToolCallDelta` |
| `content_block_stop` | - | (resets tool tracking state) |
| `message_stop` | - | `StreamEvent::Done` |
| `[DONE]` marker | - | `StreamEvent::Done` |

### Cache Control Limits

Anthropic limits `cache_control` markers to **4 blocks per request**. The system prompt uses one slot, leaving 3 for messages. The crate applies cache hints on user messages in chronological order (oldest first get priority) but does not enforce the limit - callers should cap cache hints at the context preparation layer.

---

## OpenAI Implementation

### API Endpoint

```
POST https://api.openai.com/v1/responses
```

Note: This uses the OpenAI Responses API, not the Chat Completions API. The Responses API is designed for streaming and provides more granular event types.

### Request Headers

```
Authorization: Bearer <API_KEY>
content-type: application/json
```

### Request Body Structure

```rust
fn build_request_body(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    tools: Option<&[ToolDefinition]>,
) -> Value
```

**Key transformations**:

1. **Role Mapping**: Per the OpenAI Model Spec hierarchy (Root > System > Developer > User), `Message::System` maps to `"developer"` role, not `"system"`:
   ```rust
   fn openai_role(msg: &Message) -> &'static str {
       match msg {
           Message::System(_) => "developer",  // API developers operate at this level
           Message::User(_) => "user",
           Message::Assistant(_) => "assistant",
           Message::ToolUse(_) => "assistant",
           Message::ToolResult(_) => "user",
       }
   }
   ```

2. **Input Array Format**:
   ```json
   {
     "model": "gpt-5.2",
     "input": [
       { "role": "developer", "content": "..." },
       { "role": "user", "content": "..." }
     ],
     "max_output_tokens": 4096,
     "stream": true
   }
   ```

3. **Tool Use Messages**: Converted to `function_call` items:
   ```json
   {
     "type": "function_call",
     "call_id": "call_123",
     "name": "read_file",
     "arguments": "{\"path\":\"/foo/bar\"}"
   }
   ```

4. **Tool Result Messages**: Converted to `function_call_output` items:
   ```json
   {
     "type": "function_call_output",
     "call_id": "call_123",
     "output": "file contents..."
   }
   ```

5. **Tool Definitions**: Converted to OpenAI's function format:
   ```json
   {
     "tools": [{
       "type": "function",
       "name": "read_file",
       "description": "Read a file from disk",
       "parameters": { "type": "object", ... }
     }]
   }
   ```

6. **System Prompt**: Via `instructions` field:
   ```json
   {
     "instructions": "You are a helpful assistant."
   }
   ```

7. **GPT-5 Specific Options** (applied only when model starts with `gpt-5`):
   ```json
   {
     "reasoning": { "effort": "high" },
     "text": { "verbosity": "medium" },
     "truncation": "auto"
   }
   ```

### OpenAI Request Options

```rust
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,  // none, low, medium, high, xhigh
    verbosity: OpenAITextVerbosity,           // low, medium, high
    truncation: OpenAITruncation,             // auto, disabled
}
```

These options control GPT-5's reasoning and output behavior:

- **reasoning_effort**: How much internal reasoning the model performs
- **verbosity**: Output length preference
- **truncation**: Whether to automatically truncate long inputs

### SSE Event Types (OpenAI)

| Event Type | Maps To |
|------------|---------|
| `response.output_item.added` (function_call) | `StreamEvent::ToolCallStart` + optional `ToolCallDelta` |
| `response.output_text.delta` | `StreamEvent::TextDelta` |
| `response.refusal.delta` | `StreamEvent::TextDelta` (refusals treated as text) |
| `response.output_text.done` | `StreamEvent::TextDelta` (fallback if no deltas seen) |
| `response.function_call_arguments.delta` | `StreamEvent::ToolCallDelta` |
| `response.function_call_arguments.done` | `StreamEvent::ToolCallDelta` (if no prior deltas) |
| `response.completed` | `StreamEvent::Done` |
| `response.incomplete` | `StreamEvent::Error` |
| `response.failed` / `error` | `StreamEvent::Error` |

### Item ID to Call ID Mapping

OpenAI uses two different identifiers:
- `item_id`: Internal identifier for the output item
- `call_id`: Identifier used for matching tool results

The implementation maintains a mapping (`item_to_call: HashMap<String, String>`) to translate between these, ensuring `ToolCallDelta` events use the correct `call_id`.

---

## SSE Stream Processing

### Event Boundary Detection

SSE events are delimited by double newlines (`\n\n` or `\r\n\r\n`). The parser handles both:

```rust
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
```

The function returns `(position, delimiter_length)` of the earliest boundary found.

### Event Draining

Events are extracted from the buffer and removed atomically:

```rust
fn drain_next_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let (pos, delim_len) = find_sse_event_boundary(buffer)?;
    let event = buffer[..pos].to_vec();
    buffer.drain(..pos + delim_len);
    Some(event)
}
```

### Data Line Extraction

SSE events may contain multiple lines. Only `data:` lines are relevant:

```rust
fn extract_sse_data(event: &str) -> Option<String> {
    let mut data = String::new();
    let mut found = false;

    for line in event.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        
        if let Some(mut rest) = line.strip_prefix("data:") {
            if let Some(stripped) = rest.strip_prefix(' ') {
                rest = stripped;
            }
            if found { data.push('\n'); }
            data.push_str(rest);
            found = true;
        }
    }

    if found { Some(data) } else { None }
}
```

Multi-line data (multiple `data:` lines in one event) is joined with newlines.

### Stream Processing Loop

Both provider implementations follow this pattern:

```rust
let mut buffer: Vec<u8> = Vec::new();

while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    buffer.extend_from_slice(&chunk);
    
    // Security: prevent unbounded buffer growth
    if buffer.len() > MAX_SSE_BUFFER_BYTES {
        on_event(StreamEvent::Error("SSE buffer exceeded maximum size".into()));
        return Ok(());
    }
    
    // Process all complete events in buffer
    while let Some(event) = drain_next_sse_event(&mut buffer) {
        if event.is_empty() { continue; }
        
        let event = std::str::from_utf8(&event)?;
        
        if let Some(data) = extract_sse_data(event) {
            if data == "[DONE]" {
                on_event(StreamEvent::Done);
                return Ok(());
            }
            
            // Parse JSON and dispatch to StreamEvent
            if let Ok(json) = serde_json::from_str::<Value>(&data) {
                // Provider-specific event handling...
            }
        }
    }
}
```

---

## Tool Calling / Function Calling

The providers crate supports tool/function calling for both Claude and OpenAI APIs.

### Tool Definition

Tools are defined using `ToolDefinition`:

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema
}
```

### Streaming Tool Calls

Tool calls stream in multiple phases:

1. **Start Event**: `StreamEvent::ToolCallStart { id, name }` - Emitted when a tool call begins
2. **Delta Events**: `StreamEvent::ToolCallDelta { id, arguments }` - JSON argument fragments
3. **Completion**: The caller assembles fragments and parses the complete JSON

### Claude Tool Call Format

Claude uses content blocks for tool calls:

```json
{
  "type": "content_block_start",
  "content_block": {
    "type": "tool_use",
    "id": "toolu_01ABC",
    "name": "read_file"
  }
}
```

Arguments stream via `input_json_delta`:

```json
{
  "type": "content_block_delta",
  "delta": {
    "type": "input_json_delta",
    "partial_json": "{\"path\":\"/foo"
  }
}
```

### OpenAI Tool Call Format

OpenAI uses output items:

```json
{
  "type": "response.output_item.added",
  "item": {
    "type": "function_call",
    "id": "item_1",
    "call_id": "call_ABC",
    "name": "read_file",
    "arguments": ""
  }
}
```

Arguments stream separately:

```json
{
  "type": "response.function_call_arguments.delta",
  "item_id": "item_1",
  "delta": "{\"path\":\"/foo/bar\"}"
}
```

---

## Error Handling

### Error Categories

1. **Configuration Errors** (`ApiConfigError`):
   - Returned immediately when creating `ApiConfig` with mismatched provider/key
   - Example: Using a Claude API key with an OpenAI model

2. **Network Errors** (via `anyhow::Error`):
   - Connection timeouts
   - DNS resolution failures
   - TLS errors

3. **API Errors** (via `StreamEvent::Error`):
   - HTTP 4xx/5xx responses
   - Rate limiting
   - Invalid requests
   - Incomplete responses

### Error Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        send_message()                           │
└───────────────────────────────┬─────────────────────────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        │                       │                       │
        ▼                       ▼                       ▼
┌───────────────┐      ┌───────────────┐      ┌───────────────┐
│ Network Error │      │ HTTP Error    │      │ Stream Error  │
│ (anyhow)      │      │ (4xx/5xx)     │      │ (incomplete)  │
└───────┬───────┘      └───────┬───────┘      └───────┬───────┘
        │                      │                      │
        ▼                      ▼                      ▼
   Result::Err           StreamEvent::Error     StreamEvent::Error
```

### Capped Error Body Reading

To prevent memory exhaustion from large error responses:

```rust
const MAX_ERROR_BODY_BYTES: usize = 32 * 1024;  // 32 KiB

async fn read_capped_error_body(response: reqwest::Response) -> String {
    // Reads up to MAX_ERROR_BODY_BYTES, truncates with "...(truncated)" if exceeded
}
```

### Premature EOF Detection

If the connection closes before a completion event:

```rust
if !saw_done {
    on_event(StreamEvent::Error(
        "Connection closed before stream completed".to_string(),
    ));
}
```

---

## Security Considerations

### Buffer Size Limits

```rust
/// Maximum bytes for SSE buffer (4 MiB)
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Maximum bytes for error body reads (32 KiB)
const MAX_ERROR_BODY_BYTES: usize = 32 * 1024;
```

These limits prevent memory exhaustion from:
- Malicious servers sending unbounded data
- Misbehaving proxies that don't forward event delimiters
- Large error responses

### HTTPS Only

```rust
reqwest::Client::builder()
    .https_only(true)
    // ...
```

All API requests are forced to use HTTPS, preventing accidental plaintext transmission of API keys.

### No Redirects

```rust
.redirect(reqwest::redirect::Policy::none())
```

Redirects are disabled because:
- API endpoints should never redirect
- Redirect following could leak API keys to unintended hosts

### API Key Handling

API keys are:
- Scoped to their provider via `ApiKey` enum
- Never logged (no `Debug` impl on sensitive fields)
- Validated against model provider at `ApiConfig` construction

---

## Adding a New Provider

To add support for a new LLM provider, follow these steps:

### 1. Add Provider Variant (in `forge-types`)

```rust
// types/src/lib.rs
pub enum Provider {
    Claude,
    OpenAI,
    NewProvider,  // Add new variant
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAI => "openai",
            Provider::NewProvider => "newprovider",
        }
    }
    
    pub fn default_model(&self) -> ModelName {
        match self {
            // ... existing arms
            Provider::NewProvider => ModelName::known(*self, "newprovider-default"),
        }
    }
    
    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            // ... existing arms
            Provider::NewProvider => &["newprovider-default", "newprovider-large"],
        }
    }
    
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            // ... existing arms
            "newprovider" | "np" => Some(Provider::NewProvider),
            _ => None,
        }
    }
}
```

### 2. Add API Key Variant (in `forge-types`)

```rust
// types/src/lib.rs
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    NewProvider(String),  // Add new variant
}

impl ApiKey {
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
            ApiKey::NewProvider(_) => Provider::NewProvider,
        }
    }
}
```

### 3. Add Model Parsing Rules (in `forge-types`)

```rust
// types/src/lib.rs
impl ModelName {
    pub fn parse(provider: Provider, raw: &str) -> Result<Self, ModelParseError> {
        // ... existing validation
        
        if provider == Provider::NewProvider && !trimmed.starts_with("np-") {
            return Err(ModelParseError::NewProviderPrefix(trimmed.to_string()));
        }
        
        // ... rest of function
    }
}
```

### 4. Add Provider Module (in `forge-providers`)

```rust
// providers/src/lib.rs

/// NewProvider API implementation.
pub mod newprovider {
    use super::*;
    
    const API_URL: &str = "https://api.newprovider.com/v1/chat";
    
    fn build_request_body(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
    ) -> serde_json::Value {
        // Convert messages to provider's expected format
        // Handle system prompts
        // Handle tool definitions
        todo!()
    }
    
    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        tools: Option<&[ToolDefinition]>,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        let client = http_client();
        let body = build_request_body(config, messages, limits, system_prompt, tools);
        
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
        
        // Process SSE stream using shared parsing functions
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.extend_from_slice(&chunk);
            
            if buffer.len() > MAX_SSE_BUFFER_BYTES {
                on_event(StreamEvent::Error("SSE buffer exceeded maximum size".into()));
                return Ok(());
            }
            
            while let Some(event) = drain_next_sse_event(&mut buffer) {
                // Parse and map to StreamEvent variants
                // Provider-specific event type handling goes here
            }
        }
        
        Ok(())
    }
}
```

### 5. Add Dispatch Arm

```rust
// providers/src/lib.rs
pub async fn send_message(
    config: &ApiConfig,
    // ... parameters
) -> Result<()> {
    match config.provider() {
        Provider::Claude => claude::send_message(/* ... */).await,
        Provider::OpenAI => openai::send_message(/* ... */).await,
        Provider::NewProvider => newprovider::send_message(/* ... */).await,
    }
}
```

### 6. Add Tests

```rust
#[cfg(test)]
mod newprovider_tests {
    use super::*;
    
    #[test]
    fn builds_correct_request_body() {
        // Test message transformation
    }
    
    #[test]
    fn parses_sse_events_correctly() {
        // Test event parsing
    }
}
```

### 7. Update Configuration (in `forge-engine`)

Add support for the new provider in config parsing:

```rust
// engine/src/config.rs
pub fn provider_from_str(s: &str) -> Option<Provider> {
    Provider::parse(s)  // Already handles new variant via forge-types
}
```

---

## Usage Examples

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
        None,  // No tools
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

### Tool Calling

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

### Cache Hints for Long Conversations

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

## Testing

Run the crate's tests:

```bash
cargo test -p forge-providers
```

Test categories:

- `api_config_*`: Configuration validation
- `sse_boundary::*`: SSE event boundary detection
- `sse_drain::*`: Event extraction from buffer
- `sse_extract::*`: Data line parsing
- `claude::tests::*`: Claude request body construction
- `openai::tests::*`: OpenAI request body construction and event handling

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `forge-types` | Domain types (Provider, Message, StreamEvent, etc.) |
| `reqwest` | HTTP client with streaming support |
| `futures-util` | `StreamExt` for async byte stream iteration |
| `serde` / `serde_json` | JSON serialization for API payloads |
| `anyhow` | Flexible error handling |
| `thiserror` | Custom error type derivation |
| `tracing` | Structured logging |

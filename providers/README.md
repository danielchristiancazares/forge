# forge-providers

LLM API client layer for the Forge application. Provides streaming HTTP communication with Claude, OpenAI, and Gemini APIs through a unified interface, with automatic retry and typed SSE event deserialization.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
| :--- | :--- |
| 1-45 | Header, LLM-TOC, Table of Contents |
| 47-95 | Overview |
| 97-164 | Architecture |
| 166-218 | Provider System |
| 220-333 | Type-Driven Design |
| 335-438 | SSE Streaming Infrastructure |
| 440-476 | Typed SSE Event Structures |
| 478-522 | Retry Infrastructure |
| 524-633 | Claude API Client |
| 635-730 | OpenAI API Client |
| 732-860 | Gemini API Client |
| 862-920 | Public API Reference |
| 922-936 | Model Limits |
| 938-1121 | Code Examples |
| 1123-1166 | Error Handling |
| 1168-1253 | Extension Guide |
| 1255-1274 | Quick Reference |

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Provider System](#provider-system)
4. [Type-Driven Design](#type-driven-design)
5. [SSE Streaming Infrastructure](#sse-streaming-infrastructure)
6. [Typed SSE Event Structures](#typed-sse-event-structures)
7. [Retry Infrastructure](#retry-infrastructure)
8. [Claude API Client](#claude-api-client)
9. [OpenAI API Client](#openai-api-client)
10. [Gemini API Client](#gemini-api-client)
11. [Public API Reference](#public-api-reference)
12. [Model Limits](#model-limits)
13. [Code Examples](#code-examples)
14. [Error Handling](#error-handling)
15. [Extension Guide](#extension-guide)

---

## Overview

The `forge-providers` crate handles all HTTP communication with LLM APIs. It provides a unified streaming interface that abstracts provider differences while preserving provider-specific features like Claude's extended thinking modes, OpenAI's reasoning controls, and Gemini's context caching.

### Key Responsibilities

| Responsibility | Description |
| :--- | :--- |
| **HTTP Communication** | Send requests to Claude, OpenAI, and Gemini APIs with retry |
| **SSE Parsing** | Parse Server-Sent Events streams from all three providers |
| **Request Building** | Construct provider-specific request payloads |
| **Event Normalization** | Convert provider-specific events to unified `StreamEvent` |
| **Automatic Retry** | Exponential backoff with jitter, `Retry-After` support, idempotency keys |
| **Context Caching** | Manage Gemini explicit context caches for large prompts |
| **Configuration Validation** | Ensure API keys match their intended providers |

### Crate Structure

```text
providers/
+-- Cargo.toml          # Crate manifest
+-- src/
    +-- lib.rs          # Provider implementations and dispatch
    |   +-- SSE parsing functions
    |   +-- ApiConfig struct
    |   +-- send_message() dispatch
    |   +-- pub mod claude
    |   +-- pub mod openai
    |   +-- pub mod gemini
    +-- retry.rs        # HTTP retry with exponential backoff (Stainless SDK compatible)
    +-- sse_types.rs    # Typed serde structs for Claude, OpenAI, Gemini SSE events
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
| `uuid` | Generate tool call IDs for Gemini and idempotency keys |
| `rand` | Jitter calculation for retry backoff |
| `anyhow` / `thiserror` | Error handling |
| `tracing` | Structured logging |

---

## Architecture

```text
+-----------------------------------------------------------------------------+
|                           Engine Layer (caller)                              |
|                                                                             |
|   +---------------------------------------------------------------------+   |
|   |                         send_message()                              |   |
|   |                                                                     |   |
|   |   ApiConfig ----------------------------------------------------->  |   |
|   |   CacheableMessage[] -------------------------------------------->  |   |
|   |   OutputLimits -------------------------------------------------->  |   |
|   |   system_prompt ------------------------------------------------->  |   |
|   |   tools --------------------------------------------------------->  |   |
|   |   gemini_cache -------------------------------------------------->  |   |
|   |   tx: mpsc::Sender<StreamEvent> --------------------------------->  |   |
|   +---------------------------------------------------------------------+   |
+-----------------------------------------------------------------------------+
                                      |
                                      | match config.provider()
                                      v
             +------------------------+------------------------+
             |                        |                        |
             v                        v                        v
+-------------------------+  +-------------------------+  +-------------------------+
|  claude::send_message   |  |  openai::send_message   |  |  gemini::send_message   |
|                         |  |                         |  |                         |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
| |build_request_body() | |  | |build_request_body() | |  | |build_request_body() | |
| |- System blocks      | |  | |- input items        | |  | |- system_instruction | |
| |- Messages array     | |  | |- instructions       | |  | |- contents array     | |
| |- Cache control      | |  | |- reasoning.effort   | |  | |- cachedContent ref  | |
| |- Thinking config    | |  | |- text.verbosity     | |  | |- thinkingConfig     | |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
|            |            |  |            |            |  |            |            |
|            v            |  |            v            |  |            v            |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
| | send_with_retry()   | |  | | send_with_retry()   | |  | | send_with_retry()   | |
| |   POST to API       | |  | |   POST to API       | |  | |   POST to API       | |
| | api.anthropic.com   | |  | | api.openai.com      | |  | | googleapis.com      | |
| | /v1/messages        | |  | | /v1/responses       | |  | | /v1beta/models/...  | |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
|            |            |  |            |            |  |            |            |
|            v            |  |            v            |  |            v            |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
| |process_sse_stream() | |  | |process_sse_stream() | |  | |process_sse_stream() | |
| |  + ClaudeParser     | |  | |  + OpenAIParser     | |  | |  + GeminiParser     | |
| +---------------------+ |  | +---------------------+ |  | +---------------------+ |
+-------------------------+  +-------------------------+  +-------------------------+
             |                        |                        |
             +------------------------+----------+-------------+
                                                 |
                                                 v
                                    +-------------------------+
                                    |      tx.send(event)     |
                                    |                         |
                                    |  StreamEvent::TextDelta |
                                    |  StreamEvent::ThinkingDelta |
                                    |  StreamEvent::ThinkingSignature |
                                    |  StreamEvent::ToolCallStart |
                                    |  StreamEvent::ToolCallDelta |
                                    |  StreamEvent::Usage     |
                                    |  StreamEvent::Done      |
                                    |  StreamEvent::Error     |
                                    +-------------------------+
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
    gemini_thinking_enabled: bool,
    anthropic_thinking_mode: &'static str,
    anthropic_thinking_effort: &'static str,
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
        Ok(Self {
            api_key,
            model,
            openai_options: Default::default(),
            gemini_thinking_enabled: false,
            anthropic_thinking_mode: "adaptive",
            anthropic_thinking_effort: "max",
        })
    }
}
```

This makes it impossible to create an `ApiConfig` with a Claude API key and an OpenAI model.

Default Anthropic thinking settings are `mode: "adaptive"` and `effort: "max"`.

### ApiKey - Provider-Scoped Keys

```rust
pub enum ApiKey {
    Claude(SecretString),
    OpenAI(SecretString),
    Gemini(SecretString),
}
```

The key's provider is encoded in its type, preventing the invalid state of using an API key with the wrong provider. Keys are constructed via opaque factory methods (`ApiKey::claude(...)`, `ApiKey::openai(...)`, `ApiKey::gemini(...)`) and the inner value is accessed via `expose_secret()`.

### ModelName - Provider-Scoped Models

```rust
pub struct ModelName {
    provider: Provider,
    name: Cow<'static, str>,
}
```

Validation rules by provider:

| Provider | Prefix Requirement | Example Valid |
| :--- | :--- | :--- |
| Claude | Must start with `claude-` | `claude-opus-4-6` |
| OpenAI | Must start with `gpt-5` | `gpt-5.2`, `gpt-5.2-pro` |
| Gemini | Must start with `gemini-` | `gemini-3-pro-preview` |

Models must also exist in the predefined catalog (`PredefinedModel`).

### OutputLimits - Token Budgets

```rust
pub enum OutputLimits {
    Standard { max_output_tokens: u32 },
    WithThinking { max_output_tokens: u32, thinking_budget: ThinkingBudget },
}

pub struct ThinkingBudget(u32);

pub enum ThinkingState {
    Disabled,
    Enabled(ThinkingBudget),
}
```

Construction enforces invariants:

- If thinking is enabled, `thinking_budget >= 1024`
- If thinking is enabled, `thinking_budget < max_output_tokens`

### CacheHint - Provider Caching

```rust
pub enum CacheHint {
    Default,   // No caching preference
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
| Premature EOF | Emits `StreamEvent::Error` if connection closes without completion signal |

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
    /// Text content delta
    TextDelta(String),
    /// Provider reasoning content delta (Claude extended thinking or OpenAI reasoning summaries)
    ThinkingDelta(String),
    /// Encrypted thinking signature for API replay (Claude extended thinking)
    ThinkingSignature(String),
    /// Tool call started - emitted when a tool_use content block begins
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: ThoughtSignatureState,
    },
    /// Tool call arguments delta - emitted as JSON arguments stream in
    ToolCallDelta { id: String, arguments: String },
    /// API-reported token usage (from message_start or message_delta events)
    Usage(ApiUsage),
    /// Stream completed
    Done,
    /// Error occurred
    Error(String),
}
```

`ThoughtSignatureState` is an enum distinguishing `Unsigned` tool calls from `Signed(ThoughtSignature)` tool calls (used by Gemini when thinking mode is active).

### ApiUsage - Token Consumption Tracking

```rust
pub struct ApiUsage {
    /// Total input tokens (includes cached tokens)
    pub input_tokens: u32,
    /// Input tokens read from cache (cache hits)
    pub cache_read_tokens: u32,
    /// Input tokens written to cache (cache misses that were cached)
    pub cache_creation_tokens: u32,
    /// Output tokens generated by the model
    pub output_tokens: u32,
}
```

Usage events are emitted during streaming to report token consumption. Claude emits separate events for input (at `message_start`) and output (at `message_delta`). OpenAI reports all usage at `response.completed`.

---

## Typed SSE Event Structures

The `sse_types` module (`providers/src/sse_types.rs`) provides strongly-typed serde structs for provider SSE responses. This replaces stringly-typed JSON key access with compile-time validated deserialization.

### Claude SSE Types (`sse_types::claude`)

| Type | Description |
| :--- | :--- |
| `Event` | Top-level event enum tagged by `type` field (MessageStart, MessageDelta, ContentBlockStart, ContentBlockDelta, ContentBlockStop, MessageStop, Ping, Unknown) |
| `InputUsage` | Input token breakdown: `input_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens` |
| `OutputUsage` | Output token count from `message_delta` |
| `ContentBlock` | Block types: Text, ToolUse, Thinking, Unknown |
| `Delta` | Delta types: TextDelta, ThinkingDelta, SignatureDelta, InputJsonDelta, Unknown |
| `StopReason` | EndTurn, MaxTokens, StopSequence, ToolUse, Compaction, Unknown |

### OpenAI SSE Types (`sse_types::openai`)

| Type | Description |
| :--- | :--- |
| `Event` | Top-level event enum with `response.*` variants (OutputItemAdded, OutputTextDelta, OutputTextDone, RefusalDelta, ReasoningSummaryDelta, ReasoningSummaryDone, ReasoningSummaryPartAdded, FunctionCallArgumentsDelta, FunctionCallArgumentsDone, Completed, Incomplete, Failed, Error, Unknown) |
| `OutputItem` | FunctionCall (with id, call_id, name, arguments) or Message |
| `Usage` | Token counts: `input_tokens`, `output_tokens`, `input_tokens_details` |
| `ResponseInfo` | Response metadata with usage, error, and incomplete details |

### Gemini SSE Types (`sse_types::gemini`)

| Type | Description |
| :--- | :--- |
| `Response` | Top-level response with candidates, error, and `usageMetadata` |
| `Candidate` | Content and finish reason |
| `Part` | Text or FunctionCall, with optional `thought` flag and `thoughtSignature` |
| `FinishReason` | Parsed enum: Stop, MaxTokens, Safety, Recitation, Language, Blocklist, ProhibitedContent, Spii, MalformedFunctionCall, MissingThoughtSignature, TooManyToolCalls, UnexpectedToolCall, Other, Unknown |
| `UsageMetadata` | Token counts: `promptTokenCount`, `candidatesTokenCount`, `totalTokenCount` |

All types use `#[serde(other)]` on Unknown variants for forward compatibility with new API event types.

---

## Retry Infrastructure

The `retry` module (`providers/src/retry.rs`) implements HTTP retry behavior matching official Anthropic/OpenAI SDKs (Stainless-generated).

### Retry Policy

| Parameter | Default | Description |
| :--- | :--- | :--- |
| `max_retries` | 2 | Maximum retries (3 total attempts) |
| `initial_delay` | 500ms | Backoff before first retry |
| `max_delay` | 8s | Maximum backoff cap |
| `jitter_factor` | 0.25 | Down-jitter: multiplier in [0.75, 1.0] |

### Retryable Conditions

- HTTP 408, 409, 429, 5xx (500-599)
- Connection/timeout errors
- `x-should-retry: true` header forces retry on any status
- `x-should-retry: false` header forbids retry on any status
- `Retry-After` / `Retry-After-Ms` headers respected (capped at 60s)

### Retry Headers

Each request includes:

| Header | Description |
| :--- | :--- |
| `X-Stainless-Retry-Count` | 0 for initial request, incremented per retry |
| `Idempotency-Key` | `stainless-retry-{uuid}`, consistent across all attempts |
| `X-Stainless-Timeout` | Request timeout in seconds (non-streaming only) |

### RetryOutcome

```rust
pub enum RetryOutcome {
    Success(Response),
    HttpError(Response),
    ConnectionError { attempts: u32, source: reqwest::Error },
    NonRetryable(reqwest::Error),
}
```

All providers use `send_with_retry` to wrap their HTTP requests. After retry resolution, `handle_response` converts the outcome into either an `ApiResponse::Success` (for SSE stream processing) or sends a `StreamEvent::Error` and returns `ApiResponse::StreamTerminated`.

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

An `anthropic-beta` header is conditionally included:

| Model | Beta Header |
| :--- | :--- |
| Opus 4.6 (`claude-opus-4-6*`) | `context-1m-2025-08-07` |
| Other models with thinking | `interleaved-thinking-2025-05-14,context-management-2025-06-27` |
| Other models without thinking | None |

### Request Structure

For Opus 4.6 with adaptive thinking:

```json
{
  "model": "claude-opus-4-6",
  "max_tokens": 4096,
  "stream": true,
  "system": [
    { "type": "text", "text": "System prompt", "cache_control": { "type": "ephemeral" } }
  ],
  "messages": [
    { "role": "user", "content": [{ "type": "text", "text": "Hello" }] },
    { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] }
  ],
  "tools": [
    { "name": "read_file", "description": "...", "input_schema": {...} }
  ],
  "thinking": { "type": "adaptive" },
  "output_config": { "effort": "max" }
}
```

### Opus 4.6 Thinking Modes

Opus 4.6 models use a configurable thinking system controlled via `ApiConfig`:

| Thinking Mode | `thinking` Field | `output_config` | Notes |
| :--- | :--- | :--- | :--- |
| `"adaptive"` (default) | `{ "type": "adaptive" }` | `{ "effort": "..." }` | Server decides when to think |
| `"enabled"` | `{ "type": "enabled", "budget_tokens": N }` | `{ "effort": "..." }` | Always think, with explicit budget from `OutputLimits` |
| `"disabled"` | `{ "type": "disabled" }` | None | No thinking |

Effort levels: `"low"`, `"medium"`, `"high"`, `"max"` (default: `"max"`).

Server-side compaction (`compact-2026-01-12`) is intentionally NOT enabled for Opus 4.6. Forge uses its own client-side distillation and does not reconcile after server compaction.

### Legacy Thinking Mode (Pre-4.6 Models)

Non-Opus-4.6 models with thinking enabled use the classic format:

```json
{
  "thinking": { "type": "enabled", "budget_tokens": 4096 },
  "context_management": {
    "edits": [{ "type": "clear_thinking_20251015", "keep": "all" }]
  }
}
```

The `context_management` block preserves all thinking blocks for cache efficiency, which is essential for Haiku 4.5 where thinking blocks would otherwise be stripped by default.

### Message Transformations

| Input | API Format |
| :--- | :--- |
| System prompt (parameter) | First system block with `cache_control: ephemeral` |
| `Message::System` | Additional system blocks |
| `Message::User` | User message with content blocks |
| `Message::Assistant` | Assistant text content block (grouped with adjacent ToolUse) |
| `Message::ToolUse` | Assistant `tool_use` content block (grouped with adjacent Assistant) |
| `Message::ToolResult` | User message with `tool_result` content block |
| `Message::Thinking` | `redacted_thinking` block with signature (if signed), otherwise skipped |

**Assistant message grouping**: Consecutive `Message::Assistant` and `Message::ToolUse` messages are grouped into a single assistant message with multiple content blocks. If any message in the group has `CacheHint::Ephemeral`, `cache_control` is placed on the last content block.

**Opus 4.6 trailing assistant prefill**: Trailing assistant messages are automatically dropped for Opus 4.6, as Anthropic no longer accepts assistant-prefilled final turns on this model.

### Response Parsing

| Event Type | Action |
| :--- | :--- |
| `message_start` | Emit `Usage` with input token counts |
| `content_block_start` with `tool_use` | Emit `ToolCallStart` |
| `content_block_delta` with `text_delta` | Emit `TextDelta` |
| `content_block_delta` with `thinking_delta` | Emit `ThinkingDelta` |
| `content_block_delta` with `signature_delta` | Emit `ThinkingSignature` |
| `content_block_delta` with `input_json_delta` | Emit `ToolCallDelta` |
| `content_block_stop` | Reset current tool ID |
| `message_delta` | Emit `Usage` with output token count |
| `message_delta` with `stop_reason: "compaction"` | Set compaction flag (do not end stream) |
| `message_stop` | Emit `Done` (unless compacting, then continue) |

**Server-side compaction handling**: If the API signals `stop_reason: "compaction"` in a `message_delta`, the parser sets a compaction flag. The subsequent `message_stop` does NOT end the stream; instead, the API sends a new `message_start` with the compacted context and continues streaming.

**Claude Usage Calculation**: Anthropic reports `input_tokens` as non-cached tokens only. Total input = `input_tokens` + `cache_read_input_tokens` + `cache_creation_input_tokens`.

---

## OpenAI API Client

### OpenAI Endpoint and Authentication

```rust
const API_URL: &str = "https://api.openai.com/v1/responses";

client.post(API_URL)
    .header("Authorization", format!("Bearer {}", config.api_key()))
    .header("content-type", "application/json")
```

Note: This uses the OpenAI Responses API (not Chat Completions) for GPT-5.x support.

### OpenAI Request Structure

```json
{
  "model": "gpt-5.2",
  "input": [
    { "role": "developer", "content": "Context Distillate" },
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
| `Message::Thinking` | (skipped) | Thinking is not sent back to the API |
| `Message::ToolUse` | `function_call` item | Tool invocation record |
| `Message::ToolResult` | `function_call_output` item | Tool result record |

### GPT-5 Options

For models starting with `gpt-5`, additional parameters are included:

```rust
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,   // none, low, medium, high, xhigh
    reasoning_summary: OpenAIReasoningSummary, // disabled, auto, concise, detailed
    verbosity: OpenAITextVerbosity,            // low, medium, high
    truncation: OpenAITruncation,              // auto, disabled
}
```

When `reasoning_summary` is not `Disabled`, it is included in the request:

```json
{
  "reasoning": {
    "effort": "high",
    "summary": "auto"
  }
}
```

Reasoning summaries are streamed via `response.reasoning_summary_text.delta` events and emitted as `ThinkingDelta` events.

### OpenAI Response Parsing

| Event Type | Action |
| :--- | :--- |
| `response.output_item.added` with `function_call` | Emit `ToolCallStart` (and initial args if present) |
| `response.output_text.delta` | Emit `TextDelta` |
| `response.output_text.done` | Emit `TextDelta` (fallback if no prior deltas) |
| `response.refusal.delta` | Emit `TextDelta` (model refused) |
| `response.reasoning_summary_text.delta` | Emit `ThinkingDelta` |
| `response.reasoning_summary_text.done` | Emit `ThinkingDelta` (fallback if no prior deltas) |
| `response.reasoning_summary_part.added` | Emit `ThinkingDelta` (with newline insertion between parts) |
| `response.function_call_arguments.delta` | Emit `ToolCallDelta` |
| `response.function_call_arguments.done` | Emit `ToolCallDelta` (if no prior deltas) |
| `response.completed` | Emit `Usage` (if usage present) then `Done` |
| `response.incomplete` | Emit `Error` with reason |
| `response.failed` / `error` | Emit `Error` with message |

The parser maintains state to map `item_id` to `call_id` and tracks which calls have received deltas to avoid duplicate emissions.

---

## Gemini API Client

### Gemini Endpoint and Authentication

```rust
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

let url = format!("{API_BASE}/models/{model}:streamGenerateContent?alt=sse");

client.post(&url)
    .header("x-goog-api-key", config.api_key())
    .header("content-type", "application/json")
```

### Gemini Request Structure

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
- `Message::System` is mapped to a `user` role content entry (the top-level `system_instruction` is reserved for the main system prompt parameter)
- `Message::Thinking` is skipped (not sent back to the API)

### Thinking Mode

Gemini thinking is enabled via `ApiConfig.with_gemini_thinking_enabled(true)`. When enabled, the request includes:

```json
{
  "generationConfig": {
    "thinkingConfig": {
      "thinkingLevel": "high",
      "includeThoughts": true
    }
  }
}
```

Thinking content is streamed via `ThinkingDelta` events when parts have `"thought": true`.

### Thought Signatures

Gemini requires `thoughtSignature` on tool calls when thinking mode was used. This is preserved from `ToolCall.thought_signature` (as `ThoughtSignatureState::Signed`). Tool calls without a signature use `ThoughtSignatureState::Unsigned` and no `thoughtSignature` field is emitted.

### Schema Sanitization

The `additionalProperties` field is recursively removed from tool parameter schemas, as Gemini doesn't support it. This applies both to inline tool definitions and to tools included in cached content.

### Context Caching

Gemini supports explicit context caching for large system prompts and tools:

```rust
pub struct GeminiCache {
    pub name: String,                    // "cachedContents/abc123"
    pub expire_time: DateTime<Utc>,
    pub system_prompt_hash: u64,
    pub tools_hash: u64,
}

pub struct GeminiCacheConfig {
    pub enabled: bool,
    pub ttl_seconds: u32,     // default: 3600 (1 hour)
}

pub async fn create_cache(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    tools: Option<&[ToolDefinition]>,
    ttl_seconds: u32,
) -> Result<GeminiCache>
```

**Minimum token requirements**:

- Gemini 3 Pro: 4,096 tokens (~16,384 characters)
- Gemini Flash models: 1,024 tokens (~4,096 characters)

**Important**: When using cached content, `system_instruction`, `tools`, and `tool_config` must be part of the cache -- they cannot be specified in GenerateContent. The cache is invalidated if either the system prompt or tool definitions change.

### Gemini Response Parsing

| Condition | Action |
| :--- | :--- |
| `candidates[].content.parts[].text` | Emit `TextDelta` |
| `candidates[].content.parts[].thought == true` | Emit `ThinkingDelta` |
| `candidates[].content.parts[].functionCall` | Emit `ToolCallStart` + `ToolCallDelta` |
| `candidates[].content.parts[].thoughtSignature` | Emit `ToolCallStart` with `ThoughtSignatureState::Signed` |
| `finishReason` is `STOP` or `MAX_TOKENS` | Emit `Done` |
| `finishReason` is `SAFETY`, `RECITATION`, etc. | Emit `Error` |
| `error` field present | Emit `Error` |

Gemini doesn't provide tool call IDs, so the parser generates UUIDs: `call_{uuid}`.

Content parts are processed before checking `finishReason`, ensuring final content is not dropped when both appear in the same SSE chunk.

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
| `with_gemini_thinking_enabled(enabled)` | `Self` | Builder for Gemini thinking mode |
| `with_anthropic_thinking(mode, effort)` | `Self` | Builder for Claude thinking mode and effort |
| `provider()` | `Provider` | Get the provider |
| `api_key()` | `&str` | Get the API key string (via `expose_secret()`) |
| `api_key_owned()` | `ApiKey` | Get a clone of the API key |
| `model()` | `&ModelName` | Get the model name |
| `openai_options()` | `OpenAIRequestOptions` | Get OpenAI options (copy) |
| `gemini_thinking_enabled()` | `bool` | Check if Gemini thinking is enabled |
| `anthropic_thinking_mode()` | `&str` | Get Claude thinking mode ("adaptive", "enabled", "disabled") |
| `anthropic_thinking_effort()` | `&str` | Get Claude thinking effort ("low", "medium", "high", "max") |

### HTTP Client Functions

```rust
/// Shared HTTP client for streaming requests (no total timeout).
/// Configured with TCP keepalive, connection pool, HTTPS-only,
/// platform headers (X-Stainless-*), and redirect disabled.
pub fn http_client() -> &'static reqwest::Client

/// HTTP client with timeout for synchronous operations
pub fn http_client_with_timeout(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error>

/// Read error body with 32 KiB limit
pub async fn read_capped_error_body(response: reqwest::Response) -> String
```

### Re-exports

```rust
pub use forge_types;
pub mod retry;
pub mod sse_types;
```

---

## Model Limits

Token limits are defined in `forge-context/src/model_limits.rs`:

| Model | Context Window | Max Output |
| :--- | :--- | :--- |
| `claude-opus-4-6` | 1,000,000 | 128,000 |
| `claude-haiku-4-5-20251001` | 200,000 | 64,000 |
| `gpt-5.2-pro` | 400,000 | 128,000 |
| `gpt-5.2` | 400,000 | 128,000 |
| `gemini-3-pro-preview` | 1,048,576 | 65,536 |
| `gemini-3-flash-preview` | 1,048,576 | 65,536 |

---

## Code Examples

### Basic Streaming Request

```rust
use forge_providers::{ApiConfig, send_message, forge_types::*};
use tokio::sync::mpsc;

async fn chat_with_claude() -> anyhow::Result<()> {
    let api_key = ApiKey::claude(std::env::var("ANTHROPIC_API_KEY")?);
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
let api_key = ApiKey::openai(std::env::var("OPENAI_API_KEY")?);
let model = Provider::OpenAI.parse_model("gpt-5.2")?;

let options = OpenAIRequestOptions::new(
    OpenAIReasoningEffort::High,
    OpenAIReasoningSummary::Auto,
    OpenAITextVerbosity::Medium,
    OpenAITruncation::Auto,
);

let config = ApiConfig::new(api_key, model)?
    .with_openai_options(options);
```

### Claude with Anthropic Thinking Configuration

```rust
let config = ApiConfig::new(
    ApiKey::claude(std::env::var("ANTHROPIC_API_KEY")?),
    Provider::Claude.parse_model("claude-opus-4-6")?,
)?
// Use "enabled" mode with explicit budget (instead of default "adaptive")
.with_anthropic_thinking("enabled", "high");

// When mode is "enabled", thinking budget comes from OutputLimits
let limits = OutputLimits::with_thinking(16384, 4096)?;

let (tx, mut rx) = mpsc::channel(32);

tokio::spawn(async move {
    send_message(&config, &messages, limits, None, None, None, tx).await
});

while let Some(event) = rx.recv().await {
    match event {
        StreamEvent::ThinkingDelta(thought) => {
            // Internal reasoning (typically hidden in UI)
            eprint!("[thinking] {}", thought);
        }
        StreamEvent::ThinkingSignature(sig) => {
            // Encrypted signature for replaying thinking blocks
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
// Tools must be included in cache (Gemini API requirement)
let cache = create_cache(
    &api_key,
    "gemini-3-pro-preview",
    &large_system_prompt,
    Some(&tools),  // Tools are cached with the prompt
    3600,          // TTL in seconds
).await?;

// Use cache in subsequent requests
// Note: tools are already in the cache, so they're not sent again
send_message(
    &config,
    &messages,
    limits,
    None,  // System prompt is in cache
    tools, // Passed for reference but not sent when cache is used
    Some(&cache),
    tx,
).await?;

// Check cache validity (includes both prompt and tools)
if cache.is_expired() || !cache.matches_config(&system_prompt, Some(&tools)) {
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
// Errors during streaming -> delivered via channel
let _ = tx.send(StreamEvent::Error(error_message)).await;
return Ok(()); // Function succeeds, error communicated via channel

// Only return Err for unrecoverable failures
let chunk = chunk?; // Network errors propagate
```

This allows partial responses to be captured before an error occurs.

### Retry Error Handling

After exhausting retries, the `handle_response` function converts `RetryOutcome` variants to `StreamEvent::Error` messages:

- `ConnectionError { attempts, source }` -> `"Request failed after N attempts: ..."`
- `NonRetryable(error)` -> `"Request failed: ..."`
- `HttpError(response)` -> `"API error {status}: {body}"` (body capped at 32 KiB)

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
   - Add opaque constructor and update `provider()` / `expose_secret()` methods

3. **Add model limits** (in `forge-context/src/model_limits.rs`):
   - Add entry to `KNOWN_MODELS` array

4. **Add typed SSE events** (in `providers/src/sse_types.rs`):
   - Add a new provider module with serde-tagged event types
   - Use `#[serde(other)]` on Unknown variants for forward compatibility

5. **Add provider module** (in `providers/src/lib.rs`):

```rust
pub mod your_provider {
    use super::*;

    const API_URL: &str = "https://api.yourprovider.com/v1/chat";

    #[derive(Default)]
    struct YourProviderParser;

    impl SseParser for YourProviderParser {
        fn parse(&mut self, json: &serde_json::Value) -> SseParseAction {
            // Parse provider-specific JSON events using typed SSE structs
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
        let client = http_client();
        let retry_config = RetryConfig::default();
        let body = build_request_body(...);
        let body_json = body.clone();

        let outcome = send_with_retry(
            || {
                client
                    .post(API_URL)
                    .header("Authorization", format!("Bearer {}", config.api_key()))
                    .json(&body_json)
            },
            None, // No timeout for streaming
            &retry_config,
        ).await;

        let response = match handle_response(outcome, &tx).await? {
            ApiResponse::Success(resp) => resp,
            ApiResponse::StreamTerminated => return Ok(()),
        };

        let mut parser = YourProviderParser;
        process_sse_stream(response, &mut parser, &tx).await
    }
}
```

1. **Update send_message dispatch**:

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
| `TCP_KEEPALIVE_SECS` | 60 | TCP keepalive idle time |
| `POOL_MAX_IDLE_PER_HOST` | 100 | Connection pool max idle per host |
| `POOL_IDLE_TIMEOUT_SECS` | 90 | Connection pool idle timeout |
| `DEFAULT_STREAM_IDLE_TIMEOUT_SECS` | 60 | SSE idle timeout |
| `MAX_SSE_BUFFER_BYTES` | 4 MiB | Buffer size limit |
| `MAX_SSE_PARSE_ERRORS` | 3 | Consecutive parse error threshold |
| `MAX_SSE_PARSE_ERROR_PREVIEW` | 160 | Chars logged on parse error |
| `MAX_ERROR_BODY_BYTES` | 32 KiB | Error response size limit |

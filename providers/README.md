# forge-providers

LLM provider clients with streaming support for Claude (Anthropic) and OpenAI APIs.

## Overview

This crate provides a unified interface for streaming chat completions from multiple LLM providers. It handles:

- **HTTP communication** with provider APIs using `reqwest`
- **Server-Sent Events (SSE)** streaming for real-time response delivery
- **Provider-specific request formatting** (message structure, caching, thinking modes)
- **Type-safe configuration** that prevents provider/key/model mismatches at compile time

The crate is designed as a thin, focused layer that handles only network I/O and protocol translation. Business logic, context management, and UI concerns live in other crates.

## Public API

### Core Function

```rust
pub async fn send_message(
    config: &ApiConfig,
    messages: &[CacheableMessage],
    limits: OutputLimits,
    system_prompt: Option<&str>,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()>
```

The primary entry point for sending chat requests. This function:

1. Routes to the appropriate provider implementation based on `config.provider()`
2. Serializes messages into provider-specific JSON format
3. Initiates an HTTP POST request with streaming enabled
4. Parses the SSE stream and invokes `on_event` for each chunk

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `config` | `&ApiConfig` | API key, model name, and provider-specific options |
| `messages` | `&[CacheableMessage]` | Conversation history with optional cache hints |
| `limits` | `OutputLimits` | Maximum output tokens and optional thinking budget |
| `system_prompt` | `Option<&str>` | System instructions injected before conversation |
| `on_event` | `impl Fn(StreamEvent)` | Callback invoked for each streaming event |

**Returns:** `Result<()>` - succeeds when stream completes; errors indicate network/parsing failures.

### ApiConfig

Configuration container that enforces provider consistency.

```rust
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
    openai_options: OpenAIRequestOptions,
}
```

**Construction:**

```rust
impl ApiConfig {
    /// Creates a new config, returning an error if the key and model
    /// belong to different providers.
    pub fn new(api_key: ApiKey, model: ModelName) -> Result<Self, ApiConfigError>;
    
    /// Builder method for OpenAI-specific options.
    pub fn with_openai_options(self, options: OpenAIRequestOptions) -> Self;
}
```

**Accessors:**

| Method | Return Type | Description |
|--------|-------------|-------------|
| `provider()` | `Provider` | The provider (Claude or OpenAI) |
| `api_key()` | `&str` | Raw API key string |
| `api_key_owned()` | `ApiKey` | Cloned provider-scoped key |
| `model()` | `&ModelName` | The model name with provider scope |
| `openai_options()` | `OpenAIRequestOptions` | OpenAI-specific request settings |

### ApiConfigError

```rust
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}
```

Returned when attempting to create an `ApiConfig` with mismatched provider types (e.g., Claude API key with an OpenAI model).

### Provider Modules

#### `claude` Module

Handles communication with the Anthropic Messages API.

```rust
pub mod claude {
    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()>;
}
```

**API Endpoint:** `https://api.anthropic.com/v1/messages`

**Features:**
- Ephemeral cache control markers on user messages
- Extended thinking support via `thinking.budget_tokens`
- System prompts as separate `system` array (not in messages)

#### `openai` Module

Handles communication with the OpenAI Responses API.

```rust
pub mod openai {
    pub async fn send_message(
        config: &ApiConfig,
        messages: &[CacheableMessage],
        limits: OutputLimits,
        system_prompt: Option<&str>,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()>;
}
```

**API Endpoint:** `https://api.openai.com/v1/responses`

**Features:**
- Reasoning effort control for GPT-5 models (`none`, `low`, `medium`, `high`, `xhigh`)
- Text verbosity settings (`low`, `medium`, `high`)
- Automatic truncation configuration
- System prompts via `instructions` field

## Re-exported Types

The crate re-exports `forge_types` for convenience:

```rust
pub use forge_types;
```

This provides access to all domain types without requiring a separate dependency:

| Type | Purpose |
|------|---------|
| `Provider` | Enum: `Claude` or `OpenAI` |
| `ApiKey` | Provider-scoped API key (`ApiKey::Claude(String)` or `ApiKey::OpenAI(String)`) |
| `ModelName` | Provider-scoped model identifier with known/unverified distinction |
| `Message` | Sum type for `System`, `User`, or `Assistant` messages |
| `CacheableMessage` | Message paired with a `CacheHint` |
| `CacheHint` | `None` or `Ephemeral` - controls provider caching behavior |
| `OutputLimits` | Token limits with optional thinking budget (validated at construction) |
| `StreamEvent` | Streaming events: `TextDelta`, `ThinkingDelta`, `Done`, `Error` |
| `OpenAIRequestOptions` | Reasoning effort, verbosity, and truncation settings |

## Architecture

### Request Flow

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│  ApiConfig  │────▶│ send_message │────▶│ Provider Match  │
└─────────────┘     └──────────────┘     └────────┬────────┘
                                                  │
                    ┌─────────────────────────────┼─────────────────────────────┐
                    │                             │                             │
                    ▼                             ▼                             │
           ┌────────────────┐           ┌─────────────────┐                     │
           │ claude::send_  │           │ openai::send_   │                     │
           │    message     │           │    message      │                     │
           └───────┬────────┘           └────────┬────────┘                     │
                   │                             │                              │
                   ▼                             ▼                              │
           ┌────────────────┐           ┌─────────────────┐                     │
           │ Build Anthropic│           │ Build OpenAI    │                     │
           │ JSON Payload   │           │ JSON Payload    │                     │
           └───────┬────────┘           └────────┬────────┘                     │
                   │                             │                              │
                   ▼                             ▼                              │
           ┌────────────────┐           ┌─────────────────┐                     │
           │ POST + Stream  │           │ POST + Stream   │                     │
           │ SSE Response   │           │ SSE Response    │                     │
           └───────┬────────┘           └────────┬────────┘                     │
                   │                             │                              │
                   └─────────────┬───────────────┘                              │
                                 │                                              │
                                 ▼                                              │
                    ┌────────────────────────┐                                  │
                    │ Parse SSE Events       │                                  │
                    │ Call on_event callback │                                  │
                    └────────────────────────┘                                  │
```

### SSE Stream Processing

Both providers use a common pattern for SSE parsing:

1. Accumulate bytes into a buffer
2. Split on `\n\n` (SSE event delimiter)
3. Extract `data:` lines from each event
4. Parse JSON and map to `StreamEvent` variants
5. Invoke callback immediately (no buffering)

**Claude SSE Events:**
- `content_block_delta` with `delta.type = "text_delta"` → `StreamEvent::TextDelta`
- `content_block_delta` with `delta.type = "thinking_delta"` → `StreamEvent::ThinkingDelta`
- `message_stop` or `[DONE]` → `StreamEvent::Done`

**OpenAI SSE Events:**
- `response.output_text.delta` → `StreamEvent::TextDelta`
- `response.refusal.delta` → `StreamEvent::TextDelta` (refusals treated as text)
- `response.completed` → `StreamEvent::Done`
- `response.incomplete` / `response.failed` / `error` → `StreamEvent::Error`

### Type-Driven Safety

The crate leverages types from `forge-types` to prevent invalid states:

1. **Provider/Key Matching:** `ApiKey::Claude` can only be used with Claude models. `ApiConfig::new()` validates this at construction time.

2. **Output Limits Validation:** `OutputLimits::with_thinking()` enforces that thinking budget is at least 1024 tokens and less than max output tokens.

3. **Model Scoping:** `ModelName` carries its provider, preventing accidental cross-provider model usage.

## Usage Examples

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

### OpenAI with Reasoning Settings

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
        |event| { /* handle events */ },
    ).await
}
```

### Claude Extended Thinking

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

### Caching for Long Conversations

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

## Error Handling

The crate uses `anyhow::Result` for most operations, with specific error types where meaningful:

| Error Type | Cause |
|------------|-------|
| `ApiConfigError::ProviderMismatch` | API key and model belong to different providers |
| HTTP errors (via `reqwest`) | Network failures, timeouts |
| `StreamEvent::Error` | API errors (rate limits, invalid requests, etc.) |

API errors are delivered through the `StreamEvent::Error` variant rather than as `Result::Err`, allowing partial responses to be captured before an error occurs.

## Dependencies

| Crate | Purpose |
|-------|---------|
| `forge-types` | Domain types (Provider, Message, StreamEvent, etc.) |
| `reqwest` | HTTP client with streaming support |
| `futures-util` | `StreamExt` for async byte stream iteration |
| `serde` / `serde_json` | JSON serialization for API payloads |
| `anyhow` | Error handling |
| `thiserror` | Custom error type derivation |
| `tracing` | Structured logging (warnings for unexpected messages) |

## Thread Safety

- `ApiConfig` is `Clone + Send + Sync`
- The `on_event` callback must be `Send + 'static` for cross-task delivery
- No internal state is shared between calls; each `send_message` creates its own HTTP client

## Testing

```bash
cargo test -p forge-providers
```

Tests verify:
- `ApiConfig` rejects mismatched provider/key combinations
- `ApiConfig` accepts matching provider/key combinations

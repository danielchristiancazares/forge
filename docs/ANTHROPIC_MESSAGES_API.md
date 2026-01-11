# Anthropic Messages API Reference

This document provides a reference for the Anthropic Messages API as used by Forge. It covers the endpoint structure, request/response formats, SSE streaming events, prompt caching, and extended thinking.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-82 | Overview and Request Structure: endpoint, auth, required/optional fields, message format |
| 83-180 | Tool Schema: custom tools, built-in tools (bash, text editor versions) |
| 181-250 | Tool Choice: auto, any, specific tool, disabling tools |
| 251-320 | Tool Use Response Blocks: tool_use content block format, handling |
| 321-400 | SSE Streaming: event types (message_start, content_block_delta, message_stop) |
| 401-450 | Prompt Caching: cache_control blocks, TTL, limitations |
| 451-486 | Extended Thinking: configuration, thinking blocks, budget constraints |

## Overview

| Aspect | Details |
|--------|---------|
| Base URL | `https://api.anthropic.com/v1/messages` |
| Auth Header | `x-api-key: $ANTHROPIC_API_KEY` |
| Version Header | `anthropic-version: 2023-06-01` |
| Content Type | `application/json` |

## Request Structure

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `model` | string | Model ID (e.g., `claude-sonnet-4-5-20250929`, `claude-opus-4-5-20251101`) |
| `max_tokens` | number | Maximum tokens to generate |
| `messages` | array | Conversation messages with alternating `user`/`assistant` roles |

### Optional Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `system` | string/array | - | System prompt (string or array of content blocks) |
| `stream` | boolean | false | Enable SSE streaming |
| `temperature` | number | 1.0 | Randomness (0.0-1.0) |
| `top_p` | number | - | Nucleus sampling |
| `top_k` | number | - | Top-K sampling |
| `stop_sequences` | array | - | Custom stop sequences |
| `thinking` | object | - | Extended thinking configuration |
| `tools` | array | - | Tool definitions (see [Tool Schema](#tool-schema)) |
| `tool_choice` | object | - | Control tool usage (see [Tool Choice](#tool-choice)) |

### Message Format

Messages alternate between `user` and `assistant` roles:

```json
{
  "messages": [
    {"role": "user", "content": "Hello"},
    {"role": "assistant", "content": "Hi there!"},
    {"role": "user", "content": "How are you?"}
  ]
}
```

Content can be a string or array of content blocks:

```json
{
  "role": "user",
  "content": [
    {
      "type": "text",
      "text": "Analyze this:",
      "cache_control": {"type": "ephemeral"}
    }
  ]
}
```

### System Prompt

System prompts can be strings or arrays of content blocks with cache control:

```json
{
  "system": [
    {
      "type": "text",
      "text": "You are a helpful assistant.",
      "cache_control": {"type": "ephemeral"}
    }
  ]
}
```

## Tool Schema

Tools define functions the model can call. The `tools` array accepts custom tools and built-in Anthropic tools.

### Custom Tools

```json
{
  "type": "custom",
  "name": "get_weather",
  "description": "Get current weather for a location. Be detailed - more info helps the model.",
  "input_schema": {
    "type": "object",
    "properties": {
      "location": {
        "type": "string",
        "description": "City and state, e.g., San Francisco, CA"
      }
    },
    "required": ["location"]
  },
  "cache_control": {"type": "ephemeral", "ttl": "5m"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `"custom"` | optional | Explicit type (optional for custom tools) |
| `name` | string | required | Tool name (1-128 chars), used in `tool_use` blocks |
| `description` | string | optional | Detailed description - more info = better performance |
| `input_schema` | object | required | [JSON Schema](https://json-schema.org/draft/2020-12) for tool input |
| `cache_control` | object | optional | Cache breakpoint with `type: "ephemeral"` and optional `ttl` |

### Built-in Tools

Anthropic provides built-in tools with fixed names and versioned types:

#### Bash Tool
```json
{
  "type": "bash_20250124",
  "name": "bash",
  "cache_control": {"type": "ephemeral"}
}
```

#### Text Editor Tools
```json
// Version 1 (January 2025)
{
  "type": "text_editor_20250124",
  "name": "str_replace_editor"
}

// Version 2 (April 2025)
{
  "type": "text_editor_20250429",
  "name": "str_replace_based_edit_tool"
}

// Version 3 (July 2025) - adds max_characters
{
  "type": "text_editor_20250728",
  "name": "str_replace_based_edit_tool",
  "max_characters": 10000
}
```

#### Web Search Tool
```json
{
  "type": "web_search_20250305",
  "name": "web_search",
  "allowed_domains": ["example.com"],
  "blocked_domains": ["blocked.com"],
  "max_uses": 5,
  "user_location": {
    "type": "approximate",
    "country": "US",
    "region": "CA",
    "city": "San Francisco",
    "timezone": "America/Los_Angeles"
  }
}
```

| Tool Type | Name | Key Options |
|-----------|------|-------------|
| `bash_20250124` | `"bash"` | - |
| `text_editor_20250124` | `"str_replace_editor"` | - |
| `text_editor_20250429` | `"str_replace_based_edit_tool"` | - |
| `text_editor_20250728` | `"str_replace_based_edit_tool"` | `max_characters` |
| `web_search_20250305` | `"web_search"` | `allowed_domains`, `blocked_domains`, `max_uses`, `user_location` |

### Tool Choice

Control how the model uses tools:

| Type | Description |
|------|-------------|
| `{"type": "auto"}` | Model decides whether to use tools (default) |
| `{"type": "any"}` | Model must use at least one tool |
| `{"type": "tool", "name": "..."}` | Model must use the specified tool |
| `{"type": "none"}` | Model cannot use tools |

All types support `disable_parallel_tool_use: true` to prevent multiple simultaneous tool calls.

## Response Structure

### Non-Streaming Response

```json
{
  "type": "message",
  "id": "msg_...",
  "model": "claude-sonnet-4-5-20250929",
  "role": "assistant",
  "content": [
    {
      "type": "text",
      "text": "Response text here"
    }
  ],
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {
    "input_tokens": 100,
    "output_tokens": 50,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": 0
  }
}
```

### Stop Reasons

| Reason | Description |
|--------|-------------|
| `end_turn` | Natural completion |
| `max_tokens` | Hit token limit |
| `stop_sequence` | Hit custom stop sequence |
| `tool_use` | Model wants to use a tool |

## SSE Streaming

Enable with `"stream": true`. Events are sent as Server-Sent Events.

### Event Flow

```
1. message_start       → Message object with empty content
2. content_block_start → Start of content block (index 0, 1, ...)
3. content_block_delta → Incremental content (repeated)
4. content_block_stop  → End of content block
5. message_delta       → Final message updates (stop_reason, usage)
6. message_stop        → Stream complete
```

### Event Types

#### message_start
```json
event: message_start
data: {
  "type": "message_start",
  "message": {
    "id": "msg_...",
    "type": "message",
    "role": "assistant",
    "content": [],
    "model": "claude-sonnet-4-5-20250929",
    "stop_reason": null,
    "usage": {"input_tokens": 25, "output_tokens": 1}
  }
}
```

#### content_block_start
```json
event: content_block_start
data: {
  "type": "content_block_start",
  "index": 0,
  "content_block": {"type": "text", "text": ""}
}
```

#### content_block_delta (text)
```json
event: content_block_delta
data: {
  "type": "content_block_delta",
  "index": 0,
  "delta": {"type": "text_delta", "text": "Hello"}
}
```

#### content_block_delta (thinking)
```json
event: content_block_delta
data: {
  "type": "content_block_delta",
  "index": 0,
  "delta": {"type": "thinking_delta", "thinking": "Let me analyze..."}
}
```

#### content_block_stop
```json
event: content_block_stop
data: {"type": "content_block_stop", "index": 0}
```

#### message_delta
```json
event: message_delta
data: {
  "type": "message_delta",
  "delta": {"stop_reason": "end_turn"},
  "usage": {"output_tokens": 15}
}
```

#### message_stop
```json
event: message_stop
data: {"type": "message_stop"}
```

#### ping
```json
event: ping
data: {"type": "ping"}
```

#### error
```json
event: error
data: {"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}
```

### Forge SSE Parsing

Forge parses SSE events in `providers/src/lib.rs`:

```rust
// Shared SSE utilities
fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)>
fn drain_next_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>>
fn extract_sse_data(event: &str) -> Option<String>
```

Events are mapped to `StreamEvent`:
- `content_block_delta` with `text_delta` → `StreamEvent::TextDelta`
- `content_block_delta` with `thinking_delta` → `StreamEvent::ThinkingDelta`
- `message_stop` → `StreamEvent::Done`
- Errors → `StreamEvent::Error`

## Prompt Caching

Prompt caching reduces processing time and costs for repetitive content.

### What "ephemeral" Means

The `type: "ephemeral"` value indicates a **temporary, auto-expiring cache**:

- **"Ephemeral"** = short-lived, not persistent
- Cache entries expire after their TTL (5 minutes or 1 hour)
- Currently the **only supported cache type** (future types like "persistent" may be added)
- The cache is refreshed (TTL reset) each time it's read, at no additional cost

This is NOT about data privacy or content being "ephemeral" in the sense of not being stored - it's purely about cache lifetime. The content is cached server-side for the TTL duration to avoid reprocessing on subsequent requests.

### Cache Control

Add `cache_control` to content blocks:

```json
{
  "type": "text",
  "text": "Large context here...",
  "cache_control": {"type": "ephemeral"}
}
```

### TTL Options

| TTL | Description | Cost Multiplier |
|-----|-------------|-----------------|
| `5m` (default) | 5-minute cache | 1.25x base input |
| `1h` | 1-hour cache | 2x base input |

Cache reads cost 0.1x base input price.

```json
{
  "cache_control": {
    "type": "ephemeral",
    "ttl": "1h"
  }
}
```

### Minimum Cacheable Tokens

| Model | Minimum Tokens |
|-------|----------------|
| Claude Opus 4.5 | 4096 |
| Claude Opus 4.1/4, Sonnet 4.5/4 | 1024 |
| Claude Haiku 4.5 | 4096 |
| Claude Haiku 3.5/3 | 2048 |

### Cache Hierarchy

Cache prefixes follow this order: `tools` → `system` → `messages`

Changes at any level invalidate that level and all subsequent levels.

### Usage Response

```json
{
  "usage": {
    "input_tokens": 50,
    "cache_creation_input_tokens": 1000,
    "cache_read_input_tokens": 5000,
    "output_tokens": 100
  }
}
```

Total input = `cache_read_input_tokens` + `cache_creation_input_tokens` + `input_tokens`

### Forge Implementation

In `providers/src/lib.rs`, cache hints are applied via `CacheHint`:

```rust
fn content_block(text: &str, cache_hint: CacheHint) -> serde_json::Value {
    match cache_hint {
        CacheHint::None => json!({"type": "text", "text": text}),
        CacheHint::Ephemeral => json!({
            "type": "text",
            "text": text,
            "cache_control": {"type": "ephemeral"}
        }),
    }
}
```

System prompts are cached by default. System messages from conversation history are hoisted into the `system` array.

## Extended Thinking

Extended thinking allows Claude to show its reasoning process.

### Enable Thinking

```json
{
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000
  }
}
```

Requirements:
- `budget_tokens` must be ≥ 1024
- `budget_tokens` must be < `max_tokens`

### Disable Thinking

```json
{
  "thinking": {
    "type": "disabled"
  }
}
```

### Thinking Response

Non-streaming:
```json
{
  "content": [
    {
      "type": "thinking",
      "thinking": "Let me work through this...",
      "signature": "EqQBCgIYAhIM..."
    },
    {
      "type": "text",
      "text": "The answer is 42."
    }
  ]
}
```

Streaming events:
```json
event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Let me..."}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQB..."}}
```

### Forge Implementation

In `providers/src/lib.rs`:

```rust
if let Some(budget) = limits.thinking_budget() {
    body.insert("thinking".into(), json!({
        "type": "enabled",
        "budget_tokens": budget
    }));
}
```

Thinking deltas are mapped to `StreamEvent::ThinkingDelta`.

## Error Handling

### HTTP Errors

| Status | Meaning |
|--------|---------|
| 400 | Bad request (invalid parameters) |
| 401 | Invalid API key |
| 403 | Permission denied |
| 429 | Rate limited |
| 500 | Server error |
| 529 | Overloaded |

### Error Response Format

```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "Description of the error"
  }
}
```

### Forge Error Handling

Non-2xx responses emit `StreamEvent::Error` with status and body:

```rust
if !response.status().is_success() {
    let status = response.status();
    let error_text = response.text().await...;
    on_event(StreamEvent::Error(format!("API error {}: {}", status, error_text)));
}
```

## Complete Request Example

```bash
curl https://api.anthropic.com/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-5-20250929",
    "max_tokens": 1024,
    "stream": true,
    "system": [
      {
        "type": "text",
        "text": "You are a helpful assistant.",
        "cache_control": {"type": "ephemeral"}
      }
    ],
    "messages": [
      {"role": "user", "content": "Hello, Claude!"}
    ]
  }'
```

## Forge-Specific Notes

### System Message Hoisting

Forge hoists `Message::System` variants from conversation history into the `system` array, not `messages`. This matches Anthropic's expected format where system content belongs in the dedicated system field.

### Assistant Message Format

Assistant messages are sent as plain strings, not content block arrays:

```rust
Message::Assistant(_) => {
    api_messages.push(json!({
        "role": "assistant",
        "content": msg.content()  // String, not array
    }));
}
```

This means cache control cannot be applied to assistant messages, which is acceptable since caching is most valuable for system prompts and early user messages.

### Headers

Forge sends these headers:
- `x-api-key`: API key (not `Authorization: Bearer`)
- `anthropic-version`: `2023-06-01`
- `content-type`: `application/json`

## References

- [Anthropic API Reference](https://docs.anthropic.com/en/api/messages)
- [Streaming Messages](https://docs.anthropic.com/en/api/messages-streaming)
- [Prompt Caching](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching)
- [Extended Thinking](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking)

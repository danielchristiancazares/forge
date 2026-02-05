# Anthropic Messages API Reference

This document provides a reference for the Anthropic Messages API as used by Forge. It covers the endpoint structure, request/response formats, SSE streaming events, prompt caching, extended thinking, adaptive thinking, compaction, and effort controls.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-21 | Header & TOC |
| 22-29 | Overview |
| 31-146 | Request Structure |
| 147-262 | Tool Schema |
| 263-301 | Response Structure |
| 302-434 | SSE Streaming |
| 435-535 | Prompt Caching |
| 536-681 | Extended Thinking & Adaptive Thinking |
| 682-801 | Compaction |
| 802-852 | Effort Parameter |
| 853-891 | Error Handling |
| 892-995 | Complete Request Examples |
| 996-1043 | Opus 4.6 Migration Guide |
| 1044-1088 | Forge-Specific Notes |
| 1089-1100 | References |

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
| `model` | string | Model ID (e.g., `claude-opus-4-6`, `claude-sonnet-4-5-20250514`) |
| `max_tokens` | number | Maximum tokens to generate (up to 128K on Opus 4.6, 64K on Sonnet/Haiku 4.5) |
| `messages` | array | Conversation messages with alternating `user`/`assistant` roles |

### Optional Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `system` | string/array | - | System prompt (string or array of content blocks) |
| `stream` | boolean | false | Enable SSE streaming. **Required** for large `max_tokens` values on Opus 4.6 to avoid HTTP timeouts |
| `temperature` | number | 1.0 | Randomness (0.0-1.0) |
| `top_p` | number | - | Nucleus sampling |
| `top_k` | number | - | Top-K sampling |
| `stop_sequences` | array | - | Custom stop sequences |
| `thinking` | object | - | Extended thinking configuration (see [Extended Thinking](#extended-thinking--adaptive-thinking)) |
| `tools` | array | - | Tool definitions (see [Tool Schema](#tool-schema)) |
| `tool_choice` | object | - | Control tool usage (see [Tool Choice](#tool-choice)) |
| `output_config` | object | - | Output configuration including `format` and `effort` (see below) |
| `context_management` | object | - | Context management strategies including compaction (see [Compaction](#compaction)) |
| `inference_geo` | string | `"global"` | Data residency: `"global"` or `"us"`. US-only priced at 1.1x on Opus 4.6+ |

#### Deprecated Fields (functional, removal planned)

| Field | Deprecated On | Replacement | Notes |
|-------|---------------|-------------|-------|
| `output_format` | Opus 4.6 | `output_config.format` | Same value, nested under `output_config` |

### Structured Output Configuration

The `output_config` field controls output format and effort:

```json
{
  "output_config": {
    "format": {
      "type": "json_schema",
      "schema": {
        "type": "object",
        "properties": {
          "answer": {"type": "string"}
        },
        "required": ["answer"]
      }
    },
    "effort": "high"
  }
}
```

> **Migration note:** On Opus 4.6, `output_format` at the top level is deprecated. Move it to `output_config.format`. The old parameter still works but will be removed in a future release.

```json
// Before (deprecated on Opus 4.6)
{
  "output_format": {"type": "json_schema", "schema": {...}}
}

// After
{
  "output_config": {"format": {"type": "json_schema", "schema": {...}}}
}
```

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

> **⚠️ BREAKING (Opus 4.6): Assistant message prefills are not supported.** Requests with prefilled assistant messages (last-turn assistant content intended to steer the model's response) return a **400 error**. Use structured outputs (`output_config.format`) or system prompt instructions instead. See [Opus 4.6 Migration Guide](#opus-46-migration-guide).

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

### Tool Call JSON Escaping (Opus 4.6 Behavioral Change)

Opus 4.6 may produce slightly different JSON string escaping in tool call arguments compared to previous models (e.g., different handling of Unicode escapes or forward slash escaping). Standard JSON parsers (`json.loads()`, `JSON.parse()`, `serde_json::from_str()`) handle these differences automatically.

**Forge implication:** If any code path performs string matching, regex parsing, or raw-string comparison on tool call `input` rather than deserializing through a JSON parser, those paths may silently break. Always deserialize tool call arguments through a proper JSON parser.

## Response Structure

### Non-Streaming Response

```json
{
  "type": "message",
  "id": "msg_...",
  "model": "claude-opus-4-6",
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

| Reason | Description | Since |
|--------|-------------|-------|
| `end_turn` | Natural completion | - |
| `max_tokens` | Hit token limit | - |
| `stop_sequence` | Hit custom stop sequence | - |
| `tool_use` | Model wants to use a tool | - |
| `compaction` | Context was compacted; conversation can continue from summary | Opus 4.6 (beta) |

> **Note on `max_tokens` with adaptive thinking (Opus 4.6):** At `high` and `max` effort levels, the model may use more tokens for thinking, making it more likely to hit the `max_tokens` ceiling. If you see unexpected `max_tokens` stops, either increase `max_tokens` or lower the effort level.

## SSE Streaming

Enable with `"stream": true`. Events are sent as Server-Sent Events.

> **Opus 4.6 note:** Streaming is **required** when using large `max_tokens` values (approaching 128K) to avoid HTTP timeouts. The SDKs handle this, but if using raw HTTP, use streaming and collect via `.get_final_message()` equivalent.

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
    "model": "claude-opus-4-6",
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

### Fine-Grained Tool Streaming

Fine-grained tool streaming is GA on all models and platforms as of Opus 4.6. No beta header required. This enables streaming of individual tool call arguments as they are generated, rather than receiving the entire tool call at once.

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

> **TODO:** Handle `stop_reason: "compaction"` when server-side compaction is enabled. This requires continuing the conversation from the compaction summary rather than treating it as a terminal event.

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
| Claude Opus 4.6 | TBD (assume 1024 until confirmed) |
| Claude Sonnet 4.5 | 4096 |
| Claude Haiku 4.5 | 4096 |

### Cache Hierarchy

Cache prefixes follow this order: `tools` → `system` → `messages`

Changes at any level invalidate that level and all subsequent levels.

### Cache Interaction with Adaptive Thinking (Opus 4.6)

Consecutive requests using adaptive thinking (`thinking: {type: "adaptive"}`) **preserve** prompt cache breakpoints. However, **switching between thinking modes** (adaptive ↔ enabled ↔ disabled) across turns **breaks** cache breakpoints for messages.

**Forge implication:** If Forge dynamically toggles thinking modes across turns (e.g., thinking for complex tasks, no thinking for simple routing), it will thrash cache breakpoints and pay re-caching costs on each mode switch. Pick one thinking mode and stick with it for a given conversation, or accept the cache penalty.

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
        CacheHint::Default => json!({"type": "text", "text": text}),
        CacheHint::Ephemeral => json!({
            "type": "text",
            "text": text,
            "cache_control": {"type": "ephemeral"}
        }),
    }
}
```

System prompts are cached by default. System messages from conversation history are hoisted into the `system` array.

## Extended Thinking & Adaptive Thinking

Extended thinking allows Claude to show its reasoning process. Opus 4.6 introduces **adaptive thinking** as the recommended mode, replacing the manual budget-based approach.

### Thinking Modes

| Mode | Supported Models | Description |
|------|-----------------|-------------|
| `adaptive` | Opus 4.6+ | **Recommended.** Claude decides when and how much to think. Controlled via `effort` parameter. Auto-enables interleaved thinking. |
| `enabled` | All thinking models | Manual mode with explicit `budget_tokens`. **Deprecated on Opus 4.6** (still functional, removal planned). |
| `disabled` | All thinking models | No thinking. |

### Adaptive Thinking (Opus 4.6+, Recommended)

```json
{
  "thinking": {
    "type": "adaptive"
  }
}
```

Claude evaluates the complexity of each request and decides whether and how much to think. Behavior varies by effort level:

| Effort | Thinking Behavior |
|--------|-------------------|
| `low` | May skip thinking entirely for simple problems |
| `medium` | Thinks selectively |
| `high` (default) | Almost always thinks |
| `max` | Maximum thinking depth |

Adaptive thinking automatically enables **interleaved thinking** (thinking between tool calls). The `interleaved-thinking-2025-05-14` beta header is not required and is safely ignored if present.

```json
{
  "model": "claude-opus-4-6",
  "max_tokens": 16000,
  "thinking": {"type": "adaptive"},
  "output_config": {"effort": "high"},
  "messages": [{"role": "user", "content": "Solve this complex problem..."}]
}
```

> **Key difference from `budget_tokens`:** Adaptive thinking provides no hard token ceiling for thinking. The model allocates thinking tokens dynamically. Cost control is qualitative (via effort level), not quantitative. At `high` and `max` effort, the model may consume a large portion of `max_tokens` for thinking, leaving less for the visible response.

#### Cache Behavior

- Consecutive requests with adaptive thinking **preserve** cache breakpoints.
- **Switching** between `adaptive`, `enabled`, and `disabled` modes across turns **breaks** cache breakpoints.

### Manual Thinking (Legacy, Deprecated on Opus 4.6)

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

> **Deprecation notice:** `thinking: {type: "enabled"}` and `budget_tokens` are deprecated on Opus 4.6. They remain functional but will be removed in a future model release. Migrate to adaptive thinking with the effort parameter.

### Disable Thinking

```json
{
  "thinking": {
    "type": "disabled"
  }
}
```

### Thinking Response

Non-streaming (both adaptive and manual):

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

### Thinking Block Preservation

On Opus 4.6 and Sonnet 4.5, thinking blocks from previous assistant turns are **preserved in model context by default**. This differs from earlier models which stripped prior-turn thinking.

Implications:

- Thinking blocks consume context window space across turns (context fills faster)
- Preserved thinking enables prompt cache hits on multi-step tool-use workflows
- No intelligence impact from preservation
- Combined with compaction, thinking blocks being preserved can trigger compaction earlier than expected

### Summarized Thinking

Claude 4 models return a **summary** of the full thinking process, not the raw thinking. The full thinking is encrypted in the `signature` field. You are billed for the full thinking tokens, not the summary. The billed output token count will not match the visible token count.

### Forge Implementation

In `providers/src/lib.rs`:

```rust
use forge_types::ThinkingState;

// Legacy manual mode (works on all models, deprecated on Opus 4.6)
if let ThinkingState::Enabled(budget) = limits.thinking() {
    body.insert("thinking".into(), json!({
        "type": "enabled",
        "budget_tokens": budget.as_u32()
    }));
}
```

> **TODO (Opus 4.6):** Add `ThinkingState::Adaptive` variant. When model is `claude-opus-4-6`, emit `{"type": "adaptive"}` and control depth via the effort parameter rather than `budget_tokens`. The `ThinkingState::Enabled` path should still be used for older models.

Thinking deltas are mapped to `StreamEvent::ThinkingDelta`. The streaming event format is identical for adaptive and manual modes; no changes needed in SSE parsing.

## Compaction

Compaction provides server-side context summarization, enabling long-running conversations that exceed the context window. When context approaches a configured threshold, the API automatically generates a summary and drops older messages.

### Beta Status

Compaction is in beta. Requires header: `anthropic-beta: compact-2026-01-12`

Supported models: Claude Opus 4.6 (`claude-opus-4-6`)

### Enable Compaction

Add `compact_20260112` to `context_management.edits`:

```json
{
  "model": "claude-opus-4-6",
  "max_tokens": 4096,
  "messages": [...],
  "context_management": {
    "edits": [
      {
        "type": "compact_20260112"
      }
    ]
  }
}
```

### Configure Trigger Threshold

```json
{
  "context_management": {
    "edits": [
      {
        "type": "compact_20260112",
        "trigger": {
          "type": "input_tokens",
          "value": 150000
        }
      }
    ]
  }
}
```

### Custom Summary Instructions

Replace the default summary prompt entirely:

```json
{
  "context_management": {
    "edits": [
      {
        "type": "compact_20260112",
        "instructions": "Focus on preserving code snippets, variable names, and technical decisions."
      }
    ]
  }
}
```

### Pause After Compaction

For manual control, set `pause_after_compaction: true`. The API returns `stop_reason: "compaction"` and you decide when to continue:

```json
{
  "context_management": {
    "edits": [
      {
        "type": "compact_20260112",
        "trigger": {"type": "input_tokens", "value": 100000},
        "pause_after_compaction": true
      }
    ]
  }
}
```

### Compaction Response

When compaction triggers, the API returns a compaction block at the start of the assistant response. All message blocks **prior to the compaction block are dropped** by the API. The conversation continues from the summary.

The default summary prompt produces a structured summary including:
- Task overview (core request, success criteria, constraints)
- Current state (completed work, modified files, artifacts)
- Important discoveries (constraints, decisions, errors, failed approaches)
- Next steps (actions needed, blockers, priority order)
- Context to preserve (user preferences, domain details, commitments)

The summary is wrapped in `<summary></summary>` tags.

### Forge Integration

Forge does not currently implement client-side context summarization. Server-side compaction is the natural fit for long-running conversations. To adopt:

1. Send `anthropic-beta: compact-2026-01-12` header
2. Add `{"type": "compact_20260112"}` to `context_management.edits`
3. Handle `stop_reason: "compaction"` if using `pause_after_compaction: true`

### SDK Compaction (Alternative)

The Python and TypeScript SDKs also provide **client-side** compaction via `tool_runner`. This is a separate mechanism from the server-side API compaction:

```python
compaction_control={
    "enabled": True,
    "context_token_threshold": 100000,
    "model": "claude-haiku-4-5"  # Can use a cheaper model for summaries
}
```

This SDK-level compaction injects a summary request as a user message, generates a summary, then replaces conversation history.

## Effort Parameter

The effort parameter controls how eagerly Claude spends tokens when responding. It affects all token output: text responses, tool calls, and extended thinking.

### Availability

| Model | Status | Header Required |
|-------|--------|-----------------|
| Opus 4.6 | **GA** | None |

### Effort Levels

| Level | Description | Use When |
|-------|-------------|----------|
| `low` | Most token-efficient. Fewer tool calls, shorter responses. | Speed-sensitive or simple tasks |
| `medium` | Balanced. Solid performance without full token expenditure. | Most production use cases |
| `high` (default) | Maximum thoroughness. | Complex reasoning, nuanced analysis, hard coding problems |
| `max` | Absolute highest capability. **New in Opus 4.6.** | Problems requiring deepest possible reasoning |

### Usage

On Opus 4.6 (GA, no beta header):

```json
{
  "model": "claude-opus-4-6",
  "max_tokens": 16000,
  "thinking": {"type": "adaptive"},
  "output_config": {"effort": "medium"},
  "messages": [...]
}
```

> **Note:** Effort interacts with `max_tokens`. At `high` and `max`, Claude may consume a large portion of `max_tokens` for thinking, potentially triggering `stop_reason: "max_tokens"` before the visible response completes. If this happens, increase `max_tokens` or lower effort.

## Error Handling

### HTTP Errors

| Status | Meaning |
|--------|---------|
| 400 | Bad request (invalid parameters). **On Opus 4.6, prefilled assistant messages trigger this.** |
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
    let _ = tx
        .send(StreamEvent::Error(format!("API error {}: {}", status, error_text)))
        .await;
}
```

## Complete Request Examples

### Basic Request (Opus 4.6)

```bash
curl https://api.anthropic.com/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-opus-4-6",
    "max_tokens": 16000,
    "stream": true,
    "thinking": {"type": "adaptive"},
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

### With Compaction (Opus 4.6, Beta)

```bash
curl https://api.anthropic.com/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "anthropic-beta: compact-2026-01-12" \
  -d '{
    "model": "claude-opus-4-6",
    "max_tokens": 4096,
    "stream": true,
    "thinking": {"type": "adaptive"},
    "output_config": {"effort": "high"},
    "messages": [...],
    "context_management": {
      "edits": [
        {
          "type": "compact_20260112",
          "trigger": {"type": "input_tokens", "value": 150000}
        }
      ]
    }
  }'
```

### With 1M Context Window (Beta)

```bash
curl https://api.anthropic.com/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "anthropic-beta: context-1m-2025-08-07" \
  -d '{
    "model": "claude-opus-4-6",
    "max_tokens": 16000,
    "stream": true,
    "messages": [
      {"role": "user", "content": "...up to 1M tokens of input..."}
    ]
  }'
```

**1M context pricing (>200K input tokens):**

| Token Type | Standard (≤200K) | Long Context (>200K) |
|------------|------------------|----------------------|
| Input | $5/M | $10/M |
| Output | $25/M | $37.50/M |

Requires beta header `context-1m-2025-08-07`. Available to organizations in usage tier 4 and organizations with custom rate limits.

## Opus 4.6 Migration Guide

### Breaking Changes

| Change | Impact | Action Required |
|--------|--------|-----------------|
| **Prefill removal** | Requests with last-turn assistant prefills return 400 | Model-gate prefill logic: skip on `claude-opus-4-6`, use `output_config.format` or system prompt steering instead |
| **128K output requires streaming** | Non-streaming requests with large `max_tokens` timeout | Ensure all Opus 4.6 code paths use streaming for large outputs |

### Deprecations (Functional Now, Will Break Later)

| Change | Impact | Action Required |
|--------|--------|-----------------|
| `thinking: {type: "enabled", budget_tokens: N}` | Deprecated on Opus 4.6 | Add `ThinkingState::Adaptive` variant; emit `{type: "adaptive"}` for Opus 4.6, keep `{type: "enabled"}` for older models |
| `interleaved-thinking-2025-05-14` beta header | Safely ignored on Opus 4.6 | Remove from Opus 4.6 requests (adaptive thinking auto-enables interleaving) |
| `output_format` (top-level) | Deprecated on Opus 4.6 | Move to `output_config.format` |

### Behavioral Changes

| Change | Impact | Action Required |
|--------|--------|-----------------|
| Tool call JSON escaping | Different Unicode/forward-slash escaping in tool args | Audit for raw string matching; ensure `serde_json` deserialization everywhere |
| Adaptive thinking cost | No hard token ceiling; `high`/`max` effort may exhaust `max_tokens` | Set `max_tokens` higher than expected output size to leave room for thinking |
| Cache breakpoints on mode switch | Switching thinking modes breaks cache | Avoid toggling thinking mode across turns in a conversation |
| Thinking block preservation | Prior-turn thinking blocks stay in context | Context fills faster; may trigger compaction earlier |

### Beta Headers Summary

| Header | Purpose | Required On |
|--------|---------|-------------|
| `compact-2026-01-12` | Server-side compaction | Opus 4.6 (if using compaction) |
| `context-1m-2025-08-07` | 1M token context window | Opus 4.6, Sonnet 4.5 (if using >200K input) |
| ~~`effort-2025-11-24`~~ | ~~Effort parameter~~ | ~~Opus 4.5 only~~ (not needed, Opus 4.6 has GA effort) |
| ~~`interleaved-thinking-2025-05-14`~~ | ~~Interleaved thinking~~ | ~~Deprecated on Opus 4.6~~ (safely ignored) |

### Model Capability Matrix

| Capability | Opus 4.6 | Sonnet 4.5 |
|------------|----------|------------|
| Max context | 1M (beta) | 200K (1M beta) |
| Max output | 128K | 64K |
| Adaptive thinking | ✅ | ❌ |
| Manual thinking (`budget_tokens`) | ⚠️ Deprecated | ✅ |
| Effort parameter | ✅ GA (4 levels) | ❌ |
| Compaction (server-side) | ✅ Beta | ❌ |
| Prefill | ❌ **Removed** | ✅ |
| Data residency (`inference_geo`) | ✅ | ❌ |

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

> **Opus 4.6 note:** Since prefill is removed, Forge must not append a partial assistant message as the last message in the `messages` array when targeting Opus 4.6. Any existing prefill logic must be gated by model string.

### Headers

Forge sends these headers:

- `x-api-key`: API key (not `Authorization: Bearer`)
- `anthropic-version`: `2023-06-01`
- `content-type`: `application/json`
- `anthropic-beta`: Comma-separated list of beta features as needed (e.g., `compact-2026-01-12,context-1m-2025-08-07`)

### Forge TODO Summary (Opus 4.6)

1. ~~**[BREAKING]** Gate prefill logic by model string; return error or skip for `claude-opus-4-6`~~ **Done**
2. ~~**[BREAKING]** Ensure streaming is used for large `max_tokens` on Opus 4.6~~ **Done** (always streams)
3. ~~Add `ThinkingState::Adaptive` variant; emit `{type: "adaptive"}` for Opus 4.6~~ **Done** (inline in provider)
4. ~~Add effort parameter support (`output_config.effort`); model-gate beta header~~ **Done** (hardcoded to `max`)
5. Migrate `output_format` → `output_config.format` (backward compat for older models)
6. Add `context_management` support for server-side compaction
7. Handle `stop_reason: "compaction"` in stream event handling
8. Add `inference_geo` support if data residency is needed
9. ~~Add `context-1m-2025-08-07` beta header support for 1M context~~ **Done**
10. Audit tool call argument parsing for raw string matching (JSON escaping change)

## References

- [Anthropic API Reference](https://platform.claude.com/docs/en/api/overview)
- [What's New in Claude 4.6](https://platform.claude.com/docs/en/about-claude/models/whats-new-claude-4-6)
- [Adaptive Thinking](https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking)
- [Compaction](https://platform.claude.com/docs/en/build-with-claude/compaction)
- [Effort Parameter](https://platform.claude.com/docs/en/build-with-claude/effort)
- [Extended Thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)
- [Streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming)
- [Prompt Caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [Migration Guide](https://platform.claude.com/docs/en/about-claude/models/migration-guide)
- [Introducing Claude Opus 4.6](https://www.anthropic.com/news/claude-opus-4-6)

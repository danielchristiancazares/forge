# OpenAI Reasoning Item Round-Trip (Multi-Turn + Tools)

## Problem

Forge currently drops `Message::Thinking` when building OpenAI Responses API requests (`providers/src/openai.rs:311-313`). This is correct for *UI-only* thinking, but it prevents OpenAI reasoning items from being replayed across turns.

For tool-heavy multi-turn flows, OpenAI’s Responses API can emit output items of `type: "reasoning"` (optionally with `encrypted_content`). These reasoning items are meant to be fed back in the next request’s `input` list to preserve continuity.

If we don’t replay them, the model can lose the prior tool-context / intermediate state and repeat itself.

## Important Non-Goals / Avoided Assumptions

- Do **not** assume “exactly one reasoning item per response”. Treat reasoning items as *0..N* output items.
- Do **not** assume reasoning id emission is coupled to `reasoning_summary_*` events. Summaries may be disabled while reasoning items still exist.
- Do **not** assume OpenAI “silently discards stale reasoning items” from earlier turns. Prefer cookbook-style accumulation, but keep an escape hatch to cap or filter if needed.

## OpenAI API Behavior (What We Actually Need)

### Request

- Add `include: ["reasoning.encrypted_content"]` so the stream includes `encrypted_content` for reasoning items.
- Decide application-state strategy explicitly:
  - **Stateless (recommended for ZDR-like behavior):** set `store: false` and replay reasoning items (`id` + `encrypted_content`) every turn.
  - **Stateful:** set `store: true` and use `previous_response_id` chaining instead of (or in addition to) replaying reasoning items. This doc focuses on stateless replay.

### Response stream

- Reasoning items appear as output items and complete on an `response.output_item.done` event, which contains the finalized `item` (including `item.id`, and `encrypted_content` if requested via `include`).
- Existing `response.reasoning_summary_text.*` handling is *UI-facing* and should remain independent.

### Next request

Replay the reasoning item as an input item:

```json
{ "type": "reasoning", "id": "rs_...", "encrypted_content": "gAAAA..." }
```

## Design In Forge

### Key constraint: provider-safe replay tokens

Forge already uses `ThinkingMessage.signature` (`ThoughtSignatureState`) to round-trip Claude `redacted_thinking`.

We **must not** overload that field for OpenAI:

- `providers/src/claude.rs` will serialize *any* signed `Message::Thinking` as Claude `redacted_thinking` today.
- If we stuff OpenAI `encrypted_content` into `ThoughtSignatureState::Signed`, Claude requests could send garbage.

Therefore: store OpenAI reasoning replay data in a **separate, provider-tagged field**.

### Proposed type additions

In `types/src/lib.rs`:

- Add an OpenAI-specific replay payload:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIReasoningReplay {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
}
```

- Extend `ThinkingMessage` with an optional OpenAI replay field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
openai_reasoning: Option<OpenAIReasoningReplay>,
```

- Add helpers:

```rust
pub fn openai_reasoning(&self) -> Option<&OpenAIReasoningReplay>;
pub fn has_openai_reasoning(&self) -> bool;
```

### Streaming capture

In `providers/src/sse_types.rs` (OpenAI section):

- Extend `OutputItem` with a `Reasoning` variant:

```rust
#[serde(rename = "reasoning")]
Reasoning {
    id: Option<String>,
    encrypted_content: Option<String>,
},
```

- Add `response.output_item.done` event:

```rust
#[serde(rename = "response.output_item.done")]
OutputItemDone {
    #[serde(alias = "output_item")]
    item: Option<OutputItem>,
},
```

Notes:
- Do not depend on a top-level `item_id` field; use `item.id` from the payload.

In `providers/src/openai.rs` (parser):

- On `OutputItemDone` where `item` is `Reasoning { id, encrypted_content }`, emit a new stream event carrying both fields.

In `types/src/lib.rs`:

- Add a new stream event:

```rust
StreamEvent::OpenAIReasoningDone { id: String, encrypted_content: Option<String> }
```

In `engine/src/lib.rs` (`StreamingMessage`):

- Add storage for the latest reasoning replay (or `Vec<...>` if we want 0..N faithfully):

```rust
openai_reasoning: Option<OpenAIReasoningReplay>,
```

- Handle `StreamEvent::OpenAIReasoningDone` in `apply_event`.

### Persisting into history

Today, Forge only persists thinking into history when it has a Claude signature:

- `engine/src/streaming.rs:699-709`
- `engine/src/tool_loop.rs:1074-1083`

Revise the “persist vs local-only” check to:

- Persist if:
  - Claude signature is present **OR**
  - OpenAI reasoning replay is present

This ensures the OpenAI reasoning item is available when we build the next request.

### Request building (OpenAI)

In `providers/src/openai.rs` (`build_request_body`):

- Add `include: ["reasoning.encrypted_content"]` when using GPT-5 models (as today, model ids are `gpt-5.*`).
- Also set `store: false` (if we choose stateless replay).
- Serialize `Message::Thinking(thinking)` into an OpenAI reasoning input item **only when**:
  - `thinking.provider() == Provider::OpenAI`, and
  - `thinking.openai_reasoning()` is present.

Example serialization:

```rust
Message::Thinking(thinking) => {
    if thinking.provider() == Provider::OpenAI {
        if let Some(r) = thinking.openai_reasoning() {
            let mut item = json!({ "type": "reasoning", "id": r.id });
            if let Some(enc) = &r.encrypted_content {
                item["encrypted_content"] = json!(enc);
            }
            input_items.push(item);
        }
    }
}
```

### Provider guard (Claude)

In `providers/src/claude.rs`, tighten:

- Only emit `redacted_thinking` if `thinking.provider() == Provider::Claude` and signature is signed.

This prevents OpenAI replay blobs from ever being sent to Anthropic.

## Ordering

Keep existing history ordering in `engine/src/tool_loop.rs:1060+`:

1. `ThinkingMessage` (replayed as `type: "reasoning"` for OpenAI)
2. `AssistantMessage` (if any text)
3. Tool calls (`function_call`) x N
4. Tool results (`function_call_output`) x N

This matches cookbook-style `context += response.output` accumulation.

## Implementation Checklist (Files)

- `providers/src/sse_types.rs`
  - OpenAI: add `OutputItem::Reasoning` and `Event::OutputItemDone`
- `types/src/lib.rs`
  - Add `OpenAIReasoningReplay`
  - Extend `ThinkingMessage` with `openai_reasoning`
  - Add `StreamEvent::OpenAIReasoningDone`
- `providers/src/openai.rs`
  - Parse `OutputItemDone` and emit `OpenAIReasoningDone`
  - In request builder: include `include`, set `store: false` (if chosen), and serialize OpenAI reasoning items from `Message::Thinking`
- `engine/src/lib.rs`
  - Store OpenAI reasoning replay on `StreamingMessage`
  - Handle the new stream event
- `engine/src/streaming.rs`
  - Persist thinking when it contains Claude signature OR OpenAI reasoning replay
  - Build `ThinkingMessage` with the OpenAI replay payload
- `providers/src/claude.rs`
  - Provider-guard `Message::Thinking` serialization

## Verification

- Run `just verify`.
- Add/adjust unit tests:
  - OpenAI SSE parse: `response.output_item.done` with a reasoning item yields `StreamEvent::OpenAIReasoningDone`.
  - OpenAI request builder: a `ThinkingMessage` with `openai_reasoning` becomes an `input` item of `type: "reasoning"`.
  - Claude request builder: ignores OpenAI thinking messages even if they contain signed data.

# OpenAI Responses API (GPT-5.2) - Request Field Checklist

This doc summarizes the request fields we care about when calling the Responses
API with GPT-5.2. It is intentionally scoped to GPT-5.2 usage and excludes
sampling/logprob controls (per product decision).

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-22 | Header & TOC |
| 23-30 | Core Request Fields |
| 32-41 | Reasoning Controls |
| 43-46 | Output Formatting |
| 48-53 | Tools and Tool Selection |
| 55-71 | Streaming / Background |
| 73-78 | Conversation State |
| 80-85 | Caching / Safety / Tiering |
| 87-90 | Metadata and Templates |
| 92-94 | Out of Scope |
| 96-105 | References |

## Core request fields

- model: GPT-5.2 or GPT-5.2 Pro model, or pinned snapshot (e.g., gpt-5.2-YYYY-MM-DD, gpt-5.2-pro-YYYY-MM-DD).
- input: user/system/assistant items, or text input. Supports text, image,
  and file inputs when needed.
- instructions: system/developer prompt inserted into context. Not carried
  forward when using previous_response_id unless you set it again.
- max_output_tokens: cap on total output tokens (includes reasoning tokens).

## Reasoning controls (GPT-5.2)

- reasoning.effort:
  - **GPT-5.2**: none (default) | low | medium | high | xhigh
  - **GPT-5.2 Pro**: medium | high | xhigh only (no none/low)
- reasoning.summary: configure summaries of reasoning where supported (none | auto | concise | detailed).
- include: reasoning.encrypted_content when running stateless (store=false
  or ZDR), so reasoning items can be reused across turns.

**Forge policy**: Default to xhigh for GPT-5.2 Pro (that's what Pro is for).

## Output formatting

- text.verbosity: low | medium | high to guide output length/structure.
- text.format: structured outputs with JSON schema or JSON mode when needed.

## Tools and tool selection

- tools: built-in tools, MCP tools, or function tools.
- tool_choice: control tool selection; supports allowed tools list.
- max_tool_calls: cap total built-in tool calls in a response.
- parallel_tool_calls: allow or disallow parallel tool calls.

## Streaming / background

- stream: enable SSE streaming.
- stream_options: only when stream=true.
- background: run response asynchronously (useful for long tasks).

**GPT-5.2 Pro requirements**: Per OpenAI docs, Pro requests "may take several
minutes to finish" and they recommend "using background mode" to avoid timeouts.

**Forge policy**: Auto-enable background mode for GPT-5.2 Pro. This is required,
not optional - SSE idle timeouts are fundamentally incompatible with deep
reasoning that takes minutes to hours.

**Constraints**:
- background=true requires store=true
- background=true is not ZDR compatible ("breaks ZDR guarantees")
- background=true is not available in EU region

## Conversation state

- previous_response_id: continue a multi-turn conversation.
- conversation: optional conversation object or ID.
- Note: do not use previous_response_id and conversation together.
- truncation: auto vs disabled (errors when context is too large).

## Caching / safety / tiering

- prompt_cache_key and prompt_cache_retention: enable cache control.
- safety_identifier: stable user identifier (hashed).
- service_tier: auto | default | flex | priority.
- store: whether to store responses for later retrieval.

## Metadata and templates

- metadata: up to 16 key-value pairs for indexing.
- prompt: reference a prompt template and variables.

## Out of scope (intentionally not used)

- temperature / top_p / logprobs

## References

- <https://platform.openai.com/docs/api-reference/responses/create>
- <https://platform.openai.com/docs/guides/latest-model>
- <https://platform.openai.com/docs/guides/reasoning/use-case-examples>
- <https://platform.openai.com/docs/guides/structured-outputs/introduction%3F.doc>
- <https://platform.openai.com/docs/models/gpt-5.2/>
- <https://platform.openai.com/docs/models/gpt-5.2-pro>
- <https://platform.openai.com/docs/guides/background>
- <https://platform.openai.com/docs/guides/your-data>

# Cross-Provider Token Counting

**Status:** Backlog  
**Priority:** Medium  
**Type:** Bug/Architecture

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-10 | Header & Summary |
| 11-23 | Problem: Token divergence (OpenAI vs Claude) |
| 24-39 | Impact & Current Behavior |
| 40-75 | Proposed Solutions: Enum, Margin, API |
| 76-108 | Affected Components, Questions, Related |  

## Summary

Token counting across providers is not 1:1. When switching between OpenAI and Claude (or other providers), token counts diverge, potentially causing context window overflow or inefficient budget usage.

## Problem Statement

The codebase currently uses OpenAI's `cl100k_base` tokenizer (via `tiktoken-rs`) for:

1. **WebFetch chunking** - `webfetch/chunker.rs` uses `cl100k_base` to split content into token-budgeted chunks
2. **Context window management** - Token budgets assume consistent tokenization

When the active provider is Claude (or another non-OpenAI model), the actual token consumption differs from the estimated count:

- Same text produces different token counts across providers.
- **Intra-provider divergence**: Even within OpenAI, newer models (GPT-4o, GPT-5) use `o200k_base`, which is ~20-40% more efficient than GPT-4's `cl100k_base`.
- A "600 token" chunk for OpenAI `cl100k_base` may be 450 tokens for GPT-5 (`o200k_base`) or 650 tokens for Claude.

## Impact

| Scenario | Risk |
|----------|------|
| Chunk undercount | Context overflow when assembling tool responses |
| Chunk overcount | Wasted context budget, premature summarization |
| Budget calculation drift | Accumulated error across conversation turns |

## Current Behavior

```rust
// webfetch/chunker.rs
static CL100K_BASE: OnceLock<Result<CoreBPE, String>> = OnceLock::new();
// Always uses cl100k_base regardless of active provider
```

## Proposed Solutions

### Option A: Provider-Specific Tokenizers

```rust
enum Tokenizer {
    OpenAI(CoreBPE),      // cl100k_base (GPT-4) or o200k_base (GPT-5)
    Claude(ClaudeTokenizer), // Community crate like claude-tokenizer
    Approximate(f32),     // Fallback: chars * ratio
}

fn get_tokenizer(model: ModelName) -> Tokenizer {
    match model {
        m if m.starts_with("gpt-5") => Tokenizer::OpenAI(o200k_base()),
        m if m.starts_with("gpt-4") => Tokenizer::OpenAI(cl100k_base()),
        m if m.starts_with("claude-") => Tokenizer::Claude(...),
        _ => Tokenizer::Approximate(0.25),
    }
}
```

### Option B: Safety Margin

Apply a conservative multiplier when provider != OpenAI:

```rust
const CROSS_PROVIDER_SAFETY_MARGIN: f32 = 1.15; // 15% buffer

fn effective_max_tokens(max: usize, provider: Provider) -> usize {
    match provider {
        Provider::OpenAI => max,
        _ => (max as f32 / CROSS_PROVIDER_SAFETY_MARGIN) as usize,
    }
}
```

### Option C: API-Based Counting

Use provider APIs for accurate counts (latency cost):

- **Anthropic**: Use the official `POST /v1/messages/count_tokens` endpoint.
- **OpenAI**: Continue using `tiktoken` (local, already used).
- **Offline Alternative**: Community-maintained Rust libraries (e.g., `claude-tokenizer`) embed the tokenizer data for local execution.

## Affected Components

| File | Usage |
|------|-------|
| `tools/src/webfetch/chunker.rs` | Chunk token counting |
| `context/src/token_counter.rs` | Context budget tracking |
| `context/src/working_context.rs` | Budget allocation |
| `context/src/model_limits.rs` | Per-model token limits |

## Open Questions

1. ~Does Anthropic publish their tokenizer or provide a local library?~
   - **Answer**: Yes. Anthropic provides an official **Token Count API**, official Python/TS SDK methods, and an official beta npm package (`@anthropic-ai/tokenizer`). Additionally, community crates like `claude-tokenizer` enable local tokenization in Rust by embedding the tokenizer data.
2. What's the empirical divergence between `cl100k_base` and Claude's tokenizer?
   - **Note**: Research shows `o200k_base` is significantly more efficient for multilingual and code content compared to `cl100k_base`.
3. Should we cache API-based token counts to reduce latency?
4. ~Is 15% safety margin sufficient, or do we need per-model calibration?~
   - **Answer**: 15% is a safe "blind" default (covering Claude's historic ~5-10% undercount risk), but it is wasteful for GPT-5 (`o200k`), where `cl100k` already overestimates by ~20%. **Per-model calibration** (or per-tokenizer awareness) is the robust solution to avoid compounding inefficiencies.

## Related

- `docs/WEBFETCH_SRD.md` - FR-WF-14a notes this issue
- `docs/CONTEXT_INFINITY_SRD.md` - Context budget management
- `context/src/token_counter.rs` - Current token counting implementation

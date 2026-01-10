# Cross-Provider Token Counting

**Status:** Backlog  
**Priority:** Medium  
**Type:** Bug/Architecture  

## Summary

Token counting across providers is not 1:1. When switching between OpenAI and Claude (or other providers), token counts diverge, potentially causing context window overflow or inefficient budget usage.

## Problem Statement

The codebase currently uses OpenAI's `cl100k_base` tokenizer (via `tiktoken-rs`) for:

1. **WebFetch chunking** - `webfetch/chunker.rs` uses `cl100k_base` to split content into token-budgeted chunks
2. **Context window management** - Token budgets assume consistent tokenization

When the active provider is Claude (or another non-OpenAI model), the actual token consumption differs from the estimated count:

- Claude uses a different tokenizer (not publicly documented as of 2025)
- Same text produces different token counts across providers
- A "600 token" chunk for OpenAI may be 550 or 650 tokens for Claude

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
    OpenAI(CoreBPE),      // cl100k_base
    Claude(ClaudeTokenizer), // If/when available
    Approximate(f32),     // Fallback: chars * ratio
}

fn get_tokenizer(provider: Provider) -> Tokenizer {
    match provider {
        Provider::OpenAI => Tokenizer::OpenAI(cl100k_base()),
        Provider::Claude => Tokenizer::Claude(...), // or fallback
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

- Anthropic: `POST /v1/messages/count_tokens`
- OpenAI: `tiktoken` (local, already used)

## Affected Components

| File | Usage |
|------|-------|
| `tools/src/webfetch/chunker.rs` | Chunk token counting |
| `context/src/token_counter.rs` | Context budget tracking |
| `context/src/working_context.rs` | Budget allocation |
| `context/src/model_limits.rs` | Per-model token limits |

## Open Questions

1. Does Anthropic publish their tokenizer or provide a local library?
2. What's the empirical divergence between `cl100k_base` and Claude's tokenizer?
3. Should we cache API-based token counts to reduce latency?
4. Is 15% safety margin sufficient, or do we need per-model calibration?

## Related

- `docs/WEBFETCH_SRD.md` - FR-WF-14a notes this issue
- `docs/CONTEXT_INFINITY_SRD.md` - Context budget management
- `context/src/token_counter.rs` - Current token counting implementation

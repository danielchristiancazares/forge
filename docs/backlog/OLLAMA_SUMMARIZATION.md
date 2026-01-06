# Ollama Summarization Backend

**Status:** Backlog  
**Priority:** Medium  
**Type:** Feature  

## Summary

Add Ollama as a summarization provider for The Librarian, enabling local LLM summarization without requiring cloud API keys.

## Motivation

Currently, summarization requires either an Anthropic or OpenAI API key (using Haiku or GPT-4o-mini). Adding Ollama support would:

1. Eliminate API key requirement for summarization
2. Enable offline operation
3. Zero per-token cost
4. Avoid provider mismatch bugs when user switches providers mid-conversation

## Requirements

### Hard Constraints (based on Passes Monkey research)

- **Minimum 8B parameters** - Sub-8B models exhibit categorical fidelity failures
- **Minimum Q4 quantization** - Q2/Q3 quantization causes structural instability (QDT-C/D)
- **Token budget 1024-2048** - Reasoning models degrade with excessive runway

### Empirical Basis

From `passes-monkey` evaluation:

| Model | Params | Quant | Categorical | Boundary | Viable |
|-------|--------|-------|-------------|----------|--------|
| tinyllama | 1.1B | Q4_K_M | FAIL | FAIL | ❌ |
| deepseek-r1 | 1.15B | Q4_K_M | FAIL | FAIL | ❌ |
| phi3:mini | 3.8B | Q4_K_M | FAIL | PASS | ❌ |
| deepseek-r1 | 7B | Q4_K_M | degraded | - | ❌ |
| deepseek-r1 | 8B | Q4_K_M | PASS | PASS | ✅ |
| gemma2 | 9B | Q4_K_M | PASS | FAIL | ⚠️ |

Note: 7B→8B boundary is significant for deepseek-r1; response fidelity degrades at 7B.

## Proposed Config

```toml
[summarization]
provider = "ollama"           # "claude" | "openai" | "ollama"
endpoint = "http://localhost:11434"
model = "deepseek-r1:8b"      # Must be ≥8B, ≥Q4
```

## Implementation Plan

1. **Add `SummarizationProvider` enum** - `Claude | OpenAI | Ollama`
2. **Ollama client** - Hit `/api/generate` or `/api/chat` endpoint
3. **Model validation** - Query `/api/show` to verify:
   - `parameter_size >= 8B`
   - `quantization_level >= Q4`
4. **Config parsing** - New `[summarization]` section
5. **Fallback behavior** - If Ollama unavailable, warn and disable context infinity

## API Reference

Ollama endpoints:
- `POST /api/generate` - Raw completion
- `POST /api/chat` - Chat completion (OpenAI-compatible structure)
- `POST /api/show` - Model metadata (for validation)

## Open Questions

- Should we bundle a recommended model list?
- Timeout handling for slower local inference?
- GPU vs CPU detection for realistic timeout defaults?

## Related

- `docs/CONTEXT_ARCHITECTURE.md` - The Librarian design
- `context/src/summarization.rs` - Current implementation
- `../passes-monkey/passes_monkey_paper_draft.md` - Empirical research

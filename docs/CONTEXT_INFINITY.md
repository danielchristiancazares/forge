# Context Infinity: Adaptive Context Window Management

Context Infinity is Forge's system for managing unlimited conversation context with LLMs. It preserves complete conversation history while automatically summarizing older content to fit within model-specific token limits.

## Overview

The core principle is **never discard, always compress**: messages are never deleted from history. Instead, when the context window fills up, older messages are summarized into compact representations that preserve essential information.

```
                    ContextManager
                          |
     +--------------------+--------------------+
     |                    |                    |
FullHistory          TokenCounter        ModelRegistry
(append-only)        (tiktoken)          (limits/model)
     |
     v
WorkingContext -----> API Messages
(derived view)
```

## Architecture

### Component Overview

| Component | Purpose |
|-----------|---------|
| `ContextManager` | Orchestrates all context management operations |
| `FullHistory` | Append-only storage for messages and summaries |
| `TokenCounter` | Accurate token counting via tiktoken (cl100k_base) |
| `ModelRegistry` | Model-specific token limits with prefix matching |
| `WorkingContext` | Derived view of what to send to the API |
| `StreamJournal` | SQLite-backed crash recovery for streaming responses |

### Directory Structure

```
crates/forge-context/src/
  lib.rs              # Module exports and public API
  manager.rs          # ContextManager - main orchestrator
  history.rs          # FullHistory, MessageId, SummaryId, Summary
  model_limits.rs     # ModelLimits, ModelRegistry
  token_counter.rs    # TokenCounter (tiktoken wrapper)
  working_context.rs  # WorkingContext, ContextSegment, ContextUsage
  stream_journal.rs   # StreamJournal, ActiveJournal (crash recovery)
  summarization.rs    # LLM-based summarization via cheaper models
```

## Core Concepts

### Full History (Append-Only Storage)

History is **never truncated**. Every message is stored with:

- `MessageId`: Monotonically increasing identifier (0, 1, 2, ...)
- `Message`: The actual content (User, Assistant, or System)
- `token_count`: Cached token count for the message
- `summary_id`: Optional link to a summary that covers this message

```rust
pub enum HistoryEntry {
    Original {
        id: MessageId,
        message: Message,
        token_count: u32,
        created_at: SystemTime,
    },
    Summarized {
        id: MessageId,
        message: Message,
        token_count: u32,
        summary_id: SummaryId,  // Links to the covering summary
        created_at: SystemTime,
    },
}
```

When messages are summarized, they transition from `Original` to `Summarized` but remain in history. The original content is always accessible.

### Summaries

A `Summary` represents a compressed version of a contiguous range of messages:

```rust
pub struct Summary {
    id: SummaryId,
    covers: Range<MessageId>,      // [start, end) of messages covered
    content: NonEmptyString,       // The summarized text
    token_count: u32,              // Tokens in the summary
    original_tokens: u32,          // Tokens in original messages
    created_at: SystemTime,
    generated_by: String,          // Model that generated this
}
```

Key invariant: Summaries must cover **contiguous** message ranges. Non-contiguous summarization is not supported to maintain chronological coherence.

### Working Context (Derived View)

The `WorkingContext` is rebuilt on-demand and represents what will actually be sent to the LLM API. It mixes:

1. **Original messages** - sent verbatim
2. **Summaries** - injected as system messages with `[Earlier conversation summary]` prefix

```rust
pub enum ContextSegment {
    Original { id: MessageId, tokens: u32 },
    Summarized {
        summary_id: SummaryId,
        replaces: Vec<MessageId>,  // Original IDs this replaces
        tokens: u32,
    },
}
```

The working context is **ephemeral** - it's computed from history and the current model's budget, then materialized into API messages.

## Token Budget Calculation

### Model Limits

Each model has defined limits stored in `ModelRegistry`:

```rust
pub struct ModelLimits {
    context_window: u32,  // Total context capacity
    max_output: u32,      // Reserved for model output
}
```

The **effective input budget** is calculated as:

```
effective_budget = context_window - max_output - (5% safety margin)
```

Example for Claude Sonnet 4 (200k context, 64k output):
```
available = 200,000 - 64,000 = 136,000
safety_margin = 136,000 / 20 = 6,800
effective_budget = 136,000 - 6,800 = 129,200 tokens
```

The 5% safety margin accounts for:
- Token counting inaccuracies
- System prompt overhead
- Tool definitions and formatting

### Known Model Limits

| Model Prefix | Context Window | Max Output |
|--------------|---------------|------------|
| `claude-opus-4` | 200,000 | 64,000 |
| `claude-sonnet-4` | 200,000 | 64,000 |
| `claude-3-5` | 200,000 | 64,000 |
| `claude-3` | 200,000 | 64,000 |
| `gpt-4o` | 128,000 | 16,384 |
| `gpt-4-turbo` | 128,000 | 4,096 |
| `gpt-4` | 8,192 | 4,096 |
| `gpt-3.5` | 16,385 | 4,096 |
| Unknown | 8,192 | 4,096 |

Model lookup uses **prefix matching** - `claude-sonnet-4-20250514` matches `claude-sonnet-4`.

## Context Building Algorithm

The `build_working_context()` algorithm runs in five phases:

### Phase 1: Reserve Recent Messages

The N most recent messages (default: 4) are **always included**. These represent the immediate conversation context and are never summarized.

```rust
let preserve_count = self.summarization_config.preserve_recent; // 4
let recent_start = entries.len().saturating_sub(preserve_count);
let tokens_for_recent: u32 = entries[recent_start..].iter()
    .map(|e| e.token_count())
    .sum();
```

If recent messages alone exceed the budget, summarization fails with an error.

### Phase 2: Partition Older Messages into Blocks

Older messages are grouped into contiguous blocks:

- **Unsummarized Block**: Consecutive messages with no summary
- **Summarized Block**: Consecutive messages covered by the same summary

```rust
enum Block {
    Unsummarized(Vec<(MessageId, u32)>),
    Summarized {
        summary_id: SummaryId,
        messages: Vec<(MessageId, u32)>,
        summary_tokens: u32,
    },
}
```

### Phase 3: Select Content (Newest to Oldest)

Starting from the most recent older block, include content while staying within budget:

```
remaining_budget = effective_budget - tokens_for_recent
```

For each block (newest first):

1. **Summarized Block**: 
   - If original messages fit: include originals (better quality)
   - Else if summary fits: include summary
   - Else: skip (will need re-summarization)

2. **Unsummarized Block**:
   - Include as many recent messages as fit
   - Mark the rest as needing summarization

### Phase 4: Assemble Working Context

Selected segments are arranged in chronological order:

```
[Older summaries/messages] -> [Recent messages always included]
```

### Phase 5: Return or Request Summarization

If all content fits: return `Ok(WorkingContext)`

If unsummarized messages don't fit:
```rust
Err(SummarizationNeeded {
    excess_tokens: u32,
    messages_to_summarize: Vec<MessageId>,
    suggestion: String,
})
```

## When Summarization Triggers

Summarization is triggered when:

1. **Context budget exceeded**: `build_working_context()` returns `SummarizationNeeded`
2. **Model switch to smaller context**: Switching from 200k to 8k model
3. **Manual request**: User invokes `:summarize` command

The decision flow:

```
push_message() -> usage_status()
                      |
                      v
               +------+------+
               |             |
          Ready(usage)   NeedsSummarization
               |             |
               v             v
          Continue      prepare_summarization()
                             |
                             v
                    PendingSummarization
                             |
                             v
                    generate_summary() [async]
                             |
                             v
                    complete_summarization()
```

## Summarization Process

### Configuration

```rust
pub struct SummarizationConfig {
    pub target_ratio: f32,      // 0.15 = compress to 15% of original
    pub preserve_recent: usize, // 4 = never summarize last 4 messages
}
```

### Prepare Summarization

```rust
pub fn prepare_summarization(&mut self, message_ids: &[MessageId]) 
    -> Option<PendingSummarization>
```

1. Sort and deduplicate message IDs
2. Extract first contiguous run (summaries must be contiguous)
3. Calculate target tokens: `original_tokens * target_ratio`
4. Allocate a `SummaryId`
5. Return `PendingSummarization` with messages to summarize

### Generate Summary (Async)

Summarization uses cheaper/faster models:
- **Claude**: `claude-3-haiku-20240307`
- **OpenAI**: `gpt-4o-mini`

The prompt instructs the model to:
- Preserve key facts, decisions, and important context
- Maintain chronological flow
- Stay within target token count
- Use clear, direct language
- Preserve essential code snippets and file paths
- Note unresolved questions or pending actions

### Complete Summarization

```rust
pub fn complete_summarization(
    &mut self,
    summary_id: SummaryId,
    scope: SummarizationScope,
    content: NonEmptyString,
    generated_by: String,
)
```

1. Count tokens in the generated summary
2. Create `Summary` with metadata
3. Add to history
4. Mark covered messages as `Summarized`

## Model Switching (Context Adaptation)

When switching models, the context manager adapts:

```rust
pub enum ContextAdaptation {
    NoChange,
    Shrinking {
        old_budget: u32,
        new_budget: u32,
        needs_summarization: bool,
    },
    Expanding {
        old_budget: u32,
        new_budget: u32,
        can_restore: usize,  // Messages that could use originals
    },
}
```

### Shrinking (e.g., Claude 200k -> GPT-4 8k)

If current context exceeds new budget, `needs_summarization` is true. The app should trigger summarization before the next API call.

### Expanding (e.g., GPT-4 8k -> Claude 200k)

Previously summarized messages can be restored to their originals. The `try_restore_messages()` method returns how many messages would use originals in the new budget.

This is **automatic** - no re-summarization needed. The working context builder prefers originals when budget allows.

## Stream Journal (Crash Recovery)

The `StreamJournal` ensures streaming responses survive crashes using SQLite WAL mode.

### Key Invariant

**Deltas MUST be persisted BEFORE being displayed to the user.**

This write-ahead logging approach guarantees that after a crash, partial responses can be recovered.

### Schema

```sql
CREATE TABLE stream_journal (
    step_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,  -- 'text_delta', 'done', 'error'
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    sealed INTEGER DEFAULT 0,
    PRIMARY KEY(step_id, seq)
);
```

### Lifecycle

1. **Begin Session**: `begin_session()` -> `ActiveJournal`
2. **Append Events**: `append_text()`, `append_done()`, `append_error()`
3. **Seal**: `seal()` -> marks entries complete, returns accumulated text
4. **Discard**: `discard()` -> removes unsealed entries

### Recovery

On startup, check for unsealed entries:

```rust
pub fn recover(&self) -> Option<RecoveredStream>

pub enum RecoveredStream {
    Complete {      // Has 'done' or 'error' event
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
    },
    Incomplete {    // Stream was interrupted mid-flight
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
    },
}
```

The app can then:
- **Complete**: Seal and use the recovered text
- **Incomplete**: Discard and retry, or seal what was received

### RAII Pattern

`ActiveJournal` is a proof token that a stream is in-flight:

```rust
pub struct ActiveJournal {
    journal_id: u64,
    step_id: StepId,
    next_seq: u64,
}
```

Methods like `append_text()` require `&mut ActiveJournal`, ensuring:
- Only one stream can be active at a time
- Events are properly sequenced
- The journal cannot be used incorrectly

## Token Counting

Token counting uses tiktoken's `cl100k_base` encoding, compatible with GPT-4, GPT-3.5, and Claude models.

### Implementation

```rust
pub struct TokenCounter {
    encoder: &'static CoreBPE,  // Singleton, initialized once
}

impl TokenCounter {
    pub fn count_str(&self, text: &str) -> u32;
    pub fn count_message(&self, msg: &Message) -> u32;
}
```

Per-message overhead: **~4 tokens** for role markers and formatting.

### Efficiency

The tiktoken encoder is expensive to initialize. `TokenCounter` uses a singleton pattern:

```rust
static ENCODER: OnceLock<CoreBPE> = OnceLock::new();

fn get_encoder() -> &'static CoreBPE {
    ENCODER.get_or_init(|| cl100k_base().expect("..."))
}
```

Creating multiple `TokenCounter` instances is cheap - they share the encoder.

## Usage Statistics

The `ContextUsage` struct provides UI-friendly statistics:

```rust
pub struct ContextUsage {
    pub used_tokens: u32,
    pub budget_tokens: u32,
    pub summarized_segments: usize,
}
```

### Display Format

```rust
usage.format_compact() // "2.1k / 200k (1%)" or "50k / 200k (25%) [2S]"
```

### Severity Levels

```rust
pub fn severity(&self) -> u8 {
    // 0 = green (< 70%)
    // 1 = yellow (70-90%)
    // 2 = red (> 90%)
}
```

## Persistence

### History Serialization

```rust
// Save
context_manager.save("~/.forge/history.json")?;

// Load
let manager = ContextManager::load("~/.forge/history.json", "claude-sonnet-4")?;
```

The serialization format validates:
- Message IDs are sequential (0, 1, 2, ...)
- Summary IDs are sequential
- Summary ranges reference valid messages
- Summarized messages reference valid summaries

### Stream Journal Location

```
~/.forge/stream_journal.db
```

## Configuration

Enable/disable via `~/.forge/config.toml`:

```toml
[context]
infinity = true  # Enable adaptive context management
```

Or environment variable:
```bash
FORGE_CONTEXT_INFINITY=1  # Enable
FORGE_CONTEXT_INFINITY=0  # Disable
```

## Type-Driven Design

The system follows Forge's type-driven philosophy (see `docs/DESIGN.md`):

| Type | Purpose |
|------|---------|
| `MessageId` | Proof of message existence in history |
| `SummaryId` | Proof of summary existence |
| `ActiveJournal` | Proof that a stream is in-flight (RAII) |
| `PreparedContext` | Proof that context fits within budget |
| `SummarizationNeeded` | Explicit error requiring caller action |
| `NonEmptyString` | Message content guaranteed non-empty |

### PreparedContext as Proof

The `prepare()` method returns a proof that context is ready:

```rust
pub fn prepare(&self) -> Result<PreparedContext<'_>, SummarizationNeeded>
```

`PreparedContext` can only be created if the working context fits within budget. Callers cannot accidentally send over-budget context to the API.

## Extension Points

### Adding a New Provider

1. Add model limits to `KNOWN_MODELS` in `model_limits.rs`
2. Add summarization model in `summarization.rs`
3. Implement `generate_summary_*` for the new provider

### Adjusting Summarization Behavior

Modify `SummarizationConfig`:

```rust
SummarizationConfig {
    target_ratio: 0.10,   // More aggressive compression
    preserve_recent: 6,   // Keep more recent messages
}
```

### Custom Token Counting

Replace `TokenCounter` with a model-specific counter if needed. The interface is simple:

```rust
fn count_str(&self, text: &str) -> u32;
fn count_message(&self, msg: &Message) -> u32;
```

## Limitations

1. **Summarization requires API call**: Summarization uses LLM calls (claude-3-haiku or gpt-4o-mini), adding latency and cost.

2. **Contiguous ranges only**: Summaries must cover contiguous message ranges. Selective summarization is not supported.

3. **Token counting approximation**: The cl100k_base encoding is accurate for GPT models but approximate for Claude. The 5% safety margin compensates.

4. **No streaming summarization**: Summaries are generated with non-streaming API calls.

5. **Single summary per range**: A message range can only have one summary. Re-summarization replaces the existing summary.

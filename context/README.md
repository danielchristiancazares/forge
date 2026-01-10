# forge-context

> Note: This is an implementation-focused overview of the current summarization system; the authoritative spec is `docs/CONTEXT_INFINITY_SRD.md`.

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

## Design Principles

1. **Append-only history**: Messages are never deleted. Summaries link to original messages, enabling restoration when switching to models with larger context windows.

2. **Type-driven correctness**: `PreparedContext` serves as a proof token that context was successfully built within budget before an API call.

3. **Explicit summarization**: The manager signals when summarization is needed rather than silently truncating. Callers control when and how summarization occurs.

4. **Write-ahead durability**: Stream deltas are persisted to SQLite before display, ensuring recoverability after crashes.

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
context/src/
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
| `claude-opus-4-5` | 200,000 | 64,000 |
| `claude-sonnet-4-5` | 200,000 | 64,000 |
| `claude-haiku-4-5` | 200,000 | 64,000 |
| `gpt-5.2` | 400,000 | 128,000 |
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

```text
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

```text
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

---

## Public API

### Core Types

#### `ContextManager`

The main orchestrator for context management.

```rust
use forge_context::{ContextManager, PreparedContext, SummarizationNeeded};

// Create a manager for a specific model
let mut manager = ContextManager::new("claude-sonnet-4-20250514");

// Add messages to history
let msg_id = manager.push_message(Message::try_user("Hello!")?);

// Switch models (triggers adaptation logic)
match manager.switch_model("gpt-4") {
    ContextAdaptation::Shrinking { needs_summarization, .. } => {
        if needs_summarization {
            // Handle summarization requirement
        }
    }
    ContextAdaptation::Expanding { can_restore, .. } => {
        // More context available; can restore summarized messages
    }
    ContextAdaptation::NoChange => {}
}

// Build working context for API call
match manager.prepare() {
    Ok(prepared) => {
        let messages = prepared.api_messages();
        let usage = prepared.usage();
        // Make API call with messages...
    }
    Err(SummarizationNeeded { messages_to_summarize, .. }) => {
        // Must summarize before proceeding
    }
}
```

**Key methods:**

| Method | Description |
|--------|-------------|
| `new(model)` | Create manager for initial model |
| `push_message(msg)` | Add message, returns `MessageId` |
| `switch_model(name)` | Change model, returns `ContextAdaptation` |
| `prepare()` | Build context proof or signal summarization needed |
| `prepare_summarization(ids)` | Create async summarization request |
| `complete_summarization(...)` | Apply generated summary to history |
| `usage_status()` | Get current usage with explicit status |
| `save(path)` / `load(path, model)` | Persistence |

#### `PreparedContext<'a>`

Proof that working context was successfully built within token budget. Borrowing the manager ensures the context remains valid.

```rust
let prepared: PreparedContext = manager.prepare()?;

// Get messages formatted for API
let api_messages: Vec<Message> = prepared.api_messages();

// Get usage statistics for UI
let usage: ContextUsage = prepared.usage();
println!("Using {} of {} tokens", usage.used_tokens, usage.budget_tokens);
```

#### `ContextAdaptation`

Result of switching models, indicating required actions:

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
        can_restore: usize,  // Messages that could be restored
    },
}
```

#### `SummarizationNeeded`

Error type indicating summarization is required:

```rust
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}
```

### History Types

#### `FullHistory`

Append-only storage for all conversation messages and summaries.

```rust
use forge_context::{FullHistory, MessageId, SummaryId};

let mut history = FullHistory::new();

// Add messages
let id: MessageId = history.push(message, token_count);

// Access entries
let entry = history.get_entry(id);
println!("Content: {}", entry.message().content());
println!("Tokens: {}", entry.token_count());
println!("Summarized: {}", entry.is_summarized());

// Statistics
println!("Total messages: {}", history.len());
println!("Total tokens: {}", history.total_tokens());
println!("Summarized count: {}", history.summarized_count());
```

#### `HistoryEntry`

A message with cached metadata:

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
        summary_id: SummaryId,
        created_at: SystemTime,
    },
}
```

#### `Summary`

Represents compressed conversation segments:

```rust
pub struct Summary {
    id: SummaryId,
    covers: Range<MessageId>,  // [start, end) of original messages
    content: NonEmptyString,
    token_count: u32,
    original_tokens: u32,      // For compression ratio tracking
    created_at: SystemTime,
    generated_by: String,      // Model that created summary
}
```

### Model Limits

#### `ModelRegistry`

Registry with prefix-based model lookup:

```rust
use forge_context::{ModelRegistry, ModelLimits, ResolvedModelLimits};

let registry = ModelRegistry::new();

// Lookup by exact name or prefix
let resolved: ResolvedModelLimits = registry.get("claude-sonnet-4-20250514");

match resolved.source() {
    ModelLimitsSource::Prefix("claude-sonnet-4") => { /* matched prefix */ }
    ModelLimitsSource::DefaultFallback => { /* unknown model */ }
    ModelLimitsSource::Override => { /* custom override */ }
}

let limits: ModelLimits = resolved.limits();
println!("Context window: {}", limits.context_window());
println!("Max output: {}", limits.max_output());
println!("Effective input budget: {}", limits.effective_input_budget());
```

**Known model prefixes:**

| Prefix | Context Window | Max Output |
|--------|---------------|------------|
| `claude-opus-4-5` | 200,000 | 64,000 |
| `claude-sonnet-4-5` | 200,000 | 64,000 |
| `claude-haiku-4-5` | 200,000 | 64,000 |
| `gpt-5.2` | 400,000 | 128,000 |

Unknown models fall back to 8,192 context / 4,096 output.

#### `ModelLimits`

Token constraints for a model:

```rust
let limits = ModelLimits::new(200_000, 16_000);

// Effective budget = context_window - max_output - 5% safety margin
let budget = limits.effective_input_budget();
// 200,000 - 16,000 = 184,000
// 184,000 * 0.95 = 174,800
```

The 5% safety margin accounts for token counting inaccuracies and overhead from system prompts, formatting, and tool definitions.

### Token Counting

#### `TokenCounter`

Accurate token counting using tiktoken's cl100k_base encoding:

```rust
use forge_context::TokenCounter;

let counter = TokenCounter::new();  // Cheap: uses singleton encoder

// Count string tokens
let tokens = counter.count_str("Hello, world!");

// Count message tokens (includes ~4 token overhead for role/formatting)
let msg = Message::try_user("What is Rust?")?;
let msg_tokens = counter.count_message(&msg);
```

### Stream Journal (Crash Recovery)

#### `StreamJournal`

SQLite-backed write-ahead log for streaming durability:

```rust
use forge_context::{StreamJournal, ActiveJournal, RecoveredStream};

// Open or create journal
let mut journal = StreamJournal::open("~/.forge/stream.db")?;

// Check for crash recovery on startup
if let Some(recovered) = journal.recover() {
    match recovered {
        RecoveredStream::Complete { partial_text, step_id, .. } => {
            // Stream finished but wasn't sealed
            journal.seal_unsealed(step_id)?;
        }
        RecoveredStream::Incomplete { partial_text, step_id, .. } => {
            // Stream was interrupted mid-flight
            // Option 1: Discard and retry
            journal.discard_unsealed(step_id)?;
            // Option 2: Resume from partial_text
        }
    }
}

// Begin streaming session
let mut active: ActiveJournal = journal.begin_session()?;

// Persist each delta BEFORE displaying to user
active.append_text(&mut journal, "Hello")?;
active.append_text(&mut journal, " world")?;
active.append_done(&mut journal)?;

// Seal when complete (marks entries as committed)
let full_text: String = active.seal(&mut journal)?;
```

**Key invariant:** Deltas must be persisted before display. This write-ahead approach ensures durability at the cost of slightly higher latency per delta.

#### `ActiveJournal`

RAII handle proving a stream is in-flight:

```rust
impl ActiveJournal {
    fn step_id(&self) -> StepId;
    fn append_text(&mut self, journal: &mut StreamJournal, content: impl Into<String>) -> Result<()>;
    fn append_done(&mut self, journal: &mut StreamJournal) -> Result<()>;
    fn append_error(&mut self, journal: &mut StreamJournal, message: impl Into<String>) -> Result<()>;
    fn seal(self, journal: &mut StreamJournal) -> Result<String>;
    fn discard(self, journal: &mut StreamJournal) -> Result<u64>;
}
```

#### `RecoveredStream`

Recovery state after crash:

```rust
pub enum RecoveredStream {
    Complete {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
    },
    Incomplete {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
    },
}
```

### Tool Journal (Tool Batch Durability)

The `ToolJournal` provides durable tracking for tool batches, enabling crash recovery when tool execution is interrupted.

#### `ToolJournal`

SQLite-backed journal for tool batch durability:

```rust
use forge_context::{ToolJournal, RecoveredToolBatch, ToolBatchId};
use forge_types::{ToolCall, ToolResult};

// Open or create tool journal
let mut journal = ToolJournal::open("~/.forge/tool_journal.db")?;

// Check for crash recovery on startup
if let Some(recovered) = journal.recover()? {
    println!("Recovered batch {} with {} calls, {} results",
        recovered.batch_id,
        recovered.calls.len(),
        recovered.results.len(),
    );
    // User can resume or discard
    journal.discard_batch(recovered.batch_id)?;
}

// Begin a new tool batch
let calls = vec![ToolCall::new("call_1", "read_file", json!({"path": "foo.rs"}))];
let batch_id: ToolBatchId = journal.begin_batch("claude-sonnet-4", "assistant text", &calls)?;

// Record results as tools execute
let result = ToolResult::success("call_1", "file contents...");
journal.record_result(batch_id, &result)?;

// Commit when complete (prunes batch data)
journal.commit_batch(batch_id)?;
```

**Key invariant:** Only one uncommitted batch can exist at a time. Tool calls and results are persisted immediately, enabling recovery of partial batches after crashes.

#### `RecoveredToolBatch`

Data recovered from an incomplete tool batch:

```rust
pub struct RecoveredToolBatch {
    pub batch_id: ToolBatchId,
    pub model_name: String,
    pub assistant_text: String,
    pub calls: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
}
```

#### Streaming Batch Support

For tool batches created during streaming (before arguments are complete):

```rust
// Begin streaming batch with empty calls
let batch_id = journal.begin_streaming_batch("claude-sonnet-4")?;

// Record call start as stream events arrive
journal.record_call_start(batch_id, 0, "call_1", "read_file")?;

// Append arguments as they stream in
journal.append_call_args(batch_id, "call_1", r#"{"path":"#)?;
journal.append_call_args(batch_id, "call_1", r#""foo.rs"}"#)?;

// Update assistant text
journal.update_assistant_text(batch_id, "I'll read that file...")?;
```

### Summarization

#### `generate_summary`

Async function to generate summaries via LLM:

```rust
use forge_context::{generate_summary, summarization_model};
use forge_providers::ApiConfig;

// Get the summarization model for current provider
let model_name = summarization_model(Provider::Claude);
// Returns "claude-3-haiku-20240307" (cheaper/faster)

// Generate summary
let summary_text = generate_summary(
    &api_config,
    &messages_to_summarize,  // Vec<(MessageId, Message)>
    target_tokens,           // Target size for summary
).await?;
```

**Summarization models used:**

- Claude: `claude-3-haiku-20240307`
- OpenAI: `gpt-4o-mini`

### Working Context

#### `WorkingContext`

Internal representation of what will be sent to the API:

```rust
pub struct WorkingContext {
    segments: Vec<ContextSegment>,
    token_budget: u32,
}
```

#### `ContextSegment`

A piece of the working context:

```rust
pub enum ContextSegment {
    Original { id: MessageId, tokens: u32 },
    Summarized {
        summary_id: SummaryId,
        replaces: Vec<MessageId>,
        tokens: u32,
    },
}
```

#### `ContextUsage`

Statistics for UI display:

```rust
pub struct ContextUsage {
    pub used_tokens: u32,
    pub budget_tokens: u32,
    pub summarized_segments: usize,
}

impl ContextUsage {
    fn percentage(&self) -> f32;           // 0.0 - 100.0
    fn format_compact(&self) -> String;    // "2.1k / 200k (1%)" or "50k / 200k (25%) [2S]"
    fn severity(&self) -> u8;              // 0=green, 1=yellow, 2=red
}
```

## Complete Workflow Example

```rust
use forge_context::{
    ContextManager, PreparedContext, SummarizationNeeded,
    StreamJournal, ActiveJournal, generate_summary,
};
use forge_types::Message;

// Initialize
let mut manager = ContextManager::new("claude-sonnet-4");
let mut journal = StreamJournal::open("~/.forge/journal.db")?;

// Handle crash recovery
if let Some(recovered) = journal.recover() {
    // ... handle recovery ...
}

// Add user message
manager.push_message(Message::try_user("Explain Rust lifetimes")?);

// Prepare context
let prepared = match manager.prepare() {
    Ok(p) => p,
    Err(SummarizationNeeded { messages_to_summarize, .. }) => {
        // Summarization needed - handle async
        let pending = manager.prepare_summarization(&messages_to_summarize)
            .expect("messages exist");
        
        let summary_text = generate_summary(
            &api_config,
            &pending.messages,
            pending.target_tokens,
        ).await?;
        
        manager.complete_summarization(
            pending.summary_id,
            pending.scope,
            NonEmptyString::new(&summary_text)?,
            "claude-3-haiku".to_string(),
        );
        
        manager.prepare()?  // Should succeed now
    }
};

// Make API call with streaming
let api_messages = prepared.api_messages();
let mut active = journal.begin_session()?;

for chunk in stream_response(&api_messages).await {
    active.append_text(&mut journal, &chunk)?;  // Persist first
    display_to_user(&chunk);                     // Then display
}

active.append_done(&mut journal)?;
let full_response = active.seal(&mut journal)?;

// Add assistant response to history
manager.push_message(Message::assistant(NonEmptyString::new(&full_response)?));

// Persist conversation
manager.save("~/.forge/history.json")?;
```

## Type Relationships

```
MessageId -----> HistoryEntry -----> Message
     |               |
     |               v
     +--------> SummaryId -----> Summary
                    |
                    v
              ContextSegment
                    |
                    v
              WorkingContext
                    |
                    v
              PreparedContext --> api_messages() --> Vec<Message>
```

## Error Handling

The crate uses `anyhow::Result` for most fallible operations and custom error types for domain-specific failures:

- `SummarizationNeeded`: Returned when context exceeds budget and summarization is required
- `anyhow::Error`: Used for I/O, database, and API errors

## Dependencies

- `forge-types`: Core types (`Message`, `NonEmptyString`, `Provider`)
- `forge-providers`: API configuration (`ApiConfig`)
- `tiktoken-rs`: Token counting
- `rusqlite`: Stream journal persistence
- `reqwest`: HTTP client for summarization API calls
- `serde`/`serde_json`: Serialization

## Testing

```bash
cargo test -p forge-context           # Run all tests
cargo test -p forge-context -- --nocapture  # With output
```

The crate includes comprehensive unit tests for all modules. Integration tests requiring API keys are marked with `#[ignore]`.

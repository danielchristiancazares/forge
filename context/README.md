# forge-context

> Note: This is an implementation-focused overview of the current summarization system; the authoritative spec is `docs/CONTEXT_INFINITY_SRD.md`.

Context Infinity™ is Forge's system for managing unlimited conversation context with LLMs. It preserves complete conversation history while automatically summarizing older content to fit within model-specific token limits.

## LLM-TOC
<!-- toc:start -->
| Lines | Section |
| --- | --- |
| 7-38 | LLM-TOC |
| 39-55 | Overview |
| 56-65 | Design Principles |
| 66-97 | Architecture |
| 98-166 | Core Concepts |
| 167-229 | Token Budget Calculation |
| 230-320 | Context Building Algorithm |
| 321-351 | When Summarization Triggers |
| 352-409 | Summarization Process |
| 410-439 | Model Switching (Context Adaptation) |
| 440-545 | Stream Journal (Crash Recovery) |
| 546-590 | Token Counting |
| 591-618 | Usage Statistics |
| 619-643 | Persistence |
| 644-659 | Configuration |
| 660-682 | Type-Driven Design |
| 683-710 | Extension Points |
| 711-728 | Limitations |
| 729-825 | The Librarian |
| 826-885 | Fact Store |
| 886-1605 | Public API |
| 1606-1702 | Complete Workflow Example |
| 1703-1720 | Type Relationships |
| 1721-1727 | Error Handling |
| 1728-1736 | Dependencies |
| 1737-1744 | Testing |
<!-- toc:end -->

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
| `Librarian` | Intelligent fact extraction and retrieval using Gemini Flash |
| `FactStore` | SQLite-backed persistent storage for extracted facts |

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
  tool_journal.rs     # ToolJournal (tool batch crash recovery)
  librarian.rs        # Librarian - intelligent fact extraction/retrieval
  fact_store.rs       # FactStore - SQLite persistence for facts
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

Example for Claude Opus 4.5 (200k context, 64k output):

```
available = 200,000 - 64,000 = 136,000
safety_margin = 136,000 / 20 = 6,800
effective_budget = 136,000 - 6,800 = 129,200 tokens
```

The 5% safety margin accounts for:

- Token counting inaccuracies (see [Token Counting Accuracy](#token-counting-accuracy))
- System prompt overhead
- Tool definitions and formatting

### Configured Output Limit

When a user configures a smaller output limit than the model's maximum, more tokens become available for input context. Use `set_output_limit()` to adjust:

```rust
let mut manager = ContextManager::new("claude-opus-4-5-20251101");

// Model has 64k max output, but user configured 16k
manager.set_output_limit(16_000);

// Now effective budget is:
// 200,000 - 16,000 = 184,000 available
// 184,000 * 0.95 = 174,800 effective budget (vs 129,200 without config)
```

The reserved output is clamped to the model's `max_output` - requesting more than the model supports has no effect.

### Known Model Limits

| Model Prefix | Context Window | Max Output |
|--------------|---------------|------------|
| `claude-opus-4-5` | 200,000 | 64,000 |
| `claude-haiku-4-5` | 200,000 | 64,000 |
| `gpt-5.2` | 400,000 | 128,000 |
| `gemini-3-pro` | 1,048,576 | 65,536 |
| Unknown | 8,192 | 4,096 |

Model lookup uses **prefix matching** - `claude-opus-4-5-20251101` matches `claude-opus-4-5`.

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
Err(ContextBuildError::SummarizationNeeded(SummarizationNeeded {
    excess_tokens: u32,
    messages_to_summarize: Vec<MessageId>,
    suggestion: String,
}))
```

### Error: Recent Messages Too Large

If the N most recent messages alone exceed the budget, summarization cannot help. This is an unrecoverable error:

```rust
Err(ContextBuildError::RecentMessagesTooLarge {
    required_tokens: u32,  // Tokens needed for recent messages
    budget_tokens: u32,    // Available budget
    message_count: usize,  // Number of recent messages
})
```

The user must either reduce their input or switch to a model with a larger context window.

## When Summarization Triggers

Summarization is triggered when:

1. **Context budget exceeded**: `build_working_context()` returns `SummarizationNeeded`
2. **Model switch to smaller context**: Switching from 200k to 8k model
3. **Manual request**: User invokes `/summarize` command

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

- **Claude**: `claude-haiku-4-5`
- **OpenAI**: `gpt-5-nano`
- **Gemini**: `gemini-3-pro-preview`

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

On startup, check for recoverable entries (unsealed OR sealed but uncommitted):

```rust
pub fn recover(&self) -> Result<Option<RecoveredStream>>

pub enum RecoveredStream {
    Complete {      // Has 'done' event, stream finished cleanly
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        model_name: Option<String>,  // For attribution
    },
    Errored {       // Has 'error' event, stream failed
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        error: String,
        model_name: Option<String>,
    },
    Incomplete {    // Stream was interrupted mid-flight
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        model_name: Option<String>,
    },
}
```

The app can then:

- **Complete**: Commit to history, then `commit_and_prune_step()`
- **Errored**: Log the error, then `discard_step()`
- **Incomplete**: Discard and retry, or commit partial text

### Commit-and-Prune Flow

After sealing a stream, its data remains in the journal until explicitly pruned. This enables crash recovery even after sealing:

```rust
// 1. Stream completes
active.append_done(&mut journal)?;
let text = active.seal(&mut journal)?;

// 2. Save to history (must succeed before pruning)
let step_id = active.step_id();
manager.push_message_with_step_id(Message::assistant(...), step_id);
manager.save("~/.forge/history.json")?;

// 3. ONLY after history is persisted, prune the journal
journal.commit_and_prune_step(step_id)?;
```

**Key invariant**: Never prune before history is persisted. If the app crashes between seal and prune, the journal enables recovery.

### RAII Pattern

`ActiveJournal` is a proof token that a stream is in-flight:

```rust
pub struct ActiveJournal {
    journal_id: u64,
    step_id: StepId,
    next_seq: u64,
    model_name: String,
}
```

Methods like `append_text()` require `&mut ActiveJournal`, ensuring:

- Only one stream can be active at a time
- Events are properly sequenced
- The journal cannot be used incorrectly

## Token Counting

Token counting uses tiktoken's `cl100k_base` encoding, compatible with GPT-4, GPT-3.5, and Claude models.

### Token Counting Accuracy

**Important**: Token counts are **approximate**. The `cl100k_base` encoding provides:

- **Exact counts** for GPT-4 and GPT-3.5 models
- **Approximate counts** for Claude models (~5-10% variance, Anthropic uses a different tokenizer)
- **Approximate counts** for GPT-5.x models (may use updated tokenization)

The 5% safety margin in `ModelLimits::effective_input_budget()` compensates for these inaccuracies. For precise counts, use the provider's native token counting endpoint when available.

### Implementation

```rust
pub struct TokenCounter {
    encoder: Option<&'static CoreBPE>,  // Singleton, initialized once
}

impl TokenCounter {
    pub fn count_str(&self, text: &str) -> u32;
    pub fn count_message(&self, msg: &Message) -> u32;
}
```

Per-message overhead: **~4 tokens** for role markers and formatting. This approximation covers:
- Role name (e.g., "user", "assistant")
- Message structure/delimiters

### Efficiency

The tiktoken encoder is expensive to initialize. `TokenCounter` uses a singleton pattern:

```rust
static ENCODER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_encoder() -> Option<&'static CoreBPE> {
    ENCODER.get_or_init(|| cl100k_base().ok()).as_ref()
}
```

Creating multiple `TokenCounter` instances is cheap - they share the encoder. If initialization fails, the counter falls back to byte-length estimates.

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
let manager = ContextManager::load("~/.forge/history.json", "claude-opus-4")?;
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

The system follows Forge's type-driven philosophy (see `DESIGN.md`):

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

1. **Summarization requires API call**: Summarization uses LLM calls (`claude-haiku-4-5`, `gpt-5-nano`, or `gemini-3-pro-preview`), adding latency and cost.

2. **Contiguous ranges only**: Summaries must cover contiguous message ranges. Selective summarization is not supported to maintain chronological coherence.

3. **Token counting approximation**: The `cl100k_base` encoding is accurate for GPT models but approximate for Claude (~5-10% variance). The 5% safety margin compensates.

4. **No streaming summarization**: Summaries are generated with non-streaming API calls (60 second timeout).

5. **Single summary per range**: A message range can only have one summary. Re-summarization replaces the existing summary (orphaning the old one).

6. **Recent messages cannot be summarized**: The N most recent messages (default: 4) are always preserved verbatim. If these alone exceed the budget, the error is unrecoverable.

7. **SQLite journal latency**: Stream deltas are written synchronously to SQLite before display. On slow disks, this may cause UI stutter for high-frequency deltas.

---

## The Librarian (Intelligent Context Distillation)

The Librarian is a background component that provides intelligent fact extraction and retrieval, enabling effectively unlimited conversation length while keeping API costs low.

### How It Works

Instead of sending full conversation history to the LLM, the Librarian:

1. **Extracts** structured facts from each conversation exchange (post-turn)
2. **Retrieves** relevant facts for new queries (pre-flight)

The API call then includes:
- System prompt
- Retrieved facts (what Librarian determines is relevant)
- Recent N messages (immediate context)
- Current user message

### Model Choice

The Librarian uses **Gemini Flash** (`gemini-3-flash-preview`) for cheap, fast operations. It runs invisibly in the background - users never see it directly.

### Fact Types

The Librarian extracts five types of facts:

| Type | Description | Example |
|------|-------------|---------|
| `Entity` | Files, functions, variables, paths, URLs | "File `src/lib.rs` contains the main `App` struct" |
| `Decision` | Design choices with rationale | "Chose async/await for concurrency because..." |
| `Constraint` | Limitations or requirements | "Must stay compatible with API v2" |
| `CodeState` | What was created, modified, deleted | "Added `validate()` method to `User` struct" |
| `Pinned` | User-explicitly marked important | "Never modify the authentication flow" |

### Fact Structure

```rust
pub struct Fact {
    pub fact_type: FactType,
    pub content: String,
    pub entities: Vec<String>,  // Searchable keywords
}
```

### Lifecycle

```
User message arrives
        |
        v
+------------------+
| Pre-flight:      |     Librarian.retrieve_context(query)
| Retrieve facts   | --> Returns relevant facts to inject
+------------------+
        |
        v
Build context with facts + recent messages
        |
        v
Make API call
        |
        v
+------------------+
| Post-turn:       |     Librarian.extract_and_store(user, assistant)
| Extract facts    | --> Stores new facts for future retrieval
+------------------+
```

### API Usage

```rust
use forge_context::{Librarian, Fact, FactType, RetrievalResult};

// Initialize with persistent storage
let mut librarian = Librarian::open("~/.forge/facts.db", gemini_api_key)?;

// Pre-flight: Get relevant context for a query
let retrieval: RetrievalResult = librarian.retrieve_context("How do I add tests?").await?;
let context_text = format_facts_for_context(&retrieval.relevant_facts);

// Post-turn: Extract facts from the exchange
let extraction = librarian.extract_and_store(
    "How do I add tests?",
    "You can add tests by creating a #[test] function..."
).await?;

// Manual fact pinning (user-requested)
librarian.pin_fact(
    "Never delete the migrations directory",
    &["migrations".to_string()]
)?;

// Search facts by keyword
let related = librarian.search("migrations")?;
```

---

## Fact Store (Persistent Storage)

The `FactStore` provides SQLite-backed persistence for Librarian-extracted facts with source tracking and staleness detection.

### Schema

```sql
-- Core fact storage
facts (id, fact_type, content, turn_number, created_at)

-- Searchable entities (many-to-one)
fact_entities (fact_id, entity)

-- Source file tracking for staleness
fact_sources (id, file_path, sha256, updated_at)

-- Fact-to-source links (many-to-many)
fact_source_links (fact_id, source_id)
```

### Staleness Detection

Facts can be linked to source files. When a file changes (detected via SHA256 comparison), facts derived from it are marked as potentially stale:

```rust
// Store facts with source tracking
let fact_ids = store.store_facts(&facts, turn_number)?;
store.link_facts_to_sources(&fact_ids, &["src/lib.rs".to_string()])?;

// Later: check if facts are stale
let results = store.search_with_staleness("lib.rs")?;
for result in results {
    if result.is_stale() {
        println!("Fact may be outdated, sources changed: {:?}", result.stale_sources);
    }
}
```

### Key Operations

| Method | Description |
|--------|-------------|
| `open(path)` | Open or create persistent store |
| `open_in_memory()` | Create in-memory store (testing) |
| `store_facts(facts, turn)` | Store facts with turn number |
| `get_all_facts()` | Retrieve all stored facts |
| `search_by_entity(keyword)` | Search facts by entity keyword |
| `add_pinned_fact(content, entities, turn)` | Add user-pinned fact |
| `link_facts_to_sources(ids, paths)` | Link facts to source files |
| `check_staleness(facts)` | Check if source files have changed |
| `clear()` | Delete all facts (reset) |

### Storage Location

```
~/.forge/facts.db
```

---

## Public API

### Core Types

#### `ContextManager`

The main orchestrator for context management.

```rust
use forge_context::{ContextManager, PreparedContext, ContextBuildError, SummarizationNeeded};

// Create a manager for a specific model
let mut manager = ContextManager::new("claude-opus-4-5-20251101");

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
    Err(ContextBuildError::SummarizationNeeded(needed)) => {
        // Must summarize before proceeding
        let ids = needed.messages_to_summarize;
    }
    Err(ContextBuildError::RecentMessagesTooLarge { required_tokens, budget_tokens, .. }) => {
        // Unrecoverable: user must reduce input or switch models
    }
}
```

**Key methods:**

| Method | Description |
|--------|-------------|
| `new(model)` | Create manager for initial model |
| `push_message(msg)` | Add message, returns `MessageId` |
| `push_message_with_step_id(msg, id)` | Add message with stream step ID for crash recovery |
| `has_step_id(id)` | Check if step ID exists (for idempotent recovery) |
| `rollback_last_message(id)` | Remove last message if ID matches (transactional rollback) |
| `switch_model(name)` | Change model, returns `ContextAdaptation` |
| `set_output_limit(limit)` | Configure output limit for more input budget |
| `prepare()` | Build context proof or return `ContextBuildError` |
| `prepare_summarization(ids)` | Create async summarization request |
| `complete_summarization(...)` | Apply generated summary to history |
| `usage_status()` | Get current usage with explicit status |
| `current_limits()` | Get current model's `ModelLimits` |
| `current_limits_source()` | Get where limits came from (`Prefix`, `Override`, `DefaultFallback`) |
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

#### `ContextBuildError`

Error returned when context cannot be built within budget:

```rust
pub enum ContextBuildError {
    /// Older messages need summarization to fit within budget.
    SummarizationNeeded(SummarizationNeeded),
    /// The most recent N messages alone exceed the budget (unrecoverable).
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
        message_count: usize,
    },
}
```

#### `SummarizationNeeded`

Details about summarization needed to proceed:

```rust
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}
```

#### `ContextUsageStatus`

Usage state with explicit summarization status, returned by `usage_status()`:

```rust
pub enum ContextUsageStatus {
    /// Context fits within budget
    Ready(ContextUsage),
    /// Context exceeds budget, summarization needed
    NeedsSummarization {
        usage: ContextUsage,
        needed: SummarizationNeeded,
    },
    /// Recent messages alone exceed budget (unrecoverable)
    RecentMessagesTooLarge {
        usage: ContextUsage,
        required_tokens: u32,
        budget_tokens: u32,
    },
}
```

#### `PendingSummarization`

Request for async summarization, returned by `prepare_summarization()`:

```rust
pub struct PendingSummarization {
    pub scope: SummarizationScope,           // Contiguous range of message IDs
    pub messages: Vec<(MessageId, Message)>, // Messages to summarize
    pub original_tokens: u32,                // Total tokens in originals
    pub target_tokens: u32,                  // Target summary size
}
```

#### `SummarizationScope`

Contiguous set of message IDs to summarize (passed to `complete_summarization()`):

```rust
pub struct SummarizationScope {
    ids: Vec<MessageId>,
    range: Range<MessageId>,  // [start, end) exclusive
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
let resolved: ResolvedModelLimits = registry.get("claude-opus-4-20250514");

match resolved.source() {
    ModelLimitsSource::Prefix("claude-opus-4") => { /* matched prefix */ }
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
if let Some(recovered) = journal.recover()? {
    match recovered {
        RecoveredStream::Complete { partial_text, step_id, model_name, .. } => {
            // Stream finished but wasn't committed to history
            // Commit to history, then prune
            journal.commit_and_prune_step(step_id)?;
        }
        RecoveredStream::Errored { partial_text, step_id, error, .. } => {
            // Stream failed with error
            journal.discard_step(step_id)?;
        }
        RecoveredStream::Incomplete { partial_text, step_id, .. } => {
            // Stream was interrupted mid-flight
            // Option 1: Discard and retry
            journal.discard_step(step_id)?;
            // Option 2: Commit partial text to history
        }
    }
}

// Begin streaming session (model name stored for recovery attribution)
let mut active: ActiveJournal = journal.begin_session("claude-opus-4-5")?;

// Persist each delta BEFORE displaying to user
active.append_text(&mut journal, "Hello")?;
active.append_text(&mut journal, " world")?;
active.append_done(&mut journal)?;

// Seal when complete (marks entries as sealed)
let full_text: String = active.seal(&mut journal)?;

// After history is persisted, prune the journal
let step_id = active.step_id();  // Get before seal consumes active
journal.commit_and_prune_step(step_id)?;
```

**Key invariant:** Deltas must be persisted before display. This write-ahead approach ensures durability at the cost of slightly higher latency per delta.

**Commit-and-prune invariant:** Never prune before history is persisted. The journal commit and prune operation is atomic.

#### `ActiveJournal`

RAII handle proving a stream is in-flight:

```rust
impl ActiveJournal {
    fn step_id(&self) -> StepId;
    fn model_name(&self) -> &str;  // Model name for attribution
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
        model_name: Option<String>,  // For attribution
    },
    Errored {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        error: String,
        model_name: Option<String>,
    },
    Incomplete {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        model_name: Option<String>,
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
let batch_id: ToolBatchId = journal.begin_batch("claude-opus-4", "assistant text", &calls)?;

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
let batch_id = journal.begin_streaming_batch("claude-opus-4")?;

// Record call start as stream events arrive
journal.record_call_start(batch_id, 0, "call_1", "read_file", None)?;

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
use forge_context::{generate_summary, summarization_model, TokenCounter};
use forge_providers::ApiConfig;

// Get the summarization model for current provider
let model_name = summarization_model(Provider::Claude);
// Returns "claude-haiku-4-5" (cheaper/faster)

// Generate summary
let counter = TokenCounter::new();
let summary_text = generate_summary(
    &api_config,
    &counter,
    &messages_to_summarize,  // &[(MessageId, Message)]
    target_tokens,           // Target size for summary
).await?;
```

The function validates that input doesn't exceed the summarizer model's context limit before making the API call.

**Summarization models used:**

| Provider | Model | Context Limit |
|----------|-------|---------------|
| Claude | `claude-haiku-4-5` | 190,000 tokens |
| OpenAI | `gpt-5-nano` | 380,000 tokens |

### Librarian

#### `Librarian`

High-level API for intelligent context management:

```rust
use forge_context::{Librarian, Fact, FactType, RetrievalResult, ExtractionResult};

// Create with persistent storage
let mut librarian = Librarian::open("~/.forge/facts.db", api_key)?;

// Or in-memory for testing
let mut librarian = Librarian::open_in_memory(api_key)?;

// Pre-flight: Retrieve relevant facts for a query
let result: RetrievalResult = librarian.retrieve_context("How do I add tests?").await?;
println!("Found {} relevant facts (~{} tokens)",
    result.relevant_facts.len(),
    result.token_estimate);

// Post-turn: Extract and store facts from exchange
let extraction: ExtractionResult = librarian.extract_and_store(
    "user message",
    "assistant response"
).await?;

// Manual operations
librarian.pin_fact("Important constraint", &["keyword".to_string()])?;
let facts = librarian.search("keyword")?;
let all_facts = librarian.all_facts()?;
librarian.clear()?;  // Reset for testing
```

**Key methods:**

| Method | Description |
|--------|-------------|
| `open(path, api_key)` | Create with persistent SQLite storage |
| `open_in_memory(api_key)` | Create in-memory store (testing) |
| `retrieve_context(query)` | Async: get relevant facts for pre-flight injection |
| `extract_and_store(user, assistant)` | Async: extract facts from exchange and store |
| `store_facts(facts)` | Store pre-extracted facts (sync) |
| `store_facts_with_sources(facts, paths)` | Store with source file tracking |
| `pin_fact(content, entities)` | Add user-pinned fact |
| `search(keyword)` | Search facts by entity keyword |
| `search_with_staleness(keyword)` | Search with source file staleness info |
| `all_facts()` | Get all stored facts |
| `fact_count()` | Number of stored facts |
| `turn_counter()` | Current turn number |
| `clear()` | Delete all facts |

#### `Fact`

A distilled fact extracted from conversation:

```rust
pub struct Fact {
    pub fact_type: FactType,
    pub content: String,
    pub entities: Vec<String>,
}
```

#### `FactType`

Category of extracted fact:

```rust
pub enum FactType {
    Entity,     // Files, functions, variables, paths, URLs
    Decision,   // Design choices with rationale
    Constraint, // Limitations or requirements
    CodeState,  // What was created, modified, deleted
    Pinned,     // User-explicitly marked important
}
```

#### `ExtractionResult`

Result from post-turn fact extraction:

```rust
pub struct ExtractionResult {
    pub facts: Vec<Fact>,
}
```

#### `RetrievalResult`

Result from pre-flight fact retrieval:

```rust
pub struct RetrievalResult {
    pub relevant_facts: Vec<Fact>,
    pub token_estimate: u32,
}
```

#### `format_facts_for_context`

Helper to format facts for context injection:

```rust
use forge_context::format_facts_for_context;

let formatted = format_facts_for_context(&facts);
// Returns markdown with emoji prefixes:
// ## Relevant Context
// 📁 File src/lib.rs contains the App struct
// 🔧 Chose async/await for concurrency
```

### Fact Store

#### `FactStore`

Low-level SQLite storage (used internally by Librarian):

```rust
use forge_context::{FactStore, StoredFact, FactId};

let mut store = FactStore::open("~/.forge/facts.db")?;

// Store facts
let ids: Vec<FactId> = store.store_facts(&facts, turn_number)?;

// Query
let all: Vec<StoredFact> = store.get_all_facts()?;
let matches: Vec<StoredFact> = store.search_by_entity("keyword")?;
```

#### `StoredFact`

A fact with persistence metadata:

```rust
pub struct StoredFact {
    pub id: FactId,
    pub fact: Fact,
    pub turn_number: u64,
    pub created_at: String,
}
```

#### `FactWithStaleness`

A fact with source file change detection:

```rust
pub struct FactWithStaleness {
    pub fact: StoredFact,
    pub stale_sources: Vec<String>,  // Changed source files
}

impl FactWithStaleness {
    fn is_stale(&self) -> bool;  // True if any sources changed
}
```

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
    ContextManager, PreparedContext, ContextBuildError,
    StreamJournal, ActiveJournal, generate_summary, TokenCounter,
};
use forge_types::Message;

// Initialize
let mut manager = ContextManager::new("claude-opus-4-5");
let mut journal = StreamJournal::open("~/.forge/journal.db")?;
let counter = TokenCounter::new();

// Handle crash recovery (idempotent)
if let Some(recovered) = journal.recover()? {
    match recovered {
        RecoveredStream::Complete { step_id, partial_text, model_name, .. } => {
            // Check if already in history (idempotent recovery)
            if !manager.has_step_id(step_id) {
                manager.push_message_with_step_id(
                    Message::assistant(NonEmptyString::new(&partial_text)?),
                    step_id,
                );
                manager.save("~/.forge/history.json")?;
            }
            journal.commit_and_prune_step(step_id)?;
        }
        RecoveredStream::Errored { step_id, error, .. } => {
            tracing::warn!("Recovered stream failed: {}", error);
            journal.discard_step(step_id)?;
        }
        RecoveredStream::Incomplete { step_id, .. } => {
            journal.discard_step(step_id)?;
        }
    }
}

// Add user message
let user_msg_id = manager.push_message(Message::try_user("Explain Rust lifetimes")?);

// Prepare context
let prepared = match manager.prepare() {
    Ok(p) => p,
    Err(ContextBuildError::SummarizationNeeded(needed)) => {
        // Summarization needed - handle async
        let pending = manager.prepare_summarization(&needed.messages_to_summarize)
            .expect("messages exist");
        
        let summary_text = generate_summary(
            &api_config,
            &counter,
            &pending.messages,
            pending.target_tokens,
        ).await?;
        
        manager.complete_summarization(
            pending.scope,
            NonEmptyString::new(&summary_text)?,
            "claude-haiku-4-5".to_string(),
        )?;
        
        manager.prepare()?  // Should succeed now
    }
    Err(ContextBuildError::RecentMessagesTooLarge { required_tokens, budget_tokens, .. }) => {
        // Rollback user message and report error
        manager.rollback_last_message(user_msg_id);
        return Err(anyhow!("Input too large: {} tokens > {} budget", required_tokens, budget_tokens));
    }
};

// Make API call with streaming
let api_messages = prepared.api_messages();
let mut active = journal.begin_session("claude-opus-4-5")?;
let step_id = active.step_id();

for chunk in stream_response(&api_messages).await {
    active.append_text(&mut journal, &chunk)?;  // Persist first
    display_to_user(&chunk);                     // Then display
}

active.append_done(&mut journal)?;
let full_response = active.seal(&mut journal)?;

// Add assistant response to history with step ID (for idempotent recovery)
manager.push_message_with_step_id(
    Message::assistant(NonEmptyString::new(&full_response)?),
    step_id,
);

// Persist conversation BEFORE pruning journal
manager.save("~/.forge/history.json")?;

// Only prune after history is safely persisted
journal.commit_and_prune_step(step_id)?;
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

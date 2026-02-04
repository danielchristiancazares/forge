# forge-context

> Note: This is an implementation-focused overview of the current distillation system; the authoritative spec is `docs/CONTEXT_INFINITY_SRD.md`.

Context Infinity™ is Forge's system for managing unlimited conversation context with LLMs. It preserves complete conversation history while automatically distilling older content to fit within model-specific token limits.

## LLM-TOC
<!-- toc:start -->
| Lines | Section |
| --- | --- |
| 7-38 | LLM-TOC |
| 39-55 | Overview |
| 56-65 | Design Principles |
| 66-97 | Architecture |
| 98-166 | Core Concepts |
| 167-233 | Token Budget Calculation |
| 234-324 | Context Building Algorithm |
| 325-355 | When Distillation Triggers |
| 356-413 | Distillation Process |
| 414-443 | Model Switching (Context Adaptation) |
| 444-549 | Stream Journal (Crash Recovery) |
| 550-594 | Token Counting |
| 595-622 | Usage Statistics |
| 623-650 | Persistence |
| 651-666 | Configuration |
| 667-689 | Type-Driven Design |
| 690-717 | Extension Points |
| 718-735 | Limitations |
| 736-832 | The Librarian |
| 833-892 | Fact Store |
| 893-1617 | Public API |
| 1618-1714 | Complete Workflow Example |
| 1715-1732 | Type Relationships |
| 1733-1739 | Error Handling |
| 1740-1748 | Dependencies |
| 1749-1756 | Testing |
<!-- toc:end -->

## Overview

The core principle is **never discard, always compress**: messages are never deleted from history. Instead, when the context window fills up, older messages are distilled into compact representations that preserve essential information.

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

1. **Append-only history**: Messages are never deleted. Distillates link to original messages, enabling restoration when switching to models with larger context windows.

2. **Type-driven correctness**: `PreparedContext` serves as a proof token that context was successfully built within budget before an API call.

3. **Explicit distillation**: The manager signals when distillation is needed rather than silently truncating. Callers control when and how distillation occurs.

4. **Write-ahead durability**: Stream deltas are persisted to SQLite before display, ensuring recoverability after crashes.

## Architecture

### Component Overview

| Component | Purpose |
|-----------|---------|
| `ContextManager` | Orchestrates all context management operations |
| `FullHistory` | Append-only storage for messages and distillates |
| `TokenCounter` | Accurate token counting via tiktoken (cl100k_base) |
| `ModelRegistry` | Model-specific token limits from the predefined catalog |
| `WorkingContext` | Derived view of what to send to the API |
| `StreamJournal` | SQLite-backed crash recovery for streaming responses |
| `Librarian` | Intelligent fact extraction and retrieval using Gemini Flash |
| `FactStore` | SQLite-backed persistent storage for extracted facts |

### Directory Structure

```
context/src/
  lib.rs              # Module exports and public API
  manager.rs          # ContextManager - main orchestrator
  history.rs          # FullHistory, MessageId, DistillateId, Distillate
  model_limits.rs     # ModelLimits, ModelRegistry
  token_counter.rs    # TokenCounter (tiktoken wrapper)
  working_context.rs  # WorkingContext, ContextSegment, ContextUsage
  stream_journal.rs   # StreamJournal, ActiveJournal (crash recovery)
  distillation.rs     # LLM-based distillation via cheaper models
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
- `distillate_id`: Optional link to a Distillate that covers this message

```rust
pub enum HistoryEntry {
    Original {
        id: MessageId,
        message: Message,
        token_count: u32,
        created_at: SystemTime,
    },
    Distilled {
        id: MessageId,
        message: Message,
        token_count: u32,
        distillate_id: DistillateId,  // Links to the covering Distillate
        created_at: SystemTime,
    },
}
```

When messages are distilled, they transition from `Original` to `Distilled` but remain in history. The original content is always accessible.

### Distillates

A `Distillate` represents a distilled version of a contiguous range of messages:

```rust
pub struct Distillate {
    id: DistillateId,
    covers: Range<MessageId>,      // [start, end) of messages covered
    content: NonEmptyString,       // The distilled text
    token_count: u32,              // Tokens in the Distillate
    original_tokens: u32,          // Tokens in original messages
    created_at: SystemTime,
    generated_by: String,          // Model that generated this
}
```

Key invariant: distillates must cover **contiguous** message ranges. Non-contiguous distillation is not supported to maintain chronological coherence.

### Working Context (Derived View)

The `WorkingContext` is rebuilt on-demand and represents what will actually be sent to the LLM API. It mixes:

1. **Original messages** - sent verbatim
2. **Distillates** - injected as system messages with `[Earlier conversation Distillate]` prefix

```rust
pub enum ContextSegment {
    Original { id: MessageId, tokens: u32 },
    Distilled {
        distillate_id: DistillateId,
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
effective_budget = context_window - max_output - safety_margin
safety_margin = min(available / 20, 4096)  // 5% capped at 4096 tokens
```

Example for Claude Opus 4.5 (200k context, 64k output):

```
available = 200,000 - 64,000 = 136,000
safety_margin = min(136,000 / 20, 4096) = min(6,800, 4096) = 4,096
effective_budget = 136,000 - 4,096 = 131,904 tokens
```

The safety margin (5% capped at 4096) accounts for:

- Token counting inaccuracies (see [Token Counting Accuracy](#token-counting-accuracy))
- System prompt overhead
- Tool definitions and formatting

### Configured Output Limit

When a user configures a smaller output limit than the model's maximum, more tokens become available for input context. Use `set_output_limit()` to adjust:

```rust
use forge_types::PredefinedModel;

let mut manager = ContextManager::new(PredefinedModel::ClaudeOpus.to_model_name());

// Model has 64k max output, but user configured 16k
manager.set_output_limit(16_000);

// Now effective budget is:
// 200,000 - 16,000 = 184,000 available
// safety_margin = min(184,000 / 20, 4096) = 4,096
// effective_budget = 184,000 - 4,096 = 179,904 (vs 131,904 without config)
```

The reserved output is clamped to the model's `max_output` - requesting more than the model supports has no effect.

### Known Model Limits

| Model | Context Window | Max Output |
|-------|---------------|------------|
| `claude-opus-4-5-20251101` | 200,000 | 64,000 |
| `claude-sonnet-4-5-20250514` | 200,000 | 64,000 |
| `claude-haiku-4-5-20251001` | 200,000 | 64,000 |
| `gpt-5.2-pro` | 400,000 | 128,000 |
| `gpt-5.2` | 400,000 | 128,000 |
| `gemini-3-pro-preview` | 1,048,576 | 65,536 |
| `gemini-3-flash-preview` | 1,048,576 | 65,536 |

## Context Building Algorithm

The `build_working_context()` algorithm runs in five phases:

### Phase 1: Reserve Recent Messages

The N most recent messages (default: 4) are **always included**. These represent the immediate conversation context and are never distilled.

```rust
let preserve_count = self.distillation_config.preserve_recent; // 4
let recent_start = entries.len().saturating_sub(preserve_count);
let tokens_for_recent: u32 = entries[recent_start..].iter()
    .map(|e| e.token_count())
    .sum();
```

If recent messages alone exceed the budget, distillation fails with an error.

### Phase 2: Partition Older Messages into Blocks

Older messages are grouped into contiguous blocks:

- **Undistilled Block** (`Undistilled`): Consecutive messages with no Distillate
- **Distilled Block** (`Distilled`): Consecutive messages covered by the same Distillate

```rust
enum Block {
    Undistilled(Vec<(MessageId, u32)>),
    Distilled {
        distillate_id: DistillateId,
        messages: Vec<(MessageId, u32)>,
        distillate_tokens: u32,
    },
}
```

### Phase 3: Select Content (Newest to Oldest)

Starting from the most recent older block, include content while staying within budget:

```text
remaining_budget = effective_budget - tokens_for_recent
```

For each block (newest first):

1. **Distilled Block** (`Distilled`):
   - If original messages fit: include originals (better quality)
   - Else if Distillate fits: include Distillate
   - Else: skip (will need re-distillation)

2. **Undistilled Block** (`Undistilled`):
   - Include as many recent messages as fit
   - Mark the rest as needing distillation

### Phase 4: Assemble Working Context

Selected segments are arranged in chronological order:

```text
[Older distillates/messages] -> [Recent messages always included]
```

### Phase 5: Return or Request Distillation

If all content fits: return `Ok(WorkingContext)`

If Undistilled messages don't fit:

```rust
Err(ContextBuildError::DistillationNeeded(DistillationNeeded {
    excess_tokens: u32,
    messages_to_distill: Vec<MessageId>,
    suggestion: String,
}))
```

### Error: Recent Messages Too Large

If the N most recent messages alone exceed the budget, distillation cannot help. This is an unrecoverable error:

```rust
Err(ContextBuildError::RecentMessagesTooLarge {
    required_tokens: u32,  // Tokens needed for recent messages
    budget_tokens: u32,    // Available budget
    message_count: usize,  // Number of recent messages
})
```

The user must either reduce their input or switch to a model with a larger context window.

## When Distillation Triggers

Distillation is triggered when:

1. **Context budget exceeded**: `build_working_context()` returns `DistillationNeeded`
2. **Model switch to smaller context**: Switching from 200k to 8k model
3. **Manual request**: User invokes `/distill` command

The decision flow:

```
push_message() -> usage_status()
                      |
                      v
               +------+------+
               |             |
          Ready(usage)   NeedsDistillation
               |             |
               v             v
          Continue      prepare_distillation()
                             |
                             v
                    PendingDistillation
                             |
                             v
                    generate_distillation() [async]
                             |
                             v
                    complete_distillation()
```

## Distillation Process

### Configuration

```rust
pub struct DistillationConfig {
    pub target_ratio: f32,      // 0.15 = compress to 15% of original
    pub preserve_recent: usize, // 4 = never distill last 4 messages
}
```

### Prepare Distillation

```rust
pub fn prepare_distillation(&mut self, message_ids: &[MessageId]) 
    -> Option<PendingDistillation>
```

1. Sort and deduplicate message IDs
2. Extract first contiguous run (distillations must be contiguous)
3. Calculate target tokens: `original_tokens * target_ratio`
4. Allocate a `DistillateId`
5. Return `PendingDistillation` with messages to distill

### Generate Distillation (Async)

Distillation uses cheaper/faster models:

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

### Complete Distillation

```rust
pub fn complete_distillation(
    &mut self,
    distillate_id: DistillateId,
    scope: DistillationScope,
    content: NonEmptyString,
    generated_by: String,
)
```

1. Count tokens in the generated distillate
2. Create `Distillate` with metadata
3. Add to history
4. Mark covered messages as `Distilled`

## Model Switching (Context Adaptation)

When switching models, the context manager adapts:

```rust
pub enum ContextAdaptation {
    NoChange,
    Shrinking {
        old_budget: u32,
        new_budget: u32,
        needs_distillation: bool,
    },
    Expanding {
        old_budget: u32,
        new_budget: u32,
        can_restore: usize,  // Messages that could use originals
    },
}
```

### Shrinking (e.g., Claude 200k -> GPT-4 8k)

If current context exceeds new budget, `needs_distillation` is true. The app should trigger distillation before the next API call.

### Expanding (e.g., GPT-4 8k -> Claude 200k)

Previously distilled messages can be restored to their originals. The `try_restore_messages()` method returns how many messages would use originals in the new budget.

This is **automatic** - no re-distillation needed. The working context builder prefers originals when budget allows.

## Stream Journal (Crash Recovery)

The `StreamJournal` ensures streaming responses survive crashes using SQLite WAL mode with intelligent buffering.

### Key Invariant

**Deltas are buffered and flushed to SQLite under controlled conditions.**

To balance crash recovery with UI responsiveness, deltas are buffered in memory and flushed when:

1. **First content arrives** - Ensures crash recovery has content immediately
2. **Buffer reaches threshold** - Prevents unbounded memory growth (default: 25 deltas)
3. **Time since last flush exceeds interval** - Bounds the data loss window (default: 200ms)

This means a crash can lose up to the flush threshold deltas if they arrived within the flush interval. The time-based flush bounds this window.

### Configuration

The flush behavior can be tuned via environment variables:

```bash
FORGE_STREAM_JOURNAL_FLUSH_THRESHOLD=25   # Deltas before auto-flush (default: 25)
FORGE_STREAM_JOURNAL_FLUSH_INTERVAL_MS=200  # Max ms between flushes (default: 200)
```

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
manager.save("<data_dir>/history.json")?;

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

Token counting uses tiktoken's `o200k_base` encoding, accurate for `gpt-5.2` and `gpt-5.2-pro`.

### Token Counting Accuracy

**Important**: Token counts are **approximate**. The `o200k_base` encoding provides:

- **Accurate counts** for `gpt-5.2` and `gpt-5.2-pro`
- **Approximate counts** for Claude models (~5-10% variance, Anthropic uses a proprietary tokenizer)
- **Approximate counts** for Gemini models (Google uses a proprietary tokenizer)

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
    ENCODER.get_or_init(|| o200k_base().ok()).as_ref()
}
```

Creating multiple `TokenCounter` instances is cheap - they share the encoder. If initialization fails, the counter falls back to byte-length estimates.

## Usage Statistics

The `ContextUsage` struct provides UI-friendly statistics:

```rust
pub struct ContextUsage {
    pub used_tokens: u32,
    pub budget_tokens: u32,
    pub distilled_segments: usize,
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

In the Forge application, persistent files live under the OS local data directory
(`dirs::data_local_dir()/forge`). Examples below use `<data_dir>` to denote that base path.

### History Serialization

```rust
use forge_types::PredefinedModel;

// Save
context_manager.save("<data_dir>/history.json")?;

// Load
let manager =
    ContextManager::load("<data_dir>/history.json", PredefinedModel::ClaudeOpus.to_model_name())?;
```

The serialization format validates:

- Message IDs are sequential (0, 1, 2, ...)
- Distillate IDs are sequential
- Distillate ranges reference valid messages
- Distilled (`Distilled`) messages reference valid Distillates

### Stream Journal Location

```
<data_dir>/stream_journal.db
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
| `DistillateId` | Proof of Distillate existence |
| `ActiveJournal` | Proof that a stream is in-flight (RAII) |
| `PreparedContext` | Proof that context fits within budget |
| `DistillationNeeded` | Explicit error requiring caller action |
| `NonEmptyString` | Message content guaranteed non-empty |

### PreparedContext as Proof

The `prepare()` method returns a proof that context is ready:

```rust
pub fn prepare(&self) -> Result<PreparedContext<'_>, DistillationNeeded>
```

`PreparedContext` can only be created if the working context fits within budget. Callers cannot accidentally send over-budget context to the API.

## Extension Points

### Adding a New Provider

1. Add model limits to `KNOWN_MODELS` in `model_limits.rs`
2. Add distillation model in `distillation.rs`
3. Implement `generate_distillation_*` for the new provider

### Adjusting Distillation Behavior

Modify `DistillationConfig`:

```rust
DistillationConfig {
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

1. **Distillation requires API call**: Distillation uses LLM calls (`claude-haiku-4-5`, `gpt-5-nano`, or `gemini-3-pro-preview`), adding latency and cost.

2. **Contiguous ranges only**: distillates must cover contiguous message ranges. Selective distillation is not supported to maintain chronological coherence.

3. **Token counting approximation**: The `o200k_base` encoding is accurate for `gpt-5.2` / `gpt-5.2-pro` but approximate for Claude and Gemini (~5-10% variance). The 5% safety margin compensates.

4. **No streaming distillation**: distillates are generated with non-streaming API calls (60 second timeout).

5. **Single Distillate per range**: A message range can only have one Distillate. Re-distillation replaces the existing Distillate (orphaning the old one).

6. **Recent messages cannot be distilled**: The N most recent messages (default: 4) are always preserved verbatim. If these alone exceed the budget, the error is unrecoverable.

7. **Stream journal crash window**: Stream deltas are buffered before SQLite persistence. A crash can lose up to 25 deltas (or 200ms of content, whichever comes first). The flush threshold and interval are configurable via environment variables.

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

The Librarian uses **Gemini 3 Flash** (`gemini-3-flash-preview`) for cheap, fast operations. It runs invisibly in the background - users never see it directly. The model is called with low temperature (0.1) and low thinking level for consistent extraction.

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
let mut librarian = Librarian::open("<data_dir>/librarian.db", gemini_api_key)?;

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
<data_dir>/librarian.db
```

---

## Public API

### Core Types

#### `ContextManager`

The main orchestrator for context management.

```rust
use forge_context::{ContextAdaptation, ContextBuildError, ContextManager, PreparedContext};
use forge_types::PredefinedModel;

// Create a manager for a specific model
let mut manager = ContextManager::new(PredefinedModel::ClaudeOpus.to_model_name());

// Add messages to history
let msg_id = manager.push_message(Message::try_user("Hello!")?);

// Switch models (triggers adaptation logic)
match manager.switch_model(PredefinedModel::Gpt52.to_model_name()) {
    ContextAdaptation::Shrinking { needs_distillation, .. } => {
        if needs_distillation {
            // Handle distillation requirement
        }
    }
    ContextAdaptation::Expanding { can_restore, .. } => {
        // More context available; can restore distilled messages
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
    Err(ContextBuildError::DistillationNeeded(needed)) => {
        // Must distill before proceeding
        let ids = needed.messages_to_distill;
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
| `switch_model(model)` | Change model, returns `ContextAdaptation` |
| `set_output_limit(limit)` | Configure output limit for more input budget |
| `prepare()` | Build context proof or return `ContextBuildError` |
| `prepare_distillation(ids)` | Create async distillation request |
| `complete_distillation(...)` | Apply generated distillation to history |
| `usage_status()` | Get current usage with explicit status |
| `current_limits()` | Get current model's `ModelLimits` |
| `current_limits_source()` | Get where limits came from (`Catalog` or `Override`) |
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
        needs_distillation: bool,
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
    /// Older messages need distillation to fit within budget.
    DistillationNeeded(DistillationNeeded),
    /// The most recent N messages alone exceed the budget (unrecoverable).
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
        message_count: usize,
    },
}
```

#### `DistillationNeeded`

Details about distillation needed to proceed:

```rust
pub struct DistillationNeeded {
    pub excess_tokens: u32,
    pub messages_to_distill: Vec<MessageId>,
    pub suggestion: String,
}
```

#### `ContextUsageStatus`

Usage state with explicit distillation status, returned by `usage_status()`:

```rust
pub enum ContextUsageStatus {
    /// Context fits within budget
    Ready(ContextUsage),
    /// Context exceeds budget, distillation needed
    NeedsDistillation {
        usage: ContextUsage,
        needed: DistillationNeeded,
    },
    /// Recent messages alone exceed budget (unrecoverable)
    RecentMessagesTooLarge {
        usage: ContextUsage,
        required_tokens: u32,
        budget_tokens: u32,
    },
}
```

#### `PendingDistillation`

Request for async distillation, returned by `prepare_distillation()`:

```rust
pub struct PendingDistillation {
    pub scope: DistillationScope,            // Contiguous range of message IDs
    pub messages: Vec<(MessageId, Message)>, // Messages to distill
    pub original_tokens: u32,                // Total tokens in originals
    pub target_tokens: u32,                  // Target Distillate size
}
```

#### `DistillationScope`

Contiguous set of message IDs to distill (passed to `complete_distillation()`):

```rust
pub struct DistillationScope {
    ids: Vec<MessageId>,
    range: Range<MessageId>,  // [start, end) exclusive
}
```

### History Types

#### `FullHistory`

Append-only storage for all conversation messages and distillates.

```rust
use forge_context::{FullHistory, MessageId, DistillateId};

let mut history = FullHistory::new();

// Add messages
let id: MessageId = history.push(message, token_count);

// Access entries
let entry = history.get_entry(id);
println!("Content: {}", entry.message().content());
println!("Tokens: {}", entry.token_count());
println!("Distilled: {}", entry.is_distilled());

// Statistics
println!("Total messages: {}", history.len());
println!("Total tokens: {}", history.total_tokens());
println!("Distilled count: {}", history.distilled_count());
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
    Distilled {
        id: MessageId,
        message: Message,
        token_count: u32,
        distillate_id: DistillateId,
        created_at: SystemTime,
    },
}
```

#### `Distillate`

Represents distilled conversation segments:

```rust
pub struct Distillate {
    id: DistillateId,
    covers: Range<MessageId>,  // [start, end) of original messages
    content: NonEmptyString,
    token_count: u32,
    original_tokens: u32,      // For compression ratio tracking
    created_at: SystemTime,
    generated_by: String,      // Model that created Distillate
}
```

### Model Limits

#### `ModelRegistry`

Registry with catalog-based model lookup:

```rust
use forge_context::{ModelLimits, ModelLimitsSource, ModelRegistry, ResolvedModelLimits};
use forge_types::PredefinedModel;

let registry = ModelRegistry::new();

// Lookup by exact catalog model
let model = PredefinedModel::ClaudeOpus.to_model_name();
let resolved: ResolvedModelLimits = registry.get(&model);

match resolved.source() {
    ModelLimitsSource::Catalog(model) => { /* matched catalog */ }
    ModelLimitsSource::Override => { /* custom override */ }
}

let limits: ModelLimits = resolved.limits();
println!("Context window: {}", limits.context_window());
println!("Max output: {}", limits.max_output());
println!("Effective input budget: {}", limits.effective_input_budget());
```

**Known models:**

| Model | Context Window | Max Output |
|-------|---------------|------------|
| `claude-opus-4-5-20251101` | 200,000 | 64,000 |
| `claude-sonnet-4-5-20250514` | 200,000 | 64,000 |
| `claude-haiku-4-5-20251001` | 200,000 | 64,000 |
| `gpt-5.2-pro` | 400,000 | 128,000 |
| `gpt-5.2` | 400,000 | 128,000 |
| `gemini-3-pro-preview` | 1,048,576 | 65,536 |
| `gemini-3-flash-preview` | 1,048,576 | 65,536 |

#### `ModelLimits`

Token constraints for a model:

```rust
let limits = ModelLimits::new(200_000, 16_000);

// Effective budget = context_window - max_output - safety_margin
// safety_margin = min(available / 20, 4096)
let budget = limits.effective_input_budget();
// 200,000 - 16,000 = 184,000 available
// safety_margin = min(184,000 / 20, 4096) = 4,096
// effective_budget = 184,000 - 4,096 = 179,904
```

The safety margin (5% capped at 4096) accounts for token counting inaccuracies and overhead from system prompts, formatting, and tool definitions.

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
let mut journal = StreamJournal::open("<data_dir>/stream_journal.db")?;

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

RAII handle proving a stream is in-flight. Text deltas are buffered in memory and flushed periodically to reduce SQLite write frequency and improve UI responsiveness.

```rust
impl ActiveJournal {
    fn step_id(&self) -> StepId;
    fn model_name(&self) -> &str;  // Model name for attribution
    fn append_text(&mut self, journal: &mut StreamJournal, content: impl Into<String>) -> Result<()>;
    fn append_done(&mut self, journal: &mut StreamJournal) -> Result<()>;  // Flushes buffer first
    fn append_error(&mut self, journal: &mut StreamJournal, message: impl Into<String>) -> Result<()>;  // Flushes buffer first
    fn flush(&mut self, journal: &mut StreamJournal) -> Result<()>;  // Explicit flush
    fn seal(self, journal: &mut StreamJournal) -> Result<String>;  // Flushes and seals
    fn discard(self, journal: &mut StreamJournal) -> Result<u64>;  // Discards without flush
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

#### `JournalStats`

Statistics about the stream journal:

```rust
pub struct JournalStats {
    pub total_entries: u64,
    pub sealed_entries: u64,
    pub unsealed_entries: u64,
    pub current_step_id: StepId,
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
let mut journal = ToolJournal::open("<data_dir>/tool_journal.db")?;

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

#### `ToolBatchId`

Type alias for tool batch identifiers:

```rust
pub type ToolBatchId = i64;
```

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

// Update assistant text (full replacement)
journal.update_assistant_text(batch_id, "I'll read that file...")?;

// Or append deltas efficiently (O(n) instead of O(n^2))
journal.append_assistant_delta(batch_id, "Hello")?;
journal.append_assistant_delta(batch_id, " world")?;
```

### Distillation

#### `generate_distillation`

Async function to generate distillations via LLM:

```rust
use forge_context::{generate_distillation, distillation_model, TokenCounter};
use forge_providers::ApiConfig;

// Get the distillation model for current provider
let model_name = distillation_model(Provider::Claude);
// Returns "claude-haiku-4-5" (cheaper/faster)

// Generate distillation
let counter = TokenCounter::new();
let distilled_text = generate_distillation(
    &api_config,
    &counter,
    &messages_to_distill,  // &[(MessageId, Message)]
    target_tokens,          // Target size for distillation Distillate
).await?;
```

The function validates that input doesn't exceed the distiller model's context limit before making the API call.

**Distillation models used:**

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
let mut librarian = Librarian::open("<data_dir>/librarian.db", api_key)?;

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

let mut store = FactStore::open("<data_dir>/librarian.db")?;

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
    Distilled {
        distillate_id: DistillateId,
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
    pub distilled_segments: usize,
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
    ActiveJournal, ContextBuildError, ContextManager, PreparedContext, StreamJournal, TokenCounter,
    generate_distillation,
};
use forge_types::{Message, PredefinedModel};

// Initialize
let mut manager = ContextManager::new(PredefinedModel::ClaudeOpus.to_model_name());
let mut journal = StreamJournal::open("<data_dir>/stream_journal.db")?;
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
                manager.save("<data_dir>/history.json")?;
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
    Err(ContextBuildError::DistillationNeeded(needed)) => {
        // Distillation needed - handle async
        let pending = manager.prepare_distillation(&needed.messages_to_distill)
            .expect("messages exist");
        
        let distilled_text = generate_distillation(
            &api_config,
            &counter,
            &pending.messages,
            pending.target_tokens,
        ).await?;
        
        manager.complete_distillation(
            pending.scope,
            NonEmptyString::new(&distilled_text)?,
            PredefinedModel::ClaudeHaiku.model_id().to_string(),
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
let mut active = journal.begin_session(PredefinedModel::ClaudeOpus.model_id())?;
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
manager.save("<data_dir>/history.json")?;

// Only prune after history is safely persisted
journal.commit_and_prune_step(step_id)?;
```

## Type Relationships

```
MessageId -----> HistoryEntry -----> Message
     |               |
     |               v
     +--------> DistillateId -----> Distillate
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

- `DistillationNeeded`: Returned when context exceeds budget and distillation is required
- `anyhow::Error`: Used for I/O, database, and API errors

## Dependencies

- `forge-types`: Core types (`Message`, `NonEmptyString`, `Provider`)
- `forge-providers`: API configuration (`ApiConfig`)
- `tiktoken-rs`: Token counting
- `rusqlite`: Stream journal persistence
- `reqwest`: HTTP client for distillation API calls
- `serde`/`serde_json`: Serialization

## Testing

```bash
cargo test -p forge-context           # Run all tests
cargo test -p forge-context -- --nocapture  # With output
```

The crate includes comprehensive unit tests for all modules. Integration tests requiring API keys are marked with `#[ignore]`.



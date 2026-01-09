# Context Crate Architecture

This document provides comprehensive technical documentation for the `forge-context` crate, which implements adaptive context window management and SQLite persistence for Forge.

## Table of Contents

1. [Module Overview](#module-overview)
2. [Architecture](#architecture)
3. [Context Manager Orchestration](#context-manager-orchestration)
4. [History Persistence](#history-persistence)
5. [Stream Journal (Crash Recovery)](#stream-journal-crash-recovery)
6. [Tool Journal](#tool-journal)
7. [Working Context and Token Budget](#working-context-and-token-budget)
8. [Summarization System](#summarization-system)
9. [Model Limits Registry](#model-limits-registry)
10. [Token Counting](#token-counting)
11. [Key Data Structures](#key-data-structures)
12. [Error Handling](#error-handling)
13. [Persistence Format](#persistence-format)
14. [Extension Guide](#extension-guide)

---

## Module Overview

The `context` crate provides intelligent context window management for LLM conversations. It solves the fundamental problem of finite context windows while maintaining seamless, long-running conversations.

### Source Files

```
context/src/
  lib.rs              # Module exports and public API
  manager.rs          # ContextManager - main orchestrator
  history.rs          # FullHistory, MessageId, SummaryId, Summary
  model_limits.rs     # ModelLimits, ModelRegistry
  token_counter.rs    # TokenCounter (tiktoken wrapper)
  working_context.rs  # WorkingContext, ContextSegment, ContextUsage
  stream_journal.rs   # StreamJournal, ActiveJournal (crash recovery)
  tool_journal.rs     # ToolJournal, RecoveredToolBatch (tool batch recovery)
  summarization.rs    # LLM-based summarization via cheaper models
```

### Public Exports

From `lib.rs`:

```rust
// History types
pub use history::{FullHistory, HistoryEntry, MessageId, Summary, SummaryId};

// Manager types
pub use manager::{
    ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus,
    PendingSummarization, PreparedContext, SummarizationNeeded, SummarizationScope,
};

// Model limits
pub use model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry, ResolvedModelLimits};

// Stream journal (crash recovery)
pub use stream_journal::{ActiveJournal, JournalStats, RecoveredStream, StepId, StreamJournal};

// Tool journal
pub use tool_journal::{RecoveredToolBatch, ToolBatchId, ToolJournal};

// Summarization
pub use summarization::{generate_summary, summarization_model};

// Token counting
pub use token_counter::TokenCounter;

// Working context
pub use working_context::{ContextSegment, ContextUsage, WorkingContext};
```

### Dependencies

| Dependency | Purpose |
|------------|---------|
| `forge-types` | Core types (`Message`, `NonEmptyString`, `Provider`, `ToolCall`, `ToolResult`) |
| `forge-providers` | API configuration (`ApiConfig`) |
| `tiktoken-rs` | Token counting via cl100k_base encoding |
| `rusqlite` | SQLite for stream/tool journal persistence |
| `reqwest` | HTTP client for summarization API calls |
| `serde` / `serde_json` | Serialization for history persistence |

---

## Architecture

The crate follows a layered architecture with clear separation of concerns:

```
                        ContextManager (Orchestrator)
                              |
         +--------------------+--------------------+
         |                    |                    |
    FullHistory          TokenCounter        ModelRegistry
    (append-only)        (tiktoken)          (limits/model)
         |
         v
    WorkingContext -----> PreparedContext -----> API Messages
    (derived view)        (proof token)

    [Separate Lifecycle]
    StreamJournal -----> ActiveJournal -----> RecoveredStream
    (SQLite WAL)         (RAII handle)        (crash recovery)

    ToolJournal -----> RecoveredToolBatch
    (SQLite)           (tool batch recovery)
```

### Design Principles

1. **Append-Only History**: Messages are never deleted. Summaries link to original messages, enabling restoration when switching to models with larger context windows.

2. **Type-Driven Correctness**: Proof tokens (`PreparedContext`, `ActiveJournal`) ensure operations occur in valid states.

3. **Explicit Summarization**: The manager signals when summarization is needed rather than silently truncating. Callers control when and how summarization occurs.

4. **Write-Ahead Durability**: Stream deltas are persisted to SQLite before display, ensuring recoverability after crashes.

5. **Singleton Resources**: Expensive resources like the tiktoken encoder use singleton patterns for efficiency.

---

## Context Manager Orchestration

The `ContextManager` (`manager.rs`) is the central orchestrator for all context management operations.

### Structure

```rust
pub struct ContextManager {
    /// Complete history - never discarded.
    history: FullHistory,
    /// Token counter (uses singleton encoder).
    counter: TokenCounter,
    /// Model registry for token limits.
    registry: ModelRegistry,
    /// Current model name.
    current_model: String,
    /// Current model's limits.
    current_limits: ModelLimits,
    /// Where the current limits came from.
    current_limits_source: ModelLimitsSource,
    /// Summarization configuration.
    summarization_config: SummarizationConfig,
    /// Configured output limit (if set, allows more input context).
    configured_output_limit: Option<u32>,
}
```

### Initialization

```rust
// Create a new manager for a model
let mut manager = ContextManager::new("claude-sonnet-4-20250514");

// Load from persisted history
let manager = ContextManager::load("~/.forge/history.json", "claude-sonnet-4")?;
```

### Adding Messages

```rust
// Standard message addition
let msg_id: MessageId = manager.push_message(message);

// With stream step ID (for crash recovery linkage)
let msg_id = manager.push_message_with_step_id(message, stream_step_id);

// Check if step ID exists (idempotent recovery)
if manager.has_step_id(step_id) {
    // Already recovered, skip
}

// Rollback last message (transactional recovery)
if let Some(msg) = manager.rollback_last_message(expected_id) {
    // Message removed
}
```

### Model Switching

When switching models, the manager calculates context adaptation requirements:

```rust
pub enum ContextAdaptation {
    /// No change in effective budget.
    NoChange,
    /// Switched to a model with smaller context.
    Shrinking {
        old_budget: u32,
        new_budget: u32,
        needs_summarization: bool,
    },
    /// Switched to a model with larger context.
    Expanding {
        old_budget: u32,
        new_budget: u32,
        /// Number of messages that could potentially be restored.
        can_restore: usize,
    },
}

// Usage
match manager.switch_model("gpt-4") {
    ContextAdaptation::Shrinking { needs_summarization: true, .. } => {
        // Must summarize before next API call
    }
    ContextAdaptation::Expanding { can_restore, .. } => {
        // `can_restore` messages can use originals instead of summaries
    }
    ContextAdaptation::NoChange => {}
}
```

### Preparing Context

The `prepare()` method builds a working context and returns a proof token:

```rust
pub fn prepare(&self) -> Result<PreparedContext<'_>, ContextBuildError>

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

The `PreparedContext` serves as proof that context fits within budget:

```rust
pub struct PreparedContext<'a> {
    manager: &'a ContextManager,
    working_context: WorkingContext,
}

impl<'a> PreparedContext<'a> {
    /// Materialize messages for an API call.
    pub fn api_messages(&self) -> Vec<Message>;

    /// Usage stats for UI.
    pub fn usage(&self) -> ContextUsage;
}
```

### Usage Status

For UI display without requiring successful context preparation:

```rust
pub enum ContextUsageStatus {
    Ready(ContextUsage),
    NeedsSummarization {
        usage: ContextUsage,
        needed: SummarizationNeeded,
    },
    RecentMessagesTooLarge {
        usage: ContextUsage,
        required_tokens: u32,
        budget_tokens: u32,
    },
}

let status = manager.usage_status();
```

### Output Limit Configuration

When users configure a smaller output limit than the model's maximum, more input context becomes available:

```rust
// Reserve only 4096 tokens for output instead of model's max
manager.set_output_limit(4096);

// Effective budget increases
let budget = manager.effective_budget();
```

---

## History Persistence

The `FullHistory` (`history.rs`) provides append-only storage for conversation messages and summaries.

### MessageId and SummaryId

Unique identifiers that serve as proof tokens for message/summary existence:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(u64);

impl MessageId {
    pub fn as_u64(&self) -> u64;
    pub(crate) fn next(self) -> Self;  // For range iteration
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SummaryId(u64);
```

Both are monotonically increasing (0, 1, 2, ...) and assigned sequentially.

### HistoryEntry

Each message in history is wrapped with metadata:

```rust
pub enum HistoryEntry {
    Original {
        id: MessageId,
        message: Message,
        token_count: u32,
        created_at: SystemTime,
        /// Stream journal step ID for crash recovery linkage
        stream_step_id: Option<i64>,
    },
    Summarized {
        id: MessageId,
        message: Message,
        token_count: u32,
        summary_id: SummaryId,
        created_at: SystemTime,
        stream_step_id: Option<i64>,
    },
}
```

Key methods:

```rust
impl HistoryEntry {
    pub fn id(&self) -> MessageId;
    pub fn message(&self) -> &Message;
    pub fn token_count(&self) -> u32;
    pub fn summary_id(&self) -> Option<SummaryId>;
    pub fn stream_step_id(&self) -> Option<i64>;
    pub fn is_summarized(&self) -> bool;
    pub fn mark_summarized(&mut self, summary_id: SummaryId);
}
```

### Summary

Represents a compressed version of a contiguous message range:

```rust
pub struct Summary {
    id: SummaryId,
    /// The range of message IDs this summary covers [start, end).
    covers: Range<MessageId>,
    /// The summarized content.
    content: NonEmptyString,
    /// Token count of the summary.
    token_count: u32,
    /// Total tokens of original messages (for compression ratio tracking).
    original_tokens: u32,
    /// When this summary was created.
    created_at: SystemTime,
    /// Which model generated this summary.
    generated_by: String,
}

impl Summary {
    pub fn content(&self) -> &str;
    pub fn token_count(&self) -> u32;
    // Test-only methods for compression analysis
    #[cfg(test)] pub fn compression_ratio(&self) -> f32;
    #[cfg(test)] pub fn tokens_saved(&self) -> u32;
}
```

**Key Invariant**: Summaries must cover contiguous message ranges. The `covers` field uses a half-open range `[start, end)`.

### FullHistory

The complete conversation history container:

```rust
pub struct FullHistory {
    entries: Vec<HistoryEntry>,
    summaries: Vec<Summary>,
    next_message_id: u64,
    next_summary_id: u64,
}
```

Key methods:

```rust
impl FullHistory {
    pub fn new() -> Self;
    
    // Adding messages
    pub fn push(&mut self, message: Message, token_count: u32) -> MessageId;
    pub fn push_with_step_id(&mut self, message: Message, token_count: u32, stream_step_id: i64) -> MessageId;
    
    // Rollback (transactional recovery)
    pub fn pop_if_last(&mut self, id: MessageId) -> Option<Message>;
    
    // Adding summaries
    pub fn add_summary(&mut self, summary: Summary) -> Result<SummaryId>;
    
    // Access
    pub fn entries(&self) -> &[HistoryEntry];
    pub fn get_entry(&self, id: MessageId) -> &HistoryEntry;
    pub fn summary(&self, id: SummaryId) -> &Summary;
    
    // Statistics
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn total_tokens(&self) -> u32;
    pub fn summarized_count(&self) -> usize;
    pub fn summaries_len(&self) -> usize;
    
    // Recovery checks
    pub fn has_step_id(&self, step_id: i64) -> bool;
    pub fn orphaned_summaries(&self) -> Vec<SummaryId>;
}
```

### Orphaned Summaries

When hierarchical re-summarization occurs (a new summary covers already-summarized messages), older summaries become orphaned. The `orphaned_summaries()` method detects this:

```rust
// Detect orphaned summaries (no messages reference them)
let orphans = history.orphaned_summaries();
if !orphans.is_empty() {
    tracing::warn!("Orphaned summaries detected: {:?}", orphans);
}
```

---

## Stream Journal (Crash Recovery)

The `StreamJournal` (`stream_journal.rs`) provides SQLite-backed crash recovery for streaming responses.

### Key Invariant

**Deltas MUST be persisted BEFORE being displayed to the user.**

This write-ahead logging approach ensures durability at the cost of slightly higher latency per delta.

### Database Schema

```sql
CREATE TABLE stream_journal (
    step_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,      -- 'text_delta', 'done', 'error'
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    sealed INTEGER DEFAULT 0,
    PRIMARY KEY(step_id, seq)
);

CREATE INDEX idx_journal_unsealed ON stream_journal(sealed) WHERE sealed = 0;

CREATE TABLE step_counter (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    next_step_id INTEGER NOT NULL DEFAULT 1
);

-- Track which steps have been committed to history
CREATE TABLE step_metadata (
    step_id INTEGER PRIMARY KEY,
    model_name TEXT,
    committed INTEGER DEFAULT 0,
    created_at TEXT NOT NULL
);
```

### StreamJournal

```rust
pub struct StreamJournal {
    db: Connection,
    journal_id: u64,
    active_step: Option<StepId>,
}

impl StreamJournal {
    // Opening
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;
    pub fn open_in_memory() -> Result<Self>;  // For testing
    
    // Session management
    pub fn begin_session(&mut self, model_name: impl Into<String>) -> Result<ActiveJournal>;
    
    // Recovery
    pub fn recover(&self) -> Result<Option<RecoveredStream>>;
    pub fn seal_unsealed(&mut self, step_id: StepId) -> Result<String>;
    pub fn discard_unsealed(&mut self, step_id: StepId) -> Result<u64>;
    
    // Commit lifecycle
    pub fn commit_and_prune_step(&mut self, step_id: StepId) -> Result<u64>;
    pub fn discard_step(&mut self, step_id: StepId) -> Result<u64>;
    
    // Statistics
    pub fn stats(&self) -> Result<JournalStats>;
}
```

### ActiveJournal (RAII Handle)

Proof that a stream is in-flight:

```rust
pub struct ActiveJournal {
    journal_id: u64,
    step_id: StepId,
    next_seq: u64,
    model_name: String,
}

impl ActiveJournal {
    pub fn step_id(&self) -> StepId;
    pub fn model_name(&self) -> &str;
    
    // Append events (persist before display!)
    pub fn append_text(&mut self, journal: &mut StreamJournal, content: impl Into<String>) -> Result<()>;
    pub fn append_done(&mut self, journal: &mut StreamJournal) -> Result<()>;
    pub fn append_error(&mut self, journal: &mut StreamJournal, message: impl Into<String>) -> Result<()>;
    
    // Finalization (consumes self)
    pub fn seal(self, journal: &mut StreamJournal) -> Result<String>;
    pub fn discard(self, journal: &mut StreamJournal) -> Result<u64>;
}
```

### RecoveredStream

Recovery state after crash:

```rust
pub enum RecoveredStream {
    /// The stream ended cleanly but was not sealed.
    Complete {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        model_name: Option<String>,
    },
    /// The stream ended with an error but was not sealed.
    Errored {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        error: String,
        model_name: Option<String>,
    },
    /// The stream ended mid-flight.
    Incomplete {
        step_id: StepId,
        partial_text: String,
        last_seq: u64,
        model_name: Option<String>,
    },
}
```

### Typical Workflow

```rust
// 1. Open journal
let mut journal = StreamJournal::open("~/.forge/stream.db")?;

// 2. Check for crash recovery on startup
if let Some(recovered) = journal.recover()? {
    match recovered {
        RecoveredStream::Complete { step_id, partial_text, model_name, .. } => {
            // Stream finished - seal and add to history
            journal.seal_unsealed(step_id)?;
            // Add partial_text as assistant message
        }
        RecoveredStream::Incomplete { step_id, .. } => {
            // Stream interrupted - discard or seal based on policy
            journal.discard_unsealed(step_id)?;
        }
        RecoveredStream::Errored { step_id, error, .. } => {
            // Handle error recovery
            journal.discard_step(step_id)?;
        }
    }
}

// 3. Begin streaming session
let mut active = journal.begin_session("claude-sonnet-4")?;

// 4. Persist each delta BEFORE displaying
for chunk in stream_response().await {
    active.append_text(&mut journal, &chunk)?;  // Persist first
    display_to_user(&chunk);                     // Then display
}
active.append_done(&mut journal)?;

// 5. Seal when complete
let full_text = active.seal(&mut journal)?;

// 6. After history save succeeds, commit and prune
manager.push_message_with_step_id(assistant_msg, step_id);
manager.save("history.json")?;
journal.commit_and_prune_step(step_id)?;
```

### Commit/Prune Atomicity

The `commit_and_prune_step()` method performs an atomic transaction:

1. Marks the step as committed in metadata
2. Deletes all journal entries for the step
3. Deletes the step metadata

This must be called AFTER history has been successfully persisted. If any part fails, nothing is changed (transactional safety).

---

## Tool Journal

The `ToolJournal` (`tool_journal.rs`) provides crash recovery for tool call batches.

### Database Schema

```sql
CREATE TABLE tool_batches (
    batch_id INTEGER PRIMARY KEY,
    model_name TEXT NOT NULL,
    assistant_text TEXT NOT NULL,
    committed INTEGER DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE tool_calls (
    batch_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    tool_call_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    PRIMARY KEY (batch_id, seq)
);

CREATE TABLE tool_results (
    batch_id INTEGER NOT NULL,
    tool_call_id TEXT NOT NULL,
    content TEXT NOT NULL,
    is_error INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (batch_id, tool_call_id)
);
```

### ToolJournal

```rust
pub struct ToolJournal {
    db: Connection,
}

impl ToolJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;
    
    // Batch lifecycle (non-streaming)
    pub fn begin_batch(&mut self, model_name: &str, assistant_text: &str, calls: &[ToolCall]) -> Result<ToolBatchId>;
    
    // Streaming batch lifecycle
    pub fn begin_streaming_batch(&mut self, model_name: &str) -> Result<ToolBatchId>;
    pub fn record_call_start(&mut self, batch_id: ToolBatchId, seq: usize, tool_call_id: &str, tool_name: &str) -> Result<()>;
    pub fn append_call_args(&mut self, batch_id: ToolBatchId, tool_call_id: &str, delta: &str) -> Result<()>;
    pub fn update_assistant_text(&mut self, batch_id: ToolBatchId, assistant_text: &str) -> Result<()>;
    
    // Recording results
    pub fn record_result(&mut self, batch_id: ToolBatchId, result: &ToolResult) -> Result<()>;
    
    // Finalization
    pub fn commit_batch(&mut self, batch_id: ToolBatchId) -> Result<()>;
    pub fn discard_batch(&mut self, batch_id: ToolBatchId) -> Result<()>;
    
    // Recovery
    pub fn recover(&self) -> Result<Option<RecoveredToolBatch>>;
}
```

### RecoveredToolBatch

```rust
pub struct RecoveredToolBatch {
    pub batch_id: ToolBatchId,
    pub model_name: String,
    pub assistant_text: String,
    pub calls: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
}
```

---

## Working Context and Token Budget

The `WorkingContext` (`working_context.rs`) represents the derived view of what will be sent to the LLM API.

### ContextSegment

```rust
pub enum ContextSegment {
    /// Use the original message from history.
    Original { id: MessageId, tokens: u32 },
    /// Use a summary instead of original messages.
    Summarized {
        summary_id: SummaryId,
        /// Original message IDs that this replaces.
        replaces: Vec<MessageId>,
        tokens: u32,
    },
}

impl ContextSegment {
    pub fn original(id: MessageId, tokens: u32) -> Self;
    pub fn summarized(summary_id: SummaryId, replaces: Vec<MessageId>, tokens: u32) -> Self;
    pub fn is_summarized(&self) -> bool;
    pub fn tokens(&self) -> u32;
}
```

### WorkingContext

```rust
pub struct WorkingContext {
    segments: Vec<ContextSegment>,
    token_budget: u32,
}

impl WorkingContext {
    pub fn new(token_budget: u32) -> Self;
    
    // Building
    pub fn push_original(&mut self, id: MessageId, tokens: u32);
    pub fn push_summary(&mut self, summary_id: SummaryId, replaces: Vec<MessageId>, tokens: u32);
    
    // Access
    pub fn segments(&self) -> &[ContextSegment];
    pub fn total_tokens(&self) -> u32;
    pub fn token_budget(&self) -> u32;
    pub fn summary_count(&self) -> usize;
    
    // Materialization
    pub fn materialize(&self, history: &FullHistory) -> Vec<Message>;
}
```

### Summary Injection

When materializing, summaries are injected as system messages with a prefix:

```rust
pub(crate) const SUMMARY_PREFIX: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Earlier conversation summary]");

// Materialization creates:
Message::system(SUMMARY_PREFIX + "\n" + summary.content())
```

### Context Building Algorithm

The `build_working_context()` algorithm in `manager.rs` runs in five phases:

**Phase 1: Reserve Recent Messages**

The N most recent messages (default: 4 from `SummarizationConfig::preserve_recent`) are always included. These represent immediate conversation context and are never summarized.

```rust
const DEFAULT_PRESERVE_RECENT: usize = 4;

let preserve_count = config.preserve_recent.min(entries.len());
let tokens_for_recent: u32 = entries[recent_start..].iter()
    .map(|e| e.token_count())
    .sum();

// If recent messages exceed budget, fail with unrecoverable error
if tokens_for_recent > budget {
    return Err(ContextBuildError::RecentMessagesTooLarge { ... });
}
```

**Phase 2: Partition Older Messages into Blocks**

Older messages are grouped into contiguous blocks:

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

**Phase 3: Select Content (Newest to Oldest)**

Starting from the most recent older block, include content while staying within budget:

- **Summarized Block**: Prefer originals if they fit; else use summary; else mark for re-summarization
- **Unsummarized Block**: Include as many recent messages as fit; mark the rest for summarization

**Phase 4: Assemble in Chronological Order**

Selected segments are reversed back to chronological order, then recent messages appended.

**Phase 5: Return or Request Summarization**

If unsummarized messages don't fit, return `SummarizationNeeded` with the list of message IDs.

### ContextUsage (UI Statistics)

```rust
pub struct ContextUsage {
    /// Tokens currently used in working context.
    pub used_tokens: u32,
    /// Token budget for current model.
    pub budget_tokens: u32,
    /// Count of summaries in context.
    pub summarized_segments: usize,
}

impl ContextUsage {
    pub fn from_context(ctx: &WorkingContext) -> Self;
    
    /// Usage as a percentage (0.0 - 100.0).
    pub fn percentage(&self) -> f32;
    
    /// Format for status bar: "2.1k / 200k (1%)" or "50k / 200k (25%) [2S]"
    pub fn format_compact(&self) -> String;
    
    /// Severity level: 0=green (<70%), 1=yellow (70-90%), 2=red (>90%)
    pub fn severity(&self) -> u8;
}
```

---

## Summarization System

The `summarization.rs` module provides LLM-based conversation summarization.

### Summarization Models

Cheaper/faster models are used for summarization:

| Provider | Model | Input Limit |
|----------|-------|-------------|
| Claude | `claude-3-haiku-20240307` | 190,000 tokens |
| OpenAI | `gpt-4o-mini` | 120,000 tokens |

```rust
pub fn summarization_model(provider: Provider) -> &'static str;
pub fn summarizer_input_limit(provider: Provider) -> u32;
```

### SummarizationConfig

```rust
pub struct SummarizationConfig {
    /// Target compression ratio (e.g., 0.15 = 15% of original size).
    pub target_ratio: f32,        // Default: 0.15
    /// Don't summarize the N most recent messages.
    pub preserve_recent: usize,   // Default: 4
}
```

Target tokens are calculated as:
```rust
let target_tokens = (original_tokens * target_ratio)
    .clamp(MIN_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS);  // 64..2048
```

### Summarization Flow

**Step 1: Prepare Summarization**

```rust
pub fn prepare_summarization(&mut self, message_ids: &[MessageId]) -> Option<PendingSummarization>

pub struct PendingSummarization {
    pub scope: SummarizationScope,
    pub messages: Vec<(MessageId, Message)>,
    pub original_tokens: u32,
    pub target_tokens: u32,
}

pub struct SummarizationScope {
    ids: Vec<MessageId>,
    range: Range<MessageId>,  // [start, end)
}
```

The method:
1. Sorts and deduplicates message IDs
2. Extracts first contiguous run (summaries must be contiguous)
3. Calculates target tokens based on compression ratio
4. Returns `PendingSummarization` with messages to summarize

**Step 2: Generate Summary (Async)**

```rust
pub async fn generate_summary(
    config: &ApiConfig,
    counter: &TokenCounter,
    messages: &[(MessageId, Message)],
    target_tokens: u32,
) -> Result<String>
```

The prompt instructs the model to:
- Preserve key facts, decisions, and important context
- Maintain chronological flow
- Stay within target token count
- Use clear, direct language
- Preserve essential code snippets and file paths
- Note unresolved questions or pending actions
- Format as continuous narrative (not bullet points)

**Step 3: Complete Summarization**

```rust
pub fn complete_summarization(
    &mut self,
    scope: SummarizationScope,
    content: NonEmptyString,
    generated_by: String,
) -> Result<SummaryId>
```

This method:
1. Counts tokens in the generated summary
2. Creates a `Summary` with metadata
3. Adds to history via `add_summary()`
4. Returns the new `SummaryId`

### SummarizationNeeded Error

```rust
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}
```

---

## Model Limits Registry

The `model_limits.rs` module provides per-model token constraints.

### ModelLimits

```rust
pub struct ModelLimits {
    /// Maximum input context window in tokens.
    context_window: u32,
    /// Maximum output tokens the model can generate.
    max_output: u32,
}

impl ModelLimits {
    pub const fn new(context_window: u32, max_output: u32) -> Self;
    pub const fn context_window(&self) -> u32;
    pub const fn max_output(&self) -> u32;
    
    /// Effective budget = context_window - max_output - 5% safety margin
    pub fn effective_input_budget(&self) -> u32;
    
    /// With custom reserved output (for configured output limits)
    pub fn effective_input_budget_with_reserved(&self, reserved_output: u32) -> u32;
}
```

**Budget Calculation Example** (Claude Sonnet 4: 200k context, 64k output):

```
available = 200,000 - 64,000 = 136,000
safety_margin = 136,000 / 20 = 6,800 (5%)
effective_budget = 136,000 - 6,800 = 129,200 tokens
```

The 5% safety margin accounts for:
- Token counting inaccuracies (especially for Claude)
- System prompt overhead
- Tool definitions and formatting

### Known Model Limits

```rust
const KNOWN_MODELS: &[(&str, ModelLimits)] = &[
    // Claude models (most specific first)
    ("claude-opus-4", ModelLimits::new(200_000, 64_000)),
    ("claude-sonnet-4", ModelLimits::new(200_000, 64_000)),
    ("claude-3-5", ModelLimits::new(200_000, 64_000)),
    ("claude-3", ModelLimits::new(200_000, 64_000)),
    ("claude", ModelLimits::new(200_000, 64_000)),
    // GPT models (most specific first)
    ("gpt-5", ModelLimits::new(400_000, 128_000)),
    ("gpt-4o", ModelLimits::new(128_000, 16_384)),
    ("gpt-4-turbo", ModelLimits::new(128_000, 4096)),
    ("gpt-4", ModelLimits::new(8192, 4096)),
    ("gpt-3.5", ModelLimits::new(16_385, 4096)),
];

const DEFAULT_LIMITS: ModelLimits = ModelLimits::new(8192, 4096);
```

### ModelRegistry

```rust
pub struct ModelRegistry {
    overrides: HashMap<String, ModelLimits>,
}

impl ModelRegistry {
    pub fn new() -> Self;
    
    /// Lookup with prefix matching, returns source information
    pub fn get(&self, model: &str) -> ResolvedModelLimits;
    
    // Test-only methods
    #[cfg(test)] pub fn set_override(&mut self, model: String, limits: ModelLimits);
    #[cfg(test)] pub fn remove_override(&mut self, model: &str) -> Option<ModelLimits>;
    #[cfg(test)] pub fn has_override(&self, model: &str) -> bool;
}
```

### ResolvedModelLimits

Explicit source tracking for debugging:

```rust
pub struct ResolvedModelLimits {
    limits: ModelLimits,
    source: ModelLimitsSource,
}

pub enum ModelLimitsSource {
    /// Exact match from an override.
    Override,
    /// Matched a known prefix (the matched prefix).
    Prefix(&'static str),
    /// Fell back to DEFAULT_LIMITS.
    DefaultFallback,
}

// Usage
let resolved = registry.get("claude-sonnet-4-20250514");
match resolved.source() {
    ModelLimitsSource::Prefix("claude-sonnet-4") => { /* matched */ }
    ModelLimitsSource::DefaultFallback => { /* unknown model */ }
    _ => {}
}
```

---

## Token Counting

The `token_counter.rs` module provides approximate token counting using tiktoken.

### Accuracy Notes

Token counts are **approximate**:

- **OpenAI models (GPT-4, GPT-3.5)**: Reasonably accurate with cl100k_base
- **Claude models**: Anthropic uses a different tokenizer; counts may vary by ~5-10%
- **GPT-5.x models**: May use updated tokenization not reflected in cl100k_base
- **Message overhead**: The fixed 4-token overhead per message is an approximation

The 5% safety margin in `ModelLimits::effective_input_budget()` compensates for these inaccuracies.

### Singleton Pattern

The tiktoken encoder is expensive to initialize. A singleton pattern is used:

```rust
static ENCODER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_encoder() -> Option<&'static CoreBPE> {
    ENCODER.get_or_init(|| cl100k_base().ok()).as_ref()
}
```

Creating multiple `TokenCounter` instances is cheap - they share the encoder.

### TokenCounter

```rust
#[derive(Clone, Copy)]
pub struct TokenCounter {
    encoder: Option<&'static CoreBPE>,
}

impl TokenCounter {
    pub fn new() -> Self;
    
    /// Count tokens in a string.
    pub fn count_str(&self, text: &str) -> u32;
    
    /// Count message tokens including ~4 token overhead for role/formatting.
    pub fn count_message(&self, msg: &Message) -> u32;
    
    // Test-only
    #[cfg(test)] pub fn count_messages(&self, messages: &[Message]) -> u32;
}

impl Default for TokenCounter {
    fn default() -> Self { Self::new() }
}
```

### Message Overhead

Each message has approximately 4 tokens of overhead for:
- Role markers (e.g., "user", "assistant")
- Message structure/delimiters

```rust
const MESSAGE_OVERHEAD: u32 = 4;

fn count_message(&self, msg: &Message) -> u32 {
    let content_tokens = self.count_str(msg.content());
    let role_tokens = self.count_str(msg.role_str());
    content_tokens + role_tokens + MESSAGE_OVERHEAD
}
```

---

## Key Data Structures

### Type Relationships

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

### Proof Token Pattern

Several types serve as proof tokens ensuring operations occur in valid states:

| Type | Purpose |
|------|---------|
| `MessageId` | Proof of message existence in history |
| `SummaryId` | Proof of summary existence |
| `ActiveJournal` | Proof that a stream is in-flight (RAII) |
| `PreparedContext` | Proof that context fits within budget |

### RAII Handles

- `ActiveJournal`: Consumed by `seal()` or `discard()`, ensuring stream lifecycle is properly managed
- `PreparedContext`: Borrows `ContextManager`, ensuring context remains valid during API call

---

## Error Handling

The crate uses `anyhow::Result` for most fallible operations and custom error types for domain-specific failures.

### ContextBuildError

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

`SummarizationNeeded` is recoverable by running summarization. `RecentMessagesTooLarge` requires user intervention (reduce input or switch to larger model).

### SummarizationNeeded

```rust
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}
```

### Journal Errors

Journal operations use `anyhow::Result` with contextual error messages:

```rust
bail!("Cannot begin session: already streaming step {}", step_id);
bail!("Cannot begin session: recoverable step {} exists", step_id);
bail!("No active streaming session");
```

---

## Persistence Format

### History JSON Format

The `FullHistory` is serialized to JSON with validation on deserialization:

```json
{
  "entries": [
    {
      "id": 0,
      "message": { "role": "user", "content": "Hello" },
      "token_count": 10,
      "summary_id": null,
      "created_at": { "secs_since_epoch": 1704067200, "nanos_since_epoch": 0 }
    },
    {
      "id": 1,
      "message": { "role": "assistant", "content": "Hi there!" },
      "token_count": 12,
      "summary_id": 0,
      "created_at": { "secs_since_epoch": 1704067201, "nanos_since_epoch": 0 }
    }
  ],
  "summaries": [
    {
      "id": 0,
      "covers": { "start": 0, "end": 2 },
      "content": "User greeted assistant...",
      "token_count": 8,
      "original_tokens": 22,
      "created_at": { "secs_since_epoch": 1704067300, "nanos_since_epoch": 0 },
      "generated_by": "claude-3-haiku-20240307"
    }
  ],
  "next_message_id": 2,
  "next_summary_id": 1
}
```

### Validation on Load

Deserialization validates:

1. Message IDs are sequential (0, 1, 2, ...)
2. Summary IDs are sequential
3. `next_message_id` matches entry count
4. `next_summary_id` matches summary count
5. Summary ranges reference valid messages
6. Summarized entries reference valid summaries within their range

### Atomic Save

The `save()` method uses an atomic write pattern:

1. Write to temp file with `.tmp` extension
2. On Unix: Rename over existing file
3. On Windows: Backup existing file, rename new file, delete backup

```rust
pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, json)?;
    
    if let Err(err) = std::fs::rename(&tmp_path, path) {
        // Windows: backup-restore pattern
        let backup_path = path.with_extension("bak");
        std::fs::rename(path, &backup_path)?;
        if let Err(rename_err) = std::fs::rename(&tmp_path, path) {
            std::fs::rename(&backup_path, path)?;  // Restore
            return Err(rename_err.into());
        }
        std::fs::remove_file(&backup_path)?;
    }
    Ok(())
}
```

---

## Extension Guide

### Adding a New Provider

1. **Add model limits** to `KNOWN_MODELS` in `model_limits.rs`:

```rust
const KNOWN_MODELS: &[(&str, ModelLimits)] = &[
    // ... existing ...
    ("new-provider-model", ModelLimits::new(context_window, max_output)),
];
```

2. **Add summarization model** in `summarization.rs`:

```rust
const NEW_PROVIDER_SUMMARIZATION_MODEL: &str = "new-provider-small";
const NEW_PROVIDER_SUMMARIZER_INPUT_LIMIT: u32 = 100_000;

pub fn summarization_model(provider: Provider) -> &'static str {
    match provider {
        // ... existing ...
        Provider::NewProvider => NEW_PROVIDER_SUMMARIZATION_MODEL,
    }
}
```

3. **Implement `generate_summary_*`** for the new provider's API format.

### Adjusting Summarization Behavior

Modify `SummarizationConfig` in `manager.rs`:

```rust
SummarizationConfig {
    target_ratio: 0.10,   // More aggressive compression (10%)
    preserve_recent: 6,   // Keep more recent messages
}
```

### Custom Token Counting

To use a provider-specific tokenizer:

1. Create a new counter type implementing the same interface
2. Replace `TokenCounter` in `ContextManager`

```rust
pub trait TokenCounting {
    fn count_str(&self, text: &str) -> u32;
    fn count_message(&self, msg: &Message) -> u32;
}
```

### Adding New Journal Event Types

In `stream_journal.rs`, extend `StreamDeltaEvent`:

```rust
enum StreamDeltaEvent {
    TextDelta(String),
    Done,
    Error(String),
    // Add new variant
    NewEventType { ... },
}

impl StreamDeltaEvent {
    const fn event_type(&self) -> &'static str {
        match self {
            // ... existing ...
            StreamDeltaEvent::NewEventType { .. } => "new_event",
        }
    }
}
```

---

## Testing

```bash
# Run all context crate tests
cargo test -p forge-context

# Run with output
cargo test -p forge-context -- --nocapture

# Run specific test
cargo test -p forge-context test_build_context_simple
```

The crate includes comprehensive unit tests for all modules. Integration tests requiring API keys are marked with `#[ignore]`.

### Test Utilities

- `MessageId::new_for_test(u64)` - Create test message IDs
- `SummaryId::new_for_test(u64)` - Create test summary IDs
- `StreamJournal::open_in_memory()` - In-memory journal for tests
- `ToolJournal::open_in_memory()` - In-memory tool journal for tests
- `ModelRegistry::set_override()` - Override model limits in tests

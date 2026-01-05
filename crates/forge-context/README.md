# forge-context

Adaptive context window management with summarization and persistence for LLM applications.

## Overview

`forge-context` provides intelligent context window management for conversations with Large Language Models. It solves the fundamental problem of LLMs having finite context windows while users expect seamless, long-running conversations.

**Key capabilities:**

- **Adaptive context management**: Automatically adjusts when switching between models with different context limits
- **Summarization-based compression**: Compresses older conversation segments using cheaper LLM models while preserving key information
- **Full history preservation**: Never discards original messages; summaries are additive, allowing restoration when budget permits
- **Crash recovery**: Write-ahead logging ensures streaming responses survive application crashes
- **Accurate token counting**: Uses tiktoken for precise token budgeting compatible with OpenAI and Anthropic models

## Architecture

```
ContextManager (orchestrator)
|-- history: FullHistory        # Append-only message storage
|-- counter: TokenCounter       # tiktoken-based counting
|-- registry: ModelRegistry     # Per-model token limits
+-- [external] StreamJournal    # Crash recovery (separate lifecycle)

PreparedContext (ephemeral proof)
+-- working_context: WorkingContext  # Derived view for API calls
```

### Design Principles

1. **Append-only history**: Messages are never deleted. Summaries link to original messages, enabling restoration when switching to models with larger context windows.

2. **Type-driven correctness**: `PreparedContext` serves as a proof token that context was successfully built within budget before an API call.

3. **Explicit summarization**: The manager signals when summarization is needed rather than silently truncating. Callers control when and how summarization occurs.

4. **Write-ahead durability**: Stream deltas are persisted to SQLite before display, ensuring recoverability after crashes.

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
| `claude-opus-4` | 200,000 | 64,000 |
| `claude-sonnet-4` | 200,000 | 64,000 |
| `claude-3-5` | 200,000 | 64,000 |
| `claude-3` | 200,000 | 64,000 |
| `claude` | 200,000 | 64,000 |
| `gpt-4o` | 128,000 | 16,384 |
| `gpt-4-turbo` | 128,000 | 4,096 |
| `gpt-4` | 8,192 | 4,096 |
| `gpt-3.5` | 16,385 | 4,096 |

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

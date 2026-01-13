# ContextInfinity™ Software Requirements Document (SRD)

> Note: This is the authoritative ContextInfinity spec; `context/README.md` is implementation notes and may lag behind.

**Document version:** 0.31
**Status:** Draft

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-23 | Header & TOC |
| 24-71 | 1. Overview |
| 72-124 | 2. Normative Language, Definitions, and Invariants |
| 125-190 | 3. Architecture |
| 191-231 | 4. Identifiers, Ordering, and Hashing |
| 232-494 | 5. Data Model |
| 495-583 | 6. Turn Lifecycle, Atomicity, and Recovery |
| 584-618 | 7. Context Assembly and Budgeting |
| 619-653 | 8. Retrieval Strategy |
| 654-680 | 9. Chunking and Indexing |
| 681-715 | 10. Summarization (Librarian) |
| 716-780 | 11. Provider Integration |
| 781-807 | 12. Tools Policy |
| 809-841 | 13. Streaming Durability |
| 842-873 | 14. Security, Privacy, and Prompt Hardening |
| 874-895 | 15. Observability and Telemetry |
| 896-1010 | 16. Storage Implementation (SQLite) |
| 1011-1037 | 17. TUI Requirements |
| 1038-1090 | 18. Configuration |
| 1091-1104 | 19. Risks and Mitigations |
| 1105-1117 | 20. Success Metrics |
| 1118-1151 | 21. Implementation Phases |
| 1152-1160 | 22. Acceptance Tests |

---

## 1. Overview

### 1.1 Purpose

ContextInfinity™ is a context management system that enables effectively unlimited conversation history for LLM-driven applications by maintaining a **bounded working context** and performing **on-demand retrieval** of full-fidelity historical content.

### 1.2 Core Principles

1. **Asymmetric context**

   * Users see full responses.
   * The primary LLM sees an assembled working set: system instructions + bounded state + selected retrieved history + current user input.

2. **Lossless persistence, lossy working state**

   * Full-fidelity transcript is preserved immutably once persisted.
   * The rolling state is a bounded derived summary; omitted detail remains recoverable via retrieval from durable storage.

3. **Model-agnostic state**

   * The stored “state” is provider-neutral and supports switching providers/models without rewriting history.

4. **Explicit turn closure**

   * The system distinguishes between a user-visible response (may stream) and a committed state transition.
   * Pending (uncommitted) turns are explicitly included in subsequent prompts until committed.

5. **Provider-stateless correctness by default**

   * Correctness MUST NOT depend on provider-side conversation storage.
   * Provider-side continuity features (e.g., OpenAI `previous_response_id` / conversation objects) are optional, feature-flagged optimizations.

### 1.3 Goals

* Keep the active context window bounded independent of conversation length.
* Preserve full-fidelity transcript and retrieval artifacts durably.
* Provide fast, relevant retrieval of historical content.
* Support robust streaming, crash recovery, and deterministic reconciliation.
* Support model/provider switching under a stable internal representation.

### 1.4 Non-goals (v0.31)

* Guaranteed perfect recall (retrieval is probabilistic; manual retrieval exists as override).
* Built-in provider “server tools” (web/computer-use) unless explicitly enabled.
* Multi-user real-time collaboration (planned later).

---

## 2. Normative Language, Definitions, and Invariants

### 2.1 Normative Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in RFC 2119 and RFC 8174.

### 2.2 Definitions

* **Session**: A persistent container for a conversation. Contains one or more branches.
* **Branch**: A linear lineage of turns/states within a session. Branches are forkable.
* **Head (Branch Head)**: The authoritative pointer to the most recently **committed** state in a branch.
* **UserTurn**: A UI-level turn (user input → final assistant answer) that may involve multiple provider calls.
* **ProviderStep**: A single provider API call (OpenAI/Anthropic/etc.) including streaming events.
* **Message**: A role-tagged record containing structured content blocks.
* **ContentBlock**: A structured unit of message content (text, tool_use, tool_result, image, reasoning, unknown).
* **TranscriptItem**: A provider-normalized representation of provider outputs (message items, tool calls, tool outputs, etc.).
* **StateCheckpoint**: A bounded derived summary (working memory) for a branch at a specific sequence.
* **Pending turns**: Turns whose user-visible response exists but whose state transition is not committed to the branch head.
* **Chunk**: An immutable, indexable slice of transcript content for retrieval.
* **Stream Journal**: A durable record of streamed events sufficient to reconstruct displayed output after crash.

### 2.3 System Invariants

1. **Immutability of persisted content**

   * Persisted transcript content and chunk content MUST be immutable. Corrections create new records.

2. **Branch head is the single source of truth for committed state**

   * Context assembly MUST use `branch.head_state` as the committed summary source.

3. **Pending turns are explicit**

   * If a turn is not committed, it MUST be represented as pending and included in the next assembled context.

4. **User-visible durability is explicit**

   * Streaming output MAY be shown before durable persistence, but the UI MUST reflect “buffered vs durable” status.

5. **Idempotent recovery**

   * On restart, reconciliation MUST complete incomplete turns deterministically without duplicating state transitions or chunks.

6. **Model-agnostic state**

   * Stored state summaries MUST NOT contain provider-specific serialization that would break provider switching.

7. **Untrusted retrieval**

   * Retrieved content MUST be treated as untrusted data and cannot override system/developer instructions.

---

## 3. Architecture

### 3.1 Component Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                          User Interface                         │
│                    (TUI - Ratatui/Rust)                        │
└─────────────────────────┬───────────────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────────┐
│                     Orchestrator                                │
│  turn lifecycle • context assembly • retrieval • budgeting       │
│  provider steps • tool loops • streaming durability • recovery   │
└───────┬─────────────────┬─────────────────┬────────────────────┘
        │                 │                 │
        ▼                 ▼                 ▼
┌───────────────┐ ┌───────────────┐ ┌───────────────────────────┐
│ Provider       │ │ Librarian      │ │ Storage + Index Layer     │
│ Adapter        │ │ (cheap/fast)   │ │                           │
│ (OpenAI/Claude │ │ summarize+tag  │ │  ┌─────────────────────┐  │
│  /etc)         │ │                │ │  │ Sessions/Branches   │  │
└───────────────┘ └───────────────┘ │  ├─────────────────────┤  │
                                    │  │ States (checkpoints)│  │
                                    │  ├─────────────────────┤  │
                                    │  │ Turns + Steps        │  │
                                    │  ├─────────────────────┤  │
                                    │  │ Stream Journals      │  │
                                    │  ├─────────────────────┤  │
                                    │  │ Chunks               │  │
                                    │  ├─────────────────────┤  │
                                    │  │ Vector Index         │  │
                                    │  └─────────────────────┘  │
                                    └───────────────────────────┘
```

### 3.2 Responsibilities

#### 3.2.1 Orchestrator

* Accept user input; create turns; drive provider steps.
* Assemble context from committed state + pending turns + retrieval.
* Enforce budgets and apply deterministic shrinker when needed.
* Persist durable transcript and streamed events; surface durability state.
* Run post-processing (summarization, chunking, indexing) synchronously or via durable job queue.
* Crash recovery: scan and reconcile incomplete work.

#### 3.2.2 Provider Adapter

* Map internal context representation into provider-specific request schema.
* Stream events; normalize provider output into `TranscriptItem`.
* Enforce provider-specific constraints (tool ordering, stop reasons, truncation behavior).

#### 3.2.3 Librarian

* Produce state updates under strict schema/constraints.
* Produce chunk summaries and metadata.

#### 3.2.4 Storage + Index

* Provide ACID persistence for sessions/branches/states/turns/steps.
* Maintain immutable transcript and chunk store.
* Maintain vector index and (optional) keyword index.

---

## 4. Identifiers, Ordering, and Hashing

### 4.1 Identifier Types

All durable IDs MUST be globally unique.

Recommended: ULID or UUIDv7.

```rust
type SessionId = ulid::Ulid;
type BranchId  = ulid::Ulid;
type TurnId    = ulid::Ulid;
type StepId    = ulid::Ulid;
type StateId   = ulid::Ulid;
type ChunkId   = ulid::Ulid;
```

### 4.2 Branch-Local Sequences

For UI and deterministic ordering, each branch maintains monotonic sequences:

* `turn_seq: u64` increments per UserTurn accepted on that branch.
* `state_seq: u64` increments per committed state transition.

These sequences are not global identifiers.

### 4.3 Content Hashes

The system SHOULD compute a stable `content_hash` (e.g., SHA-256) for:

* transcript blocks (rendered text)
* chunk content

Hashes are used for:

* deduplication
* idempotent retries
* integrity checks

---

## 5. Data Model

### 5.1 Content Blocks

Internal content MUST support provider block/item structures.

```rust
type Json = serde_json::Value;

#[derive(Clone, Debug)]
enum ContentBlock {
    Text {
        text: String,
        annotations: Vec<Json>,
    },
    ToolUse {
        tool_use_id: String, // Claude tool_use.id OR OpenAI call_id
        name: String,
        input: Json,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    Image {
        media_type: String,
        source: Json,
    },
    Reasoning {
        encrypted_content: Option<String>,
        summary: Option<String>,
        raw: Option<Json>,
    },
    Unknown {
        provider: String,
        raw: Json,
    },
}
```

**Requirements**

* Unknown block types MUST be preserved as `Unknown { raw }`.
* A message MUST maintain both structured blocks and a `rendered_text` view.

### 5.2 Message

```rust
#[derive(Clone, Copy, Debug)]
enum Role { User, Assistant }

#[derive(Clone, Debug)]
struct Message {
    role: Role,
    blocks: Vec<ContentBlock>,
    rendered_text: String,     // concatenation of visible text blocks
}
```

### 5.3 Provider-Normalized Transcript Items

The system stores a normalized transcript for analysis, replay, indexing, and tool loops.

```rust
#[derive(Clone, Debug)]
enum TranscriptItem {
    AssistantMessage { blocks: Vec<ContentBlock> },
    ToolCall { call_id: String, name: String, arguments: Json },
    ToolOutput { call_id: String, output: Json },
    ReasoningItem { encrypted_content: Option<String>, raw: Option<Json> },
    Metadata { key: String, value: String },
    UnknownItem { provider: String, raw: Json },
}
```

### 5.4 Session and Branch

```rust
struct Session {
    id: SessionId,
    title: Option<String>,
    created_at: Timestamp,
    active_branch: BranchId,
}

struct Branch {
    id: BranchId,
    session_id: SessionId,
    created_at: Timestamp,

    base_state: StateId,  // where it forked
    head_state: StateId,  // last committed
    head_seq: u64,        // branch-local committed seq

    label: Option<String>,
}
```

### 5.5 State Checkpoint

State is provider-agnostic and bounded.

```rust
struct StateCheckpoint {
    id: StateId,
    session_id: SessionId,
    branch_id: BranchId,

    state_seq: u64,
    parent_id: Option<StateId>,

    summary: String,       // bounded “working memory” text
    pinned_facts: String,  // always included verbatim

    token_count_est: u32,
    created_at: Timestamp,
    schema_version: u32,
}
```

### 5.6 Turn Lifecycle Model

A UserTurn may contain multiple ProviderSteps.

```rust
enum TurnPhase {
    Accepted,
    ContextPrepared,
    Responding,
    ResponseFinalized,
    StateCommitted,
    Indexed,
    Done,
    Failed,
}

struct UserTurn {
    id: TurnId,
    session_id: SessionId,
    branch_id: BranchId,
    turn_seq: u64,

    state_before: StateId,
    state_after: Option<StateId>,

    phase: TurnPhase,

    user_message: Message,
    final_assistant_message: Option<Message>,

    provider_steps: Vec<StepId>,

    retrieved_chunks: Vec<ChunkId>,
    created_chunks: Vec<ChunkId>,

    primary_provider: String,
    primary_model: String,

    created_at: Timestamp,
    updated_at: Timestamp,
}
```

### 5.7 Provider Step

```rust
enum StepOutcome {
    Completed,
    Incomplete { reason: String },
    Paused { reason: String },
    Failed { code: String, message: String },
    Cancelled,
}

struct ProviderStep {
    id: StepId,
    turn_id: TurnId,
    step_index: u32,

    provider: String,
    model_id: String,

    request_json: String,
    response_json: Option<String>,

    normalized_items: Vec<TranscriptItem>,
    outcome: StepOutcome,

    stream_path: Option<String>,
    stream_displayed_bytes: u64,
    stream_durable_bytes: u64,

    started_at: Timestamp,
    ended_at: Option<Timestamp>,
}
```

### 5.8 Chunk

Chunks are retrieval units derived from transcript content.

```rust
enum ChunkType {
    Code,
    Explanation,
    Decision,
    UserContext,
    ToolResult,
    Other,
}

struct Chunk {
    id: ChunkId,
    session_id: SessionId,
    branch_id: BranchId,

    source_turn: TurnId,

    rendered_text: String,
    summary: String,

    embedding_model: String,
    embedding_dim: u32,
    embedding: Vec<f32>,

    chunk_type: ChunkType,
    token_est: u32,

    content_hash: [u8; 32],
    created_at: Timestamp,
}
```

### 5.9 Assembled Context

This is the authoritative “what the model sees.”

```rust
struct AssembledContext {
    system_instructions: String,
    pinned_facts: String,

    head_state_summary: String, // from branch head
    pending_turns: Vec<Message>,

    retrieved_chunks: Vec<Chunk>,

    current_user_message: Message,

    // tooling (optional)
    tools: Vec<ToolSpec>,
    tool_mode: ToolsMode,

    // budgets
    budget: Budget,
}
```

**Requirement**: If there are uncommitted turns, they MUST appear in `pending_turns` in order.

---

## 6. Turn Lifecycle, Atomicity, and Recovery

### 6.1 Turn State Machine

```
Accepted
  -> ContextPrepared
    -> Responding
      -> ResponseFinalized
        -> StateCommitted
          -> Indexed
            -> Done
```

Any phase may transition to `Failed`.

### 6.2 Input Gating Rule

The UI MAY accept the next user message as soon as the prior turn reaches `ResponseFinalized`.

If a prior turn has `phase >= ResponseFinalized` but `state_after == NULL`:

* that turn MUST be included as a pending turn in the next assembled context.

This prevents “summary lag” from breaking coherence.

### 6.3 Transaction Boundaries (SQLite)

#### TX-0: Turn Acceptance

* Create `turns` row (phase = `Accepted`)
* Persist user message
* Increment `turn_seq` for branch

#### TX-A: Response Finalization (User-visible durability boundary)

* Persist final assistant message (structured blocks + rendered_text)
* Persist provider step final record (request/response, normalized items)
* Update `turns.phase = ResponseFinalized`

#### TX-B: State Commit (Atomic head advance)

In a single transaction:

1. Insert new `states` row
2. Update `turns.state_after`
3. Update `turns.phase = StateCommitted`
4. Update `branches.head_state` and `branches.head_seq`

If TX-B fails, the branch head remains valid.

#### TX-C: Indexing

* Persist chunks
* Write vector index entries
* Update `turns.phase = Indexed` or record “index degraded” state

Indexing MAY be asynchronous but MUST be recoverable and idempotent.

### 6.4 Durable Job Queue (Reconciliation)

If summarization/indexing are not performed synchronously, the system MUST persist jobs.

Job types (minimum):

* `summarize_turn`
* `index_turn`
* `reconcile_stream` (optional)

Startup reconciler MUST:

* find turns with `phase = ResponseFinalized` and `state_after IS NULL` and enqueue `summarize_turn` in branch order
* find turns with `phase = StateCommitted` but not indexed and enqueue `index_turn`

### 6.5 Crash Recovery Rules

On startup:

1. Identify any streaming steps with a journal and missing finalization → mark step outcome `Failed` or `Incomplete` depending on provider status if known.
2. For `ResponseFinalized` turns without `StateCommitted` → run `summarize_turn` and commit head.
3. For committed turns without indexing → run indexing.

No recovery path may duplicate:

* state commits (must be unique by branch + seq)
* chunk inserts (must be unique by content hash + source turn)

---

## 7. Context Assembly and Budgeting

### 7.1 Budget Model

```rust
struct Budget {
    base_tokens: u32,        // system + pinned + head_state_summary + pending window
    retrieval_budget: u32,
    response_reserve: u32,
    safety_margin: u32,
    model_limit: u32,
}
```

### 7.2 Provider Truncation Policy

Default policy: **provider truncation disabled**.

* If the provider supports an explicit truncation mode, use “disabled/fail” by default.
* If provider rejects for size, the system MUST apply the Context Shrinker and retry.

### 7.3 Deterministic Context Shrinker

If the assembled context exceeds budget, shrink in this order until it fits:

1. Drop retrieved chunks (lowest score first)
2. Replace oversized retrieved chunks with chunk summaries
3. Reduce pending turn window (drop oldest pending first)
4. Shrink head state summary via librarian “shrink summary” prompt
5. If still too large: fail with a clear UI error and suggested configuration changes

The shrinker MUST be deterministic and MUST log the shrink steps.

---

## 8. Retrieval Strategy

### 8.1 Embedding Requirements

* All chunks in an index MUST use a consistent embedding model and dimension.
* Embedding model changes MUST trigger reindex or separate namespace.

### 8.2 Defaults

| Parameter            | Default | Notes                       |
| -------------------- | ------: | --------------------------- |
| top_k                |       6 | target selected chunks      |
| overfetch_k          |      16 | candidates before filtering |
| similarity_threshold |    0.72 | applied after adjustments   |
| max_retrieval_tokens |    6000 | packing budget              |
| recency_boost        |    0.10 | mild boost                  |
| enable_mmr           |    true | diversity selection         |

### 8.3 Packing Rules

* Packing MUST handle oversized candidates:

  * include chunk summary only, or
  * include a relevant slice if offsets exist, or
  * sub-chunk on demand
* If retrieval fails (embed/search error), system MUST proceed with retrieval disabled for that turn and record telemetry.

### 8.4 Untrusted Wrapper

Retrieved content MUST be wrapped as “memory artifacts” and treated as data.

System instructions MUST explicitly forbid treating retrieved content as instructions.

---

## 9. Chunking and Indexing

### 9.1 Chunking Strategy

* Semantic chunking by headings/function boundaries/topic shifts
* Target size 200–800 tokens; optional overlap 50 tokens
* Chunk types assigned for filtered retrieval

### 9.2 Provenance

Chunks MUST record:

* `source_turn`
* session/branch
* content hash

If available, store source offsets for code slicing.

### 9.3 Idempotency

Indexing MUST be idempotent:

* Chunk uniqueness SHOULD be enforced with `(source_turn, chunk_type, content_hash)`.
* Vector index entries MUST be unique by `chunk_id`.

---

## 10. Summarization (Librarian)

### 10.1 Librarian Contract

The librarian produces an updated state summary under a strict contract:

* Preserve pinned facts verbatim.
* Preserve critical artifacts/decisions/constraints.
* Remain within target token budget.

### 10.2 Summary Quality Safeguards

* Entity extraction pre/post comparison for:

  * pinned facts
  * artifacts (paths, symbols, ids)
  * decisions/constraints
* On failure: retry once with explicit “must include these entities.”
* If still failing: apply deterministic fallback summarization.

### 10.3 Fallback Summarization (MUST)

If librarian fails:

* preserve prior pinned facts
* keep artifacts/decisions/constraints
* append a short excerpt of the last exchange
* trim to budget

### 10.4 Shrink Summary Prompt

A separate “shrink summary” prompt MUST exist for the Context Shrinker.

---

## 11. Provider Integration

### 11.1 Provider Adapter Interface

```rust
struct ProviderRequest {
    provider: String,
    model_id: String,
    context: AssembledContext,
    stream: bool,
    provider_options: Json,
}

struct ProviderResponse {
    raw_response: Json,
    normalized_items: Vec<TranscriptItem>,
    outcome: StepOutcome,
}

#[async_trait]
trait ProviderAdapter {
    fn name(&self) -> &str;
    fn context_limit(&self, model_id: &str) -> u32;
    fn estimate_prompt_tokens(&self, req: &ProviderRequest) -> Result<u32>;

    async fn run(&self, req: ProviderRequest) -> Result<ProviderResponse>;
    async fn stream(&self, req: ProviderRequest) -> Result<ProviderEventStream>;
}
```

Adapters MUST:

* preserve raw request/response JSON
* normalize outputs to transcript items
* enforce provider ordering constraints and tool policies
* handle unknown event types safely (log + ignore)

### 11.2 OpenAI Responses Integration Requirements

* Responses output is a typed array of items; extraction MUST not assume a single text field.
* Streaming must accumulate output text deltas keyed by provider indices.
* Outcome mapping MUST represent `Completed`, `Incomplete`, `Failed`.

Required explicit parameters (policy-driven):

* `max_output_tokens` (must exist and be bounded)
* `truncation` (default: disabled/fail)
* `store` (privacy-configured)
* `parallel_tool_calls` (tool policy-driven)

### 11.3 Anthropic Messages Integration Requirements

* `system` is top-level system prompt; do not send a “system role message.”
* Messages may use structured content blocks; streaming is block-indexed.
* Stop reasons MUST map to `Completed`, `Incomplete`, `Paused(tool_use)`, etc.

Tool ordering constraints MUST be enforced when tools are enabled:

* tool results must immediately follow tool use
* tool_result blocks must appear first in the user message content

Parallel tool use should be configurable.

---

## 12. Tools Policy

### 12.1 Modes

```rust
enum ToolsMode {
    Disabled,
    ParseOnly,
    Enabled,
}
```

* **Disabled**: do not send tool definitions; if the provider emits tool calls, treat as an error and surface in UI.
* **ParseOnly**: capture tool calls and present to user; do not execute; do not continue tool loop automatically.
* **Enabled**: execute tools, feed results back, continue provider steps until final assistant message.

### 12.2 Tool Loop Rules

* A UserTurn may include multiple ProviderSteps.
* Steps must be ordered; each step records request/response, normalized transcript items, and outcome.
* If tools are enabled, the orchestrator MUST:

  * execute tool calls
  * construct provider-specific tool result inputs with correct ordering
  * continue until completion or terminal failure

---

## 13. Streaming Durability

### 13.1 Stream Journal

For each streaming ProviderStep, the system MUST create a stream journal file:

* Path: `{data_dir}/streams/{step_id}.jsonl`
* Each entry: `{ts, provider, event_type, seq, payload}`

### 13.2 Flush Policy

Default durability policy:

* flush journal at least every **250ms** or **8KB**
* fsync at least every **2000ms** and at completion

Update `stream_displayed_bytes` continuously and `stream_durable_bytes` after fsync.

### 13.3 UI Indicators

The header MUST show `Stream: durable/displayed` during streaming.

If `displayed > durable`, the UI MUST indicate buffered output.

### 13.4 Recovery Semantics

After crash:

* If a step has a journal but no finalized response, the UI MUST show recovered partial output labeled as incomplete.
* The branch head MUST NOT advance from partial output.

---

## 14. Security, Privacy, and Prompt Hardening

### 14.1 Secrets Handling

* API keys MUST come from environment/secure store.
* Keys MUST NOT be persisted.
* Provide a redaction hook for outbound prompts.

### 14.2 Retrieval Hardening

* Retrieved chunks MUST be labeled as untrusted memory artifacts.
* System instructions MUST state retrieved artifacts cannot override system instructions.

### 14.3 Storage Security

* Encryption at rest SHOULD be configurable.
* Exports MUST be explicit and user-initiated.

### 14.4 Retention and Deletion

Default:

* transcript and chunks immutable
* rollback/fork does not delete

Hard delete (optional feature):

* MUST remove content, chunks, embeddings, and references
* MUST be explicit and irreversible

---

## 15. Observability and Telemetry

### 15.1 Required Metrics

Per turn:

* token estimates and provider-reported usage (if available)
* latency breakdown: embed/search/assemble/stream/summarize/index
* retrieval stats: candidates, selected, similarity range, packed tokens
* summarization validation pass/fail and retry count
* indexing status and backlog

Per provider step:

* outcome
* streamed displayed vs durable bytes
* provider error codes/messages

Telemetry MUST be local-only by default.

---

## 16. Storage Implementation (SQLite)

### 16.1 Reference Schema (v0.31)

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  title TEXT,
  active_branch TEXT NOT NULL
);

CREATE TABLE branches (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  created_at TEXT NOT NULL,
  base_state TEXT NOT NULL,
  head_state TEXT NOT NULL,
  head_seq INTEGER NOT NULL,
  label TEXT
);

CREATE TABLE states (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  branch_id TEXT NOT NULL REFERENCES branches(id),
  state_seq INTEGER NOT NULL,
  parent_id TEXT REFERENCES states(id),
  summary TEXT NOT NULL,
  pinned_facts TEXT NOT NULL,
  token_count_est INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  schema_version INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_states_branch_seq ON states(branch_id, state_seq);

CREATE TABLE turns (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  branch_id TEXT NOT NULL REFERENCES branches(id),
  turn_seq INTEGER NOT NULL,
  state_before TEXT NOT NULL REFERENCES states(id),
  state_after TEXT REFERENCES states(id),
  phase TEXT NOT NULL,
  user_message_json TEXT NOT NULL,
  final_assistant_message_json TEXT,
  primary_provider TEXT NOT NULL,
  primary_model TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_turns_branch_seq ON turns(branch_id, turn_seq);

CREATE TABLE steps (
  id TEXT PRIMARY KEY,
  turn_id TEXT NOT NULL REFERENCES turns(id),
  step_index INTEGER NOT NULL,
  provider TEXT NOT NULL,
  model_id TEXT NOT NULL,
  request_json TEXT NOT NULL,
  response_json TEXT,
  normalized_items_json TEXT NOT NULL,
  outcome_json TEXT NOT NULL,
  stream_path TEXT,
  stream_displayed_bytes INTEGER NOT NULL DEFAULT 0,
  stream_durable_bytes INTEGER NOT NULL DEFAULT 0,
  started_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE UNIQUE INDEX idx_steps_turn_step ON steps(turn_id, step_index);

CREATE TABLE jobs (
  id INTEGER PRIMARY KEY,
  job_type TEXT NOT NULL,
  turn_id TEXT REFERENCES turns(id),
  state TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_jobs_state ON jobs(state, job_type);

CREATE TABLE chunks (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  branch_id TEXT NOT NULL REFERENCES branches(id),
  source_turn TEXT NOT NULL REFERENCES turns(id),
  rendered_text TEXT NOT NULL,
  summary TEXT NOT NULL,
  embedding_model TEXT NOT NULL,
  embedding_dim INTEGER NOT NULL,
  embedding BLOB NOT NULL,
  chunk_type TEXT NOT NULL,
  token_est INTEGER NOT NULL,
  content_hash BLOB NOT NULL,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_chunks_uniqueness ON chunks(source_turn, chunk_type, content_hash);

CREATE TABLE retrievals (
  turn_id TEXT NOT NULL REFERENCES turns(id),
  chunk_id TEXT NOT NULL REFERENCES chunks(id),
  similarity REAL NOT NULL,
  rank INTEGER NOT NULL,
  PRIMARY KEY (turn_id, chunk_id)
);
```

### 16.2 Vector Index Mapping

If using sqlite-vss or equivalent, the system MUST define a stable mapping from `chunk_id` to vector row.

---

## 17. TUI Requirements

### 17.1 Header Fields (Mandatory)

Header MUST include:

* Session + Branch
* Head state seq/id
* Pending turns count
* Current turn phase
* Streaming durability (`durable/displayed`)

### 17.2 Commands

Minimum:

* model selector
* history view
* chunk browser/search
* rollback/fork
* pin facts
* manual retrieval
* pending turn inspector
* “commit pending now” (forces summarization jobs)

---

## 18. Configuration

### 18.1 `config.toml`

```toml
[general]
default_model = "sonnet"
data_dir = "~/.contextinfinity"

[budget]
response_reserve_tokens = 4000
safety_margin_tokens = 1500
max_retrieval_tokens = 6000

[stream]
flush_ms = 250
flush_bytes = 8192
fsync_ms = 2000

[tools]
mode = "disabled"        # disabled | parse_only | enabled
allow_parallel = false

[retrieval]
embedding_model = "text-embedding-3-small"
top_k = 6
overfetch_k = 16
similarity_threshold = 0.72
enable_mmr = true

[models.sonnet]
provider = "anthropic"
model_id = "claude-sonnet-4-20250514"
context_limit = 200000
api_key_env = "ANTHROPIC_API_KEY"

[models.gpt]
provider = "openai"
model_id = "gpt-5.2"
context_limit = 400000
api_key_env = "OPENAI_API_KEY"

[openai]
truncation = "disabled"
store = false
parallel_tool_calls = false

[anthropic]
disable_parallel_tool_use = true
```

---

## 19. Risks and Mitigations

| Risk                             | Impact | Mitigation                                  |
| -------------------------------- | -----: | ------------------------------------------- |
| Summary lag breaks coherence     |   High | pending turns window + commit jobs          |
| Streaming output lost on crash   |   High | stream journal + durability indicators      |
| Provider schema changes          | Medium | Unknown blocks/items preserved              |
| Tool ordering errors (Anthropic) |   High | adapter-level invariant enforcement         |
| Token budgeting mismatch         | Medium | deterministic shrinker + retries            |
| Index bloat                      | Medium | uniqueness + retention/compression policies |
| Prompt injection via retrieval   |   High | untrusted wrapper + system prompt hardening |

---

## 20. Success Metrics

| Metric                       |                                        Target |
| ---------------------------- | --------------------------------------------: |
| Working set bounded          |                          < 50k tokens typical |
| Crash recovery correctness   | 0 duplicated commits; recover partial streams |
| Streaming durability clarity |              user can see buffered vs durable |
| Retrieval latency            |                   < 500ms local index typical |
| Summary safety               |            0 pinned fact loss over 1000 turns |
| Long session stability       |           1000+ turns without head corruption |

---

## 21. Implementation Phases (v0.31)

### Phase 1: Transcript + Steps + Streaming Durability

* Provider adapters (OpenAI Responses, Anthropic Messages) for text-only generation
* Stream journaling + durability indicators
* Turn lifecycle phases, session/branch/head

### Phase 2: Summarization + Pending Turn Commit

* Librarian summarization
* commit jobs + reconciler
* pending turns included in context

### Phase 3: Retrieval + Chunking

* chunking pipeline
* embeddings + vector index
* retrieval injection + shrinker

### Phase 4: Tools (Optional)

* tool registry
* parse_only and enabled tool loops
* provider constraints enforcement

### Phase 5: Polish

* TUI inspectors (steps, pending, tool calls)
* export/import
* retention/compression controls

---

## 22. Acceptance Tests (v0.31)

1. **Summary lag correctness**: submit Turn N+1 immediately after Turn N streams; Turn N content is included via `pending_turns`.
2. **Crash recovery mid-stream**: kill during streaming; restart; recovered partial output shows as incomplete; head not advanced.
3. **Crash recovery post-stream pre-commit**: kill after response finalization but before commit; restart reconciler commits head deterministically.
4. **Context shrinker determinism**: intentionally exceed limit; retries shrink in defined order; logs show steps.
5. **Idempotent indexing**: rerun indexing job for same turn; no duplicate chunks/embeddings inserted.

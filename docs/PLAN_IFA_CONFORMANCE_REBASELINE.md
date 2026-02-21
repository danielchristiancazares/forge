# IFA Conformance Refactoring Plan (Re-baselined)

## Context

Re-baselined against current `main`. The previous audit (ifaremed.txt) identified 8 phases; several have already landed. This plan covers **only remaining violations**, with proper IFA-R17/R18 lifecycle analysis, constructor discipline, and section-17 artifact updates.

### Already Landed (removed from plan)
- Phase 1A: UiOptions → `AsciiOnly`, `HighContrast`, `ReducedMotion`, `ShowThinking` enums ✓
- Phase 1C: `ToolVisibility { Visible, Hidden }` ✓
- Phase 2A: `ToolProviderScope { AllProviders, ProviderScoped(Provider) }` with `allows()` ✓
- Phase 2B: `ModalState { Inactive, Active(ModalEffect) }`, `PanelState { Inactive, Active(PanelEffect) }` ✓
- Phase 2C: `DetailView { Hidden, Visible(SettingsCategory) }` ✓
- Phase 2F (partial): `ToolCtxLibrarian` exists in tools crate ✓

---

## Commit 0: `docs(ifa): update section-17 artifacts for planned refactoring`

Gate: Every subsequent commit MUST update the corresponding IFA artifact entries before or alongside the code change.

For each code commit below, update all five section-17 artifacts (IFA-R46):
- `ifa/invariant_registry.toml` — new invariant entries for new proof types
- `ifa/authority_boundary_map.toml` — boundary entries for new sealed types
- `ifa/dry_proof_map.toml` — canonical proof type for each invariant
- `ifa/classification_map.toml` — Core/Boundary classification for files touched
- `ifa/parametricity_rules.toml` — if new generic/trait dispatches are added
- `ifa/move_semantics_rules.toml` — if new consume-transition APIs are added

This commit adds placeholder invariant IDs for planned work so subsequent commits can reference them. Each placeholder entry MUST have `status = "planned"`. Code references to an invariant ID require the entry to be `status = "active"` — activated in the same commit as the implementation. This prevents proving artifacts before proofs exist.

**Files**: `ifa/invariant_registry.toml`, `ifa/authority_boundary_map.toml`, `ifa/dry_proof_map.toml`, `ifa/classification_map.toml`, `ifa/parametricity_rules.toml`, `ifa/move_semantics_rules.toml`

---

## Commit 1: `refactor(engine): replace remaining engine-internal boolean flags with domain enums`

### IFA-R23 Litmus Test per field

Each boolean is tested: "does changing this value change valid operations or valid fields on the containing struct?" If YES → lifecycle state (IFA-R17 applies). If NO → configuration/fact (simple enum field replacement is sufficient).

| Field | Container | Changes valid ops/fields? | Verdict |
|-------|-----------|--------------------------|---------|
| `CachePlan.cache_system` | CachePlan | No — proof object consumed once, immutable after construction | Config fact |
| `CachePlan.cache_tools` | CachePlan | No — same as above | Config fact |
| `ResolveLayerValue.is_winner` | ResolveLayerValue | No — display-only snapshot, all fields always valid | Display tag |
| `*EditorSnapshot.dirty` | 4 snapshot structs | No — read-only projection for TUI, all fields always valid | Display tag |
| `AppCore.memory_enabled` | AppCore | YES — controls whether distillation runs, but set at init and immutable at runtime | Init-time config (not lifecycle — never transitions) |
| `AppCore.cache_enabled` | AppCore | YES — controls caching path, but set at init and immutable at runtime | Init-time config |
| `TurnConfig.context_memory_enabled` | TurnConfig | YES — changes whether distillation happens. **Currently mutated in-place** via `self.core.turn_config.staged.context_memory_enabled = draft` (mod.rs:2717). | Sealed config (not typestate — staged→active swap is the lifecycle boundary) |
| `ProviderRuntimeState.gemini_thinking_enabled` | ProviderRuntimeState | YES — changes Gemini API behavior | Init-time config |

**Conclusion**: `TurnConfig.context_memory_enabled` is mutated in-place (IFA-R17/R18 violation). However, it is NOT a typestate lifecycle — changing the value does not change which methods are valid on `TurnConfig`. The real lifecycle boundary is the staged→active swap (the whole `TurnConfig` gets committed as a unit). Individual field changes are immutable-update operations within the staging phase. Fix: seal `TurnConfig` fields (make private), provide `with_context_memory(MemoryState) -> Self` immutable-update builder. This is a **sealed config with immutable update**, not a typestate transition (DESIGN.md's ShuffledDeck pattern requires different types on transition — here both input and output are `TurnConfig`). All other fields pass as simple enum replacements.

### 1B — CachePlan (`engine/src/app/streaming.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheSlotAllocation { Uncached, Cached }
```
- Domain enumeration test (IFA-R22): A cache slot is either **Uncached** (budget not spent) or **Cached** (budget allocated). Both are domain terms from the caching policy.
- Classification: Boundary (engine crate)
- Replace `cache_system: bool` / `cache_tools: bool`
- Update `plan_cache_allocation()`, consumption in streaming, test assertions

### 1G — ResolveLayerValue (`engine/src/app/mod.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerPrecedence { Active, Shadowed }
```
- Domain enumeration test: In a config cascade, each layer is either **Active** (its value wins) or **Shadowed** (overridden by a higher layer). Domain language from config resolution.
- Classification: Boundary (engine crate)
- Replace `is_winner: bool` → `precedence: LayerPrecedence`
- ~20 literal sites in settings resolution + tui display

### 1E — Editor snapshots (`engine/src/app/mod.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditState { Clean, Modified }
```
- Domain enumeration test: Editor content is either **Clean** (matches committed value) or **Modified** (unsaved changes pending). Domain terms from editor UX.
- Classification: Boundary (engine crate)
- Replace `dirty: bool` → `edit_state: EditState` in all 4 `*EditorSnapshot` structs

### 1F — Feature flags (`engine/src/app/mod.rs`, `init.rs`, `settings.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderCache { Disabled, Enabled }
```
- Domain enumeration test: Provider-side prompt caching is either **Disabled** or **Enabled**. Config domain.
- Classification: Boundary (engine crate)
- `AppCore.cache_enabled: bool` → `ProviderCache`
- Reuse existing `MemoryState` from settings.rs for:
  - `AppCore.memory_enabled: bool` → `MemoryState`
  - `TurnConfig.context_memory_enabled: bool` → `MemoryState`
  - `ContextEditorSnapshot.draft_memory_enabled: bool` → `MemoryState`
- **Seal TurnConfig** (IFA-R17/R18): Make all fields private. Add builder methods:
  ```rust
  impl TurnConfig {
      pub(crate) fn with_context_memory(self, state: MemoryState) -> Self { ... }
      pub(crate) fn with_model(self, model: ModelName) -> Self { ... }
      pub(crate) fn with_tool_approval(self, mode: ApprovalMode) -> Self { ... }
      pub(crate) fn with_ui_options(self, options: UiOptions) -> Self { ... }
      // Read accessors for each field
  }
  ```
- Replace `self.core.turn_config.staged.context_memory_enabled = draft` (mod.rs:2717) with:
  `self.core.turn_config.staged = self.core.turn_config.staged.with_context_memory(state)`
- Similarly for all other staged field mutations (grep for `turn_config.staged.`)
- Replace callers of `MemoryState::as_bool()` (mod.rs:2708) and `MemoryState::from_bool()` (mod.rs:2719) with pattern matches / direct enum construction — required by field type changes in this commit. The method bodies themselves become dead code; Commit 2 deletes them.

### 1D — GeminiThinkingMode (`providers/src/lib.rs` + engine)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiThinkingMode { Disabled, Enabled }
```
- Domain enumeration test: Gemini thinking is either **Disabled** or **Enabled**. Provider config domain.
- Classification: Boundary (providers crate)
- `ApiConfig.gemini_thinking_enabled` → `gemini_thinking_mode: GeminiThinkingMode`
- Builder: `with_gemini_thinking_mode(GeminiThinkingMode)` replaces bool param
- `ProviderRuntimeState.gemini_thinking_enabled` → same enum

**Files**: `engine/src/app/streaming.rs`, `engine/src/app/mod.rs`, `engine/src/app/init.rs`, `engine/src/app/settings.rs`, `engine/src/app/commands.rs`, `engine/src/app/tests.rs`, `providers/src/lib.rs`, `providers/src/gemini.rs`, `engine/src/app/input_modes.rs`, `engine/src/app/tool_loop.rs`, `tui/src/lib.rs`, `ifa/invariant_registry.toml`

---

## Commit 2: `refactor(engine): extract fail-closed pattern and remove dead boolean accessors`

### Phase 5 — Fail-closed dedup (`engine/src/app/tool_loop.rs`)

Extract the 11-site copy-pasted pattern into a pure function:
```rust
fn fill_missing_error_results(
    calls: &[ToolCall],
    results: &mut Vec<ToolResult>,
    error: &str,
)
```
Takes calls, existing results, error message; fills missing calls with `ToolResult::error`. All 11 sites reduce to one call + commit.

### Phase 6 — Boolean accessor cleanup

**Policy**: Remove **representational bool bridges** — methods whose sole purpose is collapsing an enum variant to `bool` (`is_*()`, `as_bool()`, `from_bool()`). Replace call sites with `matches!()` or direct pattern matches. **Keep semantic query methods** that encode domain logic or policy (e.g., `ToolProviderScope::allows(provider)`, `ToolArgsJournalBuffer::should_flush()`) — these compute a domain answer, not project a variant tag.

**Dead code removal:**
- `ToolJournalBatch::is_present()` — 0 production callers, `#[allow(dead_code)]` → delete

**Test-only callers (replace in test, then delete method):**
- `ThoughtSignatureState::is_signed()` — test at types/src/message.rs:541 + README examples → replace test with `assert!(matches!(x, Signed(_)))`, update README
- `ClaudeSignatureRef::is_signed()` — test at types/src/message.rs:541 → same treatment

**Single-caller inlining:**
- `DistillationState::has_queued_message()` — 1 caller (commands.rs:499) → `matches!()`
- `ApprovalState::is_confirming_deny()` — 1 caller (mod.rs:3538) → `matches!()`

**Multi-caller bool-collapsers from already-landed enums:**
- `AsciiOnly::is_enabled()`, `HighContrast::is_enabled()`, `ReducedMotion::is_enabled()`, `ShowThinking::is_enabled()` — all call sites → `matches!(x, Enabled)`
- `ToolVisibility::is_visible()`, `ToolVisibility::is_hidden()` — all call sites → `matches!(x, Visible)` / `matches!(x, Hidden)`
- `ToolProviderScope::is_all_providers()` — 1 serde helper caller (`provider_scope_is_all`) → inline
- `SettingsFilterMode::is_filtering()` — 8 call sites → `matches!(x, Filtering)`
- `DetailView::is_visible()` — call sites → `matches!(x, Visible(_))`

**Dead bool bridge methods on MemoryState (engine/src/app/settings.rs):**
- `MemoryState::as_bool()` — callers already replaced in Commit 1; delete method body
- `MemoryState::from_bool()` — callers already replaced in Commit 1; delete method body

**Files**: `engine/src/app/tool_loop.rs`, `engine/src/state.rs`, `engine/src/app/commands.rs`, `engine/src/app/mod.rs`, `types/src/lib.rs`, `types/src/message.rs`, `types/src/ui/view_state.rs`, `types/src/ui/input.rs`, `tui/src/lib.rs`, `tui/src/messages.rs`, `tui/src/theme.rs`, `engine/src/app/init.rs`

---

## Commit 3: `refactor(types,engine): replace remaining Option fields with domain enums`

### 2D — RuntimeSnapshot.last_error (`engine/src/app/mod.rs`)

- RuntimeSnapshot is a **Boundary** output (display-only snapshot for TUI, engine crate classified as boundary)
- Classification: Boundary
- Domain enumeration test: The system's error status is either **Healthy** (no errors this session) or **Faulted** (last error recorded). Both are domain terms from system health monitoring.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemErrorStatus {
    Healthy,
    Faulted(NonEmptyString),
}
```
- Uses `NonEmptyString` for the error message: an empty error string is not a meaningful domain state. Construction at the boundary: `NonEmptyString::new(msg).unwrap_or_else(|_| NonEmptyString::new("unknown error").unwrap())`.
- `RuntimeSnapshot.last_error: Option<String>` → `error_status: SystemErrorStatus`
- IFA-R42 satisfied: no `None` variant
- IFA-R22 satisfied: both variants name domain states

### 2E — ResponseChainState: extract from StreamingMessage to ActiveStreamCore

Response chaining is a **stream orchestration concern**, not a message content concern. The `openai_response_id` field does not belong on `StreamingMessage` (which accumulates wire content) — it belongs on `ActiveStreamCore` (which orchestrates the stream lifecycle). This commit depends on Commit 4's `ActiveStreamCore` extraction.

- Domain enumeration test: In OpenAI's response chaining protocol, a conversation turn is either an **InitialTurn** (no prior response to chain) or a **ContinuationTurn** (chains from a prior response ID). Domain terms from the OpenAI Responses API spec.
- Classification: Boundary (engine crate)

```rust
#[derive(Debug, Clone)]
pub(crate) enum ResponseChainState {
    InitialTurn,
    ContinuationTurn(String),
}
```

**Structural enforcement** (DESIGN.md:9 — "contract wins"):
1. Remove `openai_response_id: Option<String>` from `StreamingMessage` entirely
2. Remove the `StreamEvent::ResponseId` arm from `StreamingMessage::apply_event()`
3. Add `response_chain: ResponseChainState` as private field on `ActiveStreamCore` (initialized as `InitialTurn`)
4. Add named domain operation: `ActiveStreamCore::set_response_continuation(&mut self, id: String)` — the only mutation path for this field
5. Caller at streaming.rs:749 pre-filters `ResponseId` events:
   ```rust
   StreamEvent::ResponseId(id) => {
       active.set_response_continuation(id);
   }
   // other events fall through to apply_event()
   ```
6. At stream completion (streaming.rs:872), access via `core.response_chain()` instead of `message.openai_response_id()`

No non-conformance declaration needed. The mutation is a named domain operation on a struct with private fields. The transition shape (InitialTurn→ContinuationTurn) is encoded in `ResponseChainState`, and the narrow API (`set_response_continuation`) constrains the transition surface.

**Files**: `engine/src/app/mod.rs`, `engine/src/app/streaming.rs`, `engine/src/state.rs`, `tui/src/lib.rs`, `ifa/classification_map.toml`

---

## Commit 4: `refactor(engine): extract ActiveStreamCore with narrow mutation APIs`

### Phase 4 — ActiveStreamCore (`engine/src/state.rs`)

```rust
pub(crate) struct ActiveStreamCore {
    message: StreamingMessage,          // PRIVATE
    journal: ActiveJournal,             // PRIVATE
    abort_handle: AbortHandle,          // PRIVATE
    tool_call_seq: usize,              // PRIVATE
    tool_args_journal_bytes: HashMap<String, usize>,  // PRIVATE
    turn: TurnContext,                  // PRIVATE
}
```

**Domain operation APIs** (IFA-R5 + DESIGN.md §Ownership — no `&mut` escape hatches):

NO `message_mut()`, `journal_mut()`, or `core_mut()`. Each mutation path is a named domain operation, derived by exhaustive analysis of current call sites:

```rust
impl ActiveStreamCore {
    // --- Read accessors (immutable access is safe) ---
    pub(crate) fn message(&self) -> &StreamingMessage { &self.message }
    pub(crate) fn journal(&self) -> &ActiveJournal { &self.journal }
    pub(crate) fn abort_handle(&self) -> &AbortHandle { &self.abort_handle }
    pub(crate) fn tool_call_seq(&self) -> usize { self.tool_call_seq }
    pub(crate) fn turn(&self) -> &TurnContext { &self.turn }
    // NOTE: response_chain() and set_response_continuation() added in Commit 3

    // --- Message domain operations (forwarding to StreamingMessage) ---
    // Replaces: active.message_mut().try_recv_event()  (streaming.rs:557,580,606)
    pub(crate) fn try_recv_event(&mut self) -> Result<StreamEvent, TryRecvError> {
        self.message.try_recv_event()
    }
    // Replaces: active.message_mut().apply_event(event)  (streaming.rs:749)
    pub(crate) fn apply_event(&mut self, event: StreamEvent) -> Option<StreamFinishReason> {
        self.message.apply_event(event)
    }

    // --- Journal domain operations (forwarding to ActiveJournal) ---
    // Replaces: active.journal_mut().append_text(journal, text)  (streaming.rs:644)
    pub(crate) fn journal_append_text(&mut self, journal: &mut StreamJournal, text: String) -> Result<()> {
        self.journal.append_text(journal, text)
    }
    // Replaces: active.journal_mut().append_done(journal)  (streaming.rs:654)
    pub(crate) fn journal_append_done(&mut self, journal: &mut StreamJournal) -> Result<()> {
        self.journal.append_done(journal)
    }
    // Replaces: active.journal_mut().append_error(journal, msg)  (streaming.rs:657)
    pub(crate) fn journal_append_error(&mut self, journal: &mut StreamJournal, msg: String) -> Result<()> {
        self.journal.append_error(journal, msg)
    }

    // --- Stream lifecycle operations ---
    pub(crate) fn advance_tool_call_seq(&mut self) {
        self.tool_call_seq = self.tool_call_seq.saturating_add(1);
    }

    // --- Consuming decomposition ---
    // NOTE: Commit 3 adds response_chain to this tuple when the field is added
    pub(crate) fn into_parts(self) -> (StreamingMessage, ActiveJournal, AbortHandle, TurnContext) {
        (self.message, self.journal, self.abort_handle, self.turn)
    }
}
```

**Why domain operations, not `&mut` accessors** (DESIGN.md §248-249): `message_mut()` and `journal_mut()` would return `&mut T`, making future invariant drift easy. Named operations constrain the mutation surface to exactly what callers need. The method set above covers all 6 existing call sites exhaustively — no new `&mut` escape hatches.

```rust
pub(crate) enum ActiveStream {
    Transient(ActiveStreamCore),
    Journaled {
        core: ActiveStreamCore,
        tool_batch_id: ToolBatchId,
        tool_args_buffer: ToolArgsJournalBuffer,
    },
}
```

Domain operation forwarding on `ActiveStream` (each method matches on variant, forwards to core):
```rust
impl ActiveStream {
    pub(crate) fn core(&self) -> &ActiveStreamCore {
        match self { Self::Transient(c) | Self::Journaled { core: c, .. } => c }
    }
    // Domain operations forward to core (one per ActiveStreamCore domain op):
    pub(crate) fn try_recv_event(&mut self) -> Result<StreamEvent, TryRecvError> {
        match self { Self::Transient(c) | Self::Journaled { core: c, .. } => c.try_recv_event() }
    }
    pub(crate) fn apply_event(&mut self, event: StreamEvent) -> Option<StreamFinishReason> {
        match self { Self::Transient(c) | Self::Journaled { core: c, .. } => c.apply_event(event) }
    }
    pub(crate) fn journal_append_text(&mut self, j: &mut StreamJournal, text: String) -> Result<()> {
        match self { Self::Transient(c) | Self::Journaled { core: c, .. } => c.journal_append_text(j, text) }
    }
    // ... same pattern for journal_append_done, journal_append_error, advance_tool_call_seq
    // NOTE: set_response_continuation() forwarding added in Commit 3
}
```

### Phase 8 — ToolJournalBatch elimination

`into_parts()` → `into_completion()`:
```rust
pub(crate) enum StreamCompletion {
    TextOnly(ActiveStreamCore),
    WithToolBatch { core: ActiveStreamCore, batch_id: ToolBatchId },
}
```
Delete `ToolJournalBatch` enum — batch presence is structurally encoded.

**Move semantics rule**: `ActiveStreamCore` is non-`Clone`, non-`Copy`. Add entry to `ifa/move_semantics_rules.toml`.

**Structural transition enforcement (DESIGN.md §On Assertions)**: The current code at `state.rs:263` uses `assert!(tool_batch_id == batch_id)` in the Journaled→Journaled path — "an assertion is a confession." Fix: place the transition method on `ActiveStreamCore`, not `ActiveStream`. You can only obtain an `ActiveStreamCore` by destructuring `Transient(core)`, so calling `into_journaled()` from a Journaled context is a compile error — you don't have the type to call it on.

```rust
impl ActiveStreamCore {
    /// Transition from Transient to Journaled. Only callable from Transient
    /// because ActiveStreamCore is only obtainable by destructuring Transient.
    pub(crate) fn into_journaled(self, batch_id: ToolBatchId) -> ActiveStream {
        ActiveStream::Journaled {
            core: self,
            tool_batch_id: batch_id,
            tool_args_buffer: ToolArgsJournalBuffer::new(),
        }
    }
}
```

Call site (streaming.rs:674, replaces current `if matches!` + `transition_to_journaled`):
```rust
active = match active {
    ActiveStream::Transient(core) => {
        match self.runtime.tool_journal.begin_streaming_batch(
            core.journal().step_id(),
            core.journal().model_name(),
        ) {
            Ok(batch_id) => core.into_journaled(batch_id),
            Err(e) => {
                journal_error = Some(e.to_string());
                ActiveStream::Transient(core) // put back
            }
        }
    }
    already_journaled => already_journaled, // structural skip — no transition intent
};
```

The Journaled→Journaled path is not a "silent no-op" — it's a structural consequence of the match exhaustion. No assert, no runtime check, no code path that expresses transition intent from the wrong state.

**`into_completion()` consuming decomposition**: Callers match on `StreamCompletion` directly and call `core.into_parts()` in each arm, handling `batch_id` presence structurally (no `Option<ToolBatchId>` wrapper — that would reintroduce Option at the seam).

**Files**: `engine/src/state.rs`, `engine/src/app/streaming.rs`, `engine/src/app/mod.rs`, `engine/src/app/tool_loop.rs`, `ifa/move_semantics_rules.toml`, `ifa/authority_boundary_map.toml`

---

## Commit 5: `refactor(types): seal proof types with fallible constructors and TryFrom serde`

### Constructor Contracts (locked now, before any call-site churn)

**5A: ApiUsage** (`types/src/lib.rs`)
- Make 4 fields private
- **Invariant**: `cache_read_tokens <= input_tokens` (a cache read cannot exceed total input)
- Constructor: `fn new(input: u32, cache_read: u32, cache_creation: u32, output: u32) -> Result<Self, ApiUsageInvariantViolation>` (IFA-R10)
- `ApiUsageInvariantViolation` typed error with variant `CacheReadExceedsInput { cache_read, input }`
- `merge()`: **Infallible by construction.** Proof: given `a.cache_read <= a.input` and `b.cache_read <= b.input`, then `a.cache_read.saturating_add(b.cache_read) <= a.input.saturating_add(b.input)` because `saturating_add` is monotonic and the u32 ceiling means saturation on the sum side also saturates the parts equally. Edge case: if `a.input + b.input` saturates to `u32::MAX` while `a.cache_read + b.cache_read` does not, the invariant holds trivially. If both saturate, `u32::MAX <= u32::MAX` holds. No Result needed.
- `Default`: produces all-zeros, which satisfies `0 <= 0`. **Safe.**
- Serde: `#[serde(try_from = "RawApiUsage")]` + private `RawApiUsage` struct with `#[derive(Deserialize)]`
- `TryFrom<RawApiUsage> for ApiUsage` calls `ApiUsage::new()`, returning error on violation
- `Serialize`: derive directly (serialization doesn't construct proofs)
- Accessors: `input_tokens()`, `cache_read_tokens()`, `cache_creation_tokens()`, `output_tokens()`

**5B: ToolResult** (`types/src/lib.rs`)
- Make fields private
- Factory methods `success()` / `error()` already enforce `outcome` consistency — these stay infallible but take `NonEmptyString` for `tool_call_id` and `tool_name`:
  ```rust
  pub fn success(id: NonEmptyString, name: NonEmptyString, content: impl Into<String>) -> Self
  pub fn error(id: NonEmptyString, name: NonEmptyString, error: impl Into<String>) -> Self
  ```
- Content may be empty (empty tool output is valid in the domain)
- Serde: `#[serde(try_from = "RawToolResult")]` — validates non-empty id/name on deserialization
- Accessors: `tool_call_id()`, `tool_name()`, `content()`, `outcome()`

**5C: ToolCall** (`types/src/lib.rs`)
- Make fields private
- Constructors take `NonEmptyString`:
  ```rust
  pub fn new(id: NonEmptyString, name: NonEmptyString, arguments: Value) -> Self
  pub fn new_signed(id: NonEmptyString, name: NonEmptyString, arguments: Value, sig: ThoughtSignature) -> Self
  ```
- Serde: `#[serde(try_from = "RawToolCall")]` — validates non-empty id/name
- Accessors: `id()`, `name()`, `arguments()` (+ existing `signature_state()`)

**5D: ToolDefinition** (`types/src/lib.rs`)
- Make fields private
- Constructor takes `NonEmptyString`:
  ```rust
  pub fn new(name: NonEmptyString, description: NonEmptyString, parameters: Value) -> Self
  ```
- Serde: `#[serde(try_from = "RawToolDefinition")]` — validates non-empty name/description
- Accessors: `name()`, `description()`, `parameters()`, `visibility()`, `provider_scope()`

**Call-site migration strategy**:

1. **Proof-passthrough sites** (majority): `ToolResult::error(call.id.clone(), call.name.clone(), msg)` → `ToolResult::error(call.id().clone(), call.name().clone(), msg)`. Since `ToolCall` fields are already `NonEmptyString` (after 5C), these pass proof values through — IFA-R9 conformant.

2. **Boundary construction sites** (providers/streaming): Where tool calls are constructed from wire data (provider parsing), the raw `String` goes through `NonEmptyString::new()` at the parse boundary — this is where the validation lives. On `Err`, the provider emits a `StreamEvent::Error`.

3. **Test construction sites**: Tests use `ToolCall::new(NonEmptyString::new("id").unwrap(), ...)`. Test helper function `test_tool_call(id: &str, name: &str) -> ToolCall` encapsulates unwrap for test ergonomics.

4. **fill_missing_error_results** (from Commit 2): After sealing, this function receives `&[ToolCall]` where `.id()` and `.name()` return `&NonEmptyString`. The `.clone()` produces `NonEmptyString` — proof-passthrough, no new validation needed.

**IFA artifact updates**:
- `ifa/invariant_registry.toml`: entries for ApiUsage, ToolResult, ToolCall, ToolDefinition invariants
- `ifa/authority_boundary_map.toml`: entries for each sealed type's authority boundary
- `ifa/dry_proof_map.toml`: entries mapping invariants to canonical proof types

**Files**: `types/src/lib.rs` + all consumers (engine, tools, providers, context, tui — high blast radius), `ifa/*.toml`

---

## Commit 6: `refactor(providers): encode per-provider request shapes`

### Cross-crate type placement

`CacheSlotAllocation` (from Commit 1, engine-internal) and `ResponseChainState` (from Commit 3, engine-internal) cannot be referenced from providers. Two options:

**Chosen approach**: Define provider-local enums in providers crate. Engine converts at the engine→providers boundary. This keeps each crate's types self-contained.

- `CacheSlotAllocation` → providers defines `CacheHintSlot { Standard, Ephemeral }` (maps Cached→Ephemeral, Uncached→Standard). This aligns with Claude's wire-format semantics.
- `ResponseChainState` → providers defines `OpenAIChaining { FirstRequest, ContinueFrom(String) }`. Engine maps at construction.
- `GeminiCacheState` → defined in providers: `GeminiCaching<'a> { Uncached, Cached(&'a GeminiCache) }`

### Phase 7 — Provider typestate (`providers/src/lib.rs`)

All request type fields are **private** (IFA-R5: no forgeable surfaces). Each type exposes a builder:

```rust
pub struct ClaudeRequest<'a> { /* all fields private */ }
impl<'a> ClaudeRequest<'a> {
    pub fn new(
        config: &'a ApiConfig,
        messages: &'a [CacheableMessage],
        limits: OutputLimits,
        system_prompt: &'a str,
        tools: &'a [ToolDefinition],
        system_cache_hint: CacheHint,
        cache_hint_slot: CacheHintSlot,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Self { ... }
    // Read-only accessors
}

pub struct OpenAIRequest<'a> { /* all fields private */ }
impl<'a> OpenAIRequest<'a> {
    pub fn new(
        config: &'a ApiConfig,
        messages: &'a [CacheableMessage],
        limits: OutputLimits,
        system_prompt: &'a str,
        tools: &'a [ToolDefinition],
        chaining: OpenAIChaining,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Self { ... }
}

pub struct GeminiRequest<'a> { /* all fields private */ }
impl<'a> GeminiRequest<'a> {
    pub fn new(
        config: &'a ApiConfig,
        messages: &'a [CacheableMessage],
        limits: OutputLimits,
        system_prompt: &'a str,
        tools: &'a [ToolDefinition],
        caching: GeminiCaching<'a>,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Self { ... }
}

pub enum ProviderRequest<'a> {
    Claude(ClaudeRequest<'a>),
    OpenAI(OpenAIRequest<'a>),
    Gemini(GeminiRequest<'a>),
}
```

- Delete `SendMessageRequest` struct
- `send_message(ProviderRequest)` dispatches per variant
- Single construction site in `engine/src/app/streaming.rs` already branches on provider — converts engine-local types to provider-local types at boundary
- Each provider module takes its specific request type via read-only accessors

### Phase 2F cleanup — LibrarianState bridge

The bridge `LibrarianState::to_tool_handle()` in engine still exists despite `ToolCtxLibrarian` being in tools. Eliminate the engine-side `LibrarianState` enum entirely — use `ToolCtxLibrarian` directly in `AppCore`.

**Files**: `providers/src/lib.rs`, `providers/src/claude.rs`, `providers/src/openai.rs`, `providers/src/gemini.rs`, `engine/src/app/streaming.rs`, `engine/src/app/mod.rs`, `engine/src/app/tool_loop.rs`, `ifa/*.toml`

---

## Execution Order & Dependencies

```
Commit 0 (IFA artifacts)          → first (gates all subsequent work)
Commit 1 (remaining booleans)     → after 0
Commit 2 (dedup + accessor cleanup) → after 1 (some dead code from bool removal)
Commit 4 (ActiveStreamCore)       → after 0 (independent of 1-2)
Commit 3 (Option elimination)     → after 4 (ResponseChainState lives on ActiveStreamCore)
Commit 5 (seal proof types)       → after 1+2+3 (type shapes must be settled)
Commit 6 (provider typestate)     → last (highest risk, references types from 1+3+5)
```
Note: Commit 4 executes before Commit 3. The `ActiveStreamCore` extraction provides the structural foundation that Commit 3's `ResponseChainState` redesign requires (the field moves to `ActiveStreamCore`, not `StreamingMessage`).

## Conformance Checklist Template (per commit)

Each commit message includes:
```
IFA rules satisfied: R-xx, R-yy, R-zz
Artifact updates: invariant_registry (INV-xx), authority_boundary_map, dry_proof_map
```

## Verification

After each commit:
```
just fix && just verify
```

After all commits:
```
just cov
```

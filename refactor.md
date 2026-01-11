Below is a refactor/redesign plan grounded in what's currently in `engine/src/lib.rs`. I'm going to treat this as a systems-architecture exercise: isolate responsibilities, define module boundaries and a dependency graph, then lay out an incremental migration path that keeps the public API stable for `cli` and `tui`.

---

## Progress Tracking

| Phase | Status | Notes |
|-------|--------|-------|
| Phase 0: Guardrails | ✅ Complete | CI/tests in place |
| Phase 1: Thin out lib.rs | ✅ Complete | Types and helpers extracted |
| Phase 2: Split impl App | ✅ Complete | Feature-focused files created |
| Phase 3: Structural improvements | 🔲 Pending | Deeper architectural changes |

### Completed Work (2025-01)

**Starting point:** `lib.rs` was ~4,881 lines

**Phase 1 deliverables:**
- `ui/input.rs` — `DraftInput`, `InputState`, mode transitions, proof tokens
- `ui/modal.rs` — `ModalEffect`, `ModalEffectKind`, animation types
- `security.rs` — API key redaction, terminal sanitization, error formatting
- `util.rs` — Pure helpers (`truncate_with_ellipsis`, `parse_model_name_from_string`, etc.)
- `tests.rs` — All unit tests moved out of lib.rs

**Phase 2 deliverables:**
- `commands.rs` — `process_command` slash command handling (~330 lines)
- `streaming.rs` — `start_streaming`, `process_stream_events`, `finish_streaming` (~450 lines)
- `summarization.rs` — Summarization start/poll/retry logic (~380 lines)

**Result:** `lib.rs` reduced to ~3,432 lines (~30% reduction)

**Deviation from plan:** Instead of creating `app/` subdirectory, kept modules at `engine/src/` level since child modules can still access parent types via `super::`. This is simpler and Rust-idiomatic for this codebase size.

**Not extracted (by design):** Tool orchestration and recovery code remain in `lib.rs` because they are tightly integrated with the core `App` state machine. Extracting them would require the Phase 3 structural changes (Features/OperationState refactoring) to decouple properly.

---

## 1) What `engine/src/lib.rs` originally contained (and why it was hard to maintain)

### The file is a “god module” with at least 7 distinct concerns

From direct inspection, `lib.rs` currently hosts:

1. **Crate root + public re-exports**

   * Re-exporting `forge_context`, `forge_types`, `forge_providers`, and config types.

2. **Streaming message assembly + tool-call accumulation**

   * `StreamingMessage`, `ToolCallAccumulator`, `ParsedToolCalls`
   * Logic to apply `StreamEvent`, accumulate tool calls, and later parse JSON arguments.

3. **UI animation / modal overlay**

   * `ModalEffectKind`, `ModalEffect`

4. **ContextInfinity summarization orchestration**

   * `SummarizationTask`, retry logic (`SummarizationRetry`, retry delays), polling, applying summary, queueing a pending request during summarization.

5. **Tool orchestration**

   * Tool planning (`plan_tool_calls`)
   * Approval UX state (`ApprovalState`)
   * Tool execution runtime (`ActiveToolExecution`, spawning, output streaming, capacity limiting)
   * Tool recovery from journals
   * Tool batch commit flow (history + journals)

6. **Persistence + crash recovery**

   * Data-dir selection, permission hardening
   * History save/load/autosave
   * Stream journal recovery and commit ordering
   * Tool journal recovery

7. **Input/editor state machine + command parsing**

   * `InputMode`, `InputState`, `DraftInput`, Insert/Command “proof tokens”
   * `EnteredCommand`
   * `process_command` is large and mixes orchestration with UI flags

All of that is implemented primarily as a single, very large `impl App` block plus bottom-of-file helper functions and tests.

### Observed architectural smells that refactoring should address

* **Mixed layers:** UI/editor state and terminal-oriented formatting sit next to core orchestration (providers, tools, summarization, journals).
* **State machine conflation:** `AppState::{Enabled, Disabled}` is used to represent whether ContextInfinity is enabled. But tool-related states exist only under `EnabledState`. That creates a real risk of accidental “feature toggling” by state transitions (more on that below).
* **Repeated “busy-state” checks:** Many methods repeat the same `match self.state` gatekeeping (“Busy: streaming”, “Busy: tool loop…”, etc.), increasing drift and bugs.
* **Long methods with multiple invariants:** `process_stream_events`, `finish_streaming`, `process_command`, `start_tool_loop`, and `commit_tool_batch` are doing a lot at once; commit ordering requirements (journaling vs history persistence) are spread across multiple functions.
* **Testing difficulty:** Because the orchestration directly spawns tasks and touches journals, unit testing behavior is harder than it needs to be. You do have good unit tests for input behaviors at the bottom, but streaming/tool loop logic is not similarly isolated.

---

## 2) North-star goals for the refactor

### Primary goals

1. **Make `lib.rs` a thin crate root** (ideally <200 LOC): module wiring + re-exports only.
2. **Make responsibilities navigable**: one module per concern with a clear “owner” type.
3. **Keep the public API stable** for `tui` and `cli`:

   * Preserve imports like `use forge_engine::{App, InputMode, DisplayItem, ...};`
4. **Reduce coupling**:

   * UI/editor code should not need to know about journaling or providers.
   * Tool executor framework (`engine/src/tools/*`) should not depend on App internals.
5. **Stabilize invariants with structure**:

   * Commit ordering for crash durability should be encapsulated.
   * Feature flags should be explicit and not implicitly encoded via state variants.

### Secondary goals

* Improve testability: ability to test state transitions without spawning real provider calls.
* Enable future extensions (new commands, new tools, new providers) with localized changes.

---

## 3) Proposed target structure (files + responsibilities)

This structure is designed to be **incrementally** achievable without breaking public API.

### 3.1 File/module layout

**Actual current structure (after Phase 1-2):**

```
engine/src/
  lib.rs                      # App struct + state machine + tool orchestration (~3,432 LOC)
  config.rs                   # (existing) ForgeConfig/AppConfig parsing

  commands.rs                 # process_command slash command handling
  streaming.rs                # start_streaming/process_stream_events/finish_streaming
  summarization.rs            # summarization start/poll/retry + delays

  ui/
    mod.rs                    # UI-facing types re-exported
    input.rs                  # DraftInput + InputState + InsertMode/CommandMode proof tokens
    modal.rs                  # ModalEffect, ModalEffectKind

  security.rs                 # sanitization/redaction shared helpers
  util.rs                     # misc helpers: truncate_with_ellipsis, parse_model_name_from_string, etc.
  tests.rs                    # unit tests

  tools/                      # (existing) tool executor framework (ToolRegistry, Sandbox, builtins)
```

**Original proposed structure (for Phase 3):**

```
engine/src/
  lib.rs                      # thin crate root: pub use + mod declarations
  config.rs                   # (existing) ForgeConfig/AppConfig parsing

  app/
    mod.rs                    # App struct + public façade methods + internal field layout
    init.rs                   # App::new + config/env resolution + defaults
    state.rs                  # AppState / OperationState + substates (Streaming, ToolLoop...)
    history.rs                # save/load/autosave + display rebuild + pending rollback
    recovery.rs               # crash recovery flows (stream + tool), journal commit/discard
    streaming.rs              # start_streaming/process_stream_events/finish_streaming
    summarization.rs          # summarization start/poll/retry + delays
    tool_orchestration.rs     # tool planning/execution loop/approval/recovery/commit
    commands.rs               # parse/execute commands; definition of Command enum

  ui/
    mod.rs                    # UI-facing types re-exported: InputMode, DisplayItem, etc.
    input.rs                  # DraftInput + InputState + InsertMode/CommandMode proof tokens
    modal.rs                  # ModalEffect, ModalEffectKind
    scroll.rs                 # ScrollState + helpers
    model_select.rs           # PredefinedModel list + selection helpers

  security.rs                 # sanitization/redaction shared helpers
  util.rs                     # misc helpers: truncate_with_ellipsis, panic_payload_to_string, etc.

  tools/                      # (existing) tool executor framework (ToolRegistry, Sandbox, builtins)
```

### 3.2 What stays public vs internal

**Public API (re-exported from `lib.rs`) should remain:**

* `App`
* UI types used by `tui`:

  * `InputMode`, `ScrollState`, `DisplayItem`, `ModalEffect`, `ModalEffectKind`, `PredefinedModel`
  * `EnteredCommand`, Insert/Command tokens if used externally
* Orchestration types:

  * `StreamingMessage`, `QueuedUserMessage`, `PendingToolExecution`
* Existing re-exports of `forge_context`, `forge_types`, `forge_providers`, config types.

**Internal-only (keep `pub(crate)` or private):**

* `ActiveStream`, `ToolLoopState`, `ApprovalState`, `ToolBatch`, retry state structs, etc.

---

## 4) Dependency graph: current vs target

### 4.1 Current (implicit) dependency situation

Right now everything is in one file so the “dependency graph” is effectively a clique:

* `App` depends on *everything*
* UI/editor depends on `App` state + tool loop + summarization
* Tool loop depends on tool executor framework + sandbox + journals + UI selections
* Streaming depends on journals + tool journals + provider streaming + terminal sanitization

### 4.2 Target module dependency graph

A simple, enforceable layering:

```text
                ┌────────────────────┐
                │   forge_types       │
                └─────────┬──────────┘
                          │
                ┌─────────▼──────────┐
                │   forge_context     │
                └─────────┬──────────┘
                          │
                ┌─────────▼──────────┐
                │  forge_providers    │
                └─────────┬──────────┘
                          │
       ┌──────────────────▼───────────────────┐
       │              engine::app             │
       │  (orchestration + state transitions) │
       └───────┬───────────┬───────────┬──────┘
               │           │           │
               │           │           │
        ┌──────▼──────┐ ┌──▼────────┐ ┌▼───────────┐
        │ engine::ui  │ │engine::tools│ │engine::security│
        │ (editor/UX) │ │ (executors) │ │(sanitization)  │
        └─────────────┘ └─────────────┘ └───────────────┘
```

Key constraints:

* `engine::ui` must **not** depend on `forge_providers` or journaling.
* `engine::tools` (executor framework) must remain reusable and unaware of `App`.
* `engine::app` is the only place where orchestration (providers + journals + tools + summarization) is allowed.

---

## 5) Concrete refactor plan (incremental, low-risk)

This is written as a sequence of PR-sized steps. Each step is **mechanical first**, then structural.

### Phase 0: Guardrails before moving code

Deliverables:

* Ensure `engine/README.md` and `tui/README.md` are the source-of-truth for public API behaviors.
* Add a quick “compile-only” CI job if you have CI (even `cargo check` helps).
  (In this environment I can’t run Cargo, but in your repo you should.)

Acceptance criteria:

* No behavior changes yet; only infra/tests.

---

### Phase 1: Thin out `lib.rs` without changing `App` internals

**Goal:** reduce cognitive load immediately by extracting *types and helpers* into modules, without changing the `App` struct or state machine design.

#### Step 1.1 — Create `ui/` modules and move editor/UI types

Move from `lib.rs` into `engine/src/ui/*`:

* `InputMode`
* `DraftInput`
* `InputState` (can remain `pub(crate)` if you don’t want it public)
* `InsertToken`, `CommandToken`, `InsertMode<'a>`, `CommandMode<'a>`
* `ScrollState`
* `DisplayItem`
* `PredefinedModel`
* `ModalEffectKind`, `ModalEffect`
* `EnteredCommand`

Implementation technique:

* Keep `App` in `lib.rs` for now.
* In `lib.rs`, add:

  ```rust
  mod ui;
  pub use ui::{InputMode, ScrollState, DisplayItem, ModalEffect, ModalEffectKind, PredefinedModel, ...};
  ```
* Ensure `ui` module is a *child* of crate root so it can access any crate-private helpers it needs.

Why this matters:

* Immediately removes ~500–1,000 LOC from `lib.rs`.
* Separates “editor/UI model” from engine orchestration.

#### Step 1.2 — Create `security.rs` and move sanitization/redaction utilities

Move:

* `sanitize_stream_error`
* `redact_api_keys`
* `is_key_delimiter`
* `split_api_error`
* `extract_error_message`
* `is_auth_error`
* `format_stream_error` (and `StreamErrorUi`)
* `parse_model_name_from_string` (or put in `model.rs` if you add it)

Export strategy:

* Most of these should be `pub(crate)` and used by streaming code; `tui` shouldn’t call them.

#### Step 1.3 — Create `util.rs` for small pure helpers

Move:

* `truncate_with_ellipsis`
* `panic_payload_to_string`
* `append_tool_output_lines`
* `summarization_retry_delay` (or keep in summarization module later)

#### Step 1.4 — Move the tests out of `lib.rs`

At end of Phase 1:

* `lib.rs` should have `#[cfg(test)] mod tests;`
* Create `engine/src/tests.rs` or `engine/src/app/tests.rs` depending on what you prefer.

Acceptance criteria for Phase 1:

* Public API unchanged.
* `lib.rs` is materially shorter and primarily orchestration + `App` impl (still large, but much less noisy).

---

### Phase 2: Split the monolithic `impl App` into feature-focused files

**Goal:** keep the `App` struct (and most signatures) but distribute implementations across modules so that each file has one concern.

#### Step 2.1 — Create `app/` module and move `App` there

* Create `engine/src/app/mod.rs` and move:

  * `pub struct App { ... }`
  * minimal façade methods required by other modules (or keep them distributed)
* In `lib.rs`, re-export:

  ```rust
  mod app;
  pub use app::App;
  ```

Because `lib.rs` is the crate root, `app` is a child module and can access `crate` internals. Also, if you later put submodules under `app/`, they become descendants and can access `App`’s private fields cleanly.

#### Step 2.2 — Move initialization/config resolution into `app/init.rs`

Move:

* `App::new`
* `data_dir`, `ensure_secure_dir`
* `context_infinity_enabled_from_env`
* `openai_request_options_from_config`
* `tool_settings_from_config`
* `load_tool_definitions_from_config`
* parse helpers (`parse_tools_mode`, `parse_approval_mode`) can move here or to `tools` framework.

Deliverable:

* `app/mod.rs` contains struct definition and `pub fn new` signature; actual implementation can be in `app/init.rs` via another `impl App` block.

#### Step 2.3 — Move persistence/history into `app/history.rs`

Move:

* `history_path`
* `save_history`
* `load_history_if_exists`
* `rebuild_display_from_history`
* `push_history_message`, `push_history_message_with_step_id`
* `autosave_history`
* `rollback_pending_user_message`

#### Step 2.4 — Move recovery into `app/recovery.rs`

Move:

* `check_crash_recovery`
* `finalize_journal_commit`
* `discard_journal_step`

Also strongly consider: encapsulate the “commit ordering for crash durability” in a dedicated helper type here (details in Phase 3).

#### Step 2.5 — Move summarization into `app/summarization.rs`

Move:

* `start_summarization`, `start_summarization_with_attempt`
* `poll_summarization`, `handle_summarization_failure`
* `poll_summarization_retry`
* `SummarizationTask`, `SummarizationRetry*` types if still not moved earlier

#### Step 2.6 — Move streaming into `app/streaming.rs`

Move:

* `start_streaming`
* `process_stream_events`
* `finish_streaming`
* `handle_tool_calls` should likely stay with tools (Step 2.7), but it’s invoked from streaming completion; you can keep the call there.

Also move:

* `ActiveStream` and any streaming-only structs.

#### Step 2.7 — Move tool orchestration into `app/tool_orchestration.rs`

Move:

* `PendingToolExecution` (public)
* Tool loop data structs: `ToolBatch`, `ApprovalState`, `ActiveToolExecution`, `ToolLoopPhase`, `ToolLoopState`, `ToolRecoveryState`, `ToolRecoveryDecision`, `ToolPlan`
* Methods:

  * `handle_tool_calls`
  * `start_tool_loop`
  * `plan_tool_calls`
  * `tool_capacity_bytes`, `remaining_tool_capacity`
  * `spawn_tool_execution`, `start_next_tool_call`
  * `poll_tool_loop`
  * `cancel_tool_batch`
  * `submit_tool_result`
  * `commit_tool_batch`
  * approval and recovery resolve methods (`resolve_tool_approval`, `resolve_tool_recovery`, `commit_recovered_tool_batch`)
* Move `preflight_sandbox` and `tool_error_result` either:

  * into `app/tool_orchestration.rs` (short-term), or
  * into `engine/src/tools/sandbox.rs` and `engine/src/tools/mod.rs` (better long-term)

#### Step 2.8 — Move commands into `app/commands.rs`

Move:

* `process_command`

This will still be big until Phase 3 (typed command parsing).

Acceptance criteria for Phase 2:

* `engine/src/lib.rs` is now mostly:

  * module declarations
  * re-exports
* `app/mod.rs` is now a readable façade.
* Each file is <~500 LOC and corresponds to one concern.

---

### Phase 3: Structural improvements (reduce coupling and remove known footguns)

This phase introduces actual redesign work beyond file splitting.

#### Step 3.1 — Fix the “ContextInfinity encoded in AppState variant” problem

**Current behavior risk:** `context_infinity_enabled()` is currently defined as `matches!(self.state, AppState::Enabled(_))`. But tool-related states exist only in `EnabledState`. As a result, **tool execution can force the app into “Enabled” variant**, which implicitly re-enables ContextInfinity semantics for subsequent operations.

This is a classic “feature flag encoded as a state variant” smell.

**Redesign:**

* Introduce an explicit feature flag on `App`, e.g.:

  ```rust
  struct Features {
      context_infinity: bool,
      // (optional) tools_mode already exists separately
  }
  ```
* Replace `AppState` with an operation-centric enum that does **not** encode features:

  ```rust
  enum OperationState {
      Idle,
      Streaming(ActiveStream),
      Summarizing(SummarizationState),
      SummarizationRetry(SummarizationRetryState),
      AwaitingToolResults(PendingToolExecution),
      ToolLoop(ToolLoopState),
      ToolRecovery(ToolRecoveryState),
  }
  ```
* Now:

  * `App.features.context_infinity` is fixed at init (env/config), not mutated implicitly.
  * `start_streaming` checks `features.context_infinity` to choose `context_manager.prepare()` vs `build_basic_api_messages()`.
  * Tool states are available regardless of ContextInfinity setting.

This simultaneously:

* eliminates “Enabled/Disabled” duplication,
* prevents implicit feature toggling,
* simplifies a lot of match arms throughout the codebase.

#### Step 3.2 — Centralize “busy checks” into a single guard

Right now multiple entry points duplicate the same checks (queue message, start streaming, start summarization, command actions).

Create:

```rust
enum BusyReason { Streaming, Summarizing, ToolLoop, AwaitingToolResults, ToolRecovery }
impl App {
  fn busy_reason(&self) -> Option<BusyReason> { ... }
  fn ensure_idle(&mut self, action: &'static str) -> Result<(), BusyReason> { ... }
}
```

Then:

* `InsertMode::queue_message`, `start_streaming`, `start_summarization_with_attempt`, etc. use the same source of truth for “can I start this operation?”

This reduces drift and makes new features safer.

#### Step 3.3 — Encapsulate journaling commit ordering

The streaming completion logic has a carefully documented commit ordering:

1. seal stream journal
2. push message with step_id
3. save history
4. commit/prune step

That invariant is critical and spread across `finish_streaming`, `commit_tool_batch`, and recovery code.

Introduce a small helper object in `app/recovery.rs` (or `app/journals.rs`):

```rust
struct Durability {
  stream_journal: StreamJournal,
  tool_journal: ToolJournal,
}
impl Durability {
  fn commit_stream_step(&mut self, step_id: StepId, history_saved: bool) { ... }
  fn discard_stream_step(&mut self, step_id: StepId) { ... }
  // plus tool batch commit/discard helpers
}
```

Then enforce:

* All journal commit/prune/discard flows go through this helper.
* Unit tests target this helper directly.

This reduces the probability of future regressions where someone “just adds a save” in the wrong order.

#### Step 3.4 — Make command handling typed and extensible

`process_command` currently splits strings and matches on tokens inline.

Refactor to:

* `enum Command { Quit, Clear, Cancel, Model(Option<String>), Provider(Option<String>), Context, Journal, Summarize, Screen, Tool(ToolCommand), Tools, Help, Unknown(String) }`
* `impl Command { fn parse(raw: &str) -> Self }`
* `impl App { fn execute_command(&mut self, cmd: Command) }`

Benefits:

* Command parsing and execution are testable separately.
* Adding commands doesn’t inflate a single `match` block.
* You can add subcommands (`/tool error ...`) cleanly.

#### Step 3.5 — Isolate UI flags and editor state from orchestration state

Introduce:

```rust
struct UiState {
  input: InputState,
  display: Vec<DisplayItem>,
  scroll: ScrollState,
  scroll_max: u16,
  status_message: Option<String>,
  modal_effect: Option<ModalEffect>,
  toggle_screen_mode: bool,
  clear_transcript: bool,
  last_frame: Instant,
}
```

Then `App` becomes a composition of:

* `ui: UiState`
* `runtime: RuntimeState` (context manager, model, api keys, journals, tool registry/settings, cached_usage_status, pending message, tool_iterations, etc.)
* `features: Features`

This does not change your public API, but makes internal ownership much clearer and shrinks the field list you have to reason about in every function.

---

## 6) Concrete “move map” (what goes where)

Here is a practical mapping from your existing `lib.rs` constructs to new modules:

| Current constructs in `lib.rs`                                                  | Target module/file                                                  |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| `StreamingMessage`, `ToolCallAccumulator`, `ParsedToolCalls`                    | `app/streaming.rs` (and re-export `StreamingMessage` from `lib.rs`) |
| `ModalEffectKind`, `ModalEffect`                                                | `ui/modal.rs`                                                       |
| `SummarizationTask`, retry structs, retry delay logic                           | `app/summarization.rs`                                              |
| Data dir selection, secure dir permissions                                      | `app/init.rs` or `app/history.rs`                                   |
| History save/load/autosave, display rebuild                                     | `app/history.rs`                                                    |
| Crash recovery (`check_crash_recovery`) + journal commit/discard helpers        | `app/recovery.rs`                                                   |
| Tool loop states + exec spawns + approval + recovery + commit                   | `app/tool_orchestration.rs`                                         |
| `InputMode`, `DraftInput`, `InputState`, Insert/Command mode wrappers           | `ui/input.rs`                                                       |
| `ScrollState`, `DisplayItem`                                                    | `ui/scroll.rs` and `ui/display.rs` (or combined)                    |
| `PredefinedModel` and model select helpers                                      | `ui/model_select.rs`                                                |
| Stream/tool error formatting and redaction                                      | `security.rs`                                                       |
| `truncate_with_ellipsis`, `panic_payload_to_string`, `append_tool_output_lines` | `util.rs`                                                           |
| `process_command`                                                               | `app/commands.rs`                                                   |

---

## 7) Testing strategy for a safe refactor

### Keep and expand the existing tests, but relocate them

You already have good input/editor tests in `lib.rs`. Move them to:

* `ui/input.rs`’s `#[cfg(test)]` module, or
* `engine/src/ui/tests.rs`.

### Add tests for the high-risk invariants

Focus on invariants that are easy to break during restructuring:

1. **Feature flag vs state transitions**

   * If ContextInfinity is disabled, tool execution must not implicitly enable it.
   * After the Phase 3.1 redesign, this is enforced structurally.

2. **Durability commit ordering**

   * Create in-memory journals and verify step transitions:

     * append -> seal -> history save -> commit/prune
   * Verify “save failed” path leaves recovery possible.

3. **Tool batch ordering**

   * Ensure committed history order stays:

     * assistant text (if non-empty) -> tool_use messages -> tool_result messages
   * Ensure missing results produce “Missing tool result” placeholders (as current logic does).

4. **Command parser**

   * Table-driven tests for `/tool error ...`, `/model`, `/provider`, `/clear`, etc.

---

## 8) Acceptance criteria (so you know the refactor is "done")

A refactor this large needs concrete "exit conditions":

1. `engine/src/lib.rs` is < ~200–300 LOC and contains **no** `impl App`.
   - **Current status (Phase 1-2):** ~3,432 LOC. Still contains `impl App` for tightly coupled state machine logic.
   - **Note:** Full extraction requires Phase 3 structural changes.
2. Each major behavior is in one file:
   - ✅ streaming (`streaming.rs`)
   - ✅ summarization (`summarization.rs`)
   - ✅ commands (`commands.rs`)
   - ✅ ui/input (`ui/input.rs`, `ui/modal.rs`)
   - 🔲 tools, history, recovery (remain in `lib.rs`, tightly coupled to state machine)
3. There is an explicit `features.context_infinity: bool` (or equivalent) not encoded via state variant.
   - **Status:** 🔲 Pending (Phase 3.1)
4. `AppState` (or `OperationState`) models **operations only**, not feature flags.
   - **Status:** 🔲 Pending (Phase 3.1)
5. No public API break for `cli` and `tui` (imports and method calls compile without changes).
   - **Status:** ✅ All 307+ tests pass, public API unchanged
6. Tests cover:
   - ✅ input/editor behavior (existing, moved to `tests.rs`)
   - 🔲 at least one durability flow test
   - 🔲 command parsing
   - 🔲 at least one tool-loop commit flow test

---

## 9) Optional (but high leverage) improvements after the refactor

If you want to go beyond organization and improve long-term maintainability:

1. **Dependency inversion for provider calls**

   * Replace direct calls to `forge_providers::send_message` with an injected trait:

     ```rust
     trait ProviderClient { async fn send_message(...); }
     ```
   * App holds `Arc<dyn ProviderClient>`.
   * Makes streaming tests deterministic.

2. **Tool execution as a service**

   * Wrap tool planning/execution in a `ToolRuntime` struct with a narrow API.
   * `App` only calls `tool_runtime.plan(...)`, `tool_runtime.spawn(...)`, `tool_runtime.poll(...)`.

3. **Event-driven orchestration**

   * Instead of `App` spawning tasks inside methods, return “effects”:

     * `Effect::StartStream(config, messages, ...)`
     * `Effect::StartToolCall(...)`
   * The runtime executes effects. This is more involved but yields very testable logic.

---

## Recommended path

If you want the highest ROI with the lowest risk, do it in this order:

1. **Phase 1 + Phase 2:** pure modularization (largest immediate win for navigation).
2. **Phase 3.1:** separate feature flags from operation state (prevents subtle bugs and simplifies logic).
3. **Phase 3.4:** typed command parser (big maintainability win).
4. Optional deeper redesign items only after the structure is clean.

---

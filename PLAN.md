# IFA Architectural Refactor Plan (2026-02-20)

## Objective

Refactor the workspace to strict Invariant-First Architecture (IFA) purity:

1. Invalid states are unrepresentable in Core.
2. State transitions consume proof/typestate tokens.
3. Core does not use optional lifecycle state (`Option`, bool flags, sentinel fields).
4. Branch-heavy runtime guarding is pushed to boundary conversion, then eliminated in Core.

Backwards compatibility is intentionally out of scope.

## Baseline Hotspots (from current scan)

| Crate | `Option<` | `bool` | `if` | Primary hotspot files |
| --- | ---: | ---: | ---: | --- |
| `engine` | 82 | 91 | 625 | `engine/src/app/mod.rs`, `engine/src/app/tool_loop.rs` |
| `tools` | 205 | 200 | 893 | `tools/src/git.rs`, `tools/src/search.rs` |
| `tui` | 58 | 36 | 451 | `tui/src/lib.rs`, `tui/src/input.rs` |
| `providers` | 101 | 28 | 172 | `providers/src/openai.rs`, `providers/src/sse_types.rs` |
| `context` | 46 | 22 | 113 | `context/src/tool_journal.rs`, `context/src/stream_journal.rs` |

## Phase 0 - Guardrail Hardening (No New Drift)

### What this phase does

1. Tighten conformance checks so known anti-patterns fail CI immediately.
2. Make hotspot tracking repeatable per commit.
3. Lock Core/Boundary classification for touched modules.

### Concrete changes

1. Extend `scripts/ifa_conformance_check.py` to fail on targeted lifecycle patterns in Core:
   - bool status fields in domain structs.
   - paired optionals representing a single state.
   - API signatures that expose optional lifecycle state.
2. Add a machine-readable smell report output (`json`) to support before/after diffs.
3. Wire checks into `just ifa-check` and `just verify`.
4. Update `ifa/classification_map.toml` with any reclassified modules touched in later phases.
5. Update `ifa/README.md` with new checker rules.

### Files

- `scripts/ifa_conformance_check.py`
- `justfile`
- `ifa/classification_map.toml`
- `ifa/README.md`

### Exit criteria

1. `just ifa-check` fails on known banned patterns in touched Core modules.
2. Baseline smell report is committed for comparison.

## Phase 1 - Canonical Domain State Types (`types` + `context`)

### What this phase does

1. Replace sentinel/bool lifecycle representation with closed enums.
2. Collapse multi-field optional state into one canonical type.

### Concrete changes

1. Replace `ToolResult { is_error: bool, ... }` with closed result variants.
   - Introduce `ToolResult::Success` and `ToolResult::Error` (or equivalent typed payload enums).
2. Replace `FullHistory` compaction optional pairs with a single enum:
   - `HistoryCompaction::Uncompacted`
   - `HistoryCompaction::Compacted { point, summary }`
3. Replace `ContextUsage { compacted: bool, ... }` with explicit usage variants:
   - `ContextUsage::Uncompacted { ... }`
   - `ContextUsage::Compacted { ... }`
4. Update formatting/status helpers to dispatch by variant, not bool checks.
5. Update affected tests to assert full-object outcomes (no field-by-field lifecycles).

### Files

- `types/src/lib.rs`
- `context/src/history.rs`
- `context/src/working_context.rs`
- `engine/src/app/mod.rs` (call-site updates)
- `tui/src/tool_display.rs` (rendering updates)
- `types/README.md`

### Exit criteria

1. No lifecycle bool remains for these state models.
2. No representable half-compacted history state remains.
3. `just verify` passes.

## Phase 2 - Context Adaptation Typestate Pipeline

### What this phase does

1. Remove adaptation flags and encode adaptation progress as typestate.
2. Ensure compaction requirements are represented as type-level outcomes.

### Concrete changes

1. Replace `ContextAdaptation::Shrinking { ..., needs_compaction: bool }` with explicit variants:
   - `ShrinkingPendingCompaction { ... }`
   - `ShrinkingReady { ... }`
2. Introduce transition APIs that consume adaptation tokens between stages.
3. Refactor context build orchestration to pattern-match adaptation variants only.
4. Remove legacy helper methods that expose optional or bool adaptation state.

### Files

- `context/src/manager.rs`
- `context/src/working_context.rs`
- `context/src/distillation.rs`
- `context/src/lib.rs`
- `context/README.md`

### Exit criteria

1. Adaptation path has no `needs_compaction` bool.
2. Illegal adaptation transitions are unrepresentable in safe APIs.
3. `just verify` passes.

## Phase 3 - Engine Turn Lifecycle Typestate

### What this phase does

1. Replace optional turn and pending-message state with explicit engine typestates.
2. Eliminate guard-based branching around turn progress.

### Concrete changes

1. Replace `pending_user_message: Option<(...)>` with:
   - `PendingMessageState::Idle`
   - `PendingMessageState::Pending(PendingUserMessage)`
2. Replace `turn_usage: Option<TurnUsage>` with:
   - `TurnUsageState::Idle`
   - `TurnUsageState::Recording(TurnUsageProof)`
3. Update stream start/finish/error transitions to consume and return these states.
4. Move any remaining runtime "is some" logic to boundary conversion points.

### Files

- `engine/src/app/mod.rs`
- `engine/src/app/state_access.rs`
- `engine/src/app/streaming.rs`
- `engine/src/state.rs`
- `engine/README.md`

### Exit criteria

1. No `Option` lifecycle state remains for turn/pending message management.
2. Turn transitions are compile-time constrained by typestate APIs.
3. `just verify` passes.

## Phase 4 - Engine Plan Approval and Tool-Loop Continuation Tokens

### What this phase does

1. Replace bool decision paths with explicit decision types.
2. Split tool-batch continuation behavior into typed continuations.

### Concrete changes

1. Replace `approved: bool` in plan approval flow with `PlanApprovalDecision` enum.
2. Replace `commit_tool_batch(..., auto_resume: bool)` with typed continuation design:
   - either split APIs (`commit_and_resume`, `commit_and_finish`)
   - or use `PostToolBatch::Resume | Stop` token.
3. Refactor journal commit/rollback/resume sequencing to consume continuation token.
4. Remove duplicated if/else tool-loop branches now represented by typed path.

### Files

- `engine/src/app/plan.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/state.rs`
- `context/src/tool_journal.rs` (integration adjustments)
- `docs/PARALLEL_TOOL_EXECUTION.md`

### Exit criteria

1. No bool-driven branch for approval outcome or post-batch continuation.
2. Tool-loop continuation path is represented by a closed type.
3. `just verify` passes.

## Phase 5 - Tools Crate Refactor (`git` + `search` first)

### What this phase does

1. Remove multi-bool execution status and invalid flag combinations.
2. Encode backend and completion outcomes as closed sets.

### Concrete changes

1. Replace `GitExecResult` status bools with typed outcome model:
   - `GitExecutionOutcome::{Success, Failure, TimedOut}`
   - truncation metadata in dedicated type.
2. Replace `GitBranchArgs { list_all, list_remote, ... }` with a single listing mode enum + action enum.
3. Replace `SearchResponse { truncated: bool, timed_out: bool }` with typed completion outcome.
4. Replace backend selection booleans with `SearchMode` that maps deterministically to backend capability.
5. Update argument parsing/validation so unsupported combinations fail at boundary conversion.

### Files

- `tools/src/git.rs`
- `tools/src/search.rs`
- `tools/src/lib.rs`
- `tools/src/types.rs` (if introduced)
- `tools/README.md`

### Exit criteria

1. No representable conflicting status flags in git/search domain models.
2. Search backend selection is type-driven, not guard-driven.
3. `just verify` passes.

## Phase 6 - Providers, TUI, and CLI Boundary Cleanup

### What this phase does

1. Remove optional provider request surfaces.
2. Replace TUI modal/approval bool matrices with explicit view states.
3. Replace CLI boolean trigger hooks with command/event tokens.

### Concrete changes

1. Refactor provider send APIs to explicit request variants:
   - with tools
   - without tools
   - with cache
   - without cache
2. Refactor approval UI model:
   - replace `selected: Vec<bool>`, `any_selected`, `deny_confirm` with typed selection/confirmation state.
3. Replace `ApprovalItem.summary: Option<String>` with explicit summary representation.
4. Replace `app.take_clear_transcript()` bool polling with explicit transcript command/event token.
5. Update engine-tui-cli interfaces to consume these typed events.

### Files

- `providers/src/lib.rs`
- `providers/src/openai.rs`
- `providers/src/gemini.rs`
- `tui/src/shared.rs`
- `tui/src/input.rs`
- `tui/src/lib.rs`
- `cli/src/main.rs`

### Exit criteria

1. Provider request API no longer relies on optional lifecycle parameters.
2. Approval/transcript UI flow uses explicit state/event types.
3. `just verify` passes.

## Phase 7 - Control-Flow Simplification and Final IFA Sweep

### What this phase does

1. Remove remaining guard-heavy logic in touched modules by using typestate dispatch.
2. Complete docs and architectural records for the new model.

### Concrete changes

1. Replace remaining lifecycle `if` branches with enum-pattern dispatch in touched files.
2. Remove dead compatibility shims and sentinel conversion helpers.
3. Update architecture docs and crate READMEs to reflect final authority boundaries.
4. Run final conformance and coverage pass.

### Files

- `INVARIANT_FIRST_ARCHITECTURE.md` (if rule clarifications are needed)
- `docs/` affected architecture docs
- `*/README.md` for touched crates
- any touched module with residual lifecycle guards

### Exit criteria

1. Touched Core modules are free of optional lifecycle state and bool lifecycle flags.
2. `just fix`, `just verify`, and `cargo cov` pass with non-decreasing coverage.
3. IFA checker shows no regressions.

## Commit and PR Slicing

1. One phase per PR/commit group; do not mix independent phase goals.
2. Sequence is strict: 0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7.
3. Every phase must include:
   - code changes
   - tests
   - docs updates for public API changes
   - checker/rule updates if new invariants are introduced

## Validation Checklist Per Phase

1. Run `just fix` immediately after edits.
2. Run `just verify` before finishing the phase.
3. If any unrelated failures appear, stop and ask whether to fix them now.
4. For final phase and major API shifts, run `cargo cov` and confirm coverage does not decrease.


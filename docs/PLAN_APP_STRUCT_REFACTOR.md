# App Struct IFA Refactor Plan

<!--
meta:
  doc_id: plan-app-struct-refactor-v1
  scope: engine App architecture refactor
  source: INVARIANT_FIRST_ARCHITECTURE.md
  target: engine crate
  date: 2026-02-17
  compatibility: none required
-->

## Objective

Refactor `App` in `engine` so lifecycle invariants are encoded structurally, transition authority is singular, and boundary work is isolated from core state transitions, following Invariant-First Architecture (IFA).

This plan assumes **no backward compatibility requirements** for internal engine interfaces.

## Success Criteria

1. `App` no longer owns duplicated lifecycle truth (`state` + shadow state + ad-hoc focus writes).
2. All `OperationState` transitions flow through one authority boundary.
3. Core payload optionality that encodes invalid states is removed or represented as structural variants.
4. Focus lifecycle state is derived from operation state (or transition-coupled through a single authority path).
5. Boundary side effects (journals, async tasks, LSP, tool process IO) are isolated behind runtime adapters that emit typed outcomes into core transitions.
6. Section 17 IFA process artifacts are published and kept in sync with implementation.

## Non-Goals

1. Keeping internal APIs source-compatible.
2. Minimal diff size.
3. Preserving existing refactor staging shims (`replace_with_idle`, `op_restore`) once replacement exists.
4. Tuning UI visual design.

## Governing IFA References

- `INVARIANT_FIRST_ARCHITECTURE.md:228` (single mutable owner)
- `INVARIANT_FIRST_ARCHITECTURE.md:269` (single proof representation)
- `INVARIANT_FIRST_ARCHITECTURE.md:273` (single authority boundary per proof)
- `INVARIANT_FIRST_ARCHITECTURE.md:285` (derived data must not be independently stored)
- `INVARIANT_FIRST_ARCHITECTURE.md:337` (state as structural location)
- `INVARIANT_FIRST_ARCHITECTURE.md:367` (transition by moving between states)
- `INVARIANT_FIRST_ARCHITECTURE.md:379` (capability tokens for phase-conditional operations)
- `INVARIANT_FIRST_ARCHITECTURE.md:408` (core optionality as invalid representable state)
- `INVARIANT_FIRST_ARCHITECTURE.md:462` (non-conforming patterns)
- `INVARIANT_FIRST_ARCHITECTURE.md:604` (Section 17 deliverables)

## Current-State Findings (Condensed)

1. `App` is a god-struct with mixed ownership concerns.
   - `engine/src/app/mod.rs:705` through `engine/src/app/mod.rs:827`
2. Shadow ownership exists for tools-disabled state.
   - `engine/src/app/mod.rs:780`, `engine/src/app/mod.rs:1666`
3. Transition authority is fragmented by `replace_with_idle`, `op_restore`, and local `mem::replace`.
   - `engine/src/app/mod.rs:1674`, `engine/src/app/mod.rs:1869`
   - `engine/src/app/streaming.rs:581`
   - `engine/src/app/tool_loop.rs:955`
   - `engine/src/app/plan.rs:447`
   - `engine/src/app/distillation.rs:139`
4. Core payload optionality still encodes invalid states.
   - `engine/src/state.rs:58` (`thinking_message: Option<Message>`)
   - `engine/src/state.rs:63` (`tool_batch_id: Option<ToolBatchId>`)
   - `engine/src/ui/view_state.rs:56` (`step_started_at: Option<Instant>`)
5. Focus state is mutable parallel state instead of derived state.
   - `engine/src/app/mod.rs:1894`
   - `engine/src/ui/view_state.rs:50`

Related analysis references:

- `docs/analysis.md:125`
- `docs/analysis.md:238`
- `docs/analysis.prompt.md:134`
- `docs/STREAM_FREEZE_REFACTOR.md:39`

## Architecture Target

### High-Level Shape

`App` becomes a facade over three explicit domains:

1. `core` (deterministic transitions and invariants)
2. `runtime` (async/boundary IO and side effects)
3. `ui projection` (render state that is either derived or explicitly view-only)

Proposed structure:

```rust
pub struct App {
    core: core::CoreMachine,
    runtime: runtime::RuntimeBoundary,
    view: ui::ViewState,
}
```

### Ownership Rules

1. `core::CoreMachine` is the **only** owner of operation lifecycle state.
2. `runtime::RuntimeBoundary` never mutates operation lifecycle directly.
3. `view` never stores duplicated lifecycle truth derivable from `core`.
4. "Tools disabled" exists in one place only: operation state variants.

### Transition Model

1. One transition authority API in core, for example:
   - `core.transition(intent, evidence) -> TransitionResult`
2. Runtime produces typed evidence (journal persisted, tool finished, stream errored, etc).
3. Core consumes evidence and emits side-effect intents for runtime.

### Boundary/Core Split

1. Boundary modules do conversion and side effects.
2. Core modules assume validated proofs and do not retry external failures.

## Invariant Registry (Initial)

This is the initial registry to implement and maintain during refactor.

| Invariant ID | Statement | Canonical Representation | Authority Boundary |
|---|---|---|---|
| APP-INV-1 | Exactly one operation lifecycle state exists at a time. | `OperationState` discriminated union | `core::operation` |
| APP-INV-2 | Tools-disabled latch has one owner. | `OperationState::ToolsDisabled` | `core::operation` |
| APP-INV-3 | Phase-conditional operations require phase evidence. | typed phase tokens / transition evidence | `core::operation` |
| APP-INV-4 | Tool loop input cannot encode missing batch identity as optional field. | `ToolLoopInput` split variants | `core::tool_loop` |
| APP-INV-5 | Thinking payload shape is encoded once. | single shared `ThinkingPayload` abstraction | `core::streaming` |
| APP-INV-6 | Focus lifecycle mode is not independently mutable from operation state. | derived projection or transition-owned update | `core::operation` + `ui` |
| APP-INV-7 | Persist-before-apply for stream/tool events. | persisted evidence token | `runtime::journal_writer` |
| APP-INV-8 | Journal commit protocol ordering is preserved. | explicit commit protocol API | `runtime::journals` |
| APP-INV-9 | Recovery-blocked state is explicit and sticky until explicit reset path. | `OperationState::RecoveryBlocked` | `core::recovery` |

## Authority Boundary Map (Initial)

| Proof / Controlled Type | Enforced Invariant | Boundary Module |
|---|---|---|
| `OperationState` transition evidence | legal edge progression | `engine/src/core/operation.rs` (new) |
| `PersistedEventProof` (new) | journal durability before apply | `engine/src/runtime/journal_writer.rs` (new) |
| `ToolBatchIdentity` variants (new) | no optional batch identity in core | `engine/src/core/tool_loop.rs` (new or migrated) |
| `ThinkingPayload` (new) | single thinking representation | `engine/src/core/streaming.rs` |
| `FocusProjection` (new) | focus lifecycle derives from operation state + mode | `engine/src/ui/focus_projection.rs` (new) |

## DRY Proof Map (Initial)

| Invariant | Existing Duplicate Encodings | Planned Canonical Encoding |
|---|---|---|
| tools disabled | `tools_disabled_state` + `OperationState::ToolsDisabled` | `OperationState::ToolsDisabled` only |
| transition legality | scattered runtime guards + local matches | single `core` transition graph |
| operation busy checks | `busy_reason` + ad-hoc per-call matches | command routing by state-typed API |
| thinking optionality | `ToolLoopInput`, `ToolBatch`, `ToolCommitPayload` each use `Option<Message>` | one `ThinkingPayload` abstraction |
| focus lifecycle | operation-driven ad-hoc writes + independent view field | derived projection or single transition-owned write path |

## Workstreams

### WS1 - Core Lifecycle Ownership

Goal: move lifecycle ownership and transition graph into `core`.

Primary files:

- `engine/src/core/mod.rs`
- `engine/src/state.rs` (migrate or re-export while moving)
- `engine/src/app/mod.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/plan.rs`
- `engine/src/app/distillation.rs`
- `engine/src/app/commands.rs`
- `engine/src/app/persistence.rs`

### WS2 - Runtime Boundary Isolation

Goal: isolate async/boundary side effects and emit typed evidence back to core.

Primary files:

- `engine/src/runtime/mod.rs`
- `engine/src/runtime/journal_writer.rs` (new)
- `engine/src/runtime/stream_driver.rs` (new)
- `engine/src/runtime/tool_driver.rs` (new)
- `engine/src/runtime/lsp_driver.rs` (new)
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/lsp_integration.rs`

### WS3 - UI Projection Cleanup

Goal: make focus state derived or single-owned.

Primary files:

- `engine/src/ui/view_state.rs`
- `engine/src/ui/mod.rs`
- `tui/src/lib.rs`
- `tui/src/shared.rs` (if focus-dependent views are here)
- `docs/FOCUS_VIEW_REVIEW.md`

### WS4 - Process Conformance Artifacts

Goal: deliver and keep Section 17 artifacts.

Primary files:

- `docs/ifa/app_invariant_registry.md` (new)
- `docs/ifa/authority_boundary_map.md` (new)
- `docs/ifa/dry_proof_map.md` (new)
- `docs/analysis.md`
- `engine/README.md` and crate READMEs as needed

## Phase Plan

## Phase 0 - Baseline and Refactor Guardrails

### Purpose

Freeze current behavior baseline and prevent uncontrolled drift during refactor.

### Tasks

1. Capture transition matrix tests for current legal flows.
2. Add "single transition authority" lint gate by reducing direct state writes in hot paths.
3. Add TODO markers indicating temporary shims scheduled for deletion.

### File Targets

- `engine/src/app/tests.rs`
- `engine/src/app/mod.rs`
- `docs/analysis.md`

### Exit Criteria

1. Existing behaviors are test-pinned before large movement.
2. Every direct operation-state mutation site is catalogued.

## Phase 1 - Introduce Core Transition Authority

### Purpose

Create a single operation transition API and route all transitions through it.

### Tasks

1. Add core transition module with:
   - operation tag graph
   - transition intent enum
   - legality checks
   - edge effect emission interface
2. Refactor callers to use the transition API.
3. Keep temporary bridge methods only for migration.

### File Targets

- `engine/src/core/operation.rs` (new)
- `engine/src/core/mod.rs`
- `engine/src/app/mod.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/plan.rs`
- `engine/src/app/distillation.rs`
- `engine/src/app/commands.rs`

### Exit Criteria

1. No direct `self.state = ...` in app modules outside core authority.
2. `replace_with_idle` and `op_restore` are removed or become private no-op shims slated for deletion in Phase 2.
3. Transition tests cover all legal and illegal edges.

## Phase 2 - Remove Shadow Ownership (`tools_disabled_state`)

### Purpose

Make tools-disabled state single-owned by operation state.

### Tasks

1. Delete `tools_disabled_state` field from `App`.
2. Delete `idle_state()` dual-source behavior.
3. Route all checks and state derivations through `OperationState`.
4. Update persistence/recovery paths that currently set shadow latch.

### File Targets

- `engine/src/app/mod.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/persistence.rs`
- `engine/src/app/commands.rs`
- `engine/src/app/init.rs`

### Exit Criteria

1. `tools_disabled_state` removed.
2. `OperationState::ToolsDisabled` is the only representation.
3. No logic branches read a removed shadow source.

## Phase 3 - Eliminate Core Optionality Violations

### Purpose

Remove optional core fields representing invalid states.

### Tasks

1. Split `ToolLoopInput`:
   - `NewToolLoopInput` (no existing batch)
   - `ExistingToolLoopInput` (requires batch id)
2. Replace `thinking_message: Option<Message>` triplication with one shared abstraction.
3. Remove `FocusState::Executing.step_started_at: Option<Instant>`:
   - require `Instant` when in executing mode, or
   - derive elapsed without storing optional.

### File Targets

- `engine/src/state.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/plan.rs`
- `engine/src/ui/view_state.rs`

### Exit Criteria

1. No `Option<ToolBatchId>` in core tool-loop payload.
2. Thinking representation is single-source.
3. Executing focus mode has no optional start time.

## Phase 4 - Focus Projection Refactor

### Purpose

Remove mutable parallel ownership of focus lifecycle.

### Tasks

1. Implement `FocusProjection` from:
   - operation state
   - plan state
   - view mode
2. Keep mutable state only for review navigation cursor if required.
3. Remove ad-hoc focus lifecycle writes from app flow methods.

### File Targets

- `engine/src/ui/focus_projection.rs` (new)
- `engine/src/ui/view_state.rs`
- `engine/src/app/mod.rs`
- `tui/src/lib.rs`

### Exit Criteria

1. Focus lifecycle mode is derived, not independently assigned in stream/tool code paths.
2. No stale focus mismatch after cancel/error/recovery edges.

## Phase 5 - Runtime Boundary Extraction

### Purpose

Isolate boundary side effects and keep core deterministic.

### Tasks

1. Add runtime actors/drivers for journals and tool/stream IO.
2. Convert stream processing to ingest/ack model with persistence proof before apply.
3. Move retry/backoff and IO-specific error handling to runtime boundary.
4. Keep core logic free of compensatory boundary retries.

### File Targets

- `engine/src/runtime/journal_writer.rs` (new)
- `engine/src/runtime/stream_driver.rs` (new)
- `engine/src/runtime/tool_driver.rs` (new)
- `engine/src/runtime/lsp_driver.rs` (new)
- `engine/src/runtime/mod.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/lsp_integration.rs`

### Exit Criteria

1. Core streaming/tool transitions consume typed runtime outputs, not raw IO outcomes.
2. Persist-before-apply becomes unforgeable at call sites.
3. Runtime health failure enters explicit blocked state.

## Phase 6 - Command Routing by State-Shaped API

### Purpose

Replace broad `busy_reason` guard patterns with state-shaped command eligibility.

### Tasks

1. Introduce command dispatch by operation variant (or typed command gate).
2. Move command preconditions into compile-time-visible API shape.
3. Keep user-facing errors at boundary formatting layer.

### File Targets

- `engine/src/app/commands.rs`
- `engine/src/core/command_gate.rs` (new)
- `engine/src/app/mod.rs`

### Exit Criteria

1. Command legality is encoded by state-aware dispatch, not optional string guards.
2. Fewer defensive `if busy` branches in command handlers.

## Phase 7 - Conformance Artifacts and Documentation Finalization

### Purpose

Complete Section 17 artifacts and align architecture docs to implementation.

### Tasks

1. Publish invariant registry, authority boundary map, and DRY proof map.
2. Update `docs/analysis.md` status from planned to implemented.
3. Update crate docs and `engine/README.md`.

### File Targets

- `docs/ifa/app_invariant_registry.md` (new)
- `docs/ifa/authority_boundary_map.md` (new)
- `docs/ifa/dry_proof_map.md` (new)
- `docs/analysis.md`
- `engine/README.md`

### Exit Criteria

1. Section 17 deliverables exist and match code.
2. Documentation no longer references removed shims.

## PR Slicing Strategy

Recommended incremental merge plan:

1. PR-1: Phase 0 (baseline tests, guardrails)
2. PR-2: Phase 1 transition authority skeleton + limited path adoption
3. PR-3: Phase 1 completion + Phase 2 shadow state removal
4. PR-4: Phase 3 payload/optionality cleanup
5. PR-5: Phase 4 focus projection migration
6. PR-6: Phase 5 runtime boundary actorization
7. PR-7: Phase 6 command dispatch rewrite
8. PR-8: Phase 7 docs and Section 17 artifacts

## Testing Strategy

## Core Transition Tests

1. Valid edge progression matrix.
2. Invalid edge rejection matrix.
3. Edge side-effect assertions (focus projection, notifications, completion transitions).

## Recovery and Journal Tests

1. Persist-before-apply proof requirement.
2. Tool batch recovery with matching and mismatching step ids.
3. Stream/tool cancellation consistency.
4. Commit ordering correctness (`seal -> history save -> commit/prune`).

## Behavioral Regression Tests

1. Streaming cancel and clear behavior.
2. Tool approval workflows (approve selected, deny all, plan approval).
3. Distillation with queued message recovery.
4. Plan approval/rollback flows.

## Property and Stress Tests

1. No illegal focus/operation pair after any transition sequence.
2. Runtime backlog does not produce core state divergence.
3. Recovery idempotency with repeated startup recovery attempts.

## Risk Register

| Risk | Severity | Likelihood | Mitigation |
|---|---|---|---|
| Transition rewrite introduces hidden behavioral regressions | High | Medium | Phase 0 baseline tests + incremental PRs |
| Runtime actorization creates deadlock or starvation under load | High | Medium | bounded channels + timeout tests + observability counters |
| Focus derivation breaks UX navigation semantics | Medium | Medium | keep navigation state minimal and separate from lifecycle projection |
| Optionality removal causes broad compile churn | Medium | High | perform in isolated PR after transition authority stabilizes |
| Recovery behavior drift in failure paths | High | Medium | dedicated recovery matrix tests + explicit blocked-state assertions |

## Rollout and Operational Guidance

1. Refactor is internal-breaking only; perform as coordinated series.
2. Do not run partial mixed states in long-lived branches; rebase frequently.
3. Keep `docs/analysis.md` updated per merged phase.
4. Keep all new transition and invariant docs close to code changes.

## Completion Checklist

1. `tools_disabled_state` removed from `App`.
2. `replace_with_idle`, `idle_state`, and `op_restore` removed or reduced to transitional wrappers with zero production callsites.
3. No core `Option<ToolBatchId>` in tool-loop ingress.
4. Thinking payload optionality deduplicated.
5. Focus lifecycle projection no longer independently mutable from operation state.
6. Runtime side effects are isolated and proof-emitting.
7. Command legality is state-shaped.
8. Section 17 artifacts are published and current.
9. `just fix` and `just verify` pass.

## Reviewed Anchors (Source Audit)

The following anchors were reviewed while building this plan:

- `INVARIANT_FIRST_ARCHITECTURE.md:228`
- `INVARIANT_FIRST_ARCHITECTURE.md:269`
- `INVARIANT_FIRST_ARCHITECTURE.md:285`
- `INVARIANT_FIRST_ARCHITECTURE.md:337`
- `INVARIANT_FIRST_ARCHITECTURE.md:367`
- `INVARIANT_FIRST_ARCHITECTURE.md:379`
- `INVARIANT_FIRST_ARCHITECTURE.md:408`
- `INVARIANT_FIRST_ARCHITECTURE.md:462`
- `INVARIANT_FIRST_ARCHITECTURE.md:604`
- `engine/src/app/mod.rs:705`
- `engine/src/app/mod.rs:780`
- `engine/src/app/mod.rs:827`
- `engine/src/app/mod.rs:1309`
- `engine/src/app/mod.rs:1621`
- `engine/src/app/mod.rs:1666`
- `engine/src/app/mod.rs:1774`
- `engine/src/app/mod.rs:1894`
- `engine/src/app/mod.rs:2108`
- `engine/src/state.rs:56`
- `engine/src/state.rs:142`
- `engine/src/state.rs:350`
- `engine/src/state.rs:483`
- `engine/src/state.rs:597`
- `engine/src/app/init.rs:75`
- `engine/src/app/init.rs:104`
- `engine/src/app/init.rs:171`
- `engine/src/app/init.rs:266`
- `engine/src/app/commands.rs:323`
- `engine/src/app/commands.rs:401`
- `engine/src/app/tool_loop.rs:214`
- `engine/src/app/tool_loop.rs:442`
- `engine/src/app/tool_loop.rs:955`
- `engine/src/app/tool_loop.rs:1577`
- `engine/src/app/tool_loop.rs:1868`
- `engine/src/app/streaming.rs:240`
- `engine/src/app/streaming.rs:472`
- `engine/src/app/streaming.rs:581`
- `engine/src/app/streaming.rs:777`
- `engine/src/app/distillation.rs:25`
- `engine/src/app/distillation.rs:115`
- `engine/src/app/distillation.rs:139`
- `engine/src/app/persistence.rs:281`
- `engine/src/app/persistence.rs:365`
- `engine/src/app/persistence.rs:609`
- `engine/src/app/persistence.rs:763`
- `engine/src/app/plan.rs:37`
- `engine/src/app/plan.rs:442`
- `engine/src/ui/view_state.rs:50`
- `engine/src/ui/view_state.rs:56`
- `docs/analysis.md:125`
- `docs/analysis.md:238`
- `docs/STREAM_FREEZE_REFACTOR.md:39`
- `docs/FOCUS_VIEW_REVIEW.md:11`
- `cli/src/main.rs:315`

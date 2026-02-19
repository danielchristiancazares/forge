# IFA Original Phases

Source of truth: `docs/PLAN_APP_STRUCT_REFACTOR.md` (dated 2026-02-17).
This document mirrors the original phase breakdown.

## Phase 0 - Baseline and Refactor Guardrails

Purpose: freeze behavior baseline and prevent uncontrolled drift.

Tasks:
- Capture transition matrix tests for current legal flows.
- Add a single-transition-authority guardrail by reducing direct state writes.
- Add TODO markers for transitional shims scheduled for deletion.

Exit criteria:
- Existing behavior is test-pinned before large movement.
- Every direct operation-state mutation site is catalogued.

## Phase 1 - Introduce Core Transition Authority

Purpose: create one operation transition API and route all transitions through it.

Tasks:
- Add core transition module (tag graph, transition intents, legality checks, edge effect emission).
- Refactor callers to use the transition API.
- Keep temporary bridge methods only for migration.

Exit criteria:
- No direct `self.state = ...` in app modules outside core authority.
- `replace_with_idle` and `op_restore` removed or reduced to private transitional shims.
- Transition tests cover legal and illegal edges.

## Phase 2 - Remove Shadow Ownership (`tools_disabled_state`)

Purpose: make tools-disabled state single-owned by operation state.

Tasks:
- Delete `tools_disabled_state` from `App`.
- Delete dual-source `idle_state()` behavior.
- Route checks/derivations through `OperationState`.
- Update persistence/recovery paths that set shadow latch.

Exit criteria:
- `tools_disabled_state` removed.
- `OperationState::ToolsDisabled` is the only representation.
- No logic branches rely on removed shadow source.

## Phase 3 - Eliminate Core Optionality Violations

Purpose: remove optional core fields that encode invalid states.

Tasks:
- Split tool-loop ingress payload into explicit variants (`new` vs `existing` batch identity).
- Replace triplicated `thinking_message: Option<Message>` with one shared abstraction.
- Remove `FocusState::Executing.step_started_at: Option<Instant>` optionality.

Exit criteria:
- No `Option<ToolBatchId>` in core tool-loop ingress.
- Thinking representation is single-source.
- Executing focus mode has no optional start time.

## Phase 4 - Focus Projection Refactor

Purpose: remove mutable parallel ownership of focus lifecycle.

Tasks:
- Implement `FocusProjection` from operation state, plan state, and view mode.
- Keep mutable state only for review navigation cursor if needed.
- Remove ad-hoc focus lifecycle writes from app flow methods.

Exit criteria:
- Focus lifecycle mode is derived, not independently assigned in stream/tool paths.
- No stale focus mismatch after cancel/error/recovery edges.

## Phase 5 - Runtime Boundary Extraction

Purpose: isolate boundary side effects and keep core deterministic.

Tasks:
- Add runtime drivers/actors for journals and tool/stream I/O.
- Convert stream processing to ingest/ack with persistence proof before apply.
- Move retry/backoff and I/O-specific error handling to runtime.
- Keep core free of compensatory boundary retries.

Exit criteria:
- Core transitions consume typed runtime outputs, not raw I/O outcomes.
- Persist-before-apply is unforgeable at call sites.
- Runtime health failure enters explicit blocked state.

## Phase 6 - Command Routing by State-Shaped API

Purpose: replace broad busy-reason guard patterns with state-shaped command eligibility.

Tasks:
- Introduce command dispatch by operation variant (or typed command gate).
- Move command preconditions into compile-time-visible API shape.
- Keep user-facing errors at boundary formatting layer.

Exit criteria:
- Command legality encoded by state-aware dispatch, not optional string guards.
- Fewer defensive `if busy` branches in command handlers.

## Phase 7 - Conformance Artifacts and Documentation Finalization

Purpose: complete Section 17 artifacts and align architecture docs to implementation.

Tasks:
- Publish invariant registry, authority boundary map, and DRY proof map.
- Update `docs/analysis.md` status from planned to implemented.
- Update crate docs and `engine/README.md`.

Exit criteria:
- Section 17 deliverables exist and match code.
- Documentation no longer references removed shims.

## Original PR Slicing Strategy

1. PR-1: Phase 0
2. PR-2: Phase 1 skeleton + limited path adoption
3. PR-3: Phase 1 completion + Phase 2
4. PR-4: Phase 3
5. PR-5: Phase 4
6. PR-6: Phase 5
7. PR-7: Phase 6
8. PR-8: Phase 7 docs and Section 17 artifacts

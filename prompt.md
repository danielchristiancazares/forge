# Refactor Prompt: Centralize OperationState Transitions Without Behavior Changes

## Problem
`OperationState` transitions are scattered across multiple files via direct `self.state = ...` assignments.
`FocusState` is a parallel UI state machine that must stay in sync, but coupling is ad-hoc.
This makes transition flow hard to reason about and makes adding side-effects (tracing, telemetry, UI sync) error-prone.

## Goal
Centralize `OperationState` transitions behind a single API so transition side-effects are derived in one place, while preserving existing behavior.

## Non-goals
- No feature changes.
- No behavioral changes to stream/tool/distillation flow.
- No broad architecture rewrite.

## Key semantic constraints (important)
1. Focus sync must be edge-based, not target-only:
   - Enter `Executing` only when transitioning **into** `OperationState::Streaming` from a non-streaming state.
   - Do not reset `step_started_at` on `Streaming -> Streaming` updates.
2. Transitioning to `Idle` must **not** always force `Reviewing`.
   - `Reviewing` should only happen when a turn actually finishes (same semantics as current `finish_turn` path), not on every idle transition (e.g., cancel/clear/error extraction paths).
3. Keep `mem::replace` extraction patterns semantically safe.
   - Some code temporarily swaps state to take ownership (`replace_with_idle` style).
   - Those temporary swaps should not trigger UI transition side-effects.

## Implementation plan
1. Add a centralized transition method on `App`:
   - Preferred shape: `fn transition_to(&mut self, new_state: OperationState)`
   - It should compute `old_state -> new_state`, assign state, and handle side-effects.
   - Optional: `tracing::trace!` for `old_state` and `new_state`.
2. Introduce an explicit way to represent transition intent for the "turn finished" case.
   - Either:
     - a small transition cause enum (e.g., `TransitionCause::Normal | TransitionCause::TurnFinished`), or
     - a dedicated helper called at turn completion that triggers reviewing.
   - Requirement: preserve current behavior where reviewing is tied to turn completion.
3. Replace direct `self.state = OperationState::*` assignments with centralized transitions where they represent real state transitions.
4. Preserve or refactor temporary extraction paths so they do not trigger transition side-effects accidentally.
5. Remove ad-hoc manual focus-state sync from:
   - `engine/src/streaming.rs` (`start_streaming`)
   - `engine/src/tool_loop.rs` (`finish_turn`)
   after equivalent centralized behavior is in place.

## Enforcement
Make direct state assignment difficult/impossible outside the transition API:
- Prefer encapsulation and private field access patterns.
- If module privacy does not fully prevent this in current layout, add a lightweight guard (lint/check script/CI grep) to block new `self.state =` usage outside allowed internals.

## Files likely involved
- `engine/src/lib.rs`
- `engine/src/streaming.rs`
- `engine/src/tool_loop.rs`
- `engine/src/distillation.rs`
- `engine/src/commands.rs`
- `engine/src/persistence.rs`
- `engine/src/plan.rs`
- `engine/src/init.rs` (initial state setup, optional)
- `engine/src/tests.rs` (update scaffolding as needed)

## Validation
- `rg "self\.state\s*=" engine/src` should show only intentional internal helpers (or zero direct assignment sites, depending on final approach).
- Behavior should remain unchanged.
- Run:
  - `just fix`
  - `just verify`

## Acceptance criteria
- All meaningful `OperationState` transitions flow through one API.
- Focus sync is centralized and edge-correct.
- No `Streaming -> Streaming` timer reset regressions.
- No accidental `Idle -> Reviewing` on non-turn-complete paths.
- Tests and verification pass.

---

## Short copy/paste prompt
Refactor `engine` to centralize `OperationState` transitions behind one `App` API (e.g., `transition_to(...)`) with **no behavior change**.

Current issue: `self.state = ...` is scattered across multiple files, and `FocusState` sync is ad-hoc.

Requirements:
1. Route real `OperationState` transitions through one method and keep side-effects there (focus sync, future tracing/telemetry).
2. Focus sync must be edge-based:
   - set `Executing` only when entering `Streaming` from non-streaming
   - do **not** reset timer on `Streaming -> Streaming`
3. Do **not** force `Reviewing` on every `-> Idle`; only do it on actual turn completion (preserve existing `finish_turn` semantics).
4. Keep temporary ownership-extraction patterns (`mem::replace` / `replace_with_idle`) semantically safe; they should not accidentally trigger UI side-effects.
5. Replace direct `self.state = OperationState::*` assignments where they represent real transitions.
6. Remove manual focus sync in `streaming.rs:start_streaming` and `tool_loop.rs:finish_turn` once centralized behavior is equivalent.
7. Add enforcement so new direct `self.state =` assignments are discouraged/blocked (encapsulation and/or CI guard).

Validation:
- `rg "self\\.state\\s*=" engine/src` reflects the intended constrained usage.
- Run `just fix` and `just verify`.

## One-paragraph LLM-safe prompt
Please do a pure refactor (no behavior change) that centralizes all meaningful `OperationState` transitions in the engine behind one `App` transition API, and derive `FocusState` side-effects there instead of ad-hoc call sites; keep focus synchronization edge-based (enter `Executing` only on non-streaming -> streaming, never reset timers on streaming -> streaming), preserve existing turn-complete semantics for `Reviewing` (do not map every idle transition to reviewing), keep temporary ownership extraction patterns (`mem::replace`/`replace_with_idle`) side-effect safe, replace direct `self.state = OperationState::*` assignments that represent real transitions, remove redundant manual focus sync in `start_streaming`/`finish_turn` once equivalent centralized logic exists, and leave the system behavior unchanged while making direct future state writes harder via encapsulation and/or a lightweight CI guard.

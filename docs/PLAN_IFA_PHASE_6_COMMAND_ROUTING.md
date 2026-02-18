# PLAN_IFA_PHASE_6_COMMAND_ROUTING

## Purpose

Encode command legality by state shape instead of optional string guards.

## Drivers

- `busy_reason()` is a broad optional guard today:
  - `engine/src/app/mod.rs:1633-1645`
- Public mutable focus access allows side mutation:
  - `engine/src/app/mod.rs:867-869`

## Scope

- Replace guard-string checks with phase-aware command gates.
- Route command handling through state-specific APIs.
- Remove public mutable focus escape hatches.

## Tasks

1. Introduce command gate module keyed by `OperationTag` or capability tokens.
2. Convert command entrypoints to state-shaped handlers.
3. Remove `busy_reason` from operational legality checks.
4. Remove `focus_state_mut` and expose only explicit focus actions.

## Candidate files

- `engine/src/core/command_gate.rs` (new)
- `engine/src/app/commands.rs`
- `engine/src/app/mod.rs`
- `engine/src/app/input_modes.rs`

## Exit criteria

- No `busy_reason` gate remains in command start paths.
- Command legality is encoded through explicit phase/token APIs.
- No public mutable focus state accessor on `App`.

## Validation

- `just fix`
- `just verify`

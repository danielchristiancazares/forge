# PLAN_IFA_PHASE_3_TOOLING_AXIS

## Purpose

Represent tooling health as a first-class axis inside operation state ownership.

## Drivers

- Current tool latch is separate (`tool_gate`) and exposes optional and boolean APIs:
  - `engine/src/app/tool_gate.rs:36-64`
  - `engine/src/app/mod.rs:1620-1623`
- Tooling state is orthogonal to operation phase and should be encoded as such.

## Scope

- Replace standalone `ToolGate` with tooling axis in `OperationMachine`.
- Remove `Option`-based reason reads and boolean transition outputs.
- Introduce token-style capability for tool execution entrypoints.

## Tasks

1. Add `ToolingState` to operation machine:
   - `Enabled`
   - `Disabled(ToolingDisabledReason)`
2. Replace `reason() -> Option<&str>` with typed status response.
3. Replace `disable() -> bool` with typed transition result:
   - `NewlyDisabled`
   - `AlreadyDisabled`
4. Update mid-loop and recovery logic to consume typed tooling status.

## Candidate files

- `engine/src/core/operation/*`
- `engine/src/app/tool_gate.rs` (delete or absorb)
- `engine/src/app/mod.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/persistence.rs`

## Exit criteria

- App no longer stores `tool_gate`.
- Tool execution requires tooling-enabled capability.
- No optional tooling-reason API on core paths.

## Validation

- `just fix`
- `just verify`

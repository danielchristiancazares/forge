# PLAN_IFA_PHASE_2_OPERATION_AUTHORITY

## Purpose

Make `core::operation` the only authority that can change operation phase.

## Drivers

- Transition labels exist (`OperationTag`, `OperationEdge`), but authority is still fragmented.
- `mem::replace` plus `op_restore` appears in multiple modules:
  - `engine/src/app/tool_loop.rs:952-969`
  - `engine/src/app/distillation.rs:139-149`
  - `engine/src/app/plan.rs:442-454`

## Scope

- Move transition legality graph and edge logic out of `App`.
- Expose one transition API (`apply(event)` style) from `core::operation`.
- Remove direct phase mutation from app submodules.

## Tasks

1. Add `core::operation` module with:
   - machine state
   - legal transition graph
   - transition receipt/result type
2. Move legality checks from `App` into this module.
3. Convert callsites from ad hoc mutation to machine events.
4. Keep edge/event names stable via `OperationEdge`.

## Candidate files

- `engine/src/core/operation/mod.rs` (new)
- `engine/src/core/operation/machine.rs` (new)
- `engine/src/core/operation/graph.rs` (new)
- `engine/src/core/mod.rs`
- `engine/src/app/mod.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/distillation.rs`
- `engine/src/app/plan.rs`
- `engine/src/app/streaming.rs`

## Exit criteria

- No transition legality logic remains in `engine/src/app/mod.rs`.
- Operation phase mutation occurs only through `core::operation`.
- Existing edge logging semantics remain intact.

## Validation

- `just fix`
- `just verify`

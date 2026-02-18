# PLAN_IFA_REFACTOR_OVERVIEW

## Goal

Refactor `forge-engine` toward an invariant-first architecture with one owner per invariant, explicit capability boundaries, and no representable invalid core states.

This plan is derived from the review anchors listed below and from `INVARIANT_FIRST_ARCHITECTURE.md`.

## Review anchors

- `INVARIANT_FIRST_ARCHITECTURE.md:226-259`
- `INVARIANT_FIRST_ARCHITECTURE.md:263-288`
- `INVARIANT_FIRST_ARCHITECTURE.md:380-390`
- `INVARIANT_FIRST_ARCHITECTURE.md:394-419`
- `engine/src/app/mod.rs:706-828`
- `engine/src/app/mod.rs:863-905`
- `engine/src/app/mod.rs:1620-1645`
- `engine/src/app/mod.rs:1647-1661`
- `engine/src/app/mod.rs:1679-1687`
- `engine/src/state.rs:37-53`
- `engine/src/state.rs:55-76`
- `engine/src/state.rs:138-189`
- `engine/src/state.rs:269-306`
- `engine/src/state.rs:350-360`
- `engine/src/state.rs:572-639`
- `engine/src/app/tool_gate.rs:36-64`
- `engine/src/app/streaming.rs:76-111`
- `engine/src/app/streaming.rs:240-340`
- `engine/src/app/streaming.rs:851-889`
- `engine/src/app/tool_loop.rs:183-199`
- `engine/src/app/tool_loop.rs:213-241`
- `engine/src/app/tool_loop.rs:260-420`
- `engine/src/app/tool_loop.rs:952-1004`
- `engine/src/app/distillation.rs:139-149`
- `engine/src/app/plan.rs:442-454`
- `engine/src/app/persistence.rs:278-338`
- `engine/src/app/persistence.rs:408-420`
- `engine/src/ui/view_state.rs:50-67`
- `tui/src/focus/mod.rs:11-22`

## Phase map

- `docs/PLAN_IFA_PHASE_0_GUARDRAILS.md`
- `docs/PLAN_IFA_PHASE_1_APP_SPLIT.md`
- `docs/PLAN_IFA_PHASE_2_OPERATION_AUTHORITY.md`
- `docs/PLAN_IFA_PHASE_3_TOOLING_AXIS.md`
- `docs/PLAN_IFA_PHASE_4_OPTIONALITY_ELIMINATION.md`
- `docs/PLAN_IFA_PHASE_5_RUNTIME_BOUNDARY_PROOFS.md`
- `docs/PLAN_IFA_PHASE_6_COMMAND_ROUTING.md`
- `docs/PLAN_IFA_PHASE_7_FOCUS_PROJECTION.md`

## Sequencing

1. Phase 1 first to remove borrow-driven coordination pressure.
2. Phase 2 to centralize transition authority.
3. Phase 3 and 4 to encode tooling and payload validity by type shape.
4. Phase 5 to harden runtime boundary and unforgeable persistence proofs.
5. Phase 6 and 7 to finish command legality and focus projection cleanup.

## Done criteria

- No transition authority outside `core::operation`.
- No phase-sensitive legality encoded as `Option`/boolean output in core APIs.
- Tool execution and durable commits require runtime-minted capability proofs.
- Focus rendering is a pure projection from core state plus minimal UI cursor state.

# PLAN_IFA_PHASE_4_OPTIONALITY_ELIMINATION

## Purpose

Remove representable invalid states from core payloads and phase transition APIs.

## Drivers

- Optional core payload fields:
  - `engine/src/state.rs:55-76`
  - `engine/src/state.rs:350-360`
- Transition panic paths and option-returning APIs:
  - `engine/src/state.rs:138-189`
  - `engine/src/state.rs:269-306`
- Optional focus timestamp:
  - `engine/src/ui/view_state.rs:50-67`
- Optional thinking handling:
  - `engine/src/app/streaming.rs:76-111`

## Scope

- Replace optional tool-loop payload fields with domain-shaped types.
- Remove `unreachable!` usage that can be prevented by type boundaries.
- Reshape stream state to variant-specific APIs.
- Replace optional focus execution timestamp with representable state variants or always-present timestamp.

## Tasks

1. Replace `ToolLoopInput`/`ToolCommitPayload` optional members with typed phase payloads.
2. Introduce `ThinkingPayload` model for the tool pipeline.
3. Split stream typestate APIs so variant-only operations are not `Option` returning.
4. Update streaming and tool-loop callsites for new payloads.
5. Update focus state model to avoid `Option<Instant>` for executing mode.

## Candidate files

- `engine/src/state.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/ui/view_state.rs`
- `engine/src/core/thinking.rs` (new)

## Exit criteria

- No `Option<ToolBatchId>` on tool-loop ingress.
- No optional thinking field in core tool lifecycle payloads.
- No transition methods that rely on `unreachable!` for caller discipline.

## Validation

- `just fix`
- `just verify`

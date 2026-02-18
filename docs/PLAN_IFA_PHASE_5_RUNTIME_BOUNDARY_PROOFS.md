# PLAN_IFA_PHASE_5_RUNTIME_BOUNDARY_PROOFS

## Purpose

Make persistence proofs unforgeable and isolate retry policy in runtime boundary components.

## Drivers

- `JournalStatus` proof is crate-forgeable today (`engine/src/state.rs:37-53`).
- Journal fallback and retry policy lives inline in orchestration paths:
  - `engine/src/app/tool_loop.rs:260-420`
  - `engine/src/app/persistence.rs:278-338`

## Scope

- Replace forgeable proof constructors with runtime-minted capabilities.
- Move retry/backoff logic into dedicated runtime policy modules.
- Make core require capabilities for durable operations.

## Tasks

1. Replace `JournalStatus` with private-constructor proof types:
   - `PersistedToolBatch`
   - `PersistedStreamStep`
2. Add runtime writer/driver modules:
   - `journal_writer`
   - `stream_driver`
   - `tool_driver`
3. Move commit fallback/retry policy from app orchestrators into runtime modules.
4. Refactor core callsites to require proof tokens instead of raw ids.

## Candidate files

- `engine/src/runtime/journal_writer.rs` (new)
- `engine/src/runtime/stream_driver.rs` (new)
- `engine/src/runtime/tool_driver.rs` (new)
- `engine/src/runtime/mod.rs`
- `engine/src/state.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/persistence.rs`
- `engine/src/app/streaming.rs`

## Exit criteria

- Core cannot synthesize durability proofs.
- Tool execution/commit flow requires runtime-minted proof tokens.
- Retry policy no longer appears inline in core transition handlers.

## Validation

- `just fix`
- `just verify`

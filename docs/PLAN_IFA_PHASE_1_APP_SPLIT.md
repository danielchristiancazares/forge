# PLAN_IFA_PHASE_1_APP_SPLIT

## Purpose

Split the current `App` god object into explicit domains so ownership and mutation rights are clear.

## Drivers

- `engine/src/app/mod.rs:706-828` stores UI, core machine, and runtime boundary state together.
- `replace_with_idle` patterns are currently borrow-workaround driven.

## Target shape

```rust
pub struct App {
    core: core::Core,
    runtime: runtime::Runtime,
    ui: ui::UiState,
}
```

## Scope

- Move UI-only fields into `ui::UiState`.
- Move side-effecting runtime resources into `runtime::Runtime`.
- Move deterministic orchestration data into `core::Core`.
- Refactor app submodules to consume split borrows where possible.

## Tasks

1. Define `ui::UiState` and move view/input/modal state.
2. Define `runtime::Runtime` and move journals, tool registry, provider runtime, LSP runtime, file cache, and cleanup scheduling fields.
3. Define `core::Core` and move operation machine state, plan state, context state, turn counters, checkpoints.
4. Rewire app module methods to receive domain references instead of full `&mut App`.

## Candidate files

- `engine/src/app/mod.rs`
- `engine/src/app/streaming.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/plan.rs`
- `engine/src/app/distillation.rs`
- `engine/src/app/persistence.rs`
- `engine/src/app/commands.rs`
- `engine/src/app/lsp_integration.rs`
- `engine/src/app/input_modes.rs`
- `engine/src/ui/ui_state.rs` (new)
- `engine/src/core/core.rs` (new)
- `engine/src/runtime/runtime.rs` (new)

## Exit criteria

- No structural need for `mem::replace` take/restore around operation state.
- Domain fields have one owner each.
- App methods compile with split mutable borrows.

## Validation

- `just fix`
- `just verify`

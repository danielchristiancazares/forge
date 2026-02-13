# Plan Feature — Handoff State

## Current Progress

### Completed (all phases)
- **Phase 1** (Data model): `types/src/plan.rs` — all types, DAG validation, render(), serde, tests
- **Phase 2** (Tool interception): `tools/src/builtins.rs` schema, `engine/src/plan.rs` intercepts, all 6 subcommands
- **Phase 3** (Approval flow): `OperationState::PlanApproval`, `PlanApprovalKind`, TUI modal + keybinds, tests
- **Phase 4a** (Persistence): `save_plan()`, `load_plan_if_exists()`, `plan_path()`, autosave on tool batch commit
- **Phase 4b** (Checkpoints): `CheckpointKind::PlanStep(PlanStepId)`, checkpoint on advance/skip, tests
- **Phase 5** (Context injection): `inject_plan_context()` in `streaming.rs`, guard in `start_streaming()`, 5 tests
- **Phase 6a** (`/plan` command): `CommandKind::Plan`, parsing, `process_command` handler (status/clear/error), tests
- **Phase 6b** (Status bar): `plan_status_line()` accessor on App, TUI renders in bottom-right title
- **Phase 6c** (Transition notifications): `push_notification` in `plan_advance()` and `plan_skip()`

### Status
All phases complete. `just verify` passes (1304 tests, 0 failures). `just fix` clean.

### Implementation notes from Phase 6b+6c
- `plan_status_line()` at `engine/src/lib.rs:~1396`: returns `"Plan: {phase_name} — {step_desc}"` when active, `None` when inactive
- TUI renders plan status in `palette.primary` before context usage in the input box bottom-right title (`tui/src/lib.rs:~1304`)
- `plan_advance()` pushes `"Step N complete → Step M: desc"` or `"Plan complete! N phases, M steps."`
- `plan_skip()` pushes `"Step N skipped → Step M: desc"`
- Both use collapsed `if let ... && let ...` per project style rules

---

## Key Files Reference

| File | Role |
|------|------|
| `types/src/plan.rs` | Pure domain types (Plan, Phase, PlanStep, PlanState, etc.) |
| `tools/src/builtins.rs` | Plan tool schema registration |
| `engine/src/plan.rs` | Tool dispatch, enforcement, approval resolution, transition notifications |
| `engine/src/state.rs` | PlanApprovalKind, PlanApprovalState, OperationState::PlanApproval |
| `engine/src/lib.rs` | App struct (plan_state field), public accessors incl. plan_status_line() |
| `engine/src/persistence.rs` | save_plan(), load_plan_if_exists() |
| `engine/src/init.rs` | plan_path() |
| `engine/src/checkpoints.rs` | CheckpointKind, CheckpointStore |
| `engine/src/streaming.rs` | start_streaming() + inject_plan_context() |
| `engine/src/commands.rs` | CommandKind, Command parsing, process_command() |
| `engine/src/tests.rs` | test_app() helper, plan approval tests |
| `tui/src/lib.rs` | TUI rendering (status bar with plan indicator) |
| `tui/src/input.rs` | Keyboard input handling (plan approval keybinds) |

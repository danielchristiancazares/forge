# PLAN_IFA_PHASE_7_FOCUS_PROJECTION

## Purpose

Make focus rendering a pure projection from core state and minimal UI cursor data.

## Drivers

- Focus mode is currently encoded in multiple places:
  - `engine/src/ui/view_state.rs:50-67`
  - `tui/src/focus/mod.rs:11-22` (plan-active override bypass)
- Cached derived values can drift if invalidation is missed:
  - `engine/src/app/mod.rs:1647-1661`

## Scope

- Introduce focus projection function/module.
- Remove plan-special-casing that bypasses projected focus state.
- Reduce duplicate derived-state caching or move cache ownership to true data owner.

## Tasks

1. Add `ui::focus_projection` that derives focus mode from:
   - operation phase
   - plan state
   - view mode
2. Keep mutable UI state only for navigation cursor data.
3. Update TUI draw dispatch to consume projection output directly.
4. Revisit `cached_usage_status` strategy; avoid externally-invalidated derived cache where possible.

## Candidate files

- `engine/src/ui/focus_projection.rs` (new)
- `engine/src/ui/view_state.rs`
- `engine/src/app/mod.rs`
- `tui/src/focus/mod.rs`
- `tui/src/input.rs`

## Exit criteria

- Focus lifecycle is not separately writable from multiple sources.
- TUI focus rendering path uses projection output without plan bypass branch.
- Derived values follow single-point ownership or explicit projection.

## Validation

- `just fix`
- `just verify`

# Phase 2: Runtime + Resolve + Validate (Read-only)

**Goal**: Make configuration behavior observable before enabling edits.

## Scope

- Add `/runtime` live panel
- Add `/resolve` cascade/provenance view (global/project/profile/session)
- Add `/validate` dashboard with errors, warnings, and healthy checks
- Compute and display a deterministic config hash
- Keep all views read-only in this phase

## Why this phase comes before editing

Observability first prevents hidden state transitions and makes later editing safe to reason about.

## UI Surfaces

### `/runtime`

- Active profile and session hash
- Current mode model/provider
- Current context usage and distill threshold
- Health indicators (rate limit, last call/error)

### `/resolve`

- Winner per setting by layer
- Jump-to-setting affordance from each resolved row
- Absolute values plus provenance source

### `/validate`

- Blocking errors first
- Warnings second
- Optional healthy checks section
- Every finding includes a fix path

## Files to Modify

| File | Changes |
|------|---------|
| `engine/src/commands.rs` | Add `/runtime`, `/resolve`, `/validate` commands |
| `engine/src/lib.rs` | Expose runtime snapshot and resolution/validation read models |
| `tui/src/lib.rs` | Draw runtime, resolve, and validation views |
| `tui/src/input.rs` | Add read-only navigation for these views |

## Verification

1. `just verify` passes
2. `/runtime`, `/resolve`, and `/validate` are reachable
3. Hash is stable across no-op redraws
4. Validation findings include fix paths


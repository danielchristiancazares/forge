# Forge Dependency Cleanup

> **SUPERSEDED** — The `config → context` inversion for `atomic_write` has been
> resolved by the `forge-utils` crate (2026-02-17). `atomic_write`, `security`,
> and `diff` now live in `utils/`. The `config → tools` inversion remains and is
> tracked in `PLAN_APP_STRUCT_REFACTOR.md`.

## Context

Config sits at Layer 4 due to two wrong-direction dependencies (`config → tools`, `config → context`). Engine re-exports 5 other crates, making all boundaries invisible. `atomic_write` — a general-purpose utility — is trapped in the context crate.

## Current Graph

```
Layer 0:  types       → (nothing)
Layer 1:  lsp         → types
          providers   → types
Layer 2:  context     → types, providers
Layer 3:  tools       → types, context
Layer 4:  config      → types, lsp, TOOLS, CONTEXT   ← inverted deps
Layer 5:  engine      → types, providers, context, config, lsp, tools  ← re-exports all
Layer 6:  tui         → types, engine                 ← reaches through engine
Layer 7:  cli         → engine, tui
```

## Target Graph

```
Layer 0:  types       → (nothing)
          fs          → (tempfile, tracing only)
Layer 1:  lsp         → types
          providers   → types
          config      → types, lsp, fs
Layer 2:  context     → types, providers, fs
Layer 3:  tools       → types, context, config, fs
Layer 5:  engine      → ALL above (no re-exports)
Layer 6:  tui         → types, context, engine
Layer 7:  cli         → engine, tui, config
```

---

## Step 1: Create `forge-fs` crate

New crate `fs/` with package name `forge-fs`. Contains only `atomic_write` module (~170 lines).

**Create:**
- `fs/Cargo.toml` — deps: `tempfile`, `tracing` (workspace). cfg(unix): none (uses `std::os::unix` only).
- `fs/src/lib.rs` — contents of `context/src/atomic_write.rs`

**Edit:**
- `Cargo.toml` (workspace) — add `fs` to members, add `forge-fs = { path = "fs" }` to workspace deps
- `context/Cargo.toml` — add `forge-fs`, drop `tempfile`
- `context/src/lib.rs` — remove `mod atomic_write` + `pub use atomic_write::*`, add `pub use forge_fs::*` (temporary re-export for smooth transition)

**Delete:**
- `context/src/atomic_write.rs`

## Step 2: Move `ShellConfig` + `default_true` to `config`

`tools/src/config.rs` contains `ShellConfig` and `default_true()` — a legacy shim from when engine and tools were circular. Now that `forge-config` exists, these belong there.

**Edit:**
- `config/src/lib.rs` — add `ShellConfig` struct + inline `default_true` (it's `const fn() -> bool { true }`)
- `config/Cargo.toml` — drop `forge-tools` dep, drop `forge-context` dep, add `forge-fs`
- `tools/src/config.rs` — delete or gut (may keep as re-export shim temporarily, or just fix all internal imports)
- `tools/Cargo.toml` — add `forge-config` dep, add `forge-fs` dep
- `tools/src/shell.rs` — import `ShellConfig` from `forge_config`
- `tools/src/builtins.rs`, `tools/src/git.rs`, `tools/src/search.rs`, `tools/src/webfetch/types.rs` — inline `default_true` or import from config
- `tools/src/builtins.rs`, `tools/src/webfetch/cache.rs` — import atomic_write from `forge_fs`
- `engine/src/app/persistence.rs` — import atomic_write from `forge_fs`

## Step 3: Strip engine re-exports

Engine's `lib.rs` re-exports ~50 items from `forge_context`, `forge_tools`, `forge_providers`, `forge_types`, and `forge_config`. This makes all crate boundaries invisible — tui imports `Message`, `Provider`, `ContextUsageStatus` through engine.

**Edit `engine/src/lib.rs`:**
- Remove all `pub use forge_context::*` block
- Remove `pub use forge_tools as tools`
- Remove `pub use forge_providers::*` line
- Remove `pub use forge_types::*` block
- Remove `pub use forge_config::*` line
- Keep engine's own pub exports (`App`, `InputMode`, `DisplayItem`, etc.)

**Edit `tui/Cargo.toml`:**
- Add `forge-context` dep (for `ContextUsageStatus`)

**Edit tui source files** — update imports:
- `tui/src/lib.rs` — `Message`, `Provider`, `sanitize_terminal_text` → `forge_types::`; `ContextUsageStatus` → `forge_context::`
- `tui/src/shared.rs` — `Message`, `Provider` → `forge_types::`
- `tui/src/focus/executing.rs` — `PlanState` → `forge_types::`

**Edit `cli/src/main.rs`:**
- `ForgeConfig` → import from `forge_config::`
- Add `forge-config` to `cli/Cargo.toml` if not already there

**Engine-internal uses** — anything inside engine that uses `forge_context::X` or `forge_types::X` already imports those crates directly (engine has them as deps). Only the `pub use` facade in `lib.rs` changes. Internal code is unaffected.

## Step 4: V4 — `@path` expansion (documentation, not code change)

On closer inspection, `@path` expansion already routes through `sandbox.resolve_path()` (`engine/src/app/input_modes.rs:531`), which enforces deny patterns, root constraints, and unsafe character rejection. It bypasses the *approval prompt*, not the *path validation*.

This is correct behavior: the user explicitly typed `@path`, so they're the trust principal — no approval needed. The sandbox policy still applies.

**Action**: Update `@path` doc comments to explicitly note that sandbox validation is applied. No code change needed.

## Step 5: Remove context re-exports of forge-fs

After all callers have been migrated to import from `forge_fs` directly, remove the temporary re-export from `context/src/lib.rs`.

## Step 6: Update docs

- `AGENTS.md` — crate count 9→10, add `fs` to crate table, update dependency description for config
- `context/README.md` — remove atomic_write section, note it moved to `forge-fs`

---

## Verification

After each step: `just fix && just verify`

Final check: `cargo tree -e no-dev` to confirm config no longer transitively pulls context/tools/providers/SQLite/tiktoken.

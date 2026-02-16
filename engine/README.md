# Forge Engine (`forge-engine`)

Forge Engine is the orchestration layer for Forge's interactive LLM application runtime.

It owns the core state machine (`App`) that coordinates:

- Building provider-specific API requests (system prompt + tools + conversation history)
- Streaming assistant output (including optional "thinking/reasoning" channels)
- Crash-durable journaling and recovery for streaming + tool execution
- Tool planning, approval gating, and execution (via the `forge-tools` registry/executors)
- Context management (basic mode or "memory/distillation" mode)
- UI-facing state (input modes, view state, notifications, file picker)

This crate intentionally has **no rendering**. UI crates (TUI/GUI) consume the types exported under `forge_engine::ui` and drive `App` through its public methods.

## Crate layout

Current module/file structure (as implemented):

```text
engine/
  Cargo.toml
  README.md
  src/
    lib.rs

    # Small public shims / helpers
    config.rs          # re-export shim for forge-config
    environment.rs     # environment context + prompt assembly
    errors.rs          # stream error formatting
    notifications.rs   # trusted system notifications
    security.rs        # sanitization/re-exported security helpers
    session_state.rs   # persisted session.json state (draft + input history + modified files)
    state.rs           # core app operation state + proof objects
    util.rs            # small helpers

    # Architectural boundaries (scaffolding)
    core/
      mod.rs
    runtime/
      mod.rs

    # Orchestration state machine and submodules
    app/
      mod.rs
      init.rs
      commands.rs
      input_modes.rs
      streaming.rs
      tool_loop.rs
      plan.rs
      persistence.rs
      checkpoints.rs
      distillation.rs
      lsp_integration.rs

    # UI-facing types (no rendering)
    ui/
      mod.rs
      animation.rs
      display.rs
      file_picker.rs
      history.rs
      input.rs
      modal.rs
      panel.rs
      scroll.rs
      view_state.rs
```

## Public API highlights

The canonical list of public exports is `engine/src/lib.rs`. In practice, the public surface is grouped into:

### App orchestration

- `App`: main state machine (create with `App::new(SystemPrompts)`).
- `SystemPrompts`: per-provider base system prompts.
- `StreamingMessage`, `StreamEvent`, and helpers for streaming integration.
- Editor snapshots and dashboards for read-only UI surfaces (runtime/resolve/validate/editor snapshots).

Mode tokens and wrappers provide type-safe access to input-mode operations:

- `InsertToken` / `InsertMode`
- `CommandToken` / `CommandMode`
- `QueuedUserMessage`, `EnteredCommand`

### UI surface types (`forge_engine::ui`)

Exported for UI crates to render and manage interaction:

- Input: `InputMode`, `InputState`, `DraftInput`
- Display: `DisplayItem`
- View: `ViewState`, `ViewMode`, `FocusState`, `ScrollState`, `UiOptions`
- Settings modal state: `SettingsSurface`, `SettingsCategory`, `SettingsModalState`
- File picker state: `FilePickerState`, `FileEntry`, `find_match_positions`

### Config, environment, and helpers

- Config types are re-exported: `ForgeConfig`, `AppConfig` (owned by `forge-config`).
- Prompt assembly: `EnvironmentContext`, `assemble_prompt`.
- Error formatting: `format_stream_error`.
- Security helpers: `sanitize_display_text`.
- API key boundary helper: `wrap_api_key`.

### Re-exports

For convenience, `forge-engine` re-exports several types from sibling crates (for example, context and provider configuration types). Prefer importing them from their owning crate when building stable integrations.

## Dependencies

`forge-engine` directly depends on these workspace crates:

- `forge-config`
- `forge-context`
- `forge-lsp`
- `forge-providers`
- `forge-tools`
- `forge-types`

`forge-webfetch` may still be present transitively via `forge-tools`, but it is not a direct dependency of this crate.

## Feature flags

`forge-engine` does not define crate feature flags in `engine/Cargo.toml`.

Behavior is controlled by configuration (`ForgeConfig`) and environment variables.

## Build and test

From the workspace root:

- Build: `cargo build -p forge-engine`
- Test: `cargo test -p forge-engine`

This crate is a library. The application entrypoint/binary crate lives elsewhere in the workspace (not defined in `engine/`).

## Configuration

### Config file

The engine loads config via `ForgeConfig::load()` (re-exported from `forge-config`). The resolved config path is implementation-defined by `forge-config`; errors and hints typically refer to a per-user config file (commonly `~/.forge/config.toml`).

The engine reads (at minimum) these sections and keys:

- `[api_keys]` (provider keys; values may contain `${ENV_VAR}` expansions)
- `[app]` (model selection + UI options like ASCII-only/high-contrast/reduced motion + show thinking)
- `[context]` (controls memory/distillation mode)
- `[tools]` (limits + sandbox + approval policy + tool-specific limits)
- Provider-specific settings:
  - `[anthropic]` and/or `[cache]` and/or `[thinking]` (caching/thinking behavior)
  - `[openai]` (request options like reasoning effort/summary/verbosity/truncation)
  - `[google]` (Gemini cache + thinking flags)
- `[lsp]` (optional LSP integration configuration)

### Environment variables

The engine uses several environment variables:

- `FORGE_DATA_DIR`: override the data directory location.
- `FORGE_CONTEXT_INFINITY`: controls context "memory" default when `[context]` is not present.
  - Unset => default enabled
  - Set to `0|false|off|no` => disabled
- `FORGE_TOOL_JOURNAL_FLUSH_BYTES`: tuning knob for tool-argument journaling flush threshold (streaming).
- `FORGE_TOOL_JOURNAL_FLUSH_INTERVAL_MS`: tuning knob for tool-argument journaling flush interval (streaming).

Provider API keys can also be read from environment variables (the exact names are defined by the provider model in `forge-types` and are surfaced via `Provider::env_var()`).

### Data directory contents

The data directory (default: OS "local data" directory + `/forge`, or `FORGE_DATA_DIR`) stores:

- `history.json` (conversation history)
- `session.json` (draft input + input history + modified files)
- `plan.json` (plan state, when a plan is Proposed or Active)
- `stream_journal.db` (SQLite stream journal for crash recovery)
- `tool_journal.db` (SQLite tool journal for crash recovery)
- optional: `librarian.db` (memory subsystem backing store, when enabled)

If the system data directory cannot be resolved, the engine fails with an explicit error instructing the user to set `FORGE_DATA_DIR`.

## Commands

The command set is defined in `app/commands.rs` (via `command_specs()` and the parser). Commands are case-insensitive and may be entered with or without a leading `/`.

Common commands include:

- `q`, `quit`: exit
- `clear`: clear conversation state
- `cancel`: cancel streaming / tool execution / distillation
- `model [name]`: set model or open model selection UI
- `settings`, `config`: open settings modal
- `runtime`: show active runtime configuration
- `resolve`: show resolved configuration cascade
- `validate`: show validation dashboard
- `ctx`: show context usage and compaction status
- `jrnl`: show stream journal stats
- `distill`: distill older messages (when memory mode is enabled)
- `rewind <id|last|list> [code|conversation|both]`: rewind to an automatic checkpoint
- `undo`: rewind to the last turn checkpoint
- `retry`: undo last turn and restore its prompt into the input box
- `problems`, `diag`: show LSP diagnostics (when enabled)
- `plan [clear]`: show plan status or clear the active plan

## Streaming, caching, and crash recovery

### Prompt assembly and environment context

The engine supports a system prompt placeholder:

- `{environment_context}`: replaced (or appended) with an environment block derived from `EnvironmentContext`.

### Crash durability invariants

The engine journals streaming output before committing it to history, enabling:

- idempotent recovery (avoid double-appending recovered responses)
- recovery of partially completed streams
- recovery of tool batches bound to a stream "step"

### Cache allocation strategy (Claude)

When provider caching is enabled and the provider supports cache hints, the engine allocates cache slots across:

- system prompt prefix (only if large enough to be worth caching)
- tool definitions prefix (only if large enough to be worth caching)
- message breakpoints at stable cumulative token boundaries

The specific thresholds and maximum slot budget are implementation-defined, but the planner is designed to keep cache placement stable under small message growth and to avoid caching the last (still-evolving) message.

## Tools and approvals

### Tool registry

Tool executors and schemas are provided by `forge-tools`. `forge-engine` builds a `ToolRegistry` at startup and uses it to:

- validate tool schemas and arguments
- determine whether a tool is side-effecting / reads user data
- generate approval summaries
- execute tools under sandbox and resource limits

### Approval policy

Tool approval is controlled by a policy mode (`Default`, `Permissive`, `Strict`) combined with allow/deny lists. The engine may also request approval for specific high-risk calls based on executor metadata, even in permissive modes.

### Plan tool

The `Plan` tool is schema-only: it is intercepted and resolved by `forge-engine` rather than executed by a generic tool executor. Plan creation/editing can require explicit user approval before it becomes Active.

## Input expansion features

### AGENTS.md injection

At startup, the engine discovers and concatenates `AGENTS.md` instructions from:

- a global user location (e.g., a Forge user config folder)
- ancestor directories from root down to the current working directory

On the first user message, the content is prepended to the outgoing message and then consumed (so it is not repeatedly injected).

### `@path` file reference expansion

In Insert mode, the engine supports `@path` references in the user's draft:

- `@path/to/file.rs`
- `@"path with spaces/file.md"`
- `@path/with\ spaces/file.md`

Each referenced path is resolved through the sandbox policy and read subject to configured size limits. The file content is then prepended to the outgoing message in a fenced block so the model can see relevant context.

The file picker UI exists to support this feature and performs a `.gitignore`-aware scan capped to a maximum number of files to preserve responsiveness.

## Security model

The engine treats model output, tool output, file contents, and workspace-derived strings as untrusted. It sanitizes user-visible text to reduce:

- terminal escape sequence injection
- invisible prompt injection via control characters / bidi controls
- accidental secret leakage in logs or UI surfaces

Security helpers are re-used from `forge-tools` to avoid drift across UI surfaces.

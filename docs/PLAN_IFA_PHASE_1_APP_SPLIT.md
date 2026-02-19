# PLAN_IFA_PHASE_1_APP_SPLIT

## What this is

This is a mechanical implementation runbook for Phase 1.
It is written so a junior dev can execute it in order without design decisions.

## Goal

Split `App` storage into three owned domains without changing behavior:

- `ui`: rendering/input state
- `core`: deterministic orchestration state
- `runtime`: side-effecting/runtime boundary state

## Hard rules

- Do not change behavior.
- Do not change public API signatures in this phase.
- Do not delete logic paths.
- Keep all refactor changes in `engine/src/app/*` for now.
- Do not run unscoped project-wide replace for `self.<field>`; only replace inside `impl App` blocks and `app.<field>` in tests.
- Run `just fix` then `just verify` after each major step.

## Anchors this phase addresses

- `engine/src/app/mod.rs:706-828` (mixed ownership in `App`)
- `engine/src/app/mod.rs:1679-1687` (borrow workaround pressure — **prep only**; actual borrow splitting payoff requires Phase 2+ where methods take `&mut AppCore` / `&mut AppRuntime` instead of `&mut self`)

## Deliverable shape

At end of Phase 1, `App` must look like:

```rust
pub struct App {
    ui: AppUi,
    core: AppCore,
    runtime: AppRuntime,
}
```

## Step 0 - branch and baseline

```powershell
git switch -c refactor/p1-app-split
just verify
```

If baseline fails for reasons unrelated to this phase, stop and ask before continuing.

## Step 1 - add storage structs in `engine/src/app/mod.rs`

Insert the following right before `pub struct App`:

```rust
struct AppUi {
    input: InputState,
    display: DisplayLog,
    should_quit: bool,
    view: ViewState,
    settings_editor: SettingsEditorState,
    input_history: ui::InputHistory,
    last_ui_tick: Instant,
    file_picker: ui::FilePickerState,
}

struct AppCore {
    configured_model: ModelName,
    configured_tool_approval_mode: tools::ApprovalMode,
    configured_context_memory_enabled: bool,
    configured_ui_options: UiOptions,
    pending_turn_model: Option<ModelName>,
    pending_turn_tool_approval_mode: Option<tools::ApprovalMode>,
    pending_turn_context_memory_enabled: Option<bool>,
    pending_turn_ui_options: Option<UiOptions>,
    model: ModelName,
    context_manager: ContextManager,
    state: OperationState,
    memory_enabled: bool,
    output_limits: OutputLimits,
    configured_output_limits: OutputLimits,
    cache_enabled: bool,
    system_prompts: SystemPrompts,
    environment: EnvironmentContext,
    cached_usage_status: Option<ContextUsageStatus>,
    pending_user_message: Option<(MessageId, String, String)>,
    tool_definitions: Vec<ToolDefinition>,
    hidden_tools: HashSet<String>,
    tool_gate: ToolGate,
    checkpoints: checkpoints::CheckpointStore,
    tool_iterations: u32,
    session_changes: SessionChangeLog,
    turn_usage: Option<TurnUsage>,
    last_turn_usage: Option<TurnUsage>,
    notification_queue: crate::notifications::NotificationQueue,
    plan_state: PlanState,
}

struct AppRuntime {
    api_keys: HashMap<Provider, SecretString>,
    config_path: std::path::PathBuf,
    tick: usize,
    data_dir: DataDir,
    stream_journal: StreamJournal,
    provider_runtime: ProviderRuntimeState,
    tool_registry: std::sync::Arc<tools::ToolRegistry>,
    tool_settings: tools::ToolSettings,
    tool_journal: ToolJournal,
    pending_stream_cleanup: Option<forge_context::StepId>,
    pending_stream_cleanup_failures: u8,
    pending_tool_cleanup: Option<ToolBatchId>,
    pending_tool_cleanup_failures: u8,
    tool_file_cache: std::sync::Arc<tokio::sync::Mutex<tools::ToolFileCache>>,
    history_load_warning_shown: bool,
    autosave_warning_shown: bool,
    librarian: Option<std::sync::Arc<tokio::sync::Mutex<Librarian>>>,
    last_session_autosave: Instant,
    next_journal_cleanup_attempt: Instant,
    lsp_runtime: LspRuntimeState,
}
```

Now replace old `pub struct App { ... }` fields with:

```rust
pub struct App {
    ui: AppUi,
    core: AppCore,
    runtime: AppRuntime,
}
```

## Step 2 - update `build_app` in `engine/src/app/init.rs`

First, update imports at the top of `init.rs`.

Replace:

```rust
use super::{LspRuntimeState, ProviderRuntimeState, SystemPrompts};
```

with:

```rust
use super::{AppCore, AppRuntime, AppUi, LspRuntimeState, ProviderRuntimeState, SystemPrompts};
```

Then replace the `App { ... }` construction with nested construction:

```rust
App {
    ui: AppUi {
        input: InputState::default(),
        display: DisplayLog::default(),
        should_quit: false,
        view: parts.view,
        settings_editor: super::SettingsEditorState::Inactive,
        input_history: crate::ui::InputHistory::default(),
        last_ui_tick: Instant::now(),
        file_picker: crate::ui::FilePickerState::new(),
    },
    core: AppCore {
        configured_model: parts.configured_model,
        configured_tool_approval_mode: parts.configured_tool_approval_mode,
        configured_context_memory_enabled: parts.configured_context_memory_enabled,
        configured_ui_options: parts.configured_ui_options,
        pending_turn_model: None,
        pending_turn_tool_approval_mode: None,
        pending_turn_context_memory_enabled: None,
        pending_turn_ui_options: None,
        model: parts.model,
        context_manager: parts.context_manager,
        state: OperationState::Idle,
        memory_enabled: parts.memory_enabled,
        output_limits: parts.output_limits,
        configured_output_limits: parts.configured_output_limits,
        cache_enabled: parts.cache_enabled,
        system_prompts: parts.system_prompts,
        environment: parts.environment,
        cached_usage_status: None,
        pending_user_message: None,
        tool_definitions: parts.tool_definitions,
        hidden_tools: parts.hidden_tools,
        tool_gate: super::ToolGate::Enabled,
        checkpoints: super::checkpoints::CheckpointStore::default(),
        tool_iterations: 0,
        session_changes: crate::session_state::SessionChangeLog::default(),
        turn_usage: None,
        last_turn_usage: None,
        notification_queue: crate::notifications::NotificationQueue::new(),
        plan_state: crate::PlanState::Inactive,
    },
    runtime: AppRuntime {
        api_keys: parts.api_keys,
        config_path: parts.config_path,
        tick: 0,
        data_dir: parts.data_dir,
        stream_journal: parts.stream_journal,
        provider_runtime: parts.provider_runtime,
        tool_registry: parts.tool_registry,
        tool_settings: parts.tool_settings,
        tool_journal: parts.tool_journal,
        pending_stream_cleanup: None,
        pending_stream_cleanup_failures: 0,
        pending_tool_cleanup: None,
        pending_tool_cleanup_failures: 0,
        tool_file_cache: parts.tool_file_cache,
        history_load_warning_shown: false,
        autosave_warning_shown: false,
        librarian: parts.librarian,
        last_session_autosave: Instant::now(),
        next_journal_cleanup_attempt: Instant::now(),
        lsp_runtime: LspRuntimeState {
            config: parts.lsp_config,
            manager: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            snapshot: forge_lsp::DiagnosticsSnapshot::default(),
            pending_diag_check: None,
        },
    },
}
```

## Step 3 - scoped field path migration (no blind global replace)

Do **not** run blind replacements across all of `engine/src/app/*.rs`.
`mod.rs`, `tool_loop.rs`, and `input_modes.rs` include non-`App` impls that use their own fields.

### 3.1 Migrate `self.<field>` only inside `App` impl blocks

Files that contain `App` impls in this phase:

- `engine/src/app/mod.rs` (`impl App` — starts at line ~844)
- `engine/src/app/init.rs` (`impl App`)
- `engine/src/app/commands.rs` (`impl super::App`)
- `engine/src/app/distillation.rs` (`impl super::App`)
- `engine/src/app/persistence.rs` (`impl App`)
- `engine/src/app/streaming.rs` (`impl super::App`)
- `engine/src/app/tool_loop.rs` (`impl App` — starts at line ~182)
- `engine/src/app/plan.rs` (`impl App`)
- `engine/src/app/input_modes.rs` (`impl App` — starts at line ~32)
- `engine/src/app/lsp_integration.rs` (`impl App`)
- `engine/src/app/checkpoints.rs` (`impl crate::App`)

For each file above:

1. Jump to each `impl App`/`impl super::App`/`impl crate::App` block.
2. Use editor **replace-in-selection** for that block only.
3. Apply the exact whole-word replacements below.

#### Exclusion zones in multi-impl files

These files contain non-`App` impls whose `self.<field>` references must NOT be rewritten:

| File | Impl block | Fields to leave alone |
|------|------------|-----------------------|
| `mod.rs` | `impl StreamingMessage` (lines ~420–665) | `self.model`, `self.content`, `self.thinking`, etc. |
| `mod.rs` | `impl TurnUsage` (line ~118) | `self.api_calls`, `self.total`, `self.last_call` |
| `mod.rs` | `impl {Model,Tools,Context,Appearance}SettingsEditor` (lines ~236–418) | `self.draft`, `self.selected`, `self.dirty` |
| `mod.rs` | `impl SystemPrompts` (line ~664) | `self.claude`, `self.openai`, `self.gemini` |
| `input_modes.rs` | `impl InsertMode` / `impl CommandMode` (line ~56+) | Uses `self.app.<field>` — these are `App` field accesses through the borrow guard and **DO** need rewriting, but via `self.app.<field>` rules (Step 3.3), not the `self.<field>` rules above. |
| `tool_loop.rs` | `impl SpawnedTool` / `impl ToolQueue` (lines ~65–180) | `self.call`, `self.join_handle`, etc. |

None of these currently collide with the App replacement list (safe by coincidence), but the scope restriction is the real guardrail. Especially `self.model` in `impl StreamingMessage` — a blind whole-file replace on `mod.rs` would corrupt it.

### UI replacements

- `self.input` -> `self.ui.input`
- `self.display` -> `self.ui.display`
- `self.should_quit` -> `self.ui.should_quit`
- `self.view` -> `self.ui.view`
- `self.settings_editor` -> `self.ui.settings_editor`
- `self.input_history` -> `self.ui.input_history`
- `self.last_ui_tick` -> `self.ui.last_ui_tick`
- `self.file_picker` -> `self.ui.file_picker`

### Core replacements

- `self.configured_model` -> `self.core.configured_model`
- `self.configured_tool_approval_mode` -> `self.core.configured_tool_approval_mode`
- `self.configured_context_memory_enabled` -> `self.core.configured_context_memory_enabled`
- `self.configured_ui_options` -> `self.core.configured_ui_options`
- `self.pending_turn_model` -> `self.core.pending_turn_model`
- `self.pending_turn_tool_approval_mode` -> `self.core.pending_turn_tool_approval_mode`
- `self.pending_turn_context_memory_enabled` -> `self.core.pending_turn_context_memory_enabled`
- `self.pending_turn_ui_options` -> `self.core.pending_turn_ui_options`
- `self.model` -> `self.core.model`
- `self.context_manager` -> `self.core.context_manager`
- `self.state` -> `self.core.state`
- `self.memory_enabled` -> `self.core.memory_enabled`
- `self.output_limits` -> `self.core.output_limits`
- `self.configured_output_limits` -> `self.core.configured_output_limits`
- `self.cache_enabled` -> `self.core.cache_enabled`
- `self.system_prompts` -> `self.core.system_prompts`
- `self.environment` -> `self.core.environment`
- `self.cached_usage_status` -> `self.core.cached_usage_status`
- `self.pending_user_message` -> `self.core.pending_user_message`
- `self.tool_definitions` -> `self.core.tool_definitions`
- `self.hidden_tools` -> `self.core.hidden_tools`
- `self.tool_gate` -> `self.core.tool_gate`
- `self.checkpoints` -> `self.core.checkpoints`
- `self.tool_iterations` -> `self.core.tool_iterations`
- `self.session_changes` -> `self.core.session_changes`
- `self.turn_usage` -> `self.core.turn_usage`
- `self.last_turn_usage` -> `self.core.last_turn_usage`
- `self.notification_queue` -> `self.core.notification_queue`
- `self.plan_state` -> `self.core.plan_state`

### Runtime replacements

- `self.api_keys` -> `self.runtime.api_keys`
- `self.config_path` -> `self.runtime.config_path`
- `self.tick` -> `self.runtime.tick`
- `self.data_dir` -> `self.runtime.data_dir`
- `self.stream_journal` -> `self.runtime.stream_journal`
- `self.provider_runtime` -> `self.runtime.provider_runtime`
- `self.tool_registry` -> `self.runtime.tool_registry`
- `self.tool_settings` -> `self.runtime.tool_settings`
- `self.tool_journal` -> `self.runtime.tool_journal`
- `self.pending_stream_cleanup` -> `self.runtime.pending_stream_cleanup`
- `self.pending_stream_cleanup_failures` -> `self.runtime.pending_stream_cleanup_failures`
- `self.pending_tool_cleanup` -> `self.runtime.pending_tool_cleanup`
- `self.pending_tool_cleanup_failures` -> `self.runtime.pending_tool_cleanup_failures`
- `self.tool_file_cache` -> `self.runtime.tool_file_cache`
- `self.history_load_warning_shown` -> `self.runtime.history_load_warning_shown`
- `self.autosave_warning_shown` -> `self.runtime.autosave_warning_shown`
- `self.librarian` -> `self.runtime.librarian`
- `self.last_session_autosave` -> `self.runtime.last_session_autosave`
- `self.next_journal_cleanup_attempt` -> `self.runtime.next_journal_cleanup_attempt`
- `self.lsp_runtime` -> `self.runtime.lsp_runtime`

### 3.2 Migrate direct test field access (`app.<field>`) in `engine/src/app/tests.rs`

Apply the same mapping in `tests.rs` with `app.` receiver:

- `app.input` -> `app.ui.input`
- `app.display` -> `app.ui.display`
- `app.should_quit` -> `app.ui.should_quit`
- `app.view` -> `app.ui.view`
- `app.settings_editor` -> `app.ui.settings_editor`
- `app.input_history` -> `app.ui.input_history`
- `app.last_ui_tick` -> `app.ui.last_ui_tick`
- `app.file_picker` -> `app.ui.file_picker`
- `app.configured_model` -> `app.core.configured_model`
- `app.configured_tool_approval_mode` -> `app.core.configured_tool_approval_mode`
- `app.configured_context_memory_enabled` -> `app.core.configured_context_memory_enabled`
- `app.configured_ui_options` -> `app.core.configured_ui_options`
- `app.pending_turn_model` -> `app.core.pending_turn_model`
- `app.pending_turn_tool_approval_mode` -> `app.core.pending_turn_tool_approval_mode`
- `app.pending_turn_context_memory_enabled` -> `app.core.pending_turn_context_memory_enabled`
- `app.pending_turn_ui_options` -> `app.core.pending_turn_ui_options`
- `app.model` -> `app.core.model`
- `app.context_manager` -> `app.core.context_manager`
- `app.state` -> `app.core.state`
- `app.memory_enabled` -> `app.core.memory_enabled`
- `app.output_limits` -> `app.core.output_limits`
- `app.configured_output_limits` -> `app.core.configured_output_limits`
- `app.cache_enabled` -> `app.core.cache_enabled`
- `app.system_prompts` -> `app.core.system_prompts`
- `app.environment` -> `app.core.environment`
- `app.cached_usage_status` -> `app.core.cached_usage_status`
- `app.pending_user_message` -> `app.core.pending_user_message`
- `app.tool_definitions` -> `app.core.tool_definitions`
- `app.hidden_tools` -> `app.core.hidden_tools`
- `app.tool_gate` -> `app.core.tool_gate`
- `app.checkpoints` -> `app.core.checkpoints`
- `app.tool_iterations` -> `app.core.tool_iterations`
- `app.session_changes` -> `app.core.session_changes`
- `app.turn_usage` -> `app.core.turn_usage`
- `app.last_turn_usage` -> `app.core.last_turn_usage`
- `app.notification_queue` -> `app.core.notification_queue`
- `app.plan_state` -> `app.core.plan_state`
- `app.api_keys` -> `app.runtime.api_keys`
- `app.config_path` -> `app.runtime.config_path`
- `app.tick` -> `app.runtime.tick`
- `app.data_dir` -> `app.runtime.data_dir`
- `app.stream_journal` -> `app.runtime.stream_journal`
- `app.provider_runtime` -> `app.runtime.provider_runtime`
- `app.tool_registry` -> `app.runtime.tool_registry`
- `app.tool_settings` -> `app.runtime.tool_settings`
- `app.tool_journal` -> `app.runtime.tool_journal`
- `app.pending_stream_cleanup` -> `app.runtime.pending_stream_cleanup`
- `app.pending_stream_cleanup_failures` -> `app.runtime.pending_stream_cleanup_failures`
- `app.pending_tool_cleanup` -> `app.runtime.pending_tool_cleanup`
- `app.pending_tool_cleanup_failures` -> `app.runtime.pending_tool_cleanup_failures`
- `app.tool_file_cache` -> `app.runtime.tool_file_cache`
- `app.history_load_warning_shown` -> `app.runtime.history_load_warning_shown`
- `app.autosave_warning_shown` -> `app.runtime.autosave_warning_shown`
- `app.librarian` -> `app.runtime.librarian`
- `app.last_session_autosave` -> `app.runtime.last_session_autosave`
- `app.next_journal_cleanup_attempt` -> `app.runtime.next_journal_cleanup_attempt`
- `app.lsp_runtime` -> `app.runtime.lsp_runtime`

Important in `tests.rs`:

- **DO rewrite** all field access: direct assignment (`app.state = ...`), `match app.state { ... }`, `matches!(app.state, ...)`, `&app.state`, `&mut app.state`.
- **DO NOT rewrite** method calls: `app.model()`, `app.history()`, `app.is_loading()`, etc.
- The distinction: a field access has no `()` after the name; a method call does.

### 3.3 Migrate `self.app.<field>` in borrow-guard impls (`input_modes.rs`)

`InsertMode` and `CommandMode` hold `&mut App` as `self.app`.
After Step 1, `App`'s flat fields are gone — every `self.app.<field>` that moved into a sub-struct must be rewritten.

Scope: `impl InsertMode` (line ~56) and `impl CommandMode` (line ~259) in `engine/src/app/input_modes.rs`.
Do **not** touch the short `impl App` block (lines ~32–54) — that was already handled in Step 3.1.

#### UI replacements (`self.app.<field>` → `self.app.ui.<field>`)

- `self.app.input` -> `self.app.ui.input`

#### Core replacements (`self.app.<field>` → `self.app.core.<field>`)

- `self.app.state` -> `self.app.core.state`
- `self.app.pending_user_message` -> `self.app.core.pending_user_message`
- `self.app.tool_iterations` -> `self.app.core.tool_iterations`
- `self.app.model` -> `self.app.core.model`
- `self.app.environment` -> `self.app.core.environment`
- `self.app.session_changes` -> `self.app.core.session_changes`

#### Runtime replacements (`self.app.<field>` → `self.app.runtime.<field>`)

- `self.app.tool_settings` -> `self.app.runtime.tool_settings`
- `self.app.tool_file_cache` -> `self.app.runtime.tool_file_cache`
- `self.app.provider_runtime` -> `self.app.runtime.provider_runtime`

#### DO NOT rewrite (method calls, not field access)

These are method calls on `App` (note the `()` or the fact they chain through a method):

- `self.app.draft_text()`, `self.app.push_notification(...)`, `self.app.provider()`,
  `self.app.check_crash_recovery()`, `self.app.current_api_key()`,
  `self.app.record_prompt(...)`, `self.app.create_turn_checkpoint()`,
  `self.app.push_history_message(...)`, `self.app.autosave_history()`,
  `self.app.rollback_pending_user_message()`, `self.app.scroll_to_bottom()`,
  `self.app.apply_pending_turn_settings()`, `self.app.openai_options_for_model(...)`,
  `self.app.command_text()`, etc.

The distinction is the same as in Step 3.2: field access has no `()` after the name.

### 3.4 Update Phase 0 guardrail tests

The Phase 0 guardrail tests in `tests.rs` use runtime-constructed needles to scan source files.
After Step 3, the needles must track the new field paths.

| Test | Old needle | New needle |
|------|-----------|------------|
| `self_state_assignment_only_in_authorized_locations` | `["self", ".state", " ="]` | `["self", ".core.state", " ="]` |
| `replace_with_idle_usage_baseline` | `["replace", "_with_idle("]` | No change needed (function name unchanged) |

The `self_state_assignment_only_in_authorized_locations` test asserts `mod.rs` has exactly 3 `self.core.state =` sites
(same 3 authorized locations: `op_transition`, `op_transition_from`, `op_restore`).
The `replace_with_idle` baseline counts are unchanged.

## Step 4 - compile and fix fallout

Run:

```powershell
just fix
just verify
```

Common fallout to fix:

- Missing imports in `init.rs` for `AppUi`, `AppCore`, `AppRuntime`.
- Pattern matches that still reference old root fields.
- Constructor/init ordering mistakes.
- Phase 0 guardrail test needle not updated (Step 3.3 missed) — `self_state_assignment_only_in_authorized_locations` will fail with count 0 in mod.rs if the needle still searches for `self.state =` instead of `self.core.state =`.

## Step 5 - structural checks

Run quick searches:

```powershell
rg -n -P "self\.(configured_model|configured_tool_approval_mode|configured_context_memory_enabled|configured_ui_options|pending_turn_model|pending_turn_tool_approval_mode|pending_turn_context_memory_enabled|pending_turn_ui_options|api_keys|config_path|tick|data_dir|context_manager|stream_journal|memory_enabled|output_limits|configured_output_limits|cache_enabled|provider_runtime|system_prompts|environment|cached_usage_status|pending_user_message|tool_definitions|hidden_tools|tool_registry|tool_settings|tool_journal|tool_gate|pending_stream_cleanup|pending_stream_cleanup_failures|pending_tool_cleanup|pending_tool_cleanup_failures|tool_file_cache|checkpoints|tool_iterations|history_load_warning_shown|autosave_warning_shown|librarian|input_history|last_ui_tick|last_session_autosave|next_journal_cleanup_attempt|session_changes|file_picker|turn_usage|last_turn_usage|notification_queue|lsp_runtime|plan_state)\b(?!\s*\()" engine/src/app
rg -n -P "app\.(input|display|should_quit|view|settings_editor|input_history|last_ui_tick|file_picker|configured_model|configured_tool_approval_mode|configured_context_memory_enabled|configured_ui_options|pending_turn_model|pending_turn_tool_approval_mode|pending_turn_context_memory_enabled|pending_turn_ui_options|context_manager|memory_enabled|output_limits|configured_output_limits|cache_enabled|system_prompts|environment|cached_usage_status|pending_user_message|tool_definitions|hidden_tools|tool_gate|checkpoints|tool_iterations|session_changes|turn_usage|last_turn_usage|notification_queue|plan_state|api_keys|config_path|tick|data_dir|stream_journal|provider_runtime|tool_registry|tool_settings|tool_journal|pending_stream_cleanup|pending_stream_cleanup_failures|pending_tool_cleanup|pending_tool_cleanup_failures|tool_file_cache|history_load_warning_shown|autosave_warning_shown|librarian|last_session_autosave|next_journal_cleanup_attempt|lsp_runtime|state|model)\b(?!\s*\()" engine/src/app/tests.rs
rg -n -P "self\.app\.(input|state|model|pending_user_message|tool_iterations|environment|session_changes|tool_settings|tool_file_cache|provider_runtime)\b" engine/src/app/input_modes.rs
rg -n -P "self\.model\b(?!\s*\()" engine/src/app/mod.rs
rg "pub struct App \{" engine/src/app/mod.rs
```

Expected:

- First command: zero matches inside `impl App` blocks. Matches inside non-App impls are expected (e.g., `self.model` in `impl StreamingMessage`, `self.draft` in settings editors). If unsure, verify manually that every match is inside an exclusion-zone impl.
- Second command should return zero matches.
- Third command should return zero matches — all `self.app.<field>` accesses in borrow-guard impls must have been rewritten in Step 3.4.
- Fourth command should only match inside `impl StreamingMessage` (never inside `impl App`).
- `App` should only have `ui`, `core`, `runtime` fields.

## Step 6 - commit

```powershell
git add engine/src/app/mod.rs engine/src/app/init.rs engine/src/app/*.rs
git commit -m "refactor(engine): split app storage into ui core runtime"
```

## Out of scope for Phase 1

- Moving `AppUi`/`AppCore`/`AppRuntime` into `engine/src/ui`, `engine/src/core`, `engine/src/runtime`.
- Changing transition authority (`op_transition`, `op_edge`) semantics.
- Removing `replace_with_idle` call patterns.
- Capability token rollout.
- **Borrow splitting payoff**: Phase 1 groups fields into sub-structs, but all methods still take `&mut self` (where `self: App`). The borrow checker still sees one `&mut App` — simultaneous `&mut self.core` + `&mut self.runtime` borrows require methods to accept split references (e.g., `fn foo(core: &mut AppCore, rt: &mut AppRuntime)`). That's Phase 2+.

Those are Phase 2+.

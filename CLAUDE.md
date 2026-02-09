# CLAUDE.md

This file provides guidance for Claude Code (claude.ai/code) when working with code in this repository.
Adapt your Bash commands to use pwsh.exe as you are running in a powershell 7 environment.

## Validation Workflow

After making changes, always run:

```bash
just verify              # Runs fmt, clippy, and all tests
```

This ensures code quality before committing. The `just verify` command runs:
- `cargo fmt` - Format code
- `cargo clippy --workspace --all-targets -- -D warnings` - Lint with zero warnings
- `cargo test` - Run all tests

Individual commands (for debugging):
```bash
cargo check              # Fast type-check
cargo build              # Debug build
cargo test               # Run specific tests
cargo cov                # Coverage report (requires cargo-llvm-cov, coverage should never go down)
```

## Configuration

Config: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
model = "claude-opus-4-6"  # Provider inferred from model prefix
show_thinking = false      # Render provider thinking/reasoning in UI
ascii_only = false         # ASCII-only glyphs for icons/spinners
high_contrast = false      # High-contrast color palette
reduced_motion = false     # Disable modal animations

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"

[context]
memory = true              # Enable memory (librarian fact extraction/retrieval)

[anthropic]
cache_enabled = true
thinking_enabled = false

[google]
thinking_enabled = true    # Uses thinkingLevel="high" for Gemini 3 Pro
```

Models: `claude-*` → Claude, `gpt-*` → OpenAI, `gemini-*` → Gemini

Env fallbacks: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `FORGE_CONTEXT_INFINITY=0`

## Architecture

Forge is a vim-modal TUI for LLMs built with ratatui/crossterm.

### Workspace Structure

```
forge/
├── Cargo.toml      # Workspace root (pure workspace, no [package])
├── cli/            # Binary entry point + terminal session management
├── types/          # Core domain types (no IO, no async)
├── providers/      # LLM API clients (Claude, OpenAI, Gemini)
├── context/        # Context window management + SQLite persistence
├── engine/         # App state machine, commands, orchestration
├── tui/            # TUI rendering (ratatui) + input handling
├── webfetch/       # URL fetching, HTML-to-Markdown, chunking for LLM
├── tests/          # Integration tests
└── docs/           # Architecture documentation
```

### Key Files

| Crate | File | Purpose |
|-------|------|---------|
| `cli` | `main.rs` | Entry point, terminal session, event loop |
| `engine` | `lib.rs` | App state machine, orchestration |
| `engine` | `commands.rs` | Slash command parsing (`Command`) and dispatch |
| `engine` | `config.rs` | Config parsing (`ForgeConfig`) |
| `engine` | `tool_loop.rs` | Tool executor orchestration, approval flow |
| `engine` | `state.rs` | `ToolBatch`, `ApprovalState`, operation states |
| `engine` | `streaming.rs` | Stream event handling, `StreamingMessage` |
| `engine` | `ui/input.rs` | `InputMode`, `InputState`, `DraftInput` |
| `engine` | `ui/modal.rs` | `ModalEffectKind`, modal state |
| `tui` | `lib.rs` | Full-screen rendering |
| `tui` | `input.rs` | Keyboard input handling |
| `tui` | `theme.rs` | Colors and styles |
| `tui` | `markdown.rs` | Markdown to ratatui conversion |
| `tui` | `effects.rs` | Modal animations (PopScale, SlideUp) |
| `tui` | `tool_display.rs` | Tool result rendering |
| `context` | `manager.rs` | Context orchestration, distillation triggers |
| `context` | `history.rs` | Persistent storage (`MessageId`, `DistillateId`) |
| `context` | `stream_journal.rs` | SQLite WAL for crash recovery |
| `context` | `tool_journal.rs` | Tool execution journaling |
| `context` | `working_context.rs` | Token budget allocation |
| `context` | `distillation.rs` | Distillate generation |
| `context` | `model_limits.rs` | Per-model token limits |
| `context` | `token_counter.rs` | Token counting |
| `context` | `fact_store.rs` | Fact extraction and storage |
| `context` | `librarian.rs` | Context retrieval orchestration |
| `providers` | `lib.rs` | Provider dispatch, SSE parsing, inline `claude`/`openai`/`gemini` modules |
| `types` | `lib.rs` | Message types, `NonEmptyString`, `ModelName` |
| `webfetch` | `lib.rs` | URL fetch orchestration, chunking for LLM context |

### Main Event Loop (`cli/src/main.rs`)

```
loop {
    app.tick()                    // Increment counter, poll background tasks
    tokio::task::yield_now()      // Let async tasks progress (critical!)
    app.process_stream_events()   // Apply streaming chunks to UI
    terminal.draw()               // Render frame
    handle_events()               // Process keyboard input (100ms poll timeout)
}
```

The `yield_now()` is essential because crossterm's event polling is blocking.

### Input State Machine

Mode transitions are type-safe via `InputState` enum variants:

- `Normal(DraftInput)` → navigation
- `Insert(DraftInput)` → text editing with cursor
- `Command { draft, command }` → slash commands
- `ModelSelect { draft, selected }` → model picker overlay

Mode-specific operations require proof tokens:

```rust
// Can only get InsertToken when in Insert mode
let token = app.insert_token()?;
let mode = app.insert_mode(token);
mode.enter_char('x');  // Now safe to call
```

### Type-Driven Design

The codebase enforces correctness through types (see `DESIGN.md`):

| Type | Purpose |
|------|---------|
| `NonEmptyString` | Message content guaranteed non-empty at construction |
| `NonEmptyStaticStr` | Compile-time guaranteed non-empty static strings |
| `QueuedUserMessage` | Proof that message is validated and ready to send |
| `InsertToken` / `CommandToken` | Proof of current mode for safe operations |
| `ModelName` | Provider-scoped model name preventing cross-provider mixing |
| `ActiveJournal` | RAII handle ensuring stream chunks are journaled |
| `PreparedContext` | Proof that context was prepared before API call |
| `AppState` variants | Mutually exclusive async operation states |

### Provider System (`providers/src/lib.rs`)

`Provider` enum (Claude, OpenAI, Gemini) with:

- `default_model()` → provider's default model
- `available_models()` → known model catalog (`PredefinedModel`)
- `parse_model(raw)` → validates model name, returns `ModelName`

| Provider | Default Model | Context | Output |
|----------|---------------|---------|--------|
| Claude | `claude-opus-4-6` | 1M | 128K |
| OpenAI | `gpt-5.2` | 400K | 128K |
| Gemini | `gemini-3-pro-preview` | 1M | 65K |

Adding a provider: extend `Provider` enum, implement all match arms, add module in `providers/src/`.

### Context Management (`context/`)

Adaptive context management with automatic distillation:

- `manager.rs` - orchestrates token counting, triggers distillation
- `history.rs` - persistent storage with `MessageId`/`DistillateId`
- `working_context.rs` - builds working context within token budget
- `stream_journal.rs` - SQLite WAL for crash recovery
- `distillation.rs` - Distillate generation logic
- `model_limits.rs` - per-model token limits
- `token_counter.rs` - token counting utilities

See `context/README.md` for details.

### Tool Executor Framework (`engine/src/tool_loop.rs`)

Robust tool execution with crash recovery and user approval:

**Core Types** (`engine/src/state.rs`):

| Type | Purpose |
|------|---------|
| `ToolBatch` | Unit of execution: assistant text + tool calls + results |
| `ApprovalState` | Tracks user permission decisions for dangerous operations |
| `ToolLoopPhase` | State machine: `AwaitingApproval` → `Executing` |
| `ActiveToolExecution` | Running tool with output capture and abort handle |
| `ToolRecoveryState` | Recovered batch awaiting user decision (resume/discard) |

**Crash Recovery** (`context/src/tool_journal.rs`):

- `ToolJournal` - SQLite persistence with WAL mode
- Journal-before-commit pattern: tool calls written before execution
- `RecoveredToolBatch` - reconstructs partial batches after crash
- On startup, uncommitted batches prompt user to resume or discard

**Execution Flow**:

1. LLM returns tool calls → `ToolBatch` created, journaled
2. Calls partitioned: safe (execute immediately) vs dangerous (need approval)
3. If approval needed → `AwaitingApproval` state, UI shows confirmation
4. User approves/denies → `Executing` state, tools run sequentially
5. Results collected → batch committed to journal, sent back to LLM

### Key Extension Points

| Task | Location |
|------|----------|
| Add command | `Command` enum + `App::process_command()` in `engine/src/commands.rs` |
| Add input mode | `InputMode` + `InputState` in `engine/src/ui/input.rs`, handler in `tui/src/input.rs`, UI in `tui/src/lib.rs` |
| Add provider | `Provider` enum in `types/src/lib.rs` + client module in `providers/src/` |
| Change colors | `tui/src/theme.rs` (`colors::`, `styles::`) |
| Add UI overlay | `draw_*` function in `tui/src/lib.rs` |
| Add modal animation | `ModalEffectKind` in `engine/src/ui/modal.rs`, apply in `tui/src/effects.rs` |

See `tui/README.md` Extension Guide for detailed patterns.

### UI Design Patterns

**Mode Labels**: Each input mode has a colored label in the input area's top-left border. The label uses dark text on a colored background, where the background matches that mode's chrome/border color.

| Mode | Style Function | Background | Border |
|------|----------------|------------|--------|
| Normal | `styles::mode_normal()` | `text_secondary` (tan) | `text_muted` |
| Insert | `styles::mode_insert()` | `green` | `green` |
| Command | `styles::mode_command()` | `yellow` | `yellow` |
| Model | `styles::mode_model()` | `primary` (purple) | `primary` |

All mode styles use `fg(bg_dark)` + `bg(<color>)` + `BOLD` modifier.

**Overlays**: Command palette and Model selector use centered floating overlays with animations.

## Documentation

Never add trivial comments; do not restate the obvious. Comments should only ever be added when they provide value. 

| Document | Description |
|----------|-------------|
| `engine/README.md` | Engine state machine and orchestration |
| `tui/README.md` | TUI system, rendering, input handling |
| `context/README.md` | Context management, distillation, journaling |
| `providers/README.md` | LLM API clients, SSE streaming |
| `types/README.md` | Core domain types, newtypes |
| `webfetch/README.md` | URL fetching, HTML-to-Markdown |
| `DESIGN.md` | Type-driven design patterns |
| `docs/ANTHROPIC_MESSAGES_API.md` | Claude API reference |
| `docs/OPENAI_RESPONSES_GPT52.md` | OpenAI Responses API integration |
| `docs/RUST_2024_REFERENCE.md` | Rust 2024 edition features used |

## Testing

Uses wiremock for HTTP mocking, insta for snapshots, tempfile for isolation:

```bash
cargo test test_name                    # Single test
cargo test -- --nocapture               # With stdout
cargo test --test integration_test      # Integration tests only
```

## Common Pitfalls

### Claude API Limits

- **Max 4 `cache_control` blocks**: System prompt uses 1 slot, leaving 3 for messages
- Distillates are `Message::System` and get cache hints - can exceed limit if not capped

### Platform Differences

- Use `dirs::home_dir()` for config paths, not hardcoded `~/.forge/`
- Display actual path in error messages via `config::config_path()`

### TUI Rendering

- **Scrollbar visibility**: Only render when `max_scroll > 0` (content exceeds viewport)
- **Scrollbar position**: Use `max_scroll` as content_length, not `total_lines`
- **Cache expensive computations**: `context_usage_status()` should be cached, not recomputed per frame
- **No eprintln!**: Use `tracing::warn!` to avoid corrupting TUI output

### Database Transactions

- Journal commit+prune must be atomic (single transaction)
- Only commit journal if history save succeeds
- Always discard or commit steps in error paths (prevent session brick)

### Shell Commands (Claude Code on Windows)

This repo uses PowerShell via a wrapper. Some bash patterns don't work:

- **No `2>&1` redirection**: Use PowerShell's native error handling or just run the command without redirection
- **No `cd dir && command`**: The wrapper doesn't support chained commands with directory changes. Instead, commands run from the working directory automatically, or use `--manifest-path` for cargo
- **No `Push-Location`/`Set-Location` with semicolons**: The wrapper can't parse these. Just run commands directly—they execute in the repo root by default

```bash
# Won't work:
cargo check 2>&1 | head -50
cd /path && cargo test
Push-Location /path; cargo check; Pop-Location

# Works:
cargo check
cargo test
cargo clippy --workspace --all-targets -- -D warnings
```

## Commit Style

Conventional commits: `type(scope): description`

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## Commit Workflow

After completing changes and ensuring they work:

1. **Verify**: `just verify` - Ensure all tests pass and code is formatted
2. **Stage**: `git add -A` - Stage all changes
3. **Commit**: `git commit -m "type(scope): description"` - Write a conventional commit message
4. **Push**: `git push` - Push to remote

Example:
```bash
just verify
git add -A
git commit -m "refactor(config): replace Option<bool> tristate with bool + serde default"
git push
```

## Additional Coding Guidelines

- Use String::new() over "".to_string()
- Use .map(ToString::to_string) over .map(|m| m.to_string())
- Always collapse if statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
- Always inline format! args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- When making a change that adds or changes an API, ensure that the documentation in the `docs/` folder is up to date if applicable.


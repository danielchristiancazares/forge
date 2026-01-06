# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo check              # Fast type-check (use during development)
cargo build              # Debug build
cargo test               # Run tests
cargo clippy -- -D warnings  # Lint (run before committing)
cargo cov                # Coverage report (requires cargo-llvm-cov)
```

## Configuration

Config: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
provider = "claude"        # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"               # or "inline"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"

[context]
infinity = true            # Enable summarization-based context management

[anthropic]
cache_enabled = true
thinking_enabled = false
```

Env fallbacks: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `FORGE_TUI`, `FORGE_CONTEXT_INFINITY=0`

## Architecture

Forge is a vim-modal TUI for LLMs built with ratatui/crossterm.

### Workspace Structure

```
forge/
├── Cargo.toml      # Workspace root (pure workspace, no [package])
├── cli/            # Binary entry point
├── types/          # Core domain types (no IO, no async)
├── providers/      # LLM API clients (Claude, OpenAI)
├── context/        # Context window management + SQLite
├── engine/         # State machine + orchestration
├── tui/            # TUI rendering (ratatui)
├── tests/          # Integration tests
└── docs/           # Documentation
```

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

The codebase enforces correctness through types (see `docs/DESIGN.md`):

| Type | Purpose |
|------|---------|
| `NonEmptyString` | Message content guaranteed non-empty at construction |
| `QueuedUserMessage` | Proof that message is validated and ready to send |
| `InsertToken` / `CommandToken` | Proof of current mode for safe operations |
| `ModelName` | Provider-scoped model name preventing cross-provider mixing |
| `ActiveJournal` | RAII handle ensuring stream chunks are journaled |

### Provider System (`provider.rs`)

`Provider` enum (Claude, OpenAI) with:
- `default_model()` → provider's default model
- `available_models()` → known model list
- `parse_model(raw)` → validates model name, returns `ModelName`

Adding a provider: extend `Provider` enum, implement all match arms, add streaming in `start_streaming()`.

### Context Infinity (`context/`)

Adaptive context management with automatic summarization:
- `manager.rs` - orchestrates token counting, triggers summarization
- `history.rs` - persistent storage with `MessageId`/`SummaryId`
- `stream_journal.rs` - SQLite WAL for crash recovery
- `model_limits.rs` - per-model token limits

### Key Extension Points

| Task | Location |
|------|----------|
| Add command | `App::process_command()` in `app.rs` |
| Add input mode | `InputMode` + `InputState` + handler in `input.rs` + UI in `ui.rs` |
| Add provider | `Provider` enum in `provider.rs` + streaming logic |
| Change colors | `theme.rs` (`colors::`, `styles::`) |
| Add UI overlay | `draw_*` function in `ui.rs`, call from `draw()` |

See `docs/TUI_ARCHITECTURE.md` Extension Guide for detailed patterns.

## Testing

Uses wiremock for HTTP mocking, insta for snapshots, tempfile for isolation:
```bash
cargo test test_name                    # Single test
cargo test -- --nocapture               # With stdout
cargo test --test integration_test      # Integration tests only
```

## Commit Style

Conventional commits: `type(scope): summary`

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

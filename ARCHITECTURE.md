# Forge Architecture

Forge is a Vim-inspired modal TUI for LLMs built with ratatui/crossterm.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-53 | Build Commands, Workspace Structure, Key Files table |
| 54-98 | Main Event Loop, Input State Machine, Type-Driven Design patterns |
| 99-134 | Provider System, Context Infinity, Key Extension Points, Documentation |

## Build Commands

```bash
cargo check              # Fast type-check (use during development)
cargo build              # Debug build
cargo test               # Run tests
cargo clippy -- -D warnings  # Lint (run before committing)
```

## Workspace Structure

```
forge/
├── Cargo.toml      # Workspace root (pure workspace, no [package])
├── cli/            # Binary entry point + terminal session management
├── types/          # Core domain types (no IO, no async)
├── providers/      # LLM API clients (Claude, OpenAI)
├── context/        # Context window management + SQLite persistence
├── engine/         # App state machine, commands, orchestration
├── tui/            # TUI rendering (ratatui) + input handling
├── tests/          # Integration tests
└── docs/           # Architecture documentation
```

## Key Files

| Crate | File | Purpose |
|-------|------|---------|
| `cli` | `main.rs` | Entry point, terminal session, event loop |
| `engine` | `lib.rs` | App state machine, commands, streaming logic |
| `engine` | `config.rs` | Config parsing (`ForgeConfig`) |
| `tui` | `lib.rs` | Full-screen rendering |
| `tui` | `ui_inline.rs` | Inline terminal rendering |
| `tui` | `input.rs` | Keyboard input handling |
| `tui` | `theme.rs` | Colors and styles |
| `tui` | `markdown.rs` | Markdown to ratatui conversion |
| `tui` | `effects.rs` | Modal animations (PopScale, SlideUp) |
| `context` | `manager.rs` | Context orchestration, summarization triggers |
| `context` | `history.rs` | Persistent storage (`MessageId`, `SummaryId`) |
| `context` | `stream_journal.rs` | SQLite WAL for crash recovery |
| `context` | `working_context.rs` | Token budget allocation |
| `context` | `summarization.rs` | Summary generation |
| `context` | `model_limits.rs` | Per-model token limits |
| `context` | `token_counter.rs` | Token counting |
| `providers` | `lib.rs` | Provider dispatch + SSE parsing |
| `providers` | `claude.rs` | Anthropic API client |
| `providers` | `openai.rs` | OpenAI API client |
| `types` | `lib.rs` | Message types, `NonEmptyString`, `ModelName` |

## Main Event Loop (`cli/src/main.rs`)

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

## Input State Machine

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

## Type-Driven Design

The codebase enforces correctness through types (see `docs/DESIGN.md`):

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

## Provider System (`providers/src/lib.rs`)

`Provider` enum (Claude, OpenAI) with:

- `default_model()` → provider's default model
- `available_models()` → known model list
- `parse_model(raw)` → validates model name, returns `ModelName`

Adding a provider: extend `Provider` enum, implement all match arms, add module in `providers/src/`.

## Context Infinity (`context/`)

Adaptive context management with automatic summarization:

- `manager.rs` - orchestrates token counting, triggers summarization
- `history.rs` - persistent storage with `MessageId`/`SummaryId`
- `working_context.rs` - builds working context within token budget
- `stream_journal.rs` - SQLite WAL for crash recovery
- `summarization.rs` - summary generation logic
- `model_limits.rs` - per-model token limits
- `token_counter.rs` - token counting utilities

See `context/README.md` for details.

## Key Extension Points

| Task | Location |
|------|----------|
| Add command | `App::process_command()` in `engine/src/lib.rs` |
| Add input mode | `InputMode` + `InputState` in `engine/src/lib.rs`, handler in `tui/src/input.rs`, UI in `tui/src/lib.rs` |
| Add provider | `Provider` enum in `types/src/lib.rs` + client module in `providers/src/` |
| Change colors | `tui/src/theme.rs` (`colors::`, `styles::`) |
| Add UI overlay | `draw_*` function in `tui/src/lib.rs` |
| Add modal animation | `ModalEffect` in `engine/src/lib.rs`, apply in `tui/src/effects.rs` |

See `tui/README.md` Extension Guide for detailed patterns.

## Documentation

| Document | Description |
|----------|-------------|
| `tui/README.md` | Comprehensive TUI system documentation |
| `engine/README.md` | Engine state machine and orchestration |
| `providers/README.md` | LLM API clients and SSE streaming |
| `context/README.md` | Context management system overview |
| `docs/DESIGN.md` | Type-driven design patterns |
| `docs/OPENAI_RESPONSES_GPT52.md` | OpenAI Responses API integration |

## Configuration

Config: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
provider = "claude"        # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"               # or "inline"
ascii_only = false         # ASCII-only glyphs for icons/spinners
high_contrast = false      # High-contrast color palette
reduced_motion = false     # Disable modal animations

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

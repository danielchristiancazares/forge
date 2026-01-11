# Repository Guidelines

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-74 | Project Structure and Configuration: module organization, build commands, config.toml |
| 75-128 | Type-Driven Design Patterns: proof tokens, validated newtypes, mode wrappers |
| 129-200 | State Machines: InputState, AppState async operations, transitions |
| 201-269 | Extension Points, Key Files, TUI/Streaming Patterns |

## Project Structure & Module Organization

- `Cargo.toml` / `Cargo.lock`: workspace metadata and locked dependencies.
- `cli/`: binary entrypoint + assets
  - `src/main.rs`: terminal setup + async main loop
  - `src/assets.rs`: bundled prompt/assets
  - `assets/`: prompt templates
- `engine/`: core application state + command handling
  - `src/lib.rs`: `App`, input state machine, commands, model selection, state machine
  - `src/config.rs`: config parsing + env expansion
- `tui/`: TUI rendering + input handling
  - `src/lib.rs`: full-screen rendering + overlays (command palette, model picker)
  - `src/ui_inline.rs`: inline mode rendering
  - `src/input.rs`: crossterm key handling
  - `src/theme.rs`: Claude-inspired colors/styles
  - `src/markdown.rs`: markdown rendering
  - `src/effects.rs`: lightweight modal effects
- `context/`: ContextInfinity (history, summarization, journals)
  - `src/manager.rs`: orchestration
  - `src/history.rs`: persistent storage
  - `src/summarization.rs`: summarizer pipeline
  - `src/stream_journal.rs`: SQLite WAL for streams
  - `src/model_limits.rs`: per-model limits
  - `src/token_counter.rs`: token counting
  - `src/working_context.rs`: active context window
- `providers/`: provider HTTP/SSE implementations
  - `src/lib.rs`: Claude/OpenAI clients + streaming
- `types/`: shared domain types
  - `src/lib.rs`: Provider/ModelName/error types
- `tests/`: integration suites + snapshot fixtures
- `docs/`: architecture/spec docs (TUI, ContextInfinity)
- `scripts/`: tooling (coverage, etc.)

## Build, Test, and Development Commands

- `cargo check`: fast compile/type-check during development.
- `cargo build`: debug build.
- `cargo test`: run tests.
- `cargo clippy -- -D warnings`: lint and fail on warnings.
- `cargo cov`: coverage via cargo-llvm-cov.

## Configuration

Config file: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
provider = "claude"                    # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"                           # or "inline"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"

[context]
infinity = true                        # Enable summarization

[anthropic]
cache_enabled = true
thinking_enabled = false
thinking_budget_tokens = 10000

[openai]
reasoning_effort = "high"              # low/medium/high
verbosity = "high"                     # concise/detailed/high
truncation = "auto"                    # auto/disabled
```

Env fallbacks: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `FORGE_TUI`, `FORGE_CONTEXT_INFINITY=0`

**Constraint:** OpenAI models MUST start with `gpt-5` (enforced by `ModelName::new()`).

---

## Type-Driven Design Patterns

The codebase uses Rust's type system to enforce correctness at compile time. Understanding these patterns is essential for modifications.

### Proof Tokens

Zero-sized types that prove preconditions are met. Cannot be constructed arbitrarily.

```rust
// Private unit field prevents external construction
pub(crate) struct InsertToken(());
pub(crate) struct CommandToken(());

// Only returns Some when actually in that mode
fn insert_token(&self) -> Option<InsertToken>
fn command_token(&self) -> Option<CommandToken>

// Requires token to access mode-specific operations
fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_>
fn command_mode(&mut self, _token: CommandToken) -> CommandMode<'_>
```

**Usage pattern:**

```rust
let Some(token) = app.insert_token() else { return; };
let mut mode = app.insert_mode(token);
mode.enter_char('x');  // Now safe - proven to be in insert mode
```

### Validated Newtypes

| Type | Guarantee | Construction |
|------|-----------|--------------|
| `NonEmptyString` | Content is non-empty (whitespace-only rejected) | `NonEmptyString::new(s)?` |
| `NonEmptyStaticStr` | Compile-time non-empty check | `NonEmptyStaticStr::new("...")` (panics at compile time if empty) |
| `ModelName` | Valid model for provider | `ModelName::new(provider, name)?` or `ModelName::known(provider, name)` |
| `QueuedUserMessage` | Message validated, API configured, ready to send | Only from `InsertMode::queue_message()` |
| `EnteredCommand` | Command entered in command mode | Only from `CommandMode::take_command()` |
| `PreparedContext` | Context prepared for API call | Only from `ContextManager::prepare()` |
| `ActiveJournal` | Stream journaling session active | Only from `StreamJournal::begin_session()` |

### Mode Wrapper Types

Proxy types providing controlled access to mode-specific operations:

```rust
pub(crate) struct InsertMode<'a> { app: &'a mut App }
pub(crate) struct CommandMode<'a> { app: &'a mut App }
```

These borrow `App` mutably, expose only valid operations for that mode, and are only constructible with the corresponding token.

---

## State Machines

### Input State Machine (`engine/src/lib.rs`)

```rust
enum InputState {
    Normal(DraftInput),                              // Navigation mode
    Insert(DraftInput),                              // Text editing
    Command { draft: DraftInput, command: String },  // Slash commands
    ModelSelect { draft: DraftInput, selected: usize }, // Model picker
}
```

**Transitions:**

- `Normal` → `Insert`: `i`, `a`, `o` keys
- `Insert` → `Normal`: `Esc`, or `Enter` (sends message)
- `Normal` → `Command`: `:` or `/` keys
- `Command` → `Normal`: `Esc`, or `Enter` (executes command)
- `Normal` → `ModelSelect`: `/model` command without args
- `ModelSelect` → `Normal`: `Esc` or `Enter`

**Key invariant:** `DraftInput` (message being composed) persists across mode transitions.

### Async Operation State Machine (`engine/src/lib.rs`)

```rust
enum AppState {
    Enabled(EnabledState),   // ContextInfinity enabled
    Disabled(DisabledState), // ContextInfinity disabled
}

enum EnabledState {
    Idle,
    Streaming(ActiveStream),
    Summarizing(SummarizationState),
    SummarizingWithQueued(SummarizationWithQueuedState),
    SummarizationRetry(SummarizationRetryState),
    SummarizationRetryWithQueued(SummarizationRetryWithQueuedState),
}

enum DisabledState {
    Idle,
    Streaming(ActiveStream),
}
```

**Key invariant:** Cannot be streaming and summarizing simultaneously.

### Context Usage Status (`context/src/manager.rs`)

```rust
pub enum ContextUsageStatus {
    Ready(ContextUsage),                         // Normal operation
    NeedsSummarization { usage, needed },        // Context full
    RecentMessagesTooLarge {                     // Cannot fit even with summarization
        usage, required_tokens, budget_tokens
    },
}
```

---

## Streaming Pipeline

The streaming system uses a **journal-before-display** pattern for crash recovery:

```
queue_message() → start_streaming() → process_stream_events() → finish_streaming()
                         │                      │
                         │                      ├─ For each event:
                         │                      │  1. Persist to journal
                         │                      │  2. Apply to StreamingMessage
                         │                      │  3. UI renders
                         │                      │
                         └─ Creates ActiveStream with:
                            - StreamingMessage (accumulates content)
                            - ActiveJournal (RAII handle)
                            - AbortHandle (cancellation)
```

**Critical:** Events are persisted to SQLite journal BEFORE updating the UI. On crash, `StreamJournal::recover()` restores partial content.

### Stream Events

```rust
pub enum StreamEvent {
    TextDelta(String),      // Content chunk - persisted
    ThinkingDelta(String),  // Reasoning content - NOT persisted, not displayed
    Done,                   // Stream completed - persisted
    Error(String),          // Stream failed - persisted
}
```

---

## Key Implementation Locations

| Task | Location |
|------|----------|
| Add slash command | `App::process_command()` in `engine/src/lib.rs` |
| Add input mode | `InputState` enum + `InputMode` enum in `engine/src/lib.rs`, handler in `tui/src/input.rs` |
| Add key binding | `handle_*_mode()` functions in `tui/src/input.rs` |
| Add UI overlay | `draw_*` function in `tui/src/lib.rs`, call from `draw()` |
| Add provider | `Provider` enum in `types/src/lib.rs`, client in `providers/src/` |
| Change colors | `colors::` module in `tui/src/theme.rs` |
| Change styles | `styles::` module in `tui/src/theme.rs` |
| Add modal animation | `ModalEffect` in `engine/src/lib.rs`, apply via `tui/src/effects.rs` |
| Modify token limits | `model_limits.rs` in `context/src/` |
| Modify summarization | `summarization.rs` in `context/src/` |

---

## Common Patterns

### Sending a Message (from Insert Mode)

```rust
// 1. Acquire proof token
let Some(token) = app.insert_token() else { return };

// 2. Get mode wrapper and queue message (validates + creates QueuedUserMessage)
let queued = app.insert_mode(token).queue_message();

// 3. Start streaming (consumes QueuedUserMessage)
if let Some(queued) = queued {
    app.start_streaming(queued);
}
```

### Processing Commands

```rust
// 1. Acquire proof token
let Some(token) = app.command_token() else { return };

// 2. Get mode wrapper and extract command
let command = app.command_mode(token).take_command();

// 3. Process (returns to Normal mode internally)
if let Some(cmd) = command {
    app.process_command(cmd);
}
```

### Adding a New Command

In `App::process_command()` (`engine/src/lib.rs`):

```rust
match parts.first().copied() {
    // ... existing commands
    Some("mycommand" | "mc") => {
        if let Some(arg) = parts.get(1) {
            // Handle with argument
        } else {
            // Handle without argument
        }
    }
}
```

Update help text in the same function.

---

## Constraints & Invariants

1. **Mode operations require tokens:** Never call `insert_mode()` without first acquiring `insert_token()`.

2. **Messages are never empty:** All message content uses `NonEmptyString`. Empty/whitespace-only content is rejected at construction.

3. **Journal before display:** Stream events must be persisted before updating UI state.

4. **One streaming operation at a time:** `AppState` enforces mutual exclusion between streaming and summarization.

5. **OpenAI models must be GPT-5+:** `ModelName::new()` rejects models not starting with `gpt-5`.

6. **TUI parity:** Features must work in both full-screen (`tui/src/lib.rs`) and inline (`tui/src/ui_inline.rs`) modes.

7. **Proof types are consumed:** `QueuedUserMessage`, `EnteredCommand` are consumed by their processing functions, preventing double-use.

---

## Caching Strategy

For Anthropic API, messages are wrapped in `CacheableMessage`:

```rust
pub enum CacheHint {
    None,
    Ephemeral,  // cache_control: { type: "ephemeral" }
}
```

**Policy:** Cache all messages except the last 4 (most recent are likely to change).

---

## Error Handling

API errors are sanitized before display:

- `sanitize_stream_error()` - trims and redacts
- `redact_api_keys()` - replaces key-like strings with `[REDACTED]`

Error messages include provider and model context via `format_stream_error()`.

---

## Testing

- Integration tests: `tests/` directory
- Unit tests: `#[cfg(test)]` modules alongside code
- HTTP mocking: `wiremock`
- Snapshots: `insta`
- Temp files: `tempfile`

```bash
cargo test test_name           # Single test
cargo test -- --nocapture      # With stdout
```

---

## Commit Style

Conventional commits: `type(scope): summary`

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

Example: `feat(tui): add model selector animation`

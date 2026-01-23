# Repository Guidelines

## Project Structure & Module Organization

- `Cargo.toml` / `Cargo.lock`: workspace metadata and locked dependencies.
- `cli/`: binary entrypoint + assets
  - `src/main.rs`: terminal setup + async main loop
  - `src/assets.rs`: bundled prompt/assets
  - `assets/`: prompt templates
- `engine/`: core application state + command handling
  - `src/lib.rs`: `App` orchestration + streaming message handling, re-exports UI state
  - `src/state.rs`: `OperationState` + tool/summarization state
  - `src/commands.rs`: slash command parsing + dispatch
  - `src/tool_loop.rs`: tool execution loop + approvals/recovery
  - `src/ui/`: input state machine + view state
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
- `cargo clippy --workspace --all-targets -- -D warnings`: lint and fail on warnings.
- `cargo cov`: coverage via cargo-llvm-cov.

## Formatting, Linting, and Testing Workflow

- Run `just fmt` automatically after making Rust code changes; no approval needed.
- Before finalizing a change, run clippy scoped to the affected crate: `cargo clippy -p <crate> -- -D warnings` from the repo root. Prefer `-p` to avoid workspace-wide runs; only run `just lint` without `-p` when changes span shared crates or workspace-wide config.
- Tests:
  1. Run tests for the specific crate that changed (e.g., `tui/` → `cargo test -p forge-tui`, `cli/` → `cargo test -p forge`).
  2. After those pass, if changes touch shared crates (`types`, `engine`, `context`, `providers`, `webfetch`) or workspace-level code, run `cargo test --all-features`.
- Interactive approvals: ask before running `just lint` (workspace-wide clippy) and before `cargo test --all-features`. Project-specific tests can run without asking. `just fmt` never needs approval.

## Additional Coding Guidelines

- Always collapse if statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
- Always inline format! args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- When making a change that adds or changes an API, ensure that the documentation in the `docs/` folder is up to date if applicable.

## Documentation Style

- Add a blank line between list items to improve readability.

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

### Input State Machine (`engine/src/ui/input.rs`)

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

### Async Operation State Machine (`engine/src/state.rs`)

```rust
pub(crate) enum OperationState {
    Idle,
    Streaming(ActiveStream),
    AwaitingToolResults(PendingToolExecution),
    ToolLoop(Box<ToolLoopState>),
    ToolRecovery(ToolRecoveryState),
    Summarizing(SummarizationState),
    SummarizingWithQueued(SummarizationWithQueuedState),
    SummarizationRetry(SummarizationRetryState),
    SummarizationRetryWithQueued(SummarizationRetryWithQueuedState),
}
```

**Note:** `ContextInfinity` enablement is tracked on `App.context_infinity` (set at init), not encoded in `OperationState`.

**Key invariant:** Only one operation is active at a time; streaming, tool execution/recovery, and summarization are mutually exclusive.

### Tool Execution State Machine (`engine/src/tool_loop.rs`, `engine/src/state.rs`)

Tool calls are streamed as `StreamEvent::ToolCallStart`/`ToolCallDelta` and accumulated in `StreamingMessage`. After the stream completes, `handle_tool_calls()` routes by tools mode:

- `ToolsMode::Disabled`: emit error results and commit immediately.
- `ToolsMode::ParseOnly`: enter `OperationState::AwaitingToolResults(PendingToolExecution)` and wait for `/tool ...` results via `submit_tool_result()`.
- `ToolsMode::Enabled`: enter `OperationState::ToolLoop(Box<ToolLoopState>)`:
  - `ToolLoopPhase::AwaitingApproval(ApprovalState)` when approvals are required.
  - `ToolLoopPhase::Executing(ActiveToolExecution)` while tools run.

On completion, `commit_tool_batch()` records `tool_use` + `tool_result` messages and returns to `Idle`. Crash recovery can place the app in `OperationState::ToolRecovery`, prompting resume/discard.

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
                         │                      │  1. Persist to stream journal (text/done/error) or tool journal (tool call start/args)
                         │                      │  2. Apply to StreamingMessage (text + tool call accumulation)
                         │                      │  3. UI renders
                         │                      │
                         └─ Creates ActiveStream with:
                            - StreamingMessage (accumulates content)
                            - ActiveJournal (RAII handle)
                            - AbortHandle (cancellation)
```

**Critical:** Text/done/error events persist to `StreamJournal`, and tool call start/args persist to `ToolJournal`, BEFORE updating the UI. On crash, `StreamJournal::recover()` restores partial content; tool batches can be recovered from `ToolJournal`.

### Stream Events

```rust
pub enum StreamEvent {
    TextDelta(String),      // Content chunk - persisted
    ThinkingDelta(String),  // Reasoning content - NOT persisted, not displayed
    ToolCallStart { id: String, name: String, thought_signature: Option<String> }, // Tool call started (recorded to tool journal)
    ToolCallDelta { id: String, arguments: String }, // Tool call args chunk (recorded to tool journal)
    Done,                   // Stream completed - persisted
    Error(String),          // Stream failed - persisted
}
```

---

## Key Implementation Locations

| Task | Location |
|------|----------|
| Add slash command | `Command::parse` + `App::process_command()` in `engine/src/commands.rs` |
| Add input mode | `InputState` + `InputMode` in `engine/src/ui/input.rs`, handler in `tui/src/input.rs` |
| Add key binding | `handle_*_mode()` functions in `tui/src/input.rs` |
| Add UI overlay | `draw_*` function in `tui/src/lib.rs`, call from `draw()` |
| Add provider | `Provider` enum in `types/src/lib.rs`, client in `providers/src/` |
| Change colors | `colors::` module in `tui/src/theme.rs` |
| Change styles | `styles::` module in `tui/src/theme.rs` |
| Add modal animation | `ModalEffect` in `engine/src/ui/modal.rs`, apply via `tui/src/effects.rs` |
| Tool execution loop | `engine/src/tool_loop.rs` + `engine/src/state.rs` |
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

In `engine/src/commands.rs`:

```rust
// 1) Add a new Command variant
//    Command::MyCommand(Option<&'a str>)

// 2) Parse it in Command::parse
match parts.first().copied() {
    // ... existing commands
    Some("mycommand" | "mc") => Command::MyCommand(parts.get(1).copied()),
    // ...
}

// 3) Handle it in App::process_command
match parsed {
    Command::MyCommand(arg) => {
        // Handle with/without arg
    }
    // ...
}
```

Update help text in `process_command()` when needed.

---

## Constraints & Invariants

1. **Mode operations require tokens:** Never call `insert_mode()` without first acquiring `insert_token()`.

2. **Messages are never empty:** All message content uses `NonEmptyString`. Empty/whitespace-only content is rejected at construction.

3. **Journal before display:** Stream events must be persisted before updating UI state.

4. **One operation at a time:** `OperationState` enforces mutual exclusion between streaming, tool execution/recovery, and summarization.

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

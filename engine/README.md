# forge-engine

Core state machine and orchestration for Forge - a TUI-agnostic engine that manages LLM conversation state, input modes, streaming responses, and adaptive context management.

## Overview

The `forge-engine` crate provides the foundational state machine for the Forge LLM client. It decouples application logic from terminal UI concerns, enabling the same engine to power different presentation layers (fullscreen TUI, inline mode, etc.).

Key responsibilities:

- **Input Mode State Machine**: Vim-style modal editing (Normal, Insert, Command, ModelSelect)
- **Streaming Management**: Non-blocking LLM response streaming with crash recovery
- **Context Infinity**: Adaptive context window management with automatic summarization
- **Provider Abstraction**: Unified interface for Claude and OpenAI APIs
- **History Persistence**: Conversation storage and recovery across sessions

## Architecture

### State Machine Design

The engine uses an explicit state machine pattern to enforce invariants at compile time:

```
                      ┌──────────────────────────────────────────┐
                      │            AppState                       │
                      ├────────────────────┬─────────────────────┤
                      │      Enabled       │      Disabled       │
                      │  (ContextInfinity) │   (Basic Mode)      │
                      ├────────────────────┼─────────────────────┤
                      │  - Idle            │  - Idle             │
                      │  - Streaming       │  - Streaming        │
                      │  - Summarizing     │                     │
                      │  - SummarizingWith │                     │
                      │    Queued          │                     │
                      │  - Summarization   │                     │
                      │    Retry           │                     │
                      │  - Summarization   │                     │
                      │    RetryWithQueued │                     │
                      └────────────────────┴─────────────────────┘
```

### Input Mode Transitions

Input modes use typestate patterns for compile-time safety:

```
        ┌───────────────┐
        │    Normal     │ ← Default mode
        └───────┬───────┘
                │
    ┌───────────┼───────────┬────────────────┐
    │ 'i'/'a'   │ ':'       │ <Tab>          │
    ▼           ▼           ▼                │
┌───────┐  ┌────────┐  ┌─────────────┐       │
│Insert │  │Command │  │ ModelSelect │       │
└───┬───┘  └───┬────┘  └──────┬──────┘       │
    │          │              │              │
    │ <Esc>    │ <Esc>/<CR>   │ <Esc>/<CR>   │
    └──────────┴──────────────┴──────────────┘
                      │
                      ▼
              Back to Normal
```

## Public API

### Main Types

#### `App`

The central state container. All application state flows through this struct.

```rust
use forge_engine::App;

// Create a new application instance
let mut app = App::new()?;

// Main loop operations
app.tick();                      // Advance animations and poll background tasks
app.process_stream_events();     // Apply streaming response chunks
```

#### Input Modes and Tokens

Mode operations require proof tokens to ensure type-safe state transitions:

```rust
// Check if in insert mode and get a proof token
if let Some(token) = app.insert_token() {
    let mut insert = app.insert_mode(token);
    insert.enter_char('x');
    insert.delete_char();
    
    // Submit message (consumes the InsertMode wrapper)
    if let Some(queued) = insert.queue_message() {
        app.start_streaming(queued);
    }
}

// Command mode works similarly
if let Some(token) = app.command_token() {
    let mut cmd = app.command_mode(token);
    cmd.push_char('q');
    if let Some(entered) = cmd.take_command() {
        app.process_command(entered);
    }
}
```

#### `StreamingMessage`

Represents an active streaming response. Existence of this type proves streaming is in progress.

```rust
// Access current streaming state
if let Some(streaming) = app.streaming() {
    let content = streaming.content();      // Accumulated text so far
    let provider = streaming.provider();    // Which provider is streaming
    let model = streaming.model_name();     // The model being used
}
```

#### `QueuedUserMessage`

Proof token that a validated user message is ready for transmission. Obtained from `InsertMode::queue_message()`.

#### `EnteredCommand`

Proof token that a command was entered in command mode. Obtained from `CommandMode::take_command()`.

### Mode Wrapper Types

| Type | Purpose |
|------|---------|
| `InsertToken` | Proof that app is in Insert mode |
| `CommandToken` | Proof that app is in Command mode |
| `InsertMode<'a>` | Safe wrapper for insert operations |
| `CommandMode<'a>` | Safe wrapper for command operations |

### Enums

#### `InputMode`

```rust
pub enum InputMode {
    Normal,      // Navigation mode (vim-like)
    Insert,      // Text editing mode
    Command,     // Slash command entry (e.g., :quit)
    ModelSelect, // Model picker overlay
}
```

#### `ScrollState`

```rust
pub enum ScrollState {
    AutoBottom,                      // Follow newest content
    Manual { offset_from_top: u16 }, // User-controlled position
}
```

#### `DisplayItem`

```rust
pub enum DisplayItem {
    History(MessageId),  // Reference to persisted message
    Local(Message),      // Ephemeral message (errors, badges)
}
```

#### `PredefinedModel`

```rust
pub enum PredefinedModel {
    ClaudeOpus,
    Gpt52,
}
```

### Animation Types

#### `ModalEffect`

Animation state for TUI overlay transitions:

```rust
pub enum ModalEffectKind {
    PopScale,  // Scale-in effect for model selector
    SlideUp,   // Slide animation
}

// Creating effects
let effect = ModalEffect::pop_scale(Duration::from_millis(700));
let effect = ModalEffect::slide_up(Duration::from_millis(300));

// Animation queries
let progress = effect.progress();     // 0.0 to 1.0
let finished = effect.is_finished();
let kind = effect.kind();
```

### App Methods

#### Lifecycle

| Method | Description |
|--------|-------------|
| `App::new()` | Create instance, load config, recover crashes |
| `tick()` | Advance tick counter, poll background tasks |
| `frame_elapsed()` | Get time since last frame for animations |
| `should_quit()` | Check if quit was requested |
| `request_quit()` | Signal application exit |

#### State Queries

| Method | Description |
|--------|-------------|
| `input_mode()` | Current input mode |
| `is_loading()` | Whether streaming is active |
| `is_empty()` | No messages and not streaming |
| `streaming()` | Access active `StreamingMessage` if any |
| `history()` | Full conversation history |
| `display_items()` | Items to render in message view |
| `provider()` | Current LLM provider |
| `model()` | Current model name |
| `has_api_key(provider)` | Check if API key is configured |
| `context_infinity_enabled()` | Whether adaptive context is on |
| `context_usage_status()` | Token usage statistics |

#### Mode Transitions

| Method | Description |
|--------|-------------|
| `enter_normal_mode()` | Switch to Normal mode |
| `enter_insert_mode()` | Switch to Insert mode |
| `enter_insert_mode_at_end()` | Insert mode, cursor at end |
| `enter_insert_mode_with_clear()` | Insert mode, clear draft |
| `enter_command_mode()` | Switch to Command mode |
| `enter_model_select_mode()` | Open model picker |
| `insert_token()` | Get proof token for Insert mode |
| `command_token()` | Get proof token for Command mode |
| `insert_mode(token)` | Get InsertMode wrapper |
| `command_mode(token)` | Get CommandMode wrapper |

#### Model Selection

| Method | Description |
|--------|-------------|
| `model_select_index()` | Currently selected index |
| `model_select_move_up()` | Move selection up |
| `model_select_move_down()` | Move selection down |
| `model_select_set_index(i)` | Set selection directly |
| `model_select_confirm()` | Apply selection, exit mode |

#### Streaming

| Method | Description |
|--------|-------------|
| `start_streaming(queued)` | Begin API request |
| `process_stream_events()` | Apply pending stream chunks |

#### Scrolling

| Method | Description |
|--------|-------------|
| `scroll_up()` | Scroll message view up |
| `scroll_down()` | Scroll message view down |
| `scroll_to_top()` | Jump to beginning |
| `scroll_to_bottom()` | Jump to end, enable auto-scroll |
| `scroll_offset_from_top()` | Current scroll position |
| `update_scroll_max(max)` | Update scrollable range |

#### Provider/Model Management

| Method | Description |
|--------|-------------|
| `set_provider(provider)` | Switch provider |
| `set_model(model)` | Set specific model |

#### Context Management

| Method | Description |
|--------|-------------|
| `start_summarization()` | Trigger background summarization |
| `poll_summarization()` | Check for completed summarization |
| `save_history()` | Persist conversation to disk |
| `check_crash_recovery()` | Recover interrupted streams |

#### Status

| Method | Description |
|--------|-------------|
| `status_message()` | Current status text |
| `set_status(msg)` | Set status message |
| `clear_status()` | Clear status message |

#### Animation

| Method | Description |
|--------|-------------|
| `modal_effect_mut()` | Access modal animation state |
| `clear_modal_effect()` | Remove active animation |

#### Commands

| Method | Description |
|--------|-------------|
| `process_command(cmd)` | Execute a command |
| `take_toggle_screen_mode()` | Check/clear screen toggle flag |

### InsertMode Methods

| Method | Description |
|--------|-------------|
| `enter_char(c)` | Insert character at cursor |
| `delete_char()` | Delete character before cursor |
| `delete_char_forward()` | Delete character after cursor |
| `delete_word_backwards()` | Delete word before cursor |
| `move_cursor_left()` | Move cursor left |
| `move_cursor_right()` | Move cursor right |
| `reset_cursor()` | Move cursor to start |
| `move_cursor_end()` | Move cursor to end |
| `clear_line()` | Clear entire draft |
| `queue_message()` | Validate and queue draft for sending |

### CommandMode Methods

| Method | Description |
|--------|-------------|
| `push_char(c)` | Append character to command |
| `backspace()` | Remove last character |
| `take_command()` | Consume and return entered command |

## Re-exported Types

The crate re-exports commonly needed types from its dependencies:

### From `forge-context`

| Type | Description |
|------|-------------|
| `ContextManager` | Orchestrates token counting and summarization |
| `ContextAdaptation` | Result of model switch (shrinking/expanding) |
| `ContextUsageStatus` | Token usage statistics |
| `FullHistory` | Complete message history |
| `MessageId` | Unique identifier for messages |
| `SummaryId` | Unique identifier for summaries |
| `StreamJournal` | WAL for crash recovery |
| `ActiveJournal` | RAII handle for stream journaling |
| `ModelLimits` | Token limits for a model |
| `ModelRegistry` | Model configuration database |
| `TokenCounter` | Token counting utilities |

### From `forge-providers`

| Type | Description |
|------|-------------|
| `ApiConfig` | API request configuration |

### From `forge-types`

| Type | Description |
|------|-------------|
| `Provider` | LLM provider enum (Claude, OpenAI) |
| `ModelName` | Provider-scoped model identifier |
| `Message` | User/Assistant/System message |
| `NonEmptyString` | Guaranteed non-empty string |
| `ApiKey` | Provider-specific API key |
| `StreamEvent` | Streaming response events |
| `StreamFinishReason` | How streaming ended |
| `OutputLimits` | Max tokens and thinking budget |
| `CacheableMessage` | Message with caching hints |
| `OpenAIRequestOptions` | OpenAI-specific parameters |

## Configuration

Configuration is loaded from `~/.forge/config.toml`:

```toml
[app]
provider = "claude"          # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"                 # or "inline"
max_output_tokens = 16000

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"

[context]
infinity = true              # Enable adaptive context management

[anthropic]
cache_enabled = true
thinking_enabled = false
thinking_budget_tokens = 10000

[openai]
reasoning_effort = "high"
verbosity = "high"
truncation = "auto"
```

Environment variable fallbacks: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `FORGE_CONTEXT_INFINITY`

## Commands

Built-in slash commands:

| Command | Description |
|---------|-------------|
| `:q` / `:quit` | Exit application |
| `:clear` | Clear conversation and history |
| `:cancel` | Abort active stream |
| `:model [name]` | Set model or open picker |
| `:p [name]` / `:provider [name]` | Switch provider |
| `:ctx` / `:context` | Show context usage stats |
| `:jrnl` / `:journal` | Show journal statistics |
| `:sum` / `:summarize` | Trigger summarization |
| `:screen` | Toggle fullscreen/inline mode |
| `:help` | List available commands |

## Design Patterns

### Typestate for Mode Safety

Operations that only make sense in specific modes require proof tokens:

```rust
// This pattern prevents calling insert operations in normal mode
pub fn insert_token(&self) -> Option<InsertToken>;
pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_>;
```

The `InsertToken` can only be obtained when actually in insert mode, and `InsertMode` methods are only accessible through this wrapper.

### Explicit State Transitions

State transitions are explicit method calls rather than implicit flag mutations:

```rust
// Clear, auditable state changes
app.enter_insert_mode();
app.enter_normal_mode();
app.enter_command_mode();
```

### RAII for Resource Management

Streaming sessions use RAII patterns via `ActiveJournal` to ensure journal entries are properly sealed or discarded:

```rust
let journal = stream_journal.begin_session()?;
// ... streaming operations ...
// journal.seal() or journal.discard() called on drop
```

### Non-Empty Strings

Message content uses `NonEmptyString` to enforce at the type level that messages cannot be empty:

```rust
let content = NonEmptyString::new("hello")?;
let message = Message::user(content);
```

## Error Handling

The crate handles errors gracefully:

- **API Errors**: Displayed with context-aware messages (auth hints, rate limits)
- **Crash Recovery**: Incomplete streams are recovered on next startup
- **Summarization Failures**: Retried with exponential backoff (max 5 attempts)
- **API Key Redaction**: Keys are automatically redacted from error messages

## Thread Safety

The `App` struct is not thread-safe and should be used from a single async task. Background operations (summarization, streaming) are spawned as separate Tokio tasks that communicate via channels.

## Example: Main Loop Integration

```rust
use forge_engine::{App, InputMode};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = App::new()?;
    
    loop {
        // 1. Advance application state
        app.tick();
        
        // 2. Let async tasks progress
        tokio::task::yield_now().await;
        
        // 3. Process streaming events
        app.process_stream_events();
        
        // 4. Render UI (not shown - depends on TUI framework)
        // terminal.draw(|f| draw(&app, f))?;
        
        // 5. Handle input events
        // (crossterm event polling with 100ms timeout)
        
        if app.should_quit() {
            break;
        }
    }
    
    app.save_history()?;
    Ok(())
}
```

## License

See the repository root for license information.

# Forge Engine Architecture Documentation

This document provides comprehensive documentation for the `forge-engine` crate - the core state machine and orchestration layer for the Forge LLM client. It is intended for developers who want to understand, maintain, or extend the engine functionality.

## Table of Contents

1. [Overview](#overview)
2. [Architecture Diagram](#architecture-diagram)
3. [State Machine Design](#state-machine-design)
4. [Input Mode System](#input-mode-system)
5. [Type-Driven Design Patterns](#type-driven-design-patterns)
6. [Streaming Orchestration](#streaming-orchestration)
7. [Command System](#command-system)
8. [Context Management Integration](#context-management-integration)
9. [Configuration](#configuration)
10. [Public API Reference](#public-api-reference)
11. [Extension Guide](#extension-guide)

---

## Overview

The `forge-engine` crate is the heart of the Forge application - a TUI-agnostic engine that manages LLM conversation state, input modes, streaming responses, and adaptive context management. It decouples application logic from terminal UI concerns, enabling the same engine to power different presentation layers.

### Key Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Input Mode State Machine** | Vim-style modal editing (Normal, Insert, Command, ModelSelect) |
| **Async Operation State Machine** | Mutually exclusive states for streaming, summarizing, idle |
| **Streaming Management** | Non-blocking LLM response streaming with crash recovery |
| **Context Infinity** | Adaptive context window management with automatic summarization |
| **Provider Abstraction** | Unified interface for Claude and OpenAI APIs |
| **History Persistence** | Conversation storage and recovery across sessions |

### File Structure

```
engine/
├── Cargo.toml              # Crate manifest and dependencies
├── README.md               # Public API documentation
└── src/
    ├── lib.rs              # App state machine, commands, streaming logic
    └── config.rs           # Config parsing (ForgeConfig)
```

### Dependencies

The engine depends on several workspace crates:

| Crate | Purpose |
|-------|---------|
| `forge-types` | Core domain types (Message, Provider, ModelName, etc.) |
| `forge-context` | Context window management, summarization, persistence |
| `forge-providers` | LLM API clients (Claude, OpenAI) |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              App                                         │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                       InputState                                   │  │
│  │   ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────────────┐     │  │
│  │   │  Normal  │ │  Insert  │ │ Command  │ │  ModelSelect    │     │  │
│  │   │ (Draft)  │ │ (Draft)  │ │(Draft,Cmd)│ │(Draft,Selected) │     │  │
│  │   └──────────┘ └──────────┘ └──────────┘ └─────────────────┘     │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                       AppState                                     │  │
│  │   ┌─────────────────────────┐    ┌────────────────────────────┐  │  │
│  │   │     Enabled (CI On)     │    │    Disabled (CI Off)       │  │  │
│  │   │  ┌───────────────────┐  │    │  ┌───────────────────┐    │  │  │
│  │   │  │ Idle              │  │    │  │ Idle              │    │  │  │
│  │   │  │ Streaming         │  │    │  │ Streaming         │    │  │  │
│  │   │  │ Summarizing       │  │    │  └───────────────────┘    │  │  │
│  │   │  │ SummarizingQueued │  │    └────────────────────────────┘  │  │
│  │   │  │ SumRetry          │  │                                    │  │
│  │   │  │ SumRetryQueued    │  │                                    │  │
│  │   │  └───────────────────┘  │                                    │  │
│  │   └─────────────────────────┘                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐  │
│  │  ContextManager  │  │  StreamJournal   │  │  Display & Scroll    │  │
│  │  (forge-context) │  │  (crash recovery)│  │  (Vec<DisplayItem>)  │  │
│  └──────────────────┘  └──────────────────┘  └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## State Machine Design

The engine uses explicit state machines to enforce invariants at compile time, making impossible states unrepresentable.

### AppState - Async Operation State Machine

The `AppState` enum tracks the current async operation status. It is partitioned by whether ContextInfinity is enabled:

```rust
enum AppState {
    Enabled(EnabledState),   // ContextInfinity on - supports summarization
    Disabled(DisabledState), // ContextInfinity off - basic mode only
}

enum EnabledState {
    Idle,                                    // Ready for new operations
    Streaming(ActiveStream),                 // API response in progress
    Summarizing(SummarizationState),         // Background summarization
    SummarizingWithQueued(SummarizingWithQueuedState), // Summarizing + pending request
    SummarizationRetry(SummarizationRetryState),       // Retry after failure
    SummarizationRetryWithQueued(SummarizationRetryWithQueuedState),
}

enum DisabledState {
    Idle,                    // Ready for new operations
    Streaming(ActiveStream), // API response in progress
}
```

### State Transition Diagram

```
                              AppState::Enabled
    ┌────────────────────────────────────────────────────────────────────────┐
    │                                                                         │
    │     ┌──────────────────────────────────────────────────────────────┐   │
    │     │                          Idle                                 │   │
    │     └──────────────────────────────────────────────────────────────┘   │
    │           │                    │                     │                  │
    │      start_streaming()   start_summarization()   queue_message()        │
    │           │                    │               (summarization needed)   │
    │           v                    v                     │                  │
    │     ┌───────────┐        ┌───────────────┐          │                  │
    │     │ Streaming │        │  Summarizing  │<─────────┘                  │
    │     └───────────┘        └───────────────┘                             │
    │           │                    │                                        │
    │       finish/error         success/failure                              │
    │           │                    │                                        │
    │           v                    v                                        │
    │     ┌───────────┐  success ┌─────────────────────┐  failure             │
    │     │   Idle    │<─────────│ (poll_summarization │──────────┐           │
    │     └───────────┘          │  processes result)  │          │           │
    │                            └─────────────────────┘          v           │
    │                                                    ┌─────────────────┐  │
    │                                                    │ SummarizationRetry│ │
    │                                                    └─────────────────┘  │
    │                                                             │           │
    │                                                        ready_at reached │
    │                                                             │           │
    │                                                             v           │
    │                                                    ┌─────────────────┐  │
    │                                                    │   Summarizing   │  │
    │                                                    │   (retry)       │  │
    │                                                    └─────────────────┘  │
    └────────────────────────────────────────────────────────────────────────┘
```

### Design Rationale

This state machine design provides several guarantees:

| Guarantee | Implementation |
|-----------|----------------|
| **No concurrent streaming** | Only one `Streaming` state can exist |
| **No concurrent summarization** | Summarizing/Retry states are mutually exclusive |
| **Request queueing** | `WithQueued` variants hold a pending request during summarization |
| **Clean transitions** | `replace_with_idle()` ensures proper state cleanup |

---

## Input Mode System

The engine implements a vim-style modal editing system with four distinct modes.

### InputState Enum

```rust
enum InputState {
    Normal(DraftInput),                         // Navigation mode
    Insert(DraftInput),                         // Text editing mode
    Command { draft: DraftInput, command: String }, // Slash command entry
    ModelSelect { draft: DraftInput, selected: usize }, // Model picker overlay
}
```

Each variant carries `DraftInput` (the message being composed), ensuring it persists across mode transitions.

### DraftInput - Text Buffer with Cursor

```rust
struct DraftInput {
    text: String,    // The draft message content
    cursor: usize,   // Cursor position (grapheme index, not byte index)
}
```

The cursor tracks position in grapheme clusters (not bytes), enabling correct handling of Unicode characters like emoji:

```rust
// Grapheme-aware cursor movement
fn byte_index_at(&self, grapheme_index: usize) -> usize {
    self.text
        .grapheme_indices(true)
        .nth(grapheme_index)
        .map(|(i, _)| i)
        .unwrap_or(self.text.len())
}
```

### Mode Transition Diagram

```
        ┌───────────────┐
        │    Normal     │ ← Default mode, navigation
        └───────┬───────┘
                │
    ┌───────────┼───────────┬────────────────┐
    │ 'i'/'a'/'o'           │ ':'/'/'        │ Tab
    v                       v                v
┌───────┐             ┌────────┐       ┌─────────────┐
│Insert │             │Command │       │ ModelSelect │
│       │             │        │       │             │
│Draft+ │             │Draft+  │       │Draft+       │
│Cursor │             │CmdStr  │       │SelectedIdx  │
└───┬───┘             └───┬────┘       └──────┬──────┘
    │                     │                   │
    │ Esc                 │ Esc/Enter         │ Esc/Enter
    └─────────────────────┴───────────────────┘
                          │
                          v
                   Back to Normal
```

### Mode Transition Methods

| Method | Effect |
|--------|--------|
| `enter_normal_mode()` | Transition to Normal, clear modal effect |
| `enter_insert_mode()` | Transition to Insert, preserve cursor |
| `enter_insert_mode_at_end()` | Insert mode with cursor at end |
| `enter_insert_mode_with_clear()` | Insert mode with cleared draft |
| `enter_command_mode()` | Transition to Command, start new command string |
| `enter_model_select_mode()` | Open model picker with animation |

---

## Type-Driven Design Patterns

The engine uses Rust's type system extensively to enforce correctness at compile time.

### Proof Tokens

Proof tokens are zero-sized types that serve as compile-time evidence that a precondition is met. They cannot be constructed arbitrarily.

#### InsertToken and CommandToken

```rust
/// Proof token for Insert mode operations.
#[derive(Debug)]
pub struct InsertToken(());  // Private unit field prevents external construction

/// Proof token for Command mode operations.
#[derive(Debug)]
pub struct CommandToken(());
```

Usage pattern:

```rust
// Only returns Some when actually in Insert mode
pub fn insert_token(&self) -> Option<InsertToken> {
    matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
}

// Consuming the token proves we checked the mode
pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
    InsertMode { app: self }
}
```

This pattern ensures that `InsertMode` methods can only be called when actually in insert mode:

```rust
// Safe usage - compiler enforces mode check
if let Some(token) = app.insert_token() {
    let mut insert = app.insert_mode(token);
    insert.enter_char('x');  // Only accessible through InsertMode
}
```

### Mode Wrapper Types

The `InsertMode<'a>` and `CommandMode<'a>` wrappers provide safe, mode-specific APIs:

```rust
/// Mode wrapper for safe insert operations.
pub struct InsertMode<'a> {
    app: &'a mut App,
}

impl<'a> InsertMode<'a> {
    pub fn enter_char(&mut self, c: char);
    pub fn delete_char(&mut self);
    pub fn move_cursor_left(&mut self);
    pub fn move_cursor_right(&mut self);
    pub fn queue_message(self) -> Option<QueuedUserMessage>; // Consumes self
}
```

### QueuedUserMessage - Message Validation Proof

```rust
/// Proof that a non-empty user message was queued.
///
/// The `config` captures the model/provider at queue time. If summarization runs
/// before streaming starts, the original config is preserved.
#[derive(Debug)]
pub struct QueuedUserMessage {
    config: ApiConfig,
}
```

This type proves:
1. The draft text was validated as non-empty
2. An API key is available for the current provider
3. The `ApiConfig` was successfully constructed
4. The user message was added to history

Only `InsertMode::queue_message()` can create this type, ensuring all preconditions are met.

### EnteredCommand - Command Entry Proof

```rust
/// Proof token that a command line was entered in Command mode.
#[derive(Debug)]
pub struct EnteredCommand {
    raw: String,
}
```

Only `CommandMode::take_command()` can create this type, ensuring the command was properly entered in command mode.

---

## Streaming Orchestration

The engine manages streaming LLM responses with crash recovery, journaling, and proper resource cleanup.

### ActiveStream - In-Flight Request State

```rust
struct ActiveStream {
    message: StreamingMessage,   // Accumulating response content
    journal: ActiveJournal,      // RAII handle for crash recovery
    abort_handle: AbortHandle,   // For cancellation
}
```

### StreamingMessage - Accumulating Response

```rust
/// A message being streamed - existence proves streaming is active.
pub struct StreamingMessage {
    model: ModelName,
    content: String,  // Accumulated response text
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
}
```

The `StreamingMessage` provides:
- Type-level proof that streaming is active (ownership semantics)
- Event-based content accumulation via channel
- Conversion to complete `Message` when done

### Streaming Lifecycle

```rust
// 1. Queue message (validates and adds user message to history)
let queued = insert_mode.queue_message()?;

// 2. Start streaming (begins API request, spawns task)
app.start_streaming(queued);

// 3. Process events in main loop
loop {
    app.tick();
    tokio::task::yield_now().await;
    app.process_stream_events();  // Apply chunks, handle completion
    // ... render UI ...
}
```

### Stream Event Processing

```rust
pub fn process_stream_events(&mut self) {
    loop {
        let event = match active.message.try_recv_event() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => break,  // No more events
            Err(TryRecvError::Disconnected) => StreamEvent::Error(...),
        };

        // Persist BEFORE display (crash recovery)
        active.journal.append_text(&mut self.stream_journal, text)?;

        // Apply to UI
        let finish_reason = active.message.apply_event(event);

        if let Some(reason) = finish_reason {
            self.finish_streaming(reason);
            return;
        }
    }
}
```

### Journal-Based Crash Recovery

The engine uses a write-ahead log for crash recovery:

```rust
// On startup, check for incomplete streams
pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
    let recovered = self.stream_journal.recover()?;

    // Add recovered content with warning badge
    let badge = match &recovered {
        RecoveredStream::Complete { .. } => RECOVERY_COMPLETE_BADGE,
        RecoveredStream::Incomplete { .. } => RECOVERY_INCOMPLETE_BADGE,
        RecoveredStream::Errored { .. } => RECOVERY_ERROR_BADGE,
    };

    self.push_history_message(Message::assistant(model, content));
    self.stream_journal.seal_unsealed(step_id)?;
    Some(recovered)
}
```

---

## Command System

The engine provides a slash command system for user actions.

### Built-in Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `:quit` | `:q` | Exit application |
| `:clear` | - | Clear conversation and history |
| `:cancel` | - | Abort active stream |
| `:model [name]` | - | Set model or open picker |
| `:provider [name]` | `:p` | Switch provider |
| `:context` | `:ctx` | Show context usage stats |
| `:journal` | `:jrnl` | Show journal statistics |
| `:summarize` | `:sum` | Trigger summarization |
| `:screen` | - | Toggle fullscreen/inline mode |
| `:help` | - | List available commands |

### Command Processing

```rust
pub fn process_command(&mut self, command: EnteredCommand) {
    let parts: Vec<&str> = command.raw.split_whitespace().collect();

    match parts.first().copied() {
        Some("q" | "quit") => self.request_quit(),
        Some("clear") => {
            // Abort any active operation
            // Clear display and context
            self.context_manager = ContextManager::new(self.model.as_str());
            self.set_status("Conversation cleared");
        }
        Some("model") => {
            if let Some(model_name) = parts.get(1) {
                // Parse and set model
            } else {
                self.enter_model_select_mode(); // Open TUI picker
            }
        }
        // ... other commands ...
        Some(cmd) => self.set_status(format!("Unknown command: {cmd}")),
        None => {}
    }
}
```

---

## Context Management Integration

The engine integrates with `forge-context` for adaptive context window management.

### ContextManager Orchestration

```rust
// In start_streaming():
let api_messages = match self.context_manager.prepare() {
    Ok(prepared) => prepared.api_messages(),
    Err(ContextBuildError::SummarizationNeeded(needed)) => {
        // Queue the request, start summarization
        self.start_summarization_with_attempt(Some(config), 1);
        return;
    }
    Err(ContextBuildError::RecentMessagesTooLarge { .. }) => {
        self.set_status("Recent messages exceed budget");
        return;
    }
};
```

### Summarization Retry with Backoff

```rust
const MAX_SUMMARIZATION_ATTEMPTS: u8 = 5;
const SUMMARIZATION_RETRY_BASE_MS: u64 = 500;
const SUMMARIZATION_RETRY_MAX_MS: u64 = 8000;

fn summarization_retry_delay(attempt: u8) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10) as u32;
    let base = SUMMARIZATION_RETRY_BASE_MS.saturating_mul(1u64 << exponent);
    let capped = base.min(SUMMARIZATION_RETRY_MAX_MS);
    // Add jitter to prevent thundering herd
    Duration::from_millis(capped + jitter)
}
```

### Model Switch Adaptation

```rust
fn handle_context_adaptation(&mut self) {
    let adaptation = self.context_manager.switch_model(self.model.as_str());

    match adaptation {
        ContextAdaptation::NoChange => {}
        ContextAdaptation::Shrinking { needs_summarization: true, .. } => {
            self.set_status("Context budget shrank; summarizing...");
            self.start_summarization();
        }
        ContextAdaptation::Expanding { can_restore, .. } => {
            if can_restore > 0 {
                let restored = self.context_manager.try_restore_messages();
                self.set_status(format!("Restored {} messages", restored));
            }
        }
        _ => {}
    }
}
```

---

## Configuration

The engine uses a TOML-based configuration system.

### ForgeConfig Structure

```rust
#[derive(Debug, Default, Deserialize)]
pub struct ForgeConfig {
    pub app: Option<AppConfig>,
    pub api_keys: Option<ApiKeys>,
    pub context: Option<ContextConfig>,
    pub cache: Option<CacheConfig>,       // Legacy
    pub thinking: Option<ThinkingConfig>, // Legacy
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAIConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tui: Option<String>,
    pub max_output_tokens: Option<u32>,
}
```

### Configuration Loading

```rust
impl ForgeConfig {
    pub fn load() -> Option<Self> {
        let path = dirs::home_dir()?.join(".forge").join("config.toml");
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str(&content).ok()
    }
}
```

### Environment Variable Expansion

```rust
// Config values like "${ANTHROPIC_API_KEY}" are expanded
pub fn expand_env_vars(value: &str) -> String {
    // Parses ${VAR_NAME} patterns and substitutes from environment
}
```

### Configuration Precedence

| Setting | Precedence (highest first) |
|---------|---------------------------|
| API Keys | Config file -> Environment variables |
| Provider | Config file -> Auto-detect from available keys -> Default (Claude) |
| Model | Config file -> Provider default |
| Context Infinity | Config file -> Environment variable -> Default (true) |

---

## Public API Reference

### App Lifecycle

| Method | Description |
|--------|-------------|
| `App::new(system_prompt)` | Create instance, load config, recover crashes |
| `tick()` | Advance tick counter, poll background tasks |
| `frame_elapsed()` | Get time since last frame for animations |
| `should_quit()` | Check if quit was requested |
| `request_quit()` | Signal application exit |
| `save_history()` | Persist conversation to disk |

### State Queries

| Method | Return Type | Description |
|--------|-------------|-------------|
| `input_mode()` | `InputMode` | Current input mode |
| `is_loading()` | `bool` | Whether streaming is active |
| `is_empty()` | `bool` | No messages and not streaming |
| `streaming()` | `Option<&StreamingMessage>` | Access active stream |
| `history()` | `&FullHistory` | Full conversation history |
| `display_items()` | `&[DisplayItem]` | Items to render |
| `provider()` | `Provider` | Current LLM provider |
| `model()` | `&str` | Current model name |
| `has_api_key(provider)` | `bool` | Check if API key is configured |
| `context_infinity_enabled()` | `bool` | Whether adaptive context is on |
| `context_usage_status()` | `ContextUsageStatus` | Token usage statistics |

### Mode Transitions

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

### Streaming Operations

| Method | Description |
|--------|-------------|
| `start_streaming(queued)` | Begin API request |
| `process_stream_events()` | Apply pending stream chunks |

### Model/Provider Management

| Method | Description |
|--------|-------------|
| `set_provider(provider)` | Switch provider |
| `set_model(model)` | Set specific model |
| `model_select_index()` | Currently selected index |
| `model_select_move_up()` | Move selection up |
| `model_select_move_down()` | Move selection down |
| `model_select_confirm()` | Apply selection, exit mode |

### Scrolling

| Method | Description |
|--------|-------------|
| `scroll_up()` | Scroll message view up |
| `scroll_down()` | Scroll message view down |
| `scroll_to_top()` | Jump to beginning |
| `scroll_to_bottom()` | Jump to end, enable auto-scroll |
| `scroll_offset_from_top()` | Current scroll position |
| `update_scroll_max(max)` | Update scrollable range |

---

## Extension Guide

### Adding a New Command

1. **Add command handler in `process_command()`** (`engine/src/lib.rs`):

```rust
pub fn process_command(&mut self, command: EnteredCommand) {
    let parts: Vec<&str> = command.raw.split_whitespace().collect();

    match parts.first().copied() {
        // ... existing commands ...

        Some("mycommand" | "mc") => {
            // Get optional argument
            if let Some(arg) = parts.get(1) {
                // Process with argument
                self.set_status(format!("MyCommand executed with: {arg}"));
            } else {
                // No argument - show help or use default
                self.set_status("Usage: :mycommand <arg>");
            }
        }

        Some(cmd) => self.set_status(format!("Unknown command: {cmd}")),
        None => {}
    }
}
```

2. **Update help text**:

```rust
Some("help") => {
    self.set_status(
        "Commands: /q(uit), /clear, /mycommand, ..."  // Add new command
    );
}
```

### Adding a New Input Mode

1. **Extend `InputState` enum**:

```rust
enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
    ModelSelect { draft: DraftInput, selected: usize },
    MyMode { draft: DraftInput, custom_state: MyState },  // New mode
}
```

2. **Add mode enum variant**:

```rust
pub enum InputMode {
    Normal,
    Insert,
    Command,
    ModelSelect,
    MyMode,  // New mode
}
```

3. **Add transition method**:

```rust
impl InputState {
    fn into_my_mode(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => {
                InputState::MyMode {
                    draft,
                    custom_state: MyState::default(),
                }
            }
            // Handle other variants...
        }
    }
}

impl App {
    pub fn enter_my_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_my_mode();
    }
}
```

4. **Add proof token pattern** (optional):

```rust
pub struct MyModeToken(());

impl App {
    pub fn my_mode_token(&self) -> Option<MyModeToken> {
        matches!(&self.input, InputState::MyMode { .. }).then_some(MyModeToken(()))
    }

    pub fn my_mode(&mut self, _token: MyModeToken) -> MyMode<'_> {
        MyMode { app: self }
    }
}

pub struct MyMode<'a> {
    app: &'a mut App,
}

impl<'a> MyMode<'a> {
    pub fn do_something(&mut self) {
        // Mode-specific operations
    }
}
```

5. **Handle in TUI input handler** (`tui/src/input.rs`):

```rust
pub async fn handle_events(app: &mut App) -> Result<bool> {
    match app.input_mode() {
        InputMode::Normal => handle_normal_mode(app),
        InputMode::Insert => handle_insert_mode(app),
        InputMode::Command => handle_command_mode(app),
        InputMode::ModelSelect => handle_model_select_mode(app),
        InputMode::MyMode => handle_my_mode(app),  // New handler
    }
}
```

### Adding a New Provider

1. **Extend `Provider` enum** (`types/src/lib.rs`):

```rust
pub enum Provider {
    Claude,
    OpenAI,
    MyProvider,  // New provider
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Self::Claude),
            "openai" | "gpt" => Some(Self::OpenAI),
            "myprovider" | "mp" => Some(Self::MyProvider),  // New parsing
            _ => None,
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            Self::Claude => ModelName::known(Self::Claude, "claude-sonnet-4-5-20250929"),
            Self::OpenAI => ModelName::known(Self::OpenAI, "gpt-5.2"),
            Self::MyProvider => ModelName::known(Self::MyProvider, "my-model-v1"),
        }
    }
}
```

2. **Add API client** (`providers/src/my_provider.rs`)

3. **Update config structure** (`engine/src/config.rs`):

```rust
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub my_provider: Option<String>,  // New key
}
```

4. **Update key loading in `App::new()`**

### Adding a New Async Operation State

1. **Add new state variant**:

```rust
enum EnabledState {
    Idle,
    Streaming(ActiveStream),
    Summarizing(SummarizationState),
    // ... existing states ...
    MyOperation(MyOperationState),  // New operation
}
```

2. **Add state transition guards**:

```rust
pub fn start_my_operation(&mut self) {
    match &self.state {
        AppState::Enabled(EnabledState::Idle) => {
            // Start operation
            self.state = AppState::Enabled(EnabledState::MyOperation(state));
        }
        _ => {
            self.set_status("Cannot start: busy with other operation");
        }
    }
}
```

3. **Add polling in `tick()`**:

```rust
pub fn tick(&mut self) {
    self.tick = self.tick.wrapping_add(1);
    self.poll_summarization();
    self.poll_summarization_retry();
    self.poll_my_operation();  // New polling
}
```

---

## Re-exported Types

The engine re-exports commonly needed types from its dependencies:

### From `forge-context`

| Type | Description |
|------|-------------|
| `ContextManager` | Orchestrates token counting and summarization |
| `ContextAdaptation` | Result of model switch (shrinking/expanding) |
| `ContextUsageStatus` | Token usage statistics |
| `FullHistory` | Complete message history |
| `MessageId` | Unique identifier for messages |
| `StreamJournal` | WAL for crash recovery |
| `ActiveJournal` | RAII handle for stream journaling |
| `ModelLimits` | Token limits for a model |
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

---

## Error Handling

### API Error Formatting

The engine formats API errors with context-aware messages:

```rust
fn format_stream_error(provider: Provider, model: &str, err: &str) -> StreamErrorUi {
    // Detects auth errors and provides actionable guidance
    if is_auth_error(&extracted) {
        return StreamErrorUi {
            status: format!("Auth error: set {env_var}"),
            message: format!("Fix: Set {} (env) or add to config.toml", env_var),
        };
    }

    // Generic error formatting with truncation
    StreamErrorUi {
        status: truncate_with_ellipsis(&detail, 80),
        message: format!("Request failed. Details: {}", detail),
    }
}
```

### API Key Redaction

Error messages are sanitized to prevent key leakage:

```rust
fn redact_api_keys(raw: &str) -> String {
    // Replaces "sk-..." patterns with "sk-***"
}
```

---

## Thread Safety

The `App` struct is not thread-safe and should be used from a single async task. Background operations (summarization, streaming) are spawned as separate Tokio tasks that communicate via channels:

- **Streaming**: `mpsc::unbounded_channel()` for `StreamEvent` delivery
- **Summarization**: `tokio::task::JoinHandle` polled via `is_finished()`
- **Cancellation**: `AbortHandle` for graceful task termination

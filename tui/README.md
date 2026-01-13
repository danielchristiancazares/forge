# forge-tui

This document provides comprehensive documentation for the Text User Interface (TUI) system in the Forge codebase. It is intended for developers who want to understand, maintain, or extend the TUI functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-25 | Header & TOC |
| 26-45 | Table of Contents |
| 46-91 | Overview |
| 92-127 | Architecture Diagram |
| 128-232 | Core Components |
| 233-318 | Application State Management |
| 319-403 | Input Handling System |
| 404-632 | Type-Driven Design Patterns |
| 633-847 | Rendering Pipeline |
| 848-885 | UI Modes: Full vs Inline |
| 886-1052 | Message and Streaming System |
| 1053-1115 | Context Management Integration |
| 1116-1173 | Theming and Styling |
| 1174-1257 | Key Data Structures |
| 1258-1298 | Event Loop and Update Cycle |
| 1299-1413 | Command System |
| 1414-1909 | Extension Guide |
| 1910-1943 | Appendix: Markdown Rendering |
| 1944-1994 | Summary |

## Table of Contents

1. [Overview](#overview)
2. [Architecture Diagram](#architecture-diagram)
3. [Core Components](#core-components)
4. [Application State Management](#application-state-management)
5. [Input Handling System](#input-handling-system)
6. [Type-Driven Design Patterns](#type-driven-design-patterns)
7. [Rendering Pipeline](#rendering-pipeline)
8. [UI Modes: Full vs Inline](#ui-modes-full-vs-inline)
9. [Message and Streaming System](#message-and-streaming-system)
10. [Context Management Integration](#context-management-integration)
11. [Theming and Styling](#theming-and-styling)
12. [Key Data Structures](#key-data-structures)
13. [Event Loop and Update Cycle](#event-loop-and-update-cycle)
14. [Command System](#command-system)
15. [Extension Guide](#extension-guide)

---

## Overview

Forge uses a terminal-based user interface built on the [ratatui](https://github.com/ratatui-org/ratatui) library with [crossterm](https://github.com/crossterm-rs/crossterm) as the backend. The TUI follows a vim-inspired modal editing paradigm with distinct Normal, Insert, Command, and ModelSelect modes.

### Key Characteristics

- **Modal Interface**: Vim-style modes (Normal, Insert, Command, ModelSelect)
- **Dual Display Modes**: Full-screen alternate screen or inline terminal mode
- **Async Streaming**: Real-time streaming of LLM responses with crash recovery
- **Rich Markdown Rendering**: Full markdown support including tables and code blocks
- **Context-Aware**: Integration with ContextInfinity for adaptive context management

### File Structure

```
cli/src/
└── main.rs                     # Application entry point and event loop

engine/src/
├── lib.rs                      # App state machine, commands, model select
└── config.rs                   # Config parsing

tui/src/
├── lib.rs                      # Full-screen UI rendering + overlays
├── ui_inline.rs                # Inline terminal UI rendering
├── input.rs                    # Keyboard input handling
├── theme.rs                    # Colors and styling
├── markdown.rs                 # Markdown to ratatui conversion
└── effects.rs                  # Modal animation transforms (PopScale, SlideUp)

context/src/                    # Context window management
providers/src/                  # Provider HTTP/SSE implementations
types/src/                      # Shared domain types
```

> **Note:** For readability, this document refers to logical files like
> `app.rs`, `ui.rs`, and `input.rs`. In the current workspace layout those
> map to `engine/src/lib.rs` (App/state),
> `tui/src/lib.rs`/`ui_inline.rs` (rendering), and
> `tui/src/input.rs` (key handling). ContextInfinity lives in
> `context/src/`, provider references map to
> `providers/src/lib.rs`, and shared message/types live in
> `types/src/lib.rs`.

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         main.rs                                  │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                    Event Loop                            │    │
│  │  ┌─────────┐    ┌─────────────┐    ┌─────────────────┐  │    │
│  │  │  tick() │───>│yield_now()  │───>│process_stream   │  │    │
│  │  └─────────┘    └─────────────┘    │  _events()      │  │    │
│  │                                    └────────┬────────┘  │    │
│  │                                             v            │    │
│  │  ┌─────────────────────┐         ┌─────────────────────┐│    │
│  │  │  terminal.draw()    │<────────│ handle_events()     ││    │
│  │  └─────────────────────┘         └─────────────────────┘│    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          v                   v                   v
    ┌──────────┐        ┌──────────┐        ┌──────────┐
    │  app.rs  │        │  ui.rs   │        │ input.rs │
    │          │        │          │        │          │
    │ - State  │<──────>│ - Render │        │ - Events │
    │ - Logic  │        │ - Layout │        │ - Modes  │
    └──────────┘        └──────────┘        └──────────┘
          │                   │
          v                   v
    ┌──────────┐        ┌───────────┐
    │ message  │        │ markdown  │
    │   .rs    │        │    .rs    │
    └──────────┘        └───────────┘
```

---

## Core Components

### 1. Application Entry Point (`main.rs`)

The `main.rs` file bootstraps the application and manages the terminal session lifecycle.

#### Terminal Session Management

```rust
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    use_alternate_screen: bool,
}
```

The `TerminalSession` struct encapsulates terminal setup and teardown:

- **Raw mode**: Enables character-by-character input without line buffering
- **Alternate screen**: Used in full-screen mode to preserve the user's terminal content
- **Mouse capture**: Enabled in full-screen mode for potential future mouse support
- **RAII cleanup**: The `Drop` implementation ensures terminal state is restored on exit

#### UI Mode Selection

```rust
enum UiMode {
    Full,    // Alternate screen, full terminal
    Inline,  // Inline viewport, preserves terminal history
}
```

UI mode is determined by (in order of precedence):

1. Configuration file (`forge.toml` -> `app.tui`)
2. Environment variable (`FORGE_TUI`)
3. Default: `Full`

#### Main Event Loops

Two separate event loops handle the different UI modes:

```rust
async fn run_app_full<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
async fn run_app_inline<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
```

Both loops follow the same pattern:

1. `app.tick()` - Increment animation counter, poll background tasks
2. `tokio::task::yield_now().await` - Allow async tasks to progress
3. `app.process_stream_events()` - Handle streaming response chunks
4. `terminal.draw()` - Render the UI
5. `handle_events()` - Process keyboard input

The inline mode additionally calls `output.flush()` to write completed messages above the input area.

---

### 2. Application State (`app.rs`)

The `App` struct is the central state container for the entire application.

#### Core Fields

```rust
pub struct App {
    // Input state machine
    input: InputState,

    // Display state
    display: Vec<DisplayItem>,
    scroll: ScrollState,
    scroll_max: u16,

    // Application flags
    should_quit: bool,
    toggle_screen_mode: bool,
    status_message: Option<String>,

    // Provider configuration
    api_keys: HashMap<Provider, String>,
    model: ModelName,

    // Animation/timing
    tick: usize,
    last_frame: Instant,           // Frame timing for animations
    modal_effect: Option<ModalEffect>,  // Active modal animation effect

    // Persistence
    data_dir: DataDir,
    context_manager: ContextManager,
    stream_journal: StreamJournal,

    // State machine for async operations
    state: AppState,

    // Output configuration
    output_limits: OutputLimits,
    cache_enabled: bool,
    openai_options: OpenAIRequestOptions,
}
```

---

## Application State Management

### Input State Machine

The TUI uses a state machine pattern for input modes, ensuring type-safe transitions:

```rust
enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
    ModelSelect { draft: DraftInput, selected: usize },
}
```

Each state carries the `DraftInput` (the message being composed), ensuring it persists across mode transitions.

#### Mode Transitions

```
                    ┌──────────────┐
          ┌────────>│    Normal    │<────────┐
          │         └──────────────┘         │
          │ (Esc)         │ (i/a/o)          │ (Esc)
          │               v                  │
          │         ┌──────────────┐         │
          │         │    Insert    │         │
          │         └──────────────┘         │
          │               │ (Enter)          │
          │               v                  │
          │         [Send Message]           │
          │                                  │
    ┌──────────────┐                  ┌──────────────┐
    │   Command    │<─────(: or /)───│    Normal    │
    └──────────────┘                  └──────────────┘
          │                                  ^
          │ (Enter)                          │
          v                                  │
    [Execute Command]────────────────────────┘
```

### Async Operation State Machine

The application uses an explicit state machine for async operations:

```rust
enum AppState {
    Enabled(EnabledState),   // ContextInfinity enabled
    Disabled(DisabledState), // ContextInfinity disabled
}

enum EnabledState {
    Idle,
    Streaming(ActiveStream),
    AwaitingToolResults(PendingToolExecution),
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

This design:

- Makes impossible states unrepresentable
- Prevents race conditions between streaming and summarization
- Allows queuing requests while summarization is in progress

### Scroll State

```rust
enum ScrollState {
    AutoBottom,                        // Follow new content
    Manual { offset_from_top: u16 },   // User-controlled position
}
```

Scroll automatically returns to `AutoBottom` when the user scrolls past the end.

---

## Input Handling System

### Event Processing (`input.rs`)

The `handle_events` function is the entry point for all keyboard input:

```rust
pub async fn handle_events(app: &mut App) -> Result<bool>
```

Returns `true` if the application should quit.

#### Event Flow

1. **Poll with timeout**: `event::poll(Duration::from_millis(100))` - Non-blocking check
2. **Filter key events**: Only handle `KeyEventKind::Press` (important for Windows)
3. **Global handlers**: Ctrl+C always quits
4. **Mode dispatch**: Route to appropriate mode handler

#### Mode-Specific Handlers

**Normal Mode** (`handle_normal_mode`):

| Key | Action |
|-----|--------|
| `q` | Quit application |
| `i` | Enter insert mode |
| `a` | Enter insert mode at end of line |
| `o` | Enter insert mode with cleared line |
| `:` or `/` | Enter command mode |
| `k` or `Up` | Scroll up |
| `j` | Scroll down |
| `g` | Scroll to top |
| `G` | Scroll to bottom |
| `Down` or `End` | Jump to bottom |

**Insert Mode** (`handle_insert_mode`):

| Key | Action |
|-----|--------|
| `Esc` | Return to normal mode |
| `Enter` | Send message |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Left`/`Right` | Move cursor |
| `Home`/`End` | Jump to start/end |
| `Ctrl+U` | Clear entire line |
| `Ctrl+W` | Delete word backwards |
| Character | Insert at cursor |

**Command Mode** (`handle_command_mode`):

| Key | Action |
|-----|--------|
| `Esc` | Cancel and return to normal mode |
| `Enter` | Execute command |
| `Backspace` | Delete last character |
| Character | Append to command |

**Model Select Mode** (`handle_model_select_mode`):

| Key | Action |
|-----|--------|
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `Up`/`k` | Move selection up |
| `Down`/`j` | Move selection down |
| `1`/`2` | Direct selection by index |

### Token-Based Mode Access

The application uses token types to ensure mode-specific operations are only called in the correct mode:

```rust
pub(crate) struct InsertToken(());
pub(crate) struct CommandToken(());

pub(crate) fn insert_token(&self) -> Option<InsertToken>
pub(crate) fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_>
```

This pattern provides compile-time guarantees that `InsertMode` methods can only be called when actually in insert mode.

---

## Type-Driven Design Patterns

Forge uses Rust's type system extensively to enforce correctness at compile time. This section documents the key patterns used throughout the TUI.

### Proof Tokens

Proof tokens are zero-sized types that serve as compile-time evidence that a precondition is met. They cannot be constructed arbitrarily - only specific methods can create them when conditions are satisfied.

#### InsertToken and CommandToken

```rust
#[derive(Debug)]
pub(crate) struct InsertToken(());  // Private unit field prevents external construction

#[derive(Debug)]
pub(crate) struct CommandToken(());
```

These tokens prove the application is in a specific mode:

```rust
// Only returns Some when actually in Insert mode
pub(crate) fn insert_token(&self) -> Option<InsertToken> {
    matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
}

// Requires the token to access mode-specific operations
pub(crate) fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
    InsertMode { app: self }
}
```

**Usage in input.rs:**

```rust
fn handle_insert_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            // Token acquisition proves we're in insert mode
            let Some(token) = app.insert_token() else {
                return;  // Not in insert mode - impossible if called correctly
            };
            let queued = app.insert_mode(token).queue_message();
            if let Some(queued) = queued {
                app.start_streaming(queued);
            }
        }
        _ => {
            let Some(token) = app.insert_token() else { return; };
            let mut insert = app.insert_mode(token);
            // Now safe to call insert-mode-specific methods
            match key.code {
                KeyCode::Backspace => insert.delete_char(),
                KeyCode::Char(c) => insert.enter_char(c),
                // ...
            }
        }
    }
}
```

#### QueuedUserMessage

A proof type that a user message has been validated and is ready to send:

```rust
#[derive(Debug)]
pub struct QueuedUserMessage {
    config: ApiConfig,  // Contains validated API key and model
}
```

This type can only be created by `InsertMode::queue_message()`, which:

1. Validates the draft is non-empty
2. Validates an API key exists
3. Constructs the API configuration
4. Adds the user message to history

```rust
impl<'a> InsertMode<'a> {
    pub fn queue_message(self) -> Option<QueuedUserMessage> {
        let text = self.app.input.draft_mut().take_text();
        let content = NonEmptyString::new(text).ok()?;  // Validates non-empty
        
        let api_key = self.app.current_api_key()?.clone();  // Validates key exists
        let config = ApiConfig::new(/* ... */).ok()?;
        
        self.app.push_history_message(Message::user(content));
        self.app.enter_normal_mode();
        
        Some(QueuedUserMessage { config })
    }
}
```

`start_streaming()` consumes this token, ensuring a message cannot be sent twice and all preconditions are met.

### Mode Wrapper Types

`InsertMode` and `CommandMode` are proxy types that provide controlled access to mode-specific operations:

```rust
pub(crate) struct InsertMode<'a> {
    app: &'a mut App,
}

pub(crate) struct CommandMode<'a> {
    app: &'a mut App,
}
```

These wrappers:

1. Borrow `App` mutably, preventing concurrent access
2. Expose only operations valid for that mode
3. Are constructed only when the token proves the mode is active

```rust
impl<'a> InsertMode<'a> {
    pub fn move_cursor_left(&mut self) { self.draft_mut().move_cursor_left(); }
    pub fn enter_char(&mut self, c: char) { self.draft_mut().enter_char(c); }
    pub fn delete_char(&mut self) { self.draft_mut().delete_char(); }
    pub fn queue_message(self) -> Option<QueuedUserMessage> { /* ... */ }
}

impl<'a> CommandMode<'a> {
    pub fn push_char(&mut self, c: char) { /* ... */ }
    pub fn backspace(&mut self) { /* ... */ }
    pub fn take_command(self) -> Option<EnteredCommand> { /* ... */ }
}
```

### NonEmptyString

A newtype guaranteeing message content is never empty:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyStringError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value))
        }
    }
}
```

All message types use `NonEmptyString` for content, making empty messages unrepresentable.

### NonEmptyStaticStr

A compile-time guaranteed non-empty string for static badges and constants:

```rust
pub struct NonEmptyStaticStr(&'static str);

impl NonEmptyStaticStr {
    pub const fn new(value: &'static str) -> Self {
        if value.is_empty() {
            panic!("NonEmptyStaticStr must not be empty");  // Compile-time check
        }
        Self(value)
    }
}
```

Used for recovery badges and error messages that must never be empty.

### EnteredCommand

Proof that a command was entered in command mode:

```rust
#[derive(Debug)]
pub(crate) struct EnteredCommand {
    raw: String,
}
```

Created by `CommandMode::take_command()` when Enter is pressed, consumed by `App::process_command()`.

### State as Location Pattern

The `AppState` enum makes the application's async state explicit:

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
```

Key benefits:

- **Impossible states are unrepresentable**: Cannot be streaming and summarizing simultaneously
- **State data is co-located**: `ActiveStream` only exists during streaming
- **Transitions are explicit**: Must match on current state to transition

### Summary of Type-Driven Guarantees

| Type | Guarantee |
|------|-----------|
| `InsertToken` | Currently in Insert mode |
| `CommandToken` | Currently in Command mode |
| `QueuedUserMessage` | Message validated, API configured, ready to send |
| `EnteredCommand` | Command was entered in Command mode |
| `NonEmptyString` | Content is non-empty (runtime validated) |
| `NonEmptyStaticStr` | Content is non-empty (compile-time validated) |
| `ActiveJournal` | Stream journaling session is active |
| `ModelName` | Model name is valid for its provider |
| `AppState` variants | Mutually exclusive async operations |

---

## Rendering Pipeline

### Full-Screen Rendering (`ui.rs`)

The main `draw` function orchestrates the full-screen layout:

```rust
pub fn draw(frame: &mut Frame, app: &mut App) {
    // 1. Clear with background color
    // 2. Create vertical layout
    // 3. Render components
    // 4. Overlay command palette if in command mode
}
```

#### Layout Structure

```
┌─────────────────────────────────────────┐
│                                         │
│              Messages Area              │  Constraint::Min(1)
│           (scrollable, flex)            │
│                                         │
├─────────────────────────────────────────┤
│  [MODE] │ prompt text                   │  Constraint::Length(5)
│         └── key hints ──────────────────│
├─────────────────────────────────────────┤
│ ● Claude │ model-name      context: 45% │  Constraint::Length(1)
└─────────────────────────────────────────┘
```

#### Message Rendering

Messages are rendered with role-specific styling:

```rust
fn render_message(msg: &Message, lines: &mut Vec<Line>, msg_count: &mut usize) {
    // Header: icon + name
    // Content: rendered as markdown
}
```

Role styling:

| Role | Icon | Color |
|------|------|-------|
| System | `●` | Muted |
| User | `○` | Green |
| Assistant | `◆` | Purple (primary) |
| Tool Use | `⚙` | Cyan (accent) |
| Tool Result | `✓`/`✗` | Green (success) / Red (error) |

#### Streaming Indicator

When streaming is active:

- If content is empty: Animated spinner with "Thinking..."
- If content exists: Render partial content as markdown

```rust
if streaming.content().is_empty() {
    let spinner = spinner_frame(app.tick_count());
    // Display: "⠋ Thinking..."
}
```

#### Scrollbar

A custom scrollbar is rendered on the right edge:

- `↑` at top
- `█` for thumb position
- `│` for track
- `↓` at bottom

#### Command Palette

When in command mode, a centered overlay displays common commands:

```
╭─────────────────────────────────────────╮
│  Commands                               │
│                                         │
│  /q, quit        Exit the application   │
│  /clear          Clear conversation     │
│  /model <name>   Change the model       │
│  /p, provider    Switch provider        │
│  /screen         Toggle screen mode     │
│  /help           Show available commands│
╰─────────────────────────────────────────╯
```

> **Note:** The palette shows common commands for quick reference. The full command list (including `/ctx`, `/jrnl`, `/sum`, `/cancel`, `/tools`, `/tool`) is documented in the [Command System](#command-system) section.

#### Tool Approval Overlay

When tools require approval, a centered overlay displays the pending tool batch:

```
╭──────────────────────────────────────────────────╮
│  Tool Approval Required                          │
│                                                  │
│  ⏸ apply_patch (call_001)            [Medium]   │
│      Apply patch to 2 file(s): src/main.rs, ... │
│                                                  │
│  ⏸ run_command (call_002)            [High]     │
│      Run command: cargo build                    │
│                                                  │
│  ────────────────────────────────────────────── │
│  [a]pprove all  [d]eny all  [Space] toggle      │
│  [j/k] navigate  [Enter] confirm selection       │
╰──────────────────────────────────────────────────╯
```

**Visual indicators:**

- `⏸` — Pending (not yet approved)
- `✓` — Approved (selected)
- `⊘` — Pre-resolved error (invalid args, sandbox violation)

**Risk level badges:**

- `[Low]` — Read-only operations (green)
- `[Medium]` — File modifications (yellow)
- `[High]` — Shell commands (red)

#### Tool Execution Progress

During execution, the tool batch shows real-time status:

```
╭──────────────────────────────────────────────────╮
│  ⠋ Tool execution                               │
│                                                  │
│  ✓ apply_patch (call_001)                       │
│  ⠋ run_command (call_002)                       │
│                                                  │
│  Tool output:                                    │
│      ▶ run_command                               │
│      Compiling forge v0.1.0                     │
│      Finished dev [unoptimized + debuginfo]     │
╰──────────────────────────────────────────────────╯
```

**Status indicators:**

- `⠋` (spinner) — Currently executing
- `✓` — Completed successfully
- `✗` — Completed with error
- `•` — Queued (not yet started)

#### Tool Recovery Prompt

If a previous session crashed during tool execution, a recovery prompt appears on startup:

```
╭──────────────────────────────────────────────────╮
│  Recovered Tool Batch                            │
│                                                  │
│  Previous session crashed during tool execution. │
│                                                  │
│  2 of 3 tools completed:                        │
│  ✓ read_file                                     │
│  ✓ apply_patch                                   │
│  • run_command (not started)                     │
│                                                  │
│  [r]esume with partial results                   │
│  [d]iscard and mark failed                       │
╰──────────────────────────────────────────────────╯
```

**Important:** Recovery does NOT re-execute tools. The user decides whether to continue the conversation with partial results or discard the batch.

### Inline Rendering (`ui_inline.rs`)

Inline mode uses a fixed-height viewport at the bottom of the terminal:

```rust
pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;
```

#### InlineOutput State

```rust
pub struct InlineOutput {
    next_display_index: usize,  // Track which messages have been printed
    has_output: bool,           // Whether any output has been written
}
```

The `flush` method writes completed messages above the viewport using `terminal.insert_before()`, which scrolls existing terminal content up.

#### Differences from Full-Screen

| Aspect | Full-Screen | Inline |
|--------|-------------|--------|
| Message display | Scrollable widget | Written to terminal history |
| Viewport | Entire terminal | Fixed 6-line area |
| Terminal history | Preserved (alternate screen) | Visible and extended |
| Streaming | In-place update | Only shows input area |
| Markdown rendering | Full markdown parser | Plain text with indentation |

**Role Icons by Mode:**

| Role | Full-Screen | Inline |
|------|-------------|--------|
| System | `●` | `S` |
| User | `○` | `○` |
| Assistant | `◆` | `*` |

Inline mode uses simpler icons for better terminal compatibility and to distinguish flushed messages visually.

---

## UI Modes: Full vs Inline

### Full-Screen Mode

- Uses crossterm's alternate screen
- Complete control over terminal
- Supports scrolling through message history
- Mouse capture enabled
- Best for extended sessions

### Inline Mode

- Runs within normal terminal flow
- Messages become part of terminal scrollback
- Viewport is just input + status bar
- Preserves terminal context
- Good for quick queries

### Mode Switching

Users can switch modes at runtime with the `/screen` command:

```rust
Some("screen") => {
    self.toggle_screen_mode = true;
}
```

The main loop detects this flag and reinitializes the terminal session:

```rust
if app.take_toggle_screen_mode() {
    return Ok(RunResult::SwitchMode);
}
```

---

## Message and Streaming System

### Message Types (`message.rs`)

```rust
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}
```

Each message type contains:

- `content: NonEmptyString` - Guaranteed non-empty content
- `timestamp: SystemTime` - When the message was created
- `model: ModelName` (Assistant only) - Which model generated the response

### NonEmptyString

A newtype ensuring message content is never empty:

```rust
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyStringError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value))
        }
    }
}
```

### Streaming Messages

```rust
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
}
```

Stream events:

```rust
pub enum StreamEvent {
    TextDelta(String),      // Content chunk
    ThinkingDelta(String),  // Reasoning content (not displayed)
    Done,                   // Stream completed
    Error(String),          // Stream failed
}
```

### Streaming Lifecycle

The streaming system follows a strict **persist-before-display** pattern for crash recovery:

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│ queue_      │───>│ start_      │───>│ process_    │───>│ finish_     │
│ message()   │    │ streaming() │    │ stream_     │    │ streaming() │
│             │    │             │    │ events()    │    │             │
└─────────────┘    └─────────────┘    └──────┬──────┘    └─────────────┘
                                             │
                              ┌──────────────┴──────────────┐
                              │  For each StreamEvent:      │
                              │  1. Persist to journal      │
                              │  2. Apply to StreamingMessage│
                              │  3. UI renders new content  │
                              └─────────────────────────────┘
```

**Detailed Steps:**

1. **Queue Message** (`InsertMode::queue_message()`):
   - Validates draft is non-empty via `NonEmptyString::new()`
   - Validates API key exists
   - Constructs `ApiConfig` with provider and model
   - Adds user message to history
   - Returns `QueuedUserMessage` proof token

2. **Start Stream** (`App::start_streaming()`):
   - Validates state is `Idle` (not already streaming/summarizing)
   - Prepares context via `context_manager.prepare()`
   - Begins journal session via `stream_journal.begin_session()`
   - Creates MPSC channel for stream events
   - Spawns async task with abort handle
   - Transitions to `Streaming(ActiveStream)` state

3. **Process Events** (`App::process_stream_events()`):
   - Non-blocking loop processing all available events
   - **Critical**: Each event is persisted to journal BEFORE applying to UI
   - On journal failure, stream is aborted with partial content preserved
   - Handles `TextDelta`, `ThinkingDelta` (silent), `Done`, and `Error` events

4. **Finish Stream** (`App::finish_streaming()`):
   - Aborts the async task handle
   - Seals the journal session
   - Converts `StreamingMessage` to permanent `Message`
   - Handles empty responses with `EMPTY_RESPONSE_BADGE`
   - Handles errors with formatted error message

### Journal-Before-Display Pattern

```rust
pub fn process_stream_events(&mut self) {
    loop {
        let event = active.message.try_recv_event()?;
        
        // PERSIST FIRST - before any display update
        let persist_result = match &event {
            StreamEvent::TextDelta(text) => 
                active.journal.append_text(&mut self.stream_journal, text.clone()),
            StreamEvent::Done => 
                active.journal.append_done(&mut self.stream_journal),
            StreamEvent::Error(msg) => 
                active.journal.append_error(&mut self.stream_journal, msg.clone()),
            StreamEvent::ThinkingDelta(_) => Ok(()), // Not persisted
        };
        
        // Only apply to display if persistence succeeded
        if let Err(e) = persist_result {
            // Abort streaming, preserve partial content with badge
            return;
        }
        
        // NOW safe to update display
        let finish_reason = active.message.apply_event(event);
    }
}
```

### Crash Recovery

The `StreamJournal` provides durability for streaming responses. On startup, the application checks for incomplete streams:

```rust
pub fn check_crash_recovery(&mut self) -> Option<RecoveredStream> {
    let recovered = self.stream_journal.recover()?;
    // Recovered content is added with a warning badge
}
```

**Recovery Scenarios:**

| Recovery Type | Badge | Description |
|---------------|-------|-------------|
| `RecoveredStream::Complete` | `[Recovered - stream completed but not finalized]` | Stream finished but app crashed before cleanup |
| `RecoveredStream::Incomplete` | `[Recovered - incomplete response from previous session]` | Stream was interrupted mid-response |

**Additional Badges:**

| Badge | Cause |
|-------|-------|
| `[Aborted - journal write failed]` | Streaming stopped due to journal persistence failure |
| `[Stream error]` | API returned an error during streaming |
| `[Empty response - API returned no content]` | API completed but returned no text |

---

## Context Management Integration

### ContextManager

The TUI integrates with the ContextInfinity system for adaptive context window management:

```rust
context_manager: ContextManager,
```

Key interactions:

1. **Message Tracking**: All messages go through `push_history_message()` which updates both display and context manager
2. **Token Counting**: Context manager tracks token usage for each message
3. **API Preparation**: Before sending requests, `context_manager.prepare()` builds the working context
4. **Summarization**: When context exceeds limits, background summarization is triggered

### Context Usage Display

The status bar shows context usage with color coding:

```rust
let usage_color = match usage.severity() {
    0 => colors::GREEN,  // < 70%
    1 => colors::YELLOW, // 70-90%
    _ => colors::RED,    // > 90%
};
```

### Context Usage Status

The `ContextUsageStatus` enum represents the current state of context management:

```rust
pub enum ContextUsageStatus {
    Ready(ContextUsage),                    // Normal operation
    NeedsSummarization { usage, needed },   // Context full, summarization required
    RecentMessagesTooLarge {                // Recent messages exceed budget
        usage,
        required_tokens,
        budget_tokens,
    },
}
```

The `RecentMessagesTooLarge` variant indicates that recent messages alone exceed the token budget, which cannot be resolved by summarization.

### Summarization Flow

```
┌───────────────┐     ┌─────────────────┐     ┌──────────────┐
│ Context full  │────>│ start_summar-   │────>│ Background   │
│ detected      │     │ ization()       │     │ API call     │
└───────────────┘     └─────────────────┘     └──────┬───────┘
                                                     │
┌───────────────┐     ┌─────────────────┐           │
│ Resume normal │<────│ complete_       │<──────────┘
│ operation     │     │ summarization() │
└───────────────┘     └─────────────────┘
```

---

## Theming and Styling

### Color Palette (`theme.rs`)

The TUI uses a Catppuccin-inspired color palette:

```rust
pub mod colors {
    // Primary brand colors
    pub const PRIMARY: Color = Color::Rgb(139, 92, 246);      // Purple
    pub const PRIMARY_DIM: Color = Color::Rgb(109, 72, 206);  // Darker purple

    // Background colors
    pub const BG_DARK: Color = Color::Rgb(17, 17, 27);        // Near black
    pub const BG_PANEL: Color = Color::Rgb(30, 30, 46);       // Panel background
    pub const BG_HIGHLIGHT: Color = Color::Rgb(44, 46, 68);   // Row highlight (model selector)

    // Text colors
    pub const TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244);   // Main text
    pub const TEXT_SECONDARY: Color = Color::Rgb(147, 153, 178); // Dimmed
    pub const TEXT_MUTED: Color = Color::Rgb(88, 91, 112);       // Very dim

    // Accent colors
    pub const GREEN: Color = Color::Rgb(166, 227, 161);  // Success/user
    pub const YELLOW: Color = Color::Rgb(249, 226, 175); // Warning
    pub const RED: Color = Color::Rgb(243, 139, 168);    // Error
    pub const PEACH: Color = Color::Rgb(250, 179, 135);  // Accent
}
```

### Pre-defined Styles

```rust
pub mod styles {
    pub fn user_name() -> Style       // Green, bold
    pub fn assistant_name() -> Style  // Purple, bold
    pub fn mode_normal() -> Style     // Dark on gray, bold
    pub fn mode_insert() -> Style     // Dark on green, bold
    pub fn mode_command() -> Style    // Dark on yellow, bold
    pub fn key_hint() -> Style        // Muted text
    pub fn key_highlight() -> Style   // Peach, bold
}
```

### Spinner Animation

```rust
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}
```

The tick counter increments every ~100ms (the event poll timeout), creating a smooth animation.

---

## Key Data Structures

### DraftInput

Manages the text being composed with cursor position:

```rust
struct DraftInput {
    text: String,
    cursor: usize,  // Character index (not byte index)
}
```

Key methods:

- `enter_char(c)` - Insert character at cursor
- `delete_char()` - Delete character before cursor
- `delete_word_backwards()` - Delete to previous word boundary
- `byte_index()` - Convert character cursor to byte index for string operations

### DisplayItem

References messages for display:

```rust
pub(crate) enum DisplayItem {
    History(MessageId),  // Reference to persisted message
    Local(Message),      // Transient message (errors, recovered content)
}
```

### ActiveStream

Holds state for an in-progress streaming response:

```rust
struct ActiveStream {
    message: StreamingMessage,    // Accumulating content with MPSC receiver
    journal: ActiveJournal,       // RAII handle ensuring chunks are journaled
    abort_handle: AbortHandle,    // Cancellation handle from futures_util
}
```

The `ActiveStream` only exists when the application is in the `Streaming` state variant, ensuring streaming state is co-located with the streaming operation.

### QueuedUserMessage

Proof type that a message is ready to be sent:

```rust
pub struct QueuedUserMessage {
    config: ApiConfig,  // Validated API key + model configuration
}
```

Created by `InsertMode::queue_message()` after validation, consumed by `App::start_streaming()`. This type cannot be constructed outside of `InsertMode`, enforcing the message-sending workflow.

### PredefinedModel

Enum of curated model options for the model selector:

```rust
pub enum PredefinedModel {
    ClaudeOpus,  // claude-opus-4-5-20251101
    Gpt52,       // gpt-5.2
}

impl PredefinedModel {
    pub const fn all() -> &'static [PredefinedModel] { /* ... */ }
    pub const fn display_name(&self) -> &'static str { /* ... */ }
    pub fn to_model_name(&self) -> ModelName { /* ... */ }
    pub const fn provider(&self) -> Provider { /* ... */ }
}
```

> **Note:** The `PredefinedModel` options shown in the model selector differ from the provider default models:
>
> - **Provider default** (used on startup): `claude-sonnet-4-5-20250929` for Claude, `gpt-5.2` for OpenAI
> - **PredefinedModel::ClaudeOpus**: `claude-opus-4-5-20251101` (premium option)
>
> **OpenAI Model Validation:** The codebase requires OpenAI model names to start with `gpt-5`. Models like `gpt-4o` will be rejected with a `ModelParseError::OpenAIMinimum` error. This ensures compatibility with the OpenAI Responses API.

---

## Event Loop and Update Cycle

### Tick Cycle

Each iteration of the event loop:

```rust
loop {
    // 1. Increment tick counter and poll background tasks
    app.tick();
    
    // 2. Yield to tokio runtime for async task progress
    tokio::task::yield_now().await;
    
    // 3. Process any pending stream events
    app.process_stream_events();
    
    // 4. Render the UI
    terminal.draw(|frame| ui::draw(frame, app))?;
    
    // 5. Check for mode switch request
    if app.take_toggle_screen_mode() {
        return Ok(RunResult::SwitchMode);
    }
    
    // 6. Handle keyboard input (100ms timeout)
    if handle_events(app).await? {
        return Ok(RunResult::Quit);
    }
}
```

### Timing

- **Event poll timeout**: 100ms
- **Spinner animation**: Updates every tick (~100ms)
- **Streaming events**: Processed every tick (non-blocking)
- **Summarization polling**: Checked every tick

---

## Command System

### Available Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `quit` | `q` | Exit the application |
| `clear` | - | Clear conversation, abort any streaming/summarization, reset context |
| `model <name>` | - | Set specific model for current provider, or open model selector if no argument |
| `provider <name>` | `p` | Switch LLM provider (claude/gpt); shows current provider and available models if no argument |
| `context` | `ctx` | Show detailed context usage: limits, window size, budget, summarization status |
| `journal` | `jrnl` | Show stream journal statistics: entry counts, step ID, streaming state |
| `summarize` | `sum` | Manually trigger background summarization (only when ContextInfinity enabled) |
| `cancel` | - | Cancel active streaming and discard journal entries |
| `screen` | - | Toggle between full-screen and inline terminal mode |
| `help` | - | List available commands |

### Command Processing Flow

Commands are processed through a type-safe pipeline:

1. **Entry**: User presses Enter in Command mode
2. **Token acquisition**: `command_token()` returns `Some(CommandToken)` proving Command mode
3. **Command extraction**: `CommandMode::take_command()` returns `EnteredCommand`
4. **Processing**: `App::process_command()` parses and executes the command
5. **Mode transition**: Most commands return to Normal mode automatically

```rust
// In input.rs - handle_command_mode
KeyCode::Enter => {
    let Some(token) = app.command_token() else { return; };
    let command_mode = app.command_mode(token);
    let Some(command) = command_mode.take_command() else { return; };
    app.process_command(command);
}
```

### Command Implementation

```rust
pub(crate) fn process_command(&mut self, command: EnteredCommand) {
    let parts: Vec<&str> = command.raw.split_whitespace().collect();
    
    match parts.first().copied() {
        Some("q" | "quit") => {
            self.request_quit();
        }
        Some("clear") => {
            // Abort any active streaming or summarization
            let state = self.replace_with_idle();
            match state {
                AppState::Enabled(EnabledState::Streaming(active))
                | AppState::Disabled(DisabledState::Streaming(active)) => {
                    active.abort_handle.abort();
                    let _ = active.journal.discard(&mut self.stream_journal);
                }
                // Handle other states...
            }
            self.display.clear();
            self.context_manager = ContextManager::new(self.model.as_str());
            self.set_status("Conversation cleared");
        }
        Some("model") => {
            if let Some(model_name) = parts.get(1) {
                // Parse and set specific model
                match self.provider().parse_model(model_name) {
                    Ok(model) => self.set_model(model),
                    Err(e) => self.set_status(format!("Invalid model: {e}")),
                }
            } else {
                // Open model selector overlay
                self.enter_model_select_mode();
            }
        }
        // ... other commands
    }
}
```

### Model Selector

When `/model` is invoked without arguments, the application enters `ModelSelect` mode, displaying a popup overlay with predefined model options:

```rust
pub enum PredefinedModel {
    ClaudeOpus,   // "Anthropic Claude Opus 4.5"
    Gpt52,        // "OpenAI GPT 5.2"
}

impl PredefinedModel {
    pub fn to_model_name(&self) -> ModelName {
        match self {
            PredefinedModel::ClaudeOpus => 
                ModelName::known(Provider::Claude, "claude-opus-4-5-20251101"),
            PredefinedModel::Gpt52 => 
                ModelName::known(Provider::OpenAI, "gpt-5.2"),
        }
    }
}
```

The model selector features animation effects using a custom `ModalEffect` system:

```rust
pub fn enter_model_select_mode(&mut self) {
    self.input = std::mem::take(&mut self.input).into_model_select();
    // Create pop-scale animation (scales up from 60% to 100% over 700ms)
    self.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
}
```

The model selector also displays a "Google Gemini 3 Pro" preview entry (muted, non-selectable) to hint at future provider support.

---

## Extension Guide

This section provides detailed patterns for extending the TUI with new functionality.

### Adding a New Input Mode

Adding a new input mode requires changes across multiple files. Here's a complete example adding a hypothetical "Search" mode:

**Step 1: Add InputMode variant (`app.rs`)**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
    Search,  // New mode
}
```

**Step 2: Add InputState variant (`app.rs`)**

```rust
#[derive(Debug)]
enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
    ModelSelect { draft: DraftInput, selected: usize },
    Search { draft: DraftInput, query: String, results: Vec<usize> },  // New
}
```

**Step 3: Update InputState methods (`app.rs`)**

```rust
impl InputState {
    fn mode(&self) -> InputMode {
        match self {
            // ... existing matches
            InputState::Search { .. } => InputMode::Search,
        }
    }

    fn draft(&self) -> &DraftInput {
        match self {
            // ... existing matches
            InputState::Search { draft, .. } => draft,
        }
    }

    fn into_normal(self) -> InputState {
        match self {
            // ... existing matches
            InputState::Search { draft, .. } => InputState::Normal(draft),
        }
    }

    fn into_search(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => InputState::Search {
                draft,
                query: String::new(),
                results: Vec::new(),
            },
            // Handle other state transitions...
        }
    }
}
```

**Step 4: Add proof token and mode wrapper (`app.rs`)**

```rust
#[derive(Debug)]
pub(crate) struct SearchToken(());

pub(crate) struct SearchMode<'a> {
    app: &'a mut App,
}

impl App {
    pub(crate) fn search_token(&self) -> Option<SearchToken> {
        matches!(&self.input, InputState::Search { .. }).then_some(SearchToken(()))
    }

    pub(crate) fn search_mode(&mut self, _token: SearchToken) -> SearchMode<'_> {
        SearchMode { app: self }
    }

    pub fn enter_search_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_search();
    }
}

impl<'a> SearchMode<'a> {
    pub fn update_query(&mut self, c: char) {
        if let InputState::Search { query, .. } = &mut self.app.input {
            query.push(c);
            // Trigger search...
        }
    }

    pub fn get_results(&self) -> &[usize] {
        if let InputState::Search { results, .. } = &self.app.input {
            results
        } else {
            &[]
        }
    }
}
```

**Step 5: Add input handler (`input.rs`)**

```rust
pub async fn handle_events(app: &mut App) -> Result<bool> {
    // ... existing code
    match app.input_mode() {
        InputMode::Normal => handle_normal_mode(app, key),
        InputMode::Insert => handle_insert_mode(app, key),
        InputMode::Command => handle_command_mode(app, key),
        InputMode::ModelSelect => handle_model_select_mode(app, key),
        InputMode::Search => handle_search_mode(app, key),  // New
    }
}

fn handle_search_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.enter_normal_mode();
        }
        KeyCode::Enter => {
            // Execute search or select result
            let Some(token) = app.search_token() else { return };
            let search = app.search_mode(token);
            // ... handle selection
        }
        KeyCode::Char(c) => {
            let Some(token) = app.search_token() else { return };
            let mut search = app.search_mode(token);
            search.update_query(c);
        }
        _ => {}
    }
}
```

**Step 6: Add UI rendering (`ui.rs`)**

```rust
pub fn draw(frame: &mut Frame, app: &mut App) {
    // ... existing code

    // Draw search overlay if in search mode
    if app.input_mode() == InputMode::Search {
        draw_search_overlay(frame, app);
    }
}

fn draw_search_overlay(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let overlay_area = centered_rect(60, 40, area);  // Helper function

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Search ")
        .style(Style::default().bg(colors::BG_PANEL));

    // Render search UI...
    frame.render_widget(block, overlay_area);
}
```

**Step 7: Update key hints in draw_input (`ui.rs`)**

```rust
let hints = match mode {
    // ... existing modes
    InputMode::Search => vec![
        Span::styled("Enter", styles::key_highlight()),
        Span::styled(" select  ", styles::key_hint()),
        Span::styled("Esc", styles::key_highlight()),
        Span::styled(" cancel ", styles::key_hint()),
    ],
};
```

### Adding a New Command

**Step 1: Add command handling (`app.rs`)**

```rust
pub(crate) fn process_command(&mut self, command: EnteredCommand) {
    let parts: Vec<&str> = command.raw.split_whitespace().collect();

    match parts.first().copied() {
        // ... existing commands

        Some("search" | "s") => {
            if let Some(query) = parts.get(1..) {
                let query = query.join(" ");
                self.set_status(format!("Searching for: {}", query));
                // Implement search logic...
            } else {
                self.enter_search_mode();
            }
        }

        Some(cmd) => {
            self.set_status(format!("Unknown command: {cmd}"));
        }
        None => {}
    }
}
```

**Step 2: Update help command (`app.rs`)**

```rust
Some("help") => {
    self.set_status(
        "Commands: /q(uit), /clear, /cancel, /model, /p(rovider), /ctx, /jrnl, /sum, /screen, /s(earch)",
    );
}
```

**Step 3: Add to command palette (`ui.rs`)**

```rust
fn draw_command_palette(frame: &mut Frame, _app: &App) {
    let commands = vec![
        // ... existing commands
        ("s, search <query>", "Search message history"),
    ];
    // ... rest of rendering
}
```

### Adding a New UI Component

**Example: Adding a token usage bar**

```rust
// In ui.rs

fn draw_token_bar(frame: &mut Frame, app: &App, area: Rect) {
    let usage = app.context_usage_status();
    let (current, max) = match &usage {
        ContextUsageStatus::Ready(u) => (u.used_tokens(), u.budget_tokens()),
        ContextUsageStatus::NeedsSummarization { usage, .. } =>
            (usage.used_tokens(), usage.budget_tokens()),
        ContextUsageStatus::RecentMessagesTooLarge { usage, .. } =>
            (usage.used_tokens(), usage.budget_tokens()),
    };

    let ratio = (current as f64 / max as f64).min(1.0);
    let filled_width = (area.width as f64 * ratio) as u16;

    let bar_color = if ratio < 0.7 {
        colors::GREEN
    } else if ratio < 0.9 {
        colors::YELLOW
    } else {
        colors::RED
    };

    // Draw background
    let bg = Block::default().style(Style::default().bg(colors::BG_PANEL));
    frame.render_widget(bg, area);

    // Draw filled portion
    let filled_area = Rect {
        x: area.x,
        y: area.y,
        width: filled_width,
        height: area.height,
    };
    let filled = Block::default().style(Style::default().bg(bar_color));
    frame.render_widget(filled, filled_area);

    // Draw label
    let label = format!("{}/{}k", current / 1000, max / 1000);
    let label_para = Paragraph::new(label)
        .alignment(Alignment::Center)
        .style(Style::default().fg(colors::TEXT_PRIMARY));
    frame.render_widget(label_para, area);
}

// Update main draw function
pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(1),        // Messages
            Constraint::Length(1),     // Token bar (new)
            Constraint::Length(input_height), // Input
            Constraint::Length(1),     // Status bar
        ])
        .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_token_bar(frame, app, chunks[1]);  // New
    draw_input(frame, app, chunks[2]);
    draw_status_bar(frame, app, chunks[3]);
}
```

### Adding Modal Animations

The TUI uses a custom `ModalEffect` system (defined in `forge-engine`) for modal animations. Available effect types:

```rust
/// The kind of modal animation effect.
pub enum ModalEffectKind {
    PopScale,   // Scales from 60% to 100% with ease-out-cubic
    SlideUp,    // Slides up from below with ease-out-cubic
}

/// Modal animation effect state.
pub struct ModalEffect {
    kind: ModalEffectKind,
    elapsed: Duration,
    duration: Duration,
}
```

To add animations to new overlays:

```rust
impl App {
    pub fn enter_your_modal_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_your_modal();

        // Create animation effect (choose one):
        self.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
        // Or: ModalEffect::slide_up(Duration::from_millis(100))
    }

    pub fn enter_normal_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_normal();
        self.modal_effect = None;  // Clear animation on mode exit
    }
}
```

Apply the effect in your draw function using `apply_modal_effect()` from `effects.rs`:

```rust
fn draw_your_modal(frame: &mut Frame, app: &mut App) {
    let base_area = /* calculate base rectangle */;

    // Advance animation and get transformed area
    let elapsed = app.frame_elapsed();
    let (modal_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, frame.area()),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    // Render modal at transformed area
    frame.render_widget(Clear, modal_area);
    // ... render modal content
}
```

### Modifying the Color Theme

**Adding new colors (`theme.rs`)**

```rust
pub mod colors {
    // ... existing colors

    // Add new semantic colors
    pub const SUCCESS: Color = Color::Rgb(166, 227, 161);
    pub const WARNING: Color = Color::Rgb(249, 226, 175);
    pub const ERROR: Color = Color::Rgb(243, 139, 168);
    pub const INFO: Color = Color::Rgb(137, 180, 250);
}
```

**Adding new styles (`theme.rs`)**

```rust
pub mod styles {
    // ... existing styles

    pub fn success_text() -> Style {
        Style::default()
            .fg(colors::SUCCESS)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_badge() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::ERROR)
            .add_modifier(Modifier::BOLD)
    }
}
```

### Adding a New Provider

Adding a new LLM provider requires changes in `provider.rs` and updates to the App initialization:

**Step 1: Extend Provider enum (`provider.rs`)**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Claude,
    OpenAI,
    YourProvider,  // New
}

impl Provider {
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI, Provider::YourProvider]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "GPT",
            Provider::YourProvider => "YourProvider",
        }
    }

    pub fn env_var(&self) -> &'static str {
        match self {
            Provider::Claude => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::YourProvider => "YOUR_PROVIDER_API_KEY",
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            Provider::Claude => ModelName::known(Self::Claude, "claude-sonnet-4-5-20250929"),
            Provider::OpenAI => ModelName::known(Self::OpenAI, "gpt-5.2"),
            Provider::YourProvider => ModelName::known(Self::YourProvider, "your-default-model"),
        }
    }

    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            // ... existing providers
            Provider::YourProvider => &["your-model-1", "your-model-2"],
        }
    }
}
```

**Step 2: Add API key loading (`app.rs`)**

```rust
// In App::new()
if let Some(key) = keys.your_provider.as_ref() {
    let resolved = crate::config::expand_env_vars(key);
    if !resolved.trim().is_empty() {
        api_keys.insert(Provider::YourProvider, resolved.trim().to_string());
    }
}

// Environment fallback
if !api_keys.contains_key(&Provider::YourProvider) {
    if let Ok(key) = std::env::var("YOUR_PROVIDER_API_KEY") {
        if !key.trim().is_empty() {
            api_keys.insert(Provider::YourProvider, key);
        }
    }
}
```

**Step 3: Implement streaming (`provider.rs`)**

Add the streaming implementation in the `send_message` function, handling the provider's specific API format and SSE event structure.

---

## Appendix: Markdown Rendering

The `markdown.rs` module converts markdown to ratatui `Line` and `Span` types.

### Supported Features

- **Headings**: Rendered bold
- **Bold/Italic**: Modifier-based styling
- **Code blocks**: Muted color with fence markers
- **Inline code**: Peach-colored with backticks
- **Lists**: Bullet points and numbered lists with proper indentation
- **Tables**: Full box-drawing character borders with alignment

### Table Rendering Example

```
    ┌───────┬───────┬───────┐
    │ Col A │ Col B │ Col C │
    ├───────┼───────┼───────┤
    │ 1     │ 2     │ 3     │
    │ 4     │ 5     │ 6     │
    └───────┴───────┴───────┘
```

### Unicode Width Handling

The renderer uses the `unicode-width` crate for proper handling of:

- CJK characters (double-width)
- Emoji
- Combining characters

---

## Summary

The Forge TUI is a well-structured, modal terminal interface built on these principles:

### Architectural Strengths

- **Clear separation of concerns**: State management (`app.rs`), rendering (`ui.rs`, `ui_inline.rs`), and input handling (`input.rs`) are cleanly separated
- **Type-driven correctness**: Proof tokens, newtype wrappers, and explicit state machines prevent entire categories of bugs at compile time
- **Robust streaming**: Journal-before-display pattern with crash recovery ensures no data is lost
- **Dual rendering modes**: Full-screen and inline modes share components while adapting to different use cases
- **Rich markdown rendering**: Full support for tables, code blocks, and formatting
- **Adaptive context management**: Deep integration with ContextInfinity for automatic summarization

### Key Design Patterns

| Pattern | Purpose | Example |
|---------|---------|---------|
| Proof Tokens | Enforce preconditions at compile time | `InsertToken`, `CommandToken`, `QueuedUserMessage` |
| Mode Wrappers | Controlled access to mode-specific operations | `InsertMode<'a>`, `CommandMode<'a>` |
| State as Location | Make async operation state explicit | `AppState::Enabled(EnabledState::Streaming(...))` |
| Newtype Validation | Guarantee invariants via types | `NonEmptyString`, `ModelName` |
| RAII Handles | Ensure cleanup via Drop | `ActiveJournal`, `TerminalSession` |

### File Quick Reference

| File | Responsibility |
|------|----------------|
| `main.rs` | Entry point, terminal session lifecycle, event loop |
| `app.rs` | Core state, business logic, command processing |
| `input.rs` | Keyboard event dispatch to mode handlers |
| `ui.rs` | Full-screen rendering with ratatui |
| `ui_inline.rs` | Inline terminal rendering |
| `message.rs` | Message types with `NonEmptyString` content |
| `theme.rs` | Color palette and style definitions |
| `provider.rs` | LLM provider abstraction and API calls |
| `context_infinity/` | Adaptive context window management |

### Extension Points Summary

| Task | Primary Location | Key Types |
|------|------------------|-----------|
| Add input mode | `app.rs`, `input.rs`, `ui.rs` | `InputMode`, `InputState`, handler function |
| Add command | `app.rs` | `process_command()` match arm |
| Add UI overlay | `ui.rs` | `draw_*` function, `draw()` integration |
| Add provider | `provider.rs`, `app.rs` | `Provider` enum, API implementation |
| Modify theme | `theme.rs` | `colors::`, `styles::` modules |
| Add modal animation | `app.rs`, `effects.rs`, `ui.rs` | `ModalEffect`, `ModalEffectKind`, `apply_modal_effect()` |

The architecture prioritizes correctness, maintainability, and extensibility through Rust's type system rather than runtime checks.

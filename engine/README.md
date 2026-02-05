# forge-engine

This document provides comprehensive documentation for the `forge-engine` crate - the core state machine and orchestration layer for the Forge LLM client. It is intended for developers who want to understand, maintain, or extend the engine functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-40 | Header, Table of Contents |
| 41-114 | Overview: responsibilities, file structure, dependencies |
| 115-149 | Architecture Diagram: App structure with InputState, OperationState, components |
| 150-305 | State Machine Design: OperationState enum, states, transition diagrams |
| 306-387 | Input Mode System: InputState enum, DraftInput, mode transitions |
| 388-492 | Type-Driven Design Patterns: proof tokens, InsertToken, CommandToken, mode wrappers |
| 493-590 | Streaming Orchestration: ActiveStream, StreamingMessage, lifecycle, journal recovery |
| 591-672 | Command System: built-in commands table, rewind details, Command enum |
| 673-735 | Context Management Integration: ContextManager, summarization retry, model switch adaptation |
| 736-915 | Configuration: ForgeConfig structure, loading, env expansion, config sections |
| 916-1066 | Tool Execution System: ToolRegistry, built-in tools, approval workflow, sandbox |
| 1067-1150 | System Notifications: SystemNotification enum, NotificationQueue, security model |
| 1151-1400 | Public API Reference: App lifecycle, state queries, mode transitions, streaming ops |
| 1401-1700 | Extension Guide: adding commands, input modes, providers, async operation states |
| 1701-1850 | Re-exported Types, Error Handling, Thread Safety, Data Directory |

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
10. [Tool Execution System](#tool-execution-system)
11. [System Notifications](#system-notifications)
12. [Public API Reference](#public-api-reference)
13. [Extension Guide](#extension-guide)

---

## Overview

The `forge-engine` crate is the heart of the Forge application - a TUI-agnostic engine that manages LLM conversation state, input modes, streaming responses, tool execution, and adaptive context management. It decouples application logic from terminal UI concerns, enabling the same engine to power different presentation layers.

### Key Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Input Mode State Machine** | Vim-style modal editing (Normal, Insert, Command, ModelSelect, FileSelect) |
| **Async Operation State Machine** | Mutually exclusive states for streaming, tool execution, summarizing, idle |
| **Streaming Management** | Non-blocking LLM response streaming with crash recovery |
| **Tool Execution** | Built-in tools (ReadFile, WriteFile, ApplyPatch, RunCommand, Glob, Search, WebFetch) |
| **Context Infinity** | Adaptive context window management with automatic summarization |
| **Provider Abstraction** | Unified interface for Claude, OpenAI, and Gemini APIs |
| **History Persistence** | Conversation storage and recovery across sessions |

### File Structure

```
engine/
├── Cargo.toml                  # Crate manifest and dependencies
├── README.md                   # This documentation
└── src/
    ├── lib.rs                  # App struct, main module, re-exports
    ├── config.rs               # ForgeConfig parsing (TOML + env expansion)
    ├── state.rs                # OperationState enum and state transitions
    ├── input_modes.rs          # Proof tokens (InsertToken, CommandToken, etc.)
    ├── commands.rs             # Command enum and typed parsing
    ├── checkpoints.rs          # Checkpoint management for rewind/undo
    ├── session_state.rs        # Session state management (turn tracking)
    ├── streaming.rs            # start_streaming, process_stream_events
    ├── summarization.rs        # Summarization task spawn and retry logic
    ├── tool_loop.rs            # Tool execution loop, approval workflow
    ├── init.rs                 # App::new() constructor, tool settings defaults
    ├── errors.rs               # Error formatting, API key redaction
    ├── persistence.rs          # History save/load logic
    ├── security.rs             # Input sanitization and security checks
    ├── notifications.rs        # SystemNotification enum, NotificationQueue
    ├── util.rs                 # Utility functions
    ├── tests.rs                # Integration tests for engine logic
    ├── tools/
    │   ├── mod.rs              # ToolRegistry, ToolExecutor trait, ToolError
    │   ├── builtins.rs         # Built-in tool implementations
    │   ├── git.rs              # Git tool executors (status, diff, commit, etc.)
    │   ├── lp1.rs              # LP1 patch format parser and applier
    │   ├── sandbox.rs          # Sandbox path resolution and enforcement
    │   ├── search.rs           # Search tool (ugrep/ripgrep backend)
    │   ├── shell.rs            # Shell detection and command execution
    │   ├── webfetch.rs         # WebFetch tool for URL fetching
    │   └── recall.rs           # Recall tool for Context Infinity fact queries
    └── ui/
        ├── mod.rs              # UI types re-exports
        ├── display.rs          # DisplayItem enum
        ├── file_picker.rs      # File picker state and filtering
        ├── input.rs            # InputState, DraftInput, InputMode
        ├── modal.rs            # ModalEffect animations
        ├── panel.rs            # Files panel state and effects
        ├── scroll.rs           # ScrollState tracking
        └── view_state.rs       # ViewState, UiOptions for UI state
```

### Dependencies

The engine depends on several workspace crates:

| Crate | Purpose |
|-------|---------|
| `forge-types` | Core domain types (Message, Provider, ModelName, ToolCall, etc.) |
| `forge-context` | Context window management, summarization, persistence |
| `forge-providers` | LLM API clients (Claude, OpenAI, Gemini) |
| `forge-webfetch` | URL fetching for web-based tools |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              App                                         │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                       InputState                                   │  │
│  │   ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────────────┐ ┌───────────┐ │  │
│  │   │  Normal  │ │  Insert  │ │ Command  │ │  ModelSelect    │ │ FileSelect│ │  │
│  │   │ (Draft)  │ │ (Draft)  │ │(Draft,Cmd)│ │(Draft,Selected) │ │(Draft,Filter)││  │
│  │   └──────────┘ └──────────┘ └──────────┘ └─────────────────┘ └───────────┘ │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                     OperationState                                 │  │
│  │   ┌─────────────────────────────────────────────────────────────┐ │  │
│  │   │ Idle | Streaming | ToolLoop | ToolRecovery |                │ │  │
│  │   │ Summarizing | SummarizationRetry                            │ │  │
│  │   └─────────────────────────────────────────────────────────────┘ │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐  │
│  │  ContextManager  │  │  StreamJournal   │  │  ToolJournal         │  │
│  │  (forge-context) │  │  (crash recovery)│  │  (tool recovery)     │  │
│  └──────────────────┘  └──────────────────┘  └──────────────────────┘  │
│                                                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐  │
│  │  ToolRegistry    │  │  Display Items   │  │  ViewState           │  │
│  │  (tool executor) │  │ (Vec<DisplayItem>)│  │  (status, scroll)   │  │
│  └──────────────────┘  └──────────────────┘  └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## State Machine Design

The engine uses explicit state machines to enforce invariants at compile time, making impossible states unrepresentable.

### OperationState - Async Operation State Machine

The `OperationState` enum tracks the current async operation status. Each variant is mutually exclusive:

```rust
pub(crate) enum OperationState {
    Idle,                                           // Ready for new operations
    Streaming(ActiveStream),                        // API response in progress
    ToolLoop(Box<ToolLoopState>),                   // Tool execution in progress (approval + execution)
    ToolRecovery(ToolRecoveryState),                // Crash recovery: pending user decision
    Summarizing(SummarizationState),                // Background summarization (queued: Option<...>)
    SummarizationRetry(SummarizationRetryState),    // Retry after failure (queued: Option<...>)
}
```

### Supporting State Types

```rust
// Active streaming state
struct ActiveStream {
    message: StreamingMessage,           // Accumulating response content
    journal: ActiveJournal,              // RAII handle for crash recovery
    abort_handle: AbortHandle,           // For cancellation
    tool_batch_id: Option<ToolBatchId>,  // Current tool batch (if resuming after tools)
    tool_call_seq: usize,                // Tool call sequence counter
}

// Tool loop sub-states
enum ToolLoopPhase {
    AwaitingApproval(ApprovalState),      // Waiting for user to approve/deny
    Executing(ActiveToolExecution),       // Tools executing sequentially
}

struct ToolLoopState {
    batch: ToolBatch,                     // Calls, results, model info
    phase: ToolLoopPhase,
}

// Crash recovery state
struct ToolRecoveryState {
    batch: RecoveredToolBatch,            // Incomplete batch from crash
    step_id: StepId,                      // Journal step for recovery
    model: ModelName,                     // Model that made the calls
}

// Summarization task
struct SummarizationTask {
    scope: SummarizationScope,                          // Which messages to summarize
    generated_by: String,                               // Model that generated the Distillate
    handle: JoinHandle<anyhow::Result<String>>,         // Async task handle
    attempt: u8,                                        // Retry attempt number
}

// Summarization state
struct SummarizationState {
    task: SummarizationTask,
}
```

### State Transition Diagram

```
                              OperationState
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
    │     └─────┬─────┘        └───────────────┘                             │
    │           │                    │                                        │
    │      tool_calls?           success/failure                              │
    │      ┌────┴────┐               │                                        │
    │      ▼         ▼               v                                        │
    │  ┌────────┐  finish    ┌─────────────────────┐  failure                 │
    │  │ToolLoop│   │        │ (poll_summarization │──────────┐               │
    │  └───┬────┘   │        │  processes result)  │          │               │
    │      │        │        └─────────────────────┘          v               │
    │   approve/    │                                 ┌─────────────────┐     │
    │   deny/done   │                                 │ SummarizationRetry│    │
    │      │        │                                 └─────────────────┘     │
    │      v        v                                         │               │
    │     ┌───────────┐  success                         ready_at reached     │
    │     │   Idle    │<─────────────────────────────────────┘                │
    │     └─────┬─────┘                                                       │
    │           │ or                                                          │
    │           v                                                             │
    │     ┌───────────┐                                                       │
    │     │ Streaming │  (auto-resume after tool results)                     │
    │     └───────────┘                                                       │
    └────────────────────────────────────────────────────────────────────────┘
```

### Tool Loop State Machine

```
    Streaming (tool_calls detected)
        │
        v
    ┌─────────────────────────────────────────────────────────────────────┐
    │                        ToolLoop                                     │
    │  ┌──────────────────────────────────────────────────────────────┐  │
    │  │              AwaitingApproval                                 │  │
    │  │   - Validation complete                                       │  │
    │  │   - Confirmation requests built                               │  │
    │  │   - User reviews tool calls                                   │  │
    │  └────────────────────────┬─────────────────────────────────────┘  │
    │                           │                                         │
    │           ┌───────────────┼───────────────┐                        │
    │           │ ApproveAll    │ DenyAll       │ ApproveSelected        │
    │           v               v               v                        │
    │  ┌────────────────┐  ┌────────────┐  ┌────────────────────┐        │
    │  │   Executing    │  │   commit   │  │     Executing      │        │
    │  │ (all approved) │  │ (errors)   │  │ (partial approval) │        │
    │  └───────┬────────┘  └──────┬─────┘  └─────────┬──────────┘        │
    │          │                  │                  │                   │
    │          │ for each call:   │                  │                   │
    │          │   execute()      │                  │                   │
    │          │   journal result │                  │                   │
    │          v                  v                  v                   │
    │  ┌──────────────────────────────────────────────────────────────┐  │
    │  │                    commit_tool_batch()                        │  │
    │  │   - Persist results to history                                │  │
    │  │   - Commit journal                                            │  │
    │  │   - Auto-resume streaming                                     │  │
    │  └──────────────────────────────────────────────────────────────┘  │
    └─────────────────────────────────────────────────────────────────────┘
        │
        v
    Streaming (LLM continuation with tool results)
```

### Design Rationale

This state machine design provides several guarantees:

| Guarantee | Implementation |
|-----------|----------------|
| **No concurrent streaming** | Only one `Streaming` state can exist |
| **No concurrent tool execution** | Only one `ToolLoop` state can exist |
| **No concurrent summarization** | Summarizing/Retry states are mutually exclusive |
| **Request queueing** | `WithQueued` variants hold a pending request during summarization |
| **Tool batch atomicity** | All tools in a batch are approved/denied together |
| **Clean transitions** | `replace_with_idle()` ensures proper state cleanup |

---

## Input Mode System

The engine implements a vim-style modal editing system with five distinct modes.

### InputState Enum

```rust
pub(crate) enum InputState {
    Normal(DraftInput),                         // Navigation mode
    Insert(DraftInput),                         // Text editing mode
    Command { draft: DraftInput, command: DraftInput }, // Slash command entry
    ModelSelect { draft: DraftInput, selected: usize }, // Model picker overlay
    FileSelect { draft: DraftInput, filter: DraftInput }, // File picker overlay
}
```

Each variant carries `DraftInput` (the message being composed), ensuring it persists across mode transitions.

### DraftInput - Text Buffer with Cursor

```rust
pub struct DraftInput {
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
        │    Normal     │ <- Default mode, navigation
        └───────┬───────┘
                │
    ┌───────────┼───────────┬──────────────┐
    │ Insert    │ Command   │ ModelSelect  │
    │ (i/a/o)   │ (: or /)  │ (m)          │
    v           v           v
┌───────┐   ┌────────┐   ┌─────────────┐
│Insert │   │Command │   │ ModelSelect │
│       │   │        │   │             │
│Draft+ │   │Draft+  │   │Draft+       │
│Cursor │   │CmdStr  │   │SelectedIdx  │
└───┬───┘   └───┬────┘   └──────┬──────┘
    │           │               │
    │ @         │ Esc/Enter     │ Esc/Enter
    v           │               │
┌───────────┐   │               │
│FileSelect │<──┘               │
│Draft+Filt │                   │
└─────┬─────┘                   │
      │ Esc/Enter               │
      └─────────────────────────┘
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
| `enter_file_select_mode()` | Open file picker and start filtering |

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
    turn: TurnContext,
}
```

This type proves:

1. The draft text was validated as non-empty

2. An API key is available for the current provider

3. The `ApiConfig` was successfully constructed

4. A turn context was created for per-turn change tracking

5. The user message was added to history

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
| `/quit` | `/q` | Exit application |
| `/clear` | - | Clear conversation and history |
| `/cancel` | - | Cancel streaming, tool execution, or summarization |
| `/model [name]` | - | Set model or open picker |
| `/context` | `/ctx` | Show context usage stats |
| `/journal` | `/jrnl` | Show journal statistics |
| `/summarize` | `/distill` | Trigger summarization |
| `/screen` | - | Toggle fullscreen/inline mode |
| `/tools` | - | Show tool status |
| `/rewind [id\|last] [scope]` | `/rw` | Rewind to an automatic checkpoint |
| `/undo` | - | Rewind to the latest turn checkpoint (conversation only) |
| `/retry` | - | Rewind to the latest turn checkpoint and restore the prompt into the draft |
| `/help` | - | List available commands |

**Rewind Command Details:**
- `/rewind` or `/rewind list` - Show available checkpoints

- `/rewind last code` - Rewind last checkpoint, restore only code changes

- `/rewind 3 conversation` - Rewind to checkpoint 3, restore only conversation (alias: `chat`)

- `/rewind last both` - Rewind last checkpoint, restore both code and conversation

- Scopes: `code`, `conversation` (or `chat`), `both` (default: `both`)

**Shortcuts:**
- `/undo` - Rewind to the latest turn checkpoint (conversation only)

- `/retry` - Rewind to the latest turn checkpoint and restore the prompt into the draft

### Command Enum

The `Command` enum provides typed command parsing:

```rust
pub(crate) enum Command<'a> {
    Quit,
    Clear,
    Cancel,
    Model(Option<&'a str>),
    Provider(Option<&'a str>),
    Context,
    Journal,
    Summarize,
    Screen,
    Tools,
    Rewind { target: Option<&'a str>, scope: Option<&'a str> },
    Undo,
    Retry,
    Help,
    Unknown(&'a str),
    Empty,
}

impl<'a> Command<'a> {
    pub fn parse(raw: &'a str) -> Self {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        match parts.first().copied() {
            Some("q" | "quit") => Command::Quit,
            Some("clear") => Command::Clear,
            Some("rewind" | "rw") => Command::Rewind {
                target: parts.get(1).copied(),
                scope: parts.get(2).copied(),
            },
            Some("undo") => Command::Undo,
            Some("retry") => Command::Retry,
            // ... etc
        }
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
        self.push_notification("Recent messages exceed budget");
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
    let adaptation = self.context_manager.switch_model(self.model.clone());

    match adaptation {
        ContextAdaptation::NoChange => {}
        ContextAdaptation::Shrinking { needs_distillation: true, .. } => {
            self.push_notification("Context budget shrank; distilling...");
            self.start_distillation();
        }
        ContextAdaptation::Expanding { can_restore, .. } => {
            if can_restore > 0 {
                let restored = self.context_manager.try_restore_messages();
                self.push_notification(format!("Restored {} messages", restored));
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
    pub google: Option<GeminiConfig>,
    pub tools: Option<ToolsConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub model: Option<String>,
    pub tui: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub ascii_only: Option<bool>,
    pub high_contrast: Option<bool>,
    pub reduced_motion: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolsConfig {
    pub max_tool_calls_per_batch: Option<usize>,
    pub max_tool_iterations_per_user_turn: Option<u32>,
    pub definitions: Vec<ToolDefinitionConfig>,
    pub sandbox: Option<ToolSandboxConfig>,
    pub approval: Option<ToolApprovalConfig>,
    // ... specialized tool configs (read_file, search, etc.)
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

### Configuration Sections

#### [app]

```toml
[app]
model = "claude-opus-4-6"  # Provider inferred from prefix
tui = "full"               # or "inline"
max_output_tokens = 16000
ascii_only = false         # ASCII-only glyphs for icons/spinners
high_contrast = false      # High-contrast color palette
reduced_motion = false     # Disable modal animations
```

#### [api_keys]

```toml
[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"
```

#### [context]

```toml
[context]
infinity = true  # Enable Context Infinity
```

#### [anthropic]

```toml
[anthropic]
cache_enabled = true
thinking_enabled = false
thinking_budget_tokens = 10000
```

#### [openai]

```toml
[openai]
reasoning_effort = "high"  # low | medium | high | xhigh
reasoning_Distillate = "auto" # none | auto | concise | detailed (shown when show_thinking=true)
verbosity = "high"         # low | medium | high
truncation = "auto"        # auto | none | preserve
```

#### [google]

```toml
[google]
thinking_enabled = true    # Enable thinking (Gemini 3+)
cache_enabled = true       # Enable context caching
cache_ttl_seconds = 3600   # Cache TTL
```

#### [tools]

```toml
[tools]
max_tool_calls_per_batch = 8
max_tool_iterations_per_user_turn = 4

[tools.approval]
mode = "enabled"           # disabled | parse_only | enabled
```

max_tool_args_bytes = 262144

[tools.sandbox]
allowed_roots = ["."]
denied_patterns = ["**/.git/**"]
allow_absolute = false
include_default_denies = true

[tools.timeouts]
default_seconds = 30
file_operations_seconds = 30
shell_commands_seconds = 300

[tools.output]
max_bytes = 102400

[tools.approval]
enabled = true
mode = "prompt"            # auto | prompt | deny
allowlist = ["read_file"]
denylist = ["run_command"]
prompt_side_effects = true

[tools.read_file]
max_file_read_bytes = 204800
max_scan_bytes = 2097152

[tools.apply_patch]
max_patch_bytes = 524288

[tools.run.windows]
enabled = true
fallback_mode = "prompt"     # prompt | deny | allow_with_warning

[[tools.definitions]]
name = "custom_tool"
description = "A custom tool"
[tools.definitions.parameters]
type = "object"

```

### Configuration Precedence

| Setting | Precedence (highest first) |
|---------|---------------------------|
| API Keys | Config file -> Environment variables |
| Provider | Config file -> Auto-detect from available keys -> Default (Claude) |
| Model | Config file -> Provider default |
| Context Infinity | Config file -> Environment variable -> Default (true) |

---

## Tool Execution System

The engine provides a comprehensive tool execution system with built-in tools, approval workflows, and sandbox enforcement.

### ToolRegistry

The `ToolRegistry` manages tool registration and lookup:

```rust
#[derive(Default)]
pub struct ToolRegistry {
    executors: HashMap<String, Box<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn register(&mut self, executor: Box<dyn ToolExecutor>) -> Result<(), ToolError>;
    pub fn lookup(&self, name: &str) -> Result<&dyn ToolExecutor, ToolError>;
    pub fn definitions(&self) -> Vec<ToolDefinition>;
    pub fn is_empty(&self) -> bool;
}
```

### ToolExecutor Trait

All tools implement the `ToolExecutor` trait:

```rust
pub trait ToolExecutor: Send + Sync + std::panic::UnwindSafe {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn is_side_effecting(&self) -> bool;
    fn requires_approval(&self) -> bool { false }
    fn risk_level(&self) -> RiskLevel {
        if self.is_side_effecting() {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
    fn approval_Distillate(&self, args: &Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> { None }
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}
```

### Built-in Tools

#### Core Tools

| Tool | Description | Side Effects |
|------|-------------|--------------|
| `read_file` | Read file contents with optional line range | No |
| `write_file` | Write content to a new file (fails if exists) | Yes |
| `apply_patch` | Apply LP1 patches to files | Yes |
| `run_command` | Execute shell commands | Yes |
| `Glob` | Find files matching glob patterns | No |
| `Search` | Search file contents with regex (aliases: `search`, `rg`, `ripgrep`, `ugrep`, `ug`) | No |
| `WebFetch` | Fetch and parse web page content | No |
| `recall` | Query Librarian fact store for past context (Context Infinity) | No |

#### Git Tools

| Tool | Description | Side Effects |
|------|-------------|--------------|
| `git_status` | Show working tree status | No |
| `git_diff` | Show file changes in working tree or staging area | No |
| `git_log` | Show commit history | No |
| `git_show` | Show commit details and diff | No |
| `git_blame` | Show revision and author for each line | No |
| `git_add` | Stage files for commit | Yes |
| `git_commit` | Create a conventional commit | Yes |
| `git_branch` | List, create, rename, or delete branches | Yes |
| `git_checkout` | Switch branches or restore files | Yes |
| `git_restore` | Discard uncommitted changes (destructive) | Yes |
| `git_stash` | Stash changes in working directory | Yes |

### Tool Approval Workflow

The approval system provides three modes:

```rust
pub enum ApprovalMode {
    Permissive,  // Auto-approve most tools, only prompt for high-risk
    Default,     // Prompt for side-effecting tools unless allowlisted
    Strict,      // Deny all tools unless explicitly allowlisted
}
```

Approval policy:

```rust
pub struct Policy {
    pub mode: ApprovalMode,
    pub allowlist: HashSet<String>,  // Tools that skip approval
    pub denylist: HashSet<String>,   // Tools always denied
}
```

### Sandbox Enforcement

The sandbox restricts tool access to authorized paths:

```rust
pub struct SandboxConfig {
    pub allowed_roots: Vec<PathBuf>,    // Permitted directories
    pub denied_patterns: Vec<String>,   // Glob patterns to block
    pub allow_absolute: bool,           // Allow absolute paths
    pub include_default_denies: bool,   // Include .git, node_modules, etc.
}
```

Default denied patterns:

- `**/.git/**` - Git internals
- `**/node_modules/**` - Node dependencies
- `**/.env*` - Environment files
- `**/secrets/**` - Secret directories

### ToolError Enum

```rust
pub enum ToolError {
    BadArgs { message: String },
    Timeout { tool: String, elapsed: Duration },
    SandboxViolation(DenialReason),
    ExecutionFailed { tool: String, message: String },
    Cancelled,
    UnknownTool { name: String },
    DuplicateTool { name: String },
    DuplicateToolCallId { id: String },
    PatchFailed { file: PathBuf, message: String },
    StaleFile { file: PathBuf, reason: String },
}
```

### Tool Execution Flow

```
1. LLM response contains tool_calls
2. Enter ToolLoop(AwaitingApproval) state
3. Display tool calls for user review
4. User approves/denies (or auto based on config)
5. Execute approved tools sequentially
6. Journal each result for crash recovery
7. Commit batch to history
8. Auto-resume streaming with tool results
```

---

## System Notifications

The engine provides a secure notification system for trusted agent-to-model communication. Notifications are injected as assistant messages, which cannot be forged by user input, files, or tool outputs.

### Security Model

Unlike system reminders embedded in user content (which are vulnerable to prompt injection), assistant messages can only come from:

1. **API responses** - Messages generated by the LLM
2. **Forge's injection layer** - This notification module

This creates a clean trust boundary: the small, finite set of notification variants defined in the `SystemNotification` enum are trusted; everything else is untrusted.

### SystemNotification Enum

```rust
/// A system notification that Forge can inject into the conversation.
///
/// This is a closed enum - only Forge code can construct these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemNotification {
    /// Context was Distilled to fit within token budget.
    ContextDistilled,
    /// Session was recovered after a crash.
    SessionRecovered,
    /// User approved tool calls.
    ToolsApproved { count: u8 },
    /// User denied tool calls.
    ToolsDenied { count: u8 },
    /// Context budget is running low.
    ContextBudgetWarning,
    /// Model was switched during the session.
    ModelSwitched,
}
```

Each variant represents a specific system event that the model should be aware of. The enum is intentionally closed - only Forge code can construct these variants, preventing injection attacks.

### NotificationQueue

The `NotificationQueue` manages pending notifications:

```rust
pub struct NotificationQueue {
    pending: Vec<SystemNotification>,
}

impl NotificationQueue {
    /// Create a new empty notification queue.
    pub fn new() -> Self;

    /// Push a notification to the queue.
    /// Duplicate notifications are deduplicated to avoid redundant messages.
    pub fn push(&mut self, notification: SystemNotification);

    /// Take all pending notifications, clearing the queue.
    pub fn take(&mut self) -> Vec<SystemNotification>;

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool;

    /// Get the number of pending notifications.
    pub fn len(&self) -> usize;
}
```

### Notification Formatting

All notifications are prefixed with `[System: ...]` to clearly mark them as system-level messages distinct from user or model content:

| Notification | Formatted Output |
|--------------|------------------|
| `ContextDistilled` | `[System: Earlier messages were Distilled to fit context budget]` |
| `SessionRecovered` | `[System: Session recovered after unexpected termination]` |
| `ToolsApproved { count: 3 }` | `[System: User approved 3 tool call(s)]` |
| `ToolsDenied { count: 1 }` | `[System: User denied 1 tool call(s)]` |
| `ContextBudgetWarning` | `[System: Context budget running low, consider summarizing]` |
| `ModelSwitched` | `[System: Model was switched during session]` |

### Injection Mechanism

Notifications are injected into the API request at the start of streaming:

```rust
// In start_streaming():
// 1. Prepare the API messages from context
let api_messages = self.context_manager.prepare()?.api_messages();

// 2. Convert to cacheable format
let cacheable_messages = /* ... */;

// 3. Inject pending notifications as an assistant message
let cacheable_messages = self.inject_pending_notifications(cacheable_messages);

// 4. Send to provider
forge_providers::send_message(&config, &cacheable_messages, /* ... */);
```

The `inject_pending_notifications` method:

1. Takes all queued notifications via `notification_queue.take()`
2. Formats each notification using `SystemNotification::format()`
3. Combines them into a single string (newline-separated)
4. Appends as an assistant message at the **tail** of the message list

**Cache impact**: Injection at the tail preserves the cache prefix, ensuring previously cached context remains valid.

### Usage Pattern

Queue notifications in response to system events:

```rust
// Model switch
self.queue_notification(SystemNotification::ModelSwitched);

// Tool approval
self.queue_notification(SystemNotification::ToolsApproved { count: 3 });

// Session recovery
self.queue_notification(SystemNotification::SessionRecovered);

// Summarization complete
self.queue_notification(SystemNotification::ContextDistilled);
```

The notifications are automatically drained and injected on the next API request.

### Design Rationale

| Aspect | Implementation |
|--------|----------------|
| **Trust boundary** | Assistant messages are unforgeable by users |
| **Deduplication** | Same notification pushed twice only appears once |
| **Batch injection** | Multiple notifications combined into single message |
| **Cache-safe** | Appended at tail, preserving cache prefix |
| **Extensibility** | Add new variants to `SystemNotification` enum |

---

## Public API Reference

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
    Command,     // Slash command entry (e.g., /quit)
    ModelSelect, // Model picker overlay
    FileSelect,  // File picker overlay
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

#### Notifications

Notifications are local system messages pushed with `push_notification()`. They render
in the content pane and are not sent to the model.

#### `PredefinedModel`

```rust
pub enum PredefinedModel {
    ClaudeOpus,
    Gpt52Pro,
    Gpt52,
    GeminiPro,
    GeminiFlash,
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
| `is_in_tool_loop()` | `bool` | Whether in tool execution |
| `is_awaiting_tool_approval()` | `bool` | Whether waiting for tool approval |
| `pending_tool_calls()` | `Option<&[ToolCall]>` | Get pending tool calls |

### Mode Transitions

| Method | Description |
|--------|-------------|
| `enter_normal_mode()` | Switch to Normal mode |
| `enter_insert_mode()` | Switch to Insert mode |
| `enter_insert_mode_at_end()` | Insert mode, cursor at end |
| `enter_insert_mode_with_clear()` | Insert mode, clear draft |
| `enter_command_mode()` | Switch to Command mode |
| `enter_model_select_mode()` | Open model picker |
| `enter_file_select_mode()` | Open file picker |
| `insert_token()` | Get proof token for Insert mode |
| `command_token()` | Get proof token for Command mode |
| `insert_mode(token)` | Get InsertMode wrapper |
| `command_mode(token)` | Get CommandMode wrapper |

### Streaming Operations

| Method | Description |
|--------|-------------|
| `start_streaming(queued)` | Begin API request |
| `process_stream_events()` | Apply pending stream chunks |

### Tool Approval Operations

| Method | Description |
|--------|-------------|
| `approve_all_tools()` | Approve all pending tool calls |
| `deny_all_tools()` | Deny all pending tool calls |
| `approve_selected_tools(indices)` | Approve specific tool calls |

### Model Management

| Method | Description |
|--------|-------------|
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

1. **Add command variant to `Command` enum** (`engine/src/commands.rs`):

```rust
// Command uses a lifetime parameter for efficiency (borrows from input)
pub(crate) enum Command<'a> {
    // ... existing commands ...
    MyCommand(Option<&'a str>),
}

impl<'a> Command<'a> {
    pub fn parse(raw: &'a str) -> Self {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        match parts.first().copied() {
            // ... existing matches ...
            Some("mycommand" | "mc") => {
                Command::MyCommand(parts.get(1).copied())
            }
            Some(cmd) => Command::Unknown(cmd),
            None => Command::Empty,
        }
    }
}
```

1. **Handle command in `process_command()`** (`engine/src/lib.rs`):

```rust
pub fn process_command(&mut self, command: EnteredCommand) {
    match Command::parse(&command.raw) {
        // ... existing handlers ...
        Command::MyCommand(arg) => {
            if let Some(value) = arg {
                self.push_notification(format!("MyCommand executed with: {value}"));
            } else {
                self.push_notification("Usage: :mycommand <arg>");
            }
        }
    }
}
```

1. **Update help text**:

```rust
Command::Help => {
    self.push_notification(
        "Commands: /q(uit), /clear, /mycommand, ..."  // Add new command
    );
}
```

### Adding a New Input Mode

1. **Extend `InputState` enum** (`engine/src/ui/input.rs`):

```rust
pub(crate) enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: DraftInput },
    ModelSelect { draft: DraftInput, selected: usize },
    FileSelect { draft: DraftInput, filter: DraftInput },
    MyMode { draft: DraftInput, custom_state: MyState },  // New mode
}
```

1. **Add mode enum variant** (`engine/src/input_modes.rs`):

```rust
pub enum InputMode {
    Normal,
    Insert,
    Command,
    ModelSelect,
    FileSelect,
    MyMode,  // New mode
}
```

1. **Add transition method**:

```rust
impl InputState {
    pub(crate) fn into_my_mode(self) -> InputState {
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

1. **Add proof token pattern** (optional):

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

1. **Handle in TUI input handler** (`tui/src/input.rs`):

```rust
pub async fn handle_events(app: &mut App) -> Result<bool> {
    match app.input_mode() {
        InputMode::Normal => handle_normal_mode(app),
        InputMode::Insert => handle_insert_mode(app),
        InputMode::Command => handle_command_mode(app),
        InputMode::ModelSelect => handle_model_select_mode(app),
        InputMode::FileSelect => handle_file_select_mode(app),
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
    Gemini,
    MyProvider,  // New provider
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Self::Claude),
            "openai" | "gpt" => Some(Self::OpenAI),
            "gemini" | "google" => Some(Self::Gemini),
            "myprovider" | "mp" => Some(Self::MyProvider),  // New parsing
            _ => None,
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            Self::Claude => ModelName::from_predefined(PredefinedModel::ClaudeOpus),
            Self::OpenAI => ModelName::from_predefined(PredefinedModel::Gpt52),
            Self::Gemini => ModelName::from_predefined(PredefinedModel::GeminiPro),
            Self::MyProvider => ModelName::from_predefined(PredefinedModel::MyProviderModel),
        }
    }
}
```

1. **Add API client** (`providers/src/my_provider.rs`)

2. **Update config structure** (`engine/src/config.rs`):

```rust
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub my_provider: Option<String>,  // New key
}
```

1. **Update key loading in `App::new()`** (`engine/src/init.rs`)

### Adding a New Built-in Tool

1. **Create tool executor** (`engine/src/tools/my_tool.rs`):

```rust
pub struct MyTool;

impl ToolExecutor for MyTool {
    fn name(&self) -> &'static str { "my_tool" }

    fn description(&self) -> &'static str {
        "Does something useful"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "arg1": { "type": "string", "description": "First argument" }
            },
            "required": ["arg1"]
        })
    }

    fn is_side_effecting(&self) -> bool { false }

    fn approval_Distillate(&self, args: &Value) -> Result<String, ToolError> {
        let arg1 = args["arg1"].as_str().unwrap_or("?");
        Ok(format!("Run my_tool with arg: {arg1}"))
    }

    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let arg1 = args["arg1"].as_str()
                .ok_or_else(|| ToolError::BadArgs {
                    message: "arg1 required".into()
                })?;

            // Do work...

            Ok("Result".to_string())
        })
    }
}
```

1. **Register in `App::new()`** (`engine/src/init.rs`):

Tool registration occurs during application initialization. Add your tool to the registry setup in `init.rs`:

```rust
// In App::new() initialization
registry.register(Box::new(MyTool))?;
```

### Adding a New Async Operation State

1. **Add new state variant** (`engine/src/state.rs`):

```rust
pub(crate) enum OperationState {
    Idle,
    Streaming(ActiveStream),
    Summarizing(SummarizationState),
    // ... existing states ...
    MyOperation(MyOperationState),  // New operation
}
```

1. **Add state transition guards**:

```rust
pub fn start_my_operation(&mut self) {
    match &self.state {
        OperationState::Idle => {
            // Start operation
            self.state = OperationState::MyOperation(state);
        }
        _ => {
            self.push_notification("Cannot start: busy with other operation");
        }
    }
}
```

1. **Add polling in `tick()`**:

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
| `ToolJournal` | WAL for tool execution recovery |
| `ModelLimits` | Token limits for a model |
| `TokenCounter` | Token counting utilities |

### From `forge-providers`

| Type | Description |
|------|-------------|
| `ApiConfig` | API request configuration |

### From `forge-types`

| Type | Description |
|------|-------------|
| `Provider` | LLM provider enum (Claude, OpenAI, Gemini) |
| `ModelName` | Provider-scoped model identifier |
| `Message` | User/Assistant/System message |
| `NonEmptyString` | Guaranteed non-empty string |
| `ApiKey` | Provider-specific API key |
| `StreamEvent` | Streaming response events |
| `StreamFinishReason` | How streaming ended |
| `OutputLimits` | Max tokens and thinking budget |
| `ToolCall` | Tool invocation from LLM |
| `ToolResult` | Result of tool execution |
| `ToolDefinition` | Tool schema for API |

### Defined in `forge-engine`

| Type | Description |
|------|-------------|
| `SystemNotification` | Secure system-to-model notification variants |
| `StreamingMessage` | In-flight streaming response with accumulated content |
| `QueuedUserMessage` | Proof token that user message is validated and ready |
| `EnteredCommand` | Proof token that command was entered in command mode |
| `InsertToken` | Proof token for Insert mode operations |
| `CommandToken` | Proof token for Command mode operations |
| `InsertMode<'a>` | Safe wrapper for insert mode operations |
| `CommandMode<'a>` | Safe wrapper for command mode operations |
| `TurnUsage` | Aggregated API usage for a user turn |

---

## Error Handling

### API Error Formatting

The engine formats API errors with context-aware messages:

```rust
fn format_stream_error(provider: Provider, model: &str, err: &str) -> NonEmptyString {
    // Detects auth errors and provides actionable guidance
    if is_auth_error(&extracted) {
        return NonEmptyString::new(format!(
            "[Stream error]\n\n{} authentication failed for model {}.",
            provider.display_name(),
            model
        ))
        .unwrap();
    }

    // Generic error formatting with truncation
    NonEmptyString::new(format!(
        "[Stream error]\n\nRequest failed. Details: {}",
        detail
    ))
    .unwrap()
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

### Tool Errors

```rust
pub enum ToolError {
    BadArgs { message: String },
    Timeout { tool: String, elapsed: Duration },
    SandboxViolation(DenialReason),
    ExecutionFailed { tool: String, message: String },
    Cancelled,
    UnknownTool { name: String },
    DuplicateTool { name: String },
    DuplicateToolCallId { id: String },
    PatchFailed { file: PathBuf, message: String },
    StaleFile { file: PathBuf, reason: String },
}
```

### Summarization Retry

Failed summarizations are retried with exponential backoff:

- Base delay: 500ms
- Max delay: 8000ms
- Jitter: 0-200ms
- Max attempts: 5

---

## Thread Safety

The `App` struct is not thread-safe and should be used from a single async task. Background operations (summarization, streaming, tool execution) are spawned as separate Tokio tasks that communicate via channels:

- **Streaming**: `mpsc::unbounded_channel()` for `StreamEvent` delivery
- **Summarization**: `tokio::task::JoinHandle` polled via `is_finished()`
- **Tool execution**: Sequential execution with journal persistence
- **Cancellation**: `AbortHandle` for graceful task termination

---

## Data Directory

The engine stores persistent data in the OS local data directory (from
`dirs::data_local_dir()`), under a `forge/` subfolder. If no system data
directory is available, it falls back to `./forge/`.

Config remains in the home directory: `~/.forge/config.toml`.

| Path | Purpose |
|------|---------|
| `<data_dir>/history.json` | Conversation history (JSON) |
| `<data_dir>/session.json` | Draft input and input history |
| `<data_dir>/stream_journal.db` | WAL for stream crash recovery |
| `<data_dir>/tool_journal.db` | WAL for tool execution recovery |
| `<data_dir>/librarian.db` | Librarian fact store (when enabled) |

All database files use SQLite with WAL mode for durability.

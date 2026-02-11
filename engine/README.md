# forge-engine

This document provides comprehensive documentation for the `forge-engine` crate - the core state machine and orchestration layer for the Forge LLM client. It is intended for developers who want to understand, maintain, or extend the engine functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
| :--- | :--- |
| 1-40 | Header, Table of Contents |
| 41-120 | Overview: responsibilities, file structure, dependencies |
| 121-155 | Architecture Diagram: App structure with InputState, OperationState, components |
| 156-330 | State Machine Design: OperationState enum, ActiveStream typestate, state transitions |
| 331-412 | Input Mode System: InputState enum, DraftInput, mode transitions |
| 413-525 | Type-Driven Design Patterns: proof tokens, InsertToken, CommandToken, mode wrappers |
| 526-640 | Streaming Orchestration: ActiveStream typestate, StreamingMessage, lifecycle |
| 641-730 | Command System: built-in commands table, tab completion, Command enum |
| 731-800 | Context Management Integration: ContextManager, distillation, model switch adaptation |
| 801-1020 | Configuration: ForgeConfig structure, loading, env expansion, config sections |
| 1021-1190 | Tool Execution System: ToolRegistry, built-in tools, approval workflow, sandbox |
| 1191-1310 | System Notifications: SystemNotification enum, NotificationQueue, security model |
| 1311-1370 | LSP Integration: compiler diagnostics feedback loop |
| 1371-1430 | Files Panel: session change tracking, diff display, panel animations |
| 1431-1460 | Input History: prompt and command recall with Up/Down navigation |
| 1461-1730 | Public API Reference: App lifecycle, state queries, mode transitions, streaming ops |
| 1731-2050 | Extension Guide: adding commands, input modes, providers, async operation states |
| 2051-2220 | Re-exported Types, Error Handling, Thread Safety, Data Directory |

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
12. [LSP Integration](#lsp-integration)
13. [Files Panel](#files-panel)
14. [Input History](#input-history)
15. [Public API Reference](#public-api-reference)
16. [Extension Guide](#extension-guide)

---

## Overview

The `forge-engine` crate is the heart of the Forge application - a TUI-agnostic engine that manages LLM conversation state, input modes, streaming responses, tool execution, and adaptive context management. It decouples application logic from terminal UI concerns, enabling the same engine to power different presentation layers.

### Key Responsibilities

| Responsibility | Description |
| :--- | :--- |
| **Input Mode State Machine** | Vim-style modal editing (Normal, Insert, Command, ModelSelect, FileSelect) |
| **Async Operation State Machine** | Mutually exclusive states for streaming, tool execution, distilling, idle |
| **Streaming Management** | Non-blocking LLM response streaming with crash recovery |
| **Tool Execution** | Built-in tools (Read, Write, Edit, Run, Glob, Search, WebFetch, Recall, Memory) |
| **Context Infinity** | Adaptive context window management with automatic distillation |
| **Provider Abstraction** | Unified interface for Claude, OpenAI, and Gemini APIs |
| **History Persistence** | Conversation storage and recovery across sessions |
| **LSP Integration** | Compiler diagnostics feedback via Language Server Protocol |
| **Files Panel** | Session-wide file change tracking with inline diff display |

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
    ├── commands.rs             # Command enum, CommandSpec, tab completion
    ├── checkpoints.rs          # Checkpoint management for rewind/undo
    ├── session_state.rs        # SessionChangeLog, session persistence
    ├── streaming.rs            # start_streaming, process_stream_events
    ├── distillation.rs         # Distillation task spawn and polling
    ├── tool_loop.rs            # Tool execution loop, approval workflow
    ├── init.rs                 # App::new() constructor, tool settings defaults
    ├── errors.rs               # Error formatting, API key redaction
    ├── persistence.rs          # History save/load logic
    ├── security.rs             # Input sanitization and security checks
    ├── notifications.rs        # SystemNotification enum, NotificationQueue
    ├── lsp_integration.rs      # LSP event polling, diagnostics injection
    ├── util.rs                 # Utility functions
    ├── tests.rs                # Integration tests for engine logic
    └── ui/
        ├── mod.rs              # UI types re-exports
        ├── display.rs          # DisplayItem enum
        ├── file_picker.rs      # File picker state and filtering
        ├── history.rs          # InputHistory for prompt/command recall
        ├── input.rs            # InputState, DraftInput, InputMode
        ├── modal.rs            # ModalEffect animations (PopScale, SlideUp, Shake)
        ├── panel.rs            # PanelEffect animations (SlideInRight, SlideOutRight)
        ├── scroll.rs           # ScrollState tracking
        └── view_state.rs       # ViewState, UiOptions, FilesPanelState, ChangeKind
```

> **Note:** Tool executors, sandboxing, and the `ToolRegistry` live in the `forge-tools` crate (`tools/src/`). `forge-engine` re-exports it as `crate::tools` (`pub use forge_tools as tools;`) and registers built-ins via `builtins::register_builtins()` in `engine/src/init.rs`.

### Dependencies

The engine depends on several workspace crates:

| Crate | Purpose |
| :--- | :--- |
| `forge-types` | Core domain types (Message, Provider, ModelName, ToolCall, etc.) |
| `forge-context` | Context window management, distillation, persistence |
| `forge-providers` | LLM API clients (Claude, OpenAI, Gemini) |
| `forge-webfetch` | URL fetching for web-based tools |
| `forge-lsp` | Language Server Protocol client for compiler diagnostics |

Platform-specific dependencies:

| Dependency | Platform | Purpose |
| :--- | :--- | :--- |
| `libc` | Unix | File permission checks for secure config handling |
| `windows-sys` | Windows | Job Object API for Run tool process isolation |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              App                                         │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                       InputState                                   │  │
│  │   ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────────────┐ ┌───────────┐ │  │
│  │   │  Normal  │ │  Insert  │ │ Command  │ │  ModelSelect    │ │ FileSelect│ │  │
│  │   │ (Draft)  │ │ (Draft)  │ │(Draft,Cmd)│ │(Draft,Selected) │ │(Draft,Filt)│ │  │
│  │   └──────────┘ └──────────┘ └──────────┘ └─────────────────┘ └───────────┘ │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                     OperationState                                 │  │
│  │   ┌─────────────────────────────────────────────────────────────┐ │  │
│  │   │ Idle | Streaming(ActiveStream) | ToolLoop | ToolRecovery | │ │  │
│  │   │ Distilling(DistillationState)                               │ │  │
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
│                                                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐  │
│  │  LSP Manager     │  │  SessionChanges  │  │  InputHistory        │  │
│  │  (forge-lsp)     │  │  (file tracking) │  │  (prompt/cmd recall) │  │
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
    Distilling(DistillationState),                  // Background distillation
}
```

### ActiveStream - Typestate Encoding for Journal Status

The `ActiveStream` type uses a typestate pattern to track whether tool call journaling is active. A stream begins as `Transient` (no tool calls detected yet) and transitions irreversibly to `Journaled` when the first tool call is detected:

```rust
pub(crate) enum ActiveStream {
    /// Stream without tool call journaling (no tool calls yet).
    Transient {
        message: StreamingMessage,
        journal: ActiveJournal,
        abort_handle: AbortHandle,
        tool_call_seq: usize,
        tool_args_journal_bytes: HashMap<String, usize>,
        turn: TurnContext,
    },
    /// Stream with tool call journaling active (crash-recoverable).
    Journaled {
        tool_batch_id: ToolBatchId,
        message: StreamingMessage,
        journal: ActiveJournal,
        abort_handle: AbortHandle,
        tool_call_seq: usize,
        tool_args_journal_bytes: HashMap<String, usize>,
        turn: TurnContext,
    },
}
```

Accessor methods (`message()`, `journal()`, `abort_handle()`, etc.) work on both variants, while the `tool_batch_id` field is only available in the `Journaled` variant.

### Supporting State Types

```rust
// Distillation task
struct DistillationTask {
    scope: DistillationScope,                          // Which messages to distill
    generated_by: String,                               // Model that generated the distillate
    handle: JoinHandle<anyhow::Result<String>>,         // Async task handle
}

// Distillation state with typestate encoding for message queueing
enum DistillationState {
    Running(DistillationTask),
    CompletedWithQueued {
        task: DistillationTask,
        message: QueuedUserMessage,
    },
}

// Tool batch (unit of execution)
struct ToolBatch {
    assistant_text: String,
    thinking_message: Option<Message>,
    calls: Vec<ToolCall>,
    results: Vec<ToolResult>,
    model: ModelName,
    step_id: StepId,
    journal_status: JournalStatus,
    execute_now: Vec<ToolCall>,       // Safe tools (auto-approved)
    approval_calls: Vec<ToolCall>,    // Dangerous tools (need approval)
    turn: TurnContext,
}

// Tool loop sub-states
enum ToolLoopPhase {
    AwaitingApproval(ApprovalState),  // Waiting for user to approve/deny
    Processing(ToolQueue),            // Between tools or before first spawn
    Executing(ActiveExecution),       // Tool actively running
}

// Approval workflow with deny confirmation
enum ApprovalState {
    Selecting(ApprovalData),          // User is selecting which tools to approve
    ConfirmingDeny(ApprovalData),     // User pressed 'd'; awaiting second press to confirm
}

// Crash recovery state
struct ToolRecoveryState {
    batch: RecoveredToolBatch,        // Incomplete batch from crash
    step_id: StepId,                  // Journal step for recovery
    model: ModelName,                 // Model that made the calls
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
    │      start_streaming()   start_distillation()    queue_message()        │
    │           │                    │               (distillation needed)    │
    │           v                    v                     │                  │
    │     ┌───────────┐        ┌───────────────┐          │                  │
    │     │ Streaming │        │  Distilling   │<─────────┘                  │
    │     └─────┬─────┘        └───────────────┘                             │
    │           │                    │                                        │
    │      tool_calls?           success/failure                              │
    │      ┌────┴────┐               │                                        │
    │      ▼         ▼               v                                        │
    │  ┌────────┐  finish    ┌─────────────────────┐                          │
    │  │ToolLoop│   │        │  poll_distillation() │                          │
    │  └───┬────┘   │        │  processes result    │                          │
    │      │        │        └─────────────────────┘                          │
    │   approve/    │                                                         │
    │   deny/done   │                                                         │
    │      │        │                                                         │
    │      v        v                                                         │
    │     ┌───────────┐                                                       │
    │     │   Idle    │                                                       │
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
    │  │  Processing    │  │   commit   │  │    Processing      │        │
    │  │   (queue)      │  │ (errors)   │  │   (partial queue)  │        │
    │  └───────┬────────┘  └──────┬─────┘  └─────────┬──────────┘        │
    │          │                  │                  │                   │
    │          │ spawn_next_tool  │                  │                   │
    │          v                  │                  v                   │
    │  ┌────────────────┐        │          ┌────────────────┐           │
    │  │   Executing    │        │          │   Executing    │           │
    │  │ (active tool)  │        │          │ (active tool)  │           │
    │  └───────┬────────┘        │          └───────┬────────┘           │
    │          │ tool completes  │                  │                   │
    │          v                 │                  v                   │
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
| :--- | :--- |
| **No concurrent streaming** | Only one `Streaming` state can exist |
| **No concurrent tool execution** | Only one `ToolLoop` state can exist |
| **No concurrent distillation** | `Distilling` state is mutually exclusive |
| **Request queueing** | `CompletedWithQueued` holds a pending request during distillation |
| **Tool batch gating & commit** | Tool execution is gated behind an approval decision; per-call selection is supported (`ApproveSelected`), and calls+results are committed as a contiguous block (`commit_tool_batch()`) |
| **Clean transitions** | `replace_with_idle()` ensures proper state cleanup |
| **Journal typestate** | `ActiveStream::Transient` vs `Journaled` enforced at type level |
| **Deny confirmation** | `ApprovalState` requires double-press for deny (prevents accidental denial) |

---

## Input Mode System

The engine implements a vim-style modal editing system with five distinct modes.

### InputState Enum

```rust
pub(crate) enum InputState {
    Normal(DraftInput),                                     // Navigation mode
    Insert(DraftInput),                                     // Text editing mode
    Command { draft: DraftInput, command: DraftInput },     // Slash command entry
    ModelSelect { draft: DraftInput, selected: usize },     // Model picker overlay
    FileSelect { draft: DraftInput, filter: DraftInput, selected: usize }, // File picker overlay
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
        .map_or(self.text.len(), |(i, _)| i)
}
```

Key `DraftInput` methods:

| Method | Description |
| :--- | :--- |
| `enter_char(c)` | Insert character at cursor position |
| `enter_newline()` | Insert newline at cursor position |
| `enter_text(s)` | Insert multi-character string at cursor |
| `delete_char()` | Delete character before cursor (backspace) |
| `delete_char_forward()` | Delete character after cursor (delete) |
| `delete_word_backwards()` | Delete previous word (Ctrl+W) |
| `set_text(s)` | Replace text and move cursor to end |
| `clear()` | Clear text and reset cursor |

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
│+Selected  │                   │
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
| `enter_file_select_mode()` | Open file picker, scan files from cwd |

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
    pub fn enter_newline(&mut self);
    pub fn enter_text(&mut self, text: &str);
    pub fn delete_char(&mut self);
    pub fn delete_char_forward(&mut self);
    pub fn delete_word_backwards(&mut self);
    pub fn clear_line(&mut self);
    pub fn move_cursor_left(&mut self);
    pub fn move_cursor_right(&mut self);
    pub fn queue_message(self) -> Option<QueuedUserMessage>; // Consumes self
}
```

```rust
/// Mode wrapper for safe command operations.
pub struct CommandMode<'a> {
    app: &'a mut App,
}

impl<'a> CommandMode<'a> {
    pub fn push_char(&mut self, c: char);
    pub fn delete_char(&mut self);
    pub fn move_cursor_left(&mut self);
    pub fn move_cursor_right(&mut self);
    pub fn tab_complete(&mut self);  // Shell-style tab completion
    pub fn take_command(self) -> Option<EnteredCommand>; // Consumes self
}
```

### QueuedUserMessage - Message Validation Proof

```rust
/// Proof that a non-empty user message was queued.
///
/// The `config` captures the model/provider at queue time. If distillation runs
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

### ActiveStream - Typestate for Journal Status

As described in the State Machine Design section, `ActiveStream` uses a typestate pattern:

- **Transient**: No tool calls detected yet. Stream content is journaled for crash recovery, but no tool batch has been opened in the `ToolJournal`.
- **Journaled**: At least one tool call was detected. A `ToolBatchId` is allocated and tool calls are persisted to the `ToolJournal` for crash recovery.

The transition from `Transient` to `Journaled` is irreversible:

```rust
impl ActiveStream {
    pub(crate) fn transition_to_journaled(self, batch_id: ToolBatchId) -> Self;
}
```

### StreamingMessage - Accumulating Response

```rust
/// A message being streamed - existence proves streaming is active.
pub struct StreamingMessage {
    model: ModelName,
    content: String,            // Accumulated response text
    thinking: String,           // Accumulated thinking/reasoning text
    thinking_signature: ThoughtSignatureState,  // Encrypted thinking signature (Claude)
    receiver: mpsc::Receiver<StreamEvent>,
    tool_calls: Vec<ToolCallAccumulator>,
    max_tool_args_bytes: usize,
    usage: ApiUsage,            // API-reported token usage
}
```

The `StreamingMessage` provides:

- Type-level proof that streaming is active (ownership semantics)
- Event-based content accumulation via channel
- Tool call argument accumulation with size limits
- Thinking content capture (always captured, UI controls visibility)
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

The `process_stream_events()` method processes up to `DEFAULT_STREAM_EVENT_BUDGET` (512) events per call to avoid starving the render loop. Consecutive `TextDelta` and `ThinkingDelta` events are coalesced into single appends for efficiency.

Terminal control sequences in model output are sanitized via `sanitize_terminal_text()` before display.

### Journal-Based Crash Recovery

Forge keeps two independent journals so a crash can be recovered without duplicating history:

- **Stream journal** (`forge_context::StreamJournal`): records streaming deltas keyed by `StepId`.
- **Tool journal** (`forge_context::ToolJournal`): records in-flight tool batches keyed by `ToolBatchId`.

The entrypoint is `App::check_crash_recovery()` (`engine/src/persistence.rs`). Recovery is step-id aware (idempotent) and only prunes journals after recovered history is persisted.

**Tool batch recovery takes priority:**

1. `tool_journal.recover()` is attempted first.
2. If a batch is found, the engine also attempts `stream_journal.recover()` to retrieve a `StepId` and any partial assistant text.
3. If the tool batch assistant text is empty but the stream journal has partial text, the engine backfills the batch text for the recovery UI.
4. If `ContextManager::has_step_id(step_id)` is already true, the tool batch is discarded; stream cleanup is only finalized if `autosave_history()` succeeds (otherwise the journals remain recoverable).
5. Otherwise the engine enters `OperationState::ToolRecovery` and prompts the user to resume (`tool_recovery_resume`) or discard (`tool_recovery_discard`).

**Stream-only recovery:**

1. If no tool batch is pending, `stream_journal.recover()` reconstructs a partial assistant response.
2. The engine prepends a recovery badge and sanitizes recovered text (`sanitize_terminal_text`) before pushing it into history with the recovered `StepId`.
3. Sealing (`seal_unsealed`) and pruning (`finalize_journal_commit`) only occur after `autosave_history()` succeeds.

**Deferred cleanup:**

Cleanup failures are retried via `poll_journal_cleanup()` while `OperationState::Idle` using `pending_stream_cleanup` and `pending_tool_cleanup`.

### Cache Breakpoint Strategy

For Claude prompt caching, the engine places cache breakpoints using a geometric grid strategy to balance cache hit rates against the 4-breakpoint limit imposed by the Anthropic API. The system prompt uses 1 slot, leaving 3 for conversation messages.

---

## Command System

The engine provides a slash command system for user actions.

### Built-in Commands

| Command | Aliases | Description |
| :--- | :--- | :--- |
| `/quit` | `/q` | Exit application |
| `/clear` | - | Clear conversation and history |
| `/cancel` | - | Cancel streaming, tool execution, or distillation |
| `/model [name]` | - | Set model or open picker |
| `/context` | `/ctx` | Show context usage stats |
| `/journal` | `/jrnl` | Show journal statistics |
| `/distill` | - | Trigger distillation |
| `/rewind [id\|last] [scope]` | `/rw` | Rewind to an automatic checkpoint |
| `/undo` | - | Rewind to the latest turn checkpoint (conversation only) |
| `/retry` | - | Rewind to the latest turn checkpoint and restore the prompt into the draft |
| `/problems` | `/diag` | Show LSP diagnostics (compiler errors/warnings) |

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
    Context,
    Journal,
    Distill,
    Rewind { target: Option<&'a str>, scope: Option<&'a str> },
    Undo,
    Retry,
    Problems,
    Unknown(&'a str),
    Empty,
}

impl<'a> Command<'a> {
    pub fn parse(raw: &'a str) -> Self {
        // Accepts optional leading `/`, case-insensitive
        // Uses normalize_command_name() for alias resolution
    }
}
```

### Tab Completion and Command Aliases

Command names support tab completion via `CommandKind` and `CommandAlias` types. The `CommandMode::tab_complete()` method provides shell-style completion for:

- Command names (e.g., typing `dis` + Tab completes to `distill`)
- Model arguments for `/model` (e.g., `model cl` + Tab completes to `model claude-opus-4-6`)
- Rewind targets and scopes

### CommandSpec

The `CommandSpec` struct and `command_specs()` function provide command metadata for the palette and help display:

```rust
pub struct CommandSpec {
    pub palette_label: &'static str,  // Display in command palette
    pub help_label: &'static str,     // Display in help output
    pub description: &'static str,    // Brief description
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
    Err(ContextBuildError::DistillationNeeded(needed)) => {
        // Queue the request, start distillation
        self.try_start_distillation(Some(queued_request));
        return;
    }
    Err(ContextBuildError::RecentMessagesTooLarge { .. }) => {
        self.push_notification("Recent messages exceed budget");
        return;
    }
};
```

### Distillation

Distillation replaces older messages with a compressed summary to free context budget. The engine delegates to the cheapest model for each provider (e.g., Claude Haiku or Gemini Flash) via `distillation_model()`.

Key design decisions:

- **No engine-level retries**: Transport-layer retries in `providers/src/retry.rs` handle transient HTTP failures. The engine only sees errors after those retries are exhausted.
- **Queued request support**: If a user message triggers distillation, the message is queued via `DistillationState::CompletedWithQueued` and automatically streamed after distillation completes.
- **Provider consistency**: When a request is queued during distillation, the original `ApiConfig` (key + model) is preserved even if the user switches providers during distillation.

The distillation lifecycle:

1. `try_start_distillation()` checks if distillation is needed and spawns the background task
2. `poll_distillation()` (called from `tick()`) checks if the task completed
3. On success: applies the distillation result via `context_manager.complete_distillation()`
4. On failure: rolls back any queued request and notifies the user

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

The engine uses a TOML-based configuration system with environment variable expansion.

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
    pub lsp: Option<forge_lsp::LspConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub model: Option<String>,
    pub tui: Option<String>,
    #[serde(default)]
    pub ascii_only: bool,
    #[serde(default)]
    pub high_contrast: bool,
    #[serde(default)]
    pub reduced_motion: bool,
    #[serde(default)]
    pub show_thinking: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolsConfig {
    pub max_tool_calls_per_batch: Option<usize>,
    pub max_tool_iterations_per_user_turn: Option<u32>,
    pub definitions: Vec<ToolDefinitionConfig>,
    pub sandbox: Option<ToolSandboxConfig>,
    pub timeouts: Option<ToolTimeoutsConfig>,
    pub output: Option<ToolOutputConfig>,
    pub environment: Option<ToolEnvironmentConfig>,
    pub approval: Option<ToolApprovalConfig>,
    pub read_file: Option<ReadFileConfig>,
    pub apply_patch: Option<ApplyPatchConfig>,
    pub search: Option<SearchConfig>,
    pub webfetch: Option<WebFetchConfig>,
    pub run: Option<RunConfig>,
    pub shell: Option<ShellConfig>,
}
```

### Configuration Loading

```rust
impl ForgeConfig {
    pub fn load() -> Result<Option<Self>, ConfigError> {
        let path = dirs::home_dir()?.join(".forge").join("config.toml");
        // Returns Ok(None) if file doesn't exist
        // Returns Err(ConfigError::Read{..}) or Err(ConfigError::Parse{..}) on errors
    }

    pub fn persist_model(model: &str) -> std::io::Result<()> {
        // Uses toml_edit to preserve comments and formatting
        // Creates config file and parent directory if needed
        // Secure permissions on Unix (0o700 dir, 0o600 file)
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
ascii_only = false         # ASCII-only glyphs for icons/spinners
high_contrast = false      # High-contrast color palette
reduced_motion = false     # Disable modal animations
show_thinking = false      # Render provider thinking/reasoning in UI
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
memory = true  # Enable memory (librarian fact extraction/retrieval)
```

Note: `FORGE_CONTEXT_INFINITY` is only consulted when the `[context]` table is absent. If you include `[context]` but omit `memory`, serde defaults it to `false`, which disables memory even if the env var would otherwise enable it. When the env var is used, it defaults to enabled when unset and disables only for `0|false|off|no` (`engine/src/init.rs`).

#### [anthropic]

```toml
[anthropic]
cache_enabled = true
thinking_mode = "adaptive"       # adaptive | enabled | disabled (Opus 4.6+)
thinking_effort = "max"          # low | medium | high | max (Opus 4.6+)
thinking_enabled = false         # Legacy field for pre-4.6 models
thinking_budget_tokens = 10000   # Only used when thinking_mode = "enabled"
```

**Anthropic Thinking Modes:**

| Mode | Description |
| :--- | :--- |
| `adaptive` (default) | Claude decides when and how much to think |
| `enabled` | Manual mode with explicit `budget_tokens` |
| `disabled` | No thinking |

**Anthropic Effort Levels:**

| Level | Description |
| :--- | :--- |
| `low` | Minimal thinking effort |
| `medium` | Moderate thinking effort |
| `high` | High thinking effort |
| `max` (default) | Maximum thinking effort |

#### [openai]

```toml
[openai]
reasoning_effort = "high"  # none | low | medium | high | xhigh (also accepts "x-high")
reasoning_summary = "auto" # none | auto | concise | detailed (shown when show_thinking=true)
verbosity = "high"         # low | medium | high
truncation = "auto"        # auto | disabled
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
mode = "default"           # permissive | default | strict
allowlist = ["Read"]
denylist = ["Run"]

[tools.read_file]
max_file_read_bytes = 204800
max_scan_bytes = 2097152

[tools.apply_patch]
max_patch_bytes = 524288

[tools.search]
binary = "ugrep"
fallback_binary = "rg"
default_timeout_ms = 5000
default_max_results = 100
max_matches_per_file = 50
max_files = 1000
max_file_size_bytes = 1048576

[tools.webfetch]
user_agent = "CustomBot/1.0"
timeout_seconds = 30
max_redirects = 5
default_max_chunk_tokens = 2000
max_download_bytes = 10485760
cache_dir = "/tmp/webfetch"
cache_ttl_days = 7

[tools.shell]
binary = "pwsh"
args = ["-NoProfile", "-Command"]

[tools.run.windows]
enabled = true
fallback_mode = "prompt"     # prompt | deny | allow_with_warning

[[tools.definitions]]
name = "custom_tool"
description = "A custom tool"
[tools.definitions.parameters]
type = "object"
```

#### [lsp]

```toml
[lsp]
# LSP client configuration (see forge-lsp crate for options)
```

### Configuration Precedence

| Setting | Precedence (highest first) |
|---------|---------------------------|
| API Keys | Config file -> Environment variables |
| Provider | Config file -> Auto-detect from available keys -> Default (Claude) |
| Model | Config file -> Provider default |
| Memory | Config file (`[context].memory`, if `[context]` table exists) -> `FORGE_CONTEXT_INFINITY` env var (only if `[context]` absent) -> Default (true) |

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
        if self.is_side_effecting() { RiskLevel::Medium } else { RiskLevel::Low }
    }
    fn approval_summary(&self, args: &Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> { None }
    fn is_hidden(&self) -> bool { false }
    fn target_provider(&self) -> Option<Provider> { None }
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}
```

**Notable trait methods:**

| Method | Description |
| :--- | :--- |
| `is_hidden()` | Hidden tools execute normally but are invisible in the UI |
| `target_provider()` | If set, the tool definition is only sent to the specified provider |
| `approval_summary()` | Human-readable summary for the approval prompt |

### Built-in Tools

#### Core Tools

| Tool | Description | Side Effects |
|------|-------------|--------------|
| `Read` | Read file contents with optional line range | No |
| `Write` | Write content to a new file (fails if exists) | Yes |
| `Edit` | Apply edits to existing files | Yes |
| `Run` | Execute shell commands (with sandbox isolation on Windows) | Yes |
| `Glob` | Find files matching glob patterns | No |
| `Search` | Search file contents with regex (aliases: `search`, `rg`, `ripgrep`, `ugrep`, `ug`) | No |
| `WebFetch` | Fetch and parse web page content | No |
| `Recall` | Query Librarian fact store for past context (Context Infinity) | No |
| `Memory` | Store facts in the Librarian's memory (Context Infinity) | Yes |
| `PhaseGate` | Force generation boundaries in Gemini (hidden, Gemini-only) | No |

#### Git Tools

| Tool | Description | Side Effects |
|------|-------------|--------------|
| `GitStatus` | Show working tree status | No |
| `GitDiff` | Show file changes in working tree or staging area | No |
| `GitLog` | Show commit history | No |
| `GitShow` | Show commit details and diff | No |
| `GitBlame` | Show revision and author for each line | No |
| `GitAdd` | Stage files for commit | Yes |
| `GitCommit` | Create a conventional commit | Yes |
| `GitBranch` | List, create, rename, or delete branches | Yes |
| `GitCheckout` | Switch branches or restore files | Yes |
| `GitRestore` | Discard uncommitted changes (destructive) | Yes |
| `GitStash` | Stash changes in working directory | Yes |

### Tool Approval Workflow

The approval system provides three modes:

```rust
pub enum ApprovalMode {
    Permissive,  // Prompt only for tools with requires_approval() == true (others auto-execute, even if side-effecting)
    Default,     // Prompt for side-effecting tools unless allowlisted; denylisted tools are always rejected
    Strict,      // Reject non-allowlisted tools; allowlisted tools still require explicit approval
}
```

The `ApprovalState` enforces a double-press pattern for deny actions:

1. User presses `d` -> enters `ConfirmingDeny` state, cursor moves to Deny button
2. User presses `d` again (or Enter) -> executes denial
3. Any other key -> cancels deny confirmation, returns to `Selecting` state

### Sandbox Enforcement

The sandbox restricts tool access to authorized paths:

```rust
pub struct ToolSandboxConfig {
    pub allowed_roots: Vec<String>,     // Directories (strings; expanded + canonicalized at init)
    pub denied_patterns: Vec<String>,   // Glob patterns to block
    pub allow_absolute: bool,           // Allow absolute paths in tool args
    pub include_default_denies: bool,   // Append built-in credential-deny patterns
}
```

Note: if `allowed_roots` is empty, init defaults it to the current working directory (`AppConfig.working_dir`).

Default denied patterns (`DEFAULT_SANDBOX_DENIES` in `engine/src/init.rs`):

- `**/.ssh/**` - SSH keys and config
- `**/.gnupg/**` - GPG keys
- `**/.aws/**` - AWS credentials
- `**/.azure/**` - Azure credentials
- `**/.config/gcloud/**` - GCloud credentials
- `**/.git/**` - Git internals
- `**/.git-credentials` - Git credentials
- `**/.npmrc` - npm auth tokens
- `**/.pypirc` - PyPI auth tokens
- `**/.netrc` - Network credentials
- `**/.env`, `**/.env.*`, `**/*.env` - Environment files
- `**/id_rsa*`, `**/id_ed25519*`, `**/id_ecdsa*` - SSH private keys
- `**/*.pem`, `**/*.key`, `**/*.p12`, `**/*.pfx`, `**/*.der` - Certificate/key files
- `**/core`, `**/core.*`, `**/*.core`, `**/*.dmp`, `**/*.mdmp`, `**/*.stackdump` - Crash dump artifacts

### Command Blacklist

The `CommandBlacklist` blocks catastrophic shell commands (e.g., `rm -rf /`, `format C:`) before they reach the shell. Commands are checked against patterns and denied with a `DenialReason::CommandBlacklisted` error.

### Windows Run Sandbox

On Windows, the `Run` tool can isolate processes using Job Objects via `windows-sys`. The `WindowsRunConfig` controls:

- `enabled` (default: `true`): Whether to use Job Object isolation
- `fallback_mode` (default: `prompt`): Behavior when isolation is unavailable (`prompt`, `deny`, or `allow_with_warning`)

### Homoglyph Detection

Tool arguments are analyzed for Unicode homoglyph attacks (mixed-script characters) before approval. High-risk fields are checked per tool type:

- `WebFetch`: `url` field
- `Run`/`Pwsh`: `command` field
- `Read`/`Write`/`Edit`: `path` and `file_path` fields

### ToolError Enum

```rust
pub enum ToolError {
    BadArgs { message: String },
    Timeout { tool: String, elapsed: Duration },
    SandboxViolation(DenialReason),
    ExecutionFailed { tool: String, message: String },
    UnknownTool { name: String },
    DuplicateTool { name: String },
    DuplicateToolCallId { id: String },
    PatchFailed { file: PathBuf, message: String },
    StaleFile { file: PathBuf, reason: String },
}
```

### DenialReason Enum

```rust
pub enum DenialReason {
    Denylisted { tool: String },
    PathOutsideSandbox { attempted: PathBuf, resolved: PathBuf },
    DeniedPatternMatched { attempted: PathBuf, pattern: String },
    LimitsExceeded { message: String },
    CommandBlacklisted { command: String, reason: String },
}
```

### Tool Execution Flow

```
1. LLM response contains tool_calls
2. ActiveStream transitions to Journaled (if not already)
3. ToolBatch created: safe calls partitioned from dangerous calls
4. Safe tools added to execute_now, dangerous to approval_calls
5. If approval needed: enter AwaitingApproval state
6. User approves/denies (with double-press deny confirmation)
7. Approved tools queued in ToolQueue
8. Tools executed sequentially: Processing -> Executing -> Processing
9. Each result journaled for crash recovery
10. Batch committed to history
11. Auto-resume streaming with tool results
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemNotification {
    /// User approved tool calls.
    ToolsApproved { count: u8 },
    /// User denied tool calls.
    ToolsDenied { count: u8 },
    /// Compiler/linter diagnostics found in recently edited files.
    DiagnosticsFound { summary: String },
}
```

Each variant represents a specific system event that the model should be aware of. The enum is intentionally closed - only Forge code can construct these variants, preventing injection attacks.

**Note**: The enum derives `Clone` and `PartialEq` (not `Copy`) because `DiagnosticsFound` contains a `String`.

### NotificationQueue

The `NotificationQueue` manages pending notifications:

```rust
pub struct NotificationQueue {
    pending: Vec<SystemNotification>,
}

impl NotificationQueue {
    pub fn new() -> Self;
    pub fn push(&mut self, notification: SystemNotification);  // Deduplicates
    pub fn take(&mut self) -> Vec<SystemNotification>;
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}
```

### Notification Formatting

All notifications are prefixed with `[System: ...]` to clearly mark them as system-level messages distinct from user or model content:

| Notification | Formatted Output |
| :--- | :--- |
| `ToolsApproved { count: 3 }` | `[System: User approved 3 tool call(s)]` |
| `ToolsDenied { count: 1 }` | `[System: User denied 1 tool call(s)]` |
| `DiagnosticsFound { summary }` | `[System: Compiler errors detected]\n{summary}` |

### Injection Mechanism

Notifications are injected into the API request at the start of streaming:

1. Take all queued notifications via `notification_queue.take()`
2. Format each notification using `SystemNotification::format()`
3. Combine into a single string (newline-separated)
4. Append as an assistant message at the **tail** of the message list

**Cache impact**: Injection at the tail preserves the cache prefix, ensuring previously cached context remains valid.

### Design Rationale

| Aspect | Implementation |
|--------|----------------|
| **Trust boundary** | Assistant messages are unforgeable by users |
| **Deduplication** | Same notification pushed twice only appears once |
| **Batch injection** | Multiple notifications combined into single message |
| **Cache-safe** | Appended at tail, preserving cache prefix |
| **Extensibility** | Add new variants to `SystemNotification` enum |

---

## LSP Integration

The engine integrates with Language Server Protocol clients via the `forge-lsp` crate to provide compiler diagnostics feedback to the LLM.

### Architecture

The LSP manager (`forge_lsp::LspManager`) is lazily started on the first tool batch execution. It runs as a background process communicating via stdio with language servers (e.g., `rust-analyzer`, `gopls`).

### Diagnostics Flow

1. **File changes detected**: When tools modify files, the paths are recorded in `session_changes` and a deferred diagnostics check is scheduled (3-second delay to allow the language server to process changes).

2. **Polling**: `poll_lsp_events()` runs during each `tick()` call, processing up to 32 LSP events per tick. Events update `lsp_snapshot`, which caches the current diagnostics state.

3. **Injection**: When the deferred check fires and errors are detected, a `DiagnosticsFound` system notification is queued. This is injected as an assistant message on the next API request, informing the model about compiler errors in recently edited files.

### User Interface

The `/problems` (alias: `/diag`) command displays the current diagnostics snapshot:

```
Diagnostics: 2 error(s), 1 warning(s)
  src/main.rs:42:5: error: expected `;`
  src/lib.rs:10:1: warning: unused import
  src/main.rs:50:10: error: mismatched types
```

---

## Files Panel

The engine tracks all files created and modified during the session and provides an interactive files panel for reviewing changes.

### SessionChangeLog

The `SessionChangeLog` maintains two ordered sets:

- `created`: Files that did not exist before and were created during the session
- `modified`: Files that existed before and were modified during the session

Files are tracked via the `ChangeRecorder` in `TurnContext`, which captures per-turn file changes with diff statistics.

### FilesPanelState

```rust
pub struct FilesPanelState {
    pub visible: bool,                  // Whether the panel is visible
    pub selected: usize,                // Index into the flattened file list
    pub expanded: Option<PathBuf>,      // Which file's diff is expanded
    pub diff_scroll: usize,             // Scroll offset within the diff view
}
```

### FileDiff

The `FileDiff` enum represents the result of diff generation:

```rust
pub enum FileDiff {
    Diff(String),       // Unified diff between baseline and current
    Created(String),    // File created (show full content as additions)
    Deleted,            // File no longer exists on disk
    Binary(usize),      // Binary file (show size only)
    Error(String),      // Error reading file
}
```

### Panel Animations

The files panel uses `PanelEffect` for slide-in/slide-out animations:

```rust
pub enum PanelEffectKind {
    SlideInRight,   // Panel appearing from the right
    SlideOutRight,  // Panel disappearing to the right
}
```

Animations respect the `reduced_motion` config option - when enabled, panel visibility toggles instantly without animation.

---

## Input History

The engine provides prompt and command recall via the `InputHistory` type.

### Design

```rust
pub struct InputHistory {
    prompts: Vec<String>,    // Previously submitted user prompts (max 100)
    commands: Vec<String>,   // Previously executed slash commands (max 50)
    // Navigation state (not serialized):
    prompt_index: Option<usize>,
    command_index: Option<usize>,
    prompt_stash: Option<String>,
    command_stash: Option<String>,
}
```

### Navigation Behavior

**Up arrow** (first press): Stashes the current draft, shows the most recent entry.
**Up arrow** (subsequent): Shows progressively older entries.
**Down arrow**: Shows the next newer entry. At the newest entry, restores the stashed draft.

Navigation is reset after submitting a prompt or command. The history buffers (prompts and commands) are persisted across sessions; navigation state is not.

Duplicate suppression: consecutive identical entries are not added to history.

---

## Public API Reference

### Main Types

#### `App`

The central state container. All application state flows through this struct.

```rust
use forge_engine::App;

// Create a new application instance
let mut app = App::new(system_prompts)?;

// Main loop operations
app.tick();                      // Advance animations, poll background tasks
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
    cmd.tab_complete();  // Shell-style tab completion
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
    let thinking = streaming.thinking();    // Accumulated thinking text
    let provider = streaming.provider();    // Which provider is streaming
    let model = streaming.model_name();     // The model being used
    let usage = streaming.usage();          // API-reported token usage
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

Available predefined models for the model selector:

```rust
pub enum PredefinedModel {
    ClaudeOpus,
    ClaudeHaiku,
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
    Shake,     // Shake animation (e.g., invalid model selection)
}

// Creating effects
let effect = ModalEffect::pop_scale(Duration::from_millis(700));
let effect = ModalEffect::slide_up(Duration::from_millis(300));
let effect = ModalEffect::shake(Duration::from_millis(360));

// Animation queries
let progress = effect.progress();     // 0.0 to 1.0
let finished = effect.is_finished();
let kind = effect.kind();
```

#### `PanelEffect`

Animation state for the files panel:

```rust
pub enum PanelEffectKind {
    SlideInRight,   // Panel sliding in from right
    SlideOutRight,  // Panel sliding out to right
}

let effect = PanelEffect::slide_in_right(Duration::from_millis(180));
let effect = PanelEffect::slide_out_right(Duration::from_millis(180));
```

### App Instance Interface

| Method | Description |
| :--- | :--- |
| `App::new(system_prompts)` | Create instance, load config, recover crashes |
| `tick()` | Poll background tasks, update wall-clock timers |
| `frame_elapsed()` | Get time since last frame for animations |
| `should_quit()` | Check if quit was requested |
| `request_quit()` | Signal application exit |
| `process_stream_events()` | Apply pending stream chunks |
| `process_command(entered)` | Execute a parsed command |

### State Queries

| Method | Return Type | Description |
|--------|-------------|-------------|
| `input_mode()` | `InputMode` | Current input mode |
| `is_loading()` | `bool` | Whether any async operation is active |
| `is_empty()` | `bool` | No history messages and not streaming |
| `streaming()` | `Option<&StreamingMessage>` | Access active stream |
| `history()` | `&FullHistory` | Full conversation history |
| `display_items()` | `&[DisplayItem]` | Items to render |
| `display_version()` | `usize` | Version counter for render caching |
| `provider()` | `Provider` | Current LLM provider |
| `model()` | `&str` | Current model name |
| `has_api_key(provider)` | `bool` | Check if API key is configured |
| `memory_enabled()` | `bool` | Whether memory/distillation is on |
| `context_usage_status()` | `ContextUsageStatus` | Token usage statistics (cached) |
| `is_tool_hidden(name)` | `bool` | Whether a tool is hidden from UI |
| `last_turn_usage()` | `Option<&TurnUsage>` | API usage from last completed turn |
| `ui_options()` | `UiOptions` | Current UI configuration |
| `session_changes()` | `&SessionChangeLog` | Session-wide file change log |
| `files_panel_visible()` | `bool` | Whether files panel is visible |
| `files_panel_state()` | `&FilesPanelState` | Files panel interactive state |

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
| `process_stream_events()` | Apply pending stream chunks (budget: 512 events) |

### Tool Approval Operations

| Method | Description |
|--------|-------------|
| `tool_approval_approve_all()` | Approve all pending tool calls |
| `tool_approval_deny_all()` | Initiate deny (requires double-press) |
| `tool_approval_confirm_selected()` | Confirm currently selected tools |
| `tool_approval_activate()` | Enter-key action (context-dependent) |
| `tool_approval_toggle()` | Toggle selection on current tool |
| `tool_approval_toggle_details()` | Expand/collapse tool call details |
| `tool_approval_move_up()` | Move cursor up in approval list |
| `tool_approval_move_down()` | Move cursor down in approval list |
| `tool_approval_deny_confirm()` | Check if in deny confirmation state |

### Model Management

| Method | Description |
|--------|-------------|
| `set_model(model)` | Set specific model (persists to config) |
| `model_select_index()` | Currently selected index |
| `model_select_move_up()` | Move selection up |
| `model_select_move_down()` | Move selection down |
| `model_select_set_index(idx)` | Set selection to specific index |
| `model_select_confirm()` | Apply selection, exit mode |

### Files Panel Operations

| Method | Description |
|--------|-------------|
| `toggle_files_panel()` | Toggle panel visibility with animation |
| `close_files_panel()` | Close panel (no-op if hidden) |
| `files_panel_next()` | Select next file (wrapping) |
| `files_panel_prev()` | Select previous file (wrapping) |
| `files_panel_collapse()` | Collapse expanded diff |
| `files_panel_scroll_diff_down()` | Scroll diff view down |
| `files_panel_scroll_diff_up()` | Scroll diff view up |
| `files_panel_diff()` | Generate diff for expanded file |
| `ordered_files()` | Get ordered list of changed files |

### Input History Operations

| Method | Description |
|--------|-------------|
| `navigate_history_up()` | Previous prompt (Insert mode) |
| `navigate_history_down()` | Next prompt (Insert mode) |
| `navigate_command_history_up()` | Previous command (Command mode) |
| `navigate_command_history_down()` | Next command (Command mode) |

### Scrolling

| Method | Description |
|--------|-------------|
| `scroll_up()` | Scroll message view up (3 lines) |
| `scroll_down()` | Scroll message view down (3 lines) |
| `scroll_page_up()` | Scroll up by a page (10 lines) |
| `scroll_page_down()` | Scroll down by a page (10 lines) |
| `scroll_up_chunk()` | Scroll up by 20% of content |
| `scroll_to_top()` | Jump to beginning |
| `scroll_to_bottom()` | Jump to end, enable auto-scroll |
| `scroll_offset_from_top()` | Current scroll position |
| `update_scroll_max(max)` | Update scrollable range |

### Miscellaneous

| Method | Description |
|--------|-------------|
| `toggle_thinking()` | Toggle thinking/reasoning visibility |
| `take_clear_transcript()` | Check and clear transcript-clear flag |
| `cancel_active_operation()` | Cancel any in-progress operation |

---

## Extension Guide

### Adding a New Command

1. **Add command variant to `Command` enum** (`engine/src/commands.rs`):

```rust
pub(crate) enum Command<'a> {
    // ... existing commands ...
    MyCommand(Option<&'a str>),
}
```

2. **Add `CommandKind` variant and aliases**:

```rust
pub(crate) enum CommandKind {
    // ... existing kinds ...
    MyCommand,
}

// In COMMAND_ALIASES:
CommandAlias { name: "mycommand", kind: CommandKind::MyCommand },
CommandAlias { name: "mc", kind: CommandKind::MyCommand },
```

3. **Add match arm in `Command::parse()`**:

```rust
CommandKind::MyCommand => Command::MyCommand(parts.get(1).copied()),
```

4. **Handle command in `process_command()`** (`engine/src/commands.rs`):

```rust
Command::MyCommand(arg) => {
    if let Some(value) = arg {
        self.push_notification(format!("MyCommand executed with: {value}"));
    } else {
        self.push_notification("Usage: /mycommand <arg>");
    }
}
```

5. **Add `CommandSpec` for palette/help display**:

```rust
// In COMMAND_SPECS:
CommandSpec {
    palette_label: "mycommand, mc",
    help_label: "mycommand",
    description: "Does something useful",
},
```

### Adding a New Input Mode

1. **Extend `InputState` enum** (`engine/src/ui/input.rs`):

```rust
pub(crate) enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: DraftInput },
    ModelSelect { draft: DraftInput, selected: usize },
    FileSelect { draft: DraftInput, filter: DraftInput, selected: usize },
    MyMode { draft: DraftInput, custom_state: MyState },  // New mode
}
```

2. **Add mode enum variant** (`engine/src/ui/input.rs`):

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

3. **Add transition method**:

```rust
impl InputState {
    pub(crate) fn into_my_mode(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) |
            InputState::Command { draft, .. } | InputState::ModelSelect { draft, .. } |
            InputState::FileSelect { draft, .. } => {
                InputState::MyMode {
                    draft,
                    custom_state: MyState::default(),
                }
            }
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
pub fn handle_events(app: &mut App, input: &mut InputPump) -> Result<bool> {
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
    pub fn parse(s: &str) -> Result<Self, EnumParseError<Self>> {
        // Accepts aliases: "gpt", "chatgpt" -> OpenAI; "google" -> Gemini; etc.
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Ok(Self::Claude),
            "openai" | "gpt" | "chatgpt" => Ok(Self::OpenAI),
            "gemini" | "google" => Ok(Self::Gemini),
            "myprovider" | "mp" => Ok(Self::MyProvider),
            _ => Err(EnumParseError::new(/* ... */)),
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
    pub google: Option<String>,
    pub my_provider: Option<String>,  // New key
}
```

4. **Add system prompt** to `SystemPrompts` (`engine/src/lib.rs`):

```rust
pub struct SystemPrompts {
    pub claude: &'static str,
    pub openai: &'static str,
    pub gemini: &'static str,
    pub my_provider: &'static str,  // New prompt
}
```

5. **Update key loading in `App::new()`** (`engine/src/init.rs`)

### Adding a New Built-in Tool

1. **Create tool executor** (`tools/src/my_tool.rs` in the `forge-tools` crate):

```rust
pub struct MyTool;

impl ToolExecutor for MyTool {
    fn name(&self) -> &'static str { "MyTool" }

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

    // Optional: hide from UI
    fn is_hidden(&self) -> bool { false }

    // Optional: restrict to a specific provider
    fn target_provider(&self) -> Option<Provider> { None }

    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let arg1 = args["arg1"].as_str().unwrap_or("?");
        Ok(format!("Run MyTool with arg: {arg1}"))
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

2. **Register in `register_builtins()`** (`tools/src/builtins.rs`):

Tool registration occurs via `register_builtins()`, which `engine/src/init.rs` calls during `App::new()`:

```rust
// In register_builtins()
registry.register(Box::new(MyTool))?;
```

3. **Add module declaration** (`tools/src/lib.rs`):

```rust
pub mod my_tool;
```

### Adding a New Async Operation State

1. **Add new state variant** (`engine/src/state.rs`):

```rust
pub(crate) enum OperationState {
    Idle,
    Streaming(ActiveStream),
    ToolLoop(Box<ToolLoopState>),
    ToolRecovery(ToolRecoveryState),
    Distilling(DistillationState),
    MyOperation(MyOperationState),  // New operation
}
```

2. **Add state transition guards**:

```rust
pub fn start_my_operation(&mut self) {
    if self.busy_reason().is_some() {
        self.push_notification("Cannot start: busy with other operation");
        return;
    }
    self.state = OperationState::MyOperation(state);
}
```

3. **Add polling in `tick()`**:

```rust
pub fn tick(&mut self) {
    self.poll_distillation();
    self.poll_tool_loop();
    self.poll_lsp_events();
    self.poll_journal_cleanup();
    self.poll_my_operation();  // New polling
    // ... wall-clock timers ...
}
```

4. **Handle in `busy_reason()`**:

```rust
fn busy_reason(&self) -> Option<&'static str> {
    match &self.state {
        OperationState::Idle => None,
        OperationState::MyOperation(_) => Some("my operation in progress"),
        // ... existing arms ...
    }
}
```

5. **Handle in `cancel_active_operation()`**:

```rust
OperationState::MyOperation(state) => {
    // Clean up the operation
    self.push_notification("My operation cancelled");
    true
}
```

---

## Re-exported Types

The engine re-exports commonly needed types from its dependencies:

### From `forge-context`

| Type | Description |
| :--- | :--- |
| `ContextManager` | Orchestrates token counting and distillation |
| `ContextAdaptation` | Result of model switch (shrinking/expanding) |
| `ContextBuildError` | Error from context preparation |
| `ContextUsageStatus` | Token usage statistics |
| `DistillationNeeded` | Data about which messages need distillation |
| `DistillationScope` | Scope of a distillation operation |
| `PendingDistillation` | Prepared distillation request |
| `FullHistory` | Complete message history |
| `MessageId` | Unique identifier for messages |
| `StreamJournal` | WAL for crash recovery |
| `ActiveJournal` | RAII handle for stream journaling |
| `ToolJournal` | WAL for tool execution recovery |
| `ToolBatchId` | Unique identifier for tool batches |
| `ModelLimits` | Token limits for a model |
| `ModelLimitsSource` | Where limits came from (catalog vs override) |
| `ModelRegistry` | Model catalog for limit lookups |
| `TokenCounter` | Token counting utilities |
| `PreparedContext` | Proof that context was built within budget |
| `RecoveredStream` | Stream recovered from crash |
| `RecoveredToolBatch` | Tool batch recovered from crash |
| `Librarian` | Fact extraction and retrieval engine |
| `Fact`, `FactType` | Librarian fact types |
| `ExtractionResult`, `RetrievalResult` | Librarian operation results |
| `distillation_model` | Function to get cheapest model per provider |
| `generate_distillation` | Function to generate a distillation |
| `retrieve_relevant` | Function to retrieve relevant facts |

### From `forge-providers`

| Type | Description |
|------|-------------|
| `ApiConfig` | API request configuration |
| `GeminiCache` | Active Gemini context cache |
| `GeminiCacheConfig` | Gemini cache configuration |

### From `forge-types`

| Type | Description |
|------|-------------|
| `Provider` | LLM provider enum (Claude, OpenAI, Gemini) |
| `ModelName` | Provider-scoped model identifier |
| `Message` | User/Assistant/System message |
| `NonEmptyString` | Guaranteed non-empty string |
| `NonEmptyStaticStr` | Compile-time guaranteed non-empty static string |
| `ApiKey` | Provider-specific API key |
| `ApiUsage` | Token usage statistics from API |
| `StreamEvent` | Streaming response events |
| `StreamFinishReason` | How streaming ended |
| `OutputLimits` | Max tokens and thinking budget |
| `ThinkingState` | Whether thinking is enabled/disabled |
| `ThoughtSignature` | Encrypted thinking signature (Claude) |
| `ThoughtSignatureState` | Signed or unsigned thinking state |
| `ToolCall` | Tool invocation from LLM |
| `ToolResult` | Result of tool execution |
| `ToolDefinition` | Tool schema for API |
| `CacheHint` | Cache breakpoint hint |
| `CacheableMessage` | Message with cache hints |
| `OpenAIRequestOptions` | OpenAI-specific request parameters |
| `OpenAIReasoningEffort` | OpenAI reasoning effort level |
| `OpenAIReasoningSummary` | OpenAI reasoning summary format |
| `OpenAITextVerbosity` | OpenAI text verbosity level |
| `OpenAITruncation` | OpenAI truncation strategy |
| `PredefinedModel` | Predefined model enum for picker |
| `sanitize_terminal_text` | Strip terminal control sequences |

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
| `FileDiff` | Diff result for files panel display |
| `SystemPrompts` | Provider-specific system prompt container |
| `SessionChangeLog` | Session-wide file creation/modification log |
| `DistillationTask` | Background distillation task handle |
| `CommandSpec` | Command metadata for palette/help |
| `command_specs()` | Get all command specs |
| `ForgeConfig` | Configuration root type |
| `AppConfig` | Application configuration section |
| `ViewState` | View-related state for rendering |
| `UiOptions` | UI configuration (theme, motion, glyphs) |
| `InputMode` | Current input mode enum |
| `InputHistory` | Prompt and command recall history |
| `DraftInput` | Text buffer with cursor tracking |
| `ScrollState` | Scroll position tracking |
| `DisplayItem` | Renderable item (history ref or local message) |
| `ModalEffect` | Modal overlay animation state |
| `ModalEffectKind` | Modal animation type (PopScale, SlideUp, Shake) |
| `PanelEffect` | Panel animation state |
| `PanelEffectKind` | Panel animation type (SlideInRight, SlideOutRight) |
| `FilesPanelState` | Files panel interactive state |
| `ChangeKind` | File change classification (Modified, Created) |
| `FileEntry` | File entry for file picker |
| `FilePickerState` | File picker filtering state |
| `find_match_positions` | Fuzzy match position finder |

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

See [ToolError Enum](#toolerror-enum) and [DenialReason Enum](#denialreason-enum) in the Tool Execution System section.

---

## Thread Safety

The `App` struct is not thread-safe and should be used from a single async task. Background operations (distillation, streaming, tool execution) are spawned as separate Tokio tasks that communicate via channels:

- **Streaming**: `mpsc::Receiver<StreamEvent>` for event delivery (bounded channel)
- **Distillation**: `tokio::task::JoinHandle` polled via `is_finished()` + `now_or_never()`
- **Tool execution**: Sequential execution with journal persistence
- **Cancellation**: `AbortHandle` for graceful task termination
- **LSP**: `Arc<Mutex<Option<LspManager>>>` for lazy initialization from async task

Shared state behind `Arc<Mutex<...>>`:
- `gemini_cache`: Gemini context cache (updated from streaming tasks)
- `librarian`: Fact extraction/retrieval (accessed from async extraction tasks)
- `tool_file_cache`: File hash cache for stale file detection
- `lsp`: LSP manager (lazily populated from startup task)

---

## Data Directory

The engine stores persistent data in the OS local data directory (from
`dirs::data_local_dir()`), under a `forge/` subfolder. If no system data
directory is available, it falls back to `./forge/`.

Config remains in the home directory: `~/.forge/config.toml`.

| Path | Purpose |
| :--- | :--- |
| `<data_dir>/history.json` | Conversation history (JSON) |
| `<data_dir>/session.json` | Draft input, input history, session state |
| `<data_dir>/stream_journal.db` | WAL for stream crash recovery |
| `<data_dir>/tool_journal.db` | WAL for tool execution recovery |
| `<data_dir>/librarian.db` | Librarian fact store (when memory enabled) |

All database files use SQLite with WAL mode for durability.

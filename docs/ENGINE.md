# Engine Crate Architecture

The `forge-engine` crate is the core state machine and orchestration layer for Forge. It manages application state, input modes, streaming responses, tool execution, and context management without any TUI dependencies.

## Table of Contents

- [Overview](#overview)
- [Module Structure](#module-structure)
- [App State Machine](#app-state-machine)
- [Input State Machine](#input-state-machine)
- [Proof Token Patterns](#proof-token-patterns)
- [Streaming Logic](#streaming-logic)
- [Tool Execution Framework](#tool-execution-framework)
- [Command Processing](#command-processing)
- [Configuration](#configuration)
- [Modal Animations](#modal-animations)
- [Extension Guide](#extension-guide)

---

## Overview

The engine crate provides a TUI-agnostic state machine that:

- Manages vim-style modal input (Normal, Insert, Command, ModelSelect)
- Handles non-blocking LLM response streaming with crash recovery
- Implements adaptive context window management (Context Infinity)
- Provides a unified interface for Claude and OpenAI APIs
- Executes tool calls with sandbox isolation and approval workflows
- Persists conversation history across sessions

### Key Design Principles

1. **Type-Driven Safety**: Operations require proof tokens that can only be obtained in valid states
2. **Explicit State Transitions**: All state changes are method calls, not flag mutations
3. **RAII Resource Management**: Streaming sessions use journal handles that auto-commit or discard
4. **Crash Durability**: All operations are journaled before being applied to UI

---

## Module Structure

```
engine/src/
├── lib.rs           # Main module: App struct, state machines, streaming
├── config.rs        # Configuration parsing (ForgeConfig)
└── tools/
    ├── mod.rs       # Tool framework types and traits
    ├── builtins.rs  # Built-in tool implementations
    ├── sandbox.rs   # Filesystem sandbox for path validation
    └── lp1.rs       # LP1 patch format parser and applier
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `forge-types` | Core domain types (Message, Provider, ModelName) |
| `forge-providers` | LLM API clients (Claude, OpenAI) |
| `forge-context` | Context window management, SQLite persistence |

---

## App State Machine

The `App` struct uses an explicit state machine to enforce invariants at compile time. The top-level `AppState` enum branches on whether Context Infinity is enabled.

### AppState Hierarchy

```
AppState
├── Enabled(EnabledState)     # Context Infinity ON
│   ├── Idle                  # Ready for user input
│   ├── Streaming(ActiveStream)
│   ├── AwaitingToolResults(PendingToolExecution)
│   ├── ToolLoop(ToolLoopState)
│   ├── ToolRecovery(ToolRecoveryState)
│   ├── Summarizing(SummarizationState)
│   ├── SummarizingWithQueued(SummarizationWithQueuedState)
│   ├── SummarizationRetry(SummarizationRetryState)
│   └── SummarizationRetryWithQueued(SummarizationRetryWithQueuedState)
│
└── Disabled(DisabledState)   # Context Infinity OFF
    ├── Idle
    └── Streaming(ActiveStream)
```

### State Descriptions

| State | Description |
|-------|-------------|
| `Idle` | Ready for user input; no background operations |
| `Streaming` | Actively receiving chunks from LLM API |
| `AwaitingToolResults` | ParseOnly mode: waiting for manual tool result submission |
| `ToolLoop` | Enabled mode: executing tools automatically |
| `ToolRecovery` | Recovered incomplete tool batch from crash; awaiting user decision |
| `Summarizing` | Background summarization task running |
| `SummarizingWithQueued` | Summarizing with a user request queued to send after |
| `SummarizationRetry` | Waiting for retry delay after summarization failure |
| `SummarizationRetryWithQueued` | Retry pending with queued request |

### ActiveStream Structure

```rust
struct ActiveStream {
    message: StreamingMessage,      // Accumulator for streamed content
    journal: ActiveJournal,         // RAII handle for crash recovery
    abort_handle: AbortHandle,      // For cancellation
    tool_batch_id: Option<ToolBatchId>,  // If tool calls are streaming
    tool_call_seq: usize,           // Sequence number for tool calls
}
```

### State Transition Diagram

```
                    ┌─────────────────────────────────────────────┐
                    │                   Idle                       │
                    └─────────────────────────────────────────────┘
                                         │
            ┌────────────────────────────┼────────────────────────────┐
            │ start_streaming()          │ start_summarization()      │
            ▼                            ▼                            │
    ┌───────────────┐           ┌───────────────────┐                 │
    │   Streaming   │           │   Summarizing     │                 │
    └───────┬───────┘           └─────────┬─────────┘                 │
            │                             │                           │
   ┌────────┼────────┐           ┌────────┼────────┐                  │
   │        │        │           │        │        │                  │
   ▼        ▼        ▼           ▼        ▼        │                  │
 Done    Error   ToolCalls    Success  Failure   Retry                │
   │        │        │           │        │        │                  │
   │        │        ▼           │        │        ▼                  │
   │        │   ToolLoop/        │        │  SummarizationRetry       │
   │        │   AwaitingTool     │        │        │                  │
   │        │        │           │        │        │                  │
   └────────┴────────┴───────────┴────────┴────────┴──────────────────┘
                                         │
                                         ▼
                                       Idle
```

---

## Input State Machine

The input system uses a separate state machine from the app state, allowing mode transitions regardless of streaming status.

### InputState Enum

```rust
enum InputState {
    Normal(DraftInput),                           // Navigation mode
    Insert(DraftInput),                           // Text editing mode
    Command { draft: DraftInput, command: String }, // Slash command entry
    ModelSelect { draft: DraftInput, selected: usize }, // Model picker overlay
}
```

### InputMode (Public View)

```rust
pub enum InputMode {
    Normal,      // Vim-like navigation (h/j/k/l, etc.)
    Insert,      // Text input with cursor
    Command,     // Entering :command
    ModelSelect, // Tab-triggered model picker
}
```

### Mode Transition Diagram

```
                    ┌───────────────────┐
                    │      Normal       │ ← Default mode
                    └─────────┬─────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        │ 'i' / 'a'           │ ':'                 │ <Tab>
        ▼                     ▼                     ▼
    ┌───────┐           ┌──────────┐         ┌─────────────┐
    │Insert │           │ Command  │         │ ModelSelect │
    └───┬───┘           └────┬─────┘         └──────┬──────┘
        │                    │                      │
        │ <Esc>              │ <Esc>/<Enter>        │ <Esc>/<Enter>
        └────────────────────┴──────────────────────┘
                             │
                             ▼
                         Normal
```

### DraftInput

The `DraftInput` struct manages text input with cursor position:

```rust
struct DraftInput {
    text: String,    // Current draft text
    cursor: usize,   // Cursor position (grapheme index)
}
```

Operations include:
- `enter_char(c)` - Insert character at cursor
- `delete_char()` - Backspace
- `delete_char_forward()` - Delete
- `delete_word_backwards()` - Ctrl+W
- `move_cursor_left/right()` - Arrow keys
- `move_cursor_end()` - End key
- `reset_cursor()` - Home key
- `clear()` - Clear all text

---

## Proof Token Patterns

The engine uses proof tokens to enforce compile-time safety for mode-specific operations. This pattern ensures that operations can only be called when the app is in the correct state.

### Token Types

| Token | Purpose |
|-------|---------|
| `InsertToken` | Proof that app is in Insert mode |
| `CommandToken` | Proof that app is in Command mode |
| `QueuedUserMessage` | Proof that a valid user message is ready to send |
| `EnteredCommand` | Proof that a command was entered in Command mode |

### Usage Pattern

```rust
// InsertToken can only be obtained when in Insert mode
pub fn insert_token(&self) -> Option<InsertToken> {
    matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
}

// InsertMode wrapper requires the token
pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
    InsertMode { app: self }
}

// Example usage in TUI input handler:
if let Some(token) = app.insert_token() {
    let mut mode = app.insert_mode(token);
    mode.enter_char('x');
    
    // Queue message returns proof that message is valid and ready
    if let Some(queued) = mode.queue_message() {
        app.start_streaming(queued);
    }
}
```

### InsertMode Operations

```rust
impl<'a> InsertMode<'a> {
    pub fn move_cursor_left(&mut self);
    pub fn move_cursor_right(&mut self);
    pub fn enter_char(&mut self, new_char: char);
    pub fn delete_char(&mut self);
    pub fn delete_char_forward(&mut self);
    pub fn delete_word_backwards(&mut self);
    pub fn reset_cursor(&mut self);
    pub fn move_cursor_end(&mut self);
    pub fn clear_line(&mut self);
    
    /// Validate and queue message. Returns proof token if successful.
    pub fn queue_message(self) -> Option<QueuedUserMessage>;
}
```

### CommandMode Operations

```rust
impl<'a> CommandMode<'a> {
    pub fn push_char(&mut self, c: char);
    pub fn backspace(&mut self);
    
    /// Consume command and return proof token.
    pub fn take_command(self) -> Option<EnteredCommand>;
}
```

### QueuedUserMessage

The `QueuedUserMessage` struct captures the API configuration at queue time:

```rust
pub struct QueuedUserMessage {
    config: ApiConfig,  // Model + API key frozen at queue time
}
```

This ensures that if the user changes provider/model during summarization, the original request configuration is preserved.

---

## Streaming Logic

### StreamingMessage

The `StreamingMessage` struct accumulates content during streaming:

```rust
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
    tool_calls: Vec<ToolCallAccumulator>,
}
```

### Stream Events

```rust
pub enum StreamEvent {
    TextDelta(String),              // Text content chunk
    ThinkingDelta(String),          // Extended thinking (not displayed)
    ToolCallStart { id, name },     // Tool call begins
    ToolCallDelta { id, arguments }, // Tool call arguments chunk
    Done,                           // Stream completed successfully
    Error(String),                  // Stream error
}
```

### Processing Flow

```
start_streaming(queued)
    │
    ├── Create ActiveJournal for crash recovery
    ├── Create channel for stream events
    ├── Spawn async task with Abortable wrapper
    │
    ▼
process_stream_events() [called each tick]
    │
    ├── try_recv_event() from channel
    ├── Persist to journal BEFORE display
    ├── Apply event to StreamingMessage
    │
    ├── On Done/Error: finish_streaming()
    │       │
    │       ├── Seal journal
    │       ├── Check for tool calls
    │       │       │
    │       │       ├── If tools: handle_tool_calls()
    │       │       └── If no tools: push to history
    │       │
    │       ├── Autosave history
    │       └── Finalize journal commit
    │
    └── Continue receiving events
```

### Crash Recovery

The stream journal ensures durability:

1. **Before streaming**: `stream_journal.begin_session()` creates `ActiveJournal`
2. **During streaming**: Each event appended via `journal.append_text/append_done/append_error`
3. **On completion**: `journal.seal()` marks stream complete
4. **After history save**: `stream_journal.commit_and_prune_step()` cleans up
5. **On crash recovery**: `stream_journal.recover()` returns `RecoveredStream`

Recovery types:
- `RecoveredStream::Complete` - Stream finished but wasn't committed
- `RecoveredStream::Incomplete` - Stream interrupted mid-content
- `RecoveredStream::Errored` - Stream ended with error

---

## Tool Execution Framework

### Overview

The tool framework enables LLM function calling with three modes:

| Mode | Description |
|------|-------------|
| `Disabled` | No tools sent to API; tool calls rejected |
| `ParseOnly` | Tools sent to API; results submitted manually via `:tool` command |
| `Enabled` | Tools executed automatically with approval workflow |

### ToolExecutor Trait

```rust
pub trait ToolExecutor: Send + Sync + UnwindSafe {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;      // JSON Schema for arguments
    fn is_side_effecting(&self) -> bool;
    fn requires_approval(&self) -> bool { false }
    fn risk_level(&self) -> RiskLevel { ... }
    fn approval_summary(&self, args: &Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> { None }
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}
```

### Built-in Tools

| Tool | Description | Side-Effecting | Risk Level |
|------|-------------|----------------|------------|
| `read_file` | Read file contents with optional line range | No | Low |
| `apply_patch` | Apply LP1 patch to files | Yes | Medium |
| `run_command` | Execute shell command | Yes | High |

### ToolCtx (Execution Context)

```rust
pub struct ToolCtx {
    pub sandbox: Sandbox,                    // Path validation
    pub abort: AbortHandle,                  // Cancellation handle
    pub output_tx: mpsc::Sender<ToolEvent>,  // Streaming output
    pub default_timeout: Duration,
    pub max_output_bytes: usize,
    pub available_capacity_bytes: usize,     // Remaining context budget
    pub tool_call_id: String,
    pub allow_truncation: bool,
    pub working_dir: PathBuf,
    pub env_sanitizer: EnvSanitizer,         // Secret filtering
    pub file_cache: Arc<Mutex<ToolFileCache>>, // SHA cache for stale detection
}
```

### Tool Loop State Machine

```rust
enum ToolLoopPhase {
    AwaitingApproval(ApprovalState),  // User must approve/deny
    Executing(ActiveToolExecution),    // Tools running sequentially
}
```

### Tool Execution Flow

```
Streaming completes with tool_calls
    │
    ▼
handle_tool_calls()
    │
    ├── Mode::Disabled → Return error results, commit batch
    │
    ├── Mode::ParseOnly → Enter AwaitingToolResults state
    │                     User submits results via :tool command
    │
    └── Mode::Enabled → start_tool_loop()
                            │
                            ▼
                       plan_tool_calls()
                            │
                            ├── Pre-resolve denied/invalid calls
                            ├── Separate: execute_now vs approval_calls
                            │
                            ▼
                ┌───────────────────────────────┐
                │ Approval needed?              │
                ├───────────────────────────────┤
                │ Yes: ToolLoop::AwaitingApproval│
                │       │                        │
                │       ├── approve_all()        │
                │       ├── approve_selected()   │
                │       └── deny_all()           │
                │                                │
                │ No: ToolLoop::Executing        │
                └───────────────────────────────┘
                            │
                            ▼
                spawn_tool_execution()
                            │
                            ├── Pop call from queue
                            ├── Create ToolCtx
                            ├── Spawn with timeout + panic catch
                            │
                            ▼
                poll_tool_loop() [each tick]
                            │
                            ├── Process ToolEvents (stdout/stderr)
                            ├── Check join_handle completion
                            ├── Record result to journal
                            │
                            ├── More calls? → start_next_tool_call()
                            └── All done? → commit_tool_batch()
                                              │
                                              ├── Push messages to history
                                              ├── auto_resume? → start_streaming()
                                              └── Return to Idle
```

### Sandbox

The `Sandbox` validates and resolves paths to prevent directory traversal:

```rust
pub struct Sandbox {
    allowed_roots: Vec<PathBuf>,     // Canonical allowed directories
    deny_patterns: Vec<DenyPattern>, // Glob patterns for denied paths
    allow_absolute: bool,            // Whether absolute paths are permitted
}
```

Key validations:
- No `..` components in paths
- Resolved path must be under an allowed root
- No symlinks in path components (symlink traversal attack prevention)
- Path must not match any deny pattern

Default deny patterns:
```
**/.ssh/**
**/.gnupg/**
**/id_rsa*
**/*.pem
**/*.key
```

### LP1 Patch Format

The `lp1` module implements a line-based patch format:

```
LP1
F path/to/file.rs
R
old line 1
old line 2
.
new line 1
new line 2
.
END
```

Operations:
- `R [occ]` - Replace matching lines
- `I [occ]` - Insert after match
- `P [occ]` - Insert before match (Prepend)
- `E [occ]` - Erase matching lines
- `T` - Append to end (Tail)
- `B` - Prepend to beginning (Beginning)
- `N +/-` - Set final newline

The `occ` parameter specifies which occurrence (1-indexed) when matches are not unique.

### Stale File Protection

The `apply_patch` tool uses SHA-256 caching to detect concurrent modifications:

1. `read_file` computes SHA-256 and stores in `file_cache`
2. Before patching, current SHA is compared to cached value
3. If different, `ToolError::StaleFile` is returned

---

## Command Processing

Commands are entered in Command mode (`:`) and processed by `process_command()`.

### Available Commands

| Command | Description |
|---------|-------------|
| `:q` / `:quit` | Exit application |
| `:clear` | Clear conversation and history |
| `:cancel` | Abort active stream or tool execution |
| `:model [name]` | Set model or open picker (no argument) |
| `:p [name]` / `:provider [name]` | Switch provider |
| `:ctx` / `:context` | Show context usage statistics |
| `:jrnl` / `:journal` | Show journal statistics |
| `:sum` / `:summarize` | Trigger background summarization |
| `:screen` | Toggle fullscreen/inline mode |
| `:tool <id> <result>` | Submit tool result (ParseOnly mode) |
| `:tool error <id> <msg>` | Submit error result |
| `:tools` | List configured tools |
| `:help` | Show available commands |

### Command Flow

```rust
pub fn process_command(&mut self, command: EnteredCommand) {
    let parts: Vec<&str> = command.raw.split_whitespace().collect();
    
    match parts.first().copied() {
        Some("q" | "quit") => self.request_quit(),
        Some("clear") => {
            // Abort any active operations
            // Clear display and context manager
            // Autosave cleared state
        }
        Some("model") => {
            if let Some(name) = parts.get(1) {
                // Parse and set model
            } else {
                self.enter_model_select_mode();
            }
        }
        // ... other commands
    }
}
```

---

## Configuration

Configuration is loaded from `~/.forge/config.toml` with environment variable expansion.

### ForgeConfig Structure

```rust
pub struct ForgeConfig {
    pub app: Option<AppConfig>,
    pub api_keys: Option<ApiKeys>,
    pub context: Option<ContextConfig>,
    pub cache: Option<CacheConfig>,
    pub thinking: Option<ThinkingConfig>,
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAIConfig>,
    pub tools: Option<ToolsConfig>,
}
```

### Configuration Sections

#### [app]
```toml
[app]
provider = "claude"        # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"               # or "inline"
max_output_tokens = 16000
```

#### [api_keys]
```toml
[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
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
reasoning_effort = "high"  # low | medium | high
verbosity = "high"         # low | medium | high
truncation = "auto"        # auto | disabled
```

#### [tools]
```toml
[tools]
mode = "enabled"           # disabled | parse_only | enabled
allow_parallel = false
max_tool_calls_per_batch = 8
max_tool_iterations_per_user_turn = 4
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

[[tools.definitions]]
name = "custom_tool"
description = "A custom tool"
[tools.definitions.parameters]
type = "object"
```

### Environment Variable Expansion

The `expand_env_vars()` function expands `${VAR}` syntax:

```rust
pub fn expand_env_vars(value: &str) -> String {
    // Replaces ${VAR_NAME} with environment variable value
    // Empty string if variable not set
}
```

### Configuration Loading

```rust
impl ForgeConfig {
    pub fn load() -> Option<Self> {
        let path = config_path()?;  // ~/.forge/config.toml
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str(&content).ok()
    }
}
```

---

## Modal Animations

The engine supports animated transitions for modal overlays (e.g., model selector).

### ModalEffect

```rust
pub enum ModalEffectKind {
    PopScale,  // Scale-in from center
    SlideUp,   // Slide up from bottom
}

pub struct ModalEffect {
    kind: ModalEffectKind,
    elapsed: Duration,
    duration: Duration,
}
```

### Usage

```rust
// Create effect when entering model select
pub fn enter_model_select_mode(&mut self) {
    self.input = self.input.into_model_select();
    self.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(700)));
    self.last_frame = Instant::now();
}

// Advance animation each frame
let elapsed = app.frame_elapsed();
if let Some(effect) = app.modal_effect_mut() {
    effect.advance(elapsed);
    if effect.is_finished() {
        app.clear_modal_effect();
    }
}

// Query animation state for rendering
let progress = effect.progress();  // 0.0 to 1.0
```

---

## Extension Guide

### Adding a New Command

1. Add command handling in `process_command()`:

```rust
Some("mycommand" | "mc") => {
    if let Some(arg) = parts.get(1) {
        // Handle argument
    } else {
        self.set_status("Usage: /mycommand <arg>");
    }
}
```

2. Update help text:

```rust
Some("help") => {
    self.set_status("Commands: ..., /mycommand, ...");
}
```

### Adding a New Input Mode

1. Extend `InputMode` enum:

```rust
pub enum InputMode {
    Normal,
    Insert,
    Command,
    ModelSelect,
    MyMode,  // New mode
}
```

2. Extend `InputState` enum:

```rust
enum InputState {
    // ...existing variants...
    MyMode { draft: DraftInput, custom_data: MyData },
}
```

3. Add transition methods:

```rust
impl InputState {
    fn into_my_mode(self) -> InputState {
        match self {
            InputState::Normal(draft) | InputState::Insert(draft) => {
                InputState::MyMode { draft, custom_data: MyData::default() }
            }
            // ...handle other variants
        }
    }
}

impl App {
    pub fn enter_my_mode(&mut self) {
        self.input = std::mem::take(&mut self.input).into_my_mode();
    }
}
```

4. Create proof token and mode wrapper:

```rust
pub struct MyModeToken(());

pub struct MyMode<'a> {
    app: &'a mut App,
}

impl App {
    pub fn my_mode_token(&self) -> Option<MyModeToken> {
        matches!(&self.input, InputState::MyMode { .. }).then_some(MyModeToken(()))
    }
    
    pub fn my_mode(&mut self, _token: MyModeToken) -> MyMode<'_> {
        MyMode { app: self }
    }
}
```

### Adding a New Tool

1. Implement `ToolExecutor`:

```rust
#[derive(Debug)]
pub struct MyTool {
    // tool-specific configuration
}

impl ToolExecutor for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn description(&self) -> &'static str { "Does something useful" }
    
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"]
        })
    }
    
    fn is_side_effecting(&self) -> bool { false }
    
    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let input = args.get("input")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        Ok(format!("Process: {}", input))
    }
    
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            // Tool implementation
            Ok("result".to_string())
        })
    }
}
```

2. Register in `register_builtins()`:

```rust
pub fn register_builtins(registry: &mut ToolRegistry, ...) -> Result<(), ToolError> {
    // ...existing registrations...
    registry.register(Box::new(MyTool::new()))?;
    Ok(())
}
```

### Adding a New App State

1. Add variant to appropriate state enum:

```rust
enum EnabledState {
    // ...existing variants...
    MyState(MyStateData),
}
```

2. Handle in state-checking code:

```rust
pub fn is_loading(&self) -> bool {
    matches!(
        self.state,
        AppState::Enabled(EnabledState::Streaming(_))
            | AppState::Enabled(EnabledState::MyState(_))  // Add here
            // ...
    )
}
```

3. Handle in `:clear` and `:cancel` commands:

```rust
AppState::Enabled(EnabledState::MyState(data)) => {
    // Clean up state-specific resources
}
```

---

## Public API Summary

### App Lifecycle

| Method | Description |
|--------|-------------|
| `App::new(system_prompt)` | Create instance, load config, recover crashes |
| `tick()` | Advance animations, poll background tasks |
| `frame_elapsed()` | Get time since last frame |
| `should_quit()` | Check if quit was requested |
| `request_quit()` | Signal application exit |
| `save_history()` | Persist conversation to disk |

### State Queries

| Method | Description |
|--------|-------------|
| `input_mode()` | Current input mode |
| `is_loading()` | Whether any operation is active |
| `is_empty()` | No messages and not streaming |
| `streaming()` | Access active `StreamingMessage` |
| `history()` | Full conversation history |
| `display_items()` | Items to render in message view |
| `provider()` | Current LLM provider |
| `model()` | Current model name |
| `has_api_key(provider)` | Check if API key is configured |
| `context_infinity_enabled()` | Whether adaptive context is on |
| `context_usage_status()` | Token usage statistics |

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

### Streaming

| Method | Description |
|--------|-------------|
| `start_streaming(queued)` | Begin API request |
| `process_stream_events()` | Apply pending stream chunks |

### Tool Operations

| Method | Description |
|--------|-------------|
| `tool_approval_requests()` | Get pending approval requests |
| `tool_loop_calls()` | Get all tool calls in current batch |
| `tool_loop_results()` | Get completed tool results |
| `tool_loop_output_lines()` | Get streaming output lines |
| `tool_approval_approve_all()` | Approve all pending tools |
| `tool_approval_deny_all()` | Deny all pending tools |
| `submit_tool_result(result)` | Submit manual tool result (ParseOnly) |

---

## Re-exported Types

The crate re-exports commonly needed types from its dependencies:

### From `forge-context`
- `ContextManager`, `ContextAdaptation`, `ContextUsageStatus`
- `FullHistory`, `MessageId`, `SummaryId`
- `StreamJournal`, `ActiveJournal`, `RecoveredStream`
- `ToolJournal`, `ToolBatchId`, `RecoveredToolBatch`
- `ModelLimits`, `ModelRegistry`, `TokenCounter`

### From `forge-providers`
- `ApiConfig`

### From `forge-types`
- `Provider`, `ModelName`, `Message`, `NonEmptyString`
- `ApiKey`, `StreamEvent`, `StreamFinishReason`
- `OutputLimits`, `CacheableMessage`
- `ToolCall`, `ToolDefinition`, `ToolResult`
- `OpenAIRequestOptions`, `OpenAIReasoningEffort`

---

## Error Handling

### Stream Errors

Stream errors are processed to provide helpful feedback:

1. **API Key errors**: Detected by status codes and error messages; hint to set environment variable
2. **Rate limits**: Displayed with retry suggestion
3. **Generic errors**: Shown with truncated details

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

The `App` struct is **not thread-safe** and should be used from a single async task. Background operations (streaming, summarization, tool execution) are spawned as separate Tokio tasks that communicate via channels.

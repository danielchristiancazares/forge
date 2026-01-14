# forge-cli

This document provides comprehensive documentation for the `forge` CLI crate - the binary entry point and terminal session management layer for the Forge LLM client. It is intended for developers who want to understand, maintain, or extend the CLI functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 38-76 | Overview: responsibilities, file structure, dependencies |
| 77-119 | Architecture Diagram: main() flow, mode switching, terminal session lifecycle |
| 120-169 | Module Structure: main.rs types and functions, assets.rs constants and statics |
| 170-264 | Terminal Session Management: TerminalSession, init/cleanup sequences, error handling |
| 265-357 | UI Mode System: UiMode enum, resolution logic, mode characteristics |
| 358-541 | Main Event Loops: tick cycle, run_app_full, run_app_inline, transcript clear, yield_now |
| 542-602 | Asset Management: compile-time embedding, system prompt content, OnceLock initialization |
| 603-681 | Startup and Shutdown Sequence: initialization order, cleanup guarantees |
| 682-716 | Configuration Resolution: UI mode config, file location, example |
| 717-747 | Error Handling: error types, sources, recovery strategy |
| 748-834 | Extension Guide: adding UI modes, assets, startup flags, modifying event loop |
| 835-841 | Related Documentation: links to other crate READMEs |

## Table of Contents

1. [Overview](#overview)
2. [Architecture Diagram](#architecture-diagram)
3. [Module Structure](#module-structure)
4. [Terminal Session Management](#terminal-session-management)
5. [UI Mode System](#ui-mode-system)
6. [Main Event Loops](#main-event-loops)
7. [Asset Management](#asset-management)
8. [Startup and Shutdown Sequence](#startup-and-shutdown-sequence)
9. [Configuration Resolution](#configuration-resolution)
10. [Error Handling](#error-handling)
11. [Extension Guide](#extension-guide)
12. [Related Documentation](#related-documentation)

---

## Overview

The `forge` CLI crate is the application entry point that orchestrates terminal setup, UI mode selection, and the main event loop. It bridges the `forge-engine` (application state) and `forge-tui` (rendering) crates, providing RAII-based terminal session management with proper cleanup guarantees.

### Key Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Terminal Session** | RAII-based setup/teardown of raw mode, alternate screen, bracketed paste |
| **UI Mode Selection** | Resolution of full-screen vs inline mode from config and environment |
| **Event Loop Execution** | Tick-based loop coordinating async tasks, streaming, rendering, and input |
| **Mode Switching** | Runtime toggling between full-screen and inline modes |
| **Asset Loading** | Compile-time embedding and runtime initialization of system prompt |

### File Structure

```
cli/
├── Cargo.toml              # Binary manifest (package name: "forge")
├── assets/
│   └── prompt.md           # System prompt embedded at compile time
└── src/
    ├── main.rs             # Entry point, event loops, terminal session
    └── assets.rs           # Compile-time asset embedding
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `forge-engine` | Application state machine (`App`, `ForgeConfig`) |
| `forge-tui` | Rendering functions (`draw`, `draw_inline`, `handle_events`) |
| `ratatui` | Terminal UI framework |
| `crossterm` | Cross-platform terminal manipulation |
| `tokio` | Async runtime |
| `tracing-subscriber` | Logging infrastructure |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              main()                                      │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │  1. Initialize tracing                                              │ │
│  │  2. Load assets (system prompt)                                     │ │
│  │  3. Load ForgeConfig                                                │ │
│  │  4. Resolve UiMode (config -> env -> default)                       │ │
│  │  5. Create App with system prompt                                   │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                │                                         │
│                                v                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                     Main Loop (mode switching)                      │ │
│  │  ┌──────────────────────────────────────────────────────────────┐  │ │
│  │  │  TerminalSession::new(ui_mode)  ← RAII terminal setup        │  │ │
│  │  └──────────────────────────────────────────────────────────────┘  │ │
│  │                                │                                    │ │
│  │          ┌─────────────────────┴─────────────────────┐             │ │
│  │          v                                           v              │ │
│  │  ┌──────────────────┐                    ┌──────────────────────┐  │ │
│  │  │  run_app_full()  │                    │  run_app_inline()    │  │ │
│  │  │  (alternate scr) │                    │  (inline viewport)   │  │ │
│  │  └──────────────────┘                    └──────────────────────┘  │ │
│  │          │                                           │              │ │
│  │          └─────────────────────┬─────────────────────┘             │ │
│  │                                v                                    │ │
│  │  ┌──────────────────────────────────────────────────────────────┐  │ │
│  │  │  RunResult::Quit | RunResult::SwitchMode                     │  │ │
│  │  └──────────────────────────────────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                │                                         │
│                                v                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │  app.save_history()  ← Persist conversation on exit                 │ │
│  └────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Module Structure

### `main.rs`

The primary module containing the application entry point and all core types.

#### Types

| Type | Description |
|------|-------------|
| `UiMode` | Enum representing display mode: `Full` (alternate screen) or `Inline` (embedded viewport) |
| `RunResult` | Enum representing event loop exit reason: `Quit` or `SwitchMode` |
| `TerminalSession` | RAII wrapper for terminal state with guaranteed cleanup on drop |

#### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `main` | `async fn main() -> Result<()>` | Application entry point |
| `run_app_full` | `async fn run_app_full<B>(terminal, app) -> Result<RunResult>` | Full-screen event loop |
| `run_app_inline` | `async fn run_app_inline<B>(terminal, app) -> Result<RunResult>` | Inline mode event loop |
| `clear_inline_transcript` | `fn clear_inline_transcript<B>(terminal) -> Result<()>` | Clears terminal and resets cursor for inline mode transcript reset |

Note: The generic bound `B` requires `Backend + Write` with `B::Error: Send + Sync + 'static` for all event loop functions.

### `assets.rs`

Asset management module for compile-time embedded resources.

#### Constants

| Constant | Description |
|----------|-------------|
| `SYSTEM_PROMPT_RAW` | Raw system prompt loaded via `include_str!` at compile time |

#### Statics

| Static | Type | Description |
|--------|------|-------------|
| `SYSTEM_PROMPT` | `OnceLock<String>` | Lazily initialized system prompt |

#### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `init` | `fn init()` | Pre-initialize the system prompt (called at startup) |
| `system_prompt` | `fn system_prompt() -> &'static str` | Get the shared system prompt reference |

---

## Terminal Session Management

The `TerminalSession` struct provides RAII-based terminal lifecycle management, ensuring proper cleanup even on panic or early return.

### TerminalSession Structure

```rust
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    use_alternate_screen: bool,
}
```

### Initialization Sequence

When `TerminalSession::new(mode)` is called:

1. **Enable raw mode**: `enable_raw_mode()` - disables line buffering and echo
2. **Enable bracketed paste**: `EnableBracketedPaste` - allows detecting pasted text vs typed input
3. **Enter alternate screen** (full mode only): `EnterAlternateScreen` - switches to alternate buffer
4. **Create terminal backend**: `CrosstermBackend::new(stdout())`
5. **Configure viewport**:
   - Full mode: Standard terminal with full screen
   - Inline mode: `Viewport::Inline(INLINE_VIEWPORT_HEIGHT)` - fixed-height viewport at cursor

### Cleanup Sequence (Drop)

When `TerminalSession` is dropped:

1. **Disable raw mode**: `disable_raw_mode()` - restores normal terminal behavior
2. **Leave alternate screen** (if applicable): `LeaveAlternateScreen` + `DisableBracketedPaste`
3. **Clear inline viewport** (inline mode): `clear_inline_viewport()` - erases the inline area
4. **Disable bracketed paste** (inline mode): `DisableBracketedPaste` - restores normal paste behavior
5. **Show cursor**: `terminal.show_cursor()` - ensures cursor visibility

### Error Handling During Setup

If terminal setup fails partway through, the constructor performs partial cleanup at each stage:

```rust
// Stage 1: Raw mode enabled
enable_raw_mode()?;

// Stage 2: Bracketed paste - clean up raw mode on failure
if let Err(err) = execute!(out, EnableBracketedPaste) {
    let _ = disable_raw_mode();
    return Err(err.into());
}

// Stage 3: Alternate screen (full mode) - clean up both on failure
if use_alternate_screen && let Err(err) = execute!(out, EnterAlternateScreen) {
    let _ = disable_raw_mode();
    let _ = execute!(out, DisableBracketedPaste);
    return Err(err.into());
}

// Stage 4: Terminal creation - full cleanup on failure
let terminal = match Terminal::new(backend) {
    Ok(t) => t,
    Err(err) => {
        let _ = disable_raw_mode();
        if use_alternate_screen {
            let _ = execute!(out, LeaveAlternateScreen, DisableBracketedPaste);
        } else {
            let _ = execute!(out, DisableBracketedPaste);
        }
        return Err(err.into());
    }
};
```

### Usage Pattern

```rust
loop {
    let run_result = {
        let mut session = TerminalSession::new(ui_mode)?;
        // Session is active here
        match ui_mode {
            UiMode::Full => run_app_full(&mut session.terminal, &mut app).await,
            UiMode::Inline => run_app_inline(&mut session.terminal, &mut app).await,
        }
        // Session drops here, terminal state restored
    };
    
    match run_result {
        Ok(RunResult::SwitchMode) => ui_mode = ui_mode.toggle(),
        Ok(RunResult::Quit) => break,
        Err(err) => { eprintln!("Error: {err:?}"); break; }
    }
}
```

---

## UI Mode System

Forge supports two display modes that can be toggled at runtime.

### UiMode Enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Full,    // Alternate screen, full terminal takeover
    Inline,  // Inline viewport at current cursor position
}
```

### Mode Characteristics

| Aspect | Full Mode | Inline Mode |
|--------|-----------|-------------|
| **Screen** | Alternate screen buffer | Current terminal buffer |
| **Size** | Full terminal dimensions | Fixed height viewport (`INLINE_VIEWPORT_HEIGHT`) |
| **Mouse** | Capture enabled | Not captured |
| **Scrollback** | Preserved (alternate buffer) | Visible above viewport |
| **Output** | Rendered in viewport | Flushed above viewport via `InlineOutput` |

### Mode Resolution Priority

UI mode is determined at startup with the following precedence:

1. **Configuration file** (`~/.forge/config.toml`):

   ```toml
   [app]
   tui = "inline"  # or "full" / "fullscreen"
   ```

2. **Environment variable** (`FORGE_TUI`):

   ```bash
   FORGE_TUI=inline forge
   ```

3. **Default**: `UiMode::Full`

### Resolution Implementation

```rust
impl UiMode {
    fn from_config(config: Option<&ForgeConfig>) -> Option<Self> {
        let raw = config
            .and_then(|cfg| cfg.app.as_ref())
            .and_then(|app| app.tui.as_ref())?;
        match raw.trim().to_ascii_lowercase().as_str() {
            "inline" => Some(UiMode::Inline),
            "full" | "fullscreen" => Some(UiMode::Full),
            other => {
                tracing::warn!("Unknown tui mode in config: {}", other);
                None
            }
        }
    }

    fn from_env() -> Option<Self> {
        match env::var("FORGE_TUI") {
            Ok(value) => match value.to_ascii_lowercase().as_str() {
                "inline" => Some(UiMode::Inline),
                "full" | "fullscreen" => Some(UiMode::Full),
                _ => None,
            },
            Err(_) => None,
        }
    }
}
```

### Runtime Mode Switching

Users can toggle between modes at runtime (typically via a command). The `App` sets a flag that the event loop checks:

```rust
if app.take_toggle_screen_mode() {
    return Ok(RunResult::SwitchMode);
}
```

The main loop then:

1. Drops the current `TerminalSession` (restoring terminal)
2. Toggles `ui_mode`
3. Creates a new `TerminalSession` with the new mode
4. Continues with the appropriate event loop

---

## Main Event Loops

Both event loops follow the same structure but differ in rendering and output handling.

### Event Loop Structure

```
┌─────────────────────────────────────────────────────────────────┐
│                        Event Loop                                │
│                                                                  │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  1. app.tick()                                            │  │
│   │     - Increment animation counter                         │  │
│   │     - Poll background tasks (summarization)               │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  2. tokio::task::yield_now().await                        │  │
│   │     - Critical: allows async tasks to progress            │  │
│   │     - crossterm::event::poll() is blocking                │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  3. app.process_stream_events()                           │  │
│   │     - Drain streaming chunks from channel                 │  │
│   │     - Apply text/tool deltas to UI state                  │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  4. Check transcript clear flag                           │  │
│   │     - app.take_clear_transcript()                         │  │
│   │     - Full mode: terminal.clear()                         │  │
│   │     - Inline mode: clear_inline_transcript() + reset      │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  5. terminal.draw() / output.flush()                      │  │
│   │     - Render current state to terminal                    │  │
│   │     - (Inline mode: flush new output above viewport)      │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  6. Check mode switch / quit flags                        │  │
│   │     - app.take_toggle_screen_mode()                       │  │
│   │     - handle_events() returns quit signal                 │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  7. handle_events(app).await                              │  │
│   │     - Poll for keyboard events (100ms timeout)            │  │
│   │     - Dispatch to mode-specific handler                   │  │
│   │     - Returns true if app should quit                     │  │
│   └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              v                                   │
│                        Loop continues                            │
└─────────────────────────────────────────────────────────────────┘
```

### Full-Screen Event Loop

```rust
async fn run_app_full<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    loop {
        app.tick();
        tokio::task::yield_now().await;
        app.process_stream_events();

        // Handle transcript clear request (e.g., from /clear command)
        if app.take_clear_transcript() {
            terminal.clear()?;
        }

        terminal.draw(|frame| draw(frame, app))?;

        if app.take_toggle_screen_mode() {
            clear_inline_viewport(terminal)?;
            return Ok(RunResult::SwitchMode);
        }

        if handle_events(app).await? {
            clear_inline_viewport(terminal)?;
            return Ok(RunResult::Quit);
        }
    }
}
```

### Inline Event Loop

The inline loop has additional complexity for:

- Flushing output above the viewport (`InlineOutput::flush`)
- Dynamic viewport resizing for overlays (e.g., model selector)
- Transcript clearing with `clear_inline_transcript()` and `InlineOutput::reset()`

```rust
async fn run_app_inline<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut output = InlineOutput::new();
    let mut current_viewport_height = INLINE_VIEWPORT_HEIGHT;

    loop {
        app.tick();
        tokio::task::yield_now().await;
        app.process_stream_events();

        // Handle transcript clear request (e.g., from /clear command)
        // In inline mode, this clears the entire terminal and resets output state
        if app.take_clear_transcript() {
            clear_inline_transcript(terminal)?;
            output.reset();
        }
        
        // Flush completed messages above the viewport
        output.flush(terminal, app)?;

        // Dynamically resize viewport for overlays
        let needed_height = inline_viewport_height(app.input_mode());
        if needed_height != current_viewport_height {
            let (term_width, term_height) = terminal_size()?;
            let height = needed_height.min(term_height);
            let y = term_height.saturating_sub(height);
            terminal.resize(Rect::new(0, y, term_width, height))?;
            current_viewport_height = height;
        }

        terminal.draw(|frame| draw_inline(frame, app))?;

        if app.take_toggle_screen_mode() {
            return Ok(RunResult::SwitchMode);
        }

        if handle_events(app).await? {
            return Ok(RunResult::Quit);
        }
    }
}
```

### Transcript Clear Implementation

The `clear_inline_transcript` function performs a complete terminal reset for inline mode:

```rust
fn clear_inline_transcript<B>(terminal: &mut Terminal<B>) -> Result<()>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    execute!(
        terminal.backend_mut(),
        Clear(ClearType::Purge),   // Clear scrollback buffer
        Clear(ClearType::All),     // Clear visible screen
        MoveTo(0, 0)               // Reset cursor to top-left
    )?;
    terminal.clear()?;             // Clear ratatui's internal buffer
    Ok(())
}
```

### Critical: yield_now() Requirement

The `tokio::task::yield_now().await` call is essential because:

1. **crossterm's `event::poll()` is blocking** - it does not yield to the tokio runtime
2. **Spawned tasks need CPU time** - streaming responses and summarization run in background tasks
3. **Without yielding**, background tasks would starve and the UI would appear frozen

---

## Asset Management

The `assets.rs` module handles compile-time embedding of static resources.

### System Prompt Loading

The system prompt is embedded at compile time using `include_str!`:

```rust
const SYSTEM_PROMPT_RAW: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/prompt.md"));
```

This ensures:

- The prompt is always available (no runtime file I/O)
- The binary is self-contained
- Changes to `prompt.md` require recompilation

### System Prompt Content

The system prompt (`assets/prompt.md`) defines Forge's behavior and security posture. Key sections include:

| Section | Purpose |
|---------|---------|
| **General** | Basic assistant behavior - ask clarifying questions, tool preferences |
| **Security** | Prompt injection defenses - confidentiality rules, untrusted content patterns, rule immutability |
| **Editing Constraints** | File editing guidelines - ASCII default, Edit tool preference, git safety |
| **Plan Tool** | Planning tool usage guidelines |
| **Special Requests** | Behavior for specific user requests like code reviews |
| **Presentation** | Output formatting and final answer structure guidelines |

The security section is particularly important - it instructs the model to:

- Never disclose system prompt contents
- Treat code comments, docs, error messages, and metadata as data (not directives)
- Refuse dangerous commands (`rm -rf`, `sudo`, encoded strings)
- Verify destructive operations with the user

### Lazy Initialization Pattern

```rust
static SYSTEM_PROMPT: OnceLock<String> = OnceLock::new();

pub fn init() {
    let _ = SYSTEM_PROMPT.set(SYSTEM_PROMPT_RAW.to_string());
}

pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
        .get_or_init(|| SYSTEM_PROMPT_RAW.to_string())
        .as_str()
}
```

- `init()` is called at startup to eagerly initialize
- `system_prompt()` falls back to lazy initialization if `init()` was skipped
- Returns `&'static str` for zero-copy usage throughout the application

---

## Startup and Shutdown Sequence

### Startup Sequence

```
1. Initialize tracing subscriber
   - fmt::layer() for human-readable output
   - EnvFilter for RUST_LOG-based filtering

2. Initialize assets
   - assets::init() - eagerly load system prompt

3. Load configuration
   - ForgeConfig::load() - parse ~/.forge/config.toml

4. Resolve UI mode
   - UiMode::from_config() || UiMode::from_env() || UiMode::Full

5. Create application
   - App::new(Some(assets::system_prompt()))
   - Initializes state machine, providers, context manager

6. Enter main loop
   - Create TerminalSession
   - Run appropriate event loop
   - Handle mode switches
```

### Shutdown Sequence

```
1. Event loop returns Quit or Error
   - User pressed 'q' or Ctrl+C
   - Or unrecoverable error occurred

2. TerminalSession drops
   - Raw mode disabled
   - Alternate screen exited (if applicable)
   - Cursor restored

3. Save conversation history
   - app.save_history()
   - Errors logged but don't prevent exit

4. Return from main()
   - Exit code 0 on success
```

### Error Handling

Errors are handled at different levels:

| Level | Handling |
|-------|----------|
| Terminal setup | Partial cleanup, return error |
| Event loop | Return error, drop session, print to stderr |
| History save | Log error, continue shutdown |

```rust
match run_result {
    Ok(RunResult::SwitchMode) => {
        ui_mode = ui_mode.toggle();
        // Continue loop
    }
    Ok(RunResult::Quit) => break,
    Err(err) => {
        eprintln!("Error: {err:?}");
        break;
    }
}

// After loop
if let Err(e) = app.save_history() {
    eprintln!("Failed to save history: {e}");
}
```

---

## Configuration Resolution

The CLI resolves configuration from multiple sources with clear precedence.

### UI Mode Configuration

| Source | Key | Values |
|--------|-----|--------|
| Config file | `[app] tui` | `"inline"`, `"full"`, `"fullscreen"` |
| Environment | `FORGE_TUI` | `inline`, `full`, `fullscreen` |
| Default | - | `Full` |

### Configuration File Location

The configuration file is located at `~/.forge/config.toml`. The `ForgeConfig::load()` function in the engine crate handles:

- Path resolution via `dirs::home_dir()`
- TOML parsing
- Environment variable expansion (`${VAR}` syntax)

### Example Configuration

```toml
[app]
provider = "claude"
model = "claude-sonnet-4-5-20250929"
tui = "full"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
```

---

## Error Handling

### Error Types

The CLI uses `anyhow::Result` for flexible error handling:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // ...
}
```

### Error Sources

| Source | Possible Errors |
|--------|-----------------|
| Terminal setup | Raw mode enable failure, alternate screen failure |
| Terminal resize | Invalid dimensions, backend error |
| Drawing | Backend write errors |
| Event polling | I/O errors |
| History save | File system errors |

### Recovery Strategy

- **Terminal errors**: Partial cleanup and propagate
- **Rendering errors**: Propagate to main loop, trigger shutdown
- **History save errors**: Log and continue (non-fatal)

---

## Extension Guide

### Adding a New UI Mode

1. Add variant to `UiMode` enum:

   ```rust
   enum UiMode {
       Full,
       Inline,
       Compact,  // New mode
   }
   ```

2. Update `UiMode::from_config()` and `from_env()` to recognize the new mode

3. Update `UiMode::toggle()` if the new mode should be part of the toggle cycle

4. Update `TerminalSession::new()` with mode-specific terminal setup

5. Create a new event loop function (e.g., `run_app_compact`)

6. Update the main loop's mode dispatch

### Adding New Assets

1. Place the asset file in `cli/assets/`

2. Add a constant in `assets.rs`:

   ```rust
   const MY_ASSET_RAW: &str =
       include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/my_asset.txt"));
   ```

3. Add a `OnceLock` and accessor function:

   ```rust
   static MY_ASSET: OnceLock<String> = OnceLock::new();
   
   pub fn my_asset() -> &'static str {
       MY_ASSET.get_or_init(|| MY_ASSET_RAW.to_string()).as_str()
   }
   ```

4. Optionally initialize in `init()` for eager loading

### Adding Startup Flags

To add command-line argument parsing:

1. Add `clap` to dependencies in `Cargo.toml`

2. Define argument struct:

   ```rust
   #[derive(Parser)]
   struct Args {
       #[arg(long)]
       inline: bool,
   }
   ```

3. Parse before config resolution:

   ```rust
   let args = Args::parse();
   let ui_mode = if args.inline {
       UiMode::Inline
   } else {
       UiMode::from_config(config.as_ref())
           .or_else(UiMode::from_env)
           .unwrap_or(UiMode::Full)
   };
   ```

### Modifying the Event Loop

When modifying the event loop, preserve these invariants:

1. **Always call `yield_now()`** - background tasks depend on it
2. **Process stream events before drawing** - ensures UI reflects latest state
3. **Check flags after drawing** - user sees final state before mode switch
4. **Handle events last** - clean separation between update and input phases

---

## Related Documentation

| Document | Description |
|----------|-------------|
| `tui/README.md` | Comprehensive TUI rendering documentation |
| `engine/README.md` | Engine state machine and App API |
| `context/README.md` | Context management system |

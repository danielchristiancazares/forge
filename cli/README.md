# forge-cli

This document provides comprehensive documentation for the `forge` CLI crate - the binary entry point and terminal session management layer for the Forge LLM client. It is intended for developers who want to understand, maintain, or extend the CLI functionality.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-37 | Header, Table of Contents |
| 38-80 | Overview: responsibilities, file structure, dependencies |
| 81-115 | Architecture Diagram: main() flow, terminal session lifecycle |
| 116-177 | Module Structure: main.rs types and functions, assets.rs constants and statics |
| 178-260 | Terminal Session Management: TerminalSession, init/cleanup sequences, error handling |
| 261-272 | Terminal Mode |
| 273-325 | Main Event Loop: input pump, frame cadence, run_app |
| 326-427 | Asset Management: compile-time embedding, provider-specific prompts, OnceLock initialization |
| 428-525 | Startup and Shutdown Sequence: initialization order, cleanup guarantees |
| 526-560 | Configuration Resolution: file location, example |
| 561-595 | Error Handling: error types, sources, recovery strategy |
| 596-650 | Extension Guide: adding assets, startup flags, modifying event loop |
| 651-660 | Related Documentation: links to other crate READMEs |

## Table of Contents

1. [Overview](#overview)
2. [Architecture Diagram](#architecture-diagram)
3. [Module Structure](#module-structure)
4. [Terminal Session Management](#terminal-session-management)
5. [Terminal Mode](#terminal-mode)
6. [Main Event Loop](#main-event-loop)
7. [Asset Management](#asset-management)
8. [Startup and Shutdown Sequence](#startup-and-shutdown-sequence)
9. [Configuration Resolution](#configuration-resolution)
10. [Error Handling](#error-handling)
11. [Extension Guide](#extension-guide)
12. [Related Documentation](#related-documentation)

---

## Overview

The `forge` CLI crate is the application entry point that orchestrates terminal setup, tracing initialization, and the main event loop. It bridges the `forge-engine` (application state) and `forge-tui` (rendering) crates, providing RAII-based terminal session management with proper cleanup guarantees.

### Key Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Terminal Session** | RAII-based setup/teardown of raw mode, alternate screen, bracketed paste |
| **Event Loop Execution** | Tick-based loop coordinating async tasks, streaming, rendering, and input |
| **Asset Loading** | Compile-time embedding and runtime initialization of provider-specific system prompts |
| **Tracing** | File-based logging to `~/.forge/logs/forge.log` with fallback candidates |

### File Structure

```
cli/
├── Cargo.toml                              # Binary manifest (package name: "forge")
├── assets/
│   ├── base_prompt.md                      # Base system prompt (Claude, OpenAI)
│   └── base_prompt_gemini.md               # Gemini-specific system prompt
└── src/
    ├── main.rs                             # Entry point, event loop, terminal session
    └── assets.rs                           # Compile-time asset embedding
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `forge-engine` | Application state machine (`App`, `ForgeConfig`, `SystemPrompts`) |
| `forge-tui` | Rendering functions (`draw`, `handle_events`, `InputPump`) |
| `ratatui` | Terminal UI framework |
| `crossterm` | Cross-platform terminal manipulation |
| `tokio` | Async runtime (multi-thread, macros) |
| `anyhow` | Flexible error handling |
| `tracing` | Structured logging facade |
| `tracing-subscriber` | Logging infrastructure (fmt layer, env filter) |

Dev-dependencies: `forge-types`, `wiremock`, `serde_json`, `tempfile`, `insta`, `vt100`.

---

## Architecture Diagram

```
+-------------------------------------------------------------------------+
|                              main()                                      |
|  +--------------------------------------------------------------------+ |
|  |  1. init_tracing() - file-based logging                            | |
|  |  2. assets::init() - eagerly load system prompts                   | |
|  |  3. App::new(assets::system_prompts()) - create app                | |
|  +--------------------------------------------------------------------+ |
|                                |                                         |
|                                v                                         |
|  +--------------------------------------------------------------------+ |
|  |  TerminalSession::new()  <- RAII terminal setup (alternate screen) | |
|  |  run_app(&mut terminal, &mut app)  <- Main event loop              | |
|  +--------------------------------------------------------------------+ |
|                                |                                         |
|                                v                                         |
|  +--------------------------------------------------------------------+ |
|  |  app.shutdown_lsp()     <- Shut down LSP servers                   | |
|  |  app.save_history()     <- Persist conversation on exit            | |
|  |  app.save_session()     <- Persist session state on exit           | |
|  +--------------------------------------------------------------------+ |
+-------------------------------------------------------------------------+
```

---

## Module Structure

### `main.rs`

The primary module containing the application entry point, terminal session management, tracing initialization, and the main event loop.

#### Types

| Type | Description |
|------|-------------|
| `TerminalSession` | RAII wrapper for terminal state with guaranteed cleanup on drop |

#### Constants

| Constant | Type | Description |
|----------|------|-------------|
| `FRAME_DURATION` | `Duration` | 8ms frame interval (~120 FPS render cadence) |

#### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `main` | `async fn main() -> Result<()>` | Application entry point |
| `run_app` | `async fn run_app<B>(terminal, app) -> Result<()>` | Main event loop (generic over backend) |
| `init_tracing` | `fn init_tracing()` | Initialize file-based tracing subscriber |
| `open_forge_log_file` | `fn open_forge_log_file() -> (Option<(PathBuf, File)>, Vec<String>)` | Try candidate log file paths, return first success and any warnings |
| `forge_log_file_candidates` | `fn forge_log_file_candidates() -> Vec<PathBuf>` | Return ordered list of candidate log file paths |

Note: The generic bound `B` on `run_app` requires `Backend + Write` with `B::Error: Send + Sync + 'static`.

### `assets.rs`

Asset management module for compile-time embedded resources with provider-specific prompt support.

#### Constants

| Constant | Description |
|----------|-------------|
| `BASE_PROMPT_RAW` | Base system prompt loaded via `include_str!` from `assets/base_prompt.md` |
| `GEMINI_PROMPT_RAW` | Gemini-specific system prompt loaded via `include_str!` from `assets/base_prompt_gemini.md` |

#### Statics

| Static | Type | Description |
|--------|------|-------------|
| `BASE_PROMPT` | `OnceLock<String>` | Lazily initialized base system prompt |
| `GEMINI_PROMPT` | `OnceLock<String>` | Lazily initialized Gemini-specific system prompt |

#### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `init` | `fn init()` | Pre-initialize all system prompts (called at startup) |
| `system_prompts` | `fn system_prompts() -> SystemPrompts` | Get provider-specific system prompts struct |

---

## Terminal Session Management

The `TerminalSession` struct provides RAII-based terminal lifecycle management, ensuring proper cleanup even on panic or early return.

### TerminalSession Structure

```rust
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}
```

### Initialization Sequence

When `TerminalSession::new()` is called:

1. **Enable raw mode**: `enable_raw_mode()` - disables line buffering and echo
2. **Enable bracketed paste**: `EnableBracketedPaste` - allows detecting pasted text vs typed input
3. **Enter alternate screen**: `EnterAlternateScreen` - switches to alternate buffer
4. **Enable alternate scroll mode**: `CSI ? 1007 h` - map scroll wheel to arrow keys without mouse capture
5. **Create terminal backend**: `CrosstermBackend::new(stdout())`

### Cleanup Sequence (Drop)

When `TerminalSession` is dropped:

1. **Disable raw mode**: `disable_raw_mode()` - restores normal terminal behavior
2. **Disable alternate scroll mode**: `CSI ? 1007 l`
3. **Leave alternate screen**: `LeaveAlternateScreen` + `DisableBracketedPaste`
4. **Show cursor**: `terminal.show_cursor()` - ensures cursor visibility

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

// Stage 3: Alternate screen - clean up raw mode and bracketed paste on failure
if let Err(err) = execute!(out, EnterAlternateScreen) {
    let _ = disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen, DisableBracketedPaste);
    return Err(err.into());
}

// Stage 4: Terminal creation - full cleanup on failure
let terminal = match Terminal::new(backend) {
    Ok(t) => t,
    Err(err) => {
        let _ = disable_raw_mode();
        // Disable alternate scroll mode
        let _ = out.write_all(b"\x1b[?1007l");
        let _ = out.flush();
        let _ = execute!(out, LeaveAlternateScreen, DisableBracketedPaste);
        return Err(err.into());
    }
};
```

### Usage Pattern

```rust
{
    let mut session = TerminalSession::new()?;
    run_app(&mut session.terminal, &mut app).await?;
    // Session drops here, terminal state restored
}
```

---

## Terminal Mode

Forge uses crossterm's alternate screen for full terminal control:
- Alternate screen buffer preserves original terminal scrollback
- Scroll wheel mapped to arrow keys via mode 1007 (no mouse capture)
- RAII cleanup via `TerminalSession` drop

---

## Main Event Loop

The event loop runs at a fixed 8ms (~120 FPS) cadence:

```
1. frames.tick().await (fixed cadence)
2. handle_events(app, input) -- drain input queue (non-blocking)
3. app.tick() -- advance animations, poll background tasks
4. app.process_stream_events() -- apply streaming chunks to UI
5. Clear transcript if requested (terminal.clear())
6. terminal.draw(...) -- render frame
```

```rust
async fn run_app<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut input = InputPump::new();
    let mut frames = tokio::time::interval(FRAME_DURATION);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result: Result<()> = loop {
        frames.tick().await;

        // Non-blocking input (drain queue only)
        let quit_now = match handle_events(app, &mut input) {
            Ok(q) => q,
            Err(e) => break Err(e),
        };
        if quit_now {
            break Ok(());
        }

        app.tick();
        app.process_stream_events();

        if app.take_clear_transcript()
            && let Err(e) = terminal.clear()
        {
            break Err(e.into());
        }

        if let Err(e) = terminal.draw(|frame| draw(frame, app)) {
            break Err(e.into());
        }
    };

    input.shutdown().await;
    result
}
```

### InputPump and Non-Blocking Input

Input handling is delegated to a dedicated blocking reader (`InputPump`) that pushes
events into a bounded channel. The render loop only drains the queue (`handle_events`)
so it never blocks on terminal input. This keeps the UI responsive and avoids the
need for explicit `yield_now()` calls in the main loop.

---

## Asset Management

The `assets.rs` module handles compile-time embedding of static resources with support for provider-specific system prompts.

### System Prompt Loading

System prompts are embedded at compile time using `include_str!`:

```rust
const BASE_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt.md"
));

const GEMINI_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt_gemini.md"
));
```

This ensures:

- Prompts are always available (no runtime file I/O)
- The binary is self-contained
- Changes to prompt files require recompilation

### Provider-Specific Prompts

The `SystemPrompts` struct provides different prompts optimized for each LLM provider. The `claude` and `openai` fields both reference the same base prompt, while `gemini` gets its own specialized prompt:

```rust
pub fn system_prompts() -> SystemPrompts {
    let base = BASE_PROMPT
        .get_or_init(|| BASE_PROMPT_RAW.to_string())
        .as_str();
    SystemPrompts {
        claude: base,
        openai: base,
        gemini: GEMINI_PROMPT
            .get_or_init(|| GEMINI_PROMPT_RAW.to_string())
            .as_str(),
    }
}
```

| Field | File | Used By |
|-------|------|---------|
| `claude` | `assets/base_prompt.md` | Claude models |
| `openai` | `assets/base_prompt.md` | OpenAI models (same base prompt) |
| `gemini` | `assets/base_prompt_gemini.md` | Gemini models |

### Prompt Ownership

Context distillation and librarian prompts live in `context/assets/` and are
owned by the `forge-context` crate. The CLI crate only embeds provider system
prompts from `cli/assets/`.

### Lazy Initialization Pattern

```rust
static BASE_PROMPT: OnceLock<String> = OnceLock::new();
static GEMINI_PROMPT: OnceLock<String> = OnceLock::new();

pub fn init() {
    let _ = BASE_PROMPT.set(BASE_PROMPT_RAW.to_string());
    let _ = GEMINI_PROMPT.set(GEMINI_PROMPT_RAW.to_string());
}

pub fn system_prompts() -> SystemPrompts {
    let base = BASE_PROMPT
        .get_or_init(|| BASE_PROMPT_RAW.to_string())
        .as_str();
    SystemPrompts {
        claude: base,
        openai: base,
        gemini: GEMINI_PROMPT
            .get_or_init(|| GEMINI_PROMPT_RAW.to_string())
            .as_str(),
    }
}
```

- `init()` is called at startup to eagerly initialize all prompts
- `system_prompts()` falls back to lazy initialization if `init()` was skipped
- Returns `SystemPrompts` with `&'static str` fields for zero-copy usage

---

## Startup and Shutdown Sequence

### Startup Sequence

```
1. Initialize tracing subscriber (init_tracing)
   - Resolve log file: ~/.forge/logs/forge.log (primary)
     or ./.forge/logs/forge.log (fallback)
   - fmt::layer() writing to log file (ANSI disabled)
   - EnvFilter for RUST_LOG-based filtering (default: info)
   - If no log file can be opened: no-op subscriber (avoids corrupting TUI)

2. Initialize assets
   - assets::init() - eagerly load all system prompts into OnceLock

3. Create application
   - App::new(assets::system_prompts())
   - Initializes state machine, providers, context manager

4. Enter main loop
   - Create TerminalSession (alternate screen)
   - Run event loop
```

### Tracing Initialization Detail

The `init_tracing()` function sets up file-based logging to avoid corrupting TUI output:

1. Build an `EnvFilter` from `RUST_LOG` env var, falling back to `"info"`, then `"warn"`
2. Try candidate log file paths via `forge_log_file_candidates()`:
   - Primary: `~/.forge/logs/forge.log` (derived from `ForgeConfig::path()`)
   - Fallback: `./.forge/logs/forge.log` (for constrained environments)
3. Open the first candidate that succeeds (create dirs as needed)
4. If a log file is available: register a `fmt::layer()` writing to it (ANSI disabled, wrapped in `Mutex`)
5. If no log file can be opened: register a no-op subscriber (silent, no output)
6. Log any warnings accumulated during candidate resolution

### Shutdown Sequence

```
1. Event loop returns Quit or Error
   - User pressed 'q' or Ctrl+C
   - Or unrecoverable error occurred

2. TerminalSession drops (scoped block)
   - Raw mode disabled
   - Alternate screen exited
   - Cursor restored

3. Shut down LSP servers
   - app.shutdown_lsp().await
   - Graceful LSP server shutdown

4. Save conversation history
   - app.save_history()
   - Errors printed to stderr but don't prevent exit

5. Save session state
   - app.save_session()
   - Errors printed to stderr but don't prevent exit

6. Return from main()
   - Exit code 0 on success
```

### Error Handling

Errors are handled at different levels:

| Level | Handling |
|-------|----------|
| Terminal setup | Partial cleanup, return error |
| Event loop | Return error, drop session, print to stderr |
| LSP shutdown | Best-effort, always continues |
| History save | Print to stderr, continue shutdown |
| Session save | Print to stderr, continue shutdown |

```rust
// After event loop and session drop
app.shutdown_lsp().await;

if let Err(e) = app.save_history() {
    eprintln!("Failed to save history: {e}");
}

if let Err(e) = app.save_session() {
    eprintln!("Failed to save session: {e}");
}
```

---

## Configuration Resolution

The CLI resolves configuration from multiple sources with clear precedence.

### Configuration File Location

The configuration file is located at `~/.forge/config.toml`. The `ForgeConfig::load()` function in the engine crate handles:

- Path resolution via `dirs::home_dir()`
- TOML parsing
- Environment variable expansion (`${VAR}` syntax)

### Example Configuration

```toml
[app]
model = "claude-opus-4-6"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"
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
| Session save | File system errors |
| Log file open | Directory creation failure, permission errors |

### Recovery Strategy

- **Terminal errors**: Partial cleanup and propagate
- **Rendering errors**: Propagate to main loop, trigger shutdown
- **History/session save errors**: Print to stderr and continue (non-fatal)
- **Log file errors**: Fall back to no-op subscriber (silent)

---

## Extension Guide

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
       verbose: bool,
   }
   ```

3. Parse before app creation and use as needed

### Modifying the Event Loop

When modifying the event loop, preserve these invariants:

1. **Drain input before ticking** - keep input responsive via `InputPump`
2. **Process stream events before drawing** - ensures UI reflects latest state
3. **Check flags after drawing** - user sees final state before mode switch
4. **Shut down `InputPump` on exit** - clean termination before mode switch

---

## Related Documentation

| Document | Description |
|----------|-------------|
| `tui/README.md` | Comprehensive TUI rendering documentation |
| `engine/README.md` | Engine state machine and App API |
| `context/README.md` | Context management system |

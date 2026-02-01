//! Forge CLI - Binary entry point and terminal session management.
//!
//! This crate is the application entry point that orchestrates:
//! - Terminal session lifecycle (raw mode, alternate screen, bracketed paste)
//! - UI mode selection (full-screen vs inline) from config and environment
//! - Tick-based event loops coordinating async tasks, streaming, and rendering
//! - Runtime mode switching between display modes
//!
//! # Architecture
//!
//! The CLI bridges [`forge_engine`] (application state) and [`forge_tui`] (rendering),
//! providing RAII-based terminal management with guaranteed cleanup.
//!
//! ```text
//! main() -> TerminalSession::new(mode) -> run_app_{full,inline}() -> App + TUI
//!                                              |
//!                                              v
//!                               RunResult::Quit | SwitchMode
//! ```
//!
//! # Event Loop
//!
//! Both full-screen and inline modes use a fixed 8ms (~120 FPS) render cadence:
//!
//! 1. Wait for frame tick
//! 2. Drain input queue (non-blocking via [`forge_tui::InputPump`])
//! 3. Advance application state (`app.tick()`)
//! 4. Process streaming events from LLM
//! 5. Handle transcript clear requests
//! 6. Render frame
//! 7. Check for mode switch or quit

mod assets;

use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode, size as terminal_size,
    },
};
use ratatui::{TerminalOptions, Viewport, prelude::*};
use std::{
    env,
    io::{Stdout, Write, stdout},
    time::Duration,
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use forge_engine::{App, ForgeConfig};
use forge_tui::{
    INLINE_VIEWPORT_HEIGHT, InlineOutput, InputPump, clear_inline_viewport, draw, draw_inline,
    handle_events, inline_viewport_height,
};

/// Display mode for the terminal UI.
///
/// Forge supports two rendering modes that can be toggled at runtime:
/// - `Full`: Takes over the entire terminal using an alternate screen buffer
/// - `Inline`: Renders in a fixed-height viewport at the current cursor position
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    /// Full-screen mode using the alternate screen buffer.
    /// Preserves the user's scrollback history and provides the full terminal area.
    Full,
    /// Inline mode using a fixed-height viewport.
    /// Renders at the current cursor position, allowing previous terminal content
    /// to remain visible above.
    Inline,
}

impl UiMode {
    fn toggle(self) -> Self {
        match self {
            UiMode::Full => UiMode::Inline,
            UiMode::Inline => UiMode::Full,
        }
    }
}

/// Result of an event loop iteration indicating whether to quit or switch modes.
enum RunResult {
    /// User requested application exit.
    Quit,
    /// User requested switching between full-screen and inline modes.
    SwitchMode,
}

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

/// RAII wrapper for terminal state with guaranteed cleanup on drop.
///
/// Manages the terminal lifecycle including:
/// - Raw mode (disables line buffering and echo)
/// - Bracketed paste (detects pasted text vs typed input)
/// - Alternate screen (full mode only)
/// - Alternate scroll mode (maps scroll wheel to arrows without mouse capture)
///
/// On drop, all terminal state is restored to its original configuration,
/// ensuring the terminal remains usable even after panics or early returns.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    use_alternate_screen: bool,
}

impl TerminalSession {
    fn new(mode: UiMode) -> Result<Self> {
        enable_raw_mode()?;

        let mut out = stdout();
        if let Err(err) = execute!(out, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
        let use_alternate_screen = matches!(mode, UiMode::Full);
        if use_alternate_screen {
            // Enter alternate screen and enable alternate scroll mode (mode 1007).
            // Mode 1007 converts scroll wheel events to Up/Down arrow keys when in
            // alternate screen, WITHOUT capturing mouse clicks. This preserves
            // native text selection while still allowing scroll wheel to work.
            if let Err(err) = execute!(out, EnterAlternateScreen) {
                let _ = disable_raw_mode();
                let _ = execute!(out, LeaveAlternateScreen, DisableBracketedPaste);
                return Err(err.into());
            }
            // Enable alternate scroll mode: CSI ? 1007 h
            let _ = out.write_all(b"\x1b[?1007h");
            let _ = out.flush();
        }

        let backend = CrosstermBackend::new(out);
        let terminal = match mode {
            UiMode::Full => Terminal::new(backend),
            UiMode::Inline => Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
                },
            ),
        };
        let terminal = match terminal {
            Ok(t) => t,
            Err(err) => {
                let _ = disable_raw_mode();
                if use_alternate_screen {
                    let mut out = stdout();
                    // Disable alternate scroll mode: CSI ? 1007 l
                    let _ = out.write_all(b"\x1b[?1007l");
                    let _ = out.flush();
                    let _ = execute!(out, LeaveAlternateScreen, DisableBracketedPaste);
                } else {
                    let mut out = stdout();
                    let _ = execute!(out, DisableBracketedPaste);
                }
                return Err(err.into());
            }
        };

        Ok(Self {
            terminal,
            use_alternate_screen,
        })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.use_alternate_screen {
            // Disable alternate scroll mode: CSI ? 1007 l
            let _ = self.terminal.backend_mut().write_all(b"\x1b[?1007l");
            let _ = std::io::Write::flush(&mut *self.terminal.backend_mut());
            let _ = execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableBracketedPaste
            );
        } else {
            let _ = clear_inline_viewport(&mut self.terminal);
            let _ = execute!(self.terminal.backend_mut(), DisableBracketedPaste);
        }
        let _ = self.terminal.show_cursor();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    assets::init();

    let config = ForgeConfig::load().ok().flatten();
    let mut ui_mode = UiMode::from_config(config.as_ref())
        .or_else(UiMode::from_env)
        .unwrap_or(UiMode::Full);
    let mut app = App::new(assets::system_prompts())?;

    loop {
        let run_result = {
            let mut session = TerminalSession::new(ui_mode)?;
            match ui_mode {
                UiMode::Full => run_app_full(&mut session.terminal, &mut app).await,
                UiMode::Inline => run_app_inline(&mut session.terminal, &mut app).await,
            }
        };

        match run_result {
            Ok(RunResult::SwitchMode) => {
                ui_mode = ui_mode.toggle();
            }
            Ok(RunResult::Quit) => break,
            Err(err) => {
                eprintln!("Error: {err:?}");
                break;
            }
        }
    }

    if let Err(e) = app.save_history() {
        eprintln!("Failed to save history: {e}");
    }

    if let Err(e) = app.save_session() {
        eprintln!("Failed to save session: {e}");
    }

    Ok(())
}

/// Target frame duration for the render loop (~120 FPS cap).
///
/// This cadence balances UI responsiveness with CPU usage. The interval uses
/// `MissedTickBehavior::Skip` to avoid frame buildup during slow renders.
const FRAME_DURATION: Duration = Duration::from_millis(8);

/// Runs the full-screen event loop with alternate screen rendering.
///
/// # Event loop steps
///
/// 1. Wait for frame tick (8ms cadence)
/// 2. Drain input queue via [`InputPump`] (non-blocking)
/// 3. Advance app state and process streaming events
/// 4. Clear terminal if transcript reset requested
/// 5. Draw frame using [`draw`]
/// 6. Check for mode switch flag
///
/// # Returns
///
/// - `Ok(RunResult::Quit)` when user requests exit
/// - `Ok(RunResult::SwitchMode)` when user toggles display mode
async fn run_app_full<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut input = InputPump::new();
    let mut frames = tokio::time::interval(FRAME_DURATION);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result: Result<RunResult> = loop {
        frames.tick().await;

        // Non-blocking input (drain queue only)
        let quit_now = match handle_events(app, &mut input) {
            Ok(q) => q,
            Err(e) => break Err(e),
        };
        if quit_now {
            let _ = clear_inline_viewport(terminal);
            break Ok(RunResult::Quit);
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

        if app.take_toggle_screen_mode() {
            if let Err(e) = clear_inline_viewport(terminal) {
                break Err(e.into());
            }
            break Ok(RunResult::SwitchMode);
        }
    };

    input.shutdown().await;
    result
}

/// Runs the inline event loop with a fixed-height viewport.
///
/// Unlike full-screen mode, inline mode:
/// - Flushes LLM output above the viewport via [`InlineOutput`]
/// - Dynamically resizes the viewport for overlays (e.g., model selector)
/// - Uses `clear_inline_transcript` for transcript resets
///
/// # Event loop steps
///
/// 1. Wait for frame tick (8ms cadence)
/// 2. Drain input queue via [`InputPump`] (non-blocking)
/// 3. Advance app state and process streaming events
/// 4. Clear transcript if requested (full terminal clear + output reset)
/// 5. Flush pending output above viewport
/// 6. Resize viewport if input mode changed (overlays need more height)
/// 7. Draw frame using [`draw_inline`]
/// 8. Check for mode switch flag
async fn run_app_inline<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut output = InlineOutput::new();
    let mut current_viewport_height = INLINE_VIEWPORT_HEIGHT;
    let mut input = InputPump::new();
    let mut frames = tokio::time::interval(FRAME_DURATION);
    frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result: Result<RunResult> = loop {
        frames.tick().await;

        // Non-blocking input (drain queue only)
        let quit_now = match handle_events(app, &mut input) {
            Ok(q) => q,
            Err(e) => break Err(e),
        };
        if quit_now {
            break Ok(RunResult::Quit);
        }

        app.tick();
        app.process_stream_events();

        if app.take_clear_transcript() {
            if let Err(e) = clear_inline_transcript(terminal) {
                break Err(e);
            }
            output.reset();
        }

        if let Err(e) = output.flush(terminal, app) {
            break Err(e.into());
        }

        // Dynamically resize viewport for overlays (e.g., model selector needs more height)
        let needed_height = inline_viewport_height(app.input_mode());
        if needed_height != current_viewport_height {
            let (term_width, term_height) = match terminal_size() {
                Ok(s) => s,
                Err(e) => break Err(e.into()),
            };
            // Clamp height to terminal size to avoid rendering outside bounds
            let height = needed_height.min(term_height);
            let y = term_height.saturating_sub(height);
            if let Err(e) = terminal.resize(Rect::new(0, y, term_width, height)) {
                break Err(e.into());
            }
            current_viewport_height = height;
        }

        if let Err(e) = terminal.draw(|frame| draw_inline(frame, app)) {
            break Err(e.into());
        }

        if app.take_toggle_screen_mode() {
            break Ok(RunResult::SwitchMode);
        }
    };

    input.shutdown().await;
    result
}

/// Clears the entire terminal for inline mode transcript resets.
///
/// Performs a complete terminal reset:
/// 1. `ClearType::Purge` - clears scrollback buffer
/// 2. `ClearType::All` - clears visible screen
/// 3. `MoveTo(0, 0)` - resets cursor to top-left
/// 4. `terminal.clear()` - clears ratatui's internal buffer
fn clear_inline_transcript<B>(terminal: &mut Terminal<B>) -> Result<()>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    execute!(
        terminal.backend_mut(),
        Clear(ClearType::Purge),
        Clear(ClearType::All),
        MoveTo(0, 0)
    )?;
    terminal.clear()?;
    Ok(())
}

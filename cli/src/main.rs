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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Full,
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

enum RunResult {
    Quit,
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
    let mut app = App::new(Some(assets::system_prompts()))?;

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

/// Fixed 8ms render cadence (~120 FPS cap).
const FRAME_DURATION: Duration = Duration::from_millis(8);

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

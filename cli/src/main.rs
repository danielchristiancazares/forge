mod assets;

use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    event::{DisableMouseCapture, EnableMouseCapture},
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
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use forge_engine::{App, ForgeConfig};
use forge_tui::{
    INLINE_VIEWPORT_HEIGHT, InlineOutput, clear_inline_viewport, draw, draw_inline, handle_events,
    inline_viewport_height,
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

/// Result of running the app loop.
enum RunResult {
    /// User requested quit.
    Quit,
    /// User requested to switch screen mode.
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
        let use_alternate_screen = matches!(mode, UiMode::Full);
        if use_alternate_screen
            && let Err(err) = execute!(out, EnterAlternateScreen, EnableMouseCapture)
        {
            let _ = disable_raw_mode();
            return Err(err.into());
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
                    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
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
            let _ = execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            );
        } else {
            let _ = clear_inline_viewport(&mut self.terminal);
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

    let config = ForgeConfig::load();
    let mut ui_mode = UiMode::from_config(config.as_ref())
        .or_else(UiMode::from_env)
        .unwrap_or(UiMode::Full);
    let mut app = App::new(Some(assets::system_prompt()))?;

    loop {
        let run_result = {
            let mut session = TerminalSession::new(ui_mode)?;
            // Session drops here, restoring terminal state
            match ui_mode {
                UiMode::Full => run_app_full(&mut session.terminal, &mut app).await,
                UiMode::Inline => run_app_inline(&mut session.terminal, &mut app).await,
            }
        };

        match run_result {
            Ok(RunResult::SwitchMode) => {
                ui_mode = ui_mode.toggle();
                // Continue loop with new mode
            }
            Ok(RunResult::Quit) => break,
            Err(err) => {
                eprintln!("Error: {err:?}");
                break;
            }
        }
    }

    // Save history before exit
    if let Err(e) = app.save_history() {
        eprintln!("Failed to save history: {e}");
    }

    Ok(())
}

async fn run_app_full<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    loop {
        app.tick();

        // Yield to allow spawned tasks (streaming, summarization) to make progress.
        // This is critical because crossterm's event::poll() is blocking and
        // doesn't yield to the tokio runtime.
        tokio::task::yield_now().await;

        app.process_stream_events();

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

async fn run_app_inline<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<RunResult>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut output = InlineOutput::new();
    let mut current_viewport_height = INLINE_VIEWPORT_HEIGHT;

    loop {
        app.tick();

        // Yield to allow spawned tasks (streaming, summarization) to make progress.
        // This is critical because crossterm's event::poll() is blocking and
        // doesn't yield to the tokio runtime.
        tokio::task::yield_now().await;

        app.process_stream_events();

        if app.take_clear_transcript() {
            clear_inline_transcript(terminal)?;
            output.reset();
        }

        output.flush(terminal, app)?;

        // Dynamically resize viewport for overlays (e.g., model selector needs more height)
        let needed_height = inline_viewport_height(app.input_mode());
        if needed_height != current_viewport_height {
            let (term_width, term_height) = terminal_size()?;
            // Clamp height to terminal size to avoid rendering outside bounds
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

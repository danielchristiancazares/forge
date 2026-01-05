mod app;
mod config;
mod context_infinity;
mod input;
mod markdown;
mod message;
mod provider;
mod theme;
mod ui;
mod ui_inline;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, TerminalOptions, Viewport};
use std::{env, io::{Stdout, stdout}};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::app::App;
use crate::config::ForgeConfig;
use crate::input::handle_events;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Full,
    Inline,
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
        if use_alternate_screen {
            if let Err(err) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
                let _ = disable_raw_mode();
                return Err(err.into());
            }
        }

        let backend = CrosstermBackend::new(out);
        let terminal = match mode {
            UiMode::Full => Terminal::new(backend),
            UiMode::Inline => Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Inline(ui_inline::INLINE_VIEWPORT_HEIGHT),
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

    let result = {
        let config = ForgeConfig::load();
        let ui_mode = UiMode::from_config(config.as_ref())
            .or_else(UiMode::from_env)
            .unwrap_or(UiMode::Full);
        let mut session = TerminalSession::new(ui_mode)?;
        let mut app = App::new()?;

        let run_result = match ui_mode {
            UiMode::Full => run_app_full(&mut session.terminal, &mut app).await,
            UiMode::Inline => run_app_inline(&mut session.terminal, &mut app).await,
        };
        // Save history before exit
        if let Err(e) = app.save_history() {
            eprintln!("Failed to save history: {e}");
        }

        run_result
    };

    if let Err(err) = result {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app_full<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    loop {
        app.tick();

        // Yield to allow spawned tasks (streaming, summarization) to make progress.
        // This is critical because crossterm's event::poll() is blocking and
        // doesn't yield to the tokio runtime.
        tokio::task::yield_now().await;

        app.process_stream_events();

        terminal.draw(|frame| ui::draw(frame, app))?;

        if handle_events(app).await? {
            return Ok(());
        }
    }
}

async fn run_app_inline<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    let mut output = ui_inline::InlineOutput::new();

    loop {
        app.tick();

        // Yield to allow spawned tasks (streaming, summarization) to make progress.
        // This is critical because crossterm's event::poll() is blocking and
        // doesn't yield to the tokio runtime.
        tokio::task::yield_now().await;

        app.process_stream_events();
        output.flush(terminal, app)?;

        terminal.draw(|frame| ui_inline::draw(frame, app))?;

        if handle_events(app).await? {
            return Ok(());
        }
    }
}

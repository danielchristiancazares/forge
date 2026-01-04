mod app;
mod context_infinity;
mod input;
mod message;
mod provider;
mod theme;
mod ui;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io::{Stdout, stdout};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::app::App;
use crate::input::handle_events;

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> Result<Self> {
        enable_raw_mode()?;

        let mut out = stdout();
        if let Err(err) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }

        let backend = CrosstermBackend::new(out);
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(err) => {
                let _ = disable_raw_mode();
                let mut out = stdout();
                let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
                return Err(err.into());
            }
        };

        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
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
        let mut session = TerminalSession::new()?;
        let mut app = App::new()?;

        let run_result = run_app(&mut session.terminal, &mut app).await;

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

async fn run_app<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
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

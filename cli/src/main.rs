//! Forge CLI - Binary entry point and terminal session management.
//!
//! # Architecture
//!
//! The CLI bridges [`forge_engine`] (application state) and [`forge_tui`] (rendering),
//! providing RAII-based terminal management with guaranteed cleanup.
//!
//! # Event Loop
//!
//! Uses a fixed 8ms (~120 FPS) render cadence:
//!
//! 1. Wait for frame tick
//! 2. Drain input queue (non-blocking via [`forge_tui::InputPump`])
//! 3. Advance application state (`app.tick()`)
//! 4. Process streaming events from LLM
//! 5. Handle transcript clear requests
//! 6. Render frame

mod assets;
mod crash_hardening;

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Stdout, Write, stdout},
    panic,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::signal::ctrl_c;
use tokio::time::{MissedTickBehavior, interval};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use forge_engine::{App, ForgeConfig};
use forge_tui::{InputPump, draw, handle_events};

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::try_new("warn").expect("warn filter is valid"));

    let (log_file, init_warnings) = open_forge_log_file();

    if let Some((log_path, file)) = log_file {
        tracing_subscriber::registry()
            .with(fmt::layer().with_ansi(false).with_writer(Mutex::new(file)))
            .with(env_filter)
            .init();

        tracing::info!(path = %log_path.display(), "Logging initialized");
        for warning in init_warnings {
            tracing::warn!("{warning}");
        }
        return;
    }

    // If we can't open a log file, prefer "no logs" over corrupting the TUI
    // by writing to stdout/stderr.
    tracing_subscriber::registry().with(env_filter).init();
}

fn open_forge_log_file() -> (Option<(PathBuf, File)>, Vec<String>) {
    let candidates = forge_log_file_candidates();
    let mut warnings = Vec::new();

    for candidate in candidates {
        if let Some(parent) = candidate.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            warnings.push(format!(
                "Failed to create log dir {}: {e}",
                parent.display()
            ));
            continue;
        }

        // Enforce secure permissions on the log directory (Unix)
        #[cfg(unix)]
        if let Some(parent) = candidate.parent() {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(parent) {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(parent, perms);
            }
        }

        // Reject symlinks at the log file path to prevent write redirection
        if let Ok(meta) = fs::symlink_metadata(&candidate)
            && meta.file_type().is_symlink()
        {
            warnings.push(format!(
                "Refusing symlink log path: {}",
                candidate.display()
            ));
            continue;
        }

        let file_result = open_log_file_secure(&candidate);
        match file_result {
            Ok(file) => return (Some((candidate, file)), warnings),
            Err(e) => {
                warnings.push(format!(
                    "Failed to open log file {}: {e}",
                    candidate.display()
                ));
            }
        }
    }

    (None, warnings)
}

/// Open a log file with secure permissions on Unix (0600).
fn open_log_file_secure(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new().create(true).append(true).open(path)
    }
}

fn forge_log_file_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Only use ~/.forge/logs/forge.log — no CWD fallback.
    // A CWD fallback is dangerous: untrusted repos can place symlinks at
    // .forge/logs/forge.log to hijack log writes to arbitrary files.
    if let Some(config_path) = ForgeConfig::path()
        && let Some(config_dir) = config_path.parent()
    {
        candidates.push(config_dir.join("logs").join("forge.log"));
    }

    candidates
}

/// RAII wrapper for terminal state with guaranteed cleanup on drop.
///
/// Manages the terminal lifecycle including:
/// - Raw mode (disables line buffering and echo)
/// - Bracketed paste (detects pasted text vs typed input)
/// - Alternate screen buffer
/// - Alternate scroll mode (maps scroll wheel to arrows without mouse capture)
///
/// On drop, all terminal state is restored to its original configuration,
/// ensuring the terminal remains usable even after panics or early returns.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> Result<Self> {
        enable_raw_mode()?;

        let mut out = stdout();
        if let Err(err) = execute!(out, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }

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

        let backend = CrosstermBackend::new(out);
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(err) => {
                let _ = disable_raw_mode();
                let mut out = stdout();
                // Disable alternate scroll mode: CSI ? 1007 l
                let _ = out.write_all(b"\x1b[?1007l");
                let _ = out.flush();
                let _ = execute!(
                    out,
                    Clear(ClearType::All),
                    LeaveAlternateScreen,
                    DisableBracketedPaste
                );
                return Err(err.into());
            }
        };

        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        // Disable alternate scroll mode: CSI ? 1007 l
        let _ = self.terminal.backend_mut().write_all(b"\x1b[?1007l");
        let _ = Write::flush(&mut *self.terminal.backend_mut());
        // Clear the alternate screen before leaving it. iTerm2 (and some other
        // macOS terminals) can copy the final alternate-screen frame into the
        // main scrollback buffer on LeaveAlternateScreen; blanking first
        // prevents TUI remnants from lingering in the history.
        let _ = execute!(
            self.terminal.backend_mut(),
            Clear(ClearType::All),
            LeaveAlternateScreen,
            DisableBracketedPaste
        );
        let _ = self.terminal.show_cursor();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    crash_hardening::apply()?;

    // Install a panic hook that restores the terminal before printing the
    // backtrace. Without this, a panic leaves the terminal in raw mode with
    // the alternate screen active, making the error invisible.
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut out = stdout();
        // Disable alternate scroll mode
        let _ = out.write_all(b"\x1b[?1007l");
        let _ = out.flush();
        let _ = execute!(
            out,
            Clear(ClearType::All),
            LeaveAlternateScreen,
            DisableBracketedPaste
        );
        original_hook(info);
    }));

    assets::init();

    let mut app = App::new(assets::system_prompts())?;

    let run_result = {
        let mut session = TerminalSession::new()?;
        run_app(&mut session.terminal, &mut app).await
        // TerminalSession drops here, restoring the terminal
    };

    // Cleanup is unconditional — run_app errors must not skip persistence.
    app.shutdown_lsp().await;

    if let Err(e) = app.save_history() {
        eprintln!("Failed to save history: {e}");
    }

    if let Err(e) = app.save_session() {
        eprintln!("Failed to save session: {e}");
    }

    run_result
}

const FRAME_DURATION: Duration = Duration::from_millis(8);

async fn run_app<B>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    let mut input = InputPump::new();
    let mut frames = interval(FRAME_DURATION);
    frames.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let signal_quit = Arc::new(AtomicBool::new(false));
    let sq = signal_quit.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        sq.store(true, Ordering::Release);
    });

    let result: Result<()> = loop {
        frames.tick().await;

        if signal_quit.load(Ordering::Acquire) {
            break Ok(());
        }

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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut hup = signal(SignalKind::hangup()).expect("SIGHUP handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        let _ = ctrl_c().await;
    }
}

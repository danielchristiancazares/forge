//! Input handling for Forge TUI.

use anyhow::{Result, anyhow};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tracing::debug;

use forge_engine::{App, InputMode};

const INPUT_POLL_TIMEOUT: Duration = Duration::from_millis(25); // shutdown responsiveness
const INPUT_CHANNEL_CAPACITY: usize = 1024; // bounded: no OOM
const MAX_EVENTS_PER_FRAME: usize = 64; // never starve rendering

/// Heuristics for detecting paste when the terminal doesn't emit `Event::Paste`.
///
/// On Windows, crossterm reads input via WinAPI records (not VT input sequences), so paste
/// arrives as a burst of key events. During a paste burst, bare `Enter` should insert a newline
/// instead of submitting the message.
const PASTE_INTER_KEY_THRESHOLD: Duration = Duration::from_millis(20);
const PASTE_IDLE_TIMEOUT: Duration = Duration::from_millis(75);
const PASTE_QUEUE_THRESHOLD: usize = 32;

enum InputMsg {
    Event(Event),
    Error(String),
}
#[derive(Debug)]
struct PasteDetector {
    last_key_time: Instant,
    active_until: Instant,
}

impl PasteDetector {
    fn new(now: Instant) -> Self {
        Self {
            last_key_time: now,
            active_until: now,
        }
    }

    fn reset(&mut self, now: Instant) {
        self.last_key_time = now;
        self.active_until = now;
    }

    fn update(&mut self, now: Instant, backlog: usize, event: &Event) -> bool {
        // Only key press + repeat events participate in detection.
        let is_key_event = matches!(
            event,
            Event::Key(KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            })
        );

        let was_active = now < self.active_until;
        let backlog_high = backlog >= PASTE_QUEUE_THRESHOLD;
        let rapid =
            is_key_event && now.duration_since(self.last_key_time) < PASTE_INTER_KEY_THRESHOLD;

        let active = was_active || backlog_high || rapid;

        if is_key_event {
            if active {
                // Keep paste mode alive across frame pacing and scheduling hiccups.
                self.active_until = now + PASTE_IDLE_TIMEOUT;
            }
            self.last_key_time = now;
        }

        active
    }
}

/// Dedicated blocking input reader. Rendering consumes events via `try_recv` only.
pub struct InputPump {
    rx: mpsc::Receiver<InputMsg>,
    stop: Arc<AtomicBool>,
    join: Option<tokio::task::JoinHandle<()>>,
    paste: PasteDetector,
}

impl InputPump {
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();

        let join = tokio::task::spawn_blocking(move || input_loop(stop2, tx));
        let now = Instant::now();
        Self {
            rx,
            stop,
            join: Some(join),
            paste: PasteDetector::new(now),
        }
    }

    /// Deterministic shutdown (use before tearing down terminal session / switching modes).
    pub async fn shutdown(&mut self) {
        // Close the receiver first to ensure the input thread unblocks if it is currently
        // backpressured on a send (e.g., during a large paste).
        self.rx.close();

        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Default for InputPump {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for InputPump {
    fn drop(&mut self) {
        // Best-effort stop if caller exits early; do not block in Drop.
        //
        // Close the receiver to ensure the input thread unblocks if it is currently waiting on
        // channel capacity (e.g., during a large paste).
        self.rx.close();
        self.stop.store(true, Ordering::Release);
    }
}

fn input_loop(stop: Arc<AtomicBool>, tx: mpsc::Sender<InputMsg>) {
    while !stop.load(Ordering::Acquire) {
        match event::poll(INPUT_POLL_TIMEOUT) {
            Ok(true) => match event::read() {
                Ok(ev) => {
                    // Bounded queue: apply backpressure instead of dropping events.
                    // This preserves correctness (e.g., multi-line pastes) while still
                    // preventing unbounded memory growth.
                    if tx.blocking_send(InputMsg::Event(ev)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(InputMsg::Error(e.to_string()));
                    break;
                }
            },
            Ok(false) => {}
            Err(e) => {
                let _ = tx.blocking_send(InputMsg::Error(e.to_string()));
                break;
            }
        }
    }
}

/// Drain queued input events without blocking rendering.
/// Returns true if the app should quit (same semantics as before).
pub fn handle_events(app: &mut App, input: &mut InputPump) -> Result<bool> {
    for _ in 0..MAX_EVENTS_PER_FRAME {
        match input.rx.try_recv() {
            Ok(InputMsg::Event(ev)) => {
                let now = Instant::now();
                let backlog = input.rx.len();

                let paste_active = if app.input_mode() == InputMode::Insert {
                    input.paste.update(now, backlog, &ev)
                } else {
                    input.paste.reset(now);
                    false
                };

                if paste_active {
                    debug!(
                        backlog,
                        "Input paste detection active (fallback heuristics)"
                    );
                }

                if apply_event(app, ev, paste_active) {
                    return Ok(true);
                }
            }
            Ok(InputMsg::Error(msg)) => return Err(anyhow!("input error: {msg}")),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Err(anyhow!("input pump disconnected"));
            }
        }
    }
    Ok(app.should_quit())
}

/// Apply a single input event to the app.
/// `paste_active` indicates the input stream looks like a paste burst (fallback when the
/// terminal doesn't emit `Event::Paste`).
fn apply_event(app: &mut App, event: Event, paste_active: bool) -> bool {
    match event {
        Event::Key(key) => {
            // Handle press + repeat events (ignore releases)
            if matches!(key.kind, KeyEventKind::Release) {
                return app.should_quit();
            }

            // Handle Ctrl+C globally (preserve old semantics: returns true without setting should_quit)
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                if app.is_loading() {
                    app.cancel_active_operation();
                    return app.should_quit();
                }
                return true;
            }

            // Handle Esc globally for cancellation during active operations
            // Only cancel when:
            // - In Normal mode (Insert/Command mode Esc should exit to Normal first)
            // - Files panel is not expanded (Esc should collapse it first)
            // - Not in approval/recovery UI (they have their own Esc handlers)
            if key.code == KeyCode::Esc
                && app.is_loading()
                && app.input_mode() == InputMode::Normal
                && !app.files_panel_expanded()
                && app.tool_approval_requests().is_none()
                && app.tool_recovery_calls().is_none()
            {
                app.cancel_active_operation();
                return app.should_quit();
            }

            match app.input_mode() {
                InputMode::Normal => handle_normal_mode(app, key),
                InputMode::Insert => handle_insert_mode(app, key, paste_active),
                InputMode::Command => handle_command_mode(app, key),
                InputMode::ModelSelect => handle_model_select_mode(app, key),
                InputMode::FileSelect => handle_file_select_mode(app, key),
            }
        }
        Event::Paste(text) => {
            // Preserve existing paste gating + insert-token flow exactly
            if app.tool_approval_requests().is_some() || app.tool_recovery_calls().is_some() {
                return app.should_quit();
            }
            if app.input_mode() == InputMode::Insert {
                let Some(token) = app.insert_token() else {
                    return app.should_quit();
                };
                // Normalize line endings: convert \r\n to \n and remove stray \r
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                app.insert_mode(token).enter_text(&normalized);
            }
        }
        _ => {}
    }
    app.should_quit()
}

fn handle_normal_mode(app: &mut App, key: KeyEvent) {
    if app.tool_approval_requests().is_some() {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => app.tool_approval_move_up(),
            KeyCode::Char('j') | KeyCode::Down => app.tool_approval_move_down(),
            KeyCode::Char(' ') => app.tool_approval_toggle(),
            KeyCode::Tab => app.tool_approval_toggle_details(),
            KeyCode::Char('a') => app.tool_approval_approve_all(),
            KeyCode::Char('d') | KeyCode::Esc => app.tool_approval_request_deny_all(),
            KeyCode::Enter => app.tool_approval_activate(),
            _ => {}
        }
        return;
    }

    if app.tool_recovery_calls().is_some() {
        match key.code {
            KeyCode::Char('r' | 'R') => app.tool_recovery_resume(),
            KeyCode::Char('d' | 'D') | KeyCode::Esc => app.tool_recovery_discard(),
            _ => {}
        }
        return;
    }

    match key.code {
        // Quit
        KeyCode::Char('q') => {
            app.request_quit();
        }
        // Enter insert mode
        KeyCode::Char('i') => {
            app.enter_insert_mode();
        }
        // Enter insert mode at end
        KeyCode::Char('a') => {
            app.enter_insert_mode_at_end();
        }
        // Toggle thinking visibility
        KeyCode::Char('o') => {
            app.toggle_thinking();
        }
        // Enter command mode
        KeyCode::Char(':' | '/') => {
            app.enter_command_mode();
        }
        // Scroll up
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up();
        }
        // Page up
        KeyCode::PageUp => {
            app.scroll_page_up();
        }
        // Page down
        KeyCode::PageDown => {
            app.scroll_page_down();
        }
        // Page up (Ctrl+U) - context-sensitive: scroll diff when expanded
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.files_panel_expanded() {
                app.files_panel_scroll_diff_up();
            } else {
                app.scroll_page_up();
            }
        }
        // Page down (Ctrl+D) - context-sensitive: scroll diff when expanded
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.files_panel_expanded() {
                app.files_panel_scroll_diff_down();
            } else {
                app.scroll_page_down();
            }
        }
        // Scroll down
        KeyCode::Char('j') | KeyCode::Down => {
            app.scroll_down();
        }
        // Go to top
        KeyCode::Char('g') => {
            app.scroll_to_top();
        }
        // Jump to bottom (End, G, or Right)
        KeyCode::End | KeyCode::Char('G') | KeyCode::Right => {
            app.scroll_to_bottom();
        }
        // Toggle screen mode (inline/fullscreen)
        KeyCode::Char('s') => {
            app.request_toggle_screen_mode();
        }
        // Toggle files panel
        KeyCode::Char('f') => {
            app.toggle_files_panel();
        }
        // Open model picker (blocked during active operations)
        KeyCode::Char('m') if !app.is_loading() => {
            app.enter_model_select_mode();
        }
        // Scroll up by 20% chunk
        KeyCode::Left => {
            app.scroll_up_chunk();
        }
        // Files panel: Tab cycles to next file
        KeyCode::Tab => {
            if app.files_panel_visible() {
                app.files_panel_next();
            }
        }
        // Files panel: Shift+Tab cycles to previous file
        KeyCode::BackTab => {
            if app.files_panel_visible() {
                app.files_panel_prev();
            }
        }
        // Files panel: Enter or Esc collapses diff
        KeyCode::Enter | KeyCode::Esc => {
            if app.files_panel_expanded() {
                app.files_panel_collapse();
            }
        }
        // Files panel: Backspace progressively dismisses (expanded → compact → closed)
        KeyCode::Backspace => {
            if app.files_panel_expanded() {
                app.files_panel_collapse();
            } else if app.files_panel_visible() {
                app.close_files_panel();
            }
        }
        _ => {}
    }
}

/// Handle insert mode input.
/// `paste_active` indicates the input stream looks like a paste burst. In this case, bare Enter
/// inserts a newline instead of submitting.
fn handle_insert_mode(app: &mut App, key: KeyEvent, paste_active: bool) {
    // Tool approval modal takes priority over insert mode
    if app.tool_approval_requests().is_some() {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => app.tool_approval_move_up(),
            KeyCode::Char('j') | KeyCode::Down => app.tool_approval_move_down(),
            KeyCode::Char(' ') => app.tool_approval_toggle(),
            KeyCode::Tab => app.tool_approval_toggle_details(),
            KeyCode::Char('a') => app.tool_approval_approve_all(),
            KeyCode::Char('d') | KeyCode::Esc => app.tool_approval_request_deny_all(),
            KeyCode::Enter => app.tool_approval_activate(),
            _ => {}
        }
        return;
    }

    // Tool recovery modal takes priority over insert mode
    if app.tool_recovery_calls().is_some() {
        match key.code {
            KeyCode::Char('r' | 'R') => app.tool_recovery_resume(),
            KeyCode::Char('d' | 'D') | KeyCode::Esc => app.tool_recovery_discard(),
            _ => {}
        }
        return;
    }

    // Handle newline insertion:
    // - Explicit: Ctrl+Enter, Shift+Enter, Ctrl+J
    // - Implicit: bare Enter during a paste burst (fallback when `Event::Paste` isn't emitted)
    let is_explicit_newline = matches!(
        (key.code, key.modifiers),
        (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SHIFT)
    ) || matches!(key, KeyEvent { code: KeyCode::Char('j'), modifiers: m, .. } if m.contains(KeyModifiers::CONTROL));

    let is_paste_newline = paste_active && key.code == KeyCode::Enter && key.modifiers.is_empty();

    if is_explicit_newline || is_paste_newline {
        let Some(token) = app.insert_token() else {
            return;
        };
        app.insert_mode(token).enter_newline();
        return;
    }

    match key.code {
        // Exit insert mode
        KeyCode::Esc => {
            app.enter_normal_mode();
        }
        // Submit message (only when not detected as paste)
        KeyCode::Enter => {
            let Some(token) = app.insert_token() else {
                return;
            };
            let queued = app.insert_mode(token).queue_message();
            if let Some(queued) = queued {
                app.start_streaming(queued);
            }
        }
        // Navigate prompt history (Up/Down)
        KeyCode::Up => {
            app.navigate_history_up();
        }
        KeyCode::Down => {
            app.navigate_history_down();
        }
        // Backspace: exit insert mode if empty, otherwise delete char
        KeyCode::Backspace => {
            if app.draft_text().is_empty() {
                app.enter_normal_mode();
            } else if let Some(token) = app.insert_token() {
                app.insert_mode(token).delete_char();
            }
        }
        // '@' triggers file select mode (check before acquiring insert token)
        KeyCode::Char('@') => {
            app.enter_file_select_mode();
        }
        _ => {
            let Some(token) = app.insert_token() else {
                return;
            };
            let mut insert = app.insert_mode(token);

            match key.code {
                // Delete character forward
                KeyCode::Delete => {
                    insert.delete_char_forward();
                }
                // Move cursor left
                KeyCode::Left => {
                    insert.move_cursor_left();
                }
                // Move cursor right
                KeyCode::Right => {
                    insert.move_cursor_right();
                }
                // Move to start
                KeyCode::Home => {
                    insert.reset_cursor();
                }
                // Move to end
                KeyCode::End => {
                    insert.move_cursor_end();
                }
                // Clear line
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert.clear_line();
                }
                // Delete word backwards
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert.delete_word_backwards();
                }
                // Insert character (ignore \r - it's handled via Enter or normalized in paste)
                KeyCode::Char(c) if c != '\r' => {
                    insert.enter_char(c);
                }
                _ => {}
            }
        }
    }
}

fn handle_command_mode(app: &mut App, key: KeyEvent) {
    // Tool approval modal takes priority over command mode
    if app.tool_approval_requests().is_some() {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => app.tool_approval_move_up(),
            KeyCode::Char('j') | KeyCode::Down => app.tool_approval_move_down(),
            KeyCode::Char(' ') => app.tool_approval_toggle(),
            KeyCode::Tab => app.tool_approval_toggle_details(),
            KeyCode::Char('a') => app.tool_approval_approve_all(),
            KeyCode::Char('d') | KeyCode::Esc => app.tool_approval_request_deny_all(),
            KeyCode::Enter => app.tool_approval_activate(),
            _ => {}
        }
        return;
    }

    // Tool recovery modal takes priority over command mode
    if app.tool_recovery_calls().is_some() {
        match key.code {
            KeyCode::Char('r' | 'R') => app.tool_recovery_resume(),
            KeyCode::Char('d' | 'D') | KeyCode::Esc => app.tool_recovery_discard(),
            _ => {}
        }
        return;
    }

    match key.code {
        // Exit command mode
        KeyCode::Esc => {
            app.enter_normal_mode();
        }
        // Execute command
        KeyCode::Enter => {
            let Some(token) = app.command_token() else {
                return;
            };
            let command_mode = app.command_mode(token);
            let Some(command) = command_mode.take_command() else {
                return;
            };

            app.process_command(command);
        }
        // Navigate command history (Up/Down)
        KeyCode::Up => {
            app.navigate_command_history_up();
        }
        KeyCode::Down => {
            app.navigate_command_history_down();
        }
        // Backspace: exit command mode if empty, otherwise delete char
        KeyCode::Backspace => {
            if app.command_text().is_some_and(str::is_empty) {
                app.enter_normal_mode();
            } else if let Some(token) = app.command_token() {
                app.command_mode(token).backspace();
            }
        }
        _ => {
            let Some(token) = app.command_token() else {
                return;
            };
            let mut command_mode = app.command_mode(token);

            match key.code {
                // Move cursor left
                KeyCode::Left => {
                    command_mode.move_cursor_left();
                }
                // Move cursor right
                KeyCode::Right => {
                    command_mode.move_cursor_right();
                }
                // Move to start
                KeyCode::Home => {
                    command_mode.reset_cursor();
                }
                // Move to end
                KeyCode::End => {
                    command_mode.move_cursor_end();
                }
                // Move to start (Ctrl+A)
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command_mode.reset_cursor();
                }
                // Move to end (Ctrl+E)
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command_mode.move_cursor_end();
                }
                // Delete word backwards
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command_mode.delete_word_backwards();
                }
                // Clear line
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command_mode.clear_line();
                }
                // Tab completion
                KeyCode::Tab => {
                    command_mode.tab_complete();
                }
                // Insert character (ignore \r)
                KeyCode::Char(c) if c != '\r' => {
                    command_mode.push_char(c);
                }
                _ => {}
            }
        }
    }
}

fn handle_model_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        // Cancel and return to normal mode
        KeyCode::Esc => {
            app.enter_normal_mode();
        }
        // Confirm selection
        KeyCode::Enter => {
            app.model_select_confirm();
        }
        // Move selection up
        KeyCode::Up | KeyCode::Char('k') => {
            app.model_select_move_up();
        }
        // Move selection down
        KeyCode::Down | KeyCode::Char('j') => {
            app.model_select_move_down();
        }
        // Direct selection with number keys
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let digit = c.to_digit(10).unwrap_or(0);
            if digit > 0 {
                let index = (digit - 1) as usize;
                app.model_select_set_index(index);
                app.model_select_confirm();
            }
        }
        _ => {}
    }
}

fn handle_file_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        // Cancel and return to insert mode
        KeyCode::Esc => {
            app.file_select_cancel();
        }
        // Confirm selection - insert file path into draft
        KeyCode::Enter => {
            app.file_select_confirm();
        }
        // Move selection up
        KeyCode::Up => {
            app.file_select_move_up();
        }
        // Move selection down
        KeyCode::Down => {
            app.file_select_move_down();
        }
        // Backspace - delete filter character or cancel if empty
        KeyCode::Backspace => {
            let filter = app.file_select_filter().unwrap_or("");
            if filter.is_empty() {
                app.file_select_cancel();
            } else {
                app.file_select_backspace();
            }
        }
        // Type character to filter
        KeyCode::Char(c) if c != '\r' => {
            app.file_select_push_char(c);
        }
        _ => {}
    }
}

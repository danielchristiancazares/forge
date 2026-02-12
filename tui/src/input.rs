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

/// Chars accumulated before checking the system clipboard to short-circuit a
/// detected paste burst. 16 is conservative: the main false-positive vector is
/// key-repeat (N identical chars matching a clipboard prefix). 16 identical
/// chars in a clipboard prefix is essentially impossible.
const CLIPBOARD_CHECK_THRESHOLD: usize = 16;

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

enum InputMsg {
    Event(Event),
    Error(String),
}

/// Lifecycle state of a paste operation detected via timing heuristics.
///
/// Owned by `InputPump` (policy), not by `PasteDetector` (mechanism).
/// All transitions are driven by `handle_events`.
enum PastePhase {
    /// No paste in progress.
    Idle,
    /// Paste heuristics triggered; chars are inserted normally while a prefix
    /// is accumulated for clipboard verification.
    Accumulating { received: String },
    /// Clipboard check failed or was skipped; heuristic-only behavior
    /// (Enter -> newline) continues until the burst ends.
    HeuristicOnly,
    /// Clipboard confirmed; remainder bulk-inserted. Remaining burst events
    /// are discarded until the burst ends.
    Draining,
}

/// Pure timing mechanism for detecting paste bursts.
///
/// Reports whether a burst is active (`bool`). Owns no phase state — mechanism
/// reports facts, policy (`handle_events`) makes decisions.
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

pub struct InputPump {
    rx: mpsc::Receiver<InputMsg>,
    stop: Arc<AtomicBool>,
    join: Option<tokio::task::JoinHandle<()>>,
    paste: PasteDetector,
    phase: PastePhase,
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
            phase: PastePhase::Idle,
        }
    }

    pub async fn shutdown(&mut self) {
        // Close the receiver first to ensure the input thread unblocks if it is currently
        // backpressured on a send (e.g., during a large paste).
        self.rx.close();

        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), join).await;
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

/// Extract the character that `handle_insert_mode` would insert for this key
/// event during a paste burst. Returns `None` for non-content keys (editing
/// actions, modifiers, etc.) and signals whether to abort accumulation.
enum AccumulateAction {
    /// A content char was received; track it and continue.
    Track(char),
    /// An editing key was received; abort accumulation.
    Abort,
    /// A non-content, non-editing event (Resize, modifier-only, etc.); skip.
    Skip,
}

fn classify_for_accumulation(ev: &Event) -> AccumulateAction {
    match ev {
        Event::Key(KeyEvent {
            code: KeyCode::Char(c),
            kind: KeyEventKind::Press | KeyEventKind::Repeat,
            ..
        }) if *c != '\r' => AccumulateAction::Track(*c),
        Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers,
            kind: KeyEventKind::Press | KeyEventKind::Repeat,
            ..
        }) if modifiers.is_empty() => AccumulateAction::Track('\n'),
        Event::Key(KeyEvent {
            code:
                KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Esc,
            kind: KeyEventKind::Press | KeyEventKind::Repeat,
            ..
        }) => AccumulateAction::Abort,
        _ => AccumulateAction::Skip,
    }
}

pub fn handle_events(app: &mut App, input: &mut InputPump) -> Result<bool> {
    let mut processed = 0;
    while processed < MAX_EVENTS_PER_FRAME {
        let ev = match input.rx.try_recv() {
            Ok(InputMsg::Event(ev)) => ev,
            Ok(InputMsg::Error(msg)) => return Err(anyhow!("input error: {msg}")),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Err(anyhow!("input pump disconnected"));
            }
        };

        let now = Instant::now();
        let backlog = input.rx.len();

        let timing_active = if app.input_mode() == InputMode::Insert {
            input.paste.update(now, backlog, &ev)
        } else {
            input.paste.reset(now);
            input.phase = PastePhase::Idle;
            false
        };

        // Drive phase transitions based on timing.
        if !timing_active && !matches!(input.phase, PastePhase::Idle) {
            input.phase = PastePhase::Idle;
        }
        if timing_active && matches!(input.phase, PastePhase::Idle) {
            input.phase = PastePhase::Accumulating {
                received: String::new(),
            };
        }

        // Draining: discard remaining burst events without counting against
        // the per-frame budget (discarding is free — no rendering cost).
        if matches!(input.phase, PastePhase::Draining) {
            continue;
        }

        // Accumulating: track chars that will be inserted by apply_event.
        if let PastePhase::Accumulating { ref mut received } = input.phase {
            match classify_for_accumulation(&ev) {
                AccumulateAction::Track(c) => received.push(c),
                AccumulateAction::Abort => input.phase = PastePhase::HeuristicOnly,
                AccumulateAction::Skip => {}
            }
        }

        let paste_active = matches!(
            input.phase,
            PastePhase::Accumulating { .. } | PastePhase::HeuristicOnly
        );

        if paste_active {
            debug!(
                backlog,
                "Input paste detection active (fallback heuristics)"
            );
        }

        if apply_event(app, ev, paste_active) {
            return Ok(true);
        }

        // Post-apply clipboard check: the current char is now inserted, so
        // `received` exactly reflects what the input buffer contains since the
        // burst started.
        if let PastePhase::Accumulating { ref received } = input.phase
            && received.len() >= CLIPBOARD_CHECK_THRESHOLD
        {
            let matched = arboard::Clipboard::new()
                .and_then(|mut cb| cb.get_text())
                .ok()
                .map(|text| normalize_line_endings(&text))
                .filter(|clip| clip.starts_with(received.as_str()));

            if let Some(clipboard_text) = matched {
                let remainder = &clipboard_text[received.len()..];
                if !remainder.is_empty()
                    && let Some(token) = app.insert_token()
                {
                    app.insert_mode(token).enter_text(remainder);
                }
                input.phase = PastePhase::Draining;
            } else {
                input.phase = PastePhase::HeuristicOnly;
            }
        }

        processed += 1;
    }
    Ok(app.should_quit())
}

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
                InputMode::Settings => handle_settings_mode(app, key),
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
                let normalized = normalize_line_endings(&text);
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
        KeyCode::Char('q') => {
            app.request_quit();
        }
        KeyCode::Char('i') => {
            app.enter_insert_mode();
        }
        KeyCode::Char('a') => {
            app.enter_insert_mode_at_end();
        }
        KeyCode::Char('o') => {
            app.toggle_thinking();
        }
        KeyCode::Char(':' | '/') => {
            app.enter_command_mode();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up();
        }
        KeyCode::PageUp => {
            app.scroll_page_up();
        }
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
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command_mode.clear_line();
                }
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

fn handle_settings_mode(app: &mut App, key: KeyEvent) {
    if !app.settings_is_root_surface() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                app.settings_close_or_exit();
            }
            _ => {}
        }
        return;
    }

    if app.settings_detail_view().is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.settings_close_or_exit();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_detail_move_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.settings_detail_move_down();
            }
            KeyCode::Enter | KeyCode::Char(' ' | 'e') => {
                app.settings_detail_toggle_selected();
            }
            KeyCode::Char('s') => {
                app.settings_save_edits();
            }
            KeyCode::Char('r') => {
                app.settings_revert_edits();
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.settings_close_or_exit();
        }
        KeyCode::Char('q') if !app.settings_filter_active() => {
            app.settings_close_or_exit();
        }
        KeyCode::Enter => {
            app.settings_activate();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if !app.settings_filter_active() {
                app.settings_move_up();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.settings_filter_active() {
                app.settings_move_down();
            }
        }
        KeyCode::Char('/') => {
            app.settings_start_filter();
        }
        KeyCode::Backspace => {
            if app.settings_filter_active() {
                app.settings_filter_backspace();
            }
        }
        KeyCode::Char(c) if c != '\r' && app.settings_filter_active() => {
            app.settings_filter_push_char(c);
        }
        _ => {}
    }
}

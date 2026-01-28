//! Input handling for Forge TUI.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Duration;

use forge_engine::{App, InputMode};

/// Handle terminal events
/// Returns true if the app should quit
pub async fn handle_events(app: &mut App) -> Result<bool> {
    // Poll for events without blocking the async runtime.
    let event = tokio::task::spawn_blocking(|| -> Result<Option<Event>> {
        if event::poll(Duration::from_millis(100))? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    })
    .await??;

    if let Some(event) = event {
        match event {
            Event::Key(key) => {
                // Handle press + repeat events (ignore releases)
                if matches!(key.kind, KeyEventKind::Release) {
                    return Ok(app.should_quit());
                }

                // Handle Ctrl+C globally
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    if app.is_loading() {
                        app.cancel_active_operation();
                        return Ok(app.should_quit());
                    }
                    return Ok(true);
                }

                match app.input_mode() {
                    InputMode::Normal => handle_normal_mode(app, key),
                    InputMode::Insert => handle_insert_mode(app, key),
                    InputMode::Command => handle_command_mode(app, key),
                    InputMode::ModelSelect => handle_model_select_mode(app, key),
                }
            }
            Event::Paste(text) => {
                if app.tool_approval_requests().is_some() || app.tool_recovery_calls().is_some() {
                    return Ok(app.should_quit());
                }
                if app.input_mode() == InputMode::Insert {
                    let Some(token) = app.insert_token() else {
                        return Ok(app.should_quit());
                    };
                    // Normalize line endings: convert \r\n to \n and remove stray \r
                    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                    app.insert_mode(token).enter_text(&normalized);
                }
            }
            _ => {}
        }
    }

    Ok(app.should_quit())
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
        // Enter insert mode with new line
        KeyCode::Char('o') => {
            app.enter_insert_mode_with_clear();
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
        // Open model picker
        KeyCode::Char('m') => {
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
        _ => {}
    }
}

fn handle_insert_mode(app: &mut App, key: KeyEvent) {
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

    // Handle newline insertion (Ctrl+Enter, Shift+Enter, Ctrl+J)
    let is_newline = matches!(
        (key.code, key.modifiers),
        (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SHIFT)
    ) || matches!(key, KeyEvent { code: KeyCode::Char('j'), modifiers: m, .. } if m.contains(KeyModifiers::CONTROL));

    if is_newline {
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
        // Submit message
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

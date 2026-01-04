use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Duration;

use crate::app::{App, InputMode};

/// Handle terminal events
/// Returns true if the app should quit
pub async fn handle_events(app: &mut App) -> Result<bool> {
    // Poll for events with a timeout
    if event::poll(Duration::from_millis(100))?
        && let Event::Key(key) = event::read()?
    {
        // Only handle key press events (not release) - important for Windows
        if key.kind != KeyEventKind::Press {
            return Ok(app.should_quit());
        }

        // Handle Ctrl+C globally
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        match app.input_mode() {
            InputMode::Normal => handle_normal_mode(app, key),
            InputMode::Insert => handle_insert_mode(app, key),
            InputMode::Command => handle_command_mode(app, key),
        }
    }

    Ok(app.should_quit())
}

fn handle_normal_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        // Quit
        KeyCode::Char('q') => {
            app.request_quit();
        }
        // Enter insert mode
        KeyCode::Char('i') => {
            app.enter_insert_mode();
            app.clear_status();
        }
        // Enter insert mode at end
        KeyCode::Char('a') => {
            app.enter_insert_mode_at_end();
            app.clear_status();
        }
        // Enter insert mode with new line
        KeyCode::Char('o') => {
            app.enter_insert_mode_with_clear();
            app.clear_status();
        }
        // Enter command mode
        KeyCode::Char(':') => {
            app.enter_command_mode();
        }
        // Scroll up
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up();
        }
        // Scroll down
        KeyCode::Char('j') => {
            app.scroll_down();
        }
        // Jump to bottom
        KeyCode::Down => {
            app.scroll_to_bottom();
        }
        // Go to top
        KeyCode::Char('g') => {
            app.scroll_to_top();
        }
        // Go to bottom
        KeyCode::Char('G') => {
            app.scroll_to_bottom();
        }
        _ => {}
    }
}

fn handle_insert_mode(app: &mut App, key: KeyEvent) {
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
        _ => {
            let Some(token) = app.insert_token() else {
                return;
            };
            let mut insert = app.insert_mode(token);

            match key.code {
                // Delete character
                KeyCode::Backspace => {
                    insert.delete_char();
                }
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
                // Insert character
                KeyCode::Char(c) => {
                    insert.enter_char(c);
                }
                _ => {}
            }
        }
    }
}

fn handle_command_mode(app: &mut App, key: KeyEvent) {
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
        _ => {
            let Some(token) = app.command_token() else {
                return;
            };
            let mut command_mode = app.command_mode(token);

            match key.code {
                // Delete character
                KeyCode::Backspace => {
                    command_mode.backspace();
                }
                // Insert character
                KeyCode::Char(c) => {
                    command_mode.push_char(c);
                }
                _ => {}
            }
        }
    }
}

//! Input mode wrappers for type-safe mode-specific operations.
//!
//! This module provides proof-token types and mode wrappers that ensure
//! operations are only performed when the app is in the correct mode.

use super::ui::{DraftInput, InputState};
use super::{ApiConfig, ApiKey, App, Message, NonEmptyString, OperationState, Provider};

/// Proof that a user message was validated and queued for sending.
#[derive(Debug)]
pub struct QueuedUserMessage {
    pub(crate) config: ApiConfig,
}

/// Proof that a command line was entered in Command mode.
#[derive(Debug)]
pub struct EnteredCommand {
    pub(crate) raw: String,
}

/// Proof token for Insert mode operations.
#[derive(Debug)]
pub struct InsertToken(());

/// Proof token for Command mode operations.
#[derive(Debug)]
pub struct CommandToken(());

/// Mode wrapper for safe insert operations.
pub struct InsertMode<'a> {
    pub(crate) app: &'a mut App,
}

/// Mode wrapper for safe command operations.
pub struct CommandMode<'a> {
    pub(crate) app: &'a mut App,
}

// ============================================================================
// Token factory methods (called from App)
// ============================================================================

impl App {
    /// Get proof token if currently in Insert mode.
    pub fn insert_token(&self) -> Option<InsertToken> {
        matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
    }

    /// Get proof token if currently in Command mode.
    pub fn command_token(&self) -> Option<CommandToken> {
        matches!(&self.input, InputState::Command { .. }).then_some(CommandToken(()))
    }

    /// Get insert mode wrapper (requires proof token).
    pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
        InsertMode { app: self }
    }

    /// Get command mode wrapper (requires proof token).
    pub fn command_mode(&mut self, _token: CommandToken) -> CommandMode<'_> {
        CommandMode { app: self }
    }
}

// ============================================================================
// InsertMode operations
// ============================================================================

impl InsertMode<'_> {
    fn draft_mut(&mut self) -> &mut DraftInput {
        self.app.input.draft_mut()
    }

    pub fn move_cursor_left(&mut self) {
        self.draft_mut().move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        self.draft_mut().move_cursor_right();
    }

    pub fn enter_char(&mut self, new_char: char) {
        self.draft_mut().enter_char(new_char);
    }

    pub fn enter_newline(&mut self) {
        self.draft_mut().enter_newline();
    }

    pub fn enter_text(&mut self, text: &str) {
        self.draft_mut().enter_text(text);
    }

    pub fn delete_char(&mut self) {
        self.draft_mut().delete_char();
    }

    pub fn delete_char_forward(&mut self) {
        self.draft_mut().delete_char_forward();
    }

    pub fn delete_word_backwards(&mut self) {
        self.draft_mut().delete_word_backwards();
    }

    pub fn reset_cursor(&mut self) {
        self.draft_mut().reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        self.draft_mut().move_cursor_end();
    }

    pub fn clear_line(&mut self) {
        self.draft_mut().clear();
    }

    /// Queue the current draft as a user message.
    ///
    /// Returns a token proving that a non-empty user message was queued, and that it is valid to
    /// begin a new stream.
    #[must_use]
    pub fn queue_message(self) -> Option<QueuedUserMessage> {
        match &self.app.state {
            OperationState::Streaming(_) => {
                self.app.set_status_warning("Already streaming a response");
                return None;
            }
            OperationState::ToolLoop(_) => {
                self.app
                    .set_status_warning("Busy: tool execution in progress");
                return None;
            }
            OperationState::ToolRecovery(_) => {
                self.app.set_status_warning("Busy: tool recovery pending");
                return None;
            }
            OperationState::Summarizing(_)
            | OperationState::SummarizingWithQueued(_)
            | OperationState::SummarizationRetry(_)
            | OperationState::SummarizationRetryWithQueued(_) => {
                self.app
                    .set_status_warning("Busy: summarization in progress");
                return None;
            }
            OperationState::Idle => {}
        }

        if self.app.draft_text().trim().is_empty() {
            if !self.app.empty_send_warning_shown {
                self.app.set_status_warning("Type a message to send.");
                self.app.empty_send_warning_shown = true;
            }
            return None;
        }

        let api_key = if let Some(key) = self.app.current_api_key().cloned() {
            key
        } else {
            self.app.set_status_warning(format!(
                "No API key configured. Set {} environment variable.",
                self.app.provider().env_var()
            ));
            return None;
        };

        let raw_content = self.app.input.draft_mut().take_text();
        let content = if let Ok(content) = NonEmptyString::new(raw_content.clone()) {
            content
        } else {
            self.app.set_status_warning("Cannot send empty message");
            return None;
        };

        // Track user message in context manager (also adds to display)
        let msg_id = self.app.push_history_message(Message::user(content));
        self.app.autosave_history(); // Persist user message immediately for crash durability

        // Store pending message for potential rollback if stream fails with no content
        self.app.pending_user_message = Some((msg_id, raw_content));

        self.app.scroll_to_bottom();
        self.app.tool_iterations = 0;

        let api_key = match self.app.model.provider() {
            Provider::Claude => ApiKey::Claude(api_key),
            Provider::OpenAI => ApiKey::OpenAI(api_key),
            Provider::Gemini => ApiKey::Gemini(api_key),
        };

        let config = match ApiConfig::new(api_key, self.app.model.clone()) {
            Ok(config) => config.with_openai_options(self.app.openai_options),
            Err(e) => {
                self.app
                    .set_status_error(format!("Cannot queue request: {e}"));
                return None;
            }
        };

        Some(QueuedUserMessage { config })
    }
}

// ============================================================================
// CommandMode operations
// ============================================================================

impl CommandMode<'_> {
    fn command_mut(&mut self) -> Option<&mut DraftInput> {
        self.app.input.command_mut()
    }

    pub fn move_cursor_left(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_right();
    }

    pub fn reset_cursor(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_end();
    }

    pub fn push_char(&mut self, c: char) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.enter_char(c);
    }

    pub fn backspace(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.delete_char();
    }

    pub fn delete_word_backwards(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.delete_word_backwards();
    }

    pub fn clear_line(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.clear();
    }

    #[must_use]
    pub fn take_command(self) -> Option<EnteredCommand> {
        let input = std::mem::take(&mut self.app.input);
        let InputState::Command { draft, mut command } = input else {
            self.app.input = input;
            return None;
        };

        self.app.input = InputState::Normal(draft);
        Some(EnteredCommand {
            raw: command.take_text(),
        })
    }
}

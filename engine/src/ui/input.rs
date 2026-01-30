//! Input mode and draft state for the editor.

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// Input mode for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
    FileSelect,
}

/// Draft input buffer with cursor tracking.
///
/// Handles text editing with proper Unicode grapheme cluster support.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DraftInput {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

impl DraftInput {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn take_text(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn move_cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.cursor.saturating_add(1);
        self.cursor = self.clamp_cursor(cursor_moved_right);
    }

    pub fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.text.insert(index, new_char);
        self.move_cursor_right();
    }

    pub fn enter_newline(&mut self) {
        self.enter_char('\n');
    }

    pub fn enter_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let index = self.byte_index();
        self.text.insert_str(index, text);
        let inserted = text.graphemes(true).count();
        self.cursor = self.clamp_cursor(self.cursor.saturating_add(inserted));
    }

    pub fn delete_char(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let start = self.byte_index_at(self.cursor - 1);
        let end = self.byte_index_at(self.cursor);
        self.text.replace_range(start..end, "");
        self.move_cursor_left();
    }

    pub fn delete_char_forward(&mut self) {
        let grapheme_count = self.grapheme_count();
        if self.cursor >= grapheme_count {
            return;
        }

        let start = self.byte_index_at(self.cursor);
        let end = self.byte_index_at(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor = self.grapheme_count();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Set the draft text and move cursor to end.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.grapheme_count();
    }

    pub fn delete_word_backwards(&mut self) {
        while self.cursor > 0 {
            let idx = self.cursor - 1;
            if self.grapheme_is_whitespace(idx) {
                self.delete_char();
            } else {
                break;
            }
        }

        while self.cursor > 0 {
            let idx = self.cursor - 1;
            if self.grapheme_is_whitespace(idx) {
                break;
            }
            self.delete_char();
        }
    }

    #[must_use]
    pub fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_is_whitespace(&self, index: usize) -> bool {
        self.text
            .graphemes(true)
            .nth(index)
            .is_some_and(|grapheme| grapheme.chars().all(char::is_whitespace))
    }

    #[must_use]
    pub fn byte_index(&self) -> usize {
        self.byte_index_at(self.cursor)
    }

    fn byte_index_at(&self, grapheme_index: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(grapheme_index)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        let max = self.grapheme_count();
        new_cursor_pos.min(max)
    }
}

/// Internal input state machine.
///
/// Tracks the current input mode along with mode-specific state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command {
        draft: DraftInput,
        command: DraftInput,
    },
    ModelSelect {
        draft: DraftInput,
        selected: usize,
    },
    FileSelect {
        draft: DraftInput,
        filter: DraftInput,
        selected: usize,
    },
}

impl Default for InputState {
    fn default() -> Self {
        Self::Normal(DraftInput::default())
    }
}

impl InputState {
    pub fn mode(&self) -> InputMode {
        match self {
            InputState::Normal(_) => InputMode::Normal,
            InputState::Insert(_) => InputMode::Insert,
            InputState::Command { .. } => InputMode::Command,
            InputState::ModelSelect { .. } => InputMode::ModelSelect,
            InputState::FileSelect { .. } => InputMode::FileSelect,
        }
    }

    pub fn draft(&self) -> &DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => draft,
        }
    }

    pub fn draft_mut(&mut self) -> &mut DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => draft,
        }
    }

    pub fn command(&self) -> Option<&str> {
        match self {
            InputState::Command { command, .. } => Some(command.text()),
            _ => None,
        }
    }

    pub fn command_cursor(&self) -> Option<usize> {
        match self {
            InputState::Command { command, .. } => Some(command.cursor()),
            _ => None,
        }
    }

    pub fn command_cursor_byte_index(&self) -> Option<usize> {
        match self {
            InputState::Command { command, .. } => Some(command.byte_index()),
            _ => None,
        }
    }

    pub fn command_mut(&mut self) -> Option<&mut DraftInput> {
        match self {
            InputState::Command { command, .. } => Some(command),
            _ => None,
        }
    }

    pub fn model_select_index(&self) -> Option<usize> {
        match self {
            InputState::ModelSelect { selected, .. } => Some(*selected),
            _ => None,
        }
    }

    pub fn into_normal(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => InputState::Normal(draft),
        }
    }

    pub fn into_insert(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => InputState::Insert(draft),
        }
    }

    pub fn into_command(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => InputState::Command {
                draft,
                command: DraftInput::default(),
            },
        }
    }

    pub fn into_model_select(self, selected: usize) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => InputState::ModelSelect { draft, selected },
        }
    }

    pub fn into_file_select(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. } => InputState::FileSelect {
                draft,
                filter: DraftInput::default(),
                selected: 0,
            },
        }
    }

    pub fn file_select_filter(&self) -> Option<&str> {
        match self {
            InputState::FileSelect { filter, .. } => Some(filter.text()),
            _ => None,
        }
    }

    pub fn file_select_filter_mut(&mut self) -> Option<&mut DraftInput> {
        match self {
            InputState::FileSelect { filter, .. } => Some(filter),
            _ => None,
        }
    }

    pub fn file_select_index(&self) -> Option<usize> {
        match self {
            InputState::FileSelect { selected, .. } => Some(*selected),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_input_set_text_moves_cursor_to_end() {
        let mut draft = DraftInput {
            text: "initial".to_string(),
            cursor: 0,
        };

        draft.set_text("new text".to_string());

        assert_eq!(draft.text(), "new text");
        assert_eq!(draft.cursor(), 8);
    }

    #[test]
    fn draft_input_set_text_handles_unicode() {
        let mut draft = DraftInput::default();

        draft.set_text("hello 🦀 world".to_string());

        assert_eq!(draft.text(), "hello 🦀 world");
        assert_eq!(draft.cursor(), 13); // 13 graphemes
    }

    #[test]
    fn input_state_mode_transitions() {
        let state = InputState::default();
        assert_eq!(state.mode(), InputMode::Normal);

        let state = state.into_insert();
        assert_eq!(state.mode(), InputMode::Insert);

        let state = state.into_command();
        assert_eq!(state.mode(), InputMode::Command);

        let state = state.into_model_select(0);
        assert_eq!(state.mode(), InputMode::ModelSelect);

        let state = state.into_normal();
        assert_eq!(state.mode(), InputMode::Normal);
    }

    #[test]
    fn model_select_initializes_with_correct_index() {
        let state = InputState::default();
        let state = state.into_model_select(2);

        if let InputState::ModelSelect { selected, .. } = state {
            assert_eq!(selected, 2);
        } else {
            panic!("Expected ModelSelect state");
        }
    }

    // ========================================================================
    // DraftInput cursor movement tests
    // ========================================================================

    #[test]
    fn draft_move_cursor_left_from_start() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 0,
        };
        draft.move_cursor_left();
        assert_eq!(draft.cursor(), 0); // Should stay at 0
    }

    #[test]
    fn draft_move_cursor_left_from_middle() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 3,
        };
        draft.move_cursor_left();
        assert_eq!(draft.cursor(), 2);
    }

    #[test]
    fn draft_move_cursor_right_at_end() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.move_cursor_right();
        assert_eq!(draft.cursor(), 5); // Should stay at end
    }

    #[test]
    fn draft_move_cursor_right_from_middle() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 2,
        };
        draft.move_cursor_right();
        assert_eq!(draft.cursor(), 3);
    }

    #[test]
    fn draft_reset_cursor() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.reset_cursor();
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_move_cursor_end() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 0,
        };
        draft.move_cursor_end();
        assert_eq!(draft.cursor(), 5);
    }

    // ========================================================================
    // DraftInput character insertion tests
    // ========================================================================

    #[test]
    fn draft_enter_char_at_start() {
        let mut draft = DraftInput {
            text: "world".to_string(),
            cursor: 0,
        };
        draft.enter_char('H');
        assert_eq!(draft.text(), "Hworld");
        assert_eq!(draft.cursor(), 1);
    }

    #[test]
    fn draft_enter_char_at_end() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.enter_char('!');
        assert_eq!(draft.text(), "hello!");
        assert_eq!(draft.cursor(), 6);
    }

    #[test]
    fn draft_enter_char_in_middle() {
        let mut draft = DraftInput {
            text: "hllo".to_string(),
            cursor: 1,
        };
        draft.enter_char('e');
        assert_eq!(draft.text(), "hello");
        assert_eq!(draft.cursor(), 2);
    }

    #[test]
    fn draft_enter_newline() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.enter_newline();
        assert_eq!(draft.text(), "hello\n");
        assert_eq!(draft.cursor(), 6);
    }

    #[test]
    fn draft_enter_text_empty() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.enter_text("");
        assert_eq!(draft.text(), "hello");
        assert_eq!(draft.cursor(), 5);
    }

    #[test]
    fn draft_enter_text_at_cursor() {
        let mut draft = DraftInput {
            text: "hd".to_string(),
            cursor: 1,
        };
        draft.enter_text("ello worl");
        assert_eq!(draft.text(), "hello world");
        assert_eq!(draft.cursor(), 10);
    }

    // ========================================================================
    // DraftInput deletion tests
    // ========================================================================

    #[test]
    fn draft_delete_char_at_start() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 0,
        };
        draft.delete_char();
        assert_eq!(draft.text(), "hello"); // Nothing deleted
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_delete_char_at_end() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.delete_char();
        assert_eq!(draft.text(), "hell");
        assert_eq!(draft.cursor(), 4);
    }

    #[test]
    fn draft_delete_char_in_middle() {
        let mut draft = DraftInput {
            text: "hxello".to_string(),
            cursor: 2,
        };
        draft.delete_char();
        assert_eq!(draft.text(), "hello");
        assert_eq!(draft.cursor(), 1);
    }

    #[test]
    fn draft_delete_char_forward_at_end() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.delete_char_forward();
        assert_eq!(draft.text(), "hello"); // Nothing deleted
        assert_eq!(draft.cursor(), 5);
    }

    #[test]
    fn draft_delete_char_forward_at_start() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 0,
        };
        draft.delete_char_forward();
        assert_eq!(draft.text(), "ello");
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_delete_char_forward_in_middle() {
        let mut draft = DraftInput {
            text: "hxello".to_string(),
            cursor: 1,
        };
        draft.delete_char_forward();
        assert_eq!(draft.text(), "hello");
        assert_eq!(draft.cursor(), 1);
    }

    #[test]
    fn draft_clear() {
        let mut draft = DraftInput {
            text: "hello world".to_string(),
            cursor: 5,
        };
        draft.clear();
        assert_eq!(draft.text(), "");
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_take_text() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 3,
        };
        let text = draft.take_text();
        assert_eq!(text, "hello");
        assert_eq!(draft.text(), "");
        assert_eq!(draft.cursor(), 0);
    }

    // ========================================================================
    // DraftInput word deletion tests
    // ========================================================================

    #[test]
    fn draft_delete_word_backwards_single_word() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 5,
        };
        draft.delete_word_backwards();
        assert_eq!(draft.text(), "");
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_delete_word_backwards_with_trailing_spaces() {
        let mut draft = DraftInput {
            text: "hello   ".to_string(),
            cursor: 8,
        };
        draft.delete_word_backwards();
        assert_eq!(draft.text(), "");
        assert_eq!(draft.cursor(), 0);
    }

    #[test]
    fn draft_delete_word_backwards_multiple_words() {
        let mut draft = DraftInput {
            text: "hello world".to_string(),
            cursor: 11,
        };
        draft.delete_word_backwards();
        assert_eq!(draft.text(), "hello ");
        assert_eq!(draft.cursor(), 6);
    }

    #[test]
    fn draft_delete_word_backwards_at_start() {
        let mut draft = DraftInput {
            text: "hello".to_string(),
            cursor: 0,
        };
        draft.delete_word_backwards();
        assert_eq!(draft.text(), "hello");
        assert_eq!(draft.cursor(), 0);
    }

    // ========================================================================
    // DraftInput Unicode tests
    // ========================================================================

    #[test]
    fn draft_unicode_grapheme_count() {
        let draft = DraftInput {
            text: "a🦀b".to_string(),
            cursor: 0,
        };
        assert_eq!(draft.grapheme_count(), 3);
    }

    #[test]
    fn draft_unicode_cursor_movement() {
        let mut draft = DraftInput {
            text: "a🦀b".to_string(),
            cursor: 1,
        };
        draft.move_cursor_right();
        assert_eq!(draft.cursor(), 2); // Past the emoji
        draft.move_cursor_right();
        assert_eq!(draft.cursor(), 3); // At end
    }

    #[test]
    fn draft_unicode_delete() {
        let mut draft = DraftInput {
            text: "a🦀b".to_string(),
            cursor: 2,
        };
        draft.delete_char();
        assert_eq!(draft.text(), "ab");
        assert_eq!(draft.cursor(), 1);
    }

    #[test]
    fn draft_unicode_insert() {
        let mut draft = DraftInput {
            text: "ab".to_string(),
            cursor: 1,
        };
        draft.enter_char('🦀');
        assert_eq!(draft.text(), "a🦀b");
        assert_eq!(draft.cursor(), 2);
    }

    #[test]
    fn draft_byte_index_unicode() {
        let draft = DraftInput {
            text: "a🦀b".to_string(), // 🦀 is 4 bytes
            cursor: 2,
        };
        // 'a' is 1 byte, '🦀' is 4 bytes
        assert_eq!(draft.byte_index(), 5); // 1 + 4 = 5
    }

    // ========================================================================
    // InputState tests
    // ========================================================================

    #[test]
    fn input_state_draft_accessor() {
        let state = InputState::Normal(DraftInput {
            text: "test".to_string(),
            cursor: 2,
        });
        assert_eq!(state.draft().text(), "test");
        assert_eq!(state.draft().cursor(), 2);
    }

    #[test]
    fn input_state_draft_mut_accessor() {
        let mut state = InputState::Insert(DraftInput {
            text: "test".to_string(),
            cursor: 0,
        });
        state.draft_mut().enter_char('X');
        assert_eq!(state.draft().text(), "Xtest");
    }

    #[test]
    fn input_state_command_accessor_in_command_mode() {
        let state = InputState::Command {
            draft: DraftInput::default(),
            command: DraftInput {
                text: "quit".to_string(),
                cursor: 4,
            },
        };
        assert_eq!(state.command(), Some("quit"));
        assert_eq!(state.command_cursor(), Some(4));
    }

    #[test]
    fn input_state_command_accessor_not_in_command_mode() {
        let state = InputState::Normal(DraftInput::default());
        assert_eq!(state.command(), None);
        assert_eq!(state.command_cursor(), None);
    }

    #[test]
    fn input_state_command_cursor_byte_index() {
        let state = InputState::Command {
            draft: DraftInput::default(),
            command: DraftInput {
                text: "a🦀b".to_string(),
                cursor: 2,
            },
        };
        assert_eq!(state.command_cursor_byte_index(), Some(5));
    }

    #[test]
    fn input_state_model_select_index() {
        let state = InputState::ModelSelect {
            draft: DraftInput::default(),
            selected: 3,
        };
        assert_eq!(state.model_select_index(), Some(3));
    }

    #[test]
    fn input_state_model_select_index_not_in_mode() {
        let state = InputState::Normal(DraftInput::default());
        assert_eq!(state.model_select_index(), None);
    }

    #[test]
    fn input_state_transitions_preserve_draft() {
        let state = InputState::Normal(DraftInput {
            text: "preserved".to_string(),
            cursor: 5,
        });

        let state = state.into_insert();
        assert_eq!(state.draft().text(), "preserved");

        let state = state.into_command();
        assert_eq!(state.draft().text(), "preserved");

        let state = state.into_model_select(0);
        assert_eq!(state.draft().text(), "preserved");

        let state = state.into_normal();
        assert_eq!(state.draft().text(), "preserved");
    }

    #[test]
    fn input_state_command_mut_accessor() {
        let mut state = InputState::Command {
            draft: DraftInput::default(),
            command: DraftInput {
                text: "cmd".to_string(),
                cursor: 3,
            },
        };
        if let Some(cmd) = state.command_mut() {
            cmd.enter_char('!');
        }
        assert_eq!(state.command(), Some("cmd!"));
    }

    #[test]
    fn input_state_command_mut_not_in_command_mode() {
        let mut state = InputState::Normal(DraftInput::default());
        assert!(state.command_mut().is_none());
    }

    // ========================================================================
    // InputMode tests
    // ========================================================================

    #[test]
    fn input_mode_default() {
        let mode = InputMode::default();
        assert_eq!(mode, InputMode::Normal);
    }

    #[test]
    fn input_mode_clone() {
        let mode = InputMode::Insert;
        let cloned = mode;
        assert_eq!(cloned, InputMode::Insert);
    }
}

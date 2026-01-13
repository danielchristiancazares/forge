//! Input mode and draft state for the editor.

use unicode_segmentation::UnicodeSegmentation;

/// Input mode for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
}

/// Draft input buffer with cursor tracking.
///
/// Handles text editing with proper Unicode grapheme cluster support.
#[derive(Debug, Default)]
pub struct DraftInput {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

impl DraftInput {
    pub fn text(&self) -> &str {
        &self.text
    }

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

    pub fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_is_whitespace(&self, index: usize) -> bool {
        self.text
            .graphemes(true)
            .nth(index)
            .is_some_and(|grapheme| grapheme.chars().all(char::is_whitespace))
    }

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
#[derive(Debug)]
pub enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: String },
    ModelSelect { draft: DraftInput, selected: usize },
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
        }
    }

    pub fn draft(&self) -> &DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. } => draft,
        }
    }

    pub fn draft_mut(&mut self) -> &mut DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. } => draft,
        }
    }

    pub fn command(&self) -> Option<&str> {
        match self {
            InputState::Command { command, .. } => Some(command),
            _ => None,
        }
    }

    pub fn command_mut(&mut self) -> Option<&mut String> {
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
            | InputState::ModelSelect { draft, .. } => InputState::Normal(draft),
        }
    }

    pub fn into_insert(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. } => InputState::Insert(draft),
        }
    }

    pub fn into_command(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. } => InputState::Command {
                draft,
                command: String::new(),
            },
        }
    }

    pub fn into_model_select(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. } => InputState::ModelSelect { draft, selected: 0 },
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

        let state = state.into_model_select();
        assert_eq!(state.mode(), InputMode::ModelSelect);

        let state = state.into_normal();
        assert_eq!(state.mode(), InputMode::Normal);
    }
}

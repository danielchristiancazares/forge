//! Input mode and draft state for the editor.

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
    FileSelect,
    Settings,
}

/// Handles text editing with proper Unicode grapheme cluster support.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DraftInput {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

/// Top-level settings categories for the read-only settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingsCategory {
    Providers,
    Models,
    Context,
    Tools,
    Keybindings,
    Profiles,
    History,
    Appearance,
}

impl SettingsCategory {
    pub const ALL: [Self; 8] = [
        Self::Providers,
        Self::Models,
        Self::Context,
        Self::Tools,
        Self::Keybindings,
        Self::Profiles,
        Self::History,
        Self::Appearance,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Context => "Context",
            Self::Tools => "Tools",
            Self::Keybindings => "Keybindings",
            Self::Profiles => "Profiles",
            Self::History => "History",
            Self::Appearance => "Appearance",
        }
    }

    #[must_use]
    pub const fn detail_title(self) -> &'static str {
        match self {
            Self::Providers => "Settings > Providers",
            Self::Models => "Settings > Models",
            Self::Context => "Settings > Context",
            Self::Tools => "Settings > Tools",
            Self::Keybindings => "Settings > Keybindings",
            Self::Profiles => "Settings > Profiles",
            Self::History => "Settings > History",
            Self::Appearance => "Settings > Appearance",
        }
    }

    #[must_use]
    pub fn filtered(filter: &str) -> Vec<Self> {
        let needle = filter.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Self::ALL.to_vec();
        }

        Self::ALL
            .into_iter()
            .filter(|category| {
                let label = category.label().to_ascii_lowercase();
                label.contains(&needle)
                    || category
                        .keywords()
                        .iter()
                        .any(|keyword| keyword.contains(&needle))
            })
            .collect()
    }

    const fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Providers => &["provider", "api", "keys", "auth"],
            Self::Models => &["model", "chat", "code"],
            Self::Context => &["context", "memory", "distill"],
            Self::Tools => &["tools", "sandbox", "permissions"],
            Self::Keybindings => &["keys", "bindings", "vim"],
            Self::Profiles => &["profile", "preset", "switch"],
            Self::History => &["history", "retention", "privacy"],
            Self::Appearance => &["theme", "appearance", "display"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SettingsSurface {
    #[default]
    Root,
    Runtime,
    Resolve,
    Validate,
}

impl SettingsSurface {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Root => "Settings",
            Self::Runtime => "Runtime",
            Self::Resolve => "Resolution Cascade",
            Self::Validate => "Validation",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsModalState {
    pub surface: SettingsSurface,
    pub selected: usize,
    pub filter: DraftInput,
    pub filter_active: bool,
    pub detail_view: Option<SettingsCategory>,
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
    Settings {
        draft: DraftInput,
        modal: SettingsModalState,
    },
}

pub enum CommandDraftRef<'a> {
    Active(&'a DraftInput),
    Inactive,
}

pub enum CommandDraftMut<'a> {
    Active(&'a mut DraftInput),
    Inactive,
}

pub enum InsertDraftMut<'a> {
    Active(&'a mut DraftInput),
    Inactive,
}

pub enum ModelSelectRef {
    Active { selected: usize },
    Inactive,
}

pub enum ModelSelectMut<'a> {
    Active { selected: &'a mut usize },
    Inactive,
}

pub enum FileSelectRef<'a> {
    Active {
        filter: &'a DraftInput,
        selected: usize,
    },
    Inactive,
}

pub enum FileSelectMut<'a> {
    Active {
        filter: &'a mut DraftInput,
        selected: &'a mut usize,
    },
    Inactive,
}

pub enum SettingsModalRef<'a> {
    Active(&'a SettingsModalState),
    Inactive,
}

pub enum SettingsModalMut<'a> {
    Active(&'a mut SettingsModalState),
    Inactive,
}

impl Default for InputState {
    fn default() -> Self {
        Self::Normal(DraftInput::default())
    }
}

impl InputState {
    #[must_use]
    pub fn mode(&self) -> InputMode {
        match self {
            InputState::Normal(_) => InputMode::Normal,
            InputState::Insert(_) => InputMode::Insert,
            InputState::Command { .. } => InputMode::Command,
            InputState::ModelSelect { .. } => InputMode::ModelSelect,
            InputState::FileSelect { .. } => InputMode::FileSelect,
            InputState::Settings { .. } => InputMode::Settings,
        }
    }

    #[must_use]
    pub fn draft(&self) -> &DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => draft,
        }
    }

    pub fn draft_mut(&mut self) -> &mut DraftInput {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => draft,
        }
    }

    #[must_use]
    pub fn command_ref(&self) -> CommandDraftRef<'_> {
        match self {
            InputState::Command { command, .. } => CommandDraftRef::Active(command),
            _ => CommandDraftRef::Inactive,
        }
    }

    pub fn command_mut_access(&mut self) -> CommandDraftMut<'_> {
        match self {
            InputState::Command { command, .. } => CommandDraftMut::Active(command),
            _ => CommandDraftMut::Inactive,
        }
    }

    pub fn insert_mut_access(&mut self) -> InsertDraftMut<'_> {
        match self {
            InputState::Insert(draft) => InsertDraftMut::Active(draft),
            _ => InsertDraftMut::Inactive,
        }
    }

    #[must_use]
    pub fn model_select_ref(&self) -> ModelSelectRef {
        match self {
            InputState::ModelSelect { selected, .. } => ModelSelectRef::Active {
                selected: *selected,
            },
            _ => ModelSelectRef::Inactive,
        }
    }

    pub fn model_select_mut_access(&mut self) -> ModelSelectMut<'_> {
        match self {
            InputState::ModelSelect { selected, .. } => ModelSelectMut::Active { selected },
            _ => ModelSelectMut::Inactive,
        }
    }

    #[must_use]
    pub fn into_normal(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::Normal(draft),
        }
    }

    #[must_use]
    pub fn into_insert(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::Insert(draft),
        }
    }

    #[must_use]
    pub fn into_command(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::Command {
                draft,
                command: DraftInput::default(),
            },
        }
    }

    #[must_use]
    pub fn into_model_select(self, selected: usize) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::ModelSelect { draft, selected },
        }
    }

    #[must_use]
    pub fn into_file_select(self) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::FileSelect {
                draft,
                filter: DraftInput::default(),
                selected: 0,
            },
        }
    }

    #[must_use]
    pub fn into_settings(self) -> InputState {
        self.into_settings_surface(SettingsSurface::Root)
    }

    #[must_use]
    pub fn into_settings_surface(self, surface: SettingsSurface) -> InputState {
        match self {
            InputState::Normal(draft)
            | InputState::Insert(draft)
            | InputState::Command { draft, .. }
            | InputState::ModelSelect { draft, .. }
            | InputState::FileSelect { draft, .. }
            | InputState::Settings { draft, .. } => InputState::Settings {
                draft,
                modal: SettingsModalState {
                    surface,
                    ..SettingsModalState::default()
                },
            },
        }
    }

    #[must_use]
    pub fn file_select_ref(&self) -> FileSelectRef<'_> {
        match self {
            InputState::FileSelect {
                filter, selected, ..
            } => FileSelectRef::Active {
                filter,
                selected: *selected,
            },
            _ => FileSelectRef::Inactive,
        }
    }

    pub fn file_select_mut_access(&mut self) -> FileSelectMut<'_> {
        match self {
            InputState::FileSelect {
                filter, selected, ..
            } => FileSelectMut::Active { filter, selected },
            _ => FileSelectMut::Inactive,
        }
    }

    #[must_use]
    pub fn settings_modal_ref(&self) -> SettingsModalRef<'_> {
        match self {
            InputState::Settings { modal, .. } => SettingsModalRef::Active(modal),
            _ => SettingsModalRef::Inactive,
        }
    }

    pub fn settings_modal_mut_access(&mut self) -> SettingsModalMut<'_> {
        match self {
            InputState::Settings { modal, .. } => SettingsModalMut::Active(modal),
            _ => SettingsModalMut::Inactive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandDraftMut, CommandDraftRef, DraftInput, FileSelectMut, FileSelectRef, InputMode,
        InputState, InsertDraftMut, ModelSelectMut, ModelSelectRef, SettingsCategory,
        SettingsSurface,
    };

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

        let state = state.into_settings();
        assert_eq!(state.mode(), InputMode::Settings);

        let state = state.into_normal();
        assert_eq!(state.mode(), InputMode::Normal);
    }

    #[test]
    fn settings_category_filter_matches_keywords() {
        let matches = SettingsCategory::filtered("perm");
        assert_eq!(matches, vec![SettingsCategory::Tools]);
    }

    #[test]
    fn into_settings_surface_sets_requested_surface() {
        let state = InputState::default().into_settings_surface(SettingsSurface::Resolve);
        if let InputState::Settings { modal, .. } = state {
            assert_eq!(modal.surface, SettingsSurface::Resolve);
            assert!(!modal.filter_active);
            assert_eq!(modal.detail_view, None);
        } else {
            panic!("Expected Settings state");
        }
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
        match state.command_ref() {
            CommandDraftRef::Active(command) => {
                assert_eq!(command.text(), "quit");
                assert_eq!(command.cursor(), 4);
            }
            CommandDraftRef::Inactive => panic!("expected command state"),
        }
    }

    #[test]
    fn input_state_command_accessor_not_in_command_mode() {
        let state = InputState::Normal(DraftInput::default());
        assert!(matches!(state.command_ref(), CommandDraftRef::Inactive));
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
        match state.command_ref() {
            CommandDraftRef::Active(command) => assert_eq!(command.byte_index(), 5),
            CommandDraftRef::Inactive => panic!("expected command state"),
        }
    }

    #[test]
    fn input_state_model_select_index() {
        let state = InputState::ModelSelect {
            draft: DraftInput::default(),
            selected: 3,
        };
        assert!(matches!(
            state.model_select_ref(),
            ModelSelectRef::Active { selected: 3 }
        ));
    }

    #[test]
    fn input_state_model_select_index_not_in_mode() {
        let state = InputState::Normal(DraftInput::default());
        assert!(matches!(state.model_select_ref(), ModelSelectRef::Inactive));
    }

    #[test]
    fn input_state_model_select_mut_access() {
        let mut state = InputState::ModelSelect {
            draft: DraftInput::default(),
            selected: 1,
        };
        if let ModelSelectMut::Active { selected } = state.model_select_mut_access() {
            *selected = 2;
        }
        assert!(matches!(
            state.model_select_ref(),
            ModelSelectRef::Active { selected: 2 }
        ));
    }

    #[test]
    fn input_state_file_select_ref_access() {
        let state = InputState::FileSelect {
            draft: DraftInput::default(),
            filter: DraftInput {
                text: "src".to_string(),
                cursor: 3,
            },
            selected: 4,
        };
        match state.file_select_ref() {
            FileSelectRef::Active { filter, selected } => {
                assert_eq!(filter.text(), "src");
                assert_eq!(selected, 4);
            }
            FileSelectRef::Inactive => panic!("expected file-select state"),
        }
    }

    #[test]
    fn input_state_file_select_mut_access() {
        let mut state = InputState::FileSelect {
            draft: DraftInput::default(),
            filter: DraftInput {
                text: "src".to_string(),
                cursor: 3,
            },
            selected: 1,
        };
        if let FileSelectMut::Active { filter, selected } = state.file_select_mut_access() {
            filter.enter_char('!');
            *selected = 2;
        }
        match state.file_select_ref() {
            FileSelectRef::Active { filter, selected } => {
                assert_eq!(filter.text(), "src!");
                assert_eq!(selected, 2);
            }
            FileSelectRef::Inactive => panic!("expected file-select state"),
        }
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
        if let CommandDraftMut::Active(cmd) = state.command_mut_access() {
            cmd.enter_char('!');
        }
        match state.command_ref() {
            CommandDraftRef::Active(command) => assert_eq!(command.text(), "cmd!"),
            CommandDraftRef::Inactive => panic!("expected command state"),
        }
    }

    #[test]
    fn input_state_command_mut_not_in_command_mode() {
        let mut state = InputState::Normal(DraftInput::default());
        assert!(matches!(
            state.command_mut_access(),
            CommandDraftMut::Inactive
        ));
    }

    #[test]
    fn input_state_insert_mut_access() {
        let mut state = InputState::Insert(DraftInput::default());
        if let InsertDraftMut::Active(draft) = state.insert_mut_access() {
            draft.enter_char('x');
        }
        assert_eq!(state.draft().text(), "x");
    }

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

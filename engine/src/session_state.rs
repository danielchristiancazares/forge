//! Session state for draft persistence.
//!
//! This module defines the state that is persisted between sessions to preserve
//! the user's draft input and input history. Unlike conversation history (which
//! is managed by `forge_context`), session state is lightweight and focused on
//! the ephemeral UI state that would otherwise be lost on quit/crash.

use serde::{Deserialize, Serialize};

use crate::ui::{InputHistory, InputState};

/// Session state container for persistence.
///
/// This is persisted to `session.json` in the data directory alongside
/// `history.json` and the journal databases. It captures:
///
/// - The current input state (draft text, cursor position, mode)
/// - Previously submitted prompts and commands for history navigation
///
/// # Version Compatibility
///
/// The `version` field enables forward compatibility. If a newer version of
/// Forge writes session state with a higher version number, older versions
/// will ignore the persisted state and start fresh.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Draft input state (text + cursor + mode).
    ///
    /// This captures the user's in-progress composition, including which mode
    /// they were in (Normal, Insert, Command, ModelSelect).
    pub input: Option<InputState>,

    /// Input history for prompt and command recall.
    ///
    /// Stores previously submitted prompts and executed commands, enabling
    /// Up/Down navigation in Insert and Command modes.
    pub history: InputHistory,

    /// Schema version for forward compatibility.
    ///
    /// Increment this when making breaking changes to the schema.
    pub version: u32,
}

impl SessionState {
    /// Current schema version.
    pub const CURRENT_VERSION: u32 = 1;

    /// Filename for the session state file.
    pub const FILENAME: &'static str = "session.json";

    /// Create a new session state with current version.
    pub fn new(input: InputState, history: InputHistory) -> Self {
        Self {
            input: Some(input),
            history,
            version: Self::CURRENT_VERSION,
        }
    }

    /// Check if this session state is compatible with the current version.
    pub fn is_compatible(&self) -> bool {
        self.version == Self::CURRENT_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::DraftInput;

    #[test]
    fn session_state_default_version() {
        let state = SessionState::default();
        assert_eq!(state.version, 0); // Default is 0, not CURRENT_VERSION
    }

    #[test]
    fn session_state_new_has_current_version() {
        let state = SessionState::new(InputState::default(), InputHistory::default());
        assert_eq!(state.version, SessionState::CURRENT_VERSION);
    }

    #[test]
    fn session_state_compatibility() {
        let state = SessionState::new(InputState::default(), InputHistory::default());
        assert!(state.is_compatible());

        let old_state = SessionState {
            version: 0,
            ..Default::default()
        };
        assert!(!old_state.is_compatible());
    }

    #[test]
    fn session_state_serialization_roundtrip() {
        let mut history = InputHistory::default();
        history.push_prompt("test prompt".to_owned());
        history.push_command("quit".to_owned());

        let mut draft = DraftInput::default();
        draft.set_text("in progress".to_owned());
        let input = InputState::Insert(draft);

        let state = SessionState::new(input, history);

        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();

        assert!(restored.is_compatible());
        assert!(restored.input.is_some());
    }
}

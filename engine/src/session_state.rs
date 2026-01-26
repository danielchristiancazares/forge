//! Session state for draft persistence.
//!
//! This module defines the state that is persisted between sessions to preserve
//! the user's draft input and input history. Unlike conversation history (which
//! is managed by `forge_context`), session state is lightweight and focused on
//! the ephemeral UI state that would otherwise be lost on quit/crash.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::{InputHistory, InputState};

/// Tracks files created and modified during a session.
///
/// This is aggregated across all tool loop turns and persisted with session state
/// so the user can see what files were affected during their conversation.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionChangeLog {
    /// Files created during the session.
    pub created: BTreeSet<PathBuf>,
    /// Files modified (but not created) during the session.
    pub modified: BTreeSet<PathBuf>,
}

impl SessionChangeLog {
    /// Merge a turn's changes into the session-wide log.
    ///
    /// If a file was created in a previous turn and modified in a later turn,
    /// it stays in `created`. If a file was modified and later created (rare),
    /// it moves to `created`.
    pub fn merge_turn(&mut self, created: &BTreeSet<PathBuf>, modified: &BTreeSet<PathBuf>) {
        for path in created {
            self.modified.remove(path);
            self.created.insert(path.clone());
        }
        for path in modified {
            if !self.created.contains(path) {
                self.modified.insert(path.clone());
            }
        }
    }

    /// Returns true if no files have been created or modified.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty()
    }
}

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

    /// Files created and modified during this session.
    ///
    /// Tracks which files have been affected by tool operations, allowing
    /// the user to see what has changed.
    #[serde(default)]
    pub modified_files: SessionChangeLog,

    /// Schema version for forward compatibility.
    ///
    /// Increment this when making breaking changes to the schema.
    pub version: u32,
}

impl SessionState {
    /// Current schema version.
    pub const CURRENT_VERSION: u32 = 2;

    /// Filename for the session state file.
    pub const FILENAME: &'static str = "session.json";

    /// Create a new session state with current version.
    pub fn new(input: InputState, history: InputHistory, modified_files: SessionChangeLog) -> Self {
        Self {
            input: Some(input),
            history,
            modified_files,
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
        let state = SessionState::new(
            InputState::default(),
            InputHistory::default(),
            SessionChangeLog::default(),
        );
        assert_eq!(state.version, SessionState::CURRENT_VERSION);
    }

    #[test]
    fn session_state_compatibility() {
        let state = SessionState::new(
            InputState::default(),
            InputHistory::default(),
            SessionChangeLog::default(),
        );
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

        let state = SessionState::new(input, history, SessionChangeLog::default());

        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();

        assert!(restored.is_compatible());
        assert!(restored.input.is_some());
    }

    #[test]
    fn session_change_log_merge() {
        use std::path::PathBuf;

        let mut log = SessionChangeLog::default();
        assert!(log.is_empty());

        let mut created = BTreeSet::new();
        created.insert(PathBuf::from("new_file.txt"));

        let mut modified = BTreeSet::new();
        modified.insert(PathBuf::from("existing.txt"));

        log.merge_turn(&created, &modified);
        assert!(!log.is_empty());
        assert!(log.created.contains(&PathBuf::from("new_file.txt")));
        assert!(log.modified.contains(&PathBuf::from("existing.txt")));

        // Modifying a created file keeps it in created
        let mut modified2 = BTreeSet::new();
        modified2.insert(PathBuf::from("new_file.txt"));
        log.merge_turn(&BTreeSet::new(), &modified2);
        assert!(log.created.contains(&PathBuf::from("new_file.txt")));
        assert!(!log.modified.contains(&PathBuf::from("new_file.txt")));
    }
}

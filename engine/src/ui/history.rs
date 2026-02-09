//! Input history for prompt and command recall.
//!
//! Provides ring buffers for previously submitted prompts (Insert mode) and
//! executed commands (Command mode), with Up/Down navigation support.

use serde::{Deserialize, Serialize};

/// Maximum number of prompts to keep in history.
const MAX_PROMPT_HISTORY: usize = 100;

/// Maximum number of commands to keep in history.
const MAX_COMMAND_HISTORY: usize = 50;

/// Input history for prompt and command recall.
///
/// Stores previously submitted user prompts and executed slash commands,
/// enabling navigation with Up/Down keys. The history is persisted across
/// sessions via `SessionState`.
///
/// # Navigation Behavior
///
/// When the user presses Up:
/// 1. If not navigating, stash the current draft and show the most recent entry
/// 2. If already navigating, show the next older entry
///
/// When the user presses Down:
/// 1. If at the newest entry, restore the stashed draft
/// 2. Otherwise, show the next newer entry
///
/// Navigation is reset after submitting a prompt or command.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InputHistory {
    /// Previously submitted user prompts (Insert mode).
    prompts: Vec<String>,
    /// Previously executed slash commands (Command mode).
    commands: Vec<String>,

    /// Current navigation index for prompts (None = editing new).
    #[serde(skip)]
    prompt_index: Option<usize>,
    /// Current navigation index for commands.
    #[serde(skip)]
    command_index: Option<usize>,

    /// Stashed draft when navigating prompts.
    #[serde(skip)]
    prompt_stash: Option<String>,
    /// Stashed draft when navigating commands.
    #[serde(skip)]
    command_stash: Option<String>,
}

impl InputHistory {
    /// Add a prompt to history.
    ///
    /// Prompts are stored most-recent-last. Duplicates of the most recent
    /// entry are ignored. The buffer is capped at `MAX_PROMPT_HISTORY`.
    pub fn push_prompt(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.prompts.last().is_some_and(|last| last == &text) {
            return;
        }
        self.prompts.push(text);
        if self.prompts.len() > MAX_PROMPT_HISTORY {
            self.prompts.remove(0);
        }
    }

    /// Add a command to history.
    ///
    /// Commands are stored most-recent-last. Duplicates of the most recent
    /// entry are ignored. The buffer is capped at `MAX_COMMAND_HISTORY`.
    pub fn push_command(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.commands.last().is_some_and(|last| last == &text) {
            return;
        }
        self.commands.push(text);
        if self.commands.len() > MAX_COMMAND_HISTORY {
            self.commands.remove(0);
        }
    }

    /// Navigate to the previous (older) prompt.
    ///
    /// On first call, stashes `current` and returns the most recent prompt.
    /// On subsequent calls, returns the next older prompt.
    /// Returns `None` if history is empty or already at the oldest entry.
    pub fn navigate_prompt_up(&mut self, current: &str) -> Option<&str> {
        if self.prompts.is_empty() {
            return None;
        }

        match self.prompt_index {
            None => {
                self.prompt_stash = Some(current.to_owned());
                self.prompt_index = Some(self.prompts.len() - 1);
                self.prompts.last().map(String::as_str)
            }
            Some(0) => None,
            Some(idx) => {
                let new_idx = idx - 1;
                self.prompt_index = Some(new_idx);
                self.prompts.get(new_idx).map(String::as_str)
            }
        }
    }

    /// Navigate to the next (newer) prompt.
    ///
    /// Returns `None` if not currently navigating.
    pub fn navigate_prompt_down(&mut self) -> Option<&str> {
        match self.prompt_index {
            None => None,
            Some(idx) if idx + 1 >= self.prompts.len() => {
                self.prompt_index = None;
                self.prompt_stash.as_deref()
            }
            Some(idx) => {
                let new_idx = idx + 1;
                self.prompt_index = Some(new_idx);
                self.prompts.get(new_idx).map(String::as_str)
            }
        }
    }

    /// Navigate to the previous (older) command.
    ///
    /// On first call, stashes `current` and returns the most recent command.
    /// On subsequent calls, returns the next older command.
    /// Returns `None` if history is empty or already at the oldest entry.
    pub fn navigate_command_up(&mut self, current: &str) -> Option<&str> {
        if self.commands.is_empty() {
            return None;
        }

        match self.command_index {
            None => {
                self.command_stash = Some(current.to_owned());
                self.command_index = Some(self.commands.len() - 1);
                self.commands.last().map(String::as_str)
            }
            Some(0) => None,
            Some(idx) => {
                let new_idx = idx - 1;
                self.command_index = Some(new_idx);
                self.commands.get(new_idx).map(String::as_str)
            }
        }
    }

    /// Navigate to the next (newer) command.
    ///
    /// Returns `None` if not currently navigating.
    pub fn navigate_command_down(&mut self) -> Option<&str> {
        match self.command_index {
            None => None,
            Some(idx) if idx + 1 >= self.commands.len() => {
                self.command_index = None;
                self.command_stash.as_deref()
            }
            Some(idx) => {
                let new_idx = idx + 1;
                self.command_index = Some(new_idx);
                self.commands.get(new_idx).map(String::as_str)
            }
        }
    }

    /// Reset navigation state.
    ///
    /// Call this after submitting a prompt or command to clear the
    /// navigation indices and stashed drafts.
    pub fn reset_navigation(&mut self) {
        self.prompt_index = None;
        self.command_index = None;
        self.prompt_stash = None;
        self.command_stash = None;
    }

    #[cfg(test)]
    #[must_use]
    pub fn prompt_count(&self) -> usize {
        self.prompts.len()
    }

    #[cfg(test)]
    #[must_use]
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_prompt_adds_to_history() {
        let mut history = InputHistory::default();
        history.push_prompt("hello".to_owned());
        assert_eq!(history.prompt_count(), 1);
    }

    #[test]
    fn push_prompt_ignores_empty() {
        let mut history = InputHistory::default();
        history.push_prompt(String::new());
        assert_eq!(history.prompt_count(), 0);
    }

    #[test]
    fn push_prompt_ignores_duplicate_of_last() {
        let mut history = InputHistory::default();
        history.push_prompt("hello".to_owned());
        history.push_prompt("hello".to_owned());
        assert_eq!(history.prompt_count(), 1);
    }

    #[test]
    fn push_prompt_allows_duplicate_not_at_end() {
        let mut history = InputHistory::default();
        history.push_prompt("hello".to_owned());
        history.push_prompt("world".to_owned());
        history.push_prompt("hello".to_owned());
        assert_eq!(history.prompt_count(), 3);
    }

    #[test]
    fn push_prompt_caps_at_max() {
        let mut history = InputHistory::default();
        for i in 0..MAX_PROMPT_HISTORY + 10 {
            history.push_prompt(format!("prompt {i}"));
        }
        assert_eq!(history.prompt_count(), MAX_PROMPT_HISTORY);
    }

    #[test]
    fn push_command_adds_to_history() {
        let mut history = InputHistory::default();
        history.push_command("quit".to_owned());
        assert_eq!(history.command_count(), 1);
    }

    #[test]
    fn push_command_caps_at_max() {
        let mut history = InputHistory::default();
        for i in 0..MAX_COMMAND_HISTORY + 10 {
            history.push_command(format!("cmd {i}"));
        }
        assert_eq!(history.command_count(), MAX_COMMAND_HISTORY);
    }

    #[test]
    fn navigate_prompt_up_empty_history() {
        let mut history = InputHistory::default();
        assert!(history.navigate_prompt_up("current").is_none());
    }

    #[test]
    fn navigate_prompt_up_first_call() {
        let mut history = InputHistory::default();
        history.push_prompt("first".to_owned());
        history.push_prompt("second".to_owned());

        let result = history.navigate_prompt_up("current draft");
        assert_eq!(result, Some("second"));
        assert_eq!(history.prompt_stash, Some("current draft".to_owned()));
    }

    #[test]
    fn navigate_prompt_up_through_history() {
        let mut history = InputHistory::default();
        history.push_prompt("first".to_owned());
        history.push_prompt("second".to_owned());
        history.push_prompt("third".to_owned());

        assert_eq!(history.navigate_prompt_up("draft"), Some("third"));
        assert_eq!(history.navigate_prompt_up(""), Some("second"));
        assert_eq!(history.navigate_prompt_up(""), Some("first"));
        assert!(history.navigate_prompt_up("").is_none()); // At oldest
    }

    #[test]
    fn navigate_prompt_down_not_navigating() {
        let mut history = InputHistory::default();
        history.push_prompt("first".to_owned());
        assert!(history.navigate_prompt_down().is_none());
    }

    #[test]
    fn navigate_prompt_down_restores_stash() {
        let mut history = InputHistory::default();
        history.push_prompt("first".to_owned());

        history.navigate_prompt_up("my draft");
        let result = history.navigate_prompt_down();
        assert_eq!(result, Some("my draft"));
    }

    #[test]
    fn navigate_prompt_down_through_history() {
        let mut history = InputHistory::default();
        history.push_prompt("first".to_owned());
        history.push_prompt("second".to_owned());
        history.push_prompt("third".to_owned());

        // Go to oldest
        history.navigate_prompt_up("draft");
        history.navigate_prompt_up("");
        history.navigate_prompt_up("");

        // Navigate back
        assert_eq!(history.navigate_prompt_down(), Some("second"));
        assert_eq!(history.navigate_prompt_down(), Some("third"));
        assert_eq!(history.navigate_prompt_down(), Some("draft"));
    }

    #[test]
    fn navigate_command_up_empty_history() {
        let mut history = InputHistory::default();
        assert!(history.navigate_command_up("current").is_none());
    }

    #[test]
    fn navigate_command_up_first_call() {
        let mut history = InputHistory::default();
        history.push_command("quit".to_owned());
        history.push_command("clear".to_owned());

        let result = history.navigate_command_up("current cmd");
        assert_eq!(result, Some("clear"));
        assert_eq!(history.command_stash, Some("current cmd".to_owned()));
    }

    #[test]
    fn navigate_command_down_restores_stash() {
        let mut history = InputHistory::default();
        history.push_command("quit".to_owned());

        history.navigate_command_up("my command");
        let result = history.navigate_command_down();
        assert_eq!(result, Some("my command"));
    }

    #[test]
    fn reset_navigation_clears_state() {
        let mut history = InputHistory::default();
        history.push_prompt("prompt".to_owned());
        history.push_command("cmd".to_owned());

        history.navigate_prompt_up("draft");
        history.navigate_command_up("cmd draft");

        history.reset_navigation();

        assert!(history.prompt_index.is_none());
        assert!(history.command_index.is_none());
        assert!(history.prompt_stash.is_none());
        assert!(history.command_stash.is_none());
    }

    #[test]
    fn serialization_preserves_history() {
        let mut history = InputHistory::default();
        history.push_prompt("prompt1".to_owned());
        history.push_prompt("prompt2".to_owned());
        history.push_command("cmd1".to_owned());

        // Simulate navigation state (should be skipped in serialization)
        history.navigate_prompt_up("draft");

        let json = serde_json::to_string(&history).unwrap();
        let restored: InputHistory = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.prompt_count(), 2);
        assert_eq!(restored.command_count(), 1);
        // Navigation state should not be preserved
        assert!(restored.prompt_index.is_none());
        assert!(restored.prompt_stash.is_none());
    }
}

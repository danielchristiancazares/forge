//! Input history for prompt and command recall.
//!
//! Provides ring buffers for previously submitted prompts (Insert mode) and
//! executed commands (Command mode), with Up/Down navigation support.

use std::mem::take;

use serde::{Deserialize, Serialize};

use crate::NonEmptyString;

const MAX_PROMPT_HISTORY: usize = 100;

const MAX_COMMAND_HISTORY: usize = 50;

/// Navigation state for a single history lane (prompts or commands).
///
/// `Idle` means the user is editing a fresh draft.
/// `Active` means the user is browsing history entries.
#[derive(Debug, Default, Clone)]
enum NavState {
    #[default]
    Idle,
    Active {
        index: usize,
        stash: String,
    },
}

/// Result of a navigation attempt — either we moved to an entry, or we're
/// already at the boundary (top of history, or not navigating).
pub enum NavOutcome {
    Moved(String),
    AtBoundary,
}

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
    prompts: Vec<String>,
    commands: Vec<String>,

    #[serde(skip)]
    prompt_nav: NavState,
    #[serde(skip)]
    command_nav: NavState,
}

impl InputHistory {
    /// Add a prompt to history.
    ///
    /// Prompts are stored most-recent-last. Duplicates of the most recent
    /// entry are ignored. The buffer is capped at `MAX_PROMPT_HISTORY`.
    pub fn push_prompt(&mut self, text: NonEmptyString) {
        if self
            .prompts
            .last()
            .is_some_and(|last| last == text.as_str())
        {
            return;
        }
        self.prompts.push(text.into_inner());
        if self.prompts.len() > MAX_PROMPT_HISTORY {
            self.prompts.remove(0);
        }
    }

    /// Add a command to history.
    ///
    /// Commands are stored most-recent-last. Duplicates of the most recent
    /// entry are ignored. The buffer is capped at `MAX_COMMAND_HISTORY`.
    pub fn push_command(&mut self, text: NonEmptyString) {
        if self
            .commands
            .last()
            .is_some_and(|last| last == text.as_str())
        {
            return;
        }
        self.commands.push(text.into_inner());
        if self.commands.len() > MAX_COMMAND_HISTORY {
            self.commands.remove(0);
        }
    }

    /// Navigate to the previous (older) prompt.
    ///
    /// On first call, stashes `current` and returns the most recent prompt.
    /// On subsequent calls, returns the next older prompt.
    /// Returns `AtBoundary` if history is empty or already at the oldest entry.
    pub fn navigate_prompt_up(&mut self, current: &str) -> NavOutcome {
        let cur_idx = match &self.prompt_nav {
            NavState::Idle => None,
            NavState::Active { index, .. } => Some(*index),
        };

        match cur_idx {
            None => match self.prompts.len().checked_sub(1) {
                None => NavOutcome::AtBoundary,
                Some(last_idx) => {
                    self.prompt_nav = NavState::Active {
                        index: last_idx,
                        stash: current.to_owned(),
                    };
                    NavOutcome::Moved(self.prompts[last_idx].clone())
                }
            },
            Some(0) => NavOutcome::AtBoundary,
            Some(idx) => {
                let new_idx = idx - 1;
                if let NavState::Active { index, .. } = &mut self.prompt_nav {
                    *index = new_idx;
                }
                NavOutcome::Moved(self.prompts[new_idx].clone())
            }
        }
    }

    /// Navigate to the next (newer) prompt.
    ///
    /// Returns `AtBoundary` if not currently navigating.
    pub fn navigate_prompt_down(&mut self) -> NavOutcome {
        let cur_idx = match &self.prompt_nav {
            NavState::Idle => return NavOutcome::AtBoundary,
            NavState::Active { index, .. } => *index,
        };

        if cur_idx + 1 >= self.prompts.len() {
            let NavState::Active { stash, .. } = take(&mut self.prompt_nav) else {
                unreachable!()
            };
            NavOutcome::Moved(stash)
        } else {
            let new_idx = cur_idx + 1;
            if let NavState::Active { index, .. } = &mut self.prompt_nav {
                *index = new_idx;
            }
            NavOutcome::Moved(self.prompts[new_idx].clone())
        }
    }

    /// Navigate to the previous (older) command.
    ///
    /// On first call, stashes `current` and returns the most recent command.
    /// On subsequent calls, returns the next older command.
    /// Returns `AtBoundary` if history is empty or already at the oldest entry.
    pub fn navigate_command_up(&mut self, current: &str) -> NavOutcome {
        let cur_idx = match &self.command_nav {
            NavState::Idle => None,
            NavState::Active { index, .. } => Some(*index),
        };

        match cur_idx {
            None => match self.commands.len().checked_sub(1) {
                None => NavOutcome::AtBoundary,
                Some(last_idx) => {
                    self.command_nav = NavState::Active {
                        index: last_idx,
                        stash: current.to_owned(),
                    };
                    NavOutcome::Moved(self.commands[last_idx].clone())
                }
            },
            Some(0) => NavOutcome::AtBoundary,
            Some(idx) => {
                let new_idx = idx - 1;
                if let NavState::Active { index, .. } = &mut self.command_nav {
                    *index = new_idx;
                }
                NavOutcome::Moved(self.commands[new_idx].clone())
            }
        }
    }

    /// Navigate to the next (newer) command.
    ///
    /// Returns `AtBoundary` if not currently navigating.
    pub fn navigate_command_down(&mut self) -> NavOutcome {
        let cur_idx = match &self.command_nav {
            NavState::Idle => return NavOutcome::AtBoundary,
            NavState::Active { index, .. } => *index,
        };

        if cur_idx + 1 >= self.commands.len() {
            let NavState::Active { stash, .. } = take(&mut self.command_nav) else {
                unreachable!()
            };
            NavOutcome::Moved(stash)
        } else {
            let new_idx = cur_idx + 1;
            if let NavState::Active { index, .. } = &mut self.command_nav {
                *index = new_idx;
            }
            NavOutcome::Moved(self.commands[new_idx].clone())
        }
    }

    /// Reset navigation state.
    ///
    /// Call this after submitting a prompt or command to clear the
    /// navigation indices and stashed drafts.
    pub fn reset_navigation(&mut self) {
        self.prompt_nav = NavState::Idle;
        self.command_nav = NavState::Idle;
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
    use super::{InputHistory, MAX_COMMAND_HISTORY, MAX_PROMPT_HISTORY, NavOutcome, NavState};
    use crate::NonEmptyString;

    fn ne(s: &str) -> NonEmptyString {
        NonEmptyString::new(s).unwrap()
    }

    fn assert_moved(outcome: NavOutcome, expected: &str) {
        match outcome {
            NavOutcome::Moved(text) => assert_eq!(text, expected),
            NavOutcome::AtBoundary => panic!("expected Moved({expected:?}), got AtBoundary"),
        }
    }

    fn assert_at_boundary(outcome: NavOutcome) {
        assert!(
            matches!(outcome, NavOutcome::AtBoundary),
            "expected AtBoundary, got Moved"
        );
    }

    #[test]
    fn push_prompt_adds_to_history() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("hello"));
        assert_eq!(history.prompt_count(), 1);
    }

    #[test]
    fn push_prompt_ignores_duplicate_of_last() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("hello"));
        history.push_prompt(ne("hello"));
        assert_eq!(history.prompt_count(), 1);
    }

    #[test]
    fn push_prompt_allows_duplicate_not_at_end() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("hello"));
        history.push_prompt(ne("world"));
        history.push_prompt(ne("hello"));
        assert_eq!(history.prompt_count(), 3);
    }

    #[test]
    fn push_prompt_caps_at_max() {
        let mut history = InputHistory::default();
        for i in 0..MAX_PROMPT_HISTORY + 10 {
            history.push_prompt(ne(&format!("prompt {i}")));
        }
        assert_eq!(history.prompt_count(), MAX_PROMPT_HISTORY);
    }

    #[test]
    fn push_command_adds_to_history() {
        let mut history = InputHistory::default();
        history.push_command(ne("quit"));
        assert_eq!(history.command_count(), 1);
    }

    #[test]
    fn push_command_caps_at_max() {
        let mut history = InputHistory::default();
        for i in 0..MAX_COMMAND_HISTORY + 10 {
            history.push_command(ne(&format!("cmd {i}")));
        }
        assert_eq!(history.command_count(), MAX_COMMAND_HISTORY);
    }

    #[test]
    fn navigate_prompt_up_empty_history() {
        let mut history = InputHistory::default();
        assert_at_boundary(history.navigate_prompt_up("current"));
    }

    #[test]
    fn navigate_prompt_up_first_call() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("first"));
        history.push_prompt(ne("second"));

        assert_moved(history.navigate_prompt_up("current draft"), "second");
        assert!(matches!(
            history.prompt_nav,
            NavState::Active { ref stash, .. } if stash == "current draft"
        ));
    }

    #[test]
    fn navigate_prompt_up_through_history() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("first"));
        history.push_prompt(ne("second"));
        history.push_prompt(ne("third"));

        assert_moved(history.navigate_prompt_up("draft"), "third");
        assert_moved(history.navigate_prompt_up(""), "second");
        assert_moved(history.navigate_prompt_up(""), "first");
        assert_at_boundary(history.navigate_prompt_up(""));
    }

    #[test]
    fn navigate_prompt_down_not_navigating() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("first"));
        assert_at_boundary(history.navigate_prompt_down());
    }

    #[test]
    fn navigate_prompt_down_restores_stash() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("first"));

        history.navigate_prompt_up("my draft");
        assert_moved(history.navigate_prompt_down(), "my draft");
    }

    #[test]
    fn navigate_prompt_down_through_history() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("first"));
        history.push_prompt(ne("second"));
        history.push_prompt(ne("third"));

        history.navigate_prompt_up("draft");
        history.navigate_prompt_up("");
        history.navigate_prompt_up("");

        assert_moved(history.navigate_prompt_down(), "second");
        assert_moved(history.navigate_prompt_down(), "third");
        assert_moved(history.navigate_prompt_down(), "draft");
    }

    #[test]
    fn navigate_command_up_empty_history() {
        let mut history = InputHistory::default();
        assert_at_boundary(history.navigate_command_up("current"));
    }

    #[test]
    fn navigate_command_up_first_call() {
        let mut history = InputHistory::default();
        history.push_command(ne("quit"));
        history.push_command(ne("clear"));

        assert_moved(history.navigate_command_up("current cmd"), "clear");
        assert!(matches!(
            history.command_nav,
            NavState::Active { ref stash, .. } if stash == "current cmd"
        ));
    }

    #[test]
    fn navigate_command_down_restores_stash() {
        let mut history = InputHistory::default();
        history.push_command(ne("quit"));

        history.navigate_command_up("my command");
        assert_moved(history.navigate_command_down(), "my command");
    }

    #[test]
    fn reset_navigation_clears_state() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("prompt"));
        history.push_command(ne("cmd"));

        history.navigate_prompt_up("draft");
        history.navigate_command_up("cmd draft");

        history.reset_navigation();

        assert!(matches!(history.prompt_nav, NavState::Idle));
        assert!(matches!(history.command_nav, NavState::Idle));
    }

    #[test]
    fn serialization_preserves_history() {
        let mut history = InputHistory::default();
        history.push_prompt(ne("prompt1"));
        history.push_prompt(ne("prompt2"));
        history.push_command(ne("cmd1"));

        history.navigate_prompt_up("draft");

        let json = serde_json::to_string(&history).unwrap();
        let restored: InputHistory = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.prompt_count(), 2);
        assert_eq!(restored.command_count(), 1);
        assert!(matches!(restored.prompt_nav, NavState::Idle));
    }
}

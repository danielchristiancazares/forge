//! Thinking payload types.
//!
//! Invariant-First Architecture note:
//! - Core payloads should not use `Option<T>` to represent "maybe present" fields.
//! - Thinking may be absent in legitimate domains (model/provider did not emit it,
//!   or it was stripped by policy), so we model that absence explicitly.

use forge_types::{ThinkingMessage, ThinkingReplayState};

/// Provider thinking/reasoning payload captured during a turn.
///
/// This replaces `Option<Message>` in core tool-loop payloads.
#[derive(Debug, Clone)]
pub enum ThinkingPayload {
    /// No thinking content was produced (or it was intentionally withheld).
    NotProvided,
    /// A thinking message was produced.
    Provided(ThinkingMessage),
}

impl ThinkingPayload {
    #[must_use]
    pub fn replay_state_for_journal(&self) -> ThinkingReplayState {
        match self {
            Self::NotProvided => ThinkingReplayState::Unsigned,
            Self::Provided(message) => message.replay_state().clone(),
        }
    }
}

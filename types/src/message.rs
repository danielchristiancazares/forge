//! Core message domain model.
//!
//! Contains the `Message` sum type and its role-specific structs.
//! Constructors take `SystemTime` explicitly; callers own the clock.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::model::{ModelName, Provider};
use crate::proofs::{EmptyStringError, NonEmptyString, normalize_non_empty_for_persistence};
use crate::{OpenAIReasoningItem, ThinkingReplayState, ThoughtSignature, ToolCall, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    content: NonEmptyString,
    timestamp: SystemTime,
}

impl SystemMessage {
    #[must_use]
    pub fn new(content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self { content, timestamp }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            content: normalize_non_empty_for_persistence(&self.content),
            timestamp: self.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    content: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    display_content: Option<NonEmptyString>,
    timestamp: SystemTime,
}

impl UserMessage {
    #[must_use]
    pub fn new(content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self {
            content,
            display_content: None,
            timestamp,
        }
    }

    #[must_use]
    pub fn with_display(
        content: NonEmptyString,
        display_content: NonEmptyString,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            content,
            display_content: Some(display_content),
            timestamp,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn display_content(&self) -> &str {
        self.display_content
            .as_ref()
            .map_or_else(|| self.content.as_str(), NonEmptyString::as_str)
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            content: normalize_non_empty_for_persistence(&self.content),
            display_content: self
                .display_content
                .as_ref()
                .map(normalize_non_empty_for_persistence),
            timestamp: self.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    content: NonEmptyString,
    timestamp: SystemTime,
    #[serde(flatten)]
    model: ModelName,
}

impl AssistantMessage {
    #[must_use]
    pub fn new(model: ModelName, content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self {
            content,
            timestamp,
            model,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    #[must_use]
    pub fn model(&self) -> &ModelName {
        &self.model
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            content: normalize_non_empty_for_persistence(&self.content),
            timestamp: self.timestamp,
            model: self.model.clone(),
        }
    }
}

/// Provider reasoning/thinking content (Claude extended thinking, Gemini thinking, etc.).
///
/// This is separate from `AssistantMessage` because thinking is metadata about the
/// reasoning process, not part of the actual response. It can be shown/hidden
/// independently in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingMessage {
    content: NonEmptyString,
    #[serde(default, alias = "signature")]
    replay: ThinkingReplayState,
    timestamp: SystemTime,
    #[serde(flatten)]
    model: ModelName,
}

impl ThinkingMessage {
    #[must_use]
    pub fn new(model: ModelName, content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::Unsigned,
            timestamp,
            model,
        }
    }

    #[must_use]
    pub fn with_signature(
        model: ModelName,
        content: NonEmptyString,
        signature: String,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::ClaudeSigned {
                signature: ThoughtSignature::new(signature),
            },
            timestamp,
            model,
        }
    }

    #[must_use]
    pub fn with_openai_reasoning(
        model: ModelName,
        content: NonEmptyString,
        items: Vec<OpenAIReasoningItem>,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::OpenAIReasoning { items },
            timestamp,
            model,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn replay_state(&self) -> &ThinkingReplayState {
        &self.replay
    }

    #[must_use]
    pub fn requires_persistence(&self) -> bool {
        self.replay.requires_persistence()
    }

    #[must_use]
    pub fn claude_signature(&self) -> Option<&ThoughtSignature> {
        match &self.replay {
            ThinkingReplayState::ClaudeSigned { signature } => Some(signature),
            _ => None,
        }
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    #[must_use]
    pub fn model(&self) -> &ModelName {
        &self.model
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            content: normalize_non_empty_for_persistence(&self.content),
            replay: self.replay.clone(),
            timestamp: self.timestamp,
            model: self.model.clone(),
        }
    }
}

/// A complete message.
///
/// This is a real sum type (not a `Role` tag + "sometimes-meaningful" fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    Thinking(ThinkingMessage),
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}

impl Message {
    #[must_use]
    pub fn system(content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self::System(SystemMessage::new(content, timestamp))
    }

    #[must_use]
    pub fn user(content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self::User(UserMessage::new(content, timestamp))
    }

    #[must_use]
    pub fn user_with_display(
        content: NonEmptyString,
        display_content: NonEmptyString,
        timestamp: SystemTime,
    ) -> Self {
        Self::User(UserMessage::with_display(
            content,
            display_content,
            timestamp,
        ))
    }

    pub fn try_user(
        content: impl Into<String>,
        timestamp: SystemTime,
    ) -> Result<Self, EmptyStringError> {
        Ok(Self::user(NonEmptyString::new(content)?, timestamp))
    }

    #[must_use]
    pub fn assistant(model: ModelName, content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self::Assistant(AssistantMessage::new(model, content, timestamp))
    }

    #[must_use]
    pub fn thinking(model: ModelName, content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self::Thinking(ThinkingMessage::new(model, content, timestamp))
    }

    #[must_use]
    pub fn thinking_with_signature(
        model: ModelName,
        content: NonEmptyString,
        signature: String,
        timestamp: SystemTime,
    ) -> Self {
        Self::Thinking(ThinkingMessage::with_signature(
            model, content, signature, timestamp,
        ))
    }

    #[must_use]
    pub fn thinking_with_openai_reasoning(
        model: ModelName,
        content: NonEmptyString,
        items: Vec<OpenAIReasoningItem>,
        timestamp: SystemTime,
    ) -> Self {
        Self::Thinking(ThinkingMessage::with_openai_reasoning(
            model, content, items, timestamp,
        ))
    }

    #[must_use]
    pub fn tool_use(call: ToolCall) -> Self {
        Self::ToolUse(call)
    }

    #[must_use]
    pub fn tool_result(result: ToolResult) -> Self {
        Self::ToolResult(result)
    }

    #[must_use]
    pub fn role_str(&self) -> &'static str {
        match self {
            Message::System(_) => "system",
            Message::User(_) | Message::ToolResult(_) => "user",
            Message::Assistant(_) | Message::Thinking(_) | Message::ToolUse(_) => "assistant",
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            Message::System(m) => m.content(),
            Message::User(m) => m.content(),
            Message::Assistant(m) => m.content(),
            Message::Thinking(m) => m.content(),
            Message::ToolUse(call) => &call.name,
            Message::ToolResult(result) => &result.content,
        }
    }

    #[must_use]
    pub fn display_content(&self) -> &str {
        match self {
            Message::User(m) => m.display_content(),
            other => other.content(),
        }
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        match self {
            Message::System(message) => Message::System(message.normalized_for_persistence()),
            Message::User(message) => Message::User(message.normalized_for_persistence()),
            Message::Assistant(message) => Message::Assistant(message.normalized_for_persistence()),
            Message::Thinking(message) => Message::Thinking(message.normalized_for_persistence()),
            Message::ToolUse(call) => Message::ToolUse(call.normalized_for_persistence()),
            Message::ToolResult(result) => Message::ToolResult(result.normalized_for_persistence()),
        }
    }
}

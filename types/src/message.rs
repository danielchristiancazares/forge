//! Core message domain model.
//!
//! Contains the `Message` sum type and its role-specific structs.
//! Constructors take `SystemTime` explicitly; callers own the clock.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use crate::model::{ModelName, Provider};
use crate::proofs::{EmptyStringError, NonEmptyString, normalize_non_empty_for_persistence};
use crate::{
    OpenAIReasoningItem, ThinkingReplayState, ThoughtSignature, ThoughtSignatureState, ToolCall,
    ToolResult, sanitize_terminal_text, strip_steganographic_chars,
};

const SANITIZED_EMPTY_PLACEHOLDER: &str = "[content removed by sanitizer]";

fn sanitize_untrusted_owned(input: &str) -> String {
    let terminal_safe = sanitize_terminal_text(input);
    match strip_steganographic_chars(terminal_safe.as_ref()) {
        Cow::Borrowed(_) => terminal_safe.into_owned(),
        Cow::Owned(stripped) => stripped,
    }
}

fn sanitize_non_empty_untrusted(content: NonEmptyString) -> NonEmptyString {
    let sanitized = sanitize_untrusted_owned(content.as_str());
    NonEmptyString::new(sanitized).unwrap_or_else(|_| {
        NonEmptyString::try_from(SANITIZED_EMPTY_PLACEHOLDER)
            .expect("SANITIZED_EMPTY_PLACEHOLDER must be non-empty")
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
enum UserDisplayContent {
    #[default]
    Canonical,
    Override(NonEmptyString),
}

impl UserDisplayContent {
    #[must_use]
    pub const fn is_canonical(&self) -> bool {
        matches!(self, Self::Canonical)
    }

    #[must_use]
    pub fn as_str<'a>(&'a self, content: &'a NonEmptyString) -> &'a str {
        match self {
            Self::Canonical => content.as_str(),
            Self::Override(display_content) => display_content.as_str(),
        }
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        match self {
            Self::Canonical => Self::Canonical,
            Self::Override(display_content) => {
                Self::Override(normalize_non_empty_for_persistence(display_content))
            }
        }
    }
}

mod user_display_content_wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::NonEmptyString;

    use super::UserDisplayContent;

    pub fn serialize<S>(value: &UserDisplayContent, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            UserDisplayContent::Canonical => serializer.serialize_none(),
            UserDisplayContent::Override(display_content) => display_content.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<UserDisplayContent, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            DisplayContent(NonEmptyString),
            Null(()),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::DisplayContent(display_content) => UserDisplayContent::Override(display_content),
            Wire::Null(()) => UserDisplayContent::Canonical,
        })
    }
}

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
    #[serde(
        default,
        skip_serializing_if = "UserDisplayContent::is_canonical",
        with = "user_display_content_wire"
    )]
    display_content: UserDisplayContent,
    timestamp: SystemTime,
}

impl UserMessage {
    #[must_use]
    pub fn new(content: NonEmptyString, timestamp: SystemTime) -> Self {
        Self {
            content,
            display_content: UserDisplayContent::Canonical,
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
            display_content: UserDisplayContent::Override(display_content),
            timestamp,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn display_content(&self) -> &str {
        self.display_content.as_str(&self.content)
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            content: normalize_non_empty_for_persistence(&self.content),
            display_content: self.display_content.normalized_for_persistence(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeSignatureRef<'a> {
    Unsigned,
    Signed(&'a ThoughtSignature),
}

impl ClaudeSignatureRef<'_> {
    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(self, Self::Signed(_))
    }
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
    pub fn claude_signature(&self) -> ClaudeSignatureRef<'_> {
        match &self.replay {
            ThinkingReplayState::ClaudeSigned { signature } => {
                ClaudeSignatureRef::Signed(signature)
            }
            _ => ClaudeSignatureRef::Unsigned,
        }
    }

    #[must_use]
    pub fn claude_signature_state(&self) -> ThoughtSignatureState {
        match self.claude_signature() {
            ClaudeSignatureRef::Unsigned => ThoughtSignatureState::Unsigned,
            ClaudeSignatureRef::Signed(signature) => {
                ThoughtSignatureState::Signed(signature.clone())
            }
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
        let content = sanitize_non_empty_untrusted(content);
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
        let sanitized = ToolResult {
            content: sanitize_untrusted_owned(&result.content),
            ..result
        };
        Self::ToolResult(sanitized)
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

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::{ClaudeSignatureRef, Message, ThinkingMessage, UserMessage};
    use crate::{NonEmptyString, Provider, ToolResult};

    fn non_empty(value: &str) -> NonEmptyString {
        NonEmptyString::new(value).unwrap()
    }

    #[test]
    fn user_message_without_override_omits_display_content_field() {
        let message = UserMessage::new(non_empty("hello"), UNIX_EPOCH);
        let json = serde_json::to_value(&message).unwrap();
        let object = json.as_object().unwrap();
        assert!(!object.contains_key("display_content"));
    }

    #[test]
    fn user_message_deserializes_legacy_missing_display_content() {
        let json = serde_json::json!({
            "content": "hello",
            "timestamp": { "secs_since_epoch": 0, "nanos_since_epoch": 0 }
        });
        let message: UserMessage = serde_json::from_value(json).unwrap();
        assert_eq!(message.content(), "hello");
        assert_eq!(message.display_content(), "hello");
    }

    #[test]
    fn thinking_message_signature_ref_is_explicit() {
        let unsigned = ThinkingMessage::new(
            Provider::Claude.default_model(),
            non_empty("thinking"),
            UNIX_EPOCH,
        );
        assert!(matches!(
            unsigned.claude_signature(),
            ClaudeSignatureRef::Unsigned
        ));

        let signed = ThinkingMessage::with_signature(
            Provider::Claude.default_model(),
            non_empty("thinking"),
            "abc".to_string(),
            UNIX_EPOCH,
        );
        assert!(matches!(
            signed.claude_signature(),
            ClaudeSignatureRef::Signed(_)
        ));
        assert!(signed.claude_signature_state().is_signed());
    }

    #[test]
    fn assistant_constructor_sanitizes_steganographic_content() {
        let message = Message::assistant(
            Provider::Claude.default_model(),
            non_empty("Hello\u{200B}World"),
            UNIX_EPOCH,
        );
        assert_eq!(message.content(), "HelloWorld");
    }

    #[test]
    fn tool_result_constructor_sanitizes_terminal_escape_content() {
        let message =
            Message::tool_result(ToolResult::success("call-1", "Read", "Hello\x1b[31mWorld"));
        assert_eq!(message.content(), "HelloWorld");
    }
}

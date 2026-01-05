use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::provider::{ModelName, Provider, StreamEvent};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyString(String);

#[derive(Debug, Error)]
#[error("message content must not be empty")]
pub struct EmptyStringError;

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyStringError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value))
        }
    }

    pub fn append(mut self, suffix: impl AsRef<str>) -> Self {
        self.0.push_str(suffix.as_ref());
        Self(self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonEmptyStaticStr(&'static str);

impl NonEmptyStaticStr {
    pub const fn new(value: &'static str) -> Self {
        if value.is_empty() {
            panic!("NonEmptyStaticStr must not be empty");
        }
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl From<NonEmptyStaticStr> for NonEmptyString {
    fn from(value: NonEmptyStaticStr) -> Self {
        Self(value.0.to_string())
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

impl std::ops::Deref for NonEmptyString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    content: NonEmptyString,
    timestamp: SystemTime,
}

impl SystemMessage {
    pub fn new(content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
        }
    }

    pub fn content(&self) -> &str {
        self.content.as_str()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    content: NonEmptyString,
    timestamp: SystemTime,
}

impl UserMessage {
    pub fn new(content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
        }
    }

    pub fn content(&self) -> &str {
        self.content.as_str()
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
    pub fn new(model: ModelName, content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
            model,
        }
    }

    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    pub fn provider(&self) -> Provider {
        self.model.provider()
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
}

impl Message {
    pub fn system(content: NonEmptyString) -> Self {
        Self::System(SystemMessage::new(content))
    }

    pub fn user(content: NonEmptyString) -> Self {
        Self::User(UserMessage::new(content))
    }

    #[allow(dead_code)]
    pub fn try_user(content: impl Into<String>) -> Result<Self, EmptyStringError> {
        Ok(Self::user(NonEmptyString::new(content)?))
    }

    pub fn assistant(model: ModelName, content: NonEmptyString) -> Self {
        Self::Assistant(AssistantMessage::new(model, content))
    }

    pub fn role_str(&self) -> &'static str {
        match self {
            Message::System(_) => "system",
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
        }
    }

    pub fn content(&self) -> &str {
        match self {
            Message::System(m) => m.content(),
            Message::User(m) => m.content(),
            Message::Assistant(m) => m.content(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamFinishReason {
    Done,
    Error(String),
}

/// A message being streamed - existence proves streaming is active.
/// Typestate: consuming this produces a complete assistant `Message`.
#[derive(Debug)]
pub struct StreamingMessage {
    model: ModelName,
    content: String,
    timestamp: SystemTime,
    receiver: mpsc::UnboundedReceiver<StreamEvent>,
}

impl StreamingMessage {
    pub fn new(model: ModelName, receiver: mpsc::UnboundedReceiver<StreamEvent>) -> Self {
        Self {
            model,
            content: String::new(),
            timestamp: SystemTime::now(),
            receiver,
        }
    }

    pub fn provider(&self) -> Provider {
        self.model.provider()
    }

    pub fn model_name(&self) -> &ModelName {
        &self.model
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn try_recv_event(&mut self) -> Result<StreamEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn apply_event(&mut self, event: StreamEvent) -> Option<StreamFinishReason> {
        match event {
            StreamEvent::TextDelta(text) => {
                self.content.push_str(&text);
                None
            }
            StreamEvent::Done => Some(StreamFinishReason::Done),
            StreamEvent::Error(err) => Some(StreamFinishReason::Error(err)),
        }
    }

    /// Consume streaming message and produce a complete message.
    pub fn into_message(self) -> Result<Message, EmptyStringError> {
        let content = NonEmptyString::new(self.content)?;
        Ok(Message::Assistant(AssistantMessage {
            content,
            timestamp: self.timestamp,
            model: self.model,
        }))
    }
}

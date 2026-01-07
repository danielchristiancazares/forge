//! Core domain types for Forge.
//!
//! This crate contains pure domain types with no IO, no async, and minimal dependencies.
//! Everything here can be used from any layer of the application.

mod sanitize;
pub use sanitize::sanitize_terminal_text;

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::SystemTime;
use thiserror::Error;

// ============================================================================
// NonEmpty String Types
// ============================================================================

/// A string guaranteed to be non-empty (after trimming).
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

/// A compile-time checked non-empty static string.
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

// ============================================================================
// Provider & Model Types
// ============================================================================

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    Claude,
    OpenAI,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAI => "openai",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "GPT",
        }
    }

    pub fn env_var(&self) -> &'static str {
        match self {
            Provider::Claude => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            Provider::Claude => ModelName::known(*self, "claude-sonnet-4-5-20250929"),
            Provider::OpenAI => ModelName::known(*self, "gpt-5.2"),
        }
    }

    /// All available models for this provider.
    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            Provider::Claude => &[
                "claude-sonnet-4-5-20250929",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-5-20251101",
            ],
            Provider::OpenAI => &["gpt-5.2", "gpt-5.2-2025-12-11", "gpt-5.2-chat-latest"],
        }
    }

    /// Parse a model name for this provider.
    pub fn parse_model(&self, raw: &str) -> Result<ModelName, ModelParseError> {
        ModelName::parse(*self, raw)
    }

    /// Parse provider from string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Provider::Claude),
            "openai" | "gpt" | "chatgpt" => Some(Provider::OpenAI),
            _ => None,
        }
    }

    /// Get all available providers.
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI]
    }
}

/// Whether a model name is verified/known or user-supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModelNameKind {
    Known,
    #[default]
    Unverified,
}

#[derive(Debug, Error)]
pub enum ModelParseError {
    #[error("model name cannot be empty")]
    Empty,
    #[error("Claude model must start with claude- (got {0})")]
    ClaudePrefix(String),
    #[error("OpenAI model must start with gpt-5 (got {0})")]
    OpenAIMinimum(String),
}

/// Provider-scoped model name.
///
/// This prevents mixing model names across providers and makes unknown names explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelName {
    provider: Provider,
    #[serde(rename = "model")]
    name: Cow<'static, str>,
    #[serde(default)]
    kind: ModelNameKind,
}

impl ModelName {
    pub fn parse(provider: Provider, raw: &str) -> Result<Self, ModelParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ModelParseError::Empty);
        }

        if provider == Provider::Claude && !trimmed.to_ascii_lowercase().starts_with("claude-") {
            return Err(ModelParseError::ClaudePrefix(trimmed.to_string()));
        }

        if provider == Provider::OpenAI && !trimmed.to_ascii_lowercase().starts_with("gpt-5") {
            return Err(ModelParseError::OpenAIMinimum(trimmed.to_string()));
        }

        if let Some(known) = provider
            .available_models()
            .iter()
            .find(|model| model.eq_ignore_ascii_case(trimmed))
        {
            return Ok(Self {
                provider,
                name: Cow::Borrowed(*known),
                kind: ModelNameKind::Known,
            });
        }

        Ok(Self {
            provider,
            name: Cow::Owned(trimmed.to_string()),
            kind: ModelNameKind::Unverified,
        })
    }

    pub const fn known(provider: Provider, name: &'static str) -> Self {
        Self {
            provider,
            name: Cow::Borrowed(name),
            kind: ModelNameKind::Known,
        }
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub fn as_str(&self) -> &str {
        self.name.as_ref()
    }

    pub const fn kind(&self) -> ModelNameKind {
        self.kind
    }
}

impl std::fmt::Display for ModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

// ============================================================================
// API Key Types
// ============================================================================

/// Provider-scoped API key.
///
/// This prevents the invalid state "OpenAI key used with Claude" from being representable.
#[derive(Debug, Clone)]
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
}

impl ApiKey {
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ApiKey::Claude(key) | ApiKey::OpenAI(key) => key,
        }
    }
}

// ============================================================================
// OpenAI Request Options
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAIReasoningEffort {
    None,
    Low,
    Medium,
    #[default]
    High,
    XHigh,
}

impl OpenAIReasoningEffort {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "x-high" => Some(Self::XHigh),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAITextVerbosity {
    Low,
    Medium,
    #[default]
    High,
}

impl OpenAITextVerbosity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAITruncation {
    #[default]
    Auto,
    Disabled,
}

impl OpenAITruncation {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,
    verbosity: OpenAITextVerbosity,
    truncation: OpenAITruncation,
}

impl OpenAIRequestOptions {
    pub fn new(
        reasoning_effort: OpenAIReasoningEffort,
        verbosity: OpenAITextVerbosity,
        truncation: OpenAITruncation,
    ) -> Self {
        Self {
            reasoning_effort,
            verbosity,
            truncation,
        }
    }

    pub fn reasoning_effort(self) -> OpenAIReasoningEffort {
        self.reasoning_effort
    }

    pub fn verbosity(self) -> OpenAITextVerbosity {
        self.verbosity
    }

    pub fn truncation(self) -> OpenAITruncation {
        self.truncation
    }
}

impl Default for OpenAIRequestOptions {
    fn default() -> Self {
        Self::new(
            OpenAIReasoningEffort::default(),
            OpenAITextVerbosity::default(),
            OpenAITruncation::default(),
        )
    }
}

// ============================================================================
// Caching & Output Limits
// ============================================================================

/// Hint for whether content should be cached by the provider.
///
/// Different providers handle caching differently:
/// - Claude: Explicit `cache_control: { type: "ephemeral" }` markers
/// - OpenAI: Automatic server-side prefix caching (hints ignored)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheHint {
    /// No caching preference - provider uses default behavior.
    #[default]
    None,
    /// Content is stable and should be cached if supported.
    ///
    /// Named "Ephemeral" to match Anthropic's API terminology. Despite the name,
    /// this actually means "cache this content" - Anthropic uses "ephemeral" to
    /// indicate the cache entry has a limited TTL (~5 min) rather than permanent
    /// storage. The content itself should be stable/unchanging for caching to help.
    Ephemeral,
}

/// Validated output configuration that guarantees invariants.
///
/// If thinking is enabled, `thinking_budget < max_output_tokens` is guaranteed
/// by construction. You cannot create an invalid `OutputLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    max_output_tokens: u32,
    thinking_budget: Option<u32>,
}

/// Error when trying to construct invalid output limits.
#[derive(Debug, Clone, Error)]
pub enum OutputLimitsError {
    #[error("thinking budget ({budget}) must be less than max output tokens ({max_output})")]
    ThinkingBudgetTooLarge { budget: u32, max_output: u32 },
    #[error("thinking budget must be at least 1024 tokens")]
    ThinkingBudgetTooSmall,
}

impl OutputLimits {
    /// Create output limits without thinking.
    pub const fn new(max_output_tokens: u32) -> Self {
        Self {
            max_output_tokens,
            thinking_budget: None,
        }
    }

    /// Create output limits with thinking enabled.
    ///
    /// Returns an error if `thinking_budget >= max_output_tokens` or `thinking_budget < 1024`.
    pub fn with_thinking(
        max_output_tokens: u32,
        thinking_budget: u32,
    ) -> Result<Self, OutputLimitsError> {
        if thinking_budget < 1024 {
            return Err(OutputLimitsError::ThinkingBudgetTooSmall);
        }
        if thinking_budget >= max_output_tokens {
            return Err(OutputLimitsError::ThinkingBudgetTooLarge {
                budget: thinking_budget,
                max_output: max_output_tokens,
            });
        }
        Ok(Self {
            max_output_tokens,
            thinking_budget: Some(thinking_budget),
        })
    }

    /// Get max output tokens.
    pub const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    /// Get thinking budget if enabled.
    pub const fn thinking_budget(&self) -> Option<u32> {
        self.thinking_budget
    }

    /// Check if thinking is enabled.
    pub const fn has_thinking(&self) -> bool {
        self.thinking_budget.is_some()
    }
}

// ============================================================================
// Streaming Events
// ============================================================================

/// Streaming event from the API.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta.
    TextDelta(String),
    /// Thinking/reasoning content delta (Claude extended thinking).
    ThinkingDelta(String),
    /// Stream completed.
    Done,
    /// Error occurred.
    Error(String),
}

/// Reason a stream finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamFinishReason {
    Done,
    Error(String),
}

// ============================================================================
// Message Types
// ============================================================================

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

    pub fn model(&self) -> &ModelName {
        &self.model
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

/// A message with an associated cache hint for API serialization.
#[derive(Debug, Clone)]
pub struct CacheableMessage {
    pub message: Message,
    pub cache_hint: CacheHint,
}

impl CacheableMessage {
    pub fn new(message: Message, cache_hint: CacheHint) -> Self {
        Self {
            message,
            cache_hint,
        }
    }

    /// Create a message with no cache hint.
    pub fn plain(message: Message) -> Self {
        Self::new(message, CacheHint::None)
    }

    /// Create a message marked for caching.
    pub fn cached(message: Message) -> Self {
        Self::new(message, CacheHint::Ephemeral)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_string_rejects_empty() {
        assert!(NonEmptyString::new("").is_err());
        assert!(NonEmptyString::new("   ").is_err());
        assert!(NonEmptyString::new("hello").is_ok());
    }

    #[test]
    fn provider_from_str_parses_aliases() {
        assert_eq!(Provider::parse("claude"), Some(Provider::Claude));
        assert_eq!(Provider::parse("Anthropic"), Some(Provider::Claude));
        assert_eq!(Provider::parse("openai"), Some(Provider::OpenAI));
        assert_eq!(Provider::parse("gpt"), Some(Provider::OpenAI));
        assert_eq!(Provider::parse("unknown"), None);
    }

    #[test]
    fn model_parse_validates_openai_prefix() {
        let provider = Provider::OpenAI;
        assert!(provider.parse_model("gpt-4o").is_err());
        assert!(provider.parse_model("gpt-5.2").is_ok());
    }

    #[test]
    fn model_parse_validates_claude_prefix() {
        let provider = Provider::Claude;
        assert!(provider.parse_model("gpt-5.2").is_err());
        assert!(provider.parse_model("claude-sonnet-4-5-20250929").is_ok());
    }

    #[test]
    fn output_limits_validates_thinking_budget() {
        assert!(OutputLimits::with_thinking(4096, 512).is_err()); // too small
        assert!(OutputLimits::with_thinking(4096, 5000).is_err()); // too large
        assert!(OutputLimits::with_thinking(8192, 4096).is_ok());
    }
}

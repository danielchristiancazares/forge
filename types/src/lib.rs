//! Core domain types for Forge.
//!
//! This crate contains pure domain types with no IO, no async, and minimal dependencies.
//! Everything here can be used from any layer of the application.

// Pedantic lint configuration - these are intentional design choices
#![allow(clippy::missing_errors_doc)] // Result-returning functions are self-explanatory
#![allow(clippy::missing_panics_doc)] // Panics are documented in assertions

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

    #[must_use]
    pub fn append(mut self, suffix: impl AsRef<str>) -> Self {
        self.0.push_str(suffix.as_ref());
        Self(self.0)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
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
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        assert!(!value.is_empty(), "NonEmptyStaticStr must not be empty");
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl TryFrom<NonEmptyStaticStr> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: NonEmptyStaticStr) -> Result<Self, Self::Error> {
        Self::new(value.0)
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
    Gemini,
}

impl Provider {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAI => "openai",
            Provider::Gemini => "gemini",
        }
    }

    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "GPT",
            Provider::Gemini => "Gemini",
        }
    }

    #[must_use]
    pub fn env_var(&self) -> &'static str {
        match self {
            Provider::Claude => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
        }
    }

    #[must_use]
    pub fn default_model(&self) -> ModelName {
        match self {
            Provider::Claude => ModelName::known(*self, "claude-sonnet-4-5-20250929"),
            Provider::OpenAI => ModelName::known(*self, "gpt-5.2"),
            Provider::Gemini => ModelName::known(*self, "gemini-3-pro-preview"),
        }
    }

    /// All available models for this provider.
    #[must_use]
    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            Provider::Claude => &[
                "claude-sonnet-4-5-20250929",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-5-20251101",
            ],
            Provider::OpenAI => &["gpt-5.2", "gpt-5.2-2025-12-11"],
            Provider::Gemini => &["gemini-3-pro-preview", "gemini-3-flash-preview"],
        }
    }

    /// Parse a model name for this provider.
    pub fn parse_model(&self, raw: &str) -> Result<ModelName, ModelParseError> {
        ModelName::parse(*self, raw)
    }

    /// Parse provider from string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Provider::Claude),
            "openai" | "gpt" | "chatgpt" => Some(Provider::OpenAI),
            "gemini" | "google" => Some(Provider::Gemini),
            _ => None,
        }
    }

    /// Infer provider from model name prefix.
    #[must_use]
    pub fn from_model_name(model: &str) -> Option<Self> {
        let lower = model.trim().to_ascii_lowercase();
        if lower.starts_with("claude-") {
            Some(Provider::Claude)
        } else if lower.starts_with("gpt-") {
            Some(Provider::OpenAI)
        } else if lower.starts_with("gemini-") {
            Some(Provider::Gemini)
        } else {
            None
        }
    }

    /// Get all available providers.
    #[must_use]
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI, Provider::Gemini]
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
    #[error("Gemini model must start with gemini- (got {0})")]
    GeminiPrefix(String),
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

        let lower = trimmed.to_ascii_lowercase();

        if provider == Provider::Claude && !lower.starts_with("claude-") {
            return Err(ModelParseError::ClaudePrefix(trimmed.to_string()));
        }

        if provider == Provider::OpenAI && !lower.starts_with("gpt-5") {
            return Err(ModelParseError::OpenAIMinimum(trimmed.to_string()));
        }

        if provider == Provider::Gemini && !lower.starts_with("gemini-") {
            return Err(ModelParseError::GeminiPrefix(trimmed.to_string()));
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

    #[must_use]
    pub const fn known(provider: Provider, name: &'static str) -> Self {
        Self {
            provider,
            name: Cow::Borrowed(name),
            kind: ModelNameKind::Known,
        }
    }

    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_ref()
    }

    #[must_use]
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
/// This prevents the invalid state "`OpenAI` key used with Claude" from being representable.
///
/// Note: `Debug` is manually implemented to redact the key value, preventing accidental
/// credential disclosure in logs or error messages.
#[derive(Clone)]
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    Gemini(String),
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiKey::Claude(_) => write!(f, "ApiKey::Claude(<redacted>)"),
            ApiKey::OpenAI(_) => write!(f, "ApiKey::OpenAI(<redacted>)"),
            ApiKey::Gemini(_) => write!(f, "ApiKey::Gemini(<redacted>)"),
        }
    }
}

impl ApiKey {
    #[must_use]
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
            ApiKey::Gemini(_) => Provider::Gemini,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            ApiKey::Claude(key) | ApiKey::OpenAI(key) | ApiKey::Gemini(key) => key,
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
    #[must_use]
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

    #[must_use]
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
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    #[must_use]
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
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    #[must_use]
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
    #[must_use]
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

    #[must_use]
    pub fn reasoning_effort(self) -> OpenAIReasoningEffort {
        self.reasoning_effort
    }

    #[must_use]
    pub fn verbosity(self) -> OpenAITextVerbosity {
        self.verbosity
    }

    #[must_use]
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
/// - `OpenAI`: Automatic server-side prefix caching (hints ignored)
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
    #[must_use]
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
    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    /// Get thinking budget if enabled.
    #[must_use]
    pub const fn thinking_budget(&self) -> Option<u32> {
        self.thinking_budget
    }

    /// Check if thinking is enabled.
    #[must_use]
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
    /// Tool call started - emitted when a `tool_use` content block begins.
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: Option<String>,
    },
    /// Tool call arguments delta - emitted as JSON arguments stream in.
    ToolCallDelta { id: String, arguments: String },
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
// Tool Calling Types
// ============================================================================

/// Definition of a tool that can be called by the LLM.
///
/// This follows the standard function calling schema used by Claude and `OpenAI`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool (function name).
    pub name: String,
    /// A description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// A tool call requested by the LLM.
///
/// Contains the tool ID (for matching with results), the tool name,
/// and the arguments as a JSON value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call (used to match results).
    pub id: String,
    /// The name of the tool being called.
    pub name: String,
    /// The arguments to pass to the tool, as parsed JSON.
    pub arguments: serde_json::Value,
    /// Optional thought signature for providers that require it (Gemini).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl ToolCall {
    /// Create a new tool call.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: None,
        }
    }

    /// Create a new tool call with an optional thought signature.
    pub fn new_with_thought_signature(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        thought_signature: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature,
        }
    }
}

/// The result of executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The ID of the tool call this result is for.
    pub tool_call_id: String,
    /// The name of the tool that was called (needed for Gemini's functionResponse).
    pub tool_name: String,
    /// The result content (typically a string or JSON).
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful tool result.
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error tool result.
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: error.into(),
            is_error: true,
        }
    }
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
    #[must_use]
    pub fn new(content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
        }
    }

    #[must_use]
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
    #[must_use]
    pub fn new(content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
        }
    }

    #[must_use]
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
    #[must_use]
    pub fn new(model: ModelName, content: NonEmptyString) -> Self {
        Self {
            content,
            timestamp: SystemTime::now(),
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
}

/// A complete message.
///
/// This is a real sum type (not a `Role` tag + "sometimes-meaningful" fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    /// A tool call requested by the assistant.
    ToolUse(ToolCall),
    /// The result of a tool call execution.
    ToolResult(ToolResult),
}

impl Message {
    #[must_use]
    pub fn system(content: NonEmptyString) -> Self {
        Self::System(SystemMessage::new(content))
    }

    #[must_use]
    pub fn user(content: NonEmptyString) -> Self {
        Self::User(UserMessage::new(content))
    }

    pub fn try_user(content: impl Into<String>) -> Result<Self, EmptyStringError> {
        Ok(Self::user(NonEmptyString::new(content)?))
    }

    #[must_use]
    pub fn assistant(model: ModelName, content: NonEmptyString) -> Self {
        Self::Assistant(AssistantMessage::new(model, content))
    }

    /// Create a tool use message (assistant requesting a tool call).
    #[must_use]
    pub fn tool_use(call: ToolCall) -> Self {
        Self::ToolUse(call)
    }

    /// Create a tool result message (result of executing a tool).
    #[must_use]
    pub fn tool_result(result: ToolResult) -> Self {
        Self::ToolResult(result)
    }

    #[must_use]
    pub fn role_str(&self) -> &'static str {
        match self {
            Message::System(_) => "system",
            Message::User(_) | Message::ToolResult(_) => "user",
            Message::Assistant(_) | Message::ToolUse(_) => "assistant",
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            Message::System(m) => m.content(),
            Message::User(m) => m.content(),
            Message::Assistant(m) => m.content(),
            Message::ToolUse(call) => &call.name, // Return tool name as content summary
            Message::ToolResult(result) => &result.content,
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
    #[must_use]
    pub fn new(message: Message, cache_hint: CacheHint) -> Self {
        Self {
            message,
            cache_hint,
        }
    }

    /// Create a message with no cache hint.
    #[must_use]
    pub fn plain(message: Message) -> Self {
        Self::new(message, CacheHint::None)
    }

    /// Create a message marked for caching.
    #[must_use]
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
        assert_eq!(Provider::parse("gemini"), Some(Provider::Gemini));
        assert_eq!(Provider::parse("google"), Some(Provider::Gemini));
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
    fn model_parse_validates_gemini_prefix() {
        let provider = Provider::Gemini;
        assert!(provider.parse_model("gpt-5.2").is_err());
        assert!(provider.parse_model("claude-opus-4-5-20251101").is_err());
        assert!(provider.parse_model("gemini-3-pro-preview").is_ok());
    }

    #[test]
    fn output_limits_validates_thinking_budget() {
        assert!(OutputLimits::with_thinking(4096, 512).is_err()); // too small
        assert!(OutputLimits::with_thinking(4096, 5000).is_err()); // too large
        assert!(OutputLimits::with_thinking(8192, 4096).is_ok());
    }

    // ========================================================================
    // ApiKey Tests
    // ========================================================================

    #[test]
    fn api_key_provider_claude() {
        let key = ApiKey::Claude("sk-ant-test".to_string());
        assert_eq!(key.provider(), Provider::Claude);
        assert_eq!(key.as_str(), "sk-ant-test");
    }

    #[test]
    fn api_key_provider_openai() {
        let key = ApiKey::OpenAI("sk-test-xyz".to_string());
        assert_eq!(key.provider(), Provider::OpenAI);
        assert_eq!(key.as_str(), "sk-test-xyz");
    }

    #[test]
    fn api_key_provider_gemini() {
        let key = ApiKey::Gemini("AIza-test-key".to_string());
        assert_eq!(key.provider(), Provider::Gemini);
        assert_eq!(key.as_str(), "AIza-test-key");
    }

    // ========================================================================
    // OutputLimits Tests
    // ========================================================================

    #[test]
    fn output_limits_new_no_thinking() {
        let limits = OutputLimits::new(4096);
        assert_eq!(limits.max_output_tokens(), 4096);
        assert_eq!(limits.thinking_budget(), None);
        assert!(!limits.has_thinking());
    }

    #[test]
    fn output_limits_with_valid_thinking() {
        let limits = OutputLimits::with_thinking(16384, 8192).unwrap();
        assert_eq!(limits.max_output_tokens(), 16384);
        assert_eq!(limits.thinking_budget(), Some(8192));
        assert!(limits.has_thinking());
    }

    #[test]
    fn output_limits_rejects_budget_too_small() {
        let result = OutputLimits::with_thinking(4096, 1023);
        assert!(matches!(
            result,
            Err(OutputLimitsError::ThinkingBudgetTooSmall)
        ));
    }

    #[test]
    fn output_limits_accepts_minimum_budget() {
        let limits = OutputLimits::with_thinking(4096, 1024).unwrap();
        assert_eq!(limits.thinking_budget(), Some(1024));
    }

    #[test]
    fn output_limits_rejects_budget_equal_to_max() {
        let result = OutputLimits::with_thinking(4096, 4096);
        assert!(matches!(
            result,
            Err(OutputLimitsError::ThinkingBudgetTooLarge { .. })
        ));
    }

    #[test]
    fn output_limits_rejects_budget_greater_than_max() {
        let result = OutputLimits::with_thinking(4096, 8000);
        assert!(matches!(
            result,
            Err(OutputLimitsError::ThinkingBudgetTooLarge { .. })
        ));
    }

    // ========================================================================
    // CacheHint Tests
    // ========================================================================

    #[test]
    fn cache_hint_default_is_none() {
        let hint = CacheHint::default();
        assert_eq!(hint, CacheHint::None);
    }

    #[test]
    fn cacheable_message_plain_has_no_hint() {
        let msg = Message::try_user("test").unwrap();
        let cacheable = CacheableMessage::plain(msg);
        assert_eq!(cacheable.cache_hint, CacheHint::None);
    }

    #[test]
    fn cacheable_message_cached_has_ephemeral_hint() {
        let msg = Message::try_user("test").unwrap();
        let cacheable = CacheableMessage::cached(msg);
        assert_eq!(cacheable.cache_hint, CacheHint::Ephemeral);
    }

    // ========================================================================
    // OpenAI Request Options Tests
    // ========================================================================

    #[test]
    fn openai_reasoning_effort_parse() {
        assert_eq!(
            OpenAIReasoningEffort::parse("none"),
            Some(OpenAIReasoningEffort::None)
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("low"),
            Some(OpenAIReasoningEffort::Low)
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("MEDIUM"),
            Some(OpenAIReasoningEffort::Medium)
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("High"),
            Some(OpenAIReasoningEffort::High)
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("xhigh"),
            Some(OpenAIReasoningEffort::XHigh)
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("x-high"),
            Some(OpenAIReasoningEffort::XHigh)
        );
        assert_eq!(OpenAIReasoningEffort::parse("invalid"), None);
    }

    #[test]
    fn openai_reasoning_effort_as_str() {
        assert_eq!(OpenAIReasoningEffort::None.as_str(), "none");
        assert_eq!(OpenAIReasoningEffort::Low.as_str(), "low");
        assert_eq!(OpenAIReasoningEffort::Medium.as_str(), "medium");
        assert_eq!(OpenAIReasoningEffort::High.as_str(), "high");
        assert_eq!(OpenAIReasoningEffort::XHigh.as_str(), "xhigh");
    }

    #[test]
    fn openai_text_verbosity_parse() {
        assert_eq!(
            OpenAITextVerbosity::parse("low"),
            Some(OpenAITextVerbosity::Low)
        );
        assert_eq!(
            OpenAITextVerbosity::parse("MEDIUM"),
            Some(OpenAITextVerbosity::Medium)
        );
        assert_eq!(
            OpenAITextVerbosity::parse("High"),
            Some(OpenAITextVerbosity::High)
        );
        assert_eq!(OpenAITextVerbosity::parse("invalid"), None);
    }

    #[test]
    fn openai_truncation_parse() {
        assert_eq!(
            OpenAITruncation::parse("auto"),
            Some(OpenAITruncation::Auto)
        );
        assert_eq!(
            OpenAITruncation::parse("DISABLED"),
            Some(OpenAITruncation::Disabled)
        );
        assert_eq!(OpenAITruncation::parse("invalid"), None);
    }

    #[test]
    fn openai_request_options_default() {
        let options = OpenAIRequestOptions::default();
        assert_eq!(options.reasoning_effort(), OpenAIReasoningEffort::High);
        assert_eq!(options.verbosity(), OpenAITextVerbosity::High);
        assert_eq!(options.truncation(), OpenAITruncation::Auto);
    }

    #[test]
    fn openai_request_options_custom() {
        let options = OpenAIRequestOptions::new(
            OpenAIReasoningEffort::Low,
            OpenAITextVerbosity::Medium,
            OpenAITruncation::Disabled,
        );
        assert_eq!(options.reasoning_effort(), OpenAIReasoningEffort::Low);
        assert_eq!(options.verbosity(), OpenAITextVerbosity::Medium);
        assert_eq!(options.truncation(), OpenAITruncation::Disabled);
    }

    // ========================================================================
    // StreamEvent Tests
    // ========================================================================

    #[test]
    fn stream_finish_reason_equality() {
        assert_eq!(StreamFinishReason::Done, StreamFinishReason::Done);
        assert_ne!(
            StreamFinishReason::Done,
            StreamFinishReason::Error("err".to_string())
        );
        assert_eq!(
            StreamFinishReason::Error("test".to_string()),
            StreamFinishReason::Error("test".to_string())
        );
    }

    #[test]
    fn nonempty_static_str_whitespace_is_rejected_on_conversion() {
        const WHITESPACE_ONLY: NonEmptyStaticStr = NonEmptyStaticStr::new("   ");

        assert!(
            NonEmptyString::new("   ").is_err(),
            "NonEmptyString::new should reject whitespace-only strings"
        );

        assert!(
            NonEmptyString::try_from(WHITESPACE_ONLY).is_err(),
            "NonEmptyStaticStr conversion must preserve NonEmptyString's trim invariant"
        );
    }
}

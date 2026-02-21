//! Core domain types for Forge.
//!
//! This crate provides foundational types with **no IO, no async, and minimal dependencies**.
//! All types can be safely used from any layer of the application without pulling in runtime
//! complexity.
//!
//! # Design Philosophy
//!
//! This crate follows **type-driven design** principles where invalid states are unrepresentable:
//!
//! - **Invariants at construction**: Types validate constraints when created, not when used.
//!   Once you have a value, you know it satisfies all required invariants.
//!
//! - **Provider scoping**: Types like [`ModelName`] and [`ApiKey`] carry their provider
//!   association, preventing cross-provider mixing at compile time.
//!
//! - **True sum types**: [`Message`] is a proper enum where each variant contains role-specific
//!   data, rather than a role tag with optional fields.
//!
//! # Module Overview
//!
//! | Category | Types |
//! |----------|-------|
//! | String validation | [`NonEmptyString`], [`NonEmptyStaticStr`], [`PersistableContent`], [`EmptyStringError`] |
//! | Provider/model | [`Provider`], [`PredefinedModel`], [`InternalModel`], [`ModelName`], [`ModelParseError`], [`EnumParseError`], [`ApiKey`] |
//! | OpenAI options | [`OpenAIReasoningEffort`], [`OpenAIReasoningSummary`], [`OpenAITextVerbosity`], [`OpenAITruncation`], [`OpenAIRequestOptions`] |
//! | Output config | [`OutputLimits`], [`OutputLimitsError`], [`ThinkingBudget`], [`ThinkingState`], [`CacheHint`], [`CacheBudget`], [`CacheBudgetError`] |
//! | Streaming | [`StreamEvent`], [`StreamFinishReason`], [`ApiUsage`] |
//! | Tool calling | [`ToolDefinition`], [`ToolCall`], [`ToolResult`] |
//! | Messages | [`SystemMessage`], [`UserMessage`], [`AssistantMessage`], [`ThinkingMessage`], [`ClaudeSignatureRef`], [`Message`], [`CacheableMessage`] |
//! | Plan | [`Plan`], [`Phase`], [`PlanStep`], [`PlanStepId`], [`PlanState`], [`CompletedPlan`], [`EditOp`] |
//! | Security | [`sanitize_terminal_text`], [`strip_steganographic_chars`], [`detect_mixed_script`], [`MixedScriptDetection`], [`HomoglyphWarning`] |

pub mod ids;
pub mod settings;
pub mod ui;

mod budget;
mod confusables;
mod message;
mod model;
pub mod plan;
mod proofs;
mod sanitize;
mod text;

pub use budget::{
    CacheBudget, CacheBudgetError, CacheBudgetTake, CacheHint, OutputLimits, OutputLimitsError,
    ThinkingBudget, ThinkingState,
};
pub use confusables::{HomoglyphWarning, MixedScriptDetection, detect_mixed_script};
pub use ids::{MessageId, StepId, ToolBatchId};
pub use message::{
    AssistantMessage, ClaudeSignatureRef, Message, SystemMessage, ThinkingMessage, UserMessage,
};
pub use model::{
    EnumKind, EnumParseError, InternalModel, ModelName, ModelParseError, PredefinedModel, Provider,
};
pub use plan::{
    CompletedPlan, EditOp, EditValidationError, Phase, PhaseInput, Plan, PlanState, PlanStep,
    PlanStepId, PlanStepIdError, PlanTransitionError, PlanValidationError, StepInput,
};
pub use proofs::{EmptyStringError, NonEmptyStaticStr, NonEmptyString, PersistableContent};
pub use sanitize::{
    is_steganographic_char, sanitize_path_display, sanitize_path_for_display,
    sanitize_terminal_text, strip_steganographic_chars, strip_windows_extended_prefix,
};
pub use settings::{LspConfig, ServerConfig, ServerConfigError};
pub use text::{truncate_to_fit, truncate_with_ellipsis};

/// Single point of encoding for sensitive environment variable patterns (IFA-7).
///
/// Two distinct policies share a common credential subset:
/// - **Credential patterns**: env vars whose *values* are likely secrets (used by
///   `SecretRedactor` for output redaction).
/// - **Process injection patterns**: env vars that control dynamic linker behavior
///   (dangerous in child processes, but not credential-bearing).
///
/// `ENV_SECRET_DENYLIST` is the composed union of both — used by `EnvSanitizer`
/// and LSP child process sanitization to strip vars from subprocesses.
///
/// All three constants are generated from one macro invocation so the lists
/// cannot drift independently (IFA-7.5: compose, not copy).
macro_rules! env_denylist {
    (
        credential: [$($cred:expr),* $(,)?],
        injection: [$($inj:expr),* $(,)?],
    ) => {
        /// Credential-bearing env var patterns (glob, case-insensitive).
        /// Used by `SecretRedactor` to identify values worth redacting.
        pub const ENV_CREDENTIAL_PATTERNS: &[&str] = &[$($cred),*];

        /// Process integrity patterns — not secrets, but dangerous in child envs.
        pub const ENV_INJECTION_PATTERNS: &[&str] = &[$($inj),*];

        /// Full child-process denylist: credentials ∪ injection vectors.
        /// Used by `EnvSanitizer` and LSP server child process sanitization.
        pub const ENV_SECRET_DENYLIST: &[&str] = &[$($cred,)* $($inj),*];
    };
}

env_denylist! {
    credential: [
        "*_KEY",
        "*_TOKEN",
        "*_SECRET",
        "*_PASSWORD",
        "*_CREDENTIAL*",
        "*_API_*",
        "AWS_*",
        "ANTHROPIC_*",
        "OPENAI_*",
        "GEMINI_*",
        "GOOGLE_*",
        "AZURE_*",
        "GH_*",
        "GITHUB_*",
        "NPM_*",
    ],
    injection: [
        "DYLD_*",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "BASH_ENV",
        "ENV",
        "ZDOTDIR",
        "PROMPT_COMMAND",
        "GIT_SSH_COMMAND",
        "GIT_EXEC_PATH",
        "GIT_PAGER",
        "SSH_ASKPASS",
        "NODE_OPTIONS",
        "NODE_EXTRA_CA_CERTS",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "RUBYOPT",
        "PERL5OPT",
        "PERL5LIB",
    ],
}

use serde::{Deserialize, Serialize};
use std::fmt;

use proofs::normalize_string_for_persistence;

// --- Boundary types below: secrets, wire formats, provider-specific options ---

/// Opaque wrapper for secret strings that prevents accidental disclosure.
///
/// - No `Display` impl (compile error on `format!("{}", secret)`)
/// - `Debug` is redacted
/// - The only way to access the value is via `expose_secret()`
///
/// This makes every access point explicitly visible and greppable.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Deliberately named accessor that makes secret exposure auditable.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretString(<redacted>)")
    }
}

/// API key with provider tagging. Inner values are opaque [`SecretString`]s
/// to prevent credential disclosure in logs or error messages.
///
/// Construct with the associated functions: `ApiKey::claude()`, `ApiKey::openai()`,
/// `ApiKey::gemini()`.
#[derive(Clone)]
pub enum ApiKey {
    Claude(SecretString),
    OpenAI(SecretString),
    Gemini(SecretString),
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiKey::Claude(_) => write!(f, "ApiKey::Claude(<redacted>)"),
            ApiKey::OpenAI(_) => write!(f, "ApiKey::OpenAI(<redacted>)"),
            ApiKey::Gemini(_) => write!(f, "ApiKey::Gemini(<redacted>)"),
        }
    }
}

impl ApiKey {
    #[must_use]
    pub fn claude(key: impl Into<String>) -> Self {
        Self::Claude(SecretString::new(key.into()))
    }

    #[must_use]
    pub fn openai(key: impl Into<String>) -> Self {
        Self::OpenAI(SecretString::new(key.into()))
    }

    #[must_use]
    pub fn gemini(key: impl Into<String>) -> Self {
        Self::Gemini(SecretString::new(key.into()))
    }

    #[must_use]
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
            ApiKey::Gemini(_) => Provider::Gemini,
        }
    }

    /// Access the raw key value. Named to make exposure auditable.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        match self {
            ApiKey::Claude(key) | ApiKey::OpenAI(key) | ApiKey::Gemini(key) => key.expose_secret(),
        }
    }
}

const OPENAI_REASONING_EFFORT_VALUES: &[&str] =
    &["none", "low", "medium", "high", "xhigh", "x-high"];

const OPENAI_REASONING_SUMMARY_VALUES: &[&str] = &["none", "auto", "concise", "detailed"];

const OPENAI_TEXT_VERBOSITY_VALUES: &[&str] = &["low", "medium", "high"];

const OPENAI_TRUNCATION_VALUES: &[&str] = &["auto", "disabled"];

macro_rules! impl_str_parse_enum {
    (
        $ty:ident,
        $kind:expr,
        $expected:ident,
        { $( $($pat:literal)|+ => $variant:ident ),+ $(,)? },
        { $( $variant_out:ident => $out:literal ),+ $(,)? }
    ) => {
        impl $ty {
            pub fn parse(value: &str) -> Result<Self, EnumParseError> {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(EnumParseError::new($kind, trimmed, $expected));
                }
                match trimmed.to_ascii_lowercase().as_str() {
                    $( $($pat)|+ => Ok(Self::$variant), )+
                    _ => Err(EnumParseError::new($kind, trimmed, $expected)),
                }
            }

            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant_out => $out, )+
                }
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAIReasoningEffort {
    Disabled,
    Low,
    Medium,
    #[default]
    High,
    XHigh,
}

impl_str_parse_enum!(
    OpenAIReasoningEffort,
    EnumKind::OpenAIReasoningEffort,
    OPENAI_REASONING_EFFORT_VALUES,
    {
        "none" => Disabled,
        "low" => Low,
        "medium" => Medium,
        "high" => High,
        "xhigh" | "x-high" => XHigh,
    },
    {
        Disabled => "none",
        Low => "low",
        Medium => "medium",
        High => "high",
        XHigh => "xhigh",
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAIReasoningSummary {
    #[default]
    Disabled,
    Auto,
    Concise,
    Detailed,
}

impl_str_parse_enum!(
    OpenAIReasoningSummary,
    EnumKind::OpenAIReasoningSummary,
    OPENAI_REASONING_SUMMARY_VALUES,
    {
        "none" => Disabled,
        "auto" => Auto,
        "concise" => Concise,
        "detailed" => Detailed,
    },
    {
        Disabled => "none",
        Auto => "auto",
        Concise => "concise",
        Detailed => "detailed",
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAITextVerbosity {
    Low,
    Medium,
    #[default]
    High,
}

impl_str_parse_enum!(
    OpenAITextVerbosity,
    EnumKind::OpenAITextVerbosity,
    OPENAI_TEXT_VERBOSITY_VALUES,
    {
        "low" => Low,
        "medium" => Medium,
        "high" => High,
    },
    {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAITruncation {
    #[default]
    Auto,
    Disabled,
}

impl_str_parse_enum!(
    OpenAITruncation,
    EnumKind::OpenAITruncation,
    OPENAI_TRUNCATION_VALUES,
    {
        "auto" => Auto,
        "disabled" => Disabled,
    },
    {
        Auto => "auto",
        Disabled => "disabled",
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenAIRequestOptions {
    reasoning_effort: OpenAIReasoningEffort,
    reasoning_summary: OpenAIReasoningSummary,
    verbosity: OpenAITextVerbosity,
    truncation: OpenAITruncation,
}

impl OpenAIRequestOptions {
    #[must_use]
    pub fn new(
        reasoning_effort: OpenAIReasoningEffort,
        reasoning_summary: OpenAIReasoningSummary,
        verbosity: OpenAITextVerbosity,
        truncation: OpenAITruncation,
    ) -> Self {
        Self {
            reasoning_effort,
            reasoning_summary,
            verbosity,
            truncation,
        }
    }

    #[must_use]
    pub fn reasoning_effort(self) -> OpenAIReasoningEffort {
        self.reasoning_effort
    }

    #[must_use]
    pub fn reasoning_summary(self) -> OpenAIReasoningSummary {
        self.reasoning_summary
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

// --- Replay / thinking signature types (boundary: format migration logic) ---

/// Opaque provider signature for thinking/tool-call replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThoughtSignature(String);

impl ThoughtSignature {
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn push_str(&mut self, delta: &str) {
        self.0.push_str(delta);
    }
}

impl From<String> for ThoughtSignature {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ThoughtSignature {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "signature", rename_all = "snake_case")]
pub enum ThoughtSignatureState {
    Unsigned,
    Signed(ThoughtSignature),
}

impl ThoughtSignatureState {
    #[must_use]
    pub const fn is_signed(&self) -> bool {
        matches!(self, ThoughtSignatureState::Signed(_))
    }
}

/// A single OpenAI reasoning summary entry.
///
/// OpenAI requires each replayed `input` reasoning item to include `summary`
/// as an array of these objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAIReasoningSummaryPart {
    #[serde(rename = "type")]
    part_type: NonEmptyString,
    text: NonEmptyString,
}

impl OpenAIReasoningSummaryPart {
    pub fn try_new(
        part_type: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, EmptyStringError> {
        let part_type = part_type.into();
        Ok(Self {
            part_type: NonEmptyString::new(part_type.trim().to_string())?,
            text: NonEmptyString::new(text)?,
        })
    }

    pub fn summary_text(text: impl Into<String>) -> Result<Self, EmptyStringError> {
        Self::try_new("summary_text", text)
    }

    #[must_use]
    pub fn part_type(&self) -> &str {
        self.part_type.as_str()
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_str()
    }
}

/// An OpenAI reasoning output item captured for stateless replay.
///
/// Invariant: `id` is non-empty (after trim) and `summary` is always present
/// in serialized replay payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAIReasoningItem {
    id: NonEmptyString,
    #[serde(default)]
    summary: Vec<OpenAIReasoningSummaryPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted_content: Option<String>,
}

impl OpenAIReasoningItem {
    pub fn try_new(
        id: impl Into<String>,
        summary: Vec<OpenAIReasoningSummaryPart>,
        encrypted_content: Option<String>,
    ) -> Result<Self, EmptyStringError> {
        let id = id.into();
        let encrypted_content = encrypted_content.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
        Ok(Self {
            id: NonEmptyString::new(id.trim().to_string())?,
            summary,
            encrypted_content,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    #[must_use]
    pub fn summary(&self) -> &[OpenAIReasoningSummaryPart] {
        &self.summary
    }

    #[must_use]
    pub fn encrypted_content(&self) -> Option<&str> {
        self.encrypted_content.as_deref()
    }
}

/// Provider-specific replay state for thinking blocks.
///
/// Replaces the old `ThoughtSignatureState` on `ThinkingMessage` to support
/// both Claude signed thinking replay and OpenAI encrypted reasoning replay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThinkingReplayState {
    #[default]
    Unsigned,
    ClaudeSigned {
        signature: ThoughtSignature,
    },
    #[serde(rename = "openai_reasoning")]
    OpenAIReasoning {
        items: Vec<OpenAIReasoningItem>,
    },
    /// Replay payload contained a replay discriminator but could not be decoded.
    ///
    /// This keeps malformed/unknown persisted shapes observable instead of
    /// silently collapsing to `Unsigned`.
    Unknown,
}

impl ThinkingReplayState {
    #[must_use]
    pub fn requires_persistence(&self) -> bool {
        match self {
            Self::Unsigned | Self::Unknown => false,
            Self::ClaudeSigned { .. } => true,
            Self::OpenAIReasoning { items } => !items.is_empty(),
        }
    }
}

impl<'de> Deserialize<'de> for ThinkingReplayState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        let (has_kind, has_state) = if let Some(obj) = v.as_object() {
            (obj.contains_key("kind"), obj.contains_key("state"))
        } else {
            return Ok(Self::Unknown);
        };

        if has_kind {
            #[derive(Deserialize)]
            #[serde(tag = "kind", rename_all = "snake_case")]
            enum New {
                Unsigned,
                ClaudeSigned {
                    signature: ThoughtSignature,
                },
                #[serde(rename = "openai_reasoning")]
                OpenAIReasoning {
                    items: Vec<OpenAIReasoningItem>,
                },
            }
            return Ok(match serde_json::from_value::<New>(v) {
                Ok(parsed) => match parsed {
                    New::Unsigned => Self::Unsigned,
                    New::ClaudeSigned { signature } => Self::ClaudeSigned { signature },
                    New::OpenAIReasoning { items } => Self::OpenAIReasoning { items },
                },
                Err(_) => Self::Unknown,
            });
        }

        if has_state {
            return Ok(match serde_json::from_value::<ThoughtSignatureState>(v) {
                Ok(old) => match old {
                    ThoughtSignatureState::Unsigned => Self::Unsigned,
                    ThoughtSignatureState::Signed(sig) => Self::ClaudeSigned { signature: sig },
                },
                Err(_) => Self::Unknown,
            });
        }

        Ok(Self::Unsigned)
    }
}

// --- Stream / tool / usage types (boundary: wire format) ---

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ThinkingSignature(String),
    ResponseId(String),
    OpenAIReasoningDone {
        id: String,
        summary: Vec<OpenAIReasoningSummaryPart>,
        encrypted_content: Option<String>,
    },
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: ThoughtSignatureState,
    },
    ToolCallDelta {
        id: String,
        arguments: String,
    },
    Usage(ApiUsage),
    Done,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamFinishReason {
    Done,
    Error(String),
}

/// API-reported token usage from provider responses.
///
/// Captures actual token counts from the API (e.g., Anthropic's `message_start`
/// and `message_delta` events) for accurate cost tracking and cache hit analysis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApiUsage {
    pub input_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiUsagePresence {
    Unused,
    Used,
}

impl ApiUsage {
    #[must_use]
    pub const fn non_cached_input_tokens(&self) -> u32 {
        self.input_tokens.saturating_sub(self.cache_read_tokens)
    }

    pub fn merge(&mut self, other: &ApiUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
    }

    #[must_use]
    pub const fn presence(&self) -> ApiUsagePresence {
        if self.input_tokens > 0 || self.output_tokens > 0 {
            ApiUsagePresence::Used
        } else {
            ApiUsagePresence::Unused
        }
    }

    #[must_use]
    pub fn cache_hit_percentage(&self) -> f64 {
        if self.input_tokens == 0 {
            return 0.0;
        }
        (self.cache_read_tokens as f64 / self.input_tokens as f64) * 100.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolVisibility {
    #[default]
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolProviderScope {
    #[default]
    AllProviders,
    ProviderScoped(Provider),
}

const fn tool_visibility_is_visible(value: &ToolVisibility) -> bool {
    matches!(value, ToolVisibility::Visible)
}

const fn provider_scope_is_all(value: &ToolProviderScope) -> bool {
    matches!(value, ToolProviderScope::AllProviders)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default, skip_serializing_if = "tool_visibility_is_visible")]
    pub visibility: ToolVisibility,
    #[serde(default, skip_serializing_if = "provider_scope_is_all")]
    pub provider_scope: ToolProviderScope,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            visibility: ToolVisibility::Visible,
            provider_scope: ToolProviderScope::AllProviders,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub thought_signature: ThoughtSignatureState,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: ThoughtSignatureState::Unsigned,
        }
    }

    pub fn new_signed(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        thought_signature: ThoughtSignature,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: ThoughtSignatureState::Signed(thought_signature),
        }
    }

    #[must_use]
    pub const fn signature_state(&self) -> &ThoughtSignatureState {
        &self.thought_signature
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            id: normalize_string_for_persistence(&self.id),
            name: normalize_string_for_persistence(&self.name),
            arguments: normalize_json_for_persistence(&self.arguments),
            thought_signature: self.thought_signature.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolResultOutcome {
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub outcome: ToolResultOutcome,
}

impl ToolResult {
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: content.into(),
            outcome: ToolResultOutcome::Success,
        }
    }

    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: error.into(),
            outcome: ToolResultOutcome::Error,
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> ToolResultOutcome {
        self.outcome
    }

    #[must_use]
    pub fn normalized_for_persistence(&self) -> Self {
        Self {
            tool_call_id: normalize_string_for_persistence(&self.tool_call_id),
            tool_name: normalize_string_for_persistence(&self.tool_name),
            content: normalize_string_for_persistence(&self.content),
            outcome: self.outcome,
        }
    }
}

fn normalize_json_for_persistence(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(normalize_string_for_persistence(text))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(normalize_json_for_persistence)
                .collect::<Vec<_>>(),
        ),
        serde_json::Value::Object(map) => {
            let normalized = map
                .iter()
                .map(|(key, value)| (key.clone(), normalize_json_for_persistence(value)))
                .collect::<serde_json::Map<_, _>>();
            serde_json::Value::Object(normalized)
        }
        _ => value.clone(),
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

    #[must_use]
    pub fn plain(message: Message) -> Self {
        Self::new(message, CacheHint::Standard)
    }

    #[must_use]
    pub fn cached(message: Message) -> Self {
        Self::new(message, CacheHint::Ephemeral)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{
        ApiKey, ApiUsage, ApiUsagePresence, CacheBudget, CacheBudgetTake, CacheHint,
        CacheableMessage, ENV_CREDENTIAL_PATTERNS, ENV_INJECTION_PATTERNS, ENV_SECRET_DENYLIST,
        InternalModel, Message, NonEmptyStaticStr, NonEmptyString, OpenAIReasoningEffort,
        OpenAIReasoningItem, OpenAIReasoningSummary, OpenAIReasoningSummaryPart,
        OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation, OutputLimits,
        OutputLimitsError, PersistableContent, Provider, StreamFinishReason, ThinkingBudget,
        ThinkingMessage, ThinkingReplayState, ThinkingState, ThoughtSignature, ToolCall,
        ToolResult,
    };

    #[test]
    fn non_empty_string_rejects_empty() {
        assert!(NonEmptyString::new("").is_err());
        assert!(NonEmptyString::new("   ").is_err());
        assert!(NonEmptyString::new("hello").is_ok());
    }

    #[test]
    fn persistable_content_log_spoofing_normalized() {
        let attack = "File saved\rERROR: Permission denied";
        let safe = PersistableContent::new(attack);
        assert_eq!(safe.as_str(), "File saved\nERROR: Permission denied");
    }

    #[test]
    fn persistable_content_preserves_windows_line_endings() {
        let input = "Line 1\r\nLine 2\r\nLine 3";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), input);
    }

    #[test]
    fn persistable_content_preserves_unix_line_endings() {
        let input = "Line 1\nLine 2\nLine 3";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), input);
    }

    #[test]
    fn persistable_content_normalizes_standalone_cr_at_end() {
        let input = "Line 1\r";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), "Line 1\n");
    }

    #[test]
    fn persistable_content_normalizes_multiple_standalone_cr() {
        let input = "A\rB\rC";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), "A\nB\nC");
    }

    #[test]
    fn persistable_content_mixed_line_endings() {
        let input = "Unix\nWindows\r\nOld Mac\rMore";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), "Unix\nWindows\r\nOld Mac\nMore");
    }

    #[test]
    fn persistable_content_cr_before_crlf() {
        let input = "A\r\r\nB";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), "A\n\r\nB");
    }

    #[test]
    fn persistable_content_empty_string() {
        let safe = PersistableContent::new("");
        assert!(safe.is_empty());
        assert_eq!(safe.len(), 0);
    }

    #[test]
    fn persistable_content_only_cr() {
        let safe = PersistableContent::new("\r");
        assert_eq!(safe.as_str(), "\n");
    }

    #[test]
    fn persistable_content_deref_works() {
        let safe = PersistableContent::new("hello");
        assert!(safe.starts_with("hel"));
    }

    #[test]
    fn persistable_content_into_inner() {
        let safe = PersistableContent::new("content");
        let inner: String = safe.into_inner();
        assert_eq!(inner, "content");
    }

    #[test]
    fn persistable_content_serde_roundtrip() {
        let original = PersistableContent::new("test\rcontent");
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: PersistableContent = serde_json::from_str(&json).unwrap();
        assert_eq!(original.as_str(), deserialized.as_str());
    }

    #[test]
    fn persistable_content_deserialize_normalizes_standalone_carriage_return() {
        let deserialized: PersistableContent = serde_json::from_str("\"a\\rb\"").unwrap();
        assert_eq!(deserialized.as_str(), "a\nb");
    }

    #[test]
    fn message_normalized_for_persistence_normalizes_tool_fields() {
        let call = ToolCall::new(
            "id\r1",
            "Read\rTool",
            serde_json::json!({
                "path": "src\rmain.rs",
                "nested": { "note": "line1\rline2" }
            }),
        );
        let normalized_call = Message::tool_use(call).normalized_for_persistence();
        if let Message::ToolUse(call) = normalized_call {
            assert_eq!(call.id, "id\n1");
            assert_eq!(call.name, "Read\nTool");
            assert_eq!(call.arguments["path"], "src\nmain.rs");
            assert_eq!(call.arguments["nested"]["note"], "line1\nline2");
        } else {
            panic!("expected tool use message");
        }

        let result = ToolResult::success("call\rid", "Read\rTool", "ok\rvalue");
        let normalized_result = Message::tool_result(result).normalized_for_persistence();
        if let Message::ToolResult(result) = normalized_result {
            assert_eq!(result.tool_call_id, "call\nid");
            assert_eq!(result.tool_name, "Read\nTool");
            assert_eq!(result.content, "ok\nvalue");
        } else {
            panic!("expected tool result message");
        }
    }

    #[test]
    fn provider_from_str_parses_aliases() {
        assert_eq!(Provider::parse("claude").unwrap(), Provider::Claude);
        assert_eq!(Provider::parse("Anthropic").unwrap(), Provider::Claude);
        assert_eq!(Provider::parse("openai").unwrap(), Provider::OpenAI);
        assert_eq!(Provider::parse("gpt").unwrap(), Provider::OpenAI);
        assert_eq!(Provider::parse("gemini").unwrap(), Provider::Gemini);
        assert_eq!(Provider::parse("google").unwrap(), Provider::Gemini);
        assert!(Provider::parse("unknown").is_err());
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
        assert!(provider.parse_model("claude-opus-4-6").is_ok());
        assert!(provider.parse_model("claude-opus-4-5-20251101").is_err());
    }

    #[test]
    fn model_parse_validates_gemini_prefix() {
        let provider = Provider::Gemini;
        assert!(provider.parse_model("gpt-5.2").is_err());
        assert!(provider.parse_model("claude-opus-4-6").is_err());
        assert!(provider.parse_model("gemini-3.1-pro-preview").is_ok());
    }

    #[test]
    fn internal_models_have_expected_ids_and_providers() {
        assert_eq!(
            InternalModel::ClaudeDistiller.model_id(),
            "claude-haiku-4-5"
        );
        assert_eq!(InternalModel::ClaudeDistiller.provider(), Provider::Claude);

        assert_eq!(InternalModel::OpenAIDistiller.model_id(), "gpt-5-nano");
        assert_eq!(InternalModel::OpenAIDistiller.provider(), Provider::OpenAI);

        assert_eq!(
            InternalModel::GeminiDistiller.model_id(),
            "gemini-3.1-pro-preview"
        );
        assert_eq!(InternalModel::GeminiDistiller.provider(), Provider::Gemini);

        assert_eq!(
            InternalModel::GeminiLibrarian.model_id(),
            "gemini-3-flash-preview"
        );
        assert_eq!(InternalModel::GeminiLibrarian.provider(), Provider::Gemini);
    }

    #[test]
    fn output_limits_validates_thinking_budget() {
        assert!(OutputLimits::with_thinking(4096, 512).is_err());
        assert!(OutputLimits::with_thinking(4096, 5000).is_err());
        assert!(OutputLimits::with_thinking(8192, 4096).is_ok());
    }

    #[test]
    fn api_key_provider_claude() {
        let key = ApiKey::claude("sk-ant-test");
        assert_eq!(key.provider(), Provider::Claude);
        assert_eq!(key.expose_secret(), "sk-ant-test");
    }

    #[test]
    fn api_key_provider_openai() {
        let key = ApiKey::openai("sk-test-xyz");
        assert_eq!(key.provider(), Provider::OpenAI);
        assert_eq!(key.expose_secret(), "sk-test-xyz");
    }

    #[test]
    fn api_key_provider_gemini() {
        let key = ApiKey::gemini("AIza-test-key");
        assert_eq!(key.provider(), Provider::Gemini);
        assert_eq!(key.expose_secret(), "AIza-test-key");
    }

    #[test]
    fn output_limits_new_no_thinking() {
        let limits = OutputLimits::new(4096);
        assert_eq!(limits.max_output_tokens(), 4096);
        assert_eq!(limits.thinking(), ThinkingState::Disabled);
        assert!(!limits.has_thinking());
    }

    #[test]
    fn output_limits_with_valid_thinking() {
        let limits = OutputLimits::with_thinking(16384, 8192).unwrap();
        assert_eq!(limits.max_output_tokens(), 16384);
        assert_eq!(
            limits.thinking(),
            ThinkingState::Enabled(ThinkingBudget::new(8192).unwrap())
        );
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
        assert_eq!(
            limits.thinking(),
            ThinkingState::Enabled(ThinkingBudget::new(1024).unwrap())
        );
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

    #[test]
    fn cache_hint_default_is_default() {
        let hint = CacheHint::default();
        assert_eq!(hint, CacheHint::Standard);
    }

    #[test]
    fn cacheable_message_plain_has_no_hint() {
        let msg = Message::try_user("test", SystemTime::now()).unwrap();
        let cacheable = CacheableMessage::plain(msg);
        assert_eq!(cacheable.cache_hint, CacheHint::Standard);
    }

    #[test]
    fn cacheable_message_cached_has_ephemeral_hint() {
        let msg = Message::try_user("test", SystemTime::now()).unwrap();
        let cacheable = CacheableMessage::cached(msg);
        assert_eq!(cacheable.cache_hint, CacheHint::Ephemeral);
    }

    #[test]
    fn openai_reasoning_effort_parse() {
        assert_eq!(
            OpenAIReasoningEffort::parse("none").unwrap(),
            OpenAIReasoningEffort::Disabled
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("low").unwrap(),
            OpenAIReasoningEffort::Low
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("MEDIUM").unwrap(),
            OpenAIReasoningEffort::Medium
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("High").unwrap(),
            OpenAIReasoningEffort::High
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("xhigh").unwrap(),
            OpenAIReasoningEffort::XHigh
        );
        assert_eq!(
            OpenAIReasoningEffort::parse("x-high").unwrap(),
            OpenAIReasoningEffort::XHigh
        );
        assert!(OpenAIReasoningEffort::parse("invalid").is_err());
    }

    #[test]
    fn openai_reasoning_effort_as_str() {
        assert_eq!(OpenAIReasoningEffort::Disabled.as_str(), "none");
        assert_eq!(OpenAIReasoningEffort::Low.as_str(), "low");
        assert_eq!(OpenAIReasoningEffort::Medium.as_str(), "medium");
        assert_eq!(OpenAIReasoningEffort::High.as_str(), "high");
        assert_eq!(OpenAIReasoningEffort::XHigh.as_str(), "xhigh");
    }

    #[test]
    fn openai_reasoning_summary_parse() {
        assert_eq!(
            OpenAIReasoningSummary::parse("auto").unwrap(),
            OpenAIReasoningSummary::Auto
        );
        assert_eq!(
            OpenAIReasoningSummary::parse("CONCISE").unwrap(),
            OpenAIReasoningSummary::Concise
        );
        assert_eq!(
            OpenAIReasoningSummary::parse("Detailed").unwrap(),
            OpenAIReasoningSummary::Detailed
        );
        assert_eq!(
            OpenAIReasoningSummary::parse("none").unwrap(),
            OpenAIReasoningSummary::Disabled
        );
        assert!(OpenAIReasoningSummary::parse("invalid").is_err());
    }

    #[test]
    fn openai_reasoning_summary_as_str() {
        assert_eq!(OpenAIReasoningSummary::Disabled.as_str(), "none");
        assert_eq!(OpenAIReasoningSummary::Auto.as_str(), "auto");
        assert_eq!(OpenAIReasoningSummary::Concise.as_str(), "concise");
        assert_eq!(OpenAIReasoningSummary::Detailed.as_str(), "detailed");
    }

    #[test]
    fn openai_text_verbosity_parse() {
        assert_eq!(
            OpenAITextVerbosity::parse("low").unwrap(),
            OpenAITextVerbosity::Low
        );
        assert_eq!(
            OpenAITextVerbosity::parse("MEDIUM").unwrap(),
            OpenAITextVerbosity::Medium
        );
        assert_eq!(
            OpenAITextVerbosity::parse("High").unwrap(),
            OpenAITextVerbosity::High
        );
        assert!(OpenAITextVerbosity::parse("invalid").is_err());
    }

    #[test]
    fn openai_truncation_parse() {
        assert_eq!(
            OpenAITruncation::parse("auto").unwrap(),
            OpenAITruncation::Auto
        );
        assert_eq!(
            OpenAITruncation::parse("DISABLED").unwrap(),
            OpenAITruncation::Disabled
        );
        assert!(OpenAITruncation::parse("invalid").is_err());
    }

    #[test]
    fn openai_request_options_default() {
        let options = OpenAIRequestOptions::default();
        assert_eq!(options.reasoning_effort(), OpenAIReasoningEffort::High);
        assert_eq!(
            options.reasoning_summary(),
            OpenAIReasoningSummary::Disabled
        );
        assert_eq!(options.verbosity(), OpenAITextVerbosity::High);
        assert_eq!(options.truncation(), OpenAITruncation::Auto);
    }

    #[test]
    fn openai_request_options_custom() {
        let options = OpenAIRequestOptions::new(
            OpenAIReasoningEffort::Low,
            OpenAIReasoningSummary::Concise,
            OpenAITextVerbosity::Medium,
            OpenAITruncation::Disabled,
        );
        assert_eq!(options.reasoning_effort(), OpenAIReasoningEffort::Low);
        assert_eq!(options.reasoning_summary(), OpenAIReasoningSummary::Concise);
        assert_eq!(options.verbosity(), OpenAITextVerbosity::Medium);
        assert_eq!(options.truncation(), OpenAITruncation::Disabled);
    }

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

    #[test]
    fn api_usage_default_is_zero() {
        let usage = ApiUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.presence(), ApiUsagePresence::Unused);
    }

    #[test]
    fn api_usage_non_cached_input_tokens() {
        let usage = ApiUsage {
            input_tokens: 1000,
            cache_read_tokens: 800,
            cache_creation_tokens: 0,
            output_tokens: 500,
        };
        assert_eq!(usage.non_cached_input_tokens(), 200);
    }

    #[test]
    fn api_usage_non_cached_input_tokens_saturates() {
        let usage = ApiUsage {
            input_tokens: 100,
            cache_read_tokens: 200,
            cache_creation_tokens: 0,
            output_tokens: 50,
        };
        assert_eq!(usage.non_cached_input_tokens(), 0);
    }

    #[test]
    fn api_usage_merge() {
        let mut total = ApiUsage {
            input_tokens: 1000,
            cache_read_tokens: 800,
            cache_creation_tokens: 100,
            output_tokens: 500,
        };
        let call2 = ApiUsage {
            input_tokens: 2000,
            cache_read_tokens: 1500,
            cache_creation_tokens: 200,
            output_tokens: 1000,
        };
        total.merge(&call2);
        assert_eq!(total.input_tokens, 3000);
        assert_eq!(total.cache_read_tokens, 2300);
        assert_eq!(total.cache_creation_tokens, 300);
        assert_eq!(total.output_tokens, 1500);
    }

    #[test]
    fn api_usage_presence() {
        assert_eq!(ApiUsage::default().presence(), ApiUsagePresence::Unused);
        assert!(
            ApiUsage {
                input_tokens: 1,
                ..Default::default()
            }
            .presence()
                == ApiUsagePresence::Used
        );
        assert!(
            ApiUsage {
                output_tokens: 1,
                ..Default::default()
            }
            .presence()
                == ApiUsagePresence::Used
        );
    }

    #[test]
    fn api_usage_cache_hit_percentage() {
        let usage = ApiUsage {
            input_tokens: 1000,
            cache_read_tokens: 850,
            cache_creation_tokens: 0,
            output_tokens: 0,
        };
        assert!((usage.cache_hit_percentage() - 85.0).abs() < 0.01);

        let empty = ApiUsage::default();
        assert!((empty.cache_hit_percentage() - 0.0).abs() < 0.01);
    }

    #[test]
    fn openai_reasoning_summary_part_rejects_empty_fields() {
        assert!(OpenAIReasoningSummaryPart::try_new(" ", "text").is_err());
        assert!(OpenAIReasoningSummaryPart::summary_text("   ").is_err());
    }

    #[test]
    fn openai_reasoning_item_normalizes_boundary_values() {
        let item = OpenAIReasoningItem::try_new(
            "  r_1  ",
            vec![OpenAIReasoningSummaryPart::summary_text("line 1").unwrap()],
            Some("  enc_data  ".to_string()),
        )
        .unwrap();
        assert_eq!(item.id(), "r_1");
        assert_eq!(item.encrypted_content(), Some("enc_data"));
        assert_eq!(item.summary()[0].part_type(), "summary_text");
    }

    #[test]
    fn thinking_replay_state_deserializes_new_format() {
        let json = r#"{"kind":"claude_signed","signature":"abc"}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(
            state,
            ThinkingReplayState::ClaudeSigned { signature } if signature.as_str() == "abc"
        ));

        let json =
            r#"{"kind":"openai_reasoning","items":[{"id":"r_1","encrypted_content":"enc"}]}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        match state {
            ThinkingReplayState::OpenAIReasoning { items } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id(), "r_1");
                assert_eq!(items[0].encrypted_content(), Some("enc"));
                assert!(items[0].summary().is_empty());
            }
            _ => panic!("expected OpenAIReasoning"),
        }

        let json = r#"{"kind":"unsigned"}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unsigned));
    }

    #[test]
    fn thinking_replay_state_deserializes_old_format() {
        let json = r#"{"state":"signed","signature":"abc"}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(
            state,
            ThinkingReplayState::ClaudeSigned { signature } if signature.as_str() == "abc"
        ));

        let json = r#"{"state":"unsigned"}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unsigned));
    }

    #[test]
    fn thinking_replay_state_missing_defaults_to_unsigned() {
        let json = r"{}";
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unsigned));
    }

    #[test]
    fn thinking_replay_state_invalid_discriminator_is_unknown() {
        let json = r#"{"kind":"claude_signed","signature":42}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unknown));

        let json = r#"{"state":"signed","signature":42}"#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unknown));
    }

    #[test]
    fn thinking_replay_state_non_object_is_unknown() {
        let json = r#""not an object""#;
        let state: ThinkingReplayState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, ThinkingReplayState::Unknown));
    }

    #[test]
    fn thinking_replay_state_requires_persistence() {
        assert!(!ThinkingReplayState::Unsigned.requires_persistence());
        assert!(
            ThinkingReplayState::ClaudeSigned {
                signature: ThoughtSignature::new("sig")
            }
            .requires_persistence()
        );
        assert!(
            ThinkingReplayState::OpenAIReasoning {
                items: vec![OpenAIReasoningItem::try_new("r_1", vec![], None).unwrap()]
            }
            .requires_persistence()
        );
        assert!(!ThinkingReplayState::Unknown.requires_persistence());
        assert!(!ThinkingReplayState::OpenAIReasoning { items: vec![] }.requires_persistence());
    }

    #[test]
    fn thinking_replay_state_serde_roundtrip() {
        let state = ThinkingReplayState::ClaudeSigned {
            signature: ThoughtSignature::new("sig123"),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: ThinkingReplayState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);

        let state = ThinkingReplayState::OpenAIReasoning {
            items: vec![
                OpenAIReasoningItem::try_new(
                    "r_1",
                    vec![OpenAIReasoningSummaryPart::summary_text("summary line").unwrap()],
                    Some("enc".to_string()),
                )
                .unwrap(),
            ],
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: ThinkingReplayState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn thinking_message_alias_migration() {
        let json = r#"{
            "content": "thinking text",
            "signature": {"state":"signed","signature":"abc"},
            "timestamp": {"secs_since_epoch":1700000000,"nanos_since_epoch":0},
            "provider": "Claude",
            "model": "claude-opus-4-6"
        }"#;
        let msg: ThinkingMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(
            msg.replay_state(),
            ThinkingReplayState::ClaudeSigned { signature } if signature.as_str() == "abc"
        ));
    }

    #[test]
    fn cache_budget_try_new_rejects_exceeding_max() {
        let err = CacheBudget::try_new(10).unwrap_err();
        assert_eq!(err.slots, 10);
        assert_eq!(err.max, CacheBudget::MAX);
    }

    #[test]
    fn cache_budget_try_new_accepts_valid_slots() {
        assert_eq!(CacheBudget::try_new(4).unwrap().remaining(), 4);
        assert_eq!(CacheBudget::try_new(0).unwrap().remaining(), 0);
    }

    #[test]
    fn cache_budget_take_one_decrements() {
        let b = CacheBudget::full();
        assert_eq!(b.remaining(), 4);
        let CacheBudgetTake::Remaining(b) = b.take_one() else {
            panic!("expected remaining budget");
        };
        assert_eq!(b.remaining(), 3);
        let CacheBudgetTake::Remaining(b) = b.take_one() else {
            panic!("expected remaining budget");
        };
        assert_eq!(b.remaining(), 2);
        let CacheBudgetTake::Remaining(b) = b.take_one() else {
            panic!("expected remaining budget");
        };
        assert_eq!(b.remaining(), 1);
        let CacheBudgetTake::Remaining(b) = b.take_one() else {
            panic!("expected remaining budget");
        };
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn cache_budget_exhausted_returns_explicit_outcome() {
        let b = CacheBudget::try_new(0).unwrap();
        assert!(matches!(b.take_one(), CacheBudgetTake::Exhausted));

        let CacheBudgetTake::Remaining(b) = CacheBudget::try_new(1).unwrap().take_one() else {
            panic!("expected remaining budget");
        };
        assert!(matches!(b.take_one(), CacheBudgetTake::Exhausted));
    }

    #[test]
    fn env_denylist_is_union_of_credential_and_injection() {
        let mut composed: Vec<&str> = ENV_CREDENTIAL_PATTERNS
            .iter()
            .chain(ENV_INJECTION_PATTERNS.iter())
            .copied()
            .collect();
        composed.sort_unstable();

        let mut denylist: Vec<&str> = ENV_SECRET_DENYLIST.to_vec();
        denylist.sort_unstable();

        assert_eq!(composed, denylist);
    }

    #[test]
    fn env_credential_patterns_excludes_injection_vectors() {
        for pat in ENV_CREDENTIAL_PATTERNS {
            assert!(
                !pat.starts_with("DYLD") && *pat != "LD_PRELOAD" && *pat != "LD_LIBRARY_PATH",
                "credential pattern {pat} looks like an injection vector"
            );
        }
    }
}

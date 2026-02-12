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
//! | Output config | [`OutputLimits`], [`OutputLimitsError`], [`ThinkingBudget`], [`ThinkingState`], [`CacheHint`] |
//! | Streaming | [`StreamEvent`], [`StreamFinishReason`], [`ApiUsage`] |
//! | Tool calling | [`ToolDefinition`], [`ToolCall`], [`ToolResult`] |
//! | Messages | [`SystemMessage`], [`UserMessage`], [`AssistantMessage`], [`Message`], [`CacheableMessage`] |
//! | Security | [`sanitize_terminal_text`], [`strip_steganographic_chars`], [`detect_mixed_script`], [`HomoglyphWarning`] |

mod confusables;
mod sanitize;
mod text;
pub use confusables::{HomoglyphWarning, detect_mixed_script};
pub use sanitize::{sanitize_path_display, sanitize_terminal_text, strip_steganographic_chars};
pub use text::truncate_with_ellipsis;

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
    ],
}

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::SystemTime;
use thiserror::Error;

/// This type enforces the invariant that the contained string is never empty
/// (or whitespace-only) after trimming. Validation occurs at construction time,
/// so all operations on an existing `NonEmptyString` can assume the content is valid.
///
/// # Invariants
///
/// - Content is never empty after `trim()`
/// - Whitespace-only strings are rejected
///
/// # Serde
///
/// Serializes as a plain JSON string. Deserialization validates non-emptiness
/// and fails with an error if the string is empty or whitespace-only.
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

    /// The `content` argument already satisfies the trim invariant...
    #[must_use]
    pub fn prefixed(prefix: NonEmptyStaticStr, separator: &str, content: &NonEmptyString) -> Self {
        let mut value =
            String::with_capacity(prefix.as_str().len() + separator.len() + content.as_str().len());
        value.push_str(prefix.as_str());
        value.push_str(separator);
        value.push_str(content.as_str());
        Self(value)
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

/// **Note**: This only validates non-emptiness, not whitespace...
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

/// This type enforces the invariant that standalone `\r` characters are
/// normalized to `\n`. The normalization occurs at construction time
/// (single Authority Boundary per IFA-7).
///
/// # Invariant
///
/// - No standalone `\r` exists (only `\r\n` pairs permitted)
/// - Normalization: standalone `\r` → `\n`, `\r\n` preserved
///
/// # Security
///
/// Prevents log spoofing attacks where `\r` overwrites preceding content
/// when viewed in raw terminal contexts:
///
/// ```text
/// Attack: "File saved\rERROR: Permission denied"
/// Display: "ERROR: Permission denied" (overwrites "File saved")
/// Raw view: Hidden payload visible
/// ```
///
/// By normalizing at construction, we prevent this attack vector in
/// all persisted content (history, journals, logs).
///
/// # Performance
///
/// Uses a fast-path check: if no standalone `\r` is found, no allocation
/// is performed. Only strings containing attack vectors allocate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PersistableContent(String);

impl PersistableContent {
    /// Create persistable content by normalizing line endings.
    ///
    /// This is the ONLY constructor (Authority Boundary per IFA-7).
    /// Converts standalone `\r` to `\n` while preserving `\r\n` (Windows line endings).
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        let input = input.into();
        if Self::needs_normalization(&input) {
            Self(Self::normalize(&input))
        } else {
            Self(input)
        }
    }

    fn needs_normalization(input: &str) -> bool {
        let bytes = input.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
                return true;
            }
        }
        false
    }

    fn normalize(input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    result.push('\r');
                    result.push(chars.next().unwrap());
                } else {
                    result.push('\n');
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl AsRef<str> for PersistableContent {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<PersistableContent> for String {
    fn from(value: PersistableContent) -> Self {
        value.0
    }
}

impl std::ops::Deref for PersistableContent {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::fmt::Display for PersistableContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    Claude,
    OpenAI,
    Gemini,
}

const PROVIDER_PARSE_VALUES: &[&str] = &[
    "claude",
    "anthropic",
    "openai",
    "gpt",
    "chatgpt",
    "gemini",
    "google",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    Provider,
    PredefinedModel,
    OpenAIReasoningEffort,
    OpenAIReasoningSummary,
    OpenAITextVerbosity,
    OpenAITruncation,
}

impl EnumKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EnumKind::Provider => "provider",
            EnumKind::PredefinedModel => "predefined model",
            EnumKind::OpenAIReasoningEffort => "OpenAI reasoning effort",
            EnumKind::OpenAIReasoningSummary => "OpenAI reasoning summary",
            EnumKind::OpenAITextVerbosity => "OpenAI text verbosity",
            EnumKind::OpenAITruncation => "OpenAI truncation",
        }
    }
}

impl std::fmt::Display for EnumKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {kind} value '{raw}'; expected one of: {expected:?}")]
pub struct EnumParseError {
    kind: EnumKind,
    raw: String,
    expected: &'static [&'static str],
}

impl EnumParseError {
    #[must_use]
    pub fn new(kind: EnumKind, raw: impl Into<String>, expected: &'static [&'static str]) -> Self {
        Self {
            kind,
            raw: raw.into(),
            expected,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> EnumKind {
        self.kind
    }

    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    #[must_use]
    pub const fn expected(&self) -> &'static [&'static str] {
        self.expected
    }
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
            Provider::Claude => ModelName::from_predefined(PredefinedModel::ClaudeOpus),
            Provider::OpenAI => ModelName::from_predefined(PredefinedModel::Gpt52),
            Provider::Gemini => ModelName::from_predefined(PredefinedModel::GeminiPro),
        }
    }

    /// All available models for this provider.
    #[must_use]
    pub fn available_models(&self) -> Vec<PredefinedModel> {
        PredefinedModel::all()
            .iter()
            .copied()
            .filter(|model| model.provider() == *self)
            .collect()
    }

    /// Parse a model name for this provider.
    pub fn parse_model(&self, raw: &str) -> Result<ModelName, ModelParseError> {
        ModelName::parse(*self, raw)
    }

    /// Parse provider from string.
    pub fn parse(s: &str) -> Result<Self, EnumParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(EnumParseError::new(
                EnumKind::Provider,
                trimmed,
                PROVIDER_PARSE_VALUES,
            ));
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Ok(Provider::Claude),
            "openai" | "gpt" | "chatgpt" => Ok(Provider::OpenAI),
            "gemini" | "google" => Ok(Provider::Gemini),
            _ => Err(EnumParseError::new(
                EnumKind::Provider,
                trimmed,
                PROVIDER_PARSE_VALUES,
            )),
        }
    }

    /// Infer provider from model name prefix.
    pub fn from_model_name(model: &str) -> Result<Self, EnumParseError> {
        Ok(PredefinedModel::from_model_id(model)?.provider())
    }

    /// Get all available providers.
    #[must_use]
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI, Provider::Gemini]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PredefinedModel {
    ClaudeOpus,
    ClaudeHaiku,
    Gpt52Pro,
    Gpt52,
    GeminiPro,
    GeminiFlash,
}

const CLAUDE_MODEL_IDS: &[&str] = &["claude-opus-4-6", "claude-haiku-4-5-20251001"];

const OPENAI_MODEL_IDS: &[&str] = &["gpt-5.2-pro", "gpt-5.2"];

const GEMINI_MODEL_IDS: &[&str] = &["gemini-3-pro-preview", "gemini-3-flash-preview"];

const ALL_MODEL_IDS: &[&str] = &[
    "claude-opus-4-6",
    "claude-haiku-4-5-20251001",
    "gpt-5.2-pro",
    "gpt-5.2",
    "gemini-3-pro-preview",
    "gemini-3-flash-preview",
];

fn expected_model_ids(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Claude => CLAUDE_MODEL_IDS,
        Provider::OpenAI => OPENAI_MODEL_IDS,
        Provider::Gemini => GEMINI_MODEL_IDS,
    }
}

impl PredefinedModel {
    #[must_use]
    pub const fn all() -> &'static [PredefinedModel] {
        &[
            PredefinedModel::ClaudeOpus,
            PredefinedModel::ClaudeHaiku,
            PredefinedModel::Gpt52Pro,
            PredefinedModel::Gpt52,
            PredefinedModel::GeminiPro,
            PredefinedModel::GeminiFlash,
        ]
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Anthropic Claude Opus 4.6",
            PredefinedModel::ClaudeHaiku => "Anthropic Claude Haiku 4.5",
            PredefinedModel::Gpt52Pro => "OpenAI GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "OpenAI GPT 5.2",
            PredefinedModel::GeminiPro => "Google Gemini 3 Pro",
            PredefinedModel::GeminiFlash => "Google Gemini 3 Flash",
        }
    }

    #[must_use]
    pub const fn model_name(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Opus 4.6",
            PredefinedModel::ClaudeHaiku => "Haiku 4.5",
            PredefinedModel::Gpt52Pro => "GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "GPT 5.2",
            PredefinedModel::GeminiPro => "Gemini 3 Pro",
            PredefinedModel::GeminiFlash => "Gemini 3 Flash",
        }
    }

    #[must_use]
    pub const fn firm_name(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus | PredefinedModel::ClaudeHaiku => "Anthropic",
            PredefinedModel::Gpt52 | PredefinedModel::Gpt52Pro => "OpenAI",
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => "Google",
        }
    }

    #[must_use]
    pub const fn model_id(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "claude-opus-4-6",
            PredefinedModel::ClaudeHaiku => "claude-haiku-4-5-20251001",
            PredefinedModel::Gpt52Pro => "gpt-5.2-pro",
            PredefinedModel::Gpt52 => "gpt-5.2",
            PredefinedModel::GeminiPro => "gemini-3-pro-preview",
            PredefinedModel::GeminiFlash => "gemini-3-flash-preview",
        }
    }

    #[must_use]
    pub const fn provider(self) -> Provider {
        match self {
            PredefinedModel::ClaudeOpus | PredefinedModel::ClaudeHaiku => Provider::Claude,
            PredefinedModel::Gpt52 | PredefinedModel::Gpt52Pro => Provider::OpenAI,
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => Provider::Gemini,
        }
    }

    #[must_use]
    pub fn to_model_name(self) -> ModelName {
        ModelName::from_predefined(self)
    }

    pub fn from_model_id(raw: &str) -> Result<Self, EnumParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EnumParseError::new(
                EnumKind::PredefinedModel,
                trimmed,
                ALL_MODEL_IDS,
            ));
        }
        Self::all()
            .iter()
            .copied()
            .find(|model| model.model_id().eq_ignore_ascii_case(trimmed))
            .ok_or_else(|| EnumParseError::new(EnumKind::PredefinedModel, trimmed, ALL_MODEL_IDS))
    }

    pub fn from_provider_and_id(provider: Provider, raw: &str) -> Result<Self, EnumParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EnumParseError::new(
                EnumKind::PredefinedModel,
                trimmed,
                expected_model_ids(provider),
            ));
        }
        Self::all()
            .iter()
            .copied()
            .find(|model| {
                model.provider() == provider && model.model_id().eq_ignore_ascii_case(trimmed)
            })
            .ok_or_else(|| {
                EnumParseError::new(
                    EnumKind::PredefinedModel,
                    trimmed,
                    expected_model_ids(provider),
                )
            })
    }
}

/// Internal, system-owned model IDs used by background workflows.
///
/// Unlike [`PredefinedModel`], these are not user-selectable UI models. They
/// represent bounded internal choices used by distillation and librarian tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InternalModel {
    ClaudeDistiller,
    OpenAIDistiller,
    GeminiDistiller,
    GeminiLibrarian,
}

impl InternalModel {
    #[must_use]
    pub const fn model_id(self) -> &'static str {
        match self {
            InternalModel::ClaudeDistiller => "claude-haiku-4-5",
            InternalModel::OpenAIDistiller => "gpt-5-nano",
            InternalModel::GeminiDistiller => "gemini-3-pro-preview",
            InternalModel::GeminiLibrarian => "gemini-3-flash-preview",
        }
    }

    #[must_use]
    pub const fn provider(self) -> Provider {
        match self {
            InternalModel::ClaudeDistiller => Provider::Claude,
            InternalModel::OpenAIDistiller => Provider::OpenAI,
            InternalModel::GeminiDistiller | InternalModel::GeminiLibrarian => Provider::Gemini,
        }
    }
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
    #[error("unknown model name {0}")]
    UnknownModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelName {
    provider: Provider,
    #[serde(rename = "model")]
    name: Cow<'static, str>,
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

        let model = PredefinedModel::from_provider_and_id(provider, trimmed)
            .map_err(|_| ModelParseError::UnknownModel(trimmed.to_string()))?;

        Ok(Self::from_predefined(model))
    }

    #[must_use]
    pub const fn from_predefined(model: PredefinedModel) -> Self {
        Self {
            provider: model.provider(),
            name: Cow::Borrowed(model.model_id()),
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
    pub fn predefined(&self) -> PredefinedModel {
        PredefinedModel::from_provider_and_id(self.provider, self.as_str())
            .expect("ModelName should always be constructed from a known model")
    }
}

impl std::fmt::Display for ModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

impl<'de> Deserialize<'de> for ModelName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawModelName {
            provider: Provider,
            #[serde(rename = "model")]
            model: String,
        }

        let raw = RawModelName::deserialize(deserializer)?;
        ModelName::parse(raw.provider, &raw.model).map_err(serde::de::Error::custom)
    }
}

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

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    /// Construct a Claude API key.
    #[must_use]
    pub fn claude(key: impl Into<String>) -> Self {
        Self::Claude(SecretString::new(key.into()))
    }

    /// Construct an OpenAI API key.
    #[must_use]
    pub fn openai(key: impl Into<String>) -> Self {
        Self::OpenAI(SecretString::new(key.into()))
    }

    /// Construct a Gemini API key.
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

/// Hint for whether content should be cached by the provider.
///
/// Different providers handle caching differently:
/// - Claude: Explicit `cache_control: { type: "ephemeral" }` markers
/// - `OpenAI`: Automatic server-side prefix caching (hints ignored)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheHint {
    /// No caching preference - provider uses default behavior.
    #[default]
    Default,
    /// Content is stable and should be cached if supported.
    ///
    /// Named "Ephemeral" to match Anthropic's API terminology. Despite the name,
    /// this actually means "cache this content" - Anthropic uses "ephemeral" to
    /// indicate the cache entry has a limited TTL (~5 min) rather than permanent
    /// storage. The content itself should be stable/unchanging for caching to help.
    Ephemeral,
}

/// Cache slot budget for a Claude API request.
///
/// Claude allows at most 4 `cache_control` blocks per request. This type
/// makes >4 unrepresentable by construction (IFA §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudget(u8);

impl CacheBudget {
    pub const MAX: u8 = 4;

    /// Construct a budget, clamping to `MAX`.
    #[must_use]
    pub fn new(slots: u8) -> Self {
        Self(slots.min(Self::MAX))
    }

    #[must_use]
    pub fn full() -> Self {
        Self(Self::MAX)
    }

    #[must_use]
    pub fn remaining(self) -> u8 {
        self.0
    }

    /// Consume one slot. Returns the decremented budget, or `None` if exhausted.
    #[must_use]
    pub fn take_one(self) -> Option<CacheBudget> {
        if self.0 > 0 {
            Some(Self(self.0 - 1))
        } else {
            None
        }
    }
}

/// Error when trying to construct invalid output limits.
#[derive(Debug, Clone, Error)]
pub enum OutputLimitsError {
    #[error("thinking budget ({budget}) must be less than max output tokens ({max_output})")]
    ThinkingBudgetTooLarge { budget: u32, max_output: u32 },
    #[error("thinking budget must be at least 1024 tokens")]
    ThinkingBudgetTooSmall,
}

/// Validated thinking budget for extended reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinkingBudget(u32);

impl ThinkingBudget {
    pub const MIN_TOKENS: u32 = 1024;

    pub fn new(value: u32) -> Result<Self, OutputLimitsError> {
        if value < Self::MIN_TOKENS {
            return Err(OutputLimitsError::ThinkingBudgetTooSmall);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingState {
    Disabled,
    Enabled(ThinkingBudget),
}

impl ThinkingState {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, ThinkingState::Enabled(_))
    }
}

/// Validated output configuration that guarantees invariants.
///
/// If thinking is enabled, `thinking_budget < max_output_tokens` is guaranteed
/// by construction. You cannot create an invalid `OutputLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLimits {
    Standard {
        max_output_tokens: u32,
    },
    WithThinking {
        max_output_tokens: u32,
        thinking_budget: ThinkingBudget,
    },
}

impl OutputLimits {
    /// Create output limits without thinking.
    #[must_use]
    pub const fn new(max_output_tokens: u32) -> Self {
        Self::Standard { max_output_tokens }
    }

    /// Create output limits with thinking enabled.
    ///
    /// Returns an error if `thinking_budget >= max_output_tokens` or `thinking_budget < 1024`.
    pub fn with_thinking(
        max_output_tokens: u32,
        thinking_budget: u32,
    ) -> Result<Self, OutputLimitsError> {
        let budget = ThinkingBudget::new(thinking_budget)?;
        if budget.as_u32() >= max_output_tokens {
            return Err(OutputLimitsError::ThinkingBudgetTooLarge {
                budget: budget.as_u32(),
                max_output: max_output_tokens,
            });
        }
        Ok(Self::WithThinking {
            max_output_tokens,
            thinking_budget: budget,
        })
    }

    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        match self {
            OutputLimits::Standard { max_output_tokens }
            | OutputLimits::WithThinking {
                max_output_tokens, ..
            } => *max_output_tokens,
        }
    }

    #[must_use]
    pub const fn thinking(&self) -> ThinkingState {
        match self {
            OutputLimits::Standard { .. } => ThinkingState::Disabled,
            OutputLimits::WithThinking {
                thinking_budget, ..
            } => ThinkingState::Enabled(*thinking_budget),
        }
    }

    #[must_use]
    pub const fn has_thinking(&self) -> bool {
        matches!(self, OutputLimits::WithThinking { .. })
    }
}

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

/// An OpenAI reasoning output item captured for stateless replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAIReasoningItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
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

        // New format: discriminated by "kind"
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

        // Old format: discriminated by "state"
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

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta.
    TextDelta(String),
    /// Provider reasoning content delta (Claude extended thinking or OpenAI reasoning summaries).
    ThinkingDelta(String),
    /// Encrypted thinking signature for API replay (Claude extended thinking).
    ThinkingSignature(String),
    /// Completed OpenAI reasoning output item for stateless replay.
    OpenAIReasoningDone {
        id: String,
        encrypted_content: Option<String>,
    },
    /// Tool call started - emitted when a `tool_use` content block begins.
    ToolCallStart {
        id: String,
        name: String,
        thought_signature: ThoughtSignatureState,
    },
    /// Tool call arguments delta - emitted as JSON arguments stream in.
    ToolCallDelta {
        id: String,
        arguments: String,
    },
    /// API-reported token usage (from `message_start` or `message_delta` events).
    Usage(ApiUsage),
    Done,
    Error(String),
}

/// Reason a stream finished.
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
    /// Total input tokens (includes cached tokens).
    pub input_tokens: u32,
    /// Input tokens read from cache (cache hits).
    pub cache_read_tokens: u32,
    /// Input tokens written to cache (cache misses that were cached).
    pub cache_creation_tokens: u32,
    /// Output tokens generated by the model.
    pub output_tokens: u32,
}

impl ApiUsage {
    /// Input tokens that were not read from cache.
    ///
    /// For cost calculation:
    /// `cost = (non_cached_input * input_price) + (cache_read * cached_price) + (output * output_price)`
    #[must_use]
    pub const fn non_cached_input_tokens(&self) -> u32 {
        self.input_tokens.saturating_sub(self.cache_read_tokens)
    }

    /// Merge another usage into this one (for aggregation across multiple API calls).
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
    pub const fn has_data(&self) -> bool {
        self.input_tokens > 0 || self.output_tokens > 0
    }

    /// Cache hit percentage (0-100).
    #[must_use]
    pub fn cache_hit_percentage(&self) -> f64 {
        if self.input_tokens == 0 {
            return 0.0;
        }
        (self.cache_read_tokens as f64 / self.input_tokens as f64) * 100.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool (function name).
    pub name: String,
    /// A description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: serde_json::Value,
    /// Whether this tool is hidden from UI rendering.
    /// Hidden tools execute normally but are invisible to the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// If set, this tool is only included in the tool manifest for the specified provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
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
            hidden: false,
            provider: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call (used to match results).
    pub id: String,
    /// The name of the tool being called.
    pub name: String,
    /// The arguments to pass to the tool, as parsed JSON.
    pub arguments: serde_json::Value,
    /// Thought signature state for providers that require it (Gemini).
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
}

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

/// Provider reasoning/thinking content (Claude extended thinking, Gemini thinking, etc.).
///
/// This is separate from `AssistantMessage` because thinking is metadata about the
/// reasoning process, not part of the actual response. It can be shown/hidden
/// independently in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingMessage {
    content: NonEmptyString,
    /// Provider-specific replay state for thinking blocks.
    #[serde(default, alias = "signature")]
    replay: ThinkingReplayState,
    timestamp: SystemTime,
    #[serde(flatten)]
    model: ModelName,
}

impl ThinkingMessage {
    #[must_use]
    pub fn new(model: ModelName, content: NonEmptyString) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::Unsigned,
            timestamp: SystemTime::now(),
            model,
        }
    }

    #[must_use]
    pub fn with_signature(model: ModelName, content: NonEmptyString, signature: String) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::ClaudeSigned {
                signature: ThoughtSignature::new(signature),
            },
            timestamp: SystemTime::now(),
            model,
        }
    }

    #[must_use]
    pub fn with_openai_reasoning(
        model: ModelName,
        content: NonEmptyString,
        items: Vec<OpenAIReasoningItem>,
    ) -> Self {
        Self {
            content,
            replay: ThinkingReplayState::OpenAIReasoning { items },
            timestamp: SystemTime::now(),
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
}

/// A complete message.
///
/// This is a real sum type (not a `Role` tag + "sometimes-meaningful" fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    /// Provider reasoning/thinking content.
    Thinking(ThinkingMessage),
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

    #[must_use]
    pub fn thinking(model: ModelName, content: NonEmptyString) -> Self {
        Self::Thinking(ThinkingMessage::new(model, content))
    }

    #[must_use]
    pub fn thinking_with_signature(
        model: ModelName,
        content: NonEmptyString,
        signature: String,
    ) -> Self {
        Self::Thinking(ThinkingMessage::with_signature(model, content, signature))
    }

    #[must_use]
    pub fn thinking_with_openai_reasoning(
        model: ModelName,
        content: NonEmptyString,
        items: Vec<OpenAIReasoningItem>,
    ) -> Self {
        Self::Thinking(ThinkingMessage::with_openai_reasoning(
            model, content, items,
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
            // Thinking is assistant-role content (internal reasoning)
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
        Self::new(message, CacheHint::Default)
    }

    #[must_use]
    pub fn cached(message: Message) -> Self {
        Self::new(message, CacheHint::Ephemeral)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_string_rejects_empty() {
        assert!(NonEmptyString::new("").is_err());
        assert!(NonEmptyString::new("   ").is_err());
        assert!(NonEmptyString::new("hello").is_ok());
    }

    // --- PersistableContent tests ---

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
        // Unix \n, Windows \r\n, and old Mac \r all in one string
        let input = "Unix\nWindows\r\nOld Mac\rMore";
        let safe = PersistableContent::new(input);
        assert_eq!(safe.as_str(), "Unix\nWindows\r\nOld Mac\nMore");
    }

    #[test]
    fn persistable_content_cr_before_crlf() {
        // \r followed by \r\n should normalize the first \r
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
        // Note: deserialization re-normalizes (transparent serde)
        assert_eq!(original.as_str(), deserialized.as_str());
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
        assert!(provider.parse_model("gemini-3-pro-preview").is_ok());
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
            "gemini-3-pro-preview"
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
        assert_eq!(hint, CacheHint::Default);
    }

    #[test]
    fn cacheable_message_plain_has_no_hint() {
        let msg = Message::try_user("test").unwrap();
        let cacheable = CacheableMessage::plain(msg);
        assert_eq!(cacheable.cache_hint, CacheHint::Default);
    }

    #[test]
    fn cacheable_message_cached_has_ephemeral_hint() {
        let msg = Message::try_user("test").unwrap();
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
        assert!(!usage.has_data());
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
        // Edge case: cache_read > input (shouldn't happen but handle gracefully)
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
    fn api_usage_has_data() {
        assert!(!ApiUsage::default().has_data());
        assert!(
            ApiUsage {
                input_tokens: 1,
                ..Default::default()
            }
            .has_data()
        );
        assert!(
            ApiUsage {
                output_tokens: 1,
                ..Default::default()
            }
            .has_data()
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

        // Zero input tokens should return 0%
        let empty = ApiUsage::default();
        assert!((empty.cache_hit_percentage() - 0.0).abs() < 0.01);
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
                assert_eq!(items[0].id, "r_1");
                assert_eq!(items[0].encrypted_content.as_deref(), Some("enc"));
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
                items: vec![OpenAIReasoningItem {
                    id: "r_1".to_string(),
                    encrypted_content: None,
                }]
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
            items: vec![OpenAIReasoningItem {
                id: "r_1".to_string(),
                encrypted_content: Some("enc".to_string()),
            }],
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
    fn cache_budget_clamps_at_max() {
        assert_eq!(CacheBudget::new(10).remaining(), CacheBudget::MAX);
        assert_eq!(CacheBudget::new(4).remaining(), 4);
        assert_eq!(CacheBudget::new(0).remaining(), 0);
    }

    #[test]
    fn cache_budget_take_one_decrements() {
        let b = CacheBudget::full();
        assert_eq!(b.remaining(), 4);
        let b = b.take_one().unwrap();
        assert_eq!(b.remaining(), 3);
        let b = b.take_one().unwrap();
        assert_eq!(b.remaining(), 2);
        let b = b.take_one().unwrap();
        assert_eq!(b.remaining(), 1);
        let b = b.take_one().unwrap();
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn cache_budget_exhausted_returns_none() {
        let b = CacheBudget::new(0);
        assert!(b.take_one().is_none());

        let b = CacheBudget::new(1).take_one().unwrap();
        assert!(b.take_one().is_none());
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

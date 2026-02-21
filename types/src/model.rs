//! Core model domain types and provider enumeration.

use std::borrow::Cow;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

impl fmt::Display for EnumKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

    #[must_use]
    pub fn available_models(&self) -> Vec<PredefinedModel> {
        PredefinedModel::all()
            .iter()
            .copied()
            .filter(|model| model.provider() == *self)
            .collect()
    }

    pub fn parse_model(&self, raw: &str) -> Result<ModelName, ModelParseError> {
        ModelName::parse(*self, raw)
    }

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

    pub fn from_model_name(model: &str) -> Result<Self, EnumParseError> {
        Ok(PredefinedModel::from_model_id(model)?.provider())
    }

    #[must_use]
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI, Provider::Gemini]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PredefinedModel {
    ClaudeOpus,
    ClaudeSonnet,
    ClaudeHaiku,
    Gpt52Pro,
    Gpt52,
    GeminiPro,
    GeminiFlash,
}

const CLAUDE_MODEL_IDS: &[&str] = &[
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
];

const OPENAI_MODEL_IDS: &[&str] = &["gpt-5.2-pro", "gpt-5.2"];

const GEMINI_MODEL_IDS: &[&str] = &["gemini-3.1-pro-preview", "gemini-3.1-flash-preview"];

const ALL_MODEL_IDS: &[&str] = &[
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "gpt-5.2-pro",
    "gpt-5.2",
    "gemini-3.1-pro-preview",
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
            PredefinedModel::ClaudeSonnet,
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
            PredefinedModel::ClaudeSonnet => "Anthropic Claude Sonnet 4.6",
            PredefinedModel::ClaudeHaiku => "Anthropic Claude Haiku 4.5",
            PredefinedModel::Gpt52Pro => "OpenAI GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "OpenAI GPT 5.2",
            PredefinedModel::GeminiPro => "Google Gemini 3.1 Pro",
            PredefinedModel::GeminiFlash => "Google Gemini 3 Flash",
        }
    }

    #[must_use]
    pub const fn model_name(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Claude Opus 4.6",
            PredefinedModel::ClaudeSonnet => "Claude Sonnet 4.6",
            PredefinedModel::ClaudeHaiku => "Claude Haiku 4.5",
            PredefinedModel::Gpt52Pro => "GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "GPT 5.2",
            PredefinedModel::GeminiPro => "Gemini 3.1 Pro",
            PredefinedModel::GeminiFlash => "Gemini 3 Flash",
        }
    }

    #[must_use]
    pub const fn firm_name(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus
            | PredefinedModel::ClaudeSonnet
            | PredefinedModel::ClaudeHaiku => "Anthropic",
            PredefinedModel::Gpt52 | PredefinedModel::Gpt52Pro => "OpenAI",
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => "Google",
        }
    }

    #[must_use]
    pub const fn model_id(self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "claude-opus-4-6",
            PredefinedModel::ClaudeSonnet => "claude-sonnet-4-6",
            PredefinedModel::ClaudeHaiku => "claude-haiku-4-5-20251001",
            PredefinedModel::Gpt52Pro => "gpt-5.2-pro",
            PredefinedModel::Gpt52 => "gpt-5.2",
            PredefinedModel::GeminiPro => "gemini-3.1-pro-preview",
            PredefinedModel::GeminiFlash => "gemini-3-flash-preview",
        }
    }

    #[must_use]
    pub const fn provider(self) -> Provider {
        match self {
            PredefinedModel::ClaudeOpus
            | PredefinedModel::ClaudeSonnet
            | PredefinedModel::ClaudeHaiku => Provider::Claude,
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
            InternalModel::GeminiDistiller => "gemini-3.1-pro-preview",
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
    EmptyInput,
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
            return Err(ModelParseError::EmptyInput);
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

    /// Short human-readable name for UI display (e.g. "Claude Sonnet 4.6").
    #[must_use]
    pub fn short_display_name(&self) -> &'static str {
        self.predefined().model_name()
    }
}

impl fmt::Display for ModelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        ModelName::parse(raw.provider, &raw.model).map_err(D::Error::custom)
    }
}

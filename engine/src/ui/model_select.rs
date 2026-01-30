//! Predefined model options for the model selector.

use forge_types::{ModelName, Provider};

/// Predefined model options for the model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredefinedModel {
    ClaudeOpus,
    ClaudeSonnet,
    ClaudeHaiku,
    Gpt52Pro,
    Gpt52,
    GeminiPro,
    GeminiFlash,
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
    pub const fn display_name(&self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Anthropic Claude Opus 4.5",
            PredefinedModel::ClaudeSonnet => "Anthropic Claude Sonnet 4.5",
            PredefinedModel::ClaudeHaiku => "Anthropic Claude Haiku 4.5",
            PredefinedModel::Gpt52Pro => "OpenAI GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "OpenAI GPT 5.2",
            PredefinedModel::GeminiPro => "Google Gemini 3 Pro",
            PredefinedModel::GeminiFlash => "Google Gemini 3 Flash",
        }
    }

    /// Short model name without provider prefix (e.g., "Opus 4.5").
    #[must_use]
    pub const fn model_name(&self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Opus 4.5",
            PredefinedModel::ClaudeSonnet => "Sonnet 4.5",
            PredefinedModel::ClaudeHaiku => "Haiku 4.5",
            PredefinedModel::Gpt52Pro => "GPT 5.2 Pro",
            PredefinedModel::Gpt52 => "GPT 5.2",
            PredefinedModel::GeminiPro => "Gemini 3 Pro",
            PredefinedModel::GeminiFlash => "Gemini 3 Flash",
        }
    }

    /// Provider company name (e.g., "Anthropic", "OpenAI", "Google").
    #[must_use]
    pub const fn firm_name(&self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus
            | PredefinedModel::ClaudeSonnet
            | PredefinedModel::ClaudeHaiku => "Anthropic",
            PredefinedModel::Gpt52 | PredefinedModel::Gpt52Pro => "OpenAI",
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => "Google",
        }
    }

    #[must_use]
    pub fn to_model_name(&self) -> ModelName {
        match self {
            PredefinedModel::ClaudeOpus => {
                ModelName::known(Provider::Claude, "claude-opus-4-5-20251101")
            }
            PredefinedModel::ClaudeSonnet => {
                ModelName::known(Provider::Claude, "claude-sonnet-4-5-20250514")
            }
            PredefinedModel::ClaudeHaiku => {
                ModelName::known(Provider::Claude, "claude-haiku-4-5-20251001")
            }
            PredefinedModel::Gpt52Pro => ModelName::known(Provider::OpenAI, "gpt-5.2-pro"),
            PredefinedModel::Gpt52 => ModelName::known(Provider::OpenAI, "gpt-5.2"),
            PredefinedModel::GeminiPro => {
                ModelName::known(Provider::Gemini, "gemini-3-pro-preview")
            }
            PredefinedModel::GeminiFlash => {
                ModelName::known(Provider::Gemini, "gemini-3-flash-preview")
            }
        }
    }

    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            PredefinedModel::ClaudeOpus
            | PredefinedModel::ClaudeSonnet
            | PredefinedModel::ClaudeHaiku => Provider::Claude,
            PredefinedModel::Gpt52 | PredefinedModel::Gpt52Pro => Provider::OpenAI,
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => Provider::Gemini,
        }
    }
}

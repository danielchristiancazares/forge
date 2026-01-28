//! Predefined model options for the model selector.

use forge_types::{ModelName, Provider};

/// Predefined model options for the model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredefinedModel {
    ClaudeOpus,
    Gpt52,
    GeminiPro,
    GeminiFlash,
}

impl PredefinedModel {
    #[must_use]
    pub const fn all() -> &'static [PredefinedModel] {
        &[
            PredefinedModel::ClaudeOpus,
            PredefinedModel::Gpt52,
            PredefinedModel::GeminiPro,
            PredefinedModel::GeminiFlash,
        ]
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            PredefinedModel::ClaudeOpus => "Anthropic Claude Opus 4.5",
            PredefinedModel::Gpt52 => "OpenAI GPT 5.2",
            PredefinedModel::GeminiPro => "Google Gemini 3 Pro",
            PredefinedModel::GeminiFlash => "Google Gemini 3 Flash",
        }
    }

    #[must_use]
    pub fn to_model_name(&self) -> ModelName {
        match self {
            PredefinedModel::ClaudeOpus => {
                ModelName::known(Provider::Claude, "claude-opus-4-5-20251101")
            }
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
            PredefinedModel::ClaudeOpus => Provider::Claude,
            PredefinedModel::Gpt52 => Provider::OpenAI,
            PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => Provider::Gemini,
        }
    }
}

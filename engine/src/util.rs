//! Small pure helper functions.

use forge_types::{ApiKey, ModelName, PredefinedModel, Provider};

pub use forge_types::truncate_with_ellipsis;

/// Wrap a raw API key string in the provider-specific `ApiKey` enum variant.
#[inline]
pub fn wrap_api_key(provider: Provider, raw: String) -> ApiKey {
    match provider {
        Provider::Claude => ApiKey::claude(raw),
        Provider::OpenAI => ApiKey::openai(raw),
        Provider::Gemini => ApiKey::gemini(raw),
    }
}

/// Parse a model name string back into a `ModelName`.
///
/// Used for crash recovery when we need to reconstruct the model from stored metadata.
/// Falls back to None if the model name cannot be parsed.
pub fn parse_model_name_from_string(name: &str) -> Option<ModelName> {
    PredefinedModel::from_model_id(name)
        .map(ModelName::from_predefined)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_name_claude() {
        let model = parse_model_name_from_string("claude-opus-4-6");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider(), Provider::Claude);
    }

    #[test]
    fn parse_model_name_openai() {
        let model = parse_model_name_from_string("gpt-5.2");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider(), Provider::OpenAI);
    }

    #[test]
    fn parse_model_name_gemini() {
        let model = parse_model_name_from_string("gemini-3-pro-preview");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider(), Provider::Gemini);
    }

    #[test]
    fn parse_model_name_gemini_with_date() {
        let model = parse_model_name_from_string("gemini-2.5-pro-preview-05-06");
        assert!(model.is_none());
    }

    #[test]
    fn parse_model_name_empty_returns_none() {
        assert!(parse_model_name_from_string("").is_none());
        assert!(parse_model_name_from_string("  ").is_none());
    }

    #[test]
    fn parse_model_name_unknown_provider_returns_none() {
        assert!(parse_model_name_from_string("unknown-model").is_none());
    }
}

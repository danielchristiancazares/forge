//! Small pure helper functions.

use forge_types::{ApiKey, ModelName, PredefinedModel, Provider, SecretString};

/// Deliberately exposes the secret at the boundary where it enters the provider API.
#[inline]
#[must_use]
pub fn wrap_api_key(provider: Provider, secret: SecretString) -> ApiKey {
    let raw = secret.expose_secret().to_string();
    match provider {
        Provider::Claude => ApiKey::claude(raw),
        Provider::OpenAI => ApiKey::openai(raw),
        Provider::Gemini => ApiKey::gemini(raw),
    }
}

/// Used for crash recovery when we need to reconstruct the model from stored metadata.
/// Falls back to None if the model name cannot be parsed.
pub fn parse_model_name_from_string(name: &str) -> Option<ModelName> {
    PredefinedModel::from_model_id(name)
        .map(ModelName::from_predefined)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{Provider, parse_model_name_from_string};

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
        let model = parse_model_name_from_string("gemini-3.1-pro-preview");
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

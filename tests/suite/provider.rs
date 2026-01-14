//! Provider and API configuration tests

use forge_engine::{ApiConfig, ApiKey};
use forge_types::{ModelName, ModelNameKind, Provider};

#[test]
fn provider_parse_aliases() {
    // Claude aliases
    assert_eq!(Provider::parse("claude"), Some(Provider::Claude));
    assert_eq!(Provider::parse("Claude"), Some(Provider::Claude));
    assert_eq!(Provider::parse("CLAUDE"), Some(Provider::Claude));
    assert_eq!(Provider::parse("anthropic"), Some(Provider::Claude));

    // OpenAI aliases
    assert_eq!(Provider::parse("openai"), Some(Provider::OpenAI));
    assert_eq!(Provider::parse("gpt"), Some(Provider::OpenAI));
    assert_eq!(Provider::parse("chatgpt"), Some(Provider::OpenAI));

    // Unknown
    assert_eq!(Provider::parse("gemini"), None);
    assert_eq!(Provider::parse(""), None);
}

#[test]
fn provider_all_returns_both() {
    let all = Provider::all();
    assert!(all.contains(&Provider::Claude));
    assert!(all.contains(&Provider::OpenAI));
}

#[test]
fn provider_env_vars() {
    assert_eq!(Provider::Claude.env_var(), "ANTHROPIC_API_KEY");
    assert_eq!(Provider::OpenAI.env_var(), "OPENAI_API_KEY");
}

#[test]
fn model_name_parse_known() {
    let model = ModelName::parse(Provider::Claude, "claude-sonnet-4-5-20250929").unwrap();
    assert_eq!(model.kind(), ModelNameKind::Known);
    assert_eq!(model.provider(), Provider::Claude);
}

#[test]
fn model_name_parse_known_case_insensitive() {
    let model = ModelName::parse(Provider::OpenAI, "GPT-5.2").unwrap();
    assert_eq!(model.kind(), ModelNameKind::Known);
    assert_eq!(model.as_str(), "gpt-5.2"); // Normalized to known spelling
}

#[test]
fn model_name_parse_unknown() {
    let model = ModelName::parse(Provider::Claude, "claude-future-model").unwrap();
    assert_eq!(model.kind(), ModelNameKind::Unverified);
    assert_eq!(model.as_str(), "claude-future-model");
}

#[test]
fn model_name_parse_empty_fails() {
    let result = ModelName::parse(Provider::Claude, "");
    assert!(result.is_err());

    let result = ModelName::parse(Provider::Claude, "   ");
    assert!(result.is_err());
}

#[test]
fn model_name_known_constructor() {
    let model = ModelName::known(Provider::OpenAI, "gpt-4o");
    assert_eq!(model.kind(), ModelNameKind::Known);
    assert_eq!(model.provider(), Provider::OpenAI);
}

#[test]
fn api_key_provider_association() {
    let claude_key = ApiKey::Claude("sk-ant-test".to_string());
    let openai_key = ApiKey::OpenAI("sk-test".to_string());

    assert_eq!(claude_key.provider(), Provider::Claude);
    assert_eq!(openai_key.provider(), Provider::OpenAI);
}

#[test]
fn api_config_validates_provider_match() {
    let key = ApiKey::Claude("test".to_string());
    let model = Provider::Claude.default_model();

    let config = ApiConfig::new(key, model);
    assert!(config.is_ok());
}

#[test]
fn api_config_rejects_provider_mismatch() {
    let key = ApiKey::Claude("test".to_string());
    let model = Provider::OpenAI.default_model();

    let config = ApiConfig::new(key, model);
    assert!(config.is_err());
}

#[test]
fn api_config_accessors() {
    let key = ApiKey::OpenAI("sk-secret".to_string());
    let model = ModelName::known(Provider::OpenAI, "gpt-4o");

    let config = ApiConfig::new(key, model).unwrap();

    assert_eq!(config.provider(), Provider::OpenAI);
    assert_eq!(config.api_key(), "sk-secret");
    assert_eq!(config.model().as_str(), "gpt-4o");
}

#[test]
fn provider_default_models_exist() {
    for provider in Provider::all() {
        let model = provider.default_model();
        assert!(!model.as_str().is_empty());
        assert_eq!(model.provider(), *provider);
        assert_eq!(model.kind(), ModelNameKind::Known);
    }
}

#[test]
fn provider_available_models_not_empty() {
    for provider in Provider::all() {
        let models = provider.available_models();
        assert!(!models.is_empty(), "{provider:?} has no available models");
    }
}

#[test]
fn model_name_display() {
    let model = ModelName::known(Provider::Claude, "claude-sonnet-4-5-20250929");
    assert_eq!(format!("{model}"), "claude-sonnet-4-5-20250929");
}

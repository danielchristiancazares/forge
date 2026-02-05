//! Provider and API configuration tests

use forge_engine::{ApiConfig, ApiKey};
use forge_types::{ModelName, ModelParseError, PredefinedModel, Provider};

#[test]
fn provider_parse_aliases() {
    // Claude aliases
    assert_eq!(Provider::parse("claude").unwrap(), Provider::Claude);
    assert_eq!(Provider::parse("Claude").unwrap(), Provider::Claude);
    assert_eq!(Provider::parse("CLAUDE").unwrap(), Provider::Claude);
    assert_eq!(Provider::parse("anthropic").unwrap(), Provider::Claude);

    // OpenAI aliases
    assert_eq!(Provider::parse("openai").unwrap(), Provider::OpenAI);
    assert_eq!(Provider::parse("gpt").unwrap(), Provider::OpenAI);
    assert_eq!(Provider::parse("chatgpt").unwrap(), Provider::OpenAI);

    // Gemini aliases
    assert_eq!(Provider::parse("gemini").unwrap(), Provider::Gemini);
    assert_eq!(Provider::parse("google").unwrap(), Provider::Gemini);
    assert_eq!(Provider::parse("Gemini").unwrap(), Provider::Gemini);

    // Unknown
    assert!(Provider::parse("unknown_provider").is_err());
    assert!(Provider::parse("").is_err());
}

#[test]
fn provider_all_returns_all() {
    let all = Provider::all();
    assert!(all.contains(&Provider::Claude));
    assert!(all.contains(&Provider::OpenAI));
    assert!(all.contains(&Provider::Gemini));
    assert_eq!(all.len(), 3);
}

#[test]
fn provider_env_vars() {
    assert_eq!(Provider::Claude.env_var(), "ANTHROPIC_API_KEY");
    assert_eq!(Provider::OpenAI.env_var(), "OPENAI_API_KEY");
    assert_eq!(Provider::Gemini.env_var(), "GEMINI_API_KEY");
}

#[test]
fn model_name_parse_known() {
    let model = ModelName::parse(Provider::Claude, "claude-opus-4-6").unwrap();
    assert_eq!(model.predefined(), PredefinedModel::ClaudeOpus);
    assert_eq!(model.provider(), Provider::Claude);
}

#[test]
fn model_name_parse_rejects_removed_opus_4_5() {
    assert!(ModelName::parse(Provider::Claude, "claude-opus-4-5-20251101").is_err());
}

#[test]
fn model_name_parse_known_case_insensitive() {
    let model = ModelName::parse(Provider::OpenAI, "GPT-5.2").unwrap();
    assert_eq!(model.predefined(), PredefinedModel::Gpt52);
    assert_eq!(model.as_str(), "gpt-5.2"); // Normalized to known spelling
}

#[test]
fn model_name_parse_unknown() {
    let model = ModelName::parse(Provider::Claude, "claude-future-model");
    assert!(matches!(model, Err(ModelParseError::UnknownModel(_))));
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
    let model = ModelName::from_predefined(PredefinedModel::Gpt52);
    assert_eq!(model.provider(), Provider::OpenAI);
}

#[test]
fn api_key_provider_association() {
    let claude_key = ApiKey::Claude("sk-ant-test".to_string());
    let openai_key = ApiKey::OpenAI("sk-test".to_string());
    let gemini_key = ApiKey::Gemini("AIza-test".to_string());

    assert_eq!(claude_key.provider(), Provider::Claude);
    assert_eq!(openai_key.provider(), Provider::OpenAI);
    assert_eq!(gemini_key.provider(), Provider::Gemini);
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
    let model = ModelName::from_predefined(PredefinedModel::Gpt52);

    let config = ApiConfig::new(key, model).unwrap();

    assert_eq!(config.provider(), Provider::OpenAI);
    assert_eq!(config.api_key(), "sk-secret");
    assert_eq!(config.model().as_str(), "gpt-5.2");
}

#[test]
fn provider_default_models_exist() {
    for provider in Provider::all() {
        let model = provider.default_model();
        assert!(!model.as_str().is_empty());
        assert_eq!(model.provider(), *provider);
        assert_eq!(model.predefined().provider(), *provider);
    }
}

#[test]
fn provider_available_models_not_empty() {
    for provider in Provider::all() {
        let models = provider.available_models();
        assert!(!models.is_empty(), "{provider:?} has no available models");
        assert!(models.iter().all(|model| model.provider() == *provider));
    }
}

#[test]
fn model_name_display() {
    let model = ModelName::from_predefined(PredefinedModel::ClaudeOpus);
    assert_eq!(format!("{model}"), "claude-opus-4-6");
}

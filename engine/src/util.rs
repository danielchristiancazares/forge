//! Small pure helper functions.

use forge_types::{ModelName, Provider};

/// Truncate a string to a maximum length, adding ellipsis if needed.
pub fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max - 3).collect();
        format!("{head}...")
    }
}

/// Parse a model name string back into a ModelName.
///
/// Used for crash recovery when we need to reconstruct the model from stored metadata.
/// Falls back to None if the model name cannot be parsed.
pub fn parse_model_name_from_string(name: &str) -> Option<ModelName> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Detect provider from model name prefix
    let provider = if trimmed.to_ascii_lowercase().starts_with("claude-") {
        Provider::Claude
    } else if trimmed.to_ascii_lowercase().starts_with("gpt-") {
        Provider::OpenAI
    } else {
        // Unknown provider - can't parse
        return None;
    };

    // Use the standard parse method
    ModelName::parse(provider, trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");
    }

    #[test]
    fn truncate_trims_whitespace() {
        assert_eq!(truncate_with_ellipsis("  hello  ", 10), "hello");
    }

    #[test]
    fn truncate_min_length_is_three() {
        // Even with max=1, we should get at least "..."
        assert_eq!(truncate_with_ellipsis("hello", 1), "...");
    }

    #[test]
    fn parse_model_name_claude() {
        let model = parse_model_name_from_string("claude-sonnet-4-5-20250929");
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
    fn parse_model_name_empty_returns_none() {
        assert!(parse_model_name_from_string("").is_none());
        assert!(parse_model_name_from_string("  ").is_none());
    }

    #[test]
    fn parse_model_name_unknown_provider_returns_none() {
        assert!(parse_model_name_from_string("unknown-model").is_none());
    }
}

//! Model token limits and registry.
//!
//! This module provides [`ModelLimits`] for storing token constraints per model,
//! and [`ModelRegistry`] for looking up limits by model name with prefix matching.

use std::collections::HashMap;

/// Token limits for a specific model.
///
/// Each model has a maximum context window (input tokens) and maximum output tokens.
/// The effective input budget accounts for output reservation and a safety margin.
///
/// # Example
///
/// ```
/// use forge::context_infinity::ModelLimits;
///
/// let limits = ModelLimits::new(200_000, 16_000);
/// assert_eq!(limits.context_window(), 200_000);
/// assert_eq!(limits.max_output(), 16_000);
///
/// // Effective budget = context_window - max_output - 5% safety margin
/// let budget = limits.effective_input_budget();
/// assert!(budget < 200_000 - 16_000);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    /// Maximum input context window in tokens.
    context_window: u32,
    /// Maximum output tokens the model can generate.
    max_output: u32,
}

impl ModelLimits {
    /// Creates new model limits with the given context window and max output.
    ///
    /// # Arguments
    ///
    /// * `context_window` - Maximum input context in tokens
    /// * `max_output` - Maximum output tokens
    #[must_use]
    pub const fn new(context_window: u32, max_output: u32) -> Self {
        Self {
            context_window,
            max_output,
        }
    }

    /// Returns the effective input budget.
    ///
    /// This is the maximum number of tokens available for input messages,
    /// calculated as: `context_window - max_output - 5% safety margin`.
    ///
    /// The 5% safety margin accounts for token counting inaccuracies and
    /// overhead from system prompts, formatting, and tool definitions.
    #[must_use]
    pub fn effective_input_budget(&self) -> u32 {
        let available = self.context_window.saturating_sub(self.max_output);
        // Subtract 5% safety margin
        let safety_margin = available / 20; // 5% = 1/20
        available.saturating_sub(safety_margin)
    }

    /// Returns the maximum context window size in tokens.
    #[must_use]
    pub const fn context_window(&self) -> u32 {
        self.context_window
    }

    /// Returns the maximum output tokens.
    #[must_use]
    pub const fn max_output(&self) -> u32 {
        self.max_output
    }
}

/// Where model limits came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLimitsSource {
    /// Exact match from an override.
    Override,
    /// Matched a known prefix (the matched prefix).
    Prefix(&'static str),
    /// Fell back to `DEFAULT_LIMITS` because no match was found.
    DefaultFallback,
}

/// Result of looking up model limits.
///
/// This makes the "fallback OR real data" decision explicit at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedModelLimits {
    limits: ModelLimits,
    source: ModelLimitsSource,
}

impl ResolvedModelLimits {
    #[must_use]
    pub const fn new(limits: ModelLimits, source: ModelLimitsSource) -> Self {
        Self { limits, source }
    }

    #[must_use]
    pub const fn limits(self) -> ModelLimits {
        self.limits
    }

    #[must_use]
    pub const fn source(self) -> ModelLimitsSource {
        self.source
    }
}

/// Default fallback limits for unknown models.
const DEFAULT_LIMITS: ModelLimits = ModelLimits::new(8192, 4096);

/// Known model prefixes and their limits.
///
/// Ordered by specificity (more specific prefixes first) to ensure
/// correct matching when multiple prefixes could match.
const KNOWN_MODELS: &[(&str, ModelLimits)] = &[
    // Claude models (most specific first)
    ("claude-opus-4", ModelLimits::new(200_000, 64_000)),
    ("claude-sonnet-4", ModelLimits::new(200_000, 64_000)),
    ("claude-3-5", ModelLimits::new(200_000, 64_000)),
    ("claude-3", ModelLimits::new(200_000, 64_000)),
    ("claude", ModelLimits::new(200_000, 64_000)),
    // GPT models (most specific first)
    ("gpt-4o", ModelLimits::new(128_000, 16_384)),
    ("gpt-4-turbo", ModelLimits::new(128_000, 4096)),
    ("gpt-4", ModelLimits::new(8192, 4096)),
    ("gpt-3.5", ModelLimits::new(16_385, 4096)),
];

/// Registry of known model limits with support for custom overrides.
///
/// The registry provides model limits through a two-tier lookup:
/// 1. First, check custom overrides set via [`ModelRegistry::set_override`]
/// 2. If no override exists, use prefix matching against known model limits
/// 3. If no prefix matches, return `DEFAULT_LIMITS` with an explicit `DefaultFallback` source.
///
/// # Prefix Matching
///
/// Model names are matched using prefix comparison. For example:
/// - `"claude-sonnet-4-20250514"` matches prefix `"claude-sonnet-4"`
/// - `"gpt-4o-mini"` matches prefix `"gpt-4o"`
///
/// # Example
///
/// ```
/// use forge::context_infinity::{ModelRegistry, ModelLimits};
///
/// let mut registry = ModelRegistry::new();
///
/// // Get limits for a known model
/// let claude_limits = registry.get("claude-sonnet-4-20250514").limits();
/// assert_eq!(claude_limits.context_window(), 200_000);
///
/// // Set custom override
/// registry.set_override(
///     "my-custom-model".to_string(),
///     ModelLimits::new(50_000, 8000),
/// );
/// let custom = registry.get("my-custom-model").limits();
/// assert_eq!(custom.context_window(), 50_000);
/// ```
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Custom overrides that take precedence over default prefix matching.
    overrides: HashMap<String, ModelLimits>,
}

impl ModelRegistry {
    /// Creates a new model registry with no custom overrides.
    #[must_use]
    pub fn new() -> Self {
        Self {
            overrides: HashMap::new(),
        }
    }

    /// Returns the limits for the given model.
    ///
    /// Lookup order:
    /// 1. Exact match in overrides
    /// 2. Prefix match against known models
    /// 3. Default fallback limits
    ///
    /// # Arguments
    ///
    /// * `model` - The model name/identifier to look up
    #[must_use]
    pub fn get(&self, model: &str) -> ResolvedModelLimits {
        // Check overrides first (exact match)
        if let Some(limits) = self.overrides.get(model) {
            return ResolvedModelLimits::new(*limits, ModelLimitsSource::Override);
        }

        // Try prefix matching against known models
        for (prefix, limits) in KNOWN_MODELS {
            if model.starts_with(prefix) {
                return ResolvedModelLimits::new(*limits, ModelLimitsSource::Prefix(prefix));
            }
        }

        // Return default fallback
        ResolvedModelLimits::new(DEFAULT_LIMITS, ModelLimitsSource::DefaultFallback)
    }

    /// Sets a custom override for a specific model.
    ///
    /// Overrides take precedence over prefix matching for exact matches.
    ///
    /// # Arguments
    ///
    /// * `model` - The exact model name to override
    /// * `limits` - The custom limits to use for this model
    #[cfg(test)]
    pub fn set_override(&mut self, model: String, limits: ModelLimits) {
        self.overrides.insert(model, limits);
    }

    /// Removes a custom override for a model.
    ///
    /// After removal, the model will use prefix matching or default limits.
    ///
    /// # Arguments
    ///
    /// * `model` - The model name whose override should be removed
    ///
    /// # Returns
    ///
    /// The removed limits if an override existed, or `None` otherwise.
    #[cfg(test)]
    pub fn remove_override(&mut self, model: &str) -> Option<ModelLimits> {
        self.overrides.remove(model)
    }

    /// Returns `true` if the model has a custom override set.
    #[must_use]
    #[cfg(test)]
    pub fn has_override(&self, model: &str) -> bool {
        self.overrides.contains_key(model)
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod model_limits {
        use super::*;

        #[test]
        fn new_creates_limits_with_given_values() {
            let limits = ModelLimits::new(100_000, 8000);
            assert_eq!(limits.context_window(), 100_000);
            assert_eq!(limits.max_output(), 8000);
        }

        #[test]
        fn effective_input_budget_subtracts_output_and_safety_margin() {
            let limits = ModelLimits::new(200_000, 16_000);
            // available = 200_000 - 16_000 = 184_000
            // safety = 184_000 / 20 = 9_200
            // effective = 184_000 - 9_200 = 174_800
            assert_eq!(limits.effective_input_budget(), 174_800);
        }

        #[test]
        fn effective_input_budget_with_small_values() {
            let limits = ModelLimits::new(8192, 4096);
            // available = 8192 - 4096 = 4096
            // safety = 4096 / 20 = 204 (integer division)
            // effective = 4096 - 204 = 3892
            assert_eq!(limits.effective_input_budget(), 3892);
        }

        #[test]
        fn effective_input_budget_handles_output_exceeding_context() {
            // Edge case: max_output >= context_window
            let limits = ModelLimits::new(4096, 8192);
            // saturating_sub returns 0
            assert_eq!(limits.effective_input_budget(), 0);
        }

        #[test]
        fn limits_are_copy_and_clone() {
            let limits = ModelLimits::new(100_000, 8000);
            let copied = limits;
            let cloned = limits.clone();
            assert_eq!(limits, copied);
            assert_eq!(limits, cloned);
        }

        #[test]
        fn limits_equality() {
            let a = ModelLimits::new(100_000, 8000);
            let b = ModelLimits::new(100_000, 8000);
            let c = ModelLimits::new(100_000, 9000);
            assert_eq!(a, b);
            assert_ne!(a, c);
        }
    }

    mod model_registry {
        use super::*;

        #[test]
        fn new_creates_empty_registry() {
            let registry = ModelRegistry::new();
            assert!(!registry.has_override("any-model"));
        }

        #[test]
        fn get_claude_opus_4_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("claude-opus-4").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);

            let limits = registry.get("claude-opus-4-20250514").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_claude_sonnet_4_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("claude-sonnet-4").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);

            let limits = registry.get("claude-sonnet-4-20250514").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_claude_3_5_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("claude-3-5-sonnet-20241022").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);

            let limits = registry.get("claude-3-5-haiku-20241022").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_claude_3_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("claude-3-opus-20240229").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);

            let limits = registry.get("claude-3-sonnet-20240229").limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_claude_family_matches_unlisted_variants() {
            let registry = ModelRegistry::new();

            let resolved = registry.get("claude-haiku-4-5-20251001");
            assert_eq!(resolved.source(), ModelLimitsSource::Prefix("claude"));
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_gpt_4o_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("gpt-4o").limits();
            assert_eq!(limits.context_window(), 128_000);
            assert_eq!(limits.max_output(), 16_384);

            let limits = registry.get("gpt-4o-mini").limits();
            assert_eq!(limits.context_window(), 128_000);
            assert_eq!(limits.max_output(), 16_384);

            let limits = registry.get("gpt-4o-2024-08-06").limits();
            assert_eq!(limits.context_window(), 128_000);
            assert_eq!(limits.max_output(), 16_384);
        }

        #[test]
        fn get_gpt_4_turbo_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("gpt-4-turbo").limits();
            assert_eq!(limits.context_window(), 128_000);
            assert_eq!(limits.max_output(), 4096);

            let limits = registry.get("gpt-4-turbo-preview").limits();
            assert_eq!(limits.context_window(), 128_000);
            assert_eq!(limits.max_output(), 4096);
        }

        #[test]
        fn get_gpt_4_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("gpt-4").limits();
            assert_eq!(limits.context_window(), 8192);
            assert_eq!(limits.max_output(), 4096);

            let limits = registry.get("gpt-4-0613").limits();
            assert_eq!(limits.context_window(), 8192);
            assert_eq!(limits.max_output(), 4096);
        }

        #[test]
        fn get_gpt_3_5_models() {
            let registry = ModelRegistry::new();

            let limits = registry.get("gpt-3.5-turbo").limits();
            assert_eq!(limits.context_window(), 16_385);
            assert_eq!(limits.max_output(), 4096);

            let limits = registry.get("gpt-3.5-turbo-0125").limits();
            assert_eq!(limits.context_window(), 16_385);
            assert_eq!(limits.max_output(), 4096);
        }

        #[test]
        fn get_unknown_model_returns_default() {
            let registry = ModelRegistry::new();

            let resolved = registry.get("unknown-model-xyz");
            assert_eq!(resolved.source(), ModelLimitsSource::DefaultFallback);
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 8192);
            assert_eq!(limits.max_output(), 4096);

            let resolved = registry.get("llama-3-70b");
            assert_eq!(resolved.source(), ModelLimitsSource::DefaultFallback);
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 8192);
            assert_eq!(limits.max_output(), 4096);
        }

        #[test]
        fn set_override_takes_precedence() {
            let mut registry = ModelRegistry::new();

            // Before override, uses prefix matching
            let limits = registry.get("claude-sonnet-4-custom").limits();
            assert_eq!(limits.context_window(), 200_000);

            // Set override
            registry.set_override(
                "claude-sonnet-4-custom".to_string(),
                ModelLimits::new(50_000, 8000),
            );

            // After override, uses custom limits
            let resolved = registry.get("claude-sonnet-4-custom");
            assert_eq!(resolved.source(), ModelLimitsSource::Override);
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 50_000);
            assert_eq!(limits.max_output(), 8000);

            // Other claude-sonnet-4 models still use defaults
            let limits = registry.get("claude-sonnet-4-20250514").limits();
            assert_eq!(limits.context_window(), 200_000);
        }

        #[test]
        fn remove_override_restores_default_behavior() {
            let mut registry = ModelRegistry::new();

            registry.set_override("gpt-4o".to_string(), ModelLimits::new(50_000, 5000));
            assert!(registry.has_override("gpt-4o"));

            let removed = registry.remove_override("gpt-4o");
            assert!(removed.is_some());
            assert!(!registry.has_override("gpt-4o"));

            // Should now use prefix matching
            let resolved = registry.get("gpt-4o");
            assert_eq!(resolved.source(), ModelLimitsSource::Prefix("gpt-4o"));
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 128_000);
        }

        #[test]
        fn remove_nonexistent_override_returns_none() {
            let mut registry = ModelRegistry::new();
            let removed = registry.remove_override("nonexistent");
            assert!(removed.is_none());
        }

        #[test]
        fn has_override_returns_correct_state() {
            let mut registry = ModelRegistry::new();

            assert!(!registry.has_override("test-model"));
            registry.set_override("test-model".to_string(), ModelLimits::new(1000, 100));
            assert!(registry.has_override("test-model"));
        }

        #[test]
        fn default_is_same_as_new() {
            let new_registry = ModelRegistry::new();
            let default_registry = ModelRegistry::default();

            // Both should return same limits for any model
            assert_eq!(
                new_registry.get("claude-sonnet-4"),
                default_registry.get("claude-sonnet-4")
            );
            assert_eq!(new_registry.get("unknown"), default_registry.get("unknown"));
        }

        #[test]
        fn registry_is_clone() {
            let mut registry = ModelRegistry::new();
            registry.set_override("test".to_string(), ModelLimits::new(1000, 100));

            let cloned = registry.clone();
            assert_eq!(cloned.get("test"), registry.get("test"));
        }
    }
}

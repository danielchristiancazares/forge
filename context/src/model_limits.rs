//! Model token limits and registry.

use std::collections::HashMap;

use forge_types::{ModelName, PredefinedModel};

/// The effective input budget accounts for output reservation and a safety margin.
///
/// # Example
///
/// ```ignore
/// use forge_context::ModelLimits;
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
    context_window: u32,
    max_output: u32,
}

impl ModelLimits {
    #[must_use]
    pub const fn new(context_window: u32, max_output: u32) -> Self {
        Self {
            context_window,
            max_output,
        }
    }

    ///
    /// This is the maximum number of tokens available for input messages,
    /// calculated as: `context_window - max_output - 5% safety margin`.
    ///
    /// The 5% safety margin accounts for token counting inaccuracies and
    /// overhead from system prompts, formatting, and tool definitions.
    /// Note: This reserves the model's `max_output`, which may be overly conservative
    /// if the user has configured a smaller output limit. Consider using
    /// `effective_input_budget_with_reserved()` when the configured limit is known.
    #[must_use]
    pub fn effective_input_budget(&self) -> u32 {
        self.effective_input_budget_with_reserved(self.max_output)
    }

    /// The reserved amount is clamped to the model's `max_output`.
    #[must_use]
    pub fn effective_input_budget_with_reserved(&self, reserved_output: u32) -> u32 {
        let reserved = reserved_output.min(self.max_output);
        let available = self.context_window.saturating_sub(reserved);
        // Subtract 5% safety margin, capped at 4096 tokens
        let safety_margin = (available / 20).min(4096);
        available.saturating_sub(safety_margin)
    }

    #[must_use]
    pub const fn context_window(&self) -> u32 {
        self.context_window
    }

    #[must_use]
    pub const fn max_output(&self) -> u32 {
        self.max_output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLimitsSource {
    Override,
    Catalog(PredefinedModel),
}

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

fn default_limits_for(model: PredefinedModel) -> ModelLimits {
    match model {
        PredefinedModel::ClaudeOpus => ModelLimits::new(1_000_000, 128_000),
        PredefinedModel::ClaudeHaiku => ModelLimits::new(200_000, 64_000),
        PredefinedModel::Gpt52Pro | PredefinedModel::Gpt52 => ModelLimits::new(400_000, 128_000),
        PredefinedModel::GeminiPro | PredefinedModel::GeminiFlash => {
            ModelLimits::new(1_048_576, 65_536)
        }
    }
}

/// 1. First, check custom overrides set via [`ModelRegistry::set_override`]
/// 2. If no override exists, use the canonical model catalog
///
/// Unknown models must be rejected at the boundary; the core only operates on
/// validated [`ModelName`] values.
///
/// # Example
///
/// ```ignore
/// use forge_context::{ModelRegistry, ModelLimits};
///
/// let registry = ModelRegistry::new();
///
/// // Get limits for a known model
/// let model = forge_types::PredefinedModel::ClaudeOpus.to_model_name();
/// let resolved = registry.get(&model);
/// assert_eq!(resolved.limits().context_window(), 200_000);
/// ```
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Custom overrides that take precedence over catalog defaults.
    overrides: HashMap<PredefinedModel, ModelLimits>,
}

impl ModelRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            overrides: HashMap::new(),
        }
    }

    #[must_use]
    pub fn get(&self, model: &ModelName) -> ResolvedModelLimits {
        let predefined = model.predefined();
        if let Some(limits) = self.overrides.get(&predefined) {
            return ResolvedModelLimits::new(*limits, ModelLimitsSource::Override);
        }

        let limits = default_limits_for(predefined);
        ResolvedModelLimits::new(limits, ModelLimitsSource::Catalog(predefined))
    }

    #[cfg(test)]
    pub fn set_override(&mut self, model: PredefinedModel, limits: ModelLimits) {
        self.overrides.insert(model, limits);
    }

    /// The removed limits if an override existed, or `None` otherwise.
    #[cfg(test)]
    pub fn remove_override(&mut self, model: PredefinedModel) -> Option<ModelLimits> {
        self.overrides.remove(&model)
    }

    #[must_use]
    #[cfg(test)]
    pub fn has_override(&self, model: PredefinedModel) -> bool {
        self.overrides.contains_key(&model)
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
            // safety = 184_000 / 20 = 9_200 -> capped at 4096
            // effective = 184_000 - 4096 = 179,904
            assert_eq!(limits.effective_input_budget(), 179_904);
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
        fn effective_input_budget_with_reserved_uses_configured_limit() {
            // Model has 64k max output, but user configured 16k
            let limits = ModelLimits::new(200_000, 64_000);
            // Using model max: 200k - 64k = 136k
            // Safety = 136k / 20 = 6800 -> capped at 4096
            // Effective = 136k - 4096 = 131,904
            assert_eq!(limits.effective_input_budget(), 131_904);

            // Using configured 16k: 200k - 16k = 184k
            // Safety = 184k / 20 = 9200 -> capped at 4096
            // Effective = 184k - 4096 = 179,904
            assert_eq!(limits.effective_input_budget_with_reserved(16_000), 179_904);
        }

        #[test]
        fn effective_input_budget_with_reserved_clamps_to_max() {
            // Reserved can't exceed model's max_output
            let limits = ModelLimits::new(200_000, 16_000);
            // Requesting 64k reserved but model only supports 16k
            assert_eq!(
                limits.effective_input_budget_with_reserved(64_000),
                limits.effective_input_budget()
            );
        }

        #[test]
        fn limits_are_copy_and_clone() {
            let limits = ModelLimits::new(100_000, 8000);
            let copied = limits;
            let cloned = limits;
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
        use forge_types::{ModelName, PredefinedModel};

        fn model(predefined: PredefinedModel) -> ModelName {
            predefined.to_model_name()
        }

        #[test]
        fn new_creates_empty_registry() {
            let registry = ModelRegistry::new();
            assert!(!registry.has_override(PredefinedModel::ClaudeOpus));
        }

        #[test]
        fn get_claude_opus_4_6_models() {
            let registry = ModelRegistry::new();

            let resolved = registry.get(&model(PredefinedModel::ClaudeOpus));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::ClaudeOpus)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 1_000_000);
            assert_eq!(limits.max_output(), 128_000);
        }

        #[test]
        fn get_claude_haiku_4_5_models() {
            let registry = ModelRegistry::new();

            let resolved = registry.get(&model(PredefinedModel::ClaudeHaiku));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::ClaudeHaiku)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 200_000);
            assert_eq!(limits.max_output(), 64_000);
        }

        #[test]
        fn get_gpt_5_2_models() {
            let registry = ModelRegistry::new();

            let resolved = registry.get(&model(PredefinedModel::Gpt52));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::Gpt52)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 400_000);
            assert_eq!(limits.max_output(), 128_000);

            let resolved = registry.get(&model(PredefinedModel::Gpt52Pro));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::Gpt52Pro)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 400_000);
            assert_eq!(limits.max_output(), 128_000);
        }

        #[test]
        fn get_gemini_3_pro_models() {
            let registry = ModelRegistry::new();

            let resolved = registry.get(&model(PredefinedModel::GeminiPro));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::GeminiPro)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 1_048_576);
            assert_eq!(limits.max_output(), 65_536);
        }

        #[test]
        fn get_gemini_3_flash_models() {
            let registry = ModelRegistry::new();

            let resolved = registry.get(&model(PredefinedModel::GeminiFlash));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::GeminiFlash)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 1_048_576);
            assert_eq!(limits.max_output(), 65_536);
        }

        #[test]
        fn set_override_takes_precedence() {
            let mut registry = ModelRegistry::new();

            // Before override, uses catalog defaults
            let limits = registry.get(&model(PredefinedModel::ClaudeOpus)).limits();
            assert_eq!(limits.context_window(), 1_000_000);

            // Set override
            registry.set_override(PredefinedModel::ClaudeOpus, ModelLimits::new(50_000, 8000));

            // After override, uses custom limits
            let resolved = registry.get(&model(PredefinedModel::ClaudeOpus));
            assert_eq!(resolved.source(), ModelLimitsSource::Override);
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 50_000);
            assert_eq!(limits.max_output(), 8000);

            // Other Claude models still use defaults
            let limits = registry.get(&model(PredefinedModel::ClaudeHaiku)).limits();
            assert_eq!(limits.context_window(), 200_000);
        }

        #[test]
        fn remove_override_restores_default_behavior() {
            let mut registry = ModelRegistry::new();

            registry.set_override(PredefinedModel::Gpt52, ModelLimits::new(50_000, 5000));
            assert!(registry.has_override(PredefinedModel::Gpt52));

            let removed = registry.remove_override(PredefinedModel::Gpt52);
            assert!(removed.is_some());
            assert!(!registry.has_override(PredefinedModel::Gpt52));

            // Should now use catalog defaults
            let resolved = registry.get(&model(PredefinedModel::Gpt52));
            assert_eq!(
                resolved.source(),
                ModelLimitsSource::Catalog(PredefinedModel::Gpt52)
            );
            let limits = resolved.limits();
            assert_eq!(limits.context_window(), 400_000);
        }

        #[test]
        fn remove_nonexistent_override_returns_none() {
            let mut registry = ModelRegistry::new();
            let removed = registry.remove_override(PredefinedModel::GeminiPro);
            assert!(removed.is_none());
        }

        #[test]
        fn has_override_returns_correct_state() {
            let mut registry = ModelRegistry::new();

            assert!(!registry.has_override(PredefinedModel::GeminiFlash));
            registry.set_override(PredefinedModel::GeminiFlash, ModelLimits::new(1000, 100));
            assert!(registry.has_override(PredefinedModel::GeminiFlash));
        }

        #[test]
        fn default_is_same_as_new() {
            let new_registry = ModelRegistry::new();
            let default_registry = ModelRegistry::default();

            // Both should return same limits for known models
            assert_eq!(
                new_registry.get(&model(PredefinedModel::ClaudeOpus)),
                default_registry.get(&model(PredefinedModel::ClaudeOpus))
            );
        }

        #[test]
        fn registry_is_clone() {
            let mut registry = ModelRegistry::new();
            registry.set_override(PredefinedModel::Gpt52Pro, ModelLimits::new(1000, 100));

            let cloned = registry.clone();
            let model = model(PredefinedModel::Gpt52Pro);
            assert_eq!(cloned.get(&model), registry.get(&model));
        }
    }
}

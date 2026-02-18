//! Compile-time asset embedding for provider-specific system prompts.
//!
//! This module embeds system prompt files at compile time using `include_str!`,
//! ensuring the binary is self-contained with no runtime file I/O required.
//!
//! # Provider-Specific Prompts
//!
//! | Provider | File | Description |
//! |----------|------|-------------|
//! | Claude | `base_prompt.md` | Shared prompt for Anthropic/OpenAI |
//! | OpenAI | `base_prompt.md` | Shared prompt for Anthropic/OpenAI |
//! | Gemini | `base_prompt_gemini.md` | Google Gemini models (gRPC, different capabilities) |
//!
//! # Initialization
//!
//! Call [`init`] at startup to eagerly initialize all prompts. The [`system_prompts`]
//! function falls back to lazy initialization if `init()` was not called.

use std::sync::OnceLock;

use forge_engine::SystemPrompts;

const BASE_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt.md"
));

const GEMINI_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt_gemini.md"
));

const PARTIAL_SECURITY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/security.md"
));
const PARTIAL_LP1: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/lp1.md"
));
const PARTIAL_PLAN_TOOL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/plan_tool.md"
));
const PARTIAL_AGENTIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/agentic_operations.md"
));
const PARTIAL_RESPONSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/response_style.md"
));
const PARTIAL_CODING: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/partials/coding_philosophy.md"
));

static BASE_PROMPT: OnceLock<String> = OnceLock::new();
static GEMINI_PROMPT: OnceLock<String> = OnceLock::new();

fn resolve_partials(template: &str) -> String {
    template
        .replace("{security}", PARTIAL_SECURITY)
        .replace("{lp1}", PARTIAL_LP1)
        .replace("{plan_tool}", PARTIAL_PLAN_TOOL)
        .replace("{agentic_operations}", PARTIAL_AGENTIC)
        .replace("{response_style}", PARTIAL_RESPONSE)
        .replace("{coding_philosophy}", PARTIAL_CODING)
}

/// Eagerly initializes all system prompts.
///
/// Call this at startup to avoid lazy initialization overhead on first use.
/// Safe to call multiple times (subsequent calls are no-ops).
pub fn init() {
    let _ = BASE_PROMPT.set(resolve_partials(BASE_PROMPT_RAW));
    let _ = GEMINI_PROMPT.set(resolve_partials(GEMINI_PROMPT_RAW));
}

/// Returns provider-specific system prompts for LLM initialization.
///
/// The returned [`SystemPrompts`] contains `&'static str` references for zero-copy usage.
/// Falls back to lazy initialization if [`init`] was not called.
#[must_use]
pub fn system_prompts() -> SystemPrompts {
    let base = BASE_PROMPT
        .get_or_init(|| resolve_partials(BASE_PROMPT_RAW))
        .as_str();
    SystemPrompts {
        claude: base,
        openai: base,
        gemini: GEMINI_PROMPT
            .get_or_init(|| resolve_partials(GEMINI_PROMPT_RAW))
            .as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::{BASE_PROMPT_RAW, GEMINI_PROMPT_RAW, resolve_partials};

    #[test]
    fn resolved_prompts_contain_no_unresolved_placeholders() {
        let base = resolve_partials(BASE_PROMPT_RAW);
        let gemini = resolve_partials(GEMINI_PROMPT_RAW);

        let placeholders = [
            "{security}",
            "{lp1}",
            "{plan_tool}",
            "{agentic_operations}",
            "{response_style}",
            "{coding_philosophy}",
        ];

        for ph in &placeholders {
            assert!(
                !base.contains(ph),
                "base prompt contains unresolved placeholder: {ph}"
            );
            assert!(
                !gemini.contains(ph),
                "gemini prompt contains unresolved placeholder: {ph}"
            );
        }

        // {environment_context} must survive partial resolution
        assert!(base.contains("{environment_context}"));
        assert!(gemini.contains("{environment_context}"));
    }
}

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

static BASE_PROMPT: OnceLock<String> = OnceLock::new();
static GEMINI_PROMPT: OnceLock<String> = OnceLock::new();

/// Eagerly initializes all system prompts.
///
/// Call this at startup to avoid lazy initialization overhead on first use.
/// Safe to call multiple times (subsequent calls are no-ops).
pub fn init() {
    let _ = BASE_PROMPT.set(BASE_PROMPT_RAW.to_string());
    let _ = GEMINI_PROMPT.set(GEMINI_PROMPT_RAW.to_string());
}

/// Returns provider-specific system prompts for LLM initialization.
///
/// The returned [`SystemPrompts`] contains `&'static str` references for zero-copy usage.
/// Falls back to lazy initialization if [`init`] was not called.
#[must_use]
pub fn system_prompts() -> SystemPrompts {
    let base = BASE_PROMPT
        .get_or_init(|| BASE_PROMPT_RAW.to_string())
        .as_str();
    SystemPrompts {
        claude: base,
        openai: base,
        gemini: GEMINI_PROMPT
            .get_or_init(|| GEMINI_PROMPT_RAW.to_string())
            .as_str(),
    }
}

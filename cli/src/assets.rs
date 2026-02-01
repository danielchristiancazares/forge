//! Compile-time asset embedding for provider-specific system prompts.
//!
//! This module embeds system prompt files at compile time using `include_str!`,
//! ensuring the binary is self-contained with no runtime file I/O required.
//!
//! # Provider-Specific Prompts
//!
//! Each LLM provider has an optimized system prompt:
//!
//! | Provider | File | Description |
//! |----------|------|-------------|
//! | Claude | `base_prompt_claude.md` | Anthropic Claude models |
//! | OpenAI | `base_prompt_openai.md` | OpenAI GPT models |
//! | Gemini | `base_prompt_gemini.md` | Google Gemini models |
//!
//! # Initialization
//!
//! Call [`init`] at startup to eagerly initialize all prompts. The [`system_prompts`]
//! function falls back to lazy initialization if `init()` was not called.

use std::sync::OnceLock;

use forge_engine::SystemPrompts;

const CLAUDE_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt_claude.md"
));

const OPENAI_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt_openai.md"
));

const GEMINI_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/base_prompt_gemini.md"
));

static CLAUDE_PROMPT: OnceLock<String> = OnceLock::new();
static OPENAI_PROMPT: OnceLock<String> = OnceLock::new();
static GEMINI_PROMPT: OnceLock<String> = OnceLock::new();

/// Eagerly initializes all system prompts.
///
/// Call this at startup to avoid lazy initialization overhead on first use.
/// Safe to call multiple times (subsequent calls are no-ops).
pub fn init() {
    let _ = CLAUDE_PROMPT.set(CLAUDE_PROMPT_RAW.to_string());
    let _ = OPENAI_PROMPT.set(OPENAI_PROMPT_RAW.to_string());
    let _ = GEMINI_PROMPT.set(GEMINI_PROMPT_RAW.to_string());
}

/// Returns provider-specific system prompts for LLM initialization.
///
/// The returned [`SystemPrompts`] contains `&'static str` references for zero-copy usage.
/// Falls back to lazy initialization if [`init`] was not called.
#[must_use]
pub fn system_prompts() -> SystemPrompts {
    SystemPrompts {
        claude: CLAUDE_PROMPT
            .get_or_init(|| CLAUDE_PROMPT_RAW.to_string())
            .as_str(),
        openai: OPENAI_PROMPT
            .get_or_init(|| OPENAI_PROMPT_RAW.to_string())
            .as_str(),
        gemini: GEMINI_PROMPT
            .get_or_init(|| GEMINI_PROMPT_RAW.to_string())
            .as_str(),
    }
}

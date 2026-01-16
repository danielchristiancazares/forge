use std::sync::OnceLock;

use forge_engine::SystemPrompts;

const DEFAULT_PROMPT_RAW: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/prompt.md"));

const GEMINI_PROMPT_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/gemini_prompt.md"
));

static DEFAULT_PROMPT: OnceLock<String> = OnceLock::new();
static GEMINI_PROMPT: OnceLock<String> = OnceLock::new();

pub fn init() {
    let _ = DEFAULT_PROMPT.set(DEFAULT_PROMPT_RAW.to_string());
    let _ = GEMINI_PROMPT.set(GEMINI_PROMPT_RAW.to_string());
}

/// Get provider-specific system prompts.
#[must_use]
pub fn system_prompts() -> SystemPrompts {
    SystemPrompts {
        default: DEFAULT_PROMPT
            .get_or_init(|| DEFAULT_PROMPT_RAW.to_string())
            .as_str(),
        gemini: GEMINI_PROMPT
            .get_or_init(|| GEMINI_PROMPT_RAW.to_string())
            .as_str(),
    }
}

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

pub fn init() {
    let _ = CLAUDE_PROMPT.set(CLAUDE_PROMPT_RAW.to_string());
    let _ = OPENAI_PROMPT.set(OPENAI_PROMPT_RAW.to_string());
    let _ = GEMINI_PROMPT.set(GEMINI_PROMPT_RAW.to_string());
}

/// Get provider-specific system prompts.
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

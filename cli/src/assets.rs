use std::sync::OnceLock;

const SYSTEM_PROMPT_RAW: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/prompt.md"));

static SYSTEM_PROMPT: OnceLock<String> = OnceLock::new();

pub fn init() {
    let _ = SYSTEM_PROMPT.set(SYSTEM_PROMPT_RAW.to_string());
}

/// Get the shared system prompt used by all providers.
pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
        .get_or_init(|| SYSTEM_PROMPT_RAW.to_string())
        .as_str()
}

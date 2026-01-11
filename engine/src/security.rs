//! Security utilities for sanitization and redaction.
//!
//! These functions prevent sensitive data (like API keys) from leaking
//! into logs, error messages, or terminal output.

use forge_types::sanitize_terminal_text;

/// Sanitize a stream error message by redacting API keys and stripping terminal controls.
pub fn sanitize_stream_error(raw: &str) -> String {
    // First redact API keys, then strip terminal controls
    let redacted = redact_api_keys(raw.trim());
    sanitize_terminal_text(&redacted).into_owned()
}

/// Redact API keys from a string.
///
/// Detects patterns like `sk-...` and replaces them with `sk-***`.
pub fn redact_api_keys(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == 's' {
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('k') && lookahead.next() == Some('-') {
                // Consume the remaining "k-" in the real iterator.
                chars.next();
                chars.next();
                output.push_str("sk-***");
                while let Some(&next_ch) = chars.peek() {
                    if is_key_delimiter(next_ch) {
                        break;
                    }
                    chars.next();
                }
                continue;
            }
        }
        output.push(ch);
    }
    output
}

fn is_key_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | '}' | ']' | ')' | '\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_api_keys_replaces_openai_key() {
        let input = "Error: sk-abc123xyz key invalid";
        let output = redact_api_keys(input);
        assert_eq!(output, "Error: sk-*** key invalid");
    }

    #[test]
    fn redact_api_keys_handles_quoted_key() {
        let input = r#"{"key": "sk-secret123"}"#;
        let output = redact_api_keys(input);
        assert_eq!(output, r#"{"key": "sk-***"}"#);
    }

    #[test]
    fn redact_api_keys_handles_multiple_keys() {
        let input = "key1: sk-first, key2: sk-second";
        let output = redact_api_keys(input);
        assert_eq!(output, "key1: sk-***, key2: sk-***");
    }

    #[test]
    fn redact_api_keys_preserves_non_key_text() {
        let input = "This is a normal message without keys";
        let output = redact_api_keys(input);
        assert_eq!(output, input);
    }

    #[test]
    fn redact_api_keys_handles_sk_without_dash() {
        let input = "The word 'skip' should not be redacted";
        let output = redact_api_keys(input);
        assert_eq!(output, input);
    }

    #[test]
    fn sanitize_stream_error_redacts_and_strips() {
        let input = "Error with sk-secret123 and \x1b[31mred text\x1b[0m";
        let output = sanitize_stream_error(input);
        assert!(output.contains("sk-***"));
        assert!(!output.contains("sk-secret123"));
        // Terminal controls should be stripped
        assert!(!output.contains("\x1b["));
    }
}

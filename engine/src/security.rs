//! Security utilities for sanitization and redaction.
//!
//! These functions prevent sensitive data (like API keys) from leaking
//! into logs, error messages, or terminal output.
//!
//! # Dynamic Secret Redaction
//!
//! In addition to pattern-based redaction (`sk-*`, `AIza*`), this module provides
//! dynamic secret redaction via [`SecretRedactor`]. At first use, it scans environment
//! variables for sensitive patterns (e.g., `*_KEY`, `*_TOKEN`, `*_SECRET`) and builds
//! an Aho-Corasick automaton for O(n) multi-pattern matching.
//!
//! The redactor is constructed via a single Authority Boundary ([`SecretRedactor::from_env`])
//! and cached in a `OnceLock` per IFA-7.

use std::sync::OnceLock;

use aho_corasick::AhoCorasick;
use forge_types::{sanitize_terminal_text, strip_steganographic_chars};
use globset::{GlobBuilder, GlobSetBuilder};
use regex::Regex;

/// Minimum length for env var values to be considered secrets.
/// Avoids false positives on short values ("true", "1", "yes").
const MIN_SECRET_LENGTH: usize = 16;

/// Variable name patterns indicating sensitive values.
const SENSITIVE_VAR_PATTERNS: &[&str] = &[
    "*_KEY",
    "*_TOKEN",
    "*_SECRET",
    "*_PASSWORD",
    "AWS_*",
    "ANTHROPIC_*",
    "OPENAI_*",
    "GEMINI_*",
    "GITHUB_TOKEN",
    "GH_TOKEN",
];

/// Runtime secret redactor built from environment variables.
///
/// Constructed via single Authority Boundary ([`from_env`](Self::from_env)),
/// cached in `OnceLock`. Secrets are never logged or exposed via Debug.
pub struct SecretRedactor {
    automaton: Option<AhoCorasick>,
    secret_count: usize,
}

impl std::fmt::Debug for SecretRedactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRedactor")
            .field("secret_count", &self.secret_count)
            .finish_non_exhaustive() // Omit automaton contents
    }
}

impl SecretRedactor {
    /// Build redactor from environment (AUTHORITY BOUNDARY per IFA-7).
    ///
    /// Scans `std::env::vars()` for variable names matching sensitive patterns,
    /// extracts their values, and builds an Aho-Corasick automaton for O(n) matching.
    #[must_use]
    pub fn from_env() -> Self {
        let matcher = build_var_name_matcher();
        let mut secrets: Vec<String> = std::env::vars()
            .filter(|(name, _)| matcher.is_match(name))
            .map(|(_, value)| value.trim().to_string())
            .filter(|v| v.len() >= MIN_SECRET_LENGTH)
            .filter(|v| !looks_like_non_secret(v))
            .collect();

        secrets.sort();
        secrets.dedup();

        let secret_count = secrets.len();
        let automaton = if secrets.is_empty() {
            None
        } else {
            AhoCorasick::new(&secrets).ok()
        };

        tracing::debug!(secret_count, "SecretRedactor initialized");
        Self {
            automaton,
            secret_count,
        }
    }

    /// Redact all known secret values from input.
    ///
    /// Returns the input with all detected secrets replaced by `[REDACTED]`.
    #[must_use]
    pub fn redact<'a>(&self, input: &'a str) -> std::borrow::Cow<'a, str> {
        match &self.automaton {
            Some(ac) => {
                let mut result = String::with_capacity(input.len());
                ac.replace_all_with(input, &mut result, |_, _, dst| {
                    dst.push_str("[REDACTED]");
                    true
                });
                std::borrow::Cow::Owned(result)
            }
            None => std::borrow::Cow::Borrowed(input),
        }
    }

    /// Returns true if any secrets were detected in the environment.
    #[must_use]
    #[allow(dead_code)]
    pub fn has_secrets(&self) -> bool {
        self.secret_count > 0
    }

    /// Returns the number of unique secrets detected.
    #[must_use]
    #[allow(dead_code)]
    pub fn secret_count(&self) -> usize {
        self.secret_count
    }
}

fn build_var_name_matcher() -> globset::GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in SENSITIVE_VAR_PATTERNS {
        if let Ok(glob) = GlobBuilder::new(pattern).case_insensitive(true).build() {
            builder.add(glob);
        }
    }
    builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap())
}

/// Check if a value looks like a non-secret (path, URL, numeric).
fn looks_like_non_secret(value: &str) -> bool {
    // File paths
    if value.starts_with('/') || value.starts_with("C:\\") || value.starts_with("D:\\") {
        return true;
    }

    // Plain URLs without credentials
    if (value.starts_with("http://") || value.starts_with("https://"))
        && !value.contains("token=")
        && !value.contains("key=")
        && !value.contains("secret=")
        && !value.contains("password=")
    {
        return true;
    }

    // Pure numeric values
    if value.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    false
}

/// Single global instance (IFA-7 compliant).
///
/// Sanitize text for display by redacting secrets and stripping terminal controls.
///
/// Applies three sanitization passes:
/// 1. Redact pattern-based API keys (OpenAI `sk-*`, Anthropic `sk-ant-*`, Gemini `AIza*`)
/// 2. Redact value-based secrets from environment variables
/// 3. Strip terminal escape sequences and steganographic characters
#[allow(dead_code)] // Public API, currently unused by dependents in this workspace.
#[must_use]
pub fn sanitize_display_text(input: &str) -> String {
    // Normalize away terminal controls before redacting. This prevents escape-based
    // token splitting from bypassing literal pattern matching.
    let terminal_safe = sanitize_terminal_text(input);
    let pattern_redacted = redact_api_keys(terminal_safe.as_ref());
    let value_redacted = secret_redactor().redact(&pattern_redacted);
    match value_redacted {
        std::borrow::Cow::Borrowed(_) => pattern_redacted,
        std::borrow::Cow::Owned(v) => v,
    }
}

static SECRET_REDACTOR: OnceLock<SecretRedactor> = OnceLock::new();

/// Get the global secret redactor instance.
///
/// Initializes on first call by scanning environment variables.
pub fn secret_redactor() -> &'static SecretRedactor {
    SECRET_REDACTOR.get_or_init(SecretRedactor::from_env)
}

/// Sanitize a stream error message by redacting secrets and stripping controls.
///
/// Applies four sanitization passes:
/// 1. Redact pattern-based API keys (OpenAI `sk-*`, Anthropic `sk-ant-*`, Gemini `AIza*`)
/// 2. Redact value-based secrets from environment variables
/// 3. Strip terminal escape sequences
/// 4. Strip steganographic characters
pub fn sanitize_stream_error(raw: &str) -> String {
    let trimmed = raw.trim();
    let terminal_safe = sanitize_terminal_text(trimmed);
    let pattern_redacted = redact_api_keys(terminal_safe.as_ref());
    let value_redacted = secret_redactor().redact(&pattern_redacted);
    strip_steganographic_chars(&value_redacted).into_owned()
}

/// Redact sensitive tokens from a string.
///
/// Detects and redacts common high-risk token formats:
/// - OpenAI: `sk-...` → `sk-***`
/// - Anthropic: `sk-ant-...` → `sk-ant-***`
/// - Google/Gemini: `AIza...` → `AIza***`
/// - GitHub: `ghp_...`, `github_pat_...` → `<prefix>***`
/// - AWS access keys: `AKIA...`/`ASIA...` → `<prefix>***` (and paired secret keys)
/// - Stripe: `sk_live_...`, `rk_test_...`, `whsec_...` → `<prefix>***`
/// - Bearer JWTs: `Bearer <jwt>` → `Bearer [REDACTED]`
/// - PEM private keys: `-----BEGIN ... PRIVATE KEY----- ...` → `[REDACTED]`
/// - Hex private keys: `PRIVATE_KEY=0x...` → `PRIVATE_KEY=[REDACTED]`
pub fn redact_api_keys(raw: &str) -> String {
    pattern_redactor().redact(raw)
}

static PATTERN_REDACTOR: OnceLock<PatternRedactor> = OnceLock::new();

fn pattern_redactor() -> &'static PatternRedactor {
    PATTERN_REDACTOR.get_or_init(PatternRedactor::new)
}

#[derive(Debug)]
struct PatternRedactor {
    // Multiline / block formats
    pem_private_key_block: Regex,

    // AWS
    aws_access_key_pair: Regex,
    aws_access_key_id: Regex,
    aws_secret_access_key_assignment: Regex,

    // GitHub
    github_token: Regex,
    github_pat_token: Regex,

    // Stripe
    stripe_api_key: Regex,
    stripe_webhook_secret: Regex,

    // Bearer / JWT
    bearer_jwt: Regex,
    jwt: Regex,

    // Private keys
    hex_private_key_assignment: Regex,

    // Provider API keys
    anthropic_key: Regex,
    openai_key: Regex,
    gemini_key: Regex,
}

impl PatternRedactor {
    fn new() -> Self {
        Self {
            pem_private_key_block: Regex::new(
                r"(?s)(-----BEGIN [^-\n]*PRIVATE KEY-----).*?(-----END [^-\n]*PRIVATE KEY-----)",
            )
            .expect("valid PEM private key regex"),

            aws_access_key_pair: Regex::new(
                r"\b((?:AKIA|ASIA|AIDA|AROA|AGPA|AIPA|ANPA|ANVA))[A-Z0-9]{16}(\s+)[A-Za-z0-9/+=]{40}\b",
            )
            .expect("valid AWS access key pair regex"),
            aws_access_key_id: Regex::new(
                r"\b((?:AKIA|ASIA|AIDA|AROA|AGPA|AIPA|ANPA|ANVA))[A-Z0-9]{16}\b",
            )
            .expect("valid AWS access key id regex"),
            aws_secret_access_key_assignment: Regex::new(
                r"(?i)\b(aws_secret_access_key)(\s*[:=]\s*)[A-Za-z0-9/+=]{40}\b",
            )
            .expect("valid AWS secret access key assignment regex"),

            github_token: Regex::new(r"\b(gh(?:p|o|u|s|r)_)[A-Za-z0-9]{20,}\b")
                .expect("valid GitHub token regex"),
            github_pat_token: Regex::new(r"\b(github_pat_)[A-Za-z0-9_]{20,}\b")
                .expect("valid GitHub fine-grained PAT regex"),

            stripe_api_key: Regex::new(
                r"\b((?:sk|rk|pk)_(?:test|live)_)[A-Za-z0-9]{10,}\b",
            )
            .expect("valid Stripe API key regex"),
            stripe_webhook_secret: Regex::new(r"\b(whsec_)[A-Za-z0-9]{10,}\b")
                .expect("valid Stripe webhook secret regex"),

            bearer_jwt: Regex::new(
                r"(?i)\b(Bearer)(\s+)[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+){2,}",
            )
            .expect("valid Bearer JWT regex"),
            jwt: Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")
                .expect("valid JWT regex"),

            hex_private_key_assignment: Regex::new(
                r"(?i)\b(PRIVATE_KEY)(\s*[:=]\s*)0x[0-9a-f]{64}\b",
            )
            .expect("valid hex private key assignment regex"),

            // Provider API keys
            anthropic_key: Regex::new(r"sk-ant-[A-Za-z0-9_-]+")
                .expect("valid Anthropic API key regex"),
            // OpenAI keys are `sk-...`, but **do not** match the Anthropic prefix (`sk-ant-...`),
            // otherwise we'd double-redact `sk-ant-***` into `sk-******`.
            openai_key: Regex::new(
                r"sk-(?:[^a][A-Za-z0-9_-]*|a[^n][A-Za-z0-9_-]*|an[^t][A-Za-z0-9_-]*|ant[^-][A-Za-z0-9_-]*)",
            )
            .expect("valid OpenAI API key regex"),
            gemini_key: Regex::new(r"AIza[0-9A-Za-z_-]+").expect("valid Gemini API key regex"),
        }
    }

    fn redact(&self, raw: &str) -> String {
        let mut output = raw.to_string();

        // More-specific patterns first (avoid leaving partially redacted secrets behind).
        apply_if_match(
            &self.pem_private_key_block,
            "$1\n[REDACTED]\n$2",
            &mut output,
        );

        apply_if_match(&self.aws_access_key_pair, "$1***$2[REDACTED]", &mut output);
        apply_if_match(
            &self.aws_secret_access_key_assignment,
            "$1$2[REDACTED]",
            &mut output,
        );
        apply_if_match(&self.aws_access_key_id, "$1***", &mut output);

        apply_if_match(&self.github_pat_token, "$1***", &mut output);
        apply_if_match(&self.github_token, "$1***", &mut output);

        apply_if_match(&self.stripe_webhook_secret, "$1***", &mut output);
        apply_if_match(&self.stripe_api_key, "$1***", &mut output);

        apply_if_match(&self.bearer_jwt, "$1$2[REDACTED]", &mut output);
        apply_if_match(&self.jwt, "[REDACTED]", &mut output);

        apply_if_match(
            &self.hex_private_key_assignment,
            "$1$2[REDACTED]",
            &mut output,
        );

        // Provider keys last; `sk-ant-` must run before `sk-`.
        apply_if_match(&self.anthropic_key, "sk-ant-***", &mut output);
        apply_if_match(&self.openai_key, "sk-***", &mut output);
        apply_if_match(&self.gemini_key, "AIza***", &mut output);

        output
    }
}

fn apply_if_match(re: &Regex, replacement: &str, output: &mut String) {
    if !re.is_match(output) {
        return;
    }
    let replaced = re.replace_all(output.as_str(), replacement).into_owned();
    *output = replaced;
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
    fn redact_api_keys_replaces_anthropic_key() {
        let input = "Error: sk-ant-api03-abc123xyz key invalid";
        let output = redact_api_keys(input);
        assert_eq!(output, "Error: sk-ant-*** key invalid");
    }

    #[test]
    fn redact_api_keys_replaces_gemini_key() {
        let input = "Error: AIzaSyC-abc123xyz key invalid";
        let output = redact_api_keys(input);
        assert_eq!(output, "Error: AIza*** key invalid");
    }

    #[test]
    fn redact_api_keys_handles_gemini_in_url() {
        let input = "https://generativelanguage.googleapis.com/v1beta?key=AIzaSyC123";
        let output = redact_api_keys(input);
        assert_eq!(
            output,
            "https://generativelanguage.googleapis.com/v1beta?key=AIza***"
        );
    }

    #[test]
    fn redact_api_keys_handles_mixed_keys() {
        let input = "anthropic: sk-ant-abc, openai: sk-xyz, google: AIzaSyC123";
        let output = redact_api_keys(input);
        assert_eq!(
            output,
            "anthropic: sk-ant-***, openai: sk-***, google: AIza***"
        );
    }

    #[test]
    fn redact_api_keys_redacts_github_tokens() {
        let input = "token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef12 and github_pat_1234567890_abcdefghijklmnopqrstuvwxyz";
        let output = redact_api_keys(input);
        assert!(output.contains("ghp_***"));
        assert!(output.contains("github_pat_***"));
        assert!(!output.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef12"));
        assert!(!output.contains("github_pat_1234567890_abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn redact_api_keys_redacts_aws_access_key_id() {
        let input = "AWS access key: AKIAIOSFODNN7EXAMPLE";
        let output = redact_api_keys(input);
        assert_eq!(output, "AWS access key: AKIA***");
    }

    #[test]
    fn redact_api_keys_redacts_aws_access_key_pair() {
        let input = "AWS AKIAIOSFODNN7EXAMPLE wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let output = redact_api_keys(input);
        assert!(output.contains("AKIA***"));
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"));
    }

    #[test]
    fn redact_api_keys_redacts_stripe_keys() {
        let input = "stripe=sk_live_51HG3bEK7ypQ9a0b1c2d3e4f5g6h7i8j9k0lmnopqrstuvwx whsec_1234567890abcdefghijklmnopqrstuvwxyz";
        let output = redact_api_keys(input);
        assert!(output.contains("sk_live_***"));
        assert!(output.contains("whsec_***"));
        assert!(!output.contains("sk_live_51HG3bEK7ypQ9a0b1c2d3e4f5g6h7i8j9k0lmnopqrstuvwx"));
    }

    #[test]
    fn redact_api_keys_redacts_bearer_jwt() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let output = redact_api_keys(input);
        assert!(output.contains("Bearer [REDACTED]"));
        assert!(!output.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."));
    }

    #[test]
    fn redact_api_keys_redacts_pem_private_key_block() {
        let input =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3V\n-----END RSA PRIVATE KEY-----";
        let output = redact_api_keys(input);
        assert!(output.contains("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(output.contains("[REDACTED]"));
        assert!(output.contains("-----END RSA PRIVATE KEY-----"));
        assert!(!output.contains("MIIEpAIBAAKCAQEA0Z3V"));
    }

    #[test]
    fn redact_api_keys_redacts_hex_private_key_assignment() {
        let key = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let input = format!("PRIVATE_KEY={key}");
        let output = redact_api_keys(&input);
        assert_eq!(output, "PRIVATE_KEY=[REDACTED]");
    }

    #[test]
    fn redact_api_keys_preserves_ai_without_za() {
        // "AI" alone or "AIx" should not be redacted
        let input = "AI is cool and AIx is also cool";
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

    #[test]
    fn sanitize_stream_error_redacts_gemini_key() {
        let input = "API error with AIzaSyC-secretkey123";
        let output = sanitize_stream_error(input);
        assert!(output.contains("AIza***"));
        assert!(!output.contains("AIzaSyC-secretkey123"));
    }

    // SecretRedactor tests

    #[test]
    fn secret_redactor_redacts_known_value() {
        let secrets = vec!["super_secret_value_12345".to_string()];
        let ac = AhoCorasick::new(&secrets).unwrap();
        let redactor = SecretRedactor {
            automaton: Some(ac),
            secret_count: 1,
        };

        let input = "Error: auth failed with super_secret_value_12345";
        assert_eq!(redactor.redact(input), "Error: auth failed with [REDACTED]");
    }

    #[test]
    fn secret_redactor_handles_multiple_occurrences() {
        let secrets = vec!["secret_token_abcd1234".to_string()];
        let ac = AhoCorasick::new(&secrets).unwrap();
        let redactor = SecretRedactor {
            automaton: Some(ac),
            secret_count: 1,
        };

        let input = "key1=secret_token_abcd1234, key2=secret_token_abcd1234";
        assert_eq!(redactor.redact(input), "key1=[REDACTED], key2=[REDACTED]");
    }

    #[test]
    fn secret_redactor_handles_empty() {
        let redactor = SecretRedactor {
            automaton: None,
            secret_count: 0,
        };

        let input = "No secrets here";
        assert_eq!(redactor.redact(input), input);
    }

    #[test]
    fn secret_redactor_handles_multiple_secrets() {
        let secrets = vec![
            "first_secret_value_1234".to_string(),
            "second_secret_val_5678".to_string(),
        ];
        let ac = AhoCorasick::new(&secrets).unwrap();
        let redactor = SecretRedactor {
            automaton: Some(ac),
            secret_count: 2,
        };

        let input = "Found first_secret_value_1234 and second_secret_val_5678";
        assert_eq!(redactor.redact(input), "Found [REDACTED] and [REDACTED]");
    }

    #[test]
    fn looks_like_non_secret_skips_unix_paths() {
        assert!(looks_like_non_secret("/usr/local/bin/something"));
        assert!(looks_like_non_secret("/home/user/.config/app"));
    }

    #[test]
    fn looks_like_non_secret_skips_windows_paths() {
        assert!(looks_like_non_secret("C:\\Program Files\\App"));
        assert!(looks_like_non_secret("D:\\Users\\Config\\settings"));
    }

    #[test]
    fn looks_like_non_secret_skips_plain_urls() {
        assert!(looks_like_non_secret("https://api.example.com/v1"));
        assert!(looks_like_non_secret("http://localhost:8080/health"));
    }

    #[test]
    fn looks_like_non_secret_keeps_urls_with_credentials() {
        assert!(!looks_like_non_secret("https://api.example.com?token=abc"));
        assert!(!looks_like_non_secret("https://api.example.com?key=xyz"));
        assert!(!looks_like_non_secret("https://api.example.com?secret=123"));
        assert!(!looks_like_non_secret(
            "https://api.example.com?password=pw"
        ));
    }

    #[test]
    fn looks_like_non_secret_skips_pure_numeric() {
        assert!(looks_like_non_secret("1234567890123456"));
        assert!(looks_like_non_secret("12345"));
    }

    #[test]
    fn looks_like_non_secret_keeps_alphanumeric() {
        assert!(!looks_like_non_secret("abc123def456ghi789"));
        assert!(!looks_like_non_secret("sk-proj-abc123xyz"));
    }

    #[test]
    fn secret_redactor_debug_hides_secrets() {
        let secrets = vec!["super_secret_12345678".to_string()];
        let ac = AhoCorasick::new(&secrets).unwrap();
        let redactor = SecretRedactor {
            automaton: Some(ac),
            secret_count: 1,
        };

        let debug_output = format!("{redactor:?}");
        assert!(debug_output.contains("secret_count: 1"));
        assert!(!debug_output.contains("super_secret"));
    }

    #[test]
    fn secret_redactor_has_secrets_reports_correctly() {
        let empty_redactor = SecretRedactor {
            automaton: None,
            secret_count: 0,
        };
        assert!(!empty_redactor.has_secrets());

        let populated_redactor = SecretRedactor {
            automaton: Some(AhoCorasick::new(["secret123456789ab"]).unwrap()),
            secret_count: 1,
        };
        assert!(populated_redactor.has_secrets());
    }
}

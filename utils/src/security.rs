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
//! and cached in a `OnceLock`. Pattern ownership lives in `forge_types::ENV_CREDENTIAL_PATTERNS`
//! per IFA-7 (single point of encoding).

use std::sync::OnceLock;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use forge_types::{ENV_CREDENTIAL_PATTERNS, sanitize_terminal_text, strip_steganographic_chars};
use globset::{GlobBuilder, GlobSetBuilder};
use regex::Regex;

/// Minimum length for env var values to be considered secrets.
/// Avoids false positives on short values ("true", "1", "yes").
const MIN_SECRET_LENGTH: usize = 16;

/// Runtime secret redactor built from environment variables.
///
/// Constructed via single Authority Boundary ([`from_env`](Self::from_env)),
/// cached in `OnceLock`. Secrets are never logged or exposed via Debug.
pub struct SecretRedactor {
    secrets: Vec<String>,
    automaton: Option<AhoCorasick>,
}

impl std::fmt::Debug for SecretRedactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRedactor")
            .field("secret_count", &self.secrets.len())
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

        // Ensure deterministic order, and ensure fallback redaction prefers longer matches.
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();

        let secret_count = secrets.len();
        let automaton = if secrets.is_empty() {
            None
        } else {
            let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
            match AhoCorasickBuilder::new()
                .match_kind(MatchKind::LeftmostLongest)
                .build(&patterns)
            {
                Ok(ac) => Some(ac),
                Err(e) => {
                    tracing::warn!(
                        secret_count,
                        "SecretRedactor automaton build failed; using fallback redaction ({e})"
                    );
                    None
                }
            }
        };

        tracing::debug!(secret_count, "SecretRedactor initialized");
        Self { secrets, automaton }
    }

    ///
    /// Returns the input with all detected secrets replaced by `[REDACTED]`.
    #[must_use]
    pub fn redact<'a>(&self, input: &'a str) -> std::borrow::Cow<'a, str> {
        if self.secrets.is_empty() {
            return std::borrow::Cow::Borrowed(input);
        }

        if let Some(ac) = &self.automaton {
            let mut result = String::with_capacity(input.len());
            ac.replace_all_with(input, &mut result, |_, _, dst| {
                dst.push_str("[REDACTED]");
                true
            });
            return std::borrow::Cow::Owned(result);
        }

        // Fail-closed fallback: sequential replacement (longest-first order already ensured).
        let mut output: Option<String> = None;
        for secret in &self.secrets {
            let haystack = output.as_deref().unwrap_or(input);
            if !haystack.contains(secret) {
                continue;
            }
            output = Some(haystack.replace(secret, "[REDACTED]"));
        }

        output.map_or_else(
            || std::borrow::Cow::Borrowed(input),
            std::borrow::Cow::Owned,
        )
    }

    #[cfg(test)]
    #[must_use]
    pub fn has_secrets(&self) -> bool {
        !self.secrets.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub fn secret_count(&self) -> usize {
        self.secrets.len()
    }
}

fn build_var_name_matcher() -> globset::GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in ENV_CREDENTIAL_PATTERNS {
        if let Ok(glob) = GlobBuilder::new(pattern).case_insensitive(true).build() {
            builder.add(glob);
        }
    }
    builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap())
}

/// Check if a value looks like a non-secret (path, URL, numeric).
///
/// This runs at startup during `SecretRedactor::from_env()`, so filesystem
/// I/O is acceptable. The heuristic is deliberately conservative: if in
/// doubt, the value is treated as a potential secret (returns false).
fn looks_like_non_secret(value: &str) -> bool {
    // File paths — only skip if they actually exist on disk.
    // Without the existence check, a secret like "/AKIAxyz..." would be whitelisted.
    let bytes = value.as_bytes();
    let is_windows_drive_path =
        bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'\\' && bytes[0].is_ascii_alphabetic();

    if value.starts_with('/') || is_windows_drive_path {
        return std::path::Path::new(value).exists();
    }

    // Plain URLs without credentials — case-insensitive parameter matching
    // to catch `Token=`, `AUTH=`, non-standard param names, etc.
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        // URLs with userinfo (user:pass@host) are almost certainly secrets.
        let scheme_end = if lower.starts_with("http://") {
            "http://".len()
        } else {
            "https://".len()
        };
        let after_scheme = &value[scheme_end..];
        let authority = after_scheme
            .split_once('/')
            .map_or(after_scheme, |(authority, _)| authority);
        if authority.contains('@') {
            return false;
        }

        let has_credential_param = [
            "token=",
            "key=",
            "secret=",
            "password=",
            "auth=",
            "bearer=",
            "credential=",
            "api_key=",
            "apikey=",
            "access_token=",
            "client_secret=",
            "private_key=",
        ]
        .iter()
        .any(|param| lower.contains(param));

        return !has_credential_param;
    }

    // Pure numeric values — only skip very long pure-digit strings (20+ digits).
    // Shorter numeric values that passed MIN_SECRET_LENGTH are more likely to be
    // API-issued numeric IDs than secrets.
    if value.chars().all(|c| c.is_ascii_digit()) && value.len() >= 20 {
        return true;
    }

    false
}

fn normalize_untrusted(input: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    let terminal_safe = sanitize_terminal_text(input);
    match strip_steganographic_chars(terminal_safe.as_ref()) {
        Cow::Borrowed(_) => terminal_safe,
        Cow::Owned(stripped) => Cow::Owned(stripped),
    }
}

fn sanitize_impl(raw: &str) -> String {
    let normalized = normalize_untrusted(raw);
    let pattern_redacted = redact_api_keys(normalized.as_ref());
    match secret_redactor().redact(&pattern_redacted) {
        std::borrow::Cow::Borrowed(_) => pattern_redacted,
        std::borrow::Cow::Owned(v) => v,
    }
}

/// Single global instance (IFA-7 compliant).
///
/// Sanitize untrusted external text for terminal display.
///
/// Applies four sanitization passes, in order:
/// 1. Strip terminal escape sequences and control chars (`sanitize_terminal_text`)
/// 2. Strip steganographic Unicode (`strip_steganographic_chars`)
/// 3. Redact pattern-based API keys (OpenAI `sk-*`, Anthropic `sk-ant-*`, Gemini `AIza*`)
/// 4. Redact value-based secrets from environment variables
///
/// Important: normalization (1–2) MUST run before redaction. If redaction runs first,
/// an attacker can split a secret with invisible characters so it won't match, then
/// have normalization "snap" it back into a visible token.
#[must_use]
pub fn sanitize_display_text(input: &str) -> String {
    sanitize_impl(input)
}

static SECRET_REDACTOR: OnceLock<SecretRedactor> = OnceLock::new();

/// Initializes on first call by scanning environment variables.
pub fn secret_redactor() -> &'static SecretRedactor {
    SECRET_REDACTOR.get_or_init(SecretRedactor::from_env)
}

/// Sanitize a stream error message by redacting secrets and stripping controls.
///
/// Applies four sanitization passes, in order:
/// 1. Trim whitespace
/// 2. Normalize untrusted text (terminal escape stripping + stego stripping)
/// 3. Redact pattern-based API keys (OpenAI `sk-*`, Anthropic `sk-ant-*`, Gemini `AIza*`)
/// 4. Redact value-based secrets from environment variables
#[must_use]
pub fn sanitize_stream_error(raw: &str) -> String {
    sanitize_impl(raw.trim())
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
#[must_use]
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
            // OpenAI keys are `sk-...` with 40+ chars after the prefix.
            // Excludes `sk-ant-` (Anthropic) to avoid double-redaction.
            // Requires ≥20 chars after `sk-` to prevent false positives on
            // natural language containing `sk-` fragments.
            openai_key: Regex::new(
                r"sk-(?:[^a][A-Za-z0-9_-]{19,}|a[^n][A-Za-z0-9_-]{18,}|an[^t][A-Za-z0-9_-]{17,}|ant[^-][A-Za-z0-9_-]{16,})",
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
    use aho_corasick::{AhoCorasickBuilder, MatchKind};

    use super::{
        SecretRedactor, looks_like_non_secret, normalize_untrusted, redact_api_keys,
        sanitize_stream_error,
    };

    #[test]
    fn redact_api_keys_replaces_openai_key() {
        let input = "Error: sk-proj-abc123def456ghi789jkl key invalid";
        let output = redact_api_keys(input);
        assert_eq!(output, "Error: sk-*** key invalid");
    }

    #[test]
    fn redact_api_keys_handles_quoted_key() {
        let input = r#"{"key": "sk-1234567890abcdefghijklmnop"}"#;
        let output = redact_api_keys(input);
        assert_eq!(output, r#"{"key": "sk-***"}"#);
    }

    #[test]
    fn redact_api_keys_handles_multiple_keys() {
        let input = "key1: sk-first1234567890abcdefgh, key2: sk-second1234567890abcdefg";
        let output = redact_api_keys(input);
        assert_eq!(output, "key1: sk-***, key2: sk-***");
    }

    #[test]
    fn redact_api_keys_ignores_short_sk_prefix() {
        let input = "non-task-skipping messages are fine";
        let output = redact_api_keys(input);
        assert_eq!(output, input);
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
        let input =
            "anthropic: sk-ant-abc, openai: sk-proj-abc123def456ghi789jkl, google: AIzaSyC123";
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
        let input = "Error with sk-proj-abc123def456ghi789jkl and \x1b[31mred text\x1b[0m";
        let output = sanitize_stream_error(input);
        assert!(output.contains("sk-***"));
        assert!(!output.contains("sk-proj-abc123def456ghi789jkl"));
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

    #[test]
    fn sanitize_stream_error_redacts_openai_key_split_by_zwsp() {
        let input = "Error: sk-proj-abc123\u{200B}def456ghi789jkl key invalid";
        let output = sanitize_stream_error(input);
        assert!(output.contains("sk-***"));
        assert!(!output.contains("sk-proj-abc123def456ghi789jkl"));
    }

    // SecretRedactor tests

    #[test]
    fn secret_redactor_redacts_known_value() {
        let secrets = vec!["super_secret_value_12345".to_string()];
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let input = "Error: auth failed with super_secret_value_12345";
        assert_eq!(redactor.redact(input), "Error: auth failed with [REDACTED]");
    }

    #[test]
    fn secret_redactor_handles_multiple_occurrences() {
        let secrets = vec!["secret_token_abcd1234".to_string()];
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let input = "key1=secret_token_abcd1234, key2=secret_token_abcd1234";
        assert_eq!(redactor.redact(input), "key1=[REDACTED], key2=[REDACTED]");
    }

    #[test]
    fn secret_redactor_handles_empty() {
        let redactor = SecretRedactor {
            secrets: Vec::new(),
            automaton: None,
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
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let input = "Found first_secret_value_1234 and second_secret_val_5678";
        assert_eq!(redactor.redact(input), "Found [REDACTED] and [REDACTED]");
    }

    #[test]
    fn secret_redactor_prefers_longest_match() {
        let secrets = vec!["abc".to_string(), "abcde".to_string()];
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let input = "xx abcde yy";
        assert_eq!(redactor.redact(input), "xx [REDACTED] yy");
    }

    #[test]
    fn secret_redactor_fallback_redacts_when_automaton_missing() {
        let secret = "secret_value_12345".to_string();
        let redactor = SecretRedactor {
            secrets: vec![secret.clone()],
            automaton: None,
        };

        let input = format!("prefix {secret} suffix");
        assert_eq!(redactor.redact(&input), "prefix [REDACTED] suffix");
    }

    #[test]
    fn secret_redactor_redacts_env_secret_split_by_unicode_tags_after_normalization() {
        let secret = "super_secret_value_12345";
        let attacked = "Error: super_secret_value_\u{E0001}12345";
        assert!(!attacked.contains(secret));

        let normalized = normalize_untrusted(attacked);
        let secrets = vec![secret.to_string()];
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let redacted = redactor.redact(normalized.as_ref());
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains(secret));
    }

    #[test]
    fn looks_like_non_secret_only_existing_unix_paths() {
        // Non-existent paths should NOT be skipped
        assert!(!looks_like_non_secret("/nonexistent/path/a1b2c3d4e5f6g7"));
        // Existing paths should be skipped (use a path that exists on all platforms)
        // We can't test this reliably in unit tests without tempdir, so just
        // verify the non-existent case above.
    }

    #[test]
    fn looks_like_non_secret_only_existing_windows_paths() {
        // Non-existent paths should NOT be skipped
        assert!(!looks_like_non_secret(
            "C:\\NonExistent\\Path\\secret123456"
        ));
        assert!(!looks_like_non_secret("D:\\Fake\\Path\\api_key_value_1337"));
        assert!(!looks_like_non_secret("E:\\Fake\\Path\\api_key_value_1337"));
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
    fn looks_like_non_secret_rejects_urls_with_userinfo() {
        assert!(!looks_like_non_secret("https://user:pass@example.com/path"));
        assert!(!looks_like_non_secret("HTTPS://user@example.com/path"));
    }

    #[test]
    fn looks_like_non_secret_catches_new_credential_params() {
        assert!(!looks_like_non_secret("https://webhook.site/?auth=abc123"));
        assert!(!looks_like_non_secret("https://api.example.com?bearer=tok"));
        assert!(!looks_like_non_secret("https://x.com?api_key=val"));
        assert!(!looks_like_non_secret("https://x.com?access_token=val"));
        assert!(!looks_like_non_secret("https://x.com?client_secret=val"));
    }

    #[test]
    fn looks_like_non_secret_url_credential_case_insensitive() {
        assert!(!looks_like_non_secret("https://api.example.com?TOKEN=abc"));
        assert!(!looks_like_non_secret("https://api.example.com?Auth=xyz"));
    }

    #[test]
    fn looks_like_non_secret_skips_long_pure_numeric() {
        // 20+ digits — likely a non-secret identifier
        assert!(looks_like_non_secret("12345678901234567890"));
    }

    #[test]
    fn looks_like_non_secret_keeps_short_numeric() {
        // Short numeric values that passed MIN_SECRET_LENGTH could be secrets
        assert!(!looks_like_non_secret("1234567890123456"));
        assert!(!looks_like_non_secret("12345"));
    }

    #[test]
    fn looks_like_non_secret_keeps_alphanumeric() {
        assert!(!looks_like_non_secret("abc123def456ghi789"));
        assert!(!looks_like_non_secret("sk-proj-abc123xyz"));
    }

    #[test]
    fn secret_redactor_debug_hides_secrets() {
        let secrets = vec!["super_secret_12345678".to_string()];
        let patterns: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .unwrap();
        let redactor = SecretRedactor {
            secrets,
            automaton: Some(ac),
        };

        let debug_output = format!("{redactor:?}");
        assert!(debug_output.contains("secret_count: 1"));
        assert!(!debug_output.contains("super_secret"));
    }

    #[test]
    fn secret_redactor_has_secrets_reports_correctly() {
        let empty_redactor = SecretRedactor {
            secrets: Vec::new(),
            automaton: None,
        };
        assert!(!empty_redactor.has_secrets());

        let secret = "secret123456789ab".to_string();
        let populated_redactor = SecretRedactor {
            secrets: vec![secret.clone()],
            automaton: None,
        };
        assert!(populated_redactor.has_secrets());
        assert_eq!(populated_redactor.secret_count(), 1);
        assert_eq!(populated_redactor.redact(&secret), "[REDACTED]");
    }
}

//! Domain types for `WebFetch`.
//!
//! This module contains all input, output, configuration, and error types
//! as specified in `WEBFETCH_SRD.md` v2.4.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use thiserror::Error;
use url::Url;

use crate::default_true;

/// Input parameters for a `WebFetch` request.
///
/// Per FR-WF-02, the request schema includes:
/// - `url` (required): The URL to fetch
/// - `max_chunk_tokens` (optional): Token budget per chunk [128, 2048]
/// - `cache_preference` (optional): Cache behavior
#[derive(Debug, Clone)]
pub struct WebFetchInput {
    /// The URL to fetch (validated, non-empty).
    url: Url,

    /// Original URL string as provided by caller (for `requested_url` field).
    original_url: String,

    /// Maximum tokens per chunk. Default: config value (typically 600).
    /// Must be in range [128, 2048] per FR-WF-02a.
    pub max_chunk_tokens: Option<u32>,

    /// Cache policy for this request.
    pub cache_preference: CachePreference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CachePreference {
    #[default]
    UseCache,
    BypassCache,
}

impl WebFetchInput {
    /// Minimum allowed value for `max_chunk_tokens` (FR-WF-02a).
    pub const MIN_CHUNK_TOKENS: u32 = 128;

    /// Maximum allowed value for `max_chunk_tokens` (FR-WF-02a).
    pub const MAX_CHUNK_TOKENS: u32 = 2048;

    /// # Errors
    ///
    /// Returns `WebFetchError` if:
    /// - URL is empty or whitespace-only (FR-WF-02b)
    /// - URL cannot be parsed (FR-WF-04a)
    pub fn new(url: impl Into<String>) -> Result<Self, WebFetchError> {
        let original = url.into();

        // FR-WF-02b: Empty or whitespace-only URL
        if original.trim().is_empty() {
            return Err(WebFetchError::new(
                ErrorCode::BadArgs,
                "url must not be empty or whitespace-only",
                false,
            )
            .with_detail("field", "url"));
        }

        // FR-WF-04a: Parse URL
        let parsed = Url::parse(&original).map_err(|e| {
            WebFetchError::new(
                ErrorCode::InvalidUrl,
                format!("failed to parse URL: {e}"),
                false,
            )
            .with_detail("url", &original)
        })?;

        Ok(Self {
            url: parsed,
            original_url: original,
            max_chunk_tokens: None,
            cache_preference: CachePreference::UseCache,
        })
    }

    /// # Errors
    ///
    /// Returns error if value is outside [128, 2048] (FR-WF-02a).
    pub fn with_max_chunk_tokens(mut self, tokens: u32) -> Result<Self, WebFetchError> {
        if !(Self::MIN_CHUNK_TOKENS..=Self::MAX_CHUNK_TOKENS).contains(&tokens) {
            return Err(WebFetchError::new(
                ErrorCode::BadArgs,
                format!(
                    "max_chunk_tokens must be in range [{}, {}], got {}",
                    Self::MIN_CHUNK_TOKENS,
                    Self::MAX_CHUNK_TOKENS,
                    tokens
                ),
                false,
            )
            .with_detail("field", "max_chunk_tokens")
            .with_detail("min", Self::MIN_CHUNK_TOKENS.to_string())
            .with_detail("max", Self::MAX_CHUNK_TOKENS.to_string())
            .with_detail("value", tokens.to_string()));
        }
        self.max_chunk_tokens = Some(tokens);
        Ok(self)
    }

    #[must_use]
    pub fn with_cache_preference(mut self, cache_preference: CachePreference) -> Self {
        self.cache_preference = cache_preference;
        self
    }

    #[must_use]
    pub const fn max_chunk_tokens(&self) -> Option<u32> {
        self.max_chunk_tokens
    }

    #[must_use]
    pub const fn cache_preference(&self) -> CachePreference {
        self.cache_preference
    }

    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    #[must_use]
    pub fn original_url(&self) -> &str {
        &self.original_url
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum OutputCompleteness {
    Complete,
    Truncated(TruncationReason),
}

/// Successful response from `WebFetch`.
///
/// Per FR-WF-03, contains fetched content as structured chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchOutput {
    /// Original input URL as provided (unchanged). FR-WF-RESP-URL-01.
    pub requested_url: String,

    /// Canonicalized URL of last fetched URL with fragment removed. FR-WF-RESP-URL-01.
    pub final_url: String,

    /// Original fetch time (RFC3339, second precision). FR-WF-CCH-TS-01.
    pub fetched_at: String,

    /// Page title from `<title>` or first `<h1>`. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Language from `<html lang>` (BCP-47 tag). Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Content chunks. FR-WF-03b.
    pub chunks: Vec<FetchChunk>,

    /// Completeness marker for extracted output.
    pub completeness: OutputCompleteness,

    /// Condition tokens from fetch pipeline. FR-WF-03c.
    pub notes: Vec<Note>,
}

/// A chunk of extracted content.
///
/// Per FR-WF-03b, each chunk contains:
/// - `heading`: Most recent preceding heading (without `#` prefix)
/// - `text`: Markdown content (may include heading line)
/// - `token_count`: Token count of `text` only
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchChunk {
    /// Most recent preceding heading text, or empty string if none.
    pub heading: String,

    /// Chunk content as Markdown.
    pub text: String,

    /// Token count of `text` field only.
    pub token_count: u32,
}

/// Reason for content truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationReason {
    /// Output byte budget enforcement truncated chunks.
    ToolOutputLimit,
}

/// Condition tokens for the `notes` array.
///
/// Per FR-WF-03c and FR-WF-NOTES-ORDER-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    /// HTTP URL was automatically upgraded to HTTPS.
    HttpUpgradedToHttps,
    /// Response served from cache.
    CacheHit,
    /// robots.txt unavailable but `fail_open=true`.
    RobotsUnavailableFailOpen,
    /// Unknown charset; fell back to UTF-8 with replacement.
    CharsetFallback,
    /// Cache write failed (fetch still succeeded).
    CacheWriteFailed,
    /// Output truncated to fit byte budget.
    ToolOutputLimit,
}

impl Note {
    /// Canonical ordering per FR-WF-NOTES-ORDER-01.
    #[must_use]
    pub fn order(&self) -> u8 {
        match self {
            Note::HttpUpgradedToHttps => 1,
            Note::CacheHit => 2,
            Note::RobotsUnavailableFailOpen => 3,
            Note::CharsetFallback => 4,
            Note::CacheWriteFailed => 5,
            Note::ToolOutputLimit => 6,
        }
    }
}

/// `WebFetch` tool configuration.
///
/// Per FR-WF-CFG-01. Maps to `[tools.webfetch]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WebFetchConfig {
    /// Whether the tool is enabled. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// User-Agent string for HTTP requests.
    pub user_agent: Option<String>,

    /// Request timeout in seconds. Default: 20.
    pub timeout_seconds: Option<u32>,

    /// Maximum redirects to follow. Default: 5.
    pub max_redirects: Option<u32>,

    /// Default max tokens per chunk. Default: 600.
    pub default_max_chunk_tokens: Option<u32>,

    /// Cache directory path.
    pub cache_dir: Option<PathBuf>,

    /// Cache TTL in days. Default: 7.
    pub cache_ttl_days: Option<u32>,

    /// Maximum cache entries. Default: 1000.
    pub max_cache_entries: Option<u32>,

    /// Maximum total cache size in bytes. Default: 1 GiB.
    pub max_cache_bytes: Option<u64>,

    /// Maximum download size in bytes. Default: 10 MiB.
    pub max_download_bytes: Option<u64>,

    /// Maximum DNS resolution attempts. Default: 3.
    pub max_dns_attempts: Option<u32>,

    /// robots.txt cache entries. Default: 100.
    pub robots_cache_entries: Option<u32>,

    /// robots.txt cache TTL in hours. Default: 24.
    pub robots_cache_ttl_hours: Option<u32>,

    /// Allow auto-execution without approval prompts.
    #[serde(default)]
    pub allow_auto_execution: bool,

    /// Security-specific configuration.
    pub security: Option<SecurityConfig>,

    /// HTTP-specific configuration.
    pub http: Option<HttpConfig>,

    /// robots.txt-specific configuration.
    pub robots: Option<RobotsConfig>,
}

impl WebFetchConfig {
    /// Default timeout in seconds.
    pub const DEFAULT_TIMEOUT_SECONDS: u32 = 20;

    /// Default max redirects.
    pub const DEFAULT_MAX_REDIRECTS: u32 = 5;

    /// Default max chunk tokens.
    pub const DEFAULT_MAX_CHUNK_TOKENS: u32 = 600;

    /// Default cache TTL in days.
    pub const DEFAULT_CACHE_TTL_DAYS: u32 = 7;

    /// Default max cache entries.
    pub const DEFAULT_MAX_CACHE_ENTRIES: u32 = 1000;

    /// Default max cache bytes (1 GiB).
    pub const DEFAULT_MAX_CACHE_BYTES: u64 = 1024 * 1024 * 1024;

    /// Default max download bytes (10 MiB).
    pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;

    /// Default max DNS attempts.
    pub const DEFAULT_MAX_DNS_ATTEMPTS: u32 = 3;

    /// Default robots.txt cache entries.
    pub const DEFAULT_ROBOTS_CACHE_ENTRIES: u32 = 1024;

    /// Default robots.txt cache TTL in hours.
    pub const DEFAULT_ROBOTS_CACHE_TTL_HOURS: u32 = 24;

    #[must_use]
    pub fn timeout_seconds(&self) -> u32 {
        self.timeout_seconds
            .unwrap_or(Self::DEFAULT_TIMEOUT_SECONDS)
    }

    #[must_use]
    pub fn max_redirects(&self) -> u32 {
        self.max_redirects.unwrap_or(Self::DEFAULT_MAX_REDIRECTS)
    }

    #[must_use]
    pub fn default_max_chunk_tokens(&self) -> u32 {
        self.default_max_chunk_tokens
            .unwrap_or(Self::DEFAULT_MAX_CHUNK_TOKENS)
    }

    #[must_use]
    pub fn max_download_bytes(&self) -> u64 {
        self.max_download_bytes
            .unwrap_or(Self::DEFAULT_MAX_DOWNLOAD_BYTES)
    }
}

/// Security-specific configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// Additional blocked CIDR ranges.
    pub blocked_cidrs: Option<Vec<String>>,

    /// Allowed ports (overrides default allowlist).
    pub allowed_ports: Option<Vec<u16>>,

    /// Allow insecure TLS (for testing only).
    #[serde(default)]
    pub allow_insecure_tls: bool,

    /// Allow local testing relaxations (loopback CIDR and loopback non-default ports).
    ///
    /// This does **not** disable private-network SSRF protections for non-loopback
    /// addresses.
    #[serde(default)]
    pub allow_insecure_overrides: bool,
}

/// HTTP-specific configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct HttpConfig {
    /// Additional request headers.
    pub headers: Option<Vec<(String, String)>>,

    /// Use system proxy settings (`HTTP_PROXY/HTTPS_PROXY`).
    #[serde(default)]
    pub use_system_proxy: bool,

    /// Connect timeout in seconds.
    pub connect_timeout_seconds: Option<u32>,

    /// Read timeout in seconds.
    pub read_timeout_seconds: Option<u32>,
}

/// robots.txt-specific configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RobotsConfig {
    /// User-agent token for robots.txt matching.
    pub user_agent_token: Option<String>,

    /// Fail-open if robots.txt unavailable. Default: false.
    #[serde(default)]
    pub fail_open: bool,
}

/// `WebFetch` error with structured details.
///
/// Per FR-WF-18 and FR-WF-18a, errors contain:
/// - `code`: Stable error code from registry
/// - `message`: Human-readable description
/// - `retryability`: Whether retry may succeed
/// - `details`: Optional error-specific context
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct WebFetchError {
    /// Stable error code.
    pub code: ErrorCode,

    /// Human-readable description.
    pub message: String,

    /// Whether retry may succeed.
    pub retryability: Retryability,

    /// Error-specific context.
    pub details: ErrorDetails,
}

impl WebFetchError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryability: Retryability::from_bool(retryable),
            details: ErrorDetails::default(),
        }
    }

    /// Add a detail field.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.0.push((key.into(), value.into()));
        self
    }

    /// Serialize to JSON for tool output.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "error": true,
            "code": self.code,
            "message": self.message,
            "retryability": self.retryability,
        });

        if !self.details.0.is_empty() {
            let details: serde_json::Map<String, serde_json::Value> = self
                .details
                .0
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            obj["details"] = serde_json::Value::Object(details);
        }

        obj
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    Retryable,
    NotRetryable,
}

impl Retryability {
    #[must_use]
    pub const fn from_bool(retryable: bool) -> Self {
        if retryable {
            Self::Retryable
        } else {
            Self::NotRetryable
        }
    }
}

impl Serialize for WebFetchError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_json().serialize(serializer)
    }
}

/// Error codes per FR-WF-18 registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Invalid request parameters.
    BadArgs,
    /// URL parsing failed.
    InvalidUrl,
    /// Non-http(s) scheme.
    InvalidScheme,
    /// Invalid host (e.g., numeric IP forms).
    InvalidHost,
    /// Port not in allowlist.
    PortBlocked,
    /// SSRF protection triggered.
    SsrfBlocked,
    /// DNS resolution failed.
    DnsFailed,
    /// robots.txt disallows path.
    RobotsDisallowed,
    /// Could not fetch robots.txt.
    RobotsUnavailable,
    /// Max redirects exceeded.
    RedirectLimit,
    /// Request timeout.
    Timeout,
    /// Network/connection error.
    Network,
    /// Response exceeds size limit.
    ResponseTooLarge,
    /// Content-Type not supported.
    UnsupportedContentType,
    /// HTTP 4xx client error.
    Http4xx,
    /// HTTP 5xx server error.
    Http5xx,
    /// HTML extraction failed.
    ExtractionFailed,
    /// Unexpected internal error.
    Internal,
}

impl ErrorCode {
    ///
    /// Note: Some codes have conditional retryability (e.g., `http_4xx` for 408/429).
    #[must_use]
    pub fn default_retryability(&self) -> Retryability {
        if matches!(
            self,
            ErrorCode::DnsFailed
                | ErrorCode::RobotsUnavailable
                | ErrorCode::Timeout
                | ErrorCode::Network
                | ErrorCode::Http5xx
                | ErrorCode::Internal
        ) {
            Retryability::Retryable
        } else {
            Retryability::NotRetryable
        }
    }
}

/// Error details as key-value pairs.
#[derive(Debug, Clone, Default)]
pub struct ErrorDetails(pub Vec<(String, String)>);

/// Timeout phase for detailed error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutPhase {
    /// Timeout during DNS resolution.
    Dns,
    /// Timeout during TCP connection establishment.
    Connect,
    /// Timeout during TLS handshake.
    Tls,
    /// Timeout waiting for response headers.
    Request,
    /// Timeout while reading response body.
    Response,
    /// Timeout budget exhausted across redirect chain.
    Redirect,
    /// Timeout during robots.txt fetch.
    Robots,
}

/// Result of SSRF validation.
#[derive(Debug, Clone)]
pub enum SsrfCheckResult {
    /// URL is safe to fetch.
    Allowed {
        /// Resolved IP addresses (pinned for DNS rebinding protection).
        resolved_ips: Vec<IpAddr>,
    },
    /// URL is blocked.
    Blocked {
        /// Why the URL was blocked.
        reason: SsrfBlockReason,
    },
}

/// Reason for SSRF blocking.
#[derive(Debug, Clone)]
pub enum SsrfBlockReason {
    /// IP matches blocked CIDR.
    BlockedCidr { ip: IpAddr, cidr: String },
    /// Port not in allowlist.
    BlockedPort { port: u16 },
    /// Non-canonical numeric host form.
    NonCanonicalHost { raw_host: String },
    /// Userinfo present in URL.
    UserinfoPresent,
    /// IPv6 zone identifier present.
    Ipv6ZoneId,
}

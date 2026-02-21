//! WebFetch: URL fetching, extraction, chunking, and tool executor.

mod cache;
mod chunk;
mod extract;
mod http;
mod resolved;
mod robots;
pub mod types;

use cache::{Cache, CacheEntry, CacheResult, CacheWriteError};
use resolved::{CachePolicy, ResolvedConfig, ResolvedRequest};
use robots::RobotsResult;

pub use types::{
    CachePreference, ErrorCode, ErrorDetails, FetchChunk, HttpConfig, Note, OutputCompleteness,
    RobotsConfig, SecurityConfig, TruncationReason, WebFetchConfig, WebFetchError, WebFetchInput,
    WebFetchOutput,
};

use serde::Deserialize;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::SystemTime;

use super::{
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, parse_args, redact_distillate,
    sanitize_output,
};

pub async fn fetch(
    input: WebFetchInput,
    config: &WebFetchConfig,
) -> Result<WebFetchOutput, WebFetchError> {
    let mut notes = Vec::new();
    let resolved = ResolvedConfig::from_config(config)?;
    let mut request = ResolvedRequest::from_input(input, &resolved);

    // Upgrade http → https unless insecure overrides are enabled (testing)
    if request.url.scheme() == "http" && !resolved.security.allow_insecure_overrides {
        if request.url.port() == Some(80) {
            let _ = request.url.set_port(None);
        }
        let _ = request.url.set_scheme("https");
        notes.push(Note::HttpUpgradedToHttps);
    }

    let max_chunk_tokens = request.max_chunk_tokens;

    if !matches!(request.cache_preference, CachePreference::BypassCache)
        && let Some(output) = check_cache(&request, &resolved)?
    {
        return Ok(output);
    }

    let resolved_ips = http::validate_url(&request.requested_url, &request.url, &resolved).await?;

    check_robots(&request.url, &resolved, &mut notes).await?;

    let (html, final_url, charset_resolution) =
        fetch_content(&request, &resolved, &resolved_ips, &mut notes).await?;

    let extracted = extract::extract(&html, &final_url)?;

    let chunks = chunk::chunk(&extracted.markdown, max_chunk_tokens);

    let mut fetched_at = cache::format_rfc3339(SystemTime::now());
    if let CachePolicy::Enabled(settings) = &resolved.cache {
        let cache_entry = CacheEntry::new(
            canonicalize_url(&final_url),
            extracted.title.clone(),
            extracted.language.clone(),
            extracted.markdown.clone(),
            settings.ttl,
        );
        fetched_at = cache_entry.fetched_at.clone();
        if write_to_cache(&request.url, &cache_entry, settings).is_err() {
            notes.push(Note::CacheWriteFailed);
        }
    }

    if matches!(
        charset_resolution,
        http::CharsetResolution::HeaderFallbackUtf8 | http::CharsetResolution::DefaultUtf8
    ) {
        notes.push(Note::CharsetFallback);
    }

    notes.sort_by_key(types::Note::order);
    notes.dedup();

    Ok(WebFetchOutput {
        requested_url: request.requested_url,
        final_url: canonicalize_url(&final_url),
        fetched_at,
        title: extracted.title,
        language: extracted.language,
        chunks,
        completeness: OutputCompleteness::Complete,
        notes,
    })
}

fn check_cache(
    request: &ResolvedRequest,
    config: &ResolvedConfig,
) -> Result<Option<WebFetchOutput>, WebFetchError> {
    let CachePolicy::Enabled(settings) = &config.cache else {
        return Ok(None);
    };
    let mut cache = match Cache::new(settings) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    match cache.get(&request.url) {
        CacheResult::Hit(entry) => {
            let chunks = chunk::chunk(&entry.markdown, request.max_chunk_tokens);

            Ok(Some(WebFetchOutput {
                requested_url: request.requested_url.clone(),
                final_url: entry.final_url,
                fetched_at: entry.fetched_at,
                title: entry.title,
                language: entry.language,
                chunks,
                completeness: OutputCompleteness::Complete,
                notes: vec![Note::CacheHit],
            }))
        }
        CacheResult::Miss | CacheResult::VersionMismatch => Ok(None),
    }
}

async fn check_robots(
    url: &url::Url,
    config: &ResolvedConfig,
    notes: &mut Vec<Note>,
) -> Result<(), WebFetchError> {
    let result = robots::check(url, config).await?;

    match result {
        RobotsResult::Allowed => Ok(()),
        RobotsResult::Disallowed { rule } => Err(WebFetchError::new(
            ErrorCode::RobotsDisallowed,
            format!("robots.txt disallows this path: {rule}"),
            false,
        )
        .with_detail("rule", rule)),
        RobotsResult::Unavailable => {
            notes.push(Note::RobotsUnavailableFailOpen);
            Ok(())
        }
    }
}

async fn fetch_content(
    input: &ResolvedRequest,
    config: &ResolvedConfig,
    resolved_ips: &[IpAddr],
    notes: &mut Vec<Note>,
) -> Result<(String, url::Url, http::CharsetResolution), WebFetchError> {
    let response = http::fetch(&input.url, resolved_ips, config, notes).await?;
    let html = decode_body(&response.body, &response.charset_resolution)?;
    Ok((html, response.final_url, response.charset_resolution))
}

fn decode_body(
    body: &[u8],
    charset_resolution: &http::CharsetResolution,
) -> Result<String, WebFetchError> {
    match charset_resolution {
        http::CharsetResolution::Header(charset) | http::CharsetResolution::HtmlMeta(charset) => {
            if matches!(charset.as_str(), "utf-8" | "UTF-8") {
                return String::from_utf8(body.to_vec()).map_err(|e| {
                    WebFetchError::new(
                        ErrorCode::ExtractionFailed,
                        format!("invalid UTF-8 in response body: {e}"),
                        false,
                    )
                });
            }

            tracing::warn!(
                "charset {} not fully supported, using UTF-8 fallback",
                charset
            );
            Ok(String::from_utf8_lossy(body).into_owned())
        }
        http::CharsetResolution::HeaderFallbackUtf8 | http::CharsetResolution::DefaultUtf8 => {
            String::from_utf8(body.to_vec()).map_err(|e| {
                WebFetchError::new(
                    ErrorCode::ExtractionFailed,
                    format!("invalid UTF-8 in response body: {e}"),
                    false,
                )
            })
        }
    }
}

fn write_to_cache(
    url: &url::Url,
    entry: &CacheEntry,
    settings: &resolved::CacheSettings,
) -> Result<(), CacheWriteError> {
    let mut cache =
        Cache::new(settings).map_err(|e| CacheWriteError::Io(io::Error::other(e.message)))?;
    cache.put(url, entry)
}

fn canonicalize_url(url: &url::Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.to_string()
}

const WEBFETCH_TOOL_NAME: &str = "WebFetch";

/// Configuration for the `WebFetch` tool.
#[derive(Debug, Clone)]
pub struct WebFetchToolConfig {
    pub user_agent: Option<String>,
    pub timeout_seconds: u32,
    pub max_redirects: u32,
    pub default_max_chunk_tokens: u32,
    pub max_download_bytes: u64,
    pub cache_dir: Option<PathBuf>,
    pub cache_ttl_days: u32,
}

impl Default for WebFetchToolConfig {
    fn default() -> Self {
        Self {
            user_agent: None,
            timeout_seconds: 20,
            max_redirects: 5,
            default_max_chunk_tokens: 600,
            max_download_bytes: 10 * 1024 * 1024,
            cache_dir: None,
            cache_ttl_days: 7,
        }
    }
}

/// `WebFetch` tool executor.
#[derive(Debug)]
pub struct WebFetchTool {
    config: WebFetchToolConfig,
}

impl WebFetchTool {
    #[must_use]
    pub fn new(config: WebFetchToolConfig) -> Self {
        Self { config }
    }

    fn build_config(&self) -> WebFetchConfig {
        WebFetchConfig {
            user_agent: self.config.user_agent.clone(),
            timeout_seconds: Some(self.config.timeout_seconds),
            max_redirects: Some(self.config.max_redirects),
            default_max_chunk_tokens: Some(self.config.default_max_chunk_tokens),
            max_download_bytes: Some(self.config.max_download_bytes),
            cache_ttl_days: Some(self.config.cache_ttl_days),
            cache_dir: self.config.cache_dir.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    url: String,
    max_chunk_tokens: Option<u32>,
    #[serde(default)]
    cache_preference: CachePreference,
}

impl ToolExecutor for WebFetchTool {
    fn name(&self) -> &'static str {
        WEBFETCH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Fetch a URL and return structured JSON with chunked markdown content"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_chunk_tokens": {
                    "type": "integer",
                    "minimum": 128,
                    "maximum": 2048,
                    "description": "Maximum tokens per content chunk (default: 600)"
                },
                "cache_preference": {
                    "type": "string",
                    "enum": ["use_cache", "bypass_cache"],
                    "default": "use_cache",
                    "description": "Cache behavior for this fetch request"
                }
            },
            "required": ["url"]
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn risk_level(&self, _args: &serde_json::Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: WebFetchArgs = parse_args(args)?;
        let distillate = format!("Fetch URL: {}", typed.url);
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            ctx.allow_truncation = false;
            let typed: WebFetchArgs = parse_args(&args)?;

            if typed.url.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "url must not be empty".to_string(),
                });
            }

            let mut input = WebFetchInput::new(&typed.url).map_err(|e| ToolError::BadArgs {
                message: e.message.clone(),
            })?;

            if let Some(tokens) = typed.max_chunk_tokens {
                input = input
                    .with_max_chunk_tokens(tokens)
                    .map_err(|e| ToolError::BadArgs {
                        message: e.message.clone(),
                    })?;
            }

            input = input.with_cache_preference(typed.cache_preference);

            let config = self.build_config();

            let output = fetch(input, &config)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: WEBFETCH_TOOL_NAME.to_string(),
                    message: e.message.clone(),
                })?;

            let effective_max = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
            let output_str = shrink_output_to_fit(output, effective_max)?;
            Ok(sanitize_output(&output_str))
        })
    }
}

fn serialize_output(output: &WebFetchOutput) -> Result<String, ToolError> {
    serde_json::to_string(output).map_err(|e| ToolError::ExecutionFailed {
        tool: WEBFETCH_TOOL_NAME.to_string(),
        message: e.to_string(),
    })
}

fn mark_tool_truncation(output: &mut WebFetchOutput) {
    output.completeness = OutputCompleteness::Truncated(TruncationReason::ToolOutputLimit);
    if !output.notes.contains(&Note::ToolOutputLimit) {
        output.notes.push(Note::ToolOutputLimit);
        output.notes.sort_by_key(Note::order);
    }
}

fn shrink_output_to_fit(mut output: WebFetchOutput, max_bytes: usize) -> Result<String, ToolError> {
    if max_bytes == 0 {
        return Err(ToolError::ExecutionFailed {
            tool: WEBFETCH_TOOL_NAME.to_string(),
            message: "Output budget too small to serialize WebFetch result".to_string(),
        });
    }

    let mut json = serialize_output(&output)?;
    if json.len() <= max_bytes {
        return Ok(json);
    }

    mark_tool_truncation(&mut output);

    while !output.chunks.is_empty() {
        output.chunks.pop();
        json = serialize_output(&output)?;
        if json.len() <= max_bytes {
            return Ok(json);
        }
    }

    let mut reduced = true;
    while json.len() > max_bytes && reduced {
        reduced = false;
        if output.title.take().is_some() || output.language.take().is_some() {
            reduced = true;
        } else if !output.notes.is_empty() {
            output.notes.clear();
            reduced = true;
        } else if !output.fetched_at.is_empty() {
            output.fetched_at.clear();
            reduced = true;
        } else if !output.requested_url.is_empty() {
            output.requested_url.clear();
            reduced = true;
        } else if !output.final_url.is_empty() {
            output.final_url.clear();
            reduced = true;
        }

        if reduced {
            json = serialize_output(&output)?;
        }
    }

    if json.len() > max_bytes {
        return Err(ToolError::ExecutionFailed {
            tool: WEBFETCH_TOOL_NAME.to_string(),
            message: "Output budget too small to serialize WebFetch result".to_string(),
        });
    }

    Ok(json)
}

#[cfg(test)]
mod unit_tests {
    use super::{
        FetchChunk, OutputCompleteness, WebFetchOutput, WebFetchToolConfig, canonicalize_url,
        shrink_output_to_fit,
    };

    #[test]
    fn default_config() {
        let config = WebFetchToolConfig::default();
        assert_eq!(config.timeout_seconds, 20);
        assert_eq!(config.default_max_chunk_tokens, 600);
    }

    #[test]
    fn shrink_output_to_fit_preserves_valid_json() {
        let chunks = vec![
            FetchChunk {
                heading: "Header 1".to_string(),
                text: "A".repeat(120),
                token_count: 120,
            },
            FetchChunk {
                heading: "Header 2".to_string(),
                text: "B".repeat(120),
                token_count: 120,
            },
            FetchChunk {
                heading: "Header 3".to_string(),
                text: "C".repeat(120),
                token_count: 120,
            },
        ];

        let output = WebFetchOutput {
            requested_url: "https://example.com".to_string(),
            final_url: "https://example.com".to_string(),
            fetched_at: "2025-01-01T00:00:00Z".to_string(),
            title: Some("Example".to_string()),
            language: Some("en".to_string()),
            chunks,
            completeness: OutputCompleteness::Complete,
            notes: Vec::new(),
        };

        let json = shrink_output_to_fit(output, 200).expect("shrink output");
        assert!(json.len() <= 200);
        let parsed: WebFetchOutput = serde_json::from_str(&json).expect("valid json");
        assert!(matches!(
            parsed.completeness,
            OutputCompleteness::Truncated(_)
        ));
    }

    #[test]
    fn test_canonicalize_url() {
        let url = url::Url::parse("https://example.com/page#section").unwrap();
        assert_eq!(canonicalize_url(&url), "https://example.com/page");
    }
}

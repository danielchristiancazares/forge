//! Safe URL fetching with browser fallback for LLM consumption.
//!
//! This crate provides a complete pipeline for fetching web content and converting
//! it into LLM-friendly Markdown chunks. It implements comprehensive security
//! measures (SSRF protection, robots.txt compliance) and intelligent content
//! extraction with boilerplate removal.
//!
//! # Pipeline
//!
//! The fetch pipeline processes URLs through these stages:
//!
//! 1. **Cache check** - Returns cached content if valid (respects TTL, re-chunks on hit)
//! 2. **SSRF validation** - Validates scheme, host, port; resolves DNS with rebinding protection
//! 3. **robots.txt** - RFC 9309 compliant checking with origin-scoped caching
//! 4. **Content fetch** - HTTP request with optional headless browser fallback for SPAs
//! 5. **Extraction** - Boilerplate removal, HTML-to-Markdown conversion
//! 6. **Chunking** - Token-aware splitting with heading context tracking
//! 7. **Cache write** - Stores extracted markdown for future requests
//!
//! # Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`types`] | Input/output types, configuration, structured errors |
//! | [`http`] | HTTP client with SSRF protection and DNS pinning |
//! | [`browser`] | CDP-based Chromium rendering with request interception |
//! | [`robots`] | RFC 9309 parser with wildcard/anchor pattern support |
//! | [`extract`] | HTML-to-Markdown with intelligent boilerplate detection |
//! | [`chunk`] | Token-bounded chunking with code/list block handling |
//! | [`cache`] | LRU disk cache with dual-limit eviction |
//! | [`resolved`] | Internal: config resolution eliminating Option handling |
//!
//! # Usage
//!
//! ```ignore
//! use forge_webfetch::{fetch, WebFetchInput, WebFetchConfig};
//!
//! let input = WebFetchInput::new("https://example.com")?
//!     .with_max_chunk_tokens(800)?
//!     .with_no_cache(false);
//! let config = WebFetchConfig::default();
//! let output = fetch(input, &config).await?;
//!
//! for chunk in &output.chunks {
//!     println!("[{}] {} tokens", chunk.heading, chunk.token_count);
//! }
//! ```
//!
//! # Error Handling
//!
//! All errors are [`WebFetchError`] with stable [`ErrorCode`] variants, human-readable
//! messages, and `retryable` hints. Non-fatal conditions (cache write failures,
//! charset fallbacks) are reported via [`Note`] tokens in the output.

// Allow dead code during scaffold phase for user's modules (http, browser)
#![allow(dead_code)]

mod browser;
mod cache;
mod chunk;
mod extract;
mod http;
mod resolved;
mod robots;
mod types;

use cache::{Cache, CacheEntry, CacheResult, CacheWriteError};
use resolved::{CachePolicy, ResolvedConfig, ResolvedRequest};
use robots::RobotsResult;

pub use types::{
    BrowserConfig, ErrorCode, ErrorDetails, FetchChunk, HttpConfig, Note, RenderingMethod,
    RobotsConfig, SecurityConfig, TruncationReason, WebFetchConfig, WebFetchError, WebFetchInput,
    WebFetchOutput,
};

/// Fetch a URL and return structured, chunked content.
///
/// This is the main entry point for the `WebFetch` tool. It:
///
/// 1. Validates the URL for SSRF protection
/// 2. Checks robots.txt compliance
/// 3. Fetches via HTTP (with optional browser fallback)
/// 4. Extracts readable content as Markdown
/// 5. Chunks content by token budget
/// 6. Caches results for future requests
///
/// # Errors
///
/// Returns `WebFetchError` for:
/// - Invalid URLs or schemes
/// - SSRF-blocked hosts/IPs
/// - robots.txt disallowed paths
/// - Network/timeout errors
/// - Unsupported content types
pub async fn fetch(
    input: WebFetchInput,
    config: &WebFetchConfig,
) -> Result<WebFetchOutput, WebFetchError> {
    let mut notes = Vec::new();
    let resolved = ResolvedConfig::from_config(config)?;
    let request = ResolvedRequest::from_input(input, &resolved);

    let max_chunk_tokens = request.max_chunk_tokens;

    let cache_lookup_method = if request.force_browser {
        cache::RenderingMethod::Browser
    } else {
        cache::RenderingMethod::Http
    };

    if !request.no_cache
        && let Some(output) = check_cache(&request, &resolved, cache_lookup_method)?
    {
        return Ok(output);
    }

    let resolved_ips = http::validate_url(&request.requested_url, &request.url, &resolved).await?;

    check_robots(&request.url, &resolved, &mut notes).await?;

    let (html, final_url, used_browser, dom_truncated, blocked_non_get, charset_fallback) =
        fetch_content(&request, &resolved, &resolved_ips, &mut notes).await?;

    let extracted = extract::extract(&html, &final_url)?;

    let chunks = chunk::chunk(&extracted.markdown, max_chunk_tokens);

    let final_rendering_method = if used_browser {
        RenderingMethod::Browser
    } else {
        RenderingMethod::Http
    };
    let cache_write_method = if used_browser {
        cache::RenderingMethod::Browser
    } else {
        cache_lookup_method
    };

    let mut fetched_at = cache::format_rfc3339(std::time::SystemTime::now());
    if let CachePolicy::Enabled(settings) = &resolved.cache {
        let cache_entry = CacheEntry::new(
            canonicalize_url(&final_url),
            extracted.title.clone(),
            extracted.language.clone(),
            extracted.markdown.clone(),
            settings.ttl,
        );
        fetched_at = cache_entry.fetched_at.clone();
        if let Err(_e) = write_to_cache(&request.url, cache_write_method, &cache_entry, settings) {
            notes.push(Note::CacheWriteFailed);
        }
    }

    if dom_truncated {
        notes.push(Note::BrowserDomTruncated);
    }
    if blocked_non_get {
        notes.push(Note::BrowserBlockedNonGet);
    }
    if charset_fallback {
        notes.push(Note::CharsetFallback);
    }

    // Sort notes by canonical order (FR-WF-NOTES-ORDER-01)
    notes.sort_by_key(types::Note::order);
    notes.dedup();

    Ok(WebFetchOutput {
        requested_url: request.requested_url,
        final_url: canonicalize_url(&final_url),
        fetched_at,
        title: extracted.title,
        language: extracted.language,
        chunks,
        rendering_method: final_rendering_method,
        truncated: dom_truncated,
        truncation_reason: if dom_truncated {
            Some(TruncationReason::BrowserDomTruncated)
        } else {
            None
        },
        notes,
    })
}

fn check_cache(
    request: &ResolvedRequest,
    config: &ResolvedConfig,
    rendering_method: cache::RenderingMethod,
) -> Result<Option<WebFetchOutput>, WebFetchError> {
    let CachePolicy::Enabled(settings) = &config.cache else {
        return Ok(None);
    };
    // Try to create cache - if it fails, treat as miss
    let mut cache = match Cache::new(settings) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    match cache.get(&request.url, rendering_method) {
        CacheResult::Hit(entry) => {
            // Cache already checks expiration, so we can trust the hit

            // Re-chunk with request's max_chunk_tokens
            let chunks = chunk::chunk(&entry.markdown, request.max_chunk_tokens);

            let final_method = match rendering_method {
                cache::RenderingMethod::Http => RenderingMethod::Http,
                cache::RenderingMethod::Browser => RenderingMethod::Browser,
            };

            Ok(Some(WebFetchOutput {
                requested_url: request.requested_url.clone(),
                final_url: entry.final_url,
                fetched_at: entry.fetched_at,
                title: entry.title,
                language: entry.language,
                chunks,
                rendering_method: final_method,
                truncated: false,
                truncation_reason: None,
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
        RobotsResult::Unavailable { error: _ } => {
            notes.push(Note::RobotsUnavailableFailOpen);
            Ok(())
        }
    }
}

async fn fetch_content(
    input: &ResolvedRequest,
    config: &ResolvedConfig,
    resolved_ips: &[std::net::IpAddr],
    notes: &mut Vec<Note>,
) -> Result<(String, url::Url, bool, bool, bool, bool), WebFetchError> {
    if input.force_browser {
        if browser::is_available(config) {
            let response = browser::render(&input.url, config).await?;
            return Ok((
                response.html,
                response.final_url,
                true,
                response.dom_truncated,
                response.blocked_non_get,
                false,
            ));
        }
        // Browser unavailable, fall back to HTTP
        notes.push(Note::BrowserUnavailableUsedHttp);
    }

    let response = http::fetch(&input.url, resolved_ips, config).await?;
    let charset_fallback = response.charset_fallback;

    let html = decode_body(&response.body, response.charset.as_deref())?;

    if !input.force_browser && is_spa_heuristic(&html) && browser::is_available(config) {
        // Try browser fallback for SPA
        if let Ok(browser_response) = browser::render(&input.url, config).await {
            return Ok((
                browser_response.html,
                browser_response.final_url,
                true,
                browser_response.dom_truncated,
                browser_response.blocked_non_get,
                charset_fallback,
            ));
        }
    }

    Ok((
        html,
        response.final_url,
        false,
        false,
        false,
        charset_fallback,
    ))
}

fn decode_body(body: &[u8], charset: Option<&str>) -> Result<String, WebFetchError> {
    match charset {
        Some("utf-8" | "UTF-8") | None => String::from_utf8(body.to_vec()).map_err(|e| {
            WebFetchError::new(
                ErrorCode::ExtractionFailed,
                format!("invalid UTF-8 in response body: {e}"),
                false,
            )
        }),
        Some(other) => {
            // For now, try UTF-8 with lossy conversion
            // TODO: Use encoding_rs for proper charset handling
            tracing::warn!(
                "charset {} not fully supported, using UTF-8 fallback",
                other
            );
            Ok(String::from_utf8_lossy(body).into_owned())
        }
    }
}

/// Check if HTML looks like an SPA that needs JS rendering.
///
/// Heuristics:
/// - Very little text content (< 50 visible chars after stripping tags)
/// - This catches SPA shells with just `<div id="root"></div>`
fn is_spa_heuristic(html: &str) -> bool {
    // We take first 1000 chars to sample, then strip tags
    let text_content: String = html.chars().take(1000).collect();

    // If there's very little content after stripping tags, might be SPA
    let visible_estimate = strip_html_tags(&text_content);
    visible_estimate.trim().len() < 50
}

fn strip_html_tags(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }

    result
}

fn write_to_cache(
    url: &url::Url,
    method: cache::RenderingMethod,
    entry: &CacheEntry,
    settings: &resolved::CacheSettings,
) -> Result<(), CacheWriteError> {
    // Try to create cache - if it fails, return error
    let mut cache =
        Cache::new(settings).map_err(|e| CacheWriteError::Io(std::io::Error::other(e.message)))?;
    cache.put(url, method, entry)
}

/// Canonicalize URL by removing fragment.
fn canonicalize_url(url: &url::Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonicalize_url() {
        let url = url::Url::parse("https://example.com/page#section").unwrap();
        assert_eq!(canonicalize_url(&url), "https://example.com/page");
    }

    #[test]
    fn test_is_spa_heuristic_normal_html() {
        let html = r"<html><body><h1>Hello World</h1><p>This is a normal page with plenty of content that should not trigger the SPA heuristic detection.</p></body></html>";
        assert!(!is_spa_heuristic(html));
    }

    #[test]
    fn test_is_spa_heuristic_minimal_html() {
        let html =
            r#"<html><body><div id="root"></div><script src="app.js"></script></body></html>"#;
        assert!(is_spa_heuristic(html));
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html_tags("<div><span>Test</span></div>"), "Test");
    }
}

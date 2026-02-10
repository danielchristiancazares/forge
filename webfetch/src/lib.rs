//! Safe URL fetching for LLM consumption.

#![allow(dead_code)]

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
    ErrorCode, ErrorDetails, FetchChunk, HttpConfig, Note, RobotsConfig, SecurityConfig,
    TruncationReason, WebFetchConfig, WebFetchError, WebFetchInput, WebFetchOutput,
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

    if !request.no_cache
        && let Some(output) = check_cache(&request, &resolved)?
    {
        return Ok(output);
    }

    let resolved_ips = http::validate_url(&request.requested_url, &request.url, &resolved).await?;

    check_robots(&request.url, &resolved, &mut notes).await?;

    let (html, final_url, charset_fallback) =
        fetch_content(&request, &resolved, &resolved_ips).await?;

    let extracted = extract::extract(&html, &final_url)?;

    let chunks = chunk::chunk(&extracted.markdown, max_chunk_tokens);

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
        if write_to_cache(&request.url, &cache_entry, settings).is_err() {
            notes.push(Note::CacheWriteFailed);
        }
    }

    if charset_fallback {
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
        truncated: false,
        truncation_reason: None,
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
) -> Result<(String, url::Url, bool), WebFetchError> {
    let response = http::fetch(&input.url, resolved_ips, config).await?;
    let charset_fallback = response.charset_fallback;
    let html = decode_body(&response.body, response.charset.as_deref())?;
    Ok((html, response.final_url, charset_fallback))
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

fn write_to_cache(
    url: &url::Url,
    entry: &CacheEntry,
    settings: &resolved::CacheSettings,
) -> Result<(), CacheWriteError> {
    let mut cache =
        Cache::new(settings).map_err(|e| CacheWriteError::Io(std::io::Error::other(e.message)))?;
    cache.put(url, entry)
}

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
}

//! RFC 9309 robots.txt parser and checker.
//!
//! This module implements robots.txt handling per FR-WF-08 through FR-WF-09:
//! - RFC 9309 compliant parsing with most-specific-group-wins semantics
//! - User-agent group matching with case-insensitive substring matching
//! - Allow/Disallow rule matching with path prefix, wildcards, and end anchors
//! - Origin-scoped in-memory caching with TTL

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::LOCATION;
use tokio::sync::RwLock;
use url::Url;

use crate::http;
use crate::resolved::ResolvedConfig;
use crate::types::{ErrorCode, WebFetchError};

/// Maximum robots.txt file size (FR-WF-ROBOTS-SIZE-01).
pub const MAX_ROBOTS_SIZE: usize = 512 * 1024; // 512 KiB

/// Result of robots.txt checking.
#[derive(Debug, Clone)]
pub enum RobotsResult {
    /// Path is allowed.
    Allowed,
    /// Path is disallowed.
    Disallowed { rule: String },
    /// robots.txt unavailable (and fail_open applies).
    Unavailable { error: String },
}

/// Cached robots.txt entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    entry: CachedRobots,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
enum CachedRobots {
    Parsed(Robots),
    AllowAll,
}

/// Global robots.txt cache (origin -> entry).
static CACHE: std::sync::OnceLock<Arc<RwLock<HashMap<String, CacheEntry>>>> =
    std::sync::OnceLock::new();

fn cache() -> &'static Arc<RwLock<HashMap<String, CacheEntry>>> {
    CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Check if a URL is allowed by robots.txt.
///
/// Implements FR-WF-08 through FR-WF-09:
/// 1. Fetch robots.txt for the origin (with caching)
/// 2. Parse per RFC 9309 (most-specific-group-wins)
/// 3. Match user-agent group
/// 4. Evaluate Allow/Disallow rules
pub async fn check(url: &Url, config: &ResolvedConfig) -> Result<RobotsResult, WebFetchError> {
    let fail_open = config.robots.fail_open;
    let user_agent = config.robots.user_agent_token.as_str();
    let cache_entries = config.robots.cache_entries;
    let cache_ttl = config.robots.cache_ttl;

    // Compute origin key for caching
    let origin = compute_origin(url);

    // Check cache first (if enabled)
    if cache_entries > 0 {
        let cache = cache().read().await;
        if let Some(entry) = cache.get(&origin)
            && entry.expires_at > Instant::now()
        {
            return Ok(evaluate_cached(&entry.entry, url.path(), user_agent));
        }
    }

    // Fetch robots.txt
    match fetch_robots(url, config).await {
        Ok(FetchResult::Content(content)) => {
            // Parse and cache
            match parse(&content) {
                Ok(robots) => {
                    let result = robots.check(url.path(), user_agent);
                    if cache_entries > 0 {
                        cache_robots(
                            &origin,
                            CachedRobots::Parsed(robots),
                            cache_ttl,
                            cache_entries,
                        )
                        .await;
                    }
                    Ok(result)
                }
                Err(_) => {
                    // Malformed robots.txt → allow-all, cache it
                    if cache_entries > 0 {
                        cache_robots(&origin, CachedRobots::AllowAll, cache_ttl, cache_entries)
                            .await;
                    }
                    Ok(RobotsResult::Allowed)
                }
            }
        }
        Ok(FetchResult::AllowAll) => {
            // 404 or 4xx → allow-all, cache it
            if cache_entries > 0 {
                cache_robots(&origin, CachedRobots::AllowAll, cache_ttl, cache_entries).await;
            }
            Ok(RobotsResult::Allowed)
        }
        Err(e) => {
            // 5xx or network error
            if fail_open {
                // Don't cache fail_open outcomes
                Ok(RobotsResult::Unavailable {
                    error: e.message.clone(),
                })
            } else {
                Err(e)
            }
        }
    }
}

/// Evaluate a cached robots entry.
fn evaluate_cached(robots: &CachedRobots, path: &str, user_agent: &str) -> RobotsResult {
    match robots {
        CachedRobots::Parsed(r) => r.check(path, user_agent),
        CachedRobots::AllowAll => RobotsResult::Allowed,
    }
}

/// Cache a robots.txt result.
async fn cache_robots(origin: &str, robots: CachedRobots, ttl: Duration, max_entries: usize) {
    let entry = CacheEntry {
        entry: robots,
        expires_at: Instant::now() + ttl,
    };

    let mut cache = cache().write().await;
    cache.insert(origin.to_string(), entry);

    if cache.len() > max_entries {
        let now = Instant::now();
        cache.retain(|_, v| v.expires_at > now);

        if cache.len() > max_entries {
            let mut entries: Vec<_> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.expires_at))
                .collect();
            entries.sort_by_key(|(_, exp)| *exp);
            let excess = cache.len() - max_entries;
            for (key, _) in entries.into_iter().take(excess) {
                cache.remove(&key);
            }
        }
    }
}

/// Compute the origin key for caching.
fn compute_origin(url: &Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("");
    let port = url
        .port_or_known_default()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });

    // Only include port if non-standard
    let default_port = if scheme == "https" { 443 } else { 80 };
    if port == default_port {
        format!("{}://{}", scheme, host)
    } else {
        format!("{}://{}:{}", scheme, host, port)
    }
}

/// Result of fetching robots.txt.
enum FetchResult {
    /// Got content to parse.
    Content(String),
    /// Should allow all (404, 4xx, or other allow-all condition).
    AllowAll,
}

/// Fetch robots.txt for a URL's origin.
///
/// Handles FR-WF-ROBOTS-REDIR-02: Allow http→https redirect, same host/port.
async fn fetch_robots(url: &Url, config: &ResolvedConfig) -> Result<FetchResult, WebFetchError> {
    let robots_url = build_robots_url(url)?;

    let mut current_url = robots_url;
    let origin = compute_origin(&current_url);
    let mut redirect_count = 0u32;
    let deadline = Instant::now() + config.timeout;

    loop {
        let resolved_ips = http::validate_url(current_url.as_str(), &current_url, config).await?;
        let response = match http::send_with_pinning(
            &current_url,
            &resolved_ips,
            reqwest::Method::GET,
            &[],
            config,
            deadline,
        )
        .await
        {
            Ok(resp) => resp,
            Err(err) => return Err(map_robots_error(err, &origin, config)),
        };

        let status = response.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            redirect_count += 1;
            if redirect_count > config.max_redirects {
                return Err(robots_unavailable(&origin, "redirect_limit_exceeded"));
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if location.is_empty() {
                return Err(robots_unavailable(&origin, "invalid_redirect"));
            }

            let next_url = current_url
                .join(location)
                .map_err(|_| robots_unavailable(&origin, "invalid_redirect"))?;

            if !is_valid_robots_redirect(&current_url, &next_url) {
                return Err(robots_unavailable(&origin, "robots_cross_origin_redirect"));
            }

            current_url = next_url;
            continue;
        }

        if status.as_u16() == 404 || status.is_client_error() {
            return Ok(FetchResult::AllowAll);
        }

        if status.is_server_error() {
            return Err(robots_unavailable(
                &origin,
                format!("http_{}", status.as_u16()),
            ));
        }

        if status.is_success() {
            let mut body = Vec::new();
            let mut truncated = false;
            let max_bytes = MAX_ROBOTS_SIZE;

            if let Some(len) = response.content_length()
                && len as usize > max_bytes
            {
                truncated = true;
            }

            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    WebFetchError::new(
                        ErrorCode::Network,
                        format!("robots.txt stream error: {e}"),
                        true,
                    )
                    .with_detail("error", e.to_string())
                })?;

                if body.len() + chunk.len() > max_bytes {
                    let remaining = max_bytes.saturating_sub(body.len());
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }

                body.extend_from_slice(&chunk);
            }

            if truncated {
                trim_incomplete_utf8(&mut body);
                trim_partial_line(&mut body);
            }

            if body.is_empty() {
                return Ok(FetchResult::AllowAll);
            }

            if has_non_utf8_bom(&body) {
                return Ok(FetchResult::AllowAll);
            }

            if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
                body.drain(0..3);
            }

            let text = match std::str::from_utf8(&body) {
                Ok(text) => text,
                Err(_) => return Ok(FetchResult::AllowAll),
            };

            return Ok(FetchResult::Content(text.to_string()));
        }

        return Err(robots_unavailable(
            &origin,
            format!("unexpected_status_{}", status.as_u16()),
        ));
    }
}

/// Build the robots.txt URL for an origin.
fn build_robots_url(url: &Url) -> Result<Url, WebFetchError> {
    let scheme = url.scheme();
    let host = url
        .host_str()
        .ok_or_else(|| WebFetchError::new(ErrorCode::InvalidUrl, "URL has no host", false))?;

    let port = url
        .port_or_known_default()
        .unwrap_or_else(|| if scheme == "https" { 443 } else { 80 });
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port_part = if port == default_port {
        String::new()
    } else {
        format!(":{port}")
    };

    let robots_url_str = format!("{}://{}{}/robots.txt", scheme, host, port_part);

    Url::parse(&robots_url_str)
        .map_err(|e| WebFetchError::new(ErrorCode::InvalidUrl, e.to_string(), false))
}

/// Validate a redirect for robots.txt fetching.
///
/// FR-WF-ROBOTS-REDIR-02: Allow http→https upgrade, but host/port must match exactly.
pub fn is_valid_robots_redirect(original: &Url, redirect: &Url) -> bool {
    // Host must match exactly
    if original.host_str() != redirect.host_str() {
        return false;
    }

    // Check scheme compatibility first
    let scheme_ok = match (original.scheme(), redirect.scheme()) {
        ("http", "http") => true,
        ("http", "https") => true, // Upgrade allowed
        ("https", "https") => true,
        ("https", "http") => false, // Downgrade not allowed
        _ => false,
    };

    if !scheme_ok {
        return false;
    }

    let orig_port = original.port_or_known_default().unwrap_or_else(|| {
        if original.scheme() == "https" {
            443
        } else {
            80
        }
    });
    let redir_port = redirect.port_or_known_default().unwrap_or_else(|| {
        if redirect.scheme() == "https" {
            443
        } else {
            80
        }
    });

    if original.scheme() == "http" && redirect.scheme() == "https" {
        if orig_port == 80 && redir_port == 443 {
            return true;
        }
        return orig_port == redir_port;
    }

    orig_port == redir_port
}

fn robots_unavailable(origin: &str, error: impl Into<String>) -> WebFetchError {
    WebFetchError::new(ErrorCode::RobotsUnavailable, "robots.txt unavailable", true)
        .with_detail("origin", origin.to_string())
        .with_detail("error", error.into())
}

fn map_robots_error(err: WebFetchError, origin: &str, config: &ResolvedConfig) -> WebFetchError {
    match err.code {
        ErrorCode::Timeout => robots_unavailable(origin, err.message)
            .with_detail("timeout_ms", config.timeout.as_millis().to_string()),
        ErrorCode::DnsFailed | ErrorCode::Network => robots_unavailable(origin, err.message),
        _ => err,
    }
}

fn has_non_utf8_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
        || bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF])
        || bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00])
}

fn trim_incomplete_utf8(bytes: &mut Vec<u8>) {
    while !bytes.is_empty() && std::str::from_utf8(bytes).is_err() {
        bytes.pop();
    }
}

fn trim_partial_line(bytes: &mut Vec<u8>) {
    if let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') {
        bytes.truncate(pos + 1);
    } else {
        bytes.clear();
    }
}

/// Parsed robots.txt file.
#[derive(Debug, Clone, Default)]
pub struct Robots {
    /// User-agent groups with their rules (lowercase UA → group).
    groups: HashMap<String, RobotsGroup>,
}

/// A user-agent group with Allow/Disallow rules.
#[derive(Debug, Clone, Default)]
pub struct RobotsGroup {
    /// Allow rules (path patterns).
    allow: Vec<String>,
    /// Disallow rules (path patterns).
    disallow: Vec<String>,
}

impl Robots {
    /// Check if a path is allowed for a user-agent.
    ///
    /// Implements FR-WF-ROBOTS-UA-01 through FR-WF-ROBOTS-UA-03:
    /// - Most specific user-agent group wins (longest substring match)
    /// - `*` group only applies if no named group matches
    /// - Longer rule paths take precedence; Allow wins ties
    pub fn check(&self, path: &str, user_agent: &str) -> RobotsResult {
        // Find matching group (most specific wins)
        let group = self.find_group(user_agent);

        // Evaluate rules
        if let Some(g) = group
            && let Some(disallowed_rule) = g.is_disallowed(path)
        {
            return RobotsResult::Disallowed {
                rule: disallowed_rule,
            };
        }

        RobotsResult::Allowed
    }

    /// Find the most specific matching user-agent group.
    ///
    /// FR-WF-ROBOTS-UA-01: Case-insensitive substring matching.
    /// FR-WF-ROBOTS-UA-02: Longest matching user-agent wins.
    /// FR-WF-ROBOTS-UA-03 / FR-WF-ROBOTS-WILDCARD-01: `*` loses ties against named groups.
    fn find_group(&self, user_agent: &str) -> Option<&RobotsGroup> {
        let ua_lower = user_agent.to_lowercase();

        // Find all matching groups and their specificity
        let mut best_match: Option<(&str, &RobotsGroup)> = None;
        let mut best_len = 0;

        for (group_ua, group) in &self.groups {
            // Skip wildcard for now, handle separately
            if group_ua == "*" {
                continue;
            }

            // Case-insensitive substring match
            let group_ua_lower = group_ua.to_lowercase();
            if ua_lower.contains(&group_ua_lower) {
                // Longer match wins
                if group_ua.len() > best_len {
                    best_match = Some((group_ua, group));
                    best_len = group_ua.len();
                }
            }
        }

        // If we found a named match, use it
        if let Some((_, group)) = best_match {
            return Some(group);
        }

        // Fall back to wildcard group
        self.groups.get("*")
    }
}

impl RobotsGroup {
    /// Check if path is disallowed, returning the matching rule if so.
    ///
    /// FR-WF-ROBOTS-MATCH-01: Path prefix matching.
    /// FR-WF-ROBOTS-MATCH-02: Wildcard (*) support.
    /// FR-WF-ROBOTS-MATCH-03: End anchor ($) support.
    /// FR-WF-ROBOTS-EMPTY-01: Empty Disallow = allow-all.
    /// FR-WF-ROBOTS-EMPTY-02: Empty Allow = matches nothing.
    fn is_disallowed(&self, path: &str) -> Option<String> {
        // Find the longest matching Disallow rule
        let mut disallow_match: Option<(&str, usize)> = None;

        for disallow in &self.disallow {
            // Empty Disallow means "allow all" per FR-WF-ROBOTS-EMPTY-01
            if disallow.is_empty() {
                continue;
            }

            if path_matches(path, disallow) {
                let match_len = effective_length(disallow);
                if disallow_match.is_none() || match_len > disallow_match.unwrap().1 {
                    disallow_match = Some((disallow, match_len));
                }
            }
        }

        // If no Disallow matched, path is allowed
        let (disallow_rule, disallow_len) = disallow_match?;

        // Check if there's a longer or equal Allow rule
        for allow in &self.allow {
            // Empty Allow matches nothing per FR-WF-ROBOTS-EMPTY-02
            if allow.is_empty() {
                continue;
            }

            if path_matches(path, allow) {
                let allow_len = effective_length(allow);
                // Allow wins ties (>=), and longer rules win
                if allow_len >= disallow_len {
                    return None; // Allowed
                }
            }
        }

        Some(disallow_rule.to_string())
    }
}

/// Calculate the effective length of a pattern for comparison.
///
/// Wildcards (*) and anchors ($) don't contribute to length.
fn effective_length(pattern: &str) -> usize {
    pattern.chars().filter(|&c| c != '*' && c != '$').count()
}

/// Check if a path matches a robots.txt rule pattern.
///
/// Supports:
/// - Prefix matching (default)
/// - `*` wildcard (matches any sequence of characters)
/// - `$` end anchor (pattern must match end of path)
fn path_matches(path: &str, pattern: &str) -> bool {
    // Handle end anchor
    let (pattern, anchored) = if let Some(p) = pattern.strip_suffix('$') {
        (p, true)
    } else {
        (pattern, false)
    };

    // If no wildcards, simple prefix match
    if !pattern.contains('*') {
        if anchored {
            path == pattern
        } else {
            path.starts_with(pattern)
        }
    } else {
        // Wildcard matching
        wildcard_match(path, pattern, anchored)
    }
}

/// Match a path against a pattern with wildcards.
fn wildcard_match(path: &str, pattern: &str, anchored: bool) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.is_empty() {
        return true; // Pattern is just "*"
    }

    let mut pos = 0;

    // First part must match at the start
    if !parts[0].is_empty() {
        if !path.starts_with(parts[0]) {
            return false;
        }
        pos = parts[0].len();
    }

    // Middle parts can match anywhere after current position
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue; // Consecutive wildcards
        }
        if let Some(found_pos) = path[pos..].find(part) {
            pos += found_pos + part.len();
        } else {
            return false;
        }
    }

    // Last part handling
    if parts.len() > 1 {
        let last = parts[parts.len() - 1];
        if !last.is_empty() {
            if anchored {
                // Must end with the last part
                if !path.ends_with(last) {
                    return false;
                }
                // And the last part must be after our current position
                let last_start = path.len() - last.len();
                if last_start < pos {
                    return false;
                }
            } else {
                // Just needs to be found after current position
                if !path[pos..].contains(last) {
                    return false;
                }
            }
        } else if anchored {
            // Pattern ends with *, but has $ anchor - must match entire path
            // This is unusual but valid: "/*$" means path must start with /
            // The path matches as long as prefix matched
            return true;
        }
    } else if anchored {
        // No wildcards but anchored - already handled in path_matches
        return path.len() == pattern.len();
    }

    true
}

/// Parse robots.txt content per RFC 9309.
///
/// Implements FR-WF-ROBOTS-PARSE-01:
/// - Permissive line-level parsing
/// - UTF-8 BOM stripping (FR-WF-ROBOTS-BOM-01)
/// - Empty Allow matches nothing (FR-WF-ROBOTS-EMPTY-02)
/// - Empty Disallow allows all (FR-WF-ROBOTS-EMPTY-01)
pub fn parse(content: &str) -> Result<Robots, WebFetchError> {
    // FR-WF-ROBOTS-BOM-01: Strip UTF-8 BOM
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut robots = Robots::default();
    let mut current_agents: Vec<String> = Vec::new();
    let mut in_group = false;

    for line in content.lines() {
        // Strip inline comments
        let line = line.split('#').next().unwrap_or("").trim();

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        // Parse directive
        if let Some((directive, value)) = line.split_once(':') {
            let directive = directive.trim().to_lowercase();
            let value = value.trim();

            match directive.as_str() {
                "user-agent" => {
                    // Starting a new group if we were in one
                    if in_group {
                        current_agents.clear();
                        in_group = false;
                    }
                    // Normalize UA to lowercase for storage
                    current_agents.push(value.to_lowercase());
                }
                "allow" => {
                    in_group = true;
                    for agent in &current_agents {
                        robots
                            .groups
                            .entry(agent.clone())
                            .or_default()
                            .allow
                            .push(value.to_string());
                    }
                }
                "disallow" => {
                    in_group = true;
                    for agent in &current_agents {
                        robots
                            .groups
                            .entry(agent.clone())
                            .or_default()
                            .disallow
                            .push(value.to_string());
                    }
                }
                _ => {
                    // FR-WF-ROBOTS-PARSE-01: Ignore unknown directives
                    // (Sitemap, Crawl-delay, etc.)
                }
            }
        }
    }

    Ok(robots)
}

/// Clear the robots.txt cache (for testing).
#[cfg(test)]
pub async fn clear_cache() {
    let mut cache = cache().write().await;
    cache.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let content = "User-agent: *\nDisallow: /private/\nAllow: /public/";
        let robots = parse(content).unwrap();
        assert!(robots.groups.contains_key("*"));
    }

    #[test]
    fn test_parse_bom() {
        let content = "\u{FEFF}User-agent: *\nDisallow: /";
        let robots = parse(content).unwrap();
        assert!(robots.groups.contains_key("*"));
    }

    #[test]
    fn test_empty_disallow_allows_all() {
        let content = "User-agent: *\nDisallow:";
        let robots = parse(content).unwrap();
        let result = robots.check("/any/path", "*");
        assert!(matches!(result, RobotsResult::Allowed));
    }

    #[test]
    fn test_path_prefix_match() {
        assert!(path_matches("/admin/page", "/admin/"));
        assert!(path_matches("/admin", "/admin"));
        assert!(!path_matches("/administrator", "/admin/"));
    }

    #[test]
    fn test_path_wildcard_match() {
        // Single wildcard
        assert!(path_matches("/admin/secret/page", "/admin/*/page"));
        assert!(path_matches("/a/b/c/d/page", "/a/*/page"));

        // Wildcard at end
        assert!(path_matches("/images/photo.jpg", "/images/*"));
        assert!(path_matches("/images/", "/images/*"));

        // Wildcard at start
        assert!(path_matches("/path/file.php", "*.php"));
    }

    #[test]
    fn test_path_anchor_match() {
        // End anchor
        assert!(path_matches("/path", "/path$"));
        assert!(!path_matches("/path/more", "/path$"));

        // Wildcard + anchor
        assert!(path_matches("/foo.php", "/*.php$"));
        assert!(!path_matches("/foo.php/bar", "/*.php$"));
    }

    #[test]
    fn test_allow_wins_ties() {
        let content = "User-agent: *\nDisallow: /path\nAllow: /path";
        let robots = parse(content).unwrap();
        let result = robots.check("/path", "*");
        // Equal length, Allow wins
        assert!(matches!(result, RobotsResult::Allowed));
    }

    #[test]
    fn test_longer_rule_wins() {
        let content = "User-agent: *\nDisallow: /\nAllow: /public/";
        let robots = parse(content).unwrap();

        // /public/ has longer Allow rule
        let result = robots.check("/public/page", "*");
        assert!(matches!(result, RobotsResult::Allowed));

        // / only has Disallow
        let result = robots.check("/secret", "*");
        assert!(matches!(result, RobotsResult::Disallowed { .. }));
    }

    #[test]
    fn test_ua_substring_match() {
        let content =
            "User-agent: Googlebot\nDisallow: /google-only/\n\nUser-agent: *\nDisallow: /";
        let robots = parse(content).unwrap();

        // "Googlebot-Image" contains "googlebot" (case-insensitive)
        let result = robots.check("/google-only/page", "Googlebot-Image");
        assert!(matches!(result, RobotsResult::Disallowed { .. }));

        // Different bot falls back to * which has Disallow: /
        let result = robots.check("/google-only/page", "Bingbot");
        assert!(matches!(result, RobotsResult::Disallowed { .. })); // Disallowed by * group's "/"

        // Bingbot disallowed from any path due to "Disallow: /"
        let result = robots.check("/other", "Bingbot");
        assert!(matches!(result, RobotsResult::Disallowed { .. }));
    }

    #[test]
    fn test_ua_specificity() {
        let content = "User-agent: Googlebot\nDisallow: /\n\nUser-agent: Googlebot-Image\nAllow: /";
        let robots = parse(content).unwrap();

        // More specific UA wins
        let result = robots.check("/page", "Googlebot-Image");
        assert!(matches!(result, RobotsResult::Allowed));

        // Less specific UA
        let result = robots.check("/page", "Googlebot");
        assert!(matches!(result, RobotsResult::Disallowed { .. }));
    }

    #[test]
    fn test_inline_comments() {
        let content = "User-agent: * # comment\nDisallow: /private/ # another comment";
        let robots = parse(content).unwrap();
        assert!(robots.groups.contains_key("*"));
        let result = robots.check("/private/page", "*");
        assert!(matches!(result, RobotsResult::Disallowed { .. }));
    }

    #[test]
    fn test_robots_redirect_validation() {
        let orig = Url::parse("http://example.com/robots.txt").unwrap();

        // Valid: http → https same host
        let redir = Url::parse("https://example.com/robots.txt").unwrap();
        assert!(is_valid_robots_redirect(&orig, &redir));

        // Valid: http → http same host
        let redir = Url::parse("http://example.com/other").unwrap();
        assert!(is_valid_robots_redirect(&orig, &redir));

        // Invalid: different host
        let redir = Url::parse("http://other.com/robots.txt").unwrap();
        assert!(!is_valid_robots_redirect(&orig, &redir));

        // Invalid: https → http downgrade
        let orig_https = Url::parse("https://example.com/robots.txt").unwrap();
        let redir_http = Url::parse("http://example.com/robots.txt").unwrap();
        assert!(!is_valid_robots_redirect(&orig_https, &redir_http));
    }

    #[test]
    fn test_compute_origin() {
        let url = Url::parse("https://example.com/path/page").unwrap();
        assert_eq!(compute_origin(&url), "https://example.com");

        let url = Url::parse("http://example.com:8080/path").unwrap();
        assert_eq!(compute_origin(&url), "http://example.com:8080");

        // Standard ports omitted
        let url = Url::parse("https://example.com:443/path").unwrap();
        assert_eq!(compute_origin(&url), "https://example.com");
    }

    #[test]
    fn test_effective_length() {
        assert_eq!(effective_length("/admin/"), 7);
        assert_eq!(effective_length("/admin/*"), 7);
        assert_eq!(effective_length("/admin/$"), 7);
        assert_eq!(effective_length("/*.php$"), 5);
    }
}

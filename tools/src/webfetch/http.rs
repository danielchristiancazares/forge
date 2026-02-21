//! HTTP client with SSRF validation.
//!
//! This module implements:
//! - URL validation (FR-WF-04 through FR-WF-04c)
//! - SSRF protection (FR-WF-05 through FR-WF-06a)
//! - DNS pinning for rebinding mitigation (FR-WF-DNS-01)
//! - HTTP fetching with redirect handling (FR-WF-07)
//! - Content-Type detection and validation (FR-WF-10)
use super::resolved::{ResolvedConfig, TimeoutSetting};
use super::types::{ErrorCode, Note, Retryability, SsrfBlockReason, TimeoutPhase, WebFetchError};
use futures_util::StreamExt;
use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue, LOCATION};
use reqwest::redirect::Policy;
use std::cmp::Ordering;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;
use tokio::net::lookup_host;
use tokio::time::timeout;
use url::{Host, Url};

/// Default blocked CIDR ranges for SSRF protection (FR-WF-05).
pub const DEFAULT_BLOCKED_CIDRS: &[&str] = &[
    // IPv4
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "0.0.0.0/8",
    "100.64.0.0/10",
    "192.0.0.0/24",
    "192.0.2.0/24",
    "198.51.100.0/24",
    "203.0.113.0/24",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "255.255.255.255/32",
    // IPv6
    "::1/128",
    "::/128",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
    "2001:db8::/32",
];

/// Validate a URL for SSRF protection.
///
/// Implements FR-WF-04 through FR-WF-06a:
/// 1. Scheme must be http or https
/// 2. No userinfo allowed
/// 3. No IPv6 zone identifiers
/// 4. No non-canonical numeric IP forms
/// 5. Port must be in allowlist
/// 6. Resolved IPs must not match blocked CIDRs
pub async fn validate_url(
    raw_url: &str,
    url: &Url,
    config: &ResolvedConfig,
) -> Result<Vec<IpAddr>, WebFetchError> {
    validate_scheme(url)?;

    // FR-WF-04c: Userinfo rejection
    if url.username() != "" || url.password().is_some() {
        return Err(WebFetchError::new(
            ErrorCode::InvalidUrl,
            "userinfo not allowed in URL",
            false,
        )
        .with_detail("url", raw_url));
    }

    // FR-WF-IPV6-ZONE-01: IPv6 zone identifier rejection
    if let Some(raw_host) = extract_raw_host(raw_url)
        && raw_host.bracketed
        && raw_host.host.contains('%')
    {
        return Err(WebFetchError::new(
            ErrorCode::InvalidUrl,
            "IPv6 zone identifiers are not allowed",
            false,
        )
        .with_detail("host", raw_host.host));
    }

    // FR-WF-04b: Non-canonical numeric host detection
    validate_numeric_host(raw_url, url)?;

    let host = url.host().ok_or_else(|| {
        WebFetchError::new(ErrorCode::InvalidUrl, "URL has no host", false)
            .with_detail("url", raw_url)
    })?;
    let port = port_for_url(url);

    match host {
        Host::Ipv4(ip) => {
            let ip_addr = IpAddr::V4(ip);
            if let Some(cidr) = check_ip_blocked(ip_addr, config)? {
                return Err(ssrf_block_to_error(
                    SsrfBlockReason::BlockedCidr { ip: ip_addr, cidr },
                    config,
                ));
            }
            if !is_port_allowed(port, config, &[ip_addr]) {
                return Err(ssrf_block_to_error(
                    SsrfBlockReason::BlockedPort { port },
                    config,
                ));
            }
            Ok(vec![ip_addr])
        }
        Host::Ipv6(ip) => {
            let ip_addr = IpAddr::V6(ip);
            if let Some(cidr) = check_ip_blocked(ip_addr, config)? {
                return Err(ssrf_block_to_error(
                    SsrfBlockReason::BlockedCidr { ip: ip_addr, cidr },
                    config,
                ));
            }
            if !is_port_allowed(port, config, &[ip_addr]) {
                return Err(ssrf_block_to_error(
                    SsrfBlockReason::BlockedPort { port },
                    config,
                ));
            }
            Ok(vec![ip_addr])
        }
        Host::Domain(name) => {
            let resolved_ips = resolve_and_validate(name, port, config).await?;
            if !is_port_allowed(port, config, &resolved_ips) {
                return Err(ssrf_block_to_error(
                    SsrfBlockReason::BlockedPort { port },
                    config,
                ));
            }
            Ok(resolved_ips)
        }
    }
}

fn validate_scheme(url: &Url) -> Result<(), WebFetchError> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(WebFetchError::new(
            ErrorCode::InvalidScheme,
            format!("scheme '{scheme}' not allowed; only http and https are supported"),
            false,
        )
        .with_detail("scheme", scheme)),
    }
}

fn validate_numeric_host(raw_url: &str, url: &Url) -> Result<(), WebFetchError> {
    if !matches!(url.host(), Some(Host::Ipv4(_))) {
        return Ok(());
    }

    let Some(raw_host) = extract_raw_host(raw_url) else {
        return Ok(());
    };

    if raw_host.bracketed {
        return Ok(());
    }

    if !is_canonical_ipv4(&raw_host.host) {
        return Err(WebFetchError::new(
            ErrorCode::InvalidHost,
            "non-canonical numeric host",
            false,
        )
        .with_detail("host", raw_host.host));
    }

    Ok(())
}

async fn resolve_and_validate(
    host: &str,
    port: u16,
    config: &ResolvedConfig,
) -> Result<Vec<IpAddr>, WebFetchError> {
    let mut addrs = lookup_host((host, port)).await.map_err(|e| {
        WebFetchError::new(
            ErrorCode::DnsFailed,
            format!("dns lookup failed: {e}"),
            true,
        )
        .with_detail("host", host)
        .with_detail("error", e.to_string())
    })?;

    let mut ips: Vec<IpAddr> = addrs.by_ref().map(|addr| addr.ip()).collect();
    if ips.is_empty() {
        return Err(WebFetchError::new(
            ErrorCode::DnsFailed,
            "dns lookup returned no addresses",
            true,
        )
        .with_detail("host", host));
    }

    sort_ips(&mut ips);

    let mut allowed = Vec::new();
    let mut blocked: Vec<(IpAddr, String)> = Vec::new();
    for ip in ips {
        match check_ip_blocked(ip, config)? {
            Some(cidr) => blocked.push((ip, cidr)),
            None => allowed.push(ip),
        }
    }

    if allowed.is_empty()
        && let Some((ip, cidr)) = blocked.first()
    {
        return Err(ssrf_block_to_error(
            SsrfBlockReason::BlockedCidr {
                ip: *ip,
                cidr: cidr.clone(),
            },
            config,
        ));
    }

    Ok(allowed)
}

///
/// Implements FR-WF-10 through FR-WF-10h:
/// - Redirect handling with SSRF validation at each hop
/// - Content-Type detection and validation
/// - Response size limits
/// - Charset handling
pub async fn fetch(
    url: &Url,
    resolved_ips: &[IpAddr],
    config: &ResolvedConfig,
    notes: &mut Vec<Note>,
) -> Result<HttpResponse, WebFetchError> {
    let mut current_url = url.clone();
    let mut current_ips = if !resolved_ips.is_empty() {
        resolved_ips.to_vec()
    } else if let Some(host) = current_url.host_str() {
        let port = port_for_url(&current_url);
        resolve_and_validate(host, port, config).await?
    } else {
        return Err(WebFetchError::new(
            ErrorCode::InvalidUrl,
            "URL has no host",
            false,
        ));
    };

    let max_redirects = config.max_redirects;
    let deadline = Instant::now() + config.timeout;
    let mut redirect_count = 0u32;

    let mut default_headers = Vec::new();
    default_headers.push((
        "Accept".to_string(),
        "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1".to_string(),
    ));
    default_headers.push((
        "Accept-Encoding".to_string(),
        "gzip, deflate, br".to_string(),
    ));
    default_headers.extend(config.http.headers.clone());

    loop {
        let response = send_with_pinning(
            &current_url,
            &current_ips,
            Method::GET,
            &default_headers,
            config,
            deadline,
        )
        .await?;
        let status = response.status();

        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            redirect_count += 1;
            if redirect_count > max_redirects {
                return Err(WebFetchError::new(
                    ErrorCode::RedirectLimit,
                    "redirect limit exceeded",
                    false,
                )
                .with_detail("count", redirect_count.to_string())
                .with_detail("max", max_redirects.to_string()));
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if location.is_empty() {
                return Err(WebFetchError::new(
                    ErrorCode::InvalidUrl,
                    "redirect missing Location header",
                    false,
                )
                .with_detail("url", ""));
            }

            let next_url = current_url.join(location).map_err(|_| {
                WebFetchError::new(
                    ErrorCode::InvalidUrl,
                    "redirect Location could not be resolved",
                    false,
                )
                .with_detail("url", location)
            })?;

            let raw_for_validation = if Url::parse(location).is_ok() {
                location
            } else {
                next_url.as_str()
            };

            current_ips = validate_url(raw_for_validation, &next_url, config).await?;
            current_url = next_url;
            super::check_robots(&current_url, config, notes).await?;
            continue;
        }

        if status.as_u16() == 200 {
            let content_type_header = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok());

            let (media_type, header_charset, requires_sniff) =
                parse_content_type(content_type_header);

            if let Some(ref media_type) = media_type
                && !is_supported_media_type(media_type)
            {
                return Err(WebFetchError::new(
                    ErrorCode::UnsupportedContentType,
                    format!("unsupported content type: {media_type}"),
                    false,
                )
                .with_detail("content_type", media_type));
            }

            let max_bytes = config.max_download_bytes as usize;
            if let Some(len) = response.content_length()
                && len > max_bytes as u64
            {
                return Err(WebFetchError::new(
                    ErrorCode::ResponseTooLarge,
                    "response exceeds size limit",
                    false,
                )
                .with_detail("size", len.to_string())
                .with_detail("max_bytes", max_bytes.to_string()));
            }

            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timeout_error(TimeoutPhase::Response, config));
                }
                let next = timeout(remaining, stream.next())
                    .await
                    .map_err(|_| timeout_error(TimeoutPhase::Response, config))?;
                let Some(chunk) = next else {
                    break;
                };
                let chunk = chunk.map_err(|e| {
                    WebFetchError::new(
                        ErrorCode::Network,
                        format!("response stream error: {e}"),
                        true,
                    )
                    .with_detail("error", e.to_string())
                })?;

                if body.len() + chunk.len() > max_bytes {
                    return Err(WebFetchError::new(
                        ErrorCode::ResponseTooLarge,
                        "response exceeds size limit",
                        false,
                    )
                    .with_detail("size", (body.len() + chunk.len()).to_string())
                    .with_detail("max_bytes", max_bytes.to_string()));
                }

                body.extend_from_slice(&chunk);
            }

            let (final_media_type, is_html) = if requires_sniff {
                match sniff_content_type(&body)? {
                    ContentKind::Html => ("text/html".to_string(), true),
                    ContentKind::Plain => ("text/plain".to_string(), false),
                }
            } else if let Some(mt) = media_type.clone() {
                let is_html = mt.eq_ignore_ascii_case("text/html")
                    || mt.eq_ignore_ascii_case("application/xhtml+xml");
                (mt, is_html)
            } else {
                let kind = sniff_content_type(&body)?;
                match kind {
                    ContentKind::Html => ("text/html".to_string(), true),
                    ContentKind::Plain => ("text/plain".to_string(), false),
                }
            };

            if !is_supported_media_type(&final_media_type) {
                return Err(WebFetchError::new(
                    ErrorCode::UnsupportedContentType,
                    format!("unsupported content type: {final_media_type}"),
                    false,
                )
                .with_detail("content_type", final_media_type));
            }

            let charset_resolution = determine_charset(header_charset, &body, is_html);

            return Ok(HttpResponse {
                final_url: current_url,
                body,
                charset_resolution,
            });
        }

        if status.as_u16() == 204 || status.as_u16() == 304 || status.is_informational() {
            return Err(unexpected_status_error(status.as_u16()));
        }

        if status.is_success() {
            return Err(unexpected_status_error(status.as_u16()));
        }

        if status.is_client_error() {
            let retryability = match status.as_u16() {
                408 | 429 => Retryability::Retryable,
                _ => Retryability::NotRetryable,
            };
            return Err(WebFetchError::new(
                ErrorCode::Http4xx,
                format!("HTTP {}", status.as_u16()),
                matches!(retryability, Retryability::Retryable),
            )
            .with_detail("status", status.as_u16().to_string())
            .with_detail(
                "status_text",
                status.canonical_reason().unwrap_or("").to_string(),
            ));
        }

        if status.is_server_error() {
            return Err(WebFetchError::new(
                ErrorCode::Http5xx,
                format!("HTTP {}", status.as_u16()),
                true,
            )
            .with_detail("status", status.as_u16().to_string())
            .with_detail(
                "status_text",
                status.canonical_reason().unwrap_or("").to_string(),
            ));
        }

        return Err(unexpected_status_error(status.as_u16()));
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    /// Final URL after redirects.
    pub final_url: Url,

    /// Response body bytes.
    pub body: Vec<u8>,

    /// Charset resolution state for response decoding.
    pub charset_resolution: CharsetResolution,
}

#[derive(Debug, Clone)]
pub enum CharsetResolution {
    Header(String),
    HtmlMeta(String),
    HeaderFallbackUtf8,
    DefaultUtf8,
}

#[derive(Clone)]
struct Cidr {
    network: IpAddr,
    prefix: u8,
    text: String,
}

struct RawHostInfo {
    host: String,
    bracketed: bool,
}

enum ContentKind {
    Html,
    Plain,
}

fn port_for_url(url: &Url) -> u16 {
    url.port_or_known_default().unwrap_or(80)
}

fn extract_raw_host(raw_url: &str) -> Option<RawHostInfo> {
    let scheme_idx = raw_url.find("://")?;
    let after_scheme = &raw_url[(scheme_idx + 3)..];
    let end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);

    if let Some(stripped) = host_port.strip_prefix('[')
        && let Some(close_idx) = stripped.find(']')
    {
        let host = stripped[..close_idx].to_string();
        return Some(RawHostInfo {
            host,
            bracketed: true,
        });
    }

    if let Some(colon) = host_port.rfind(':') {
        if host_port[..colon].contains(':') {
            return Some(RawHostInfo {
                host: host_port.to_string(),
                bracketed: false,
            });
        }
        return Some(RawHostInfo {
            host: host_port[..colon].to_string(),
            bracketed: false,
        });
    }

    Some(RawHostInfo {
        host: host_port.to_string(),
        bracketed: false,
    })
}

fn is_canonical_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for part in parts {
        if part.is_empty() {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.len() > 3 {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        let value: u16 = match part.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        if value > 255 {
            return false;
        }
    }
    true
}

fn parse_cidr(text: &str) -> Option<Cidr> {
    let (addr, prefix) = text.split_once('/')?;
    let network = addr.parse::<IpAddr>().ok()?;
    let prefix = prefix.parse::<u8>().ok()?;
    match network {
        IpAddr::V4(_) if prefix > 32 => return None,
        IpAddr::V6(_) if prefix > 128 => return None,
        _ => {}
    }
    Some(Cidr {
        network,
        prefix,
        text: text.to_string(),
    })
}

fn ip_in_cidr(ip: IpAddr, cidr: &Cidr) -> bool {
    match (ip, cidr.network) {
        (IpAddr::V4(ipv4), IpAddr::V4(net)) => {
            prefix_match(&ipv4.octets(), &net.octets(), cidr.prefix)
        }
        (IpAddr::V6(ipv6), IpAddr::V6(net)) => {
            prefix_match(&ipv6.octets(), &net.octets(), cidr.prefix)
        }
        _ => false,
    }
}

fn prefix_match(ip: &[u8], net: &[u8], prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let full = (prefix / 8) as usize;
    let rem = prefix % 8;

    if ip.len() < full || net.len() < full {
        return false;
    }

    if ip[..full] != net[..full] {
        return false;
    }

    if rem == 0 {
        return true;
    }

    let mask = 0xFFu8 << (8 - rem);
    ip[full] & mask == net[full] & mask
}

fn check_ip_blocked(ip: IpAddr, config: &ResolvedConfig) -> Result<Option<String>, WebFetchError> {
    if config.security.allow_insecure_overrides && is_loopback_ip(ip) {
        return Ok(None);
    }

    let mut cidrs = Vec::new();
    for entry in DEFAULT_BLOCKED_CIDRS {
        if let Some(cidr) = parse_cidr(entry) {
            cidrs.push(cidr);
        }
    }

    if !config.security.blocked_cidrs.is_empty() {
        for entry in &config.security.blocked_cidrs {
            let cidr = parse_cidr(entry).ok_or_else(|| {
                WebFetchError::new(
                    ErrorCode::Internal,
                    format!("invalid blocked cidr: {entry}"),
                    false,
                )
                .with_detail("cidr", entry)
            })?;
            cidrs.push(cidr);
        }
    }

    if let IpAddr::V6(v6) = ip
        && let Some(v4) = v6.to_ipv4()
    {
        let mapped = IpAddr::V4(v4);
        for cidr in cidrs.iter().filter(|c| matches!(c.network, IpAddr::V4(_))) {
            if ip_in_cidr(mapped, cidr) {
                return Ok(Some(cidr.text.clone()));
            }
        }
    }

    for cidr in cidrs.iter().filter(|c| {
        matches!(
            (c.network, ip),
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
        )
    }) {
        if ip_in_cidr(ip, cidr) {
            return Ok(Some(cidr.text.clone()));
        }
    }

    Ok(None)
}

fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4().is_some_and(|v4| v4.is_loopback()),
    }
}

fn is_port_allowed(port: u16, config: &ResolvedConfig, resolved_ips: &[IpAddr]) -> bool {
    if config.security.allowed_ports.contains(&port) {
        return true;
    }

    config.security.allow_insecure_overrides
        && !resolved_ips.is_empty()
        && resolved_ips.iter().copied().all(is_loopback_ip)
}

fn sort_ips(ips: &mut [IpAddr]) {
    ips.sort_by(|a, b| match (a, b) {
        (IpAddr::V6(a6), IpAddr::V6(b6)) => a6.octets().cmp(&b6.octets()),
        (IpAddr::V4(a4), IpAddr::V4(b4)) => a4.octets().cmp(&b4.octets()),
        (IpAddr::V6(_), IpAddr::V4(_)) => Ordering::Less,
        (IpAddr::V4(_), IpAddr::V6(_)) => Ordering::Greater,
    });
}

pub(crate) async fn send_with_pinning(
    url: &Url,
    ips: &[IpAddr],
    method: Method,
    headers: &[(String, String)],
    config: &ResolvedConfig,
    deadline: Instant,
) -> Result<reqwest::Response, WebFetchError> {
    let host = url
        .host_str()
        .ok_or_else(|| WebFetchError::new(ErrorCode::InvalidUrl, "URL has no host", false))?;
    let port = port_for_url(url);
    let max_attempts = config.max_dns_attempts as usize;

    let mut first_error: Option<String> = None;
    let mut attempted = Vec::new();

    let is_literal = host.parse::<IpAddr>().is_ok();

    for ip in ips.iter().take(max_attempts) {
        attempted.push(ip.to_string());
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error(TimeoutPhase::Redirect, config));
        }

        let client = build_client(config, host, *ip, port, !is_literal)?;

        let mut request = client.request(method.clone(), url.clone());

        for (k, v) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                request = request.header(name, value);
            }
        }

        let send_future = request.send();
        let response = match timeout(remaining, send_future).await {
            Ok(res) => res,
            Err(_) => return Err(timeout_error(TimeoutPhase::Request, config)),
        };

        match response {
            Ok(resp) => return Ok(resp),
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err.to_string());
                }
                if err.is_timeout() {
                    return Err(timeout_error(TimeoutPhase::Request, config));
                }
            }
        }
    }

    Err(
        WebFetchError::new(ErrorCode::Network, "all connection attempts failed", true)
            .with_detail("error", first_error.unwrap_or_else(|| "unknown".into()))
            .with_detail("attempted_ips", attempted.join(",")),
    )
}

fn build_client(
    config: &ResolvedConfig,
    host: &str,
    ip: IpAddr,
    port: u16,
    pin_dns: bool,
) -> Result<reqwest::Client, WebFetchError> {
    let mut builder = reqwest::Client::builder().redirect(Policy::none());

    builder = builder.user_agent(&config.user_agent);

    let use_system_proxy = config.http.use_system_proxy;
    if !use_system_proxy {
        builder = builder.no_proxy();
    }

    if let TimeoutSetting::Enabled(timeout) = config.http.connect_timeout {
        builder = builder.connect_timeout(timeout);
    }

    if let TimeoutSetting::Enabled(timeout) = config.http.read_timeout {
        builder = builder.timeout(timeout);
    }

    if pin_dns {
        builder = builder.resolve(host, SocketAddr::new(ip, port));
    }

    if config.security.allow_insecure_tls {
        tracing::warn!(
            "allow_insecure_tls is enabled: TLS certificate validation is disabled for all WebFetch calls"
        );
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }

    builder.build().map_err(|e| {
        WebFetchError::new(
            ErrorCode::Internal,
            format!("failed to build HTTP client: {e}"),
            false,
        )
    })
}

fn parse_content_type(header: Option<&str>) -> (Option<String>, Option<String>, bool) {
    let Some(header) = header else {
        return (None, None, true);
    };

    let mut parts = header.split(';');
    let media_type = parts.next().unwrap_or("").trim();
    let media_type = if media_type.is_empty() {
        None
    } else {
        Some(media_type.to_ascii_lowercase())
    };

    let mut charset: Option<String> = None;
    for part in parts {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=')
            && key.trim().eq_ignore_ascii_case("charset")
        {
            let value = value.trim().trim_matches('\"').trim_matches('\'');
            if !value.is_empty() {
                charset = Some(value.to_string());
            }
        }
    }

    (media_type, charset, false)
}

fn is_supported_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "text/html" | "application/xhtml+xml" | "text/plain"
    )
}

fn sniff_content_type(body: &[u8]) -> Result<ContentKind, WebFetchError> {
    let mut offset = 0usize;
    if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
        offset = 3;
    }
    while offset < body.len() && matches!(body[offset], b' ' | b'\t' | b'\r' | b'\n') {
        offset += 1;
    }
    let sniff = &body[offset..body.len().min(offset + 512)];
    if sniff.contains(&0) || has_binary_magic(sniff) {
        return Err(WebFetchError::new(
            ErrorCode::UnsupportedContentType,
            "unsupported content type (binary)",
            false,
        )
        .with_detail("content_type", "sniffed:binary"));
    }

    let prefix = to_ascii_lowercase(sniff);
    if prefix.starts_with(b"<!doctype") || prefix.starts_with(b"<html") {
        Ok(ContentKind::Html)
    } else {
        Ok(ContentKind::Plain)
    }
}

fn has_binary_magic(sniff: &[u8]) -> bool {
    if sniff.starts_with(b"%PDF-") {
        return true;
    }
    if sniff.starts_with(&[0x89, b'P', b'N', b'G']) {
        return true;
    }
    if sniff.starts_with(b"GIF87a") || sniff.starts_with(b"GIF89a") {
        return true;
    }
    if sniff.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    if sniff.starts_with(b"PK\x03\x04") {
        return true;
    }
    if sniff.len() >= 8 && &sniff[4..8] == b"ftyp" {
        return true;
    }
    false
}

fn to_ascii_lowercase(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(u8::to_ascii_lowercase).collect()
}

fn determine_charset(
    header_charset: Option<String>,
    body: &[u8],
    is_html: bool,
) -> CharsetResolution {
    if let Some(header) = header_charset {
        if let Some(norm) = normalize_charset(header) {
            return CharsetResolution::Header(norm.to_string());
        }
        return CharsetResolution::HeaderFallbackUtf8;
    }

    if is_html
        && let Some(meta) = sniff_charset_from_html(body)
        && let Some(norm) = normalize_charset(meta)
    {
        return CharsetResolution::HtmlMeta(norm.to_string());
    }

    CharsetResolution::DefaultUtf8
}

fn normalize_charset(charset: String) -> Option<&'static str> {
    let charset = charset
        .trim()
        .trim_matches('\"')
        .trim_matches('\'')
        .to_ascii_lowercase();
    match charset.as_str() {
        "utf-8" | "utf8" | "utf_8" => Some("utf-8"),
        "iso-8859-1" | "latin1" | "latin-1" => Some("iso-8859-1"),
        "windows-1252" | "cp1252" => Some("windows-1252"),
        _ => None,
    }
}

fn sniff_charset_from_html(body: &[u8]) -> Option<String> {
    let prefix = &body[..body.len().min(1024)];
    let lower = to_ascii_lowercase(prefix);
    let hay = String::from_utf8_lossy(&lower);

    if let Some(idx) = hay.find("charset=") {
        let after = &hay[idx + 8..];
        let value = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ';' && *c != '>')
            .collect::<String>();
        if !value.is_empty() {
            return Some(value);
        }
    }

    if let Some(idx) = hay.find("<meta charset=") {
        let after = &hay[idx + 14..];
        let value = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ';' && *c != '>')
            .collect::<String>();
        if !value.is_empty() {
            return Some(value);
        }
    }

    None
}

pub(crate) fn ssrf_block_to_error(
    reason: SsrfBlockReason,
    config: &ResolvedConfig,
) -> WebFetchError {
    match reason {
        SsrfBlockReason::BlockedPort { port } => WebFetchError::new(
            ErrorCode::PortBlocked,
            format!("port {port} is not allowed"),
            false,
        )
        .with_detail("port", port.to_string())
        .with_detail(
            "allowed_ports",
            config
                .security
                .allowed_ports
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
        SsrfBlockReason::BlockedCidr { ip, cidr } => WebFetchError::new(
            ErrorCode::SsrfBlocked,
            format!("connection to {ip} blocked"),
            false,
        )
        .with_detail("blocked_ip", ip.to_string())
        .with_detail("cidr", cidr),
        SsrfBlockReason::NonCanonicalHost { raw_host } => {
            WebFetchError::new(ErrorCode::InvalidHost, "non-canonical numeric host", false)
                .with_detail("host", raw_host)
        }
        SsrfBlockReason::UserinfoPresent => {
            WebFetchError::new(ErrorCode::InvalidUrl, "userinfo not allowed in URL", false)
        }
        SsrfBlockReason::Ipv6ZoneId => WebFetchError::new(
            ErrorCode::InvalidUrl,
            "IPv6 zone identifiers are not allowed",
            false,
        ),
    }
}

fn unexpected_status_error(status: u16) -> WebFetchError {
    WebFetchError::new(
        ErrorCode::Network,
        format!("unexpected status {status}"),
        true,
    )
    .with_detail("error", "unexpected_status")
    .with_detail("status", status.to_string())
}

fn timeout_error(phase: TimeoutPhase, config: &ResolvedConfig) -> WebFetchError {
    let timeout_ms = config.timeout.as_millis() as u64;
    WebFetchError::new(ErrorCode::Timeout, "request timed out", true)
        .with_detail("timeout_ms", timeout_ms.to_string())
        .with_detail("phase", timeout_phase_label(phase))
}

fn timeout_phase_label(phase: TimeoutPhase) -> &'static str {
    match phase {
        TimeoutPhase::Dns => "dns",
        TimeoutPhase::Connect => "connect",
        TimeoutPhase::Tls => "tls",
        TimeoutPhase::Request => "request",
        TimeoutPhase::Response => "response",
        TimeoutPhase::Redirect => "redirect",
        TimeoutPhase::Robots => "robots",
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Once;

    use super::super::resolved::{DEFAULT_ALLOWED_PORTS, ResolvedConfig};
    use super::super::types::{SecurityConfig, WebFetchConfig};
    use super::{DEFAULT_BLOCKED_CIDRS, check_ip_blocked, is_port_allowed};

    fn enable_insecure_overrides_opt_in_for_tests() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            // SAFETY: test-only global opt-in set once and never mutated.
            unsafe {
                env::set_var("FORGE_WEBFETCH_ALLOW_INSECURE_OVERRIDES", "1");
            }
        });
    }

    fn config_with_insecure_overrides(allow_insecure_overrides: bool) -> ResolvedConfig {
        if allow_insecure_overrides {
            enable_insecure_overrides_opt_in_for_tests();
        }
        ResolvedConfig::from_config(&WebFetchConfig {
            security: Some(SecurityConfig {
                allow_insecure_overrides,
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("resolved config")
    }

    #[test]
    fn test_default_blocked_cidrs() {
        assert!(!DEFAULT_BLOCKED_CIDRS.is_empty());
    }

    #[test]
    fn test_default_allowed_ports() {
        assert!(DEFAULT_ALLOWED_PORTS.contains(&80));
        assert!(DEFAULT_ALLOWED_PORTS.contains(&443));
    }

    #[test]
    fn test_insecure_overrides_do_not_unblock_private_ips() {
        let config = config_with_insecure_overrides(true);
        let blocked = check_ip_blocked(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), &config)
            .expect("cidr evaluation");
        assert!(blocked.is_some());
    }

    #[test]
    fn test_insecure_overrides_allow_loopback_only() {
        let config = config_with_insecure_overrides(true);
        let blocked =
            check_ip_blocked(IpAddr::V4(Ipv4Addr::LOCALHOST), &config).expect("cidr evaluation");
        assert!(blocked.is_none());
    }

    #[test]
    fn test_non_default_ports_allowed_only_for_loopback_with_insecure_overrides() {
        let config = config_with_insecure_overrides(true);
        assert!(is_port_allowed(
            3000,
            &config,
            &[IpAddr::V4(Ipv4Addr::LOCALHOST)]
        ));
        assert!(!is_port_allowed(
            3000,
            &config,
            &[IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]
        ));
    }
}

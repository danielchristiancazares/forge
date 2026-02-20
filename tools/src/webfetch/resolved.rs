//! Invariant-safe configuration resolution.
//!
//! This module transforms optional boundary-level configuration ([`WebFetchConfig`])
//! into concrete internal types ([`ResolvedConfig`], [`ResolvedRequest`]) that have no
//! `Option` fields. This follows the "Parse, don't validate" principle - optional
//! configuration is resolved once at the boundary, and core logic operates on
//! types where all invariants are enforced.
//!
//! # Types
//!
//! - [`ResolvedConfig`]: Fully resolved configuration with concrete values
//! - [`ResolvedRequest`]: Request parameters with resolved defaults
//! - [`CachePolicy`]: Enabled with settings or Disabled
//!
//! # Usage
//!
//! ```ignore
//! let resolved = ResolvedConfig::from_config(&config)?;
//! let request = ResolvedRequest::from_input(input, &resolved);
//! // Now use resolved.timeout, request.max_chunk_tokens, etc.
//! // No Option unwrapping needed in downstream code.
//! ```
use std::path::PathBuf;
use std::time::Duration;

use url::Url;

use super::types::{WebFetchConfig, WebFetchError, WebFetchInput};

pub(crate) const DEFAULT_USER_AGENT: &str = "forge-webfetch/1.0";
pub(crate) const DEFAULT_ALLOWED_PORTS: &[u16] = &[80, 443];
const ENV_ALLOW_INSECURE_TLS: &str = "FORGE_WEBFETCH_ALLOW_INSECURE_TLS";
const ENV_ALLOW_INSECURE_OVERRIDES: &str = "FORGE_WEBFETCH_ALLOW_INSECURE_OVERRIDES";

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRequest {
    pub url: Url,
    pub requested_url: String,
    pub max_chunk_tokens: u32,
    pub no_cache: bool,
}

impl ResolvedRequest {
    pub fn from_input(input: WebFetchInput, config: &ResolvedConfig) -> Self {
        let max_chunk_tokens = input
            .max_chunk_tokens
            .unwrap_or(config.default_max_chunk_tokens);
        Self {
            url: input.url().clone(),
            requested_url: input.original_url().to_string(),
            max_chunk_tokens,
            no_cache: input.no_cache,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedConfig {
    pub user_agent: String,
    pub timeout: Duration,
    pub max_redirects: u32,
    pub default_max_chunk_tokens: u32,
    pub max_download_bytes: u64,
    pub max_dns_attempts: u32,
    pub cache: CachePolicy,
    pub robots: ResolvedRobotsConfig,
    pub security: ResolvedSecurityConfig,
    pub http: ResolvedHttpConfig,
}

impl ResolvedConfig {
    pub fn from_config(config: &WebFetchConfig) -> Result<Self, WebFetchError> {
        let user_agent = config
            .user_agent
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());

        let timeout = Duration::from_secs(u64::from(config.timeout_seconds()));

        let default_max_chunk_tokens = config.default_max_chunk_tokens();
        let max_download_bytes = config.max_download_bytes();
        let max_dns_attempts = config
            .max_dns_attempts
            .unwrap_or(WebFetchConfig::DEFAULT_MAX_DNS_ATTEMPTS)
            .max(1);
        let max_redirects = config.max_redirects();

        let security = ResolvedSecurityConfig::from_config(config);
        let http = ResolvedHttpConfig::from_config(config);
        let robots = ResolvedRobotsConfig::from_config(config, &user_agent);
        let cache = CachePolicy::from_config(config);

        Ok(Self {
            user_agent,
            timeout,
            max_redirects,
            default_max_chunk_tokens,
            max_download_bytes,
            max_dns_attempts,
            cache,
            robots,
            security,
            http,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSecurityConfig {
    pub blocked_cidrs: Vec<String>,
    pub allowed_ports: Vec<u16>,
    pub allow_insecure_tls: bool,
    pub allow_insecure_overrides: bool,
}

impl ResolvedSecurityConfig {
    fn from_config(config: &WebFetchConfig) -> Self {
        let security = config.security.as_ref();
        let blocked_cidrs = security
            .and_then(|s| s.blocked_cidrs.clone())
            .unwrap_or_default();
        let allowed_ports = security
            .and_then(|s| s.allowed_ports.clone())
            .filter(|ports| !ports.is_empty())
            .unwrap_or_else(|| DEFAULT_ALLOWED_PORTS.to_vec());
        let requested_insecure_tls = security.is_some_and(|s| s.allow_insecure_tls);
        let requested_insecure_overrides = security.is_some_and(|s| s.allow_insecure_overrides);
        let insecure_tls_opt_in = env_opt_in_enabled(ENV_ALLOW_INSECURE_TLS);
        let insecure_overrides_opt_in = env_opt_in_enabled(ENV_ALLOW_INSECURE_OVERRIDES);

        if requested_insecure_tls && !insecure_tls_opt_in {
            tracing::warn!(
                "WebFetch allow_insecure_tls requested in config but disabled: set {}=1 to opt in",
                ENV_ALLOW_INSECURE_TLS
            );
        }
        if requested_insecure_overrides && !insecure_overrides_opt_in {
            tracing::warn!(
                "WebFetch allow_insecure_overrides requested in config but disabled: set {}=1 to opt in",
                ENV_ALLOW_INSECURE_OVERRIDES
            );
        }

        Self {
            blocked_cidrs,
            allowed_ports,
            allow_insecure_tls: requested_insecure_tls && insecure_tls_opt_in,
            allow_insecure_overrides: requested_insecure_overrides && insecure_overrides_opt_in,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedHttpConfig {
    pub headers: Vec<(String, String)>,
    pub use_system_proxy: bool,
    pub connect_timeout: TimeoutSetting,
    pub read_timeout: TimeoutSetting,
}

impl ResolvedHttpConfig {
    fn from_config(config: &WebFetchConfig) -> Self {
        let http = config.http.as_ref();
        let headers = http.and_then(|h| h.headers.clone()).unwrap_or_default();
        let use_system_proxy = http.is_some_and(|h| h.use_system_proxy);
        let connect_timeout = http
            .and_then(|h| h.connect_timeout_seconds)
            .map_or(TimeoutSetting::Disabled, |s| {
                TimeoutSetting::Enabled(Duration::from_secs(u64::from(s)))
            });
        let read_timeout = http
            .and_then(|h| h.read_timeout_seconds)
            .map_or(TimeoutSetting::Disabled, |s| {
                TimeoutSetting::Enabled(Duration::from_secs(u64::from(s)))
            });

        Self {
            headers,
            use_system_proxy,
            connect_timeout,
            read_timeout,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TimeoutSetting {
    Disabled,
    Enabled(Duration),
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRobotsConfig {
    pub fail_open: bool,
    pub user_agent_token: String,
    pub cache_ttl: Duration,
    pub cache_entries: usize,
}

impl ResolvedRobotsConfig {
    fn from_config(config: &WebFetchConfig, user_agent: &str) -> Self {
        let robots = config.robots.as_ref();
        let fail_open = robots.is_some_and(|r| r.fail_open);
        let user_agent_token = robots
            .and_then(|r| r.user_agent_token.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| derive_robots_token(user_agent));
        let cache_entries = config
            .robots_cache_entries
            .unwrap_or(WebFetchConfig::DEFAULT_ROBOTS_CACHE_ENTRIES)
            as usize;
        let ttl_hours = config
            .robots_cache_ttl_hours
            .unwrap_or(WebFetchConfig::DEFAULT_ROBOTS_CACHE_TTL_HOURS)
            .max(1);

        Self {
            fail_open,
            user_agent_token,
            cache_ttl: Duration::from_secs(u64::from(ttl_hours) * 60 * 60),
            cache_entries,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum CachePolicy {
    Disabled,
    Enabled(CacheSettings),
}

impl CachePolicy {
    fn from_config(config: &WebFetchConfig) -> Self {
        let max_entries = config
            .max_cache_entries
            .unwrap_or(WebFetchConfig::DEFAULT_MAX_CACHE_ENTRIES);
        if max_entries == 0 {
            return CachePolicy::Disabled;
        }

        let dir = config.cache_dir.clone().or_else(default_cache_dir);
        let dir = match dir {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => return CachePolicy::Disabled,
        };

        let ttl_days = config
            .cache_ttl_days
            .unwrap_or(WebFetchConfig::DEFAULT_CACHE_TTL_DAYS)
            .max(1);
        let max_bytes = config
            .max_cache_bytes
            .unwrap_or(WebFetchConfig::DEFAULT_MAX_CACHE_BYTES);

        CachePolicy::Enabled(CacheSettings {
            dir,
            max_entries,
            max_bytes,
            ttl: Duration::from_secs(u64::from(ttl_days) * 24 * 60 * 60),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CacheSettings {
    pub dir: PathBuf,
    pub max_entries: u32,
    pub max_bytes: u64,
    pub ttl: Duration,
}

fn default_cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("forge").join("webfetch"))
}

fn derive_robots_token(user_agent: &str) -> String {
    let token = user_agent.split('/').next().unwrap_or("");
    let filtered: String = token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if filtered.is_empty() {
        "forge-webfetch".to_string()
    } else {
        filtered
    }
}

fn env_opt_in_enabled(name: &str) -> bool {
    is_truthy_env(std::env::var(name).ok().as_deref())
}

fn is_truthy_env(value: Option<&str>) -> bool {
    value.is_some_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::is_truthy_env;

    #[test]
    fn insecure_flags_ignored_without_env_var() {
        assert!(!is_truthy_env(None));
    }

    #[test]
    fn insecure_flags_accept_truthy_env_values() {
        assert!(is_truthy_env(Some("1")));
        assert!(is_truthy_env(Some("true")));
        assert!(is_truthy_env(Some("YES")));
        assert!(is_truthy_env(Some("on")));
    }
}

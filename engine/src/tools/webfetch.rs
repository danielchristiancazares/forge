//! WebFetch tool executor for URL fetching with browser fallback.

use serde::Deserialize;
use std::path::PathBuf;

use super::{
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, redact_summary, sanitize_output,
};

const WEBFETCH_TOOL_NAME: &str = "WebFetch";

/// Configuration for the WebFetch tool.
#[derive(Debug, Clone)]
pub struct WebFetchToolConfig {
    pub enabled: bool,
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
            enabled: false,
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

/// WebFetch tool executor.
#[derive(Debug)]
pub struct WebFetchTool {
    config: WebFetchToolConfig,
}

impl WebFetchTool {
    pub fn new(config: WebFetchToolConfig) -> Self {
        Self { config }
    }

    fn build_config(&self) -> forge_webfetch::WebFetchConfig {
        forge_webfetch::WebFetchConfig {
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
    no_cache: Option<bool>,
    force_browser: Option<bool>,
}

impl ToolExecutor for WebFetchTool {
    fn name(&self) -> &'static str {
        WEBFETCH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Fetch a URL and return structured, chunked markdown content"
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
                "no_cache": {
                    "type": "boolean",
                    "default": false,
                    "description": "Bypass cache and fetch fresh content"
                },
                "force_browser": {
                    "type": "boolean",
                    "default": false,
                    "description": "Force headless browser rendering"
                }
            },
            "required": ["url"]
        })
    }

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: WebFetchArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
        let summary = format!("Fetch URL: {}", typed.url);
        Ok(redact_summary(&summary))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: WebFetchArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;

            if typed.url.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "url must not be empty".to_string(),
                });
            }

            // Build input
            let mut input =
                forge_webfetch::WebFetchInput::new(&typed.url).map_err(|e| ToolError::BadArgs {
                    message: e.message.clone(),
                })?;

            if let Some(tokens) = typed.max_chunk_tokens {
                input = input
                    .with_max_chunk_tokens(tokens)
                    .map_err(|e| ToolError::BadArgs {
                        message: e.message.clone(),
                    })?;
            }

            input = input.with_no_cache(typed.no_cache.unwrap_or(false));
            input = input.with_force_browser(typed.force_browser.unwrap_or(false));

            // Build config
            let config = self.build_config();

            // Execute fetch
            let output = forge_webfetch::fetch(input, &config).await.map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: WEBFETCH_TOOL_NAME.to_string(),
                    message: e.message.clone(),
                }
            })?;

            // Serialize output
            let json = serde_json::to_string(&output).map_err(|e| ToolError::ExecutionFailed {
                tool: WEBFETCH_TOOL_NAME.to_string(),
                message: e.to_string(),
            })?;

            // Truncate if needed
            let effective_max = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
            let output_str = if json.len() > effective_max {
                super::truncate_output(json, effective_max)
            } else {
                json
            };

            Ok(sanitize_output(&output_str))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = WebFetchToolConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.timeout_seconds, 20);
        assert_eq!(config.default_max_chunk_tokens, 600);
    }
}

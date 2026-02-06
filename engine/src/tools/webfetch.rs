//! `WebFetch` tool executor for URL fetching with browser fallback.

use serde::Deserialize;
use std::path::PathBuf;

use forge_webfetch::{Note, TruncationReason, WebFetchOutput};

use super::{
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, parse_args, redact_distillate,
    sanitize_output,
};

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
    #[serde(default)]
    no_cache: bool,
    #[serde(default)]
    force_browser: bool,
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

            input = input.with_no_cache(typed.no_cache);
            input = input.with_force_browser(typed.force_browser);

            // Build config
            let config = self.build_config();

            // Execute fetch
            let output = forge_webfetch::fetch(input, &config).await.map_err(|e| {
                ToolError::ExecutionFailed {
                    tool: WEBFETCH_TOOL_NAME.to_string(),
                    message: e.message.clone(),
                }
            })?;

            // Trim to fit output budget while preserving valid JSON.
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
    output.truncated = true;
    output.truncation_reason = Some(TruncationReason::ToolOutputLimit);
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
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = WebFetchToolConfig::default();
        assert_eq!(config.timeout_seconds, 20);
        assert_eq!(config.default_max_chunk_tokens, 600);
    }

    #[test]
    fn shrink_output_to_fit_preserves_valid_json() {
        let chunks = vec![
            forge_webfetch::FetchChunk {
                heading: "Header 1".to_string(),
                text: "A".repeat(120),
                token_count: 120,
            },
            forge_webfetch::FetchChunk {
                heading: "Header 2".to_string(),
                text: "B".repeat(120),
                token_count: 120,
            },
            forge_webfetch::FetchChunk {
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
            rendering_method: forge_webfetch::RenderingMethod::Http,
            truncated: false,
            truncation_reason: None,
            notes: Vec::new(),
        };

        let json = shrink_output_to_fit(output, 200).expect("shrink output");
        assert!(json.len() <= 200);
        let parsed: WebFetchOutput = serde_json::from_str(&json).expect("valid json");
        assert!(parsed.truncated);
    }
}

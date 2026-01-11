use serde::Deserialize;
use std::{env, path::PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct ForgeConfig {
    pub app: Option<AppConfig>,
    pub api_keys: Option<ApiKeys>,
    pub context: Option<ContextConfig>,
    pub cache: Option<CacheConfig>,
    pub thinking: Option<ThinkingConfig>,
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAIConfig>,
    /// Tool configurations for function calling.
    pub tools: Option<ToolsConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tui: Option<String>,
    /// Maximum output tokens for responses. Overrides model default.
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ContextConfig {
    pub infinity: Option<bool>,
}

/// Legacy configuration for prompt caching.
/// Prefer [anthropic] cache_enabled going forward.
#[derive(Debug, Default, Deserialize)]
pub struct CacheConfig {
    /// Enable prompt caching. Default: true for Claude, ignored for OpenAI.
    pub enabled: Option<bool>,
}

/// Legacy configuration for extended thinking/reasoning.
/// Prefer [anthropic] thinking_* fields going forward.
#[derive(Debug, Default, Deserialize)]
pub struct ThinkingConfig {
    /// Enable extended thinking. Default: false.
    pub enabled: Option<bool>,
    /// Token budget for thinking. Default: 10000. Minimum: 1024.
    pub budget_tokens: Option<u32>,
}

/// Anthropic/Claude request defaults.
///
/// ```toml
/// [anthropic]
/// cache_enabled = true
/// thinking_enabled = false
/// thinking_budget_tokens = 10000
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct AnthropicConfig {
    pub cache_enabled: Option<bool>,
    pub thinking_enabled: Option<bool>,
    pub thinking_budget_tokens: Option<u32>,
}

/// OpenAI Responses API request defaults.
///
/// ```toml
/// [openai]
/// reasoning_effort = "high"
/// verbosity = "high"
/// truncation = "auto"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct OpenAIConfig {
    pub reasoning_effort: Option<String>,
    pub verbosity: Option<String>,
    pub truncation: Option<String>,
}

/// Tool configurations for function calling.
///
/// ```toml
/// [[tools.definitions]]
/// name = "get_weather"
/// description = "Get current weather for a location"
/// [tools.definitions.parameters]
/// type = "object"
/// [tools.definitions.parameters.properties.location]
/// type = "string"
/// description = "City name, e.g. 'Seattle, WA'"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ToolsConfig {
    /// Tool loop mode: disabled | parse_only | enabled
    pub mode: Option<String>,
    /// Whether parallel tool execution is allowed.
    pub allow_parallel: Option<bool>,
    /// Maximum tool calls per batch.
    pub max_tool_calls_per_batch: Option<usize>,
    /// Maximum tool iterations per user turn.
    pub max_tool_iterations_per_user_turn: Option<u32>,
    /// Maximum serialized tool args size (bytes).
    pub max_tool_args_bytes: Option<usize>,
    /// List of tool definitions.
    #[serde(default)]
    pub definitions: Vec<ToolDefinitionConfig>,
    /// Sandbox config.
    pub sandbox: Option<ToolSandboxConfig>,
    /// Timeout config.
    pub timeouts: Option<ToolTimeoutsConfig>,
    /// Output config.
    pub output: Option<ToolOutputConfig>,
    /// Environment sanitization config.
    pub environment: Option<ToolEnvironmentConfig>,
    /// Approval policy config.
    pub approval: Option<ToolApprovalConfig>,
    /// read_file limits.
    pub read_file: Option<ReadFileConfig>,
    /// apply_patch limits.
    pub apply_patch: Option<ApplyPatchConfig>,
    /// search limits.
    pub search: Option<SearchConfig>,
}

/// Configuration for a single tool definition.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolDefinitionConfig {
    pub name: String,
    pub description: String,
    /// JSON Schema as inline TOML table.
    pub parameters: toml::Value,
}

/// Sandbox configuration for tools.
#[derive(Debug, Default, Deserialize)]
pub struct ToolSandboxConfig {
    #[serde(default)]
    pub allowed_roots: Vec<String>,
    #[serde(default)]
    pub denied_patterns: Vec<String>,
    pub allow_absolute: Option<bool>,
    pub include_default_denies: Option<bool>,
}

/// Timeout configuration for tools.
#[derive(Debug, Default, Deserialize)]
pub struct ToolTimeoutsConfig {
    pub default_seconds: Option<u64>,
    pub file_operations_seconds: Option<u64>,
    pub shell_commands_seconds: Option<u64>,
}

/// Output configuration for tools.
#[derive(Debug, Default, Deserialize)]
pub struct ToolOutputConfig {
    pub max_bytes: Option<usize>,
}

/// Environment sanitization configuration for tools.
#[derive(Debug, Default, Deserialize)]
pub struct ToolEnvironmentConfig {
    #[serde(default)]
    pub denylist: Vec<String>,
}

/// Approval policy configuration for tools.
#[derive(Debug, Default, Deserialize)]
pub struct ToolApprovalConfig {
    pub enabled: Option<bool>,
    pub mode: Option<String>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub denylist: Vec<String>,
    pub prompt_side_effects: Option<bool>,
}

/// read_file limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct ReadFileConfig {
    pub max_file_read_bytes: Option<usize>,
    pub max_scan_bytes: Option<usize>,
}

/// apply_patch limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct ApplyPatchConfig {
    pub max_patch_bytes: Option<usize>,
}

/// search tool limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct SearchConfig {
    pub enabled: Option<bool>,
    pub binary: Option<String>,
    pub fallback_binary: Option<String>,
    pub default_timeout_ms: Option<u64>,
    pub default_max_results: Option<usize>,
    pub max_matches_per_file: Option<usize>,
    pub max_files: Option<usize>,
    pub max_file_size_bytes: Option<u64>,
}

impl ToolDefinitionConfig {
    /// Convert this config to a ToolDefinition.
    pub fn to_tool_definition(&self) -> Result<forge_types::ToolDefinition, String> {
        let params_json = toml_to_json(&self.parameters)?;
        Ok(forge_types::ToolDefinition::new(
            self.name.clone(),
            self.description.clone(),
            params_json,
        ))
    }
}

/// Convert a TOML value to a JSON value.
fn toml_to_json(value: &toml::Value) -> Result<serde_json::Value, String> {
    match value {
        toml::Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        toml::Value::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        toml::Value::Float(f) => {
            let n =
                serde_json::Number::from_f64(*f).ok_or_else(|| format!("Invalid float: {}", f))?;
            Ok(serde_json::Value::Number(n))
        }
        toml::Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        toml::Value::Array(arr) => {
            let json_arr: Result<Vec<_>, _> = arr.iter().map(toml_to_json).collect();
            Ok(serde_json::Value::Array(json_arr?))
        }
        toml::Value::Table(table) => {
            let mut map = serde_json::Map::new();
            for (k, v) in table {
                map.insert(k.clone(), toml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
    }
}

pub fn expand_env_vars(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut i = 0;

    while i < value.len() {
        if value[i..].starts_with("${") {
            let start = i + 2;
            if let Some(end_rel) = value[start..].find('}') {
                let end = start + end_rel;
                let var = &value[start..end];
                if !var.is_empty() {
                    let replacement = env::var(var).unwrap_or_default();
                    out.push_str(&replacement);
                }
                i = end + 1;
                continue;
            }
        }

        let ch = value[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

impl ForgeConfig {
    pub fn load() -> Option<Self> {
        let path = config_path()?;
        if !path.exists() {
            return None;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!("Failed to read config at {:?}: {}", path, err);
                return None;
            }
        };

        match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(err) => {
                tracing::warn!("Failed to parse config at {:?}: {}", path, err);
                None
            }
        }
    }

    pub fn path() -> Option<PathBuf> {
        config_path()
    }
}

/// Returns the path to the forge config file.
pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".forge").join("config.toml"))
}

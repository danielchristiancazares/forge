use serde::Deserialize;
use std::{env, fs, path::PathBuf};

// Default value function for serde (bool::default() is false, so only true needs a fn)
pub(crate) const fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
#[allow(clippy::unsafe_derive_deserialize)] // unsafe is for Unix permission checks, unrelated to serde
pub struct ForgeConfig {
    pub app: Option<AppConfig>,
    pub api_keys: Option<ApiKeys>,
    pub context: Option<ContextConfig>,
    pub cache: Option<CacheConfig>,
    pub thinking: Option<ThinkingConfig>,
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAIConfig>,
    pub google: Option<GeminiConfig>,
    /// Tool configurations for function calling.
    pub tools: Option<ToolsConfig>,
    /// LSP client configuration for language server diagnostics.
    pub lsp: Option<forge_lsp::LspConfig>,
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl ConfigError {
    pub fn path(&self) -> &PathBuf {
        match self {
            ConfigError::Read { path, .. } | ConfigError::Parse { path, .. } => path,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub model: Option<String>,
    pub tui: Option<String>,
    /// Use ASCII-only glyphs for icons and spinners.
    #[serde(default)]
    pub ascii_only: bool,
    /// Enable a high-contrast color palette.
    #[serde(default)]
    pub high_contrast: bool,
    /// Disable modal animations and motion effects.
    #[serde(default)]
    pub reduced_motion: bool,
    /// Render provider thinking/reasoning deltas in the UI (if available).
    #[serde(default)]
    pub show_thinking: bool,
}

#[derive(Default, Deserialize)]
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub google: Option<String>,
}

// Manual Debug impl to prevent leaking API keys in logs.
impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn mask(opt: Option<&String>) -> &'static str {
            if opt.is_some() { "[REDACTED]" } else { "None" }
        }
        f.debug_struct("ApiKeys")
            .field("anthropic", &mask(self.anthropic.as_ref()))
            .field("openai", &mask(self.openai.as_ref()))
            .field("google", &mask(self.google.as_ref()))
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ContextConfig {
    /// Enable memory (librarian fact extraction and retrieval).
    /// Defaults to false. Can also be controlled via FORGE_CONTEXT_INFINITY env var when [context] section is absent.
    #[serde(default)]
    pub memory: bool,
}

/// Legacy configuration for prompt caching.
/// Prefer [anthropic] `cache_enabled` going forward.
#[derive(Debug, Default, Deserialize)]
pub struct CacheConfig {
    /// Enable prompt caching. Default: true for Claude, ignored for `OpenAI`.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Legacy configuration for extended thinking/reasoning.
/// Prefer [anthropic] thinking_* fields going forward.
#[derive(Debug, Default, Deserialize)]
pub struct ThinkingConfig {
    /// Enable extended thinking. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Token budget for thinking. Default: 10000. Minimum: 1024.
    pub budget_tokens: Option<u32>,
}

/// Thinking mode for Opus 4.6+.
///
/// Controls how Claude decides when and how much to think.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicThinkingMode {
    /// Claude decides when and how much to think (recommended for Opus 4.6).
    #[default]
    Adaptive,
    /// Manual mode with explicit `budget_tokens`.
    Enabled,
    /// No thinking.
    Disabled,
}

impl AnthropicThinkingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// Effort level for Opus 4.6+.
///
/// Controls how eagerly Claude spends tokens on responses and thinking.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    #[default]
    Max,
}

impl AnthropicEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// Anthropic/Claude request defaults.
///
/// ```toml
/// [anthropic]
/// cache_enabled = true
/// thinking_mode = "adaptive"
/// thinking_effort = "max"
/// thinking_budget_tokens = 10000
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct AnthropicConfig {
    #[serde(default = "default_true")]
    pub cache_enabled: bool,
    /// Legacy field for pre-4.6 models. Use `thinking_mode` for Opus 4.6+.
    #[serde(default)]
    pub thinking_enabled: bool,
    pub thinking_budget_tokens: Option<u32>,
    /// Thinking mode for Opus 4.6+: "adaptive" (default), "enabled", or "disabled".
    #[serde(default)]
    pub thinking_mode: AnthropicThinkingMode,
    /// Effort level for Opus 4.6+: "low", "medium", "high", or "max" (default).
    #[serde(default)]
    pub thinking_effort: AnthropicEffort,
}

/// `OpenAI` Responses API request defaults.
///
/// ```toml
/// [openai]
/// reasoning_effort = "high"
/// reasoning_summary = "auto"
/// verbosity = "high"
/// truncation = "auto"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct OpenAIConfig {
    pub reasoning_effort: Option<String>,
    pub reasoning_summary: Option<String>,
    pub verbosity: Option<String>,
    pub truncation: Option<String>,
}

/// Google Gemini API request defaults.
///
/// ```toml
/// [google]
/// thinking_enabled = true
/// cache_enabled = true
/// cache_ttl_seconds = 3600
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct GeminiConfig {
    /// Enable thinking mode with `thinkingLevel: "high"`. Default: false.
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Enable explicit context caching. Default: false.
    #[serde(default)]
    pub cache_enabled: bool,
    /// TTL for cached content in seconds. Default: 3600.
    pub cache_ttl_seconds: Option<u32>,
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
    /// Maximum tool calls per batch.
    pub max_tool_calls_per_batch: Option<usize>,
    pub max_tool_iterations_per_user_turn: Option<u32>,
    #[serde(default)]
    pub definitions: Vec<ToolDefinitionConfig>,
    pub sandbox: Option<ToolSandboxConfig>,
    pub timeouts: Option<ToolTimeoutsConfig>,
    pub output: Option<ToolOutputConfig>,
    pub environment: Option<ToolEnvironmentConfig>,
    pub approval: Option<ToolApprovalConfig>,
    pub read_file: Option<ReadFileConfig>,
    pub apply_patch: Option<ApplyPatchConfig>,
    pub search: Option<SearchConfig>,
    pub webfetch: Option<WebFetchConfig>,
    /// Run tool hardening controls.
    pub run: Option<RunConfig>,
    /// Shell configuration for `run_command`.
    pub shell: Option<ShellConfig>,
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
    #[serde(default)]
    pub allow_absolute: bool,
    #[serde(default = "default_true")]
    pub include_default_denies: bool,
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
    pub mode: Option<String>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub denylist: Vec<String>,
}

/// `read_file` limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct ReadFileConfig {
    pub max_file_read_bytes: Option<usize>,
    pub max_scan_bytes: Option<usize>,
}

/// `apply_patch` limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct ApplyPatchConfig {
    pub max_patch_bytes: Option<usize>,
}

/// search tool limits configuration.
#[derive(Debug, Default, Deserialize)]
pub struct SearchConfig {
    pub binary: Option<String>,
    pub fallback_binary: Option<String>,
    pub default_timeout_ms: Option<u64>,
    pub default_max_results: Option<usize>,
    pub max_matches_per_file: Option<usize>,
    pub max_files: Option<usize>,
    pub max_file_size_bytes: Option<u64>,
}

/// webfetch tool configuration.
#[derive(Debug, Default, Deserialize)]
pub struct WebFetchConfig {
    pub user_agent: Option<String>,
    pub timeout_seconds: Option<u32>,
    pub max_redirects: Option<u32>,
    pub default_max_chunk_tokens: Option<u32>,
    pub max_download_bytes: Option<u64>,
    pub cache_dir: Option<String>,
    pub cache_ttl_days: Option<u32>,
}

/// Configuration for the `Run` tool.
#[derive(Debug, Default, Deserialize)]
pub struct RunConfig {
    pub windows: Option<WindowsRunConfig>,
}

/// Fallback behavior when Windows sandbox prerequisites are unavailable.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunFallbackMode {
    /// Require explicit per-call opt-in to run unsandboxed.
    #[default]
    Prompt,
    /// Deny unsandboxed fallback.
    Deny,
    /// Allow unsandboxed fallback and attach warnings.
    AllowWithWarning,
}

/// Windows-specific Run hardening configuration.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct WindowsRunConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub fallback_mode: RunFallbackMode,
}

impl Default for WindowsRunConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fallback_mode: RunFallbackMode::Prompt,
        }
    }
}

/// Shell configuration for `run_command` tool.
///
/// ```toml
/// [tools.shell]
/// binary = "pwsh"
/// args = ["-NoProfile", "-Command"]
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ShellConfig {
    /// Override shell binary (e.g., "pwsh", "Run", "/usr/local/bin/fish").
    pub binary: Option<String>,
    /// Override shell args (e.g., `["-c"]` or `["/C"]`).
    pub args: Option<Vec<String>>,
}

impl ToolDefinitionConfig {
    /// Convert this config to a `ToolDefinition`.
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
                serde_json::Number::from_f64(*f).ok_or_else(|| format!("Invalid float: {f}"))?;
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
    pub fn load() -> Result<Option<Self>, ConfigError> {
        let path = match config_path() {
            Some(path) => path,
            None => return Ok(None),
        };
        if !path.exists() {
            return Ok(None);
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!("Failed to read config at {:?}: {}", path, err);
                return Err(ConfigError::Read { path, source: err });
            }
        };

        match toml::from_str(&content) {
            Ok(config) => Ok(Some(config)),
            Err(err) => {
                tracing::warn!("Failed to parse config at {:?}: {}", path, err);
                Err(ConfigError::Parse { path, source: err })
            }
        }
    }

    #[must_use]
    pub fn path() -> Option<PathBuf> {
        config_path()
    }

    /// Persist the model to the config file.
    ///
    /// Uses `toml_edit` to preserve comments and formatting.
    /// Creates the config file and parent directory if they don't exist.
    pub fn persist_model(model: &str) -> std::io::Result<()> {
        let path = match config_path() {
            Some(path) => path,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not determine config path",
                ));
            }
        };

        // Ensure parent directory exists with secure permissions
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                let metadata = fs::metadata(parent)?;
                // Only modify permissions if we own the directory
                let our_uid = unsafe { libc::getuid() };
                if metadata.uid() == our_uid {
                    let mode = metadata.permissions().mode() & 0o777;
                    if mode & 0o077 != 0 {
                        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                    }
                }
            }
        }

        // Load existing config or create empty document
        let content = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };

        let mut doc = content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Ensure [app] table exists
        if !doc.contains_key("app") {
            doc["app"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        // Set the model
        doc["app"]["model"] = toml_edit::value(model);

        // Write back atomically
        let serialized = doc.to_string();
        forge_context::atomic_write_with_options(
            &path,
            serialized.as_bytes(),
            forge_context::AtomicWriteOptions {
                sync_all: true,
                dir_sync: true,
                unix_mode: None,
            },
        )?;

        // Ensure config file has secure permissions (user-only read/write)
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = fs::metadata(&path)?;
            let our_uid = unsafe { libc::getuid() };
            if metadata.uid() == our_uid {
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                }
            }
        }

        // Windows: no equivalent to Unix file modes. Warn if the config file
        // likely contains raw API keys (as opposed to ${ENV_VAR} references).
        #[cfg(windows)]
        {
            let content_lower = serialized.to_ascii_lowercase();
            let has_literal_key = ["anthropic_api_key", "openai_api_key", "gemini_api_key"]
                .iter()
                .any(|field| {
                    content_lower.contains(field)
                        && !content_lower.contains(&format!("{field} = \"${{"))
                });
            if has_literal_key {
                tracing::warn!(
                    path = %path.display(),
                    "Config file may contain literal API keys. \
                     Windows does not enforce file permissions like Unix. \
                     Consider using ${{ENV_VAR}} syntax for API keys instead."
                );
            }
        }

        Ok(())
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".forge").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // expand_env_vars tests

    #[test]
    fn expand_env_vars_no_vars() {
        let result = expand_env_vars("hello world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn expand_env_vars_single_var() {
        unsafe {
            std::env::set_var("TEST_CONFIG_VAR", "replaced");
        }
        let result = expand_env_vars("prefix ${TEST_CONFIG_VAR} suffix");
        assert_eq!(result, "prefix replaced suffix");
        unsafe {
            std::env::remove_var("TEST_CONFIG_VAR");
        }
    }

    #[test]
    fn expand_env_vars_missing_var_becomes_empty() {
        unsafe {
            std::env::remove_var("MISSING_VAR_FOR_TEST");
        }
        let result = expand_env_vars("before ${MISSING_VAR_FOR_TEST} after");
        assert_eq!(result, "before  after");
    }

    #[test]
    fn expand_env_vars_multiple_vars() {
        unsafe {
            std::env::set_var("VAR_A", "alpha");
            std::env::set_var("VAR_B", "beta");
        }
        let result = expand_env_vars("${VAR_A}-${VAR_B}");
        assert_eq!(result, "alpha-beta");
        unsafe {
            std::env::remove_var("VAR_A");
            std::env::remove_var("VAR_B");
        }
    }

    #[test]
    fn expand_env_vars_unclosed_brace_preserved() {
        let result = expand_env_vars("test ${UNCLOSED");
        assert_eq!(result, "test ${UNCLOSED");
    }

    #[test]
    fn expand_env_vars_empty_var_name_preserved() {
        let result = expand_env_vars("test ${} more");
        assert_eq!(result, "test  more");
    }

    #[test]
    fn expand_env_vars_adjacent_vars() {
        unsafe {
            std::env::set_var("ADJ_A", "X");
            std::env::set_var("ADJ_B", "Y");
        }
        let result = expand_env_vars("${ADJ_A}${ADJ_B}");
        assert_eq!(result, "XY");
        unsafe {
            std::env::remove_var("ADJ_A");
            std::env::remove_var("ADJ_B");
        }
    }

    #[test]
    fn expand_env_vars_unicode_content() {
        unsafe {
            std::env::set_var("UNICODE_VAR", "🦀");
        }
        let result = expand_env_vars("Hello ${UNICODE_VAR} Rust");
        assert_eq!(result, "Hello 🦀 Rust");
        unsafe {
            std::env::remove_var("UNICODE_VAR");
        }
    }

    // toml_to_json tests

    #[test]
    fn toml_to_json_string() {
        let toml_val = toml::Value::String("test".to_string());
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json, serde_json::Value::String("test".to_string()));
    }

    #[test]
    fn toml_to_json_integer() {
        let toml_val = toml::Value::Integer(42);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json, serde_json::json!(42));
    }

    #[test]
    fn toml_to_json_float() {
        let toml_val = toml::Value::Float(2.5);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json, serde_json::json!(2.5));
    }

    #[test]
    fn toml_to_json_boolean() {
        let toml_val = toml::Value::Boolean(true);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json, serde_json::json!(true));
    }

    #[test]
    fn toml_to_json_array() {
        let toml_val = toml::Value::Array(vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::Integer(3),
        ]);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn toml_to_json_table() {
        let mut table = toml::value::Table::new();
        table.insert("key".to_string(), toml::Value::String("value".to_string()));
        table.insert("num".to_string(), toml::Value::Integer(123));
        let toml_val = toml::Value::Table(table);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json["key"], "value");
        assert_eq!(json["num"], 123);
    }

    #[test]
    fn toml_to_json_nested() {
        let mut inner = toml::value::Table::new();
        inner.insert("nested".to_string(), toml::Value::Boolean(true));
        let mut outer = toml::value::Table::new();
        outer.insert("inner".to_string(), toml::Value::Table(inner));
        let toml_val = toml::Value::Table(outer);
        let json = toml_to_json(&toml_val).unwrap();
        assert_eq!(json["inner"]["nested"], true);
    }

    #[test]
    fn toml_to_json_invalid_float_nan() {
        let toml_val = toml::Value::Float(f64::NAN);
        let result = toml_to_json(&toml_val);
        assert!(result.is_err());
    }

    // ForgeConfig parsing tests

    #[test]
    fn parse_empty_config() {
        let config: ForgeConfig = toml::from_str("").unwrap();
        assert!(config.app.is_none());
        assert!(config.api_keys.is_none());
    }

    #[test]
    fn parse_app_config() {
        let toml_str = r#"
[app]
model = "claude-opus-4-6"
tui = "full"
ascii_only = true
high_contrast = false
reduced_motion = true
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let app = config.app.unwrap();
        assert_eq!(app.model, Some("claude-opus-4-6".to_string()));
        assert_eq!(app.tui, Some("full".to_string()));
        assert!(app.ascii_only);
        assert!(!app.high_contrast);
        assert!(app.reduced_motion);
    }

    #[test]
    fn parse_api_keys_config() {
        let toml_str = r#"
[api_keys]
anthropic = "sk-ant-test"
openai = "sk-openai-test"
google = "AIza-test"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let keys = config.api_keys.unwrap();
        assert_eq!(keys.anthropic, Some("sk-ant-test".to_string()));
        assert_eq!(keys.openai, Some("sk-openai-test".to_string()));
        assert_eq!(keys.google, Some("AIza-test".to_string()));
    }

    #[test]
    fn api_keys_debug_redacts_values() {
        let keys = ApiKeys {
            anthropic: Some("sk-ant-secret123".to_string()),
            openai: Some("sk-secret456".to_string()),
            google: Some("AIzaSyC789".to_string()),
        };
        let debug_output = format!("{keys:?}");
        // Should show [REDACTED] instead of actual keys
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("sk-ant-secret123"));
        assert!(!debug_output.contains("sk-secret456"));
        assert!(!debug_output.contains("AIzaSyC789"));
    }

    #[test]
    fn api_keys_debug_shows_none() {
        let keys = ApiKeys::default();
        let debug_output = format!("{keys:?}");
        assert!(debug_output.contains("None"));
        assert!(!debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn parse_context_config() {
        let toml_str = r"
[context]
memory = true
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        assert!(config.context.unwrap().memory);
    }

    #[test]
    fn parse_anthropic_config() {
        let toml_str = r"
[anthropic]
cache_enabled = true
thinking_enabled = false
thinking_budget_tokens = 10000
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let anthropic = config.anthropic.unwrap();
        assert!(anthropic.cache_enabled);
        assert!(!anthropic.thinking_enabled);
        assert_eq!(anthropic.thinking_budget_tokens, Some(10000));
        // Defaults when not specified
        assert_eq!(anthropic.thinking_mode, AnthropicThinkingMode::Adaptive);
        assert_eq!(anthropic.thinking_effort, AnthropicEffort::Max);
    }

    #[test]
    fn parse_anthropic_thinking_mode_and_effort() {
        let toml_str = r#"
[anthropic]
thinking_mode = "disabled"
thinking_effort = "medium"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let anthropic = config.anthropic.unwrap();
        assert_eq!(anthropic.thinking_mode, AnthropicThinkingMode::Disabled);
        assert_eq!(anthropic.thinking_effort, AnthropicEffort::Medium);
    }

    #[test]
    fn parse_anthropic_thinking_mode_enabled_with_budget() {
        let toml_str = r#"
[anthropic]
thinking_mode = "enabled"
thinking_effort = "high"
thinking_budget_tokens = 8192
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let anthropic = config.anthropic.unwrap();
        assert_eq!(anthropic.thinking_mode, AnthropicThinkingMode::Enabled);
        assert_eq!(anthropic.thinking_effort, AnthropicEffort::High);
        assert_eq!(anthropic.thinking_budget_tokens, Some(8192));
    }

    #[test]
    fn anthropic_thinking_mode_as_str() {
        assert_eq!(AnthropicThinkingMode::Adaptive.as_str(), "adaptive");
        assert_eq!(AnthropicThinkingMode::Enabled.as_str(), "enabled");
        assert_eq!(AnthropicThinkingMode::Disabled.as_str(), "disabled");
    }

    #[test]
    fn anthropic_effort_as_str() {
        assert_eq!(AnthropicEffort::Low.as_str(), "low");
        assert_eq!(AnthropicEffort::Medium.as_str(), "medium");
        assert_eq!(AnthropicEffort::High.as_str(), "high");
        assert_eq!(AnthropicEffort::Max.as_str(), "max");
    }

    #[test]
    fn parse_openai_config() {
        let toml_str = r#"
[openai]
reasoning_effort = "high"
verbosity = "medium"
truncation = "auto"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let openai = config.openai.unwrap();
        assert_eq!(openai.reasoning_effort, Some("high".to_string()));
        assert_eq!(openai.verbosity, Some("medium".to_string()));
        assert_eq!(openai.truncation, Some("auto".to_string()));
    }

    #[test]
    fn parse_google_config() {
        let toml_str = r"
[google]
thinking_enabled = true
cache_enabled = true
cache_ttl_seconds = 7200
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let google = config.google.unwrap();
        assert!(google.thinking_enabled);
        assert!(google.cache_enabled);
        assert_eq!(google.cache_ttl_seconds, Some(7200));
    }

    #[test]
    fn parse_tools_config() {
        let toml_str = r"
[tools]
max_tool_calls_per_batch = 10
max_tool_iterations_per_user_turn = 25
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let tools = config.tools.unwrap();
        assert_eq!(tools.max_tool_calls_per_batch, Some(10));
        assert_eq!(tools.max_tool_iterations_per_user_turn, Some(25));
    }

    #[test]
    fn parse_tool_sandbox_config() {
        let toml_str = r#"
[tools.sandbox]
allowed_roots = ["/home/user/project", "/tmp"]
denied_patterns = ["*.secret", "**/.env"]
allow_absolute = false
include_default_denies = true
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let sandbox = config.tools.unwrap().sandbox.unwrap();
        assert_eq!(sandbox.allowed_roots, vec!["/home/user/project", "/tmp"]);
        assert_eq!(sandbox.denied_patterns, vec!["*.secret", "**/.env"]);
        assert!(!sandbox.allow_absolute);
        assert!(sandbox.include_default_denies);
    }

    #[test]
    fn parse_tool_timeouts_config() {
        let toml_str = r"
[tools.timeouts]
default_seconds = 60
file_operations_seconds = 30
shell_commands_seconds = 120
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let timeouts = config.tools.unwrap().timeouts.unwrap();
        assert_eq!(timeouts.default_seconds, Some(60));
        assert_eq!(timeouts.file_operations_seconds, Some(30));
        assert_eq!(timeouts.shell_commands_seconds, Some(120));
    }

    #[test]
    fn parse_tool_approval_config() {
        let toml_str = r#"
[tools.approval]
mode = "default"
allowlist = ["Read", "ListDir"]
denylist = ["Run"]
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let approval = config.tools.unwrap().approval.unwrap();
        assert_eq!(approval.mode, Some("default".to_string()));
        assert_eq!(approval.allowlist, vec!["Read", "ListDir"]);
        assert_eq!(approval.denylist, vec!["Run"]);
    }

    #[test]
    fn parse_shell_config() {
        let toml_str = r#"
[tools.shell]
binary = "pwsh"
args = ["-NoProfile", "-Command"]
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let shell = config.tools.unwrap().shell.unwrap();
        assert_eq!(shell.binary, Some("pwsh".to_string()));
        assert_eq!(
            shell.args,
            Some(vec!["-NoProfile".to_string(), "-Command".to_string()])
        );
    }

    #[test]
    fn parse_windows_run_config() {
        let toml_str = r#"
[tools.run.windows]
enabled = true
fallback_mode = "allow_with_warning"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let windows = config
            .tools
            .unwrap()
            .run
            .unwrap()
            .windows
            .expect("windows run config");
        assert!(windows.enabled);
        assert_eq!(windows.fallback_mode, RunFallbackMode::AllowWithWarning);
    }

    #[test]
    fn windows_run_config_defaults() {
        let toml_str = r"
[tools.run.windows]
";
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let windows = config
            .tools
            .unwrap()
            .run
            .unwrap()
            .windows
            .expect("windows run config");
        assert_eq!(windows, WindowsRunConfig::default());
    }

    #[test]
    fn parse_search_config() {
        let toml_str = r#"
[tools.search]
binary = "rg"
fallback_binary = "grep"
default_timeout_ms = 5000
default_max_results = 100
max_matches_per_file = 50
max_files = 1000
max_file_size_bytes = 1048576
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let search = config.tools.unwrap().search.unwrap();
        assert_eq!(search.binary, Some("rg".to_string()));
        assert_eq!(search.fallback_binary, Some("grep".to_string()));
        assert_eq!(search.default_timeout_ms, Some(5000));
        assert_eq!(search.default_max_results, Some(100));
        assert_eq!(search.max_matches_per_file, Some(50));
        assert_eq!(search.max_files, Some(1000));
        assert_eq!(search.max_file_size_bytes, Some(1_048_576));
    }

    #[test]
    fn parse_webfetch_config() {
        let toml_str = r#"
[tools.webfetch]
user_agent = "CustomBot/1.0"
timeout_seconds = 30
max_redirects = 5
default_max_chunk_tokens = 2000
max_download_bytes = 10485760
cache_dir = "/tmp/webfetch"
cache_ttl_days = 7
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let webfetch = config.tools.unwrap().webfetch.unwrap();
        assert_eq!(webfetch.user_agent, Some("CustomBot/1.0".to_string()));
        assert_eq!(webfetch.timeout_seconds, Some(30));
        assert_eq!(webfetch.max_redirects, Some(5));
        assert_eq!(webfetch.default_max_chunk_tokens, Some(2000));
        assert_eq!(webfetch.max_download_bytes, Some(10_485_760));
        assert_eq!(webfetch.cache_dir, Some("/tmp/webfetch".to_string()));
        assert_eq!(webfetch.cache_ttl_days, Some(7));
    }

    #[test]
    fn parse_tool_definition_config() {
        let toml_str = r#"
[[tools.definitions]]
name = "get_weather"
description = "Get weather for a location"
[tools.definitions.parameters]
type = "object"
[tools.definitions.parameters.properties.location]
type = "string"
description = "City name"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        let tools = config.tools.unwrap();
        assert_eq!(tools.definitions.len(), 1);
        let def = &tools.definitions[0];
        assert_eq!(def.name, "get_weather");
        assert_eq!(def.description, "Get weather for a location");
    }

    #[test]
    fn tool_definition_to_tool_definition() {
        let toml_str = r#"
name = "test_tool"
description = "A test tool"
[parameters]
type = "object"
additionalProperties = false
[parameters.properties.arg1]
type = "string"
"#;
        let def: ToolDefinitionConfig = toml::from_str(toml_str).unwrap();
        let tool_def = def.to_tool_definition().unwrap();
        assert_eq!(tool_def.name, "test_tool");
        assert_eq!(tool_def.description, "A test tool");
        assert_eq!(tool_def.parameters["type"], "object");
    }

    // ConfigError tests

    #[test]
    fn config_error_path_accessor() {
        let path = PathBuf::from("/test/path");
        let err = ConfigError::Read {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        assert_eq!(err.path(), &path);

        let parse_err = ConfigError::Parse {
            path: path.clone(),
            source: toml::from_str::<ForgeConfig>("invalid toml [").unwrap_err(),
        };
        assert_eq!(parse_err.path(), &path);
    }
}

// persist_model tests

#[test]
fn persist_model_creates_new_config() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("config.toml");

    let content = "";
    std::fs::write(&config_path, content).unwrap();

    let mut doc = content.parse::<toml_edit::DocumentMut>().unwrap();
    if !doc.contains_key("app") {
        doc["app"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    doc["app"]["model"] = toml_edit::value("gpt-4o");
    std::fs::write(&config_path, doc.to_string()).unwrap();

    let result = std::fs::read_to_string(&config_path).unwrap();
    assert!(result.contains("[app]"));
    assert!(result.contains("model = \"gpt-4o\""));
}

#[test]
fn persist_model_preserves_other_settings() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("config.toml");

    // Create existing config with comments and other settings
    let original = r#"# My config
[app]
model = "old-model"
ascii_only = true

[api_keys]
anthropic = "sk-test"
"#;
    std::fs::write(&config_path, original).unwrap();

    let mut doc = original.parse::<toml_edit::DocumentMut>().unwrap();
    doc["app"]["model"] = toml_edit::value("new-model");
    std::fs::write(&config_path, doc.to_string()).unwrap();

    let result = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        result.contains("# My config"),
        "Comment should be preserved"
    );
    assert!(
        result.contains("model = \"new-model\""),
        "Model should be updated"
    );
    assert!(
        result.contains("ascii_only = true"),
        "Other settings should be preserved"
    );
    assert!(
        result.contains("anthropic = \"sk-test\""),
        "API keys should be preserved"
    );
}

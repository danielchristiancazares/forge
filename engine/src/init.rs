//! Application initialization for the App.
//!
//! This module contains:
//! - `App::new` constructor
//! - Configuration loading and parsing
//! - Default constants for tool settings
//! - Data directory and path helpers

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use forge_types::{OutputLimits, Provider};

use crate::config::{self, ForgeConfig, OpenAIConfig};
use crate::state::{DataDir, DataDirSource, OperationState};
use crate::tools::{self, builtins};
use crate::ui::InputState;
use crate::{
    App, ContextManager, Librarian, OpenAIReasoningEffort, OpenAIReasoningSummary,
    OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation, StreamJournal, SystemPrompts,
    ToolJournal, UiOptions, ViewState,
};

// Tool limit defaults
pub(crate) const DEFAULT_MAX_TOOL_CALLS_PER_BATCH: usize = 8;
pub(crate) const DEFAULT_MAX_TOOL_ITERATIONS_PER_TURN: u32 = 4;
pub(crate) const DEFAULT_MAX_TOOL_ARGS_BYTES: usize = 256 * 1024;
pub(crate) const DEFAULT_MAX_TOOL_OUTPUT_BYTES: usize = 102_400;
pub(crate) const DEFAULT_MAX_PATCH_BYTES: usize = 512 * 1024;
pub(crate) const DEFAULT_MAX_READ_FILE_BYTES: usize = 200 * 1024;
pub(crate) const DEFAULT_MAX_READ_FILE_SCAN_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 30;
pub(crate) const DEFAULT_TOOL_FILE_TIMEOUT_SECS: u64 = 30;
pub(crate) const DEFAULT_TOOL_SHELL_TIMEOUT_SECS: u64 = 300;
pub(crate) const DEFAULT_TOOL_CAPACITY_BYTES: usize = 64 * 1024;
pub(crate) const TOOL_OUTPUT_SAFETY_MARGIN_TOKENS: u32 = 256;
pub(crate) const TOOL_EVENT_CHANNEL_CAPACITY: usize = 64;

// Environment variable denylist patterns
const DEFAULT_ENV_DENYLIST: [&str; 7] = [
    "*_KEY",
    "*_TOKEN",
    "*_SECRET",
    "*_PASSWORD",
    "AWS_*",
    "ANTHROPIC_*",
    "OPENAI_*",
];

// Sandbox deny patterns for sensitive files
const DEFAULT_SANDBOX_DENIES: [&str; 21] = [
    "**/.ssh/**",
    "**/.gnupg/**",
    "**/.aws/**",
    "**/.azure/**",
    "**/.config/gcloud/**",
    "**/.git/**",
    "**/.git-credentials",
    "**/.npmrc",
    "**/.pypirc",
    "**/.netrc",
    "**/.env",
    "**/.env.*",
    "**/*.env",
    "**/id_rsa*",
    "**/id_ed25519*",
    "**/id_ecdsa*",
    "**/*.pem",
    "**/*.key",
    "**/*.p12",
    "**/*.pfx",
    "**/*.der",
];

impl App {
    pub fn new(system_prompts: SystemPrompts) -> anyhow::Result<Self> {
        let (config, config_error) = match ForgeConfig::load() {
            Ok(config) => (config, None),
            Err(err) => (None, Some(err)),
        };

        // Load API keys from config, then fall back to environment.
        let mut api_keys = HashMap::new();
        if let Some(keys) = config.as_ref().and_then(|cfg| cfg.api_keys.as_ref()) {
            if let Some(key) = keys.anthropic.as_ref() {
                let resolved = config::expand_env_vars(key);
                let trimmed = resolved.trim();
                if !trimmed.is_empty() {
                    api_keys.insert(Provider::Claude, trimmed.to_string());
                }
            }
            if let Some(key) = keys.openai.as_ref() {
                let resolved = config::expand_env_vars(key);
                let trimmed = resolved.trim();
                if !trimmed.is_empty() {
                    api_keys.insert(Provider::OpenAI, trimmed.to_string());
                }
            }
            if let Some(key) = keys.google.as_ref() {
                let resolved = config::expand_env_vars(key);
                let trimmed = resolved.trim();
                if !trimmed.is_empty() {
                    api_keys.insert(Provider::Gemini, trimmed.to_string());
                }
            }
        }

        if let std::collections::hash_map::Entry::Vacant(e) = api_keys.entry(Provider::Claude)
            && let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                e.insert(key);
            }
        }
        if let std::collections::hash_map::Entry::Vacant(e) = api_keys.entry(Provider::OpenAI)
            && let Ok(key) = std::env::var("OPENAI_API_KEY")
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                e.insert(key);
            }
        }
        if let std::collections::hash_map::Entry::Vacant(e) = api_keys.entry(Provider::Gemini)
            && let Ok(key) = std::env::var("GEMINI_API_KEY")
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                e.insert(key);
            }
        }

        // Infer provider from model name, or fall back to API key detection
        let model_raw = config
            .as_ref()
            .and_then(|cfg| cfg.app.as_ref())
            .and_then(|app| app.model.as_ref());

        let provider = model_raw
            .and_then(|m| Provider::from_model_name(m))
            .or_else(|| {
                if api_keys.contains_key(&Provider::Claude) {
                    Some(Provider::Claude)
                } else if api_keys.contains_key(&Provider::OpenAI) {
                    Some(Provider::OpenAI)
                } else if api_keys.contains_key(&Provider::Gemini) {
                    Some(Provider::Gemini)
                } else {
                    None
                }
            })
            .unwrap_or(Provider::Claude);

        let model = model_raw
            .map(|raw| match provider.parse_model(raw) {
                Ok(model) => model,
                Err(err) => {
                    tracing::warn!("Invalid model in config: {err}");
                    provider.default_model()
                }
            })
            .unwrap_or_else(|| provider.default_model());

        let context_manager = ContextManager::new(model.as_str());
        let context_infinity_enabled = config
            .as_ref()
            .and_then(|cfg| cfg.context.as_ref())
            .and_then(|ctx| ctx.infinity)
            .unwrap_or_else(Self::context_infinity_enabled_from_env);

        let anthropic_config = config.as_ref().and_then(|cfg| cfg.anthropic.as_ref());

        // Load cache config (default: enabled)
        let cache_enabled = anthropic_config
            .and_then(|cfg| cfg.cache_enabled)
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|cfg| cfg.cache.as_ref())
                    .and_then(|cache| cache.enabled)
            })
            .unwrap_or(true);

        // Build OutputLimits at the boundary - validates invariants here, not at runtime
        let output_limits = {
            let max_output = config
                .as_ref()
                .and_then(|cfg| cfg.app.as_ref())
                .and_then(|app| app.max_output_tokens)
                .unwrap_or(16_000); // Default max output

            let thinking_enabled = anthropic_config
                .and_then(|cfg| cfg.thinking_enabled)
                .or_else(|| {
                    config
                        .as_ref()
                        .and_then(|cfg| cfg.thinking.as_ref())
                        .and_then(|t| t.enabled)
                })
                .unwrap_or(false);

            if thinking_enabled {
                let budget = anthropic_config
                    .and_then(|cfg| cfg.thinking_budget_tokens)
                    .or_else(|| {
                        config
                            .as_ref()
                            .and_then(|cfg| cfg.thinking.as_ref())
                            .and_then(|t| t.budget_tokens)
                    })
                    .unwrap_or(10_000);

                // Validate at boundary - if invalid, warn and fall back to no thinking
                match OutputLimits::with_thinking(max_output, budget) {
                    Ok(limits) => limits,
                    Err(e) => {
                        tracing::warn!(
                            "Invalid thinking config: {e}. Disabling extended thinking."
                        );
                        OutputLimits::new(max_output)
                    }
                }
            } else {
                OutputLimits::new(max_output)
            }
        };

        let openai_options = Self::openai_request_options_from_config(
            config.as_ref().and_then(|cfg| cfg.openai.as_ref()),
        );

        // Load Gemini cache config
        let gemini_config = config.as_ref().and_then(|cfg| cfg.google.as_ref());
        let gemini_cache_config = crate::GeminiCacheConfig {
            enabled: gemini_config
                .and_then(|cfg| cfg.cache_enabled)
                .unwrap_or(false), // Default disabled - requires explicit opt-in
            ttl_seconds: gemini_config
                .and_then(|cfg| cfg.cache_ttl_seconds)
                .unwrap_or(3600), // Default 1 hour
        };
        let gemini_thinking_enabled = gemini_config
            .and_then(|cfg| cfg.thinking_enabled)
            .unwrap_or(false);

        let data_dir = Self::data_dir();

        // Initialize Librarian for Context Infinity (if enabled and Gemini API key available)
        let librarian = if context_infinity_enabled {
            if let Some(gemini_key) = api_keys.get(&Provider::Gemini).cloned() {
                let librarian_path = data_dir.join("librarian.db");
                match Librarian::open(&librarian_path, gemini_key) {
                    Ok(lib) => {
                        tracing::info!("Librarian initialized with {} facts", lib.fact_count());
                        Some(std::sync::Arc::new(tokio::sync::Mutex::new(lib)))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to initialize Librarian: {e}");
                        None
                    }
                }
            } else {
                tracing::info!(
                    "Context Infinity enabled but no Gemini API key - Librarian disabled"
                );
                None
            }
        } else {
            None
        };

        Self::ensure_secure_dir(&data_dir.path)?;

        // Initialize stream journal (required for streaming durability).
        let journal_path = data_dir.join("stream_journal.db");
        let stream_journal = StreamJournal::open(&journal_path)?;

        // Tool settings and registry.
        let tool_settings = Self::tool_settings_from_config(config.as_ref());
        let mut tool_registry = tools::ToolRegistry::default();
        if let Err(e) = builtins::register_builtins(
            &mut tool_registry,
            tool_settings.read_limits,
            tool_settings.patch_limits,
            tool_settings.search.clone(),
            tool_settings.webfetch.clone(),
            tool_settings.shell.clone(),
        ) {
            tracing::warn!("Failed to register built-in tools: {e}");
        }
        let tool_registry = std::sync::Arc::new(tool_registry);
        let tool_definitions = tool_registry.definitions();

        let tool_journal_path = data_dir.join("tool_journal.db");
        let tool_journal = ToolJournal::open(&tool_journal_path)?;
        let tool_file_cache =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

        let ui_options = Self::ui_options_from_config(config.as_ref());
        let view = ViewState {
            ui_options,
            ..ViewState::default()
        };

        let mut app = Self {
            input: InputState::default(),
            display: Vec::new(),
            display_version: 0,
            should_quit: false,
            view,
            api_keys,
            model,
            tick: 0,
            data_dir,
            context_manager,
            stream_journal,
            state: OperationState::Idle,
            context_infinity: context_infinity_enabled,
            output_limits,
            cache_enabled,
            openai_options,
            system_prompts,
            cached_usage_status: None,
            pending_user_message: None,
            tool_definitions,
            tool_registry,
            tool_settings,
            tool_journal,
            tool_file_cache,
            checkpoints: crate::checkpoints::CheckpointStore::default(),
            tool_iterations: 0,
            history_load_warning_shown: false,
            autosave_warning_shown: false,
            gemini_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            gemini_thinking_enabled,
            gemini_cache_config,
            librarian,
            input_history: crate::ui::InputHistory::default(),
            last_ui_tick: Instant::now(),
            last_session_autosave: Instant::now(),
            session_changes: crate::session_state::SessionChangeLog::default(),
            file_picker: crate::ui::FilePickerState::new(),
            turn_usage: None,
            last_turn_usage: None,
            notification_queue: crate::notifications::NotificationQueue::new(),
        };

        app.clamp_output_limits_to_model();
        // Sync output limit to context manager for accurate budget calculation
        app.context_manager
            .set_output_limit(app.output_limits.max_output_tokens());

        // Load previous session's history if available
        app.load_history_if_exists();
        // Load session state (draft input + input history)
        app.load_session();
        app.check_crash_recovery();
        if let Some(err) = config_error {
            let path = err.path().display().to_string();
            let message = match &err {
                config::ConfigError::Parse { source, .. } => {
                    format!("Couldn't parse {path} ({source}). Using defaults.")
                }
                config::ConfigError::Read { source, .. } => {
                    format!("Couldn't read {path} ({source}). Using defaults.")
                }
            };
            app.push_notification(message);
        }

        if matches!(app.data_dir.source, DataDirSource::Fallback) {
            app.push_notification(format!(
                "Using fallback data dir: {}",
                app.data_dir.path.display()
            ));
        }

        Ok(app)
    }

    fn ui_options_from_config(config: Option<&ForgeConfig>) -> UiOptions {
        let app = config.and_then(|cfg| cfg.app.as_ref());
        UiOptions {
            ascii_only: app.and_then(|cfg| cfg.ascii_only).unwrap_or(false),
            high_contrast: app.and_then(|cfg| cfg.high_contrast).unwrap_or(false),
            reduced_motion: app.and_then(|cfg| cfg.reduced_motion).unwrap_or(false),
            show_thinking: app.and_then(|cfg| cfg.show_thinking).unwrap_or(false),
        }
    }

    /// Get the base data directory for forge.
    pub(crate) fn data_dir() -> DataDir {
        match dirs::data_local_dir() {
            Some(path) => DataDir {
                path: path.join("forge"),
                source: DataDirSource::System,
            },
            None => DataDir {
                path: PathBuf::from(".").join("forge"),
                source: DataDirSource::Fallback,
            },
        }
    }

    pub(crate) fn ensure_secure_dir(path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = std::fs::metadata(path)?;

            // Only modify permissions if we own the directory
            let our_uid = unsafe { libc::getuid() };
            if metadata.uid() != our_uid {
                // Not our directory - skip silently (e.g., /tmp)
                return Ok(());
            }

            // Check if permissions are already secure (0o700 or stricter)
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                tracing::warn!(
                    "Data dir permissions are too open ({:o}); tightening to 0700",
                    mode
                );
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    /// Get the path to the history file.
    pub(crate) fn history_path(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    /// Get the path to the session state file.
    pub(crate) fn session_path(&self) -> std::path::PathBuf {
        self.data_dir
            .join(crate::session_state::SessionState::FILENAME)
    }

    pub(crate) fn context_infinity_enabled_from_env() -> bool {
        match std::env::var("FORGE_CONTEXT_INFINITY") {
            Ok(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            ),
            Err(_) => true,
        }
    }

    fn openai_request_options_from_config(config: Option<&OpenAIConfig>) -> OpenAIRequestOptions {
        let reasoning_effort = config
            .and_then(|cfg| cfg.reasoning_effort.as_deref())
            .map(|raw| {
                OpenAIReasoningEffort::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI reasoning_effort in config: {raw}");
                    OpenAIReasoningEffort::default()
                })
            })
            .unwrap_or_default();

        let reasoning_summary = config
            .and_then(|cfg| cfg.reasoning_summary.as_deref())
            .map(|raw| {
                OpenAIReasoningSummary::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI reasoning_summary in config: {raw}");
                    OpenAIReasoningSummary::default()
                })
            })
            .unwrap_or_default();

        let verbosity = config
            .and_then(|cfg| cfg.verbosity.as_deref())
            .map(|raw| {
                OpenAITextVerbosity::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI verbosity in config: {raw}");
                    OpenAITextVerbosity::default()
                })
            })
            .unwrap_or_default();

        let truncation = config
            .and_then(|cfg| cfg.truncation.as_deref())
            .map(|raw| {
                OpenAITruncation::parse(raw).unwrap_or_else(|| {
                    tracing::warn!("Unknown OpenAI truncation in config: {raw}");
                    OpenAITruncation::default()
                })
            })
            .unwrap_or_default();

        OpenAIRequestOptions::new(reasoning_effort, reasoning_summary, verbosity, truncation)
    }

    pub(crate) fn tool_settings_from_config(config: Option<&ForgeConfig>) -> tools::ToolSettings {
        let tools_cfg = config.and_then(|cfg| cfg.tools.as_ref());

        let limits = tools::ToolLimits {
            max_tool_calls_per_batch: tools_cfg
                .and_then(|cfg| cfg.max_tool_calls_per_batch)
                .unwrap_or(DEFAULT_MAX_TOOL_CALLS_PER_BATCH),
            max_tool_iterations_per_user_turn: tools_cfg
                .and_then(|cfg| cfg.max_tool_iterations_per_user_turn)
                .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS_PER_TURN),
            max_tool_args_bytes: DEFAULT_MAX_TOOL_ARGS_BYTES,
        };

        let read_limits = tools::ReadFileLimits {
            max_file_read_bytes: tools_cfg
                .and_then(|cfg| cfg.read_file.as_ref())
                .and_then(|cfg| cfg.max_file_read_bytes)
                .unwrap_or(DEFAULT_MAX_READ_FILE_BYTES),
            max_scan_bytes: tools_cfg
                .and_then(|cfg| cfg.read_file.as_ref())
                .and_then(|cfg| cfg.max_scan_bytes)
                .unwrap_or(DEFAULT_MAX_READ_FILE_SCAN_BYTES),
        };

        let patch_limits = tools::PatchLimits {
            max_patch_bytes: tools_cfg
                .and_then(|cfg| cfg.apply_patch.as_ref())
                .and_then(|cfg| cfg.max_patch_bytes)
                .unwrap_or(DEFAULT_MAX_PATCH_BYTES),
        };

        let search_cfg = tools_cfg.and_then(|cfg| cfg.search.as_ref());
        let search = tools::SearchToolConfig {
            binary: search_cfg
                .and_then(|cfg| cfg.binary.clone())
                .unwrap_or_else(|| "ugrep".to_string()),
            fallback_binary: search_cfg
                .and_then(|cfg| cfg.fallback_binary.clone())
                .unwrap_or_else(|| "rg".to_string()),
            default_timeout_ms: search_cfg
                .and_then(|cfg| cfg.default_timeout_ms)
                .unwrap_or(20_000),
            default_max_results: search_cfg
                .and_then(|cfg| cfg.default_max_results)
                .unwrap_or(200),
            max_matches_per_file: search_cfg
                .and_then(|cfg| cfg.max_matches_per_file)
                .unwrap_or(50),
            max_files: search_cfg.and_then(|cfg| cfg.max_files).unwrap_or(10_000),
            max_file_size_bytes: search_cfg
                .and_then(|cfg| cfg.max_file_size_bytes)
                .unwrap_or(2_000_000),
        };

        let webfetch_cfg = tools_cfg.and_then(|cfg| cfg.webfetch.as_ref());
        let webfetch = tools::WebFetchToolConfig {
            user_agent: webfetch_cfg.and_then(|cfg| cfg.user_agent.clone()),
            timeout_seconds: webfetch_cfg
                .and_then(|cfg| cfg.timeout_seconds)
                .unwrap_or(20),
            max_redirects: webfetch_cfg.and_then(|cfg| cfg.max_redirects).unwrap_or(5),
            default_max_chunk_tokens: webfetch_cfg
                .and_then(|cfg| cfg.default_max_chunk_tokens)
                .unwrap_or(600),
            max_download_bytes: webfetch_cfg
                .and_then(|cfg| cfg.max_download_bytes)
                .unwrap_or(10 * 1024 * 1024),
            cache_dir: webfetch_cfg
                .and_then(|cfg| cfg.cache_dir.as_ref())
                .map(|s| PathBuf::from(config::expand_env_vars(s))),
            cache_ttl_days: webfetch_cfg.and_then(|cfg| cfg.cache_ttl_days).unwrap_or(7),
        };

        let shell_cfg = tools_cfg.and_then(|cfg| cfg.shell.as_ref());
        let shell = tools::shell::detect_shell(shell_cfg);
        tracing::info!(shell = %shell.name, binary = ?shell.binary, "Detected shell");

        let timeouts = tools::ToolTimeouts {
            default_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.default_seconds)
                    .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS),
            ),
            file_operations_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.file_operations_seconds)
                    .unwrap_or(DEFAULT_TOOL_FILE_TIMEOUT_SECS),
            ),
            shell_commands_timeout: Duration::from_secs(
                tools_cfg
                    .and_then(|cfg| cfg.timeouts.as_ref())
                    .and_then(|cfg| cfg.shell_commands_seconds)
                    .unwrap_or(DEFAULT_TOOL_SHELL_TIMEOUT_SECS),
            ),
        };

        let max_output_bytes = tools_cfg
            .and_then(|cfg| cfg.output.as_ref())
            .and_then(|cfg| cfg.max_bytes)
            .unwrap_or(DEFAULT_MAX_TOOL_OUTPUT_BYTES);

        let policy_cfg = tools_cfg.and_then(|cfg| cfg.approval.as_ref());
        let policy = tools::Policy {
            mode: parse_approval_mode(policy_cfg.and_then(|cfg| cfg.mode.as_deref())),
            allowlist: {
                let list = policy_cfg
                    .map(|cfg| cfg.allowlist.clone())
                    .unwrap_or_else(|| {
                        vec![
                            "Read".to_string(),
                            "GitStatus".to_string(),
                            "GitDiff".to_string(),
                            "GitLog".to_string(),
                            "GitShow".to_string(),
                            "GitBlame".to_string(),
                        ]
                    });
                list.into_iter().collect()
            },
            denylist: {
                let list = if policy_cfg.map(|cfg| &cfg.denylist).is_some() {
                    policy_cfg
                        .map(|cfg| cfg.denylist.clone())
                        .unwrap_or_default()
                } else {
                    vec!["Bash".to_string()]
                };
                list.into_iter().collect()
            },
        };

        let env_patterns: Vec<String> = tools_cfg
            .and_then(|cfg| cfg.environment.as_ref())
            .map(|cfg| cfg.denylist.clone())
            .filter(|list| !list.is_empty())
            .unwrap_or_else(|| {
                DEFAULT_ENV_DENYLIST
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            });
        let env_sanitizer = tools::EnvSanitizer::new(&env_patterns).unwrap_or_else(|e| {
            tracing::warn!("Invalid env denylist: {e}. Using defaults.");
            tools::EnvSanitizer::new(
                &DEFAULT_ENV_DENYLIST
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .expect("default env sanitizer")
        });

        let sandbox_cfg = tools_cfg.and_then(|cfg| cfg.sandbox.as_ref());
        let include_default_denies = sandbox_cfg
            .and_then(|cfg| cfg.include_default_denies)
            .unwrap_or(true);
        let mut denied_patterns = sandbox_cfg
            .map(|cfg| cfg.denied_patterns.clone())
            .unwrap_or_default();
        if include_default_denies {
            denied_patterns.extend(
                DEFAULT_SANDBOX_DENIES
                    .iter()
                    .map(std::string::ToString::to_string),
            );
        }

        let mut allowed_roots: Vec<PathBuf> = sandbox_cfg
            .map(|cfg| cfg.allowed_roots.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|raw| PathBuf::from(config::expand_env_vars(&raw)))
            .collect();
        if allowed_roots.is_empty() {
            allowed_roots.push(PathBuf::from("."));
        }
        let allow_absolute = sandbox_cfg
            .and_then(|cfg| cfg.allow_absolute)
            .unwrap_or(false);

        let sandbox = tools::sandbox::Sandbox::new(
            allowed_roots.clone(),
            denied_patterns.clone(),
            allow_absolute,
        )
        .unwrap_or_else(|e| {
            tracing::warn!("Invalid sandbox config: {e}. Using defaults.");
            tools::sandbox::Sandbox::new(
                vec![PathBuf::from(".")],
                DEFAULT_SANDBOX_DENIES
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                false,
            )
            .expect("default sandbox")
        });

        // Command blacklist initialization (patterns defined in command_blacklist module)
        let command_blacklist = tools::CommandBlacklist::with_defaults().unwrap_or_else(|e| {
            tracing::error!("Failed to compile command blacklist: {e}. Using empty blacklist.");
            tools::CommandBlacklist::new(&[]).expect("empty blacklist")
        });

        tools::ToolSettings {
            limits,
            read_limits,
            patch_limits,
            search,
            webfetch,
            shell,
            timeouts,
            max_output_bytes,
            policy,
            sandbox,
            env_sanitizer,
            command_blacklist,
        }
    }
}

fn parse_approval_mode(raw: Option<&str>) -> tools::ApprovalMode {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("permissive" | "auto") => tools::ApprovalMode::Permissive,
        Some("strict" | "deny") => tools::ApprovalMode::Strict,
        // "default", "prompt", or anything else
        _ => tools::ApprovalMode::Default,
    }
}

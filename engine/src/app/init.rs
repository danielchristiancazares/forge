//! Application initialization for the App.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use forge_types::{ModelName, OutputLimits, Provider, SecretString, ToolDefinition};

use super::{LspRuntimeState, ProviderRuntimeState, SystemPrompts};
use crate::config::{self, ForgeConfig, OpenAIConfig};
use crate::state::{DataDir, DataDirSource, OperationState};
use crate::tools::{self, builtins};
use crate::ui::{DisplayLog, InputState};
use crate::{
    App, ContextManager, EnvironmentContext, Librarian, OpenAIReasoningEffort,
    OpenAIReasoningSummary, OpenAIRequestOptions, OpenAITextVerbosity, OpenAITruncation,
    StreamJournal, ToolJournal, UiOptions, ViewMode, ViewState,
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

use forge_types::ENV_SECRET_DENYLIST as DEFAULT_ENV_DENYLIST;

fn default_env_denylist_patterns() -> Vec<String> {
    DEFAULT_ENV_DENYLIST
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

fn default_sandbox_deny_patterns() -> Vec<String> {
    tools::sandbox::default_sandbox_deny_patterns()
}

fn insert_resolved_key(
    api_keys: &mut HashMap<Provider, SecretString>,
    provider: Provider,
    raw: Option<&str>,
) {
    if let Some(raw) = raw {
        let resolved = config::expand_env_vars(raw);
        let trimmed = resolved.trim();
        if !trimmed.is_empty() {
            api_keys.insert(provider, SecretString::new(trimmed.to_string()));
        }
    }
}

fn insert_env_key_if_missing(api_keys: &mut HashMap<Provider, SecretString>, provider: Provider) {
    use std::collections::hash_map::Entry;

    if let Entry::Vacant(e) = api_keys.entry(provider)
        && let Ok(key) = std::env::var(provider.env_var())
    {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            e.insert(SecretString::new(trimmed.to_string()));
        }
    }
}

pub(crate) struct AppBuildParts {
    pub(crate) view: ViewState,
    pub(crate) configured_model: ModelName,
    pub(crate) configured_tool_approval_mode: tools::ApprovalMode,
    pub(crate) configured_context_memory_enabled: bool,
    pub(crate) configured_ui_options: UiOptions,
    pub(crate) api_keys: HashMap<Provider, SecretString>,
    pub(crate) config_path: PathBuf,
    pub(crate) model: ModelName,
    pub(crate) data_dir: DataDir,
    pub(crate) context_manager: ContextManager,
    pub(crate) stream_journal: StreamJournal,
    pub(crate) memory_enabled: bool,
    pub(crate) output_limits: OutputLimits,
    pub(crate) configured_output_limits: OutputLimits,
    pub(crate) cache_enabled: bool,
    pub(crate) provider_runtime: ProviderRuntimeState,
    pub(crate) system_prompts: SystemPrompts,
    pub(crate) environment: EnvironmentContext,
    pub(crate) tool_definitions: Vec<ToolDefinition>,
    pub(crate) hidden_tools: std::collections::HashSet<String>,
    pub(crate) tool_registry: std::sync::Arc<tools::ToolRegistry>,
    pub(crate) tool_settings: tools::ToolSettings,
    pub(crate) tool_journal: ToolJournal,
    pub(crate) tool_file_cache: std::sync::Arc<tokio::sync::Mutex<tools::ToolFileCache>>,
    pub(crate) librarian: Option<std::sync::Arc<tokio::sync::Mutex<Librarian>>>,
    pub(crate) lsp_config: Option<forge_lsp::LspConfig>,
}

pub(crate) fn build_app(parts: AppBuildParts) -> App {
    App {
        input: InputState::default(),
        display: DisplayLog::default(),
        should_quit: false,
        view: parts.view,
        configured_model: parts.configured_model,
        configured_tool_approval_mode: parts.configured_tool_approval_mode,
        configured_context_memory_enabled: parts.configured_context_memory_enabled,
        configured_ui_options: parts.configured_ui_options,
        pending_turn_model: None,
        pending_turn_tool_approval_mode: None,
        pending_turn_context_memory_enabled: None,
        pending_turn_ui_options: None,
        settings_editor: super::SettingsEditorState::Inactive,
        api_keys: parts.api_keys,
        config_path: parts.config_path,
        model: parts.model,
        tick: 0,
        data_dir: parts.data_dir,
        context_manager: parts.context_manager,
        stream_journal: parts.stream_journal,
        state: OperationState::Idle,
        memory_enabled: parts.memory_enabled,
        output_limits: parts.output_limits,
        configured_output_limits: parts.configured_output_limits,
        cache_enabled: parts.cache_enabled,
        provider_runtime: parts.provider_runtime,
        system_prompts: parts.system_prompts,
        environment: parts.environment,
        cached_usage_status: None,
        pending_user_message: None,
        tool_definitions: parts.tool_definitions,
        hidden_tools: parts.hidden_tools,
        tool_registry: parts.tool_registry,
        tool_settings: parts.tool_settings,
        tool_journal: parts.tool_journal,
        tool_gate: super::ToolGate::Enabled,
        pending_stream_cleanup: None,
        pending_stream_cleanup_failures: 0,
        pending_tool_cleanup: None,
        pending_tool_cleanup_failures: 0,
        tool_file_cache: parts.tool_file_cache,
        checkpoints: super::checkpoints::CheckpointStore::default(),
        tool_iterations: 0,
        history_load_warning_shown: false,
        autosave_warning_shown: false,
        librarian: parts.librarian,
        input_history: crate::ui::InputHistory::default(),
        last_ui_tick: Instant::now(),
        last_session_autosave: Instant::now(),
        next_journal_cleanup_attempt: Instant::now(),
        session_changes: crate::session_state::SessionChangeLog::default(),
        file_picker: crate::ui::FilePickerState::new(),
        turn_usage: None,
        last_turn_usage: None,
        notification_queue: crate::notifications::NotificationQueue::new(),
        lsp_runtime: LspRuntimeState {
            config: parts.lsp_config,
            manager: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            snapshot: forge_lsp::DiagnosticsSnapshot::default(),
            pending_diag_check: None,
        },
        plan_state: crate::PlanState::Inactive,
    }
}

impl App {
    pub fn new(system_prompts: SystemPrompts) -> anyhow::Result<Self> {
        let (config, config_error) = match ForgeConfig::load() {
            Ok(config) => (config, None),
            Err(err) => (None, Some(err)),
        };

        // Load API keys from config, then fall back to environment.
        let mut api_keys = HashMap::new();
        if let Some(keys) = config.as_ref().and_then(|cfg| cfg.api_keys.as_ref()) {
            for (provider, key) in [
                (Provider::Claude, keys.anthropic.as_deref()),
                (Provider::OpenAI, keys.openai.as_deref()),
                (Provider::Gemini, keys.google.as_deref()),
            ] {
                insert_resolved_key(&mut api_keys, provider, key);
            }
        }

        for &provider in Provider::all() {
            insert_env_key_if_missing(&mut api_keys, provider);
        }

        let app_config = config.as_ref().and_then(|cfg| cfg.app.as_ref());

        // Infer provider from model name, or fall back to API key detection
        let model_raw = app_config.and_then(|app| app.model.as_ref());

        let provider = model_raw
            .and_then(|m| Provider::from_model_name(m).ok())
            .or_else(|| {
                Provider::all()
                    .iter()
                    .copied()
                    .find(|p| api_keys.contains_key(p))
            })
            .unwrap_or(Provider::Claude);

        let base_model = model_raw
            .map(|raw| match provider.parse_model(raw) {
                Ok(model) => model,
                Err(err) => {
                    tracing::warn!("Invalid model in config: {err}");
                    provider.default_model()
                }
            })
            .unwrap_or_else(|| provider.default_model());
        // Backwards compat: if no `model` key but `chat_model` exists, use it (boundary migration).
        let configured_model = if model_raw.is_some() {
            base_model.clone()
        } else if let Some(chat_model_raw) = app_config.and_then(|app| app.chat_model.as_ref()) {
            if let Some(model) = Provider::from_model_name(chat_model_raw)
                .ok()
                .and_then(|p| p.parse_model(chat_model_raw).ok())
            {
                tracing::warn!(
                    "Config uses legacy chat_model key; migrating to model = \"{chat_model_raw}\""
                );
                model
            } else {
                tracing::warn!("Invalid app.chat_model in config: \"{chat_model_raw}\"");
                base_model.clone()
            }
        } else {
            base_model.clone()
        };
        let model = configured_model.clone();

        let context_manager = ContextManager::new(model.clone());
        let memory_enabled = config
            .as_ref()
            .and_then(|cfg| cfg.context.as_ref())
            .map_or_else(Self::context_infinity_enabled_from_env, |ctx| ctx.memory);

        let anthropic_config = config.as_ref().and_then(|cfg| cfg.anthropic.as_ref());

        // Load cache config (default: enabled)
        let cache_enabled = anthropic_config
            .map(|cfg| cfg.cache_enabled)
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|cfg| cfg.cache.as_ref())
                    .map(|cache| cache.enabled)
            })
            .unwrap_or(true);

        // Resolve thinking mode first — it drives OutputLimits construction
        let mut anthropic_thinking_mode = anthropic_config
            .map(|cfg| cfg.thinking_mode)
            .unwrap_or_default();
        let anthropic_thinking_effort = anthropic_config
            .map(|cfg| cfg.thinking_effort)
            .unwrap_or_default();

        // Build OutputLimits at the boundary - validates invariants here, not at runtime
        // Use the model's max_output from the registry (provider-specific limits)
        let output_limits = {
            let max_output = context_manager.current_limits().max_output();

            // thinking_mode = "enabled" implies thinking is on (no separate flag needed).
            // Legacy thinking_enabled is still honored for pre-4.6 models.
            let thinking_enabled = anthropic_thinking_mode
                == config::AnthropicThinkingMode::Enabled
                || anthropic_config
                    .map(|cfg| cfg.thinking_enabled)
                    .or_else(|| {
                        config
                            .as_ref()
                            .and_then(|cfg| cfg.thinking.as_ref())
                            .map(|t| t.enabled)
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
                        if anthropic_thinking_mode == config::AnthropicThinkingMode::Enabled {
                            tracing::warn!("Falling back to adaptive thinking.");
                            anthropic_thinking_mode = config::AnthropicThinkingMode::Adaptive;
                        }
                        OutputLimits::new(max_output)
                    }
                }
            } else {
                OutputLimits::new(max_output)
            }
        };

        let openai_cfg = config.as_ref().and_then(|cfg| cfg.openai.as_ref());
        let openai_reasoning_effort_explicit = openai_cfg
            .and_then(|cfg| cfg.reasoning_effort.as_deref())
            .is_some();
        let openai_options = Self::openai_request_options_from_config(openai_cfg);

        // Load Gemini cache config
        let gemini_config = config.as_ref().and_then(|cfg| cfg.google.as_ref());
        let gemini_cache_config = crate::GeminiCacheConfig {
            enabled: gemini_config.is_some_and(|cfg| cfg.cache_enabled), // Default disabled - requires explicit opt-in
            ttl_seconds: gemini_config
                .and_then(|cfg| cfg.cache_ttl_seconds)
                .unwrap_or(3600), // Default 1 hour
        };
        let gemini_thinking_enabled = gemini_config.is_some_and(|cfg| cfg.thinking_enabled);

        let data_dir = Self::data_dir()?;

        // Initialize Librarian for memory when enabled.
        let librarian = if memory_enabled {
            let librarian_path = data_dir.join("librarian.db");
            match Librarian::open(&librarian_path) {
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
            None
        };

        Self::ensure_secure_dir(&data_dir.path)?;

        // Initialize stream journal (required for streaming durability).
        let journal_path = data_dir.join("stream_journal.db");
        let stream_journal = StreamJournal::open(&journal_path)?;

        // Tool settings and registry.
        let tool_settings = Self::tool_settings_from_config(config.as_ref());
        let (tool_registry, tool_definitions, hidden_tools) =
            Self::build_tool_registry(&tool_settings);

        let tool_journal_path = data_dir.join("tool_journal.db");
        let tool_journal = ToolJournal::open(&tool_journal_path)?;
        let tool_file_cache =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

        // Store LSP config for lazy start on first tool batch
        let lsp_config = config
            .as_ref()
            .and_then(|cfg| cfg.lsp.clone())
            .filter(forge_lsp::LspConfig::enabled);

        let environment = EnvironmentContext::gather();

        let ui_options = Self::ui_options_from_config(config.as_ref());
        let view = ViewState {
            ui_options,
            view_mode: Self::view_mode_from_config(config.as_ref()),
            ..ViewState::default()
        };
        let configured_tool_approval_mode = tool_settings.policy.mode;

        let mut app = build_app(AppBuildParts {
            view,
            configured_model: configured_model.clone(),
            configured_tool_approval_mode,
            configured_context_memory_enabled: memory_enabled,
            configured_ui_options: ui_options,
            api_keys,
            config_path: config::config_path().unwrap_or_default(),
            model,
            data_dir,
            context_manager,
            stream_journal,
            memory_enabled,
            output_limits,
            configured_output_limits: output_limits,
            cache_enabled,
            provider_runtime: ProviderRuntimeState {
                openai_options,
                openai_reasoning_effort_explicit,
                gemini_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                gemini_thinking_enabled,
                anthropic_thinking_mode,
                anthropic_thinking_effort,
                gemini_cache_config,
                openai_previous_response_id: None,
            },
            system_prompts,
            environment,
            tool_definitions,
            hidden_tools,
            tool_registry,
            tool_settings,
            tool_journal,
            tool_file_cache,
            librarian,
            lsp_config,
        });

        app.reconcile_output_limits_with_model();
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

        if matches!(app.data_dir.source, DataDirSource::Custom) {
            app.push_notification(format!(
                "Using custom data dir: {}",
                app.data_dir.path.display()
            ));
        }

        Ok(app)
    }

    fn ui_options_from_config(config: Option<&ForgeConfig>) -> UiOptions {
        let app = config.and_then(|cfg| cfg.app.as_ref());
        UiOptions {
            ascii_only: app.is_some_and(|cfg| cfg.ascii_only),
            high_contrast: app.is_some_and(|cfg| cfg.high_contrast),
            reduced_motion: app.is_some_and(|cfg| cfg.reduced_motion),
            show_thinking: app.is_some_and(|cfg| cfg.show_thinking),
        }
    }

    fn view_mode_from_config(config: Option<&ForgeConfig>) -> ViewMode {
        let app = config.and_then(|cfg| cfg.app.as_ref());
        #[cfg(feature = "focus-view")]
        {
            if app.is_some_and(|cfg| cfg.focus_view) {
                ViewMode::Focus
            } else {
                ViewMode::Classic
            }
        }
        #[cfg(not(feature = "focus-view"))]
        {
            let _ = app;
            ViewMode::Classic
        }
    }

    pub(crate) fn data_dir() -> anyhow::Result<DataDir> {
        if let Ok(custom) = std::env::var("FORGE_DATA_DIR") {
            return Ok(DataDir {
                path: PathBuf::from(custom),
                source: DataDirSource::Custom,
            });
        }
        if let Some(path) = dirs::data_local_dir() {
            Ok(DataDir {
                path: path.join("forge"),
                source: DataDirSource::System,
            })
        } else {
            anyhow::bail!(
                "System data directory unavailable. Set FORGE_DATA_DIR to specify a data directory."
            )
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

    pub(crate) fn history_path(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    pub(crate) fn session_path(&self) -> std::path::PathBuf {
        self.data_dir
            .join(crate::session_state::SessionState::FILENAME)
    }

    pub(crate) fn plan_path(&self) -> PathBuf {
        self.data_dir.join("plan.json")
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
        let reasoning_effort = parse_config_enum_or_default(
            config.and_then(|cfg| cfg.reasoning_effort.as_deref()),
            "OpenAI reasoning_effort",
            OpenAIReasoningEffort::parse,
        );

        let reasoning_summary = parse_config_enum_or_default(
            config.and_then(|cfg| cfg.reasoning_summary.as_deref()),
            "OpenAI reasoning_summary",
            OpenAIReasoningSummary::parse,
        );

        let verbosity = parse_config_enum_or_default(
            config.and_then(|cfg| cfg.verbosity.as_deref()),
            "OpenAI verbosity",
            OpenAITextVerbosity::parse,
        );

        let truncation = parse_config_enum_or_default(
            config.and_then(|cfg| cfg.truncation.as_deref()),
            "OpenAI truncation",
            OpenAITruncation::parse,
        );

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

        let windows_run_cfg = tools_cfg
            .and_then(|cfg| cfg.run.as_ref())
            .and_then(|cfg| cfg.windows.as_ref());
        let macos_run_cfg = tools_cfg
            .and_then(|cfg| cfg.run.as_ref())
            .and_then(|cfg| cfg.macos.as_ref());
        let run_policy = tools::RunSandboxPolicy {
            windows: tools::WindowsRunSandboxPolicy {
                enabled: windows_run_cfg.is_none_or(|cfg| cfg.enabled),
                enforce_powershell_only: true,
                block_network: true,
                fallback_mode: parse_run_fallback_mode(
                    windows_run_cfg.map(|cfg| cfg.fallback_mode),
                ),
            },
            macos: tools::MacOsRunSandboxPolicy {
                enabled: macos_run_cfg.is_none_or(|cfg| cfg.enabled),
                fallback_mode: parse_run_fallback_mode(macos_run_cfg.map(|cfg| cfg.fallback_mode)),
            },
        };

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
                    .map_or_else(|| vec!["Read".to_string()], |cfg| cfg.allowlist.clone());
                list.into_iter().collect()
            },
            denylist: {
                let list = if policy_cfg.map(|cfg| &cfg.denylist).is_some() {
                    policy_cfg
                        .map(|cfg| cfg.denylist.clone())
                        .unwrap_or_default()
                } else {
                    vec!["Run".to_string()]
                };
                list.into_iter().collect()
            },
        };

        let env_cfg = tools_cfg.and_then(|cfg| cfg.environment.as_ref());
        let include_default_env = env_cfg.is_none_or(|cfg| cfg.include_default_denies);
        let env_patterns: Vec<String> = {
            let mut patterns = if include_default_env {
                default_env_denylist_patterns()
            } else {
                Vec::new()
            };
            if let Some(cfg) = env_cfg {
                for p in &cfg.denylist {
                    if !patterns.iter().any(|existing| existing == p) {
                        patterns.push(p.clone());
                    }
                }
            }
            if patterns.is_empty() {
                default_env_denylist_patterns()
            } else {
                patterns
            }
        };
        let env_sanitizer = tools::EnvSanitizer::new(&env_patterns).unwrap_or_else(|e| {
            tracing::warn!("Invalid env denylist: {e}. Using defaults.");
            tools::EnvSanitizer::new(&default_env_denylist_patterns())
                .expect("default env sanitizer")
        });

        let sandbox_cfg = tools_cfg.and_then(|cfg| cfg.sandbox.as_ref());
        let include_default_denies = sandbox_cfg.is_none_or(|cfg| cfg.include_default_denies);
        let mut denied_patterns = sandbox_cfg
            .map(|cfg| cfg.denied_patterns.clone())
            .unwrap_or_default();
        if include_default_denies {
            denied_patterns.extend(default_sandbox_deny_patterns());
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
        let allow_absolute = sandbox_cfg.is_some_and(|cfg| cfg.allow_absolute);

        let sandbox = tools::sandbox::Sandbox::new(
            allowed_roots.clone(),
            denied_patterns.clone(),
            allow_absolute,
        )
        .unwrap_or_else(|e| {
            tracing::warn!("Invalid sandbox config: {e}. Using defaults.");
            tools::sandbox::Sandbox::new(
                vec![PathBuf::from(".")],
                default_sandbox_deny_patterns(),
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
            run_policy,
        }
    }

    pub(crate) fn build_tool_registry(
        tool_settings: &tools::ToolSettings,
    ) -> (
        std::sync::Arc<tools::ToolRegistry>,
        Vec<ToolDefinition>,
        std::collections::HashSet<String>,
    ) {
        let mut tool_registry = tools::ToolRegistry::default();
        if let Err(e) = builtins::register_builtins(
            &mut tool_registry,
            tool_settings.read_limits,
            tool_settings.patch_limits,
            tool_settings.search.clone(),
            tool_settings.webfetch.clone(),
            tool_settings.shell.clone(),
            tool_settings.run_policy,
        ) {
            tracing::warn!("Failed to register built-in tools: {e}");
        }
        let tool_registry = std::sync::Arc::new(tool_registry);
        let tool_definitions = tool_registry.definitions();
        let hidden_tools = tool_definitions
            .iter()
            .filter(|d| d.hidden)
            .map(|d| d.name.clone())
            .collect();
        (tool_registry, tool_definitions, hidden_tools)
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

fn parse_config_enum_or_default<T, E, F>(raw: Option<&str>, field: &str, parse: F) -> T
where
    T: Default,
    E: std::fmt::Display,
    F: Fn(&str) -> Result<T, E>,
{
    raw.map(|value| {
        parse(value).unwrap_or_else(|_| {
            tracing::warn!("Unknown {field} in config: {value}");
            T::default()
        })
    })
    .unwrap_or_default()
}

fn parse_run_fallback_mode(mode: Option<config::RunFallbackMode>) -> tools::RunSandboxFallbackMode {
    match mode.unwrap_or_default() {
        config::RunFallbackMode::Prompt => tools::RunSandboxFallbackMode::Prompt,
        config::RunFallbackMode::Deny => tools::RunSandboxFallbackMode::Deny,
        config::RunFallbackMode::AllowWithWarning => {
            tools::RunSandboxFallbackMode::AllowWithWarning
        }
    }
}

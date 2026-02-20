//! Tool Executor Framework - core types, helpers, and built-in tool implementations.

pub mod builtins;
pub mod change_recording;
pub mod command_blacklist;
pub mod config;
pub mod git;
pub mod lp1;
pub mod memory;
pub mod phase_gate;
pub mod powershell_ast;
pub mod process;
pub mod recall;
pub(crate) mod region_hash;
pub mod sandbox;
pub mod search;
pub mod shell;
pub mod webfetch;
pub mod windows_run;
pub mod windows_run_host;

pub use command_blacklist::CommandBlacklist;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use change_recording::ChangeRecorder;
use forge_context::Librarian;
use forge_types::{
    HomoglyphWarning, MixedScriptDetection, Provider, ToolDefinition, detect_mixed_script,
};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use sandbox::Sandbox;
pub use search::SearchToolConfig;
pub use shell::DetectedShell;
pub use webfetch::WebFetchToolConfig;
pub use windows_run::{
    MacOsRunSandboxPolicy, RunSandboxFallbackMode, RunSandboxPolicy, WindowsRunSandboxPolicy,
};

/// Tool execution future type alias.
pub type ToolFut<'a> = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;

/// Risk level for approval prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Approval decision from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    ApproveAll,
    ApproveSelected(Vec<String>),
    DenyAll,
}

/// Approval mode policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Auto-approve most tools, only prompt for high-risk operations.
    Permissive,
    /// Prompt for any side-effecting tool unless allowlisted.
    Default,
    /// Deny all tools unless explicitly allowlisted.
    Strict,
}

/// Policy for tool approval and deny/allow lists.
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: ApprovalMode,
    pub allowlist: HashSet<String>,
    pub denylist: HashSet<String>,
}

impl Policy {
    #[must_use]
    pub fn is_allowlisted(&self, tool: &str) -> bool {
        self.allowlist.contains(tool)
    }

    #[must_use]
    pub fn is_denylisted(&self, tool: &str) -> bool {
        self.denylist.contains(tool)
    }
}

/// Confirmation request for a tool call.
#[derive(Debug, Clone)]
pub struct ConfirmationRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
    /// Optional user-facing reason provided by the model for escalation prompts.
    pub reason: Option<String>,
    pub risk_level: RiskLevel,
    pub arguments: Value,
    /// Homoglyph warnings detected in tool arguments.
    /// Existence of warnings proves analysis was performed and found issues.
    pub warnings: Vec<HomoglyphWarning>,
}

/// Analyze tool arguments for homoglyphs (BOUNDARY per IFA-11).
///
/// Returns proof objects for any detected issues. This analysis happens
/// at the boundary when preparing approval requests.
#[must_use]
pub fn analyze_tool_arguments(tool_name: &str, args: &Value) -> Vec<HomoglyphWarning> {
    let mut warnings = Vec::new();

    // High-risk fields by tool type
    let fields_to_check: &[&str] = match tool_name {
        "WebFetch" | "web_fetch" => &["url"],
        "Run" | "Pwsh" | "run" | "shell" | "bash" | "pwsh" => &["command"],
        "Read" | "Write" | "Edit" | "read" | "write" | "edit" | "patch" => &["path", "file_path"],
        "Git" => &["path", "paths", "branch", "create_branch"],
        _ => &[],
    };

    for field in fields_to_check {
        if let Some(value) = args.get(field) {
            collect_homoglyph_warnings(value, field, &mut warnings);
        }
    }

    warnings
}

/// Recursively scan a JSON value for homoglyphs in all string leaves.
fn collect_homoglyph_warnings(value: &Value, field: &str, warnings: &mut Vec<HomoglyphWarning>) {
    match value {
        Value::String(s) => match detect_mixed_script(s, field) {
            MixedScriptDetection::Suspicious(warning) => warnings.push(warning),
            MixedScriptDetection::Clean => {}
        },
        Value::Array(arr) => {
            for item in arr {
                collect_homoglyph_warnings(item, field, warnings);
            }
        }
        Value::Object(map) => {
            for (key, val) in map {
                collect_homoglyph_warnings(val, key, warnings);
            }
        }
        _ => {}
    }
}

/// Tool events for streaming output.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    Started {
        tool_call_id: String,
        tool_name: String,
    },
    /// A subprocess-backed tool spawned an OS process (best-effort metadata).
    ///
    /// This is currently emitted by the `Run` tool to reduce orphaned-process risk
    /// after crashes. The engine records this in the tool journal for recovery.
    ProcessSpawned {
        tool_call_id: String,
        pid: u32,
        process_started_at_unix_ms: i64,
    },
    StdoutChunk {
        tool_call_id: String,
        chunk: String,
    },
    StderrChunk {
        tool_call_id: String,
        chunk: String,
    },
    Completed {
        tool_call_id: String,
    },
}

/// Error types for tool execution.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Bad tool args: {message}")]
    BadArgs { message: String },
    #[error("Tool timed out: {tool}")]
    Timeout { tool: String, elapsed: Duration },
    #[error("Sandbox violation: {0}")]
    SandboxViolation(DenialReason),
    #[error("Tool execution failed: {tool}: {message}")]
    ExecutionFailed { tool: String, message: String },
    #[error("Unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("Duplicate tool registered: {name}")]
    DuplicateTool { name: String },
    #[error("Duplicate tool call id: {id}")]
    DuplicateToolCallId { id: String },
    #[error("Patch failed for {file:?}: {message}")]
    PatchFailed { file: PathBuf, message: String },
    #[error("Stale file: {file:?}: {reason}")]
    StaleFile { file: PathBuf, reason: String },
}

/// Denial reason for sandbox or policy.
#[derive(Debug, Clone)]
pub enum DenialReason {
    Denylisted {
        tool: String,
    },
    PathOutsideSandbox {
        attempted: PathBuf,
        resolved: PathBuf,
    },
    DeniedPatternMatched {
        attempted: PathBuf,
        pattern: String,
    },
    LimitsExceeded {
        message: String,
    },
    CommandBlacklisted {
        command: String,
        reason: String,
    },
}

impl std::fmt::Display for DenialReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenialReason::Denylisted { tool } => write!(f, "Tool '{tool}' is denylisted"),
            DenialReason::PathOutsideSandbox {
                attempted,
                resolved,
            } => write!(
                f,
                "Path outside sandbox (attempted: {}, resolved: {})",
                attempted.display(),
                resolved.display()
            ),
            DenialReason::DeniedPatternMatched { attempted, pattern } => write!(
                f,
                "Path '{}' matched denied pattern '{}'",
                attempted.display(),
                pattern
            ),
            DenialReason::LimitsExceeded { message } => write!(f, "{message}"),
            DenialReason::CommandBlacklisted { command, reason } => {
                write!(f, "Command blocked: {reason} (command: {command})")
            }
        }
    }
}

/// Proof that a tool executor is safe for dynamic dispatch.
pub trait ToolExecutor: Send + Sync + std::panic::UnwindSafe {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> Value;
    fn is_side_effecting(&self, args: &Value) -> bool;
    /// Whether this tool reads local user data that will be sent to the LLM provider.
    /// In Default approval mode, such tools require approval unless allowlisted.
    fn reads_user_data(&self, _args: &Value) -> bool {
        false
    }
    fn requires_approval(&self) -> bool {
        false
    }
    fn risk_level(&self, args: &Value) -> RiskLevel {
        if self.is_side_effecting(args) {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
    fn approval_summary(&self, args: &Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> {
        None
    }
    /// Hidden tools still execute normally but are invisible to the user.
    fn is_hidden(&self) -> bool {
        false
    }
    /// If set, this tool is only sent to the specified provider.
    /// Returns `None` for tools available to all providers.
    fn target_provider(&self) -> Option<Provider> {
        None
    }
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}

pub(crate) fn parse_args<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
        message: e.to_string(),
    })
}

/// Tool registry for executors and schema-only tools.
///
/// Schema-only tools are visible to the LLM (included in tool definitions)
/// but their execution is intercepted by the engine before reaching an executor.
#[derive(Default)]
pub struct ToolRegistry {
    executors: HashMap<String, Box<dyn ToolExecutor>>,
    schema_only: Vec<ToolDefinition>,
}

impl ToolRegistry {
    pub fn register(&mut self, executor: Box<dyn ToolExecutor>) -> Result<(), ToolError> {
        let name = executor.name().to_string();
        if self.executors.contains_key(&name) {
            return Err(ToolError::DuplicateTool { name });
        }
        self.executors.insert(name, executor);
        Ok(())
    }

    /// Register a schema-only tool definition (no executor).
    ///
    /// The tool appears in the LLM's tool manifest but the engine must
    /// intercept calls before they reach the executor dispatch path.
    pub fn register_schema(&mut self, def: ToolDefinition) -> Result<(), ToolError> {
        let name = &def.name;
        if self.executors.contains_key(name) || self.schema_only.iter().any(|d| d.name == *name) {
            return Err(ToolError::DuplicateTool { name: name.clone() });
        }
        self.schema_only.push(def);
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Result<&dyn ToolExecutor, ToolError> {
        self.executors
            .get(name)
            .map(std::convert::AsRef::as_ref)
            .ok_or_else(|| ToolError::UnknownTool {
                name: name.to_string(),
            })
    }

    /// Whether the given tool name is registered as schema-only (engine-intercepted).
    #[must_use]
    pub fn is_schema_only(&self, name: &str) -> bool {
        self.schema_only.iter().any(|d| d.name == name)
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .executors
            .values()
            .map(|exec| {
                let mut def = ToolDefinition::new(exec.name(), exec.description(), exec.schema());
                def.hidden = exec.is_hidden();
                def.provider = exec.target_provider();
                def
            })
            .chain(self.schema_only.iter().cloned())
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }
}

/// Proof object: grants permission to edit lines within [start_line, end_line].
///
/// IFA §10 (Capability Tokens): This is a capability token for editing a region.
/// An edit to line N is only permitted if start_line <= N <= end_line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRegion {
    /// First line of observed region (1-indexed, inclusive)
    pub start_line: u32,
    /// Last line of observed region (1-indexed, inclusive)
    pub end_line: u32,
    /// Hash of lines 1..start_line-1 (all zeros if start_line == 1).
    /// Detects insertions/deletions above the region that would shift line numbers.
    pub prefix_hash: [u8; 32],
    /// Hash of lines start_line..=end_line.
    /// Detects modifications within the observed region.
    pub region_hash: [u8; 32],
}

impl ObservedRegion {
    pub const EMPTY_HASH: [u8; 32] = [0u8; 32];
}

/// File cache entry for stale file detection using surgical region hashing.
#[derive(Debug, Clone)]
pub struct FileCacheEntry {
    /// Observed region covering all reads. Merged on each read.
    pub observed: ObservedRegion,
}

pub type ToolFileCache = HashMap<PathBuf, FileCacheEntry>;

/// Normalize a path for use as a cache key.
///
/// On Windows, paths are case-insensitive but HashMap keys are case-sensitive.
/// This normalizes to lowercase to prevent cache misses due to casing differences
/// in canonicalized paths (e.g., `C:\Users\Danie` vs `C:\Users\danie`).
#[cfg(windows)]
pub(crate) fn normalize_cache_key(path: &std::path::Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
pub(crate) fn normalize_cache_key(path: &std::path::Path) -> PathBuf {
    path.to_path_buf()
}

/// Record a file read in the tool file cache.
///
/// Creates or merges an `ObservedRegion` covering lines `1..=line_count`,
/// enabling stale-edit protection for files read via `@path` expansion.
pub fn record_file_read(
    cache: &mut ToolFileCache,
    path: &std::path::Path,
    line_count: u32,
) -> std::io::Result<()> {
    let key = normalize_cache_key(path);
    if let Some(entry) = cache.get(&key) {
        if let Ok(merged) = region_hash::merge_regions(path, &entry.observed, 1, line_count) {
            cache.insert(key, FileCacheEntry { observed: merged });
        }
    } else if let Ok(region) = region_hash::create_region(path, 1, line_count) {
        cache.insert(key, FileCacheEntry { observed: region });
    }
    Ok(())
}

/// Per-call tool context.
#[derive(Debug)]
pub struct ToolCtx {
    pub sandbox: Sandbox,
    pub output_tx: mpsc::Sender<ToolEvent>,
    pub default_timeout: Duration,
    pub max_output_bytes: usize,
    pub available_capacity_bytes: usize,
    pub tool_call_id: String,
    pub allow_truncation: bool,
    pub working_dir: PathBuf,
    pub env_sanitizer: EnvSanitizer,
    pub file_cache: Arc<Mutex<ToolFileCache>>,
    pub turn_changes: ChangeRecorder,
    /// The Librarian for fact recall (Context Infinity).
    pub librarian: Option<Arc<Mutex<Librarian>>>,
    /// Command blacklist for blocking catastrophic commands.
    pub command_blacklist: CommandBlacklist,
}

/// Per-batch limits for tool execution.
#[derive(Debug, Clone, Copy)]
pub struct ToolLimits {
    pub max_tool_calls_per_batch: usize,
    pub max_tool_iterations_per_user_turn: u32,
    pub max_tool_args_bytes: usize,
    pub max_batch_wall_time: Duration,
}

/// Tool-specific limits for `read_file`.
#[derive(Debug, Clone, Copy)]
pub struct ReadFileLimits {
    pub max_file_read_bytes: usize,
    pub max_scan_bytes: usize,
}

/// Tool-specific limits for `apply_patch`.
#[derive(Debug, Clone, Copy)]
pub struct PatchLimits {
    pub max_patch_bytes: usize,
}

/// Tool-specific timeout configuration.
#[derive(Debug, Clone, Copy)]
pub struct ToolTimeouts {
    pub default_timeout: Duration,
    pub file_operations_timeout: Duration,
    pub shell_commands_timeout: Duration,
}

/// Aggregated tool settings derived from config.
#[derive(Debug, Clone)]
pub struct ToolSettings {
    pub limits: ToolLimits,
    pub read_limits: ReadFileLimits,
    pub patch_limits: PatchLimits,
    pub search: SearchToolConfig,
    pub webfetch: WebFetchToolConfig,
    pub shell: DetectedShell,
    pub timeouts: ToolTimeouts,
    pub max_output_bytes: usize,
    pub policy: Policy,
    pub sandbox: Sandbox,
    pub env_sanitizer: EnvSanitizer,
    pub command_blacklist: CommandBlacklist,
    pub run_policy: RunSandboxPolicy,
}

/// Sanitizes environment variables before executing commands.
#[derive(Debug, Clone)]
pub struct EnvSanitizer {
    denylist: globset::GlobSet,
}

impl EnvSanitizer {
    pub fn new(patterns: &[String]) -> Result<Self, ToolError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pat in patterns {
            let mut glob = globset::GlobBuilder::new(pat);
            // Always use case-insensitive matching for env var patterns.
            // This ensures security patterns like *_KEY, *_SECRET, *_TOKEN
            // match regardless of case (API_KEY, api_key, Api_Key, etc.)
            glob.case_insensitive(true);
            let glob = glob.build().map_err(|e| ToolError::BadArgs {
                message: format!("Invalid env denylist pattern '{pat}': {e}"),
            })?;
            builder.add(glob);
        }
        let set = builder.build().map_err(|e| ToolError::BadArgs {
            message: format!("Invalid env denylist: {e}"),
        })?;
        Ok(Self { denylist: set })
    }

    #[must_use]
    pub fn sanitize_env(&self, env: &[(String, String)]) -> Vec<(String, String)> {
        env.iter()
            .filter(|(k, _)| !self.denylist.is_match(k))
            .cloned()
            .collect()
    }
}

/// Validate arguments against a JSON schema.
pub fn validate_args(schema: &Value, args: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::validator_for(schema).map_err(|e| ToolError::BadArgs {
        message: format!("Invalid tool schema: {e}"),
    })?;
    let result = validator.validate(args);
    if let Err(err) = result {
        return Err(ToolError::BadArgs {
            message: err.to_string(),
        });
    }
    Ok(())
}

/// Truncate tool output to the effective maximum length.
#[must_use]
pub fn truncate_output(output: String, effective_max: usize) -> String {
    if output.len() <= effective_max {
        return output;
    }
    let marker = "\n\n... [output truncated]";
    if effective_max <= marker.len() {
        return marker[..effective_max].to_string();
    }
    let max_body = effective_max - marker.len();
    let mut end = max_body;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = output;
    truncated.truncate(end);
    truncated.push_str(marker);
    truncated
}

/// Sanitize tool output for terminal display and context inclusion.
///
/// Tool output is untrusted external content that enters the LLM context
/// window, so we apply:
/// - terminal escape stripping
/// - steganographic character stripping
/// - secret redaction (pattern + env-derived)
#[must_use]
pub fn sanitize_output(output: &str) -> String {
    forge_utils::sanitize_display_text(output)
}

/// Redact obvious secrets in output distillates (best-effort).
#[must_use]
pub fn redact_distillate(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == 's' {
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('k') && lookahead.next() == Some('-') {
                chars.next();
                chars.next();
                output.push_str("sk-*******");
                while let Some(&next_ch) = chars.peek() {
                    if next_ch.is_whitespace() {
                        break;
                    }
                    chars.next();
                }
                continue;
            }
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{EnvSanitizer, analyze_tool_arguments, sanitize_output};
    use serde_json::json;

    #[test]
    fn analyze_tool_arguments_detects_webfetch_url() {
        // Cyrillic 'а' (U+0430) looks like Latin 'a'
        let args = json!({"url": "https://pаypal.com"});
        let warnings = analyze_tool_arguments("WebFetch", &args);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].field_name, "url");
    }

    #[test]
    fn analyze_tool_arguments_detects_run_command() {
        // Cyrillic 'е' (U+0435) looks like Latin 'e'
        let args = json!({"command": "wgеt evil.com"});
        let warnings = analyze_tool_arguments("Run", &args);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].field_name, "command");
    }

    #[test]
    fn analyze_tool_arguments_detects_shell_command() {
        let args = json!({"command": "curl gооgle.com"});
        let warnings = analyze_tool_arguments("shell", &args);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn analyze_tool_arguments_detects_edit_path() {
        // Path with Cyrillic character
        let args = json!({"file_path": "/tmp/tеst.py"});
        let warnings = analyze_tool_arguments("Edit", &args);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].field_name, "file_path");
    }

    #[test]
    fn analyze_tool_arguments_clean_webfetch_url() {
        let args = json!({"url": "https://google.com"});
        let warnings = analyze_tool_arguments("WebFetch", &args);
        assert!(warnings.is_empty());
    }

    #[test]
    fn analyze_tool_arguments_ignores_untracked_tools() {
        let args = json!({"url": "https://pаypal.com"});
        let warnings = analyze_tool_arguments("UnknownTool", &args);
        assert!(warnings.is_empty());
    }

    #[test]
    fn analyze_tool_arguments_handles_missing_field() {
        let args = json!({"other_field": "value"});
        let warnings = analyze_tool_arguments("WebFetch", &args);
        assert!(warnings.is_empty());
    }

    #[test]
    fn analyze_tool_arguments_handles_non_string_field() {
        let args = json!({"url": 123});
        let warnings = analyze_tool_arguments("WebFetch", &args);
        assert!(warnings.is_empty());
    }

    #[test]
    fn sanitize_output_strips_steganographic_chars() {
        let input = "Hello\u{200B}World";
        assert_eq!(sanitize_output(input), "HelloWorld");
    }

    #[test]
    fn sanitize_output_redacts_openai_keys() {
        let input = "key=sk-proj-abc123def456ghi789jkl";
        let output = sanitize_output(input);
        assert!(output.contains("sk-***"));
        assert!(!output.contains("abc123def456ghi789jkl"));
    }

    #[test]
    fn env_sanitizer_strips_dyld_and_ld_vars() {
        use forge_types::ENV_SECRET_DENYLIST;

        let sanitizer = EnvSanitizer::new(
            &ENV_SECRET_DENYLIST
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            (
                "DYLD_INSERT_LIBRARIES".to_string(),
                "/evil.dylib".to_string(),
            ),
            ("DYLD_LIBRARY_PATH".to_string(), "/evil".to_string()),
            ("LD_PRELOAD".to_string(), "/evil.so".to_string()),
            ("LD_LIBRARY_PATH".to_string(), "/evil".to_string()),
            ("HOME".to_string(), "/Users/test".to_string()),
        ];
        let clean = sanitizer.sanitize_env(&env);
        let keys: Vec<&str> = clean.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"PATH"));
        assert!(keys.contains(&"HOME"));
        assert!(!keys.contains(&"DYLD_INSERT_LIBRARIES"));
        assert!(!keys.contains(&"DYLD_LIBRARY_PATH"));
        assert!(!keys.contains(&"LD_PRELOAD"));
        assert!(!keys.contains(&"LD_LIBRARY_PATH"));
    }
}

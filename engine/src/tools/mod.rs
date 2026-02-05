//! Tool Executor Framework - core types and helpers.

pub mod builtins;
pub mod command_blacklist;
pub mod git;
pub mod lp1;
pub mod recall;
pub mod sandbox;
pub mod search;
pub mod shell;
pub mod webfetch;

pub use command_blacklist::CommandBlacklist;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use forge_context::Librarian;
use forge_types::{
    HomoglyphWarning, ToolDefinition, ToolResult, detect_mixed_script, sanitize_terminal_text,
    strip_steganographic_chars,
};
use jsonschema::JSONSchema;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use crate::input_modes::ChangeRecorder;
use sandbox::Sandbox;
pub use search::SearchToolConfig;
pub use shell::DetectedShell;
pub use webfetch::WebFetchToolConfig;

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
    pub fn is_allowlisted(&self, tool: &str) -> bool {
        self.allowlist.contains(tool)
    }

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
        _ => &[],
    };

    for field in fields_to_check {
        if let Some(value) = args.get(field).and_then(|v| v.as_str())
            && let Some(warning) = detect_mixed_script(value, field)
        {
            warnings.push(warning);
        }
    }

    warnings
}

/// Planned disposition for a tool call.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PlannedDisposition {
    ExecuteNow,
    RequiresConfirmation(ConfirmationRequest),
    PreResolved(ToolResult),
}

/// Tool events for streaming output.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    Started {
        tool_call_id: String,
        tool_name: String,
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
    fn is_side_effecting(&self) -> bool;
    fn requires_approval(&self) -> bool {
        false
    }
    fn risk_level(&self) -> RiskLevel {
        if self.is_side_effecting() {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
    fn approval_summary(&self, args: &Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> {
        None
    }
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}

/// Tool registry for executors.
#[derive(Default)]
pub struct ToolRegistry {
    executors: HashMap<String, Box<dyn ToolExecutor>>,
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

    pub fn lookup(&self, name: &str) -> Result<&dyn ToolExecutor, ToolError> {
        self.executors
            .get(name)
            .map(std::convert::AsRef::as_ref)
            .ok_or_else(|| ToolError::UnknownTool {
                name: name.to_string(),
            })
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .executors
            .values()
            .map(|exec| ToolDefinition::new(exec.name(), exec.description(), exec.schema()))
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}

/// File SHA cache entry for stale file detection.
#[derive(Debug, Clone)]
pub struct FileCacheEntry {
    pub sha256: [u8; 32],
    #[allow(dead_code)]
    pub read_at: SystemTime,
}

pub type ToolFileCache = HashMap<PathBuf, FileCacheEntry>;

/// Per-call tool context.
#[derive(Debug)]
pub struct ToolCtx {
    pub sandbox: Sandbox,
    #[allow(dead_code)]
    pub abort: futures_util::future::AbortHandle,
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

/// Shared batch-level tool context.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SharedToolCtx {
    pub sandbox: Sandbox,
    pub output_tx: mpsc::Sender<ToolEvent>,
    pub default_timeout: Duration,
    pub max_output_bytes: usize,
    pub initial_capacity_bytes: usize,
    pub env_sanitizer: EnvSanitizer,
    pub file_cache: Arc<tokio::sync::Mutex<ToolFileCache>>,
}

/// Per-batch limits for tool execution.
#[derive(Debug, Clone, Copy)]
pub struct ToolLimits {
    pub max_tool_calls_per_batch: usize,
    pub max_tool_iterations_per_user_turn: u32,
    pub max_tool_args_bytes: usize,
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

    pub fn sanitize_env(&self, env: &[(String, String)]) -> Vec<(String, String)> {
        env.iter()
            .filter(|(k, _)| !self.denylist.is_match(k))
            .cloned()
            .collect()
    }
}

/// Validate arguments against a JSON schema.
pub fn validate_args(schema: &Value, args: &Value) -> Result<(), ToolError> {
    let compiled = JSONSchema::compile(schema).map_err(|e| ToolError::BadArgs {
        message: format!("Invalid tool schema: {e}"),
    })?;
    if let Err(errors) = compiled.validate(args) {
        let msg = errors
            .map(|err| err.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ToolError::BadArgs { message: msg });
    }
    Ok(())
}

/// Truncate tool output to the effective maximum length.
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

/// Sanitize tool output for terminal display and steganographic injection.
///
/// Applies both terminal escape stripping and steganographic character
/// removal. Tool output is untrusted content that enters the LLM context
/// window, so both sanitization passes are required.
pub fn sanitize_output(output: &str) -> String {
    let terminal_safe = sanitize_terminal_text(output);
    strip_steganographic_chars(&terminal_safe).into_owned()
}

/// Redact obvious secrets in output distillates (best-effort).
pub fn redact_distillate(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == 's' {
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('k') && lookahead.next() == Some('-') {
                chars.next();
                chars.next();
                output.push_str("sk-***");
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
    use super::*;
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
}

//! Git tool executors.
//!
//! Note: JSON schema literals use numbers like 5000000 which clippy warns about,
//! but JSON doesn't support numeric separators, so we allow this lint.
#![allow(clippy::unreadable_literal)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time;

use super::{
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, parse_args, redact_distillate,
    sanitize_output,
};

const DEFAULT_GIT_TIMEOUT_MS: u64 = 30_000;
const MAX_GIT_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_GIT_STDOUT_BYTES: usize = 200_000;
const DEFAULT_GIT_STDERR_BYTES: usize = 100_000;
const MAX_OUTPUT_BYTES: usize = 5_000_000;

use crate::config::default_true;

#[derive(Debug, Clone, Copy)]
enum GitToolKind {
    Status,
    Diff,
    Restore,
    Add,
    Commit,
    Log,
    Branch,
    Checkout,
    Stash,
    Show,
    Blame,
}

impl GitToolKind {
    fn from_command_str(s: &str) -> Option<Self> {
        match s {
            "status" => Some(Self::Status),
            "diff" => Some(Self::Diff),
            "restore" => Some(Self::Restore),
            "add" => Some(Self::Add),
            "commit" => Some(Self::Commit),
            "log" => Some(Self::Log),
            "branch" => Some(Self::Branch),
            "checkout" => Some(Self::Checkout),
            "stash" => Some(Self::Stash),
            "show" => Some(Self::Show),
            "blame" => Some(Self::Blame),
            _ => None,
        }
    }

    fn command_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Diff => "diff",
            Self::Restore => "restore",
            Self::Add => "add",
            Self::Commit => "commit",
            Self::Log => "log",
            Self::Branch => "branch",
            Self::Checkout => "checkout",
            Self::Stash => "stash",
            Self::Show => "show",
            Self::Blame => "blame",
        }
    }

    fn is_side_effecting(self) -> bool {
        matches!(
            self,
            GitToolKind::Restore
                | GitToolKind::Add
                | GitToolKind::Commit
                | GitToolKind::Branch
                | GitToolKind::Checkout
                | GitToolKind::Stash
        )
    }

    fn risk_level(self) -> RiskLevel {
        match self {
            GitToolKind::Restore => RiskLevel::High,
            GitToolKind::Add
            | GitToolKind::Commit
            | GitToolKind::Branch
            | GitToolKind::Checkout
            | GitToolKind::Stash => RiskLevel::Medium,
            GitToolKind::Status
            | GitToolKind::Diff
            | GitToolKind::Log
            | GitToolKind::Show
            | GitToolKind::Blame => RiskLevel::Low,
        }
    }
}

struct GitTool;

impl GitTool {
    fn parse_kind(args: &Value) -> Result<GitToolKind, ToolError> {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs {
                message: "missing required field: command".to_string(),
            })?;
        GitToolKind::from_command_str(cmd).ok_or_else(|| ToolError::BadArgs {
            message: format!("unknown git command: {cmd}"),
        })
    }
}

fn git_tool_schema() -> Value {
    let mut variants = Vec::with_capacity(11);

    variants.push(json!({
        "title": "status",
        "description": "Show working tree status: staged, modified, and untracked files.",
        "properties": {
            "command": { "type": "string", "const": "status" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "porcelain": { "type": "boolean", "default": true, "description": "Use porcelain output (--porcelain=1)" },
            "branch": { "type": "boolean", "default": true, "description": "Include branch info (-b) in porcelain mode" },
            "untracked": { "type": "boolean", "default": true, "description": "Include untracked files (false uses -uno)" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "diff",
        "description": "Show file changes in working tree, staging area, or between refs. When from_ref+to_ref+output_dir are set, writes per-file patches.",
        "properties": {
            "command": { "type": "string", "const": "diff" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "cached": { "type": "boolean", "default": false, "description": "Diff staged changes (--cached)" },
            "stat": { "type": "boolean", "default": false, "description": "Show diffstat only (--stat)" },
            "name_only": { "type": "boolean", "default": false, "description": "Show only changed file names" },
            "unified": { "type": "integer", "minimum": 0, "description": "Context lines (-U<N>)" },
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths to diff (after --)" },
            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Max stdout bytes" },
            "from_ref": { "type": "string", "description": "Starting ref for ref-to-ref comparison" },
            "to_ref": { "type": "string", "description": "Ending ref for ref-to-ref comparison" },
            "output_dir": { "type": "string", "description": "Directory to write per-file patches (requires from_ref+to_ref)" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "restore",
        "description": "Discard uncommitted changes to specific files. WARNING: destructive.",
        "properties": {
            "command": { "type": "string", "const": "restore" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths to restore (after --)" },
            "staged": { "type": "boolean", "default": false, "description": "Restore the index/staging area (--staged)" },
            "worktree": { "type": "boolean", "default": true, "description": "Restore the working tree (--worktree)" }
        },
        "required": ["command", "paths"]
    }));

    variants.push(json!({
        "title": "add",
        "description": "Stage files for commit.",
        "properties": {
            "command": { "type": "string", "const": "add" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Files to stage" },
            "all": { "type": "boolean", "default": false, "description": "Stage all changes (-A)" },
            "update": { "type": "boolean", "default": false, "description": "Stage modified/deleted only (-u)" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "commit",
        "description": "Create a conventional commit (type(scope): message).",
        "properties": {
            "command": { "type": "string", "const": "commit" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "type": { "type": "string", "description": "Commit type: feat, fix, docs, style, refactor, test, chore" },
            "scope": { "type": "string", "description": "Optional scope/area of change" },
            "message": { "type": "string", "description": "Commit description" }
        },
        "required": ["command", "type", "message"]
    }));

    variants.push(json!({
        "title": "log",
        "description": "Show commit history with configurable format and filters.",
        "properties": {
            "command": { "type": "string", "const": "log" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "max_count": { "type": "integer", "minimum": 1, "description": "Limit number of commits" },
            "oneline": { "type": "boolean", "default": false, "description": "One line per commit" },
            "format": { "type": "string", "description": "Pretty-print format (e.g. '%H %s')" },
            "author": { "type": "string", "description": "Filter by author" },
            "since": { "type": "string", "description": "After date (e.g. '2024-01-01')" },
            "until": { "type": "string", "description": "Before date" },
            "grep": { "type": "string", "description": "Filter by message pattern" },
            "path": { "type": "string", "description": "Show commits affecting this path" },
            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Max output bytes" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "branch",
        "description": "List, create, rename, or delete branches.",
        "properties": {
            "command": { "type": "string", "const": "branch" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "list_all": { "type": "boolean", "default": false, "description": "List local and remote branches (-a)" },
            "list_remote": { "type": "boolean", "default": false, "description": "List only remote branches (-r)" },
            "create": { "type": "string", "description": "Create a new branch with this name" },
            "delete": { "type": "string", "description": "Delete this branch (-d, must be merged)" },
            "force_delete": { "type": "string", "description": "Force delete this branch (-D)" },
            "rename": { "type": "string", "description": "Rename this branch (requires new_name)" },
            "new_name": { "type": "string", "description": "New name when renaming" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "checkout",
        "description": "Switch branches or restore working tree files.",
        "properties": {
            "command": { "type": "string", "const": "checkout" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "branch": { "type": "string", "description": "Branch to switch to" },
            "create_branch": { "type": "string", "description": "Create and switch to new branch (-b)" },
            "commit": { "type": "string", "description": "Checkout a specific commit (detached HEAD)" },
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Restore these paths from HEAD" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "stash",
        "description": "Stash changes in a dirty working directory.",
        "properties": {
            "command": { "type": "string", "const": "stash" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "action": { "type": "string", "enum": ["push", "pop", "apply", "drop", "list", "show", "clear"], "default": "push", "description": "Stash action" },
            "message": { "type": "string", "description": "Message for stash (with push)" },
            "index": { "type": "integer", "minimum": 0, "description": "Stash index for pop/apply/drop/show" },
            "include_untracked": { "type": "boolean", "default": false, "description": "Include untracked files (with push)" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "show",
        "description": "Show commit details and diff.",
        "properties": {
            "command": { "type": "string", "const": "show" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "commit": { "type": "string", "description": "Commit to show (default: HEAD)" },
            "stat": { "type": "boolean", "default": false, "description": "Show diffstat only" },
            "name_only": { "type": "boolean", "default": false, "description": "Show only changed file names" },
            "format": { "type": "string", "description": "Pretty-print format" },
            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Max output bytes" }
        },
        "required": ["command"]
    }));

    variants.push(json!({
        "title": "blame",
        "description": "Show what revision and author last modified each line of a file.",
        "properties": {
            "command": { "type": "string", "const": "blame" },
            "timeout_ms": { "type": "integer", "description": "Timeout in ms (default 30000)", "minimum": 100 },
            "path": { "type": "string", "description": "File path to blame" },
            "start_line": { "type": "integer", "minimum": 1, "description": "Start line for range" },
            "end_line": { "type": "integer", "minimum": 1, "description": "End line for range" },
            "commit": { "type": "string", "description": "Blame at specific commit instead of HEAD" },
            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Max output bytes" }
        },
        "required": ["command", "path"]
    }));

    json!({
        "type": "object",
        "required": ["command"],
        "oneOf": variants
    })
}

impl ToolExecutor for GitTool {
    fn name(&self) -> &'static str {
        "Git"
    }

    fn description(&self) -> &'static str {
        "Git version control. Use `command` to select: status, diff, restore, add, commit, \
         log, branch, checkout, stash, show, blame."
    }

    fn schema(&self) -> Value {
        git_tool_schema()
    }

    fn is_side_effecting(&self, args: &Value) -> bool {
        match Self::parse_kind(args) {
            Ok(GitToolKind::Diff) => {
                // Diff is side-effecting when output_dir is specified (writes patch files)
                args.get("output_dir")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            }
            Ok(kind) => kind.is_side_effecting(),
            Err(_) => true,
        }
    }

    fn reads_user_data(&self, args: &Value) -> bool {
        match Self::parse_kind(args) {
            Ok(kind) => !kind.is_side_effecting(),
            Err(_) => true,
        }
    }

    fn risk_level(&self, args: &Value) -> RiskLevel {
        match Self::parse_kind(args) {
            Ok(GitToolKind::Diff)
                if args
                    .get("output_dir")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty()) =>
            {
                RiskLevel::Medium
            }
            Ok(kind) => kind.risk_level(),
            Err(_) => RiskLevel::High,
        }
    }

    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let kind = Self::parse_kind(args)?;
        let distillate = match kind {
            GitToolKind::Status => "Git status".to_string(),
            GitToolKind::Diff => {
                let typed: GitDiffArgs = parse_args(args)?;
                match (typed.from_ref, typed.to_ref) {
                    (Some(from_ref), Some(to_ref)) => format!("Git diff {from_ref}..{to_ref}"),
                    (Some(from_ref), None) => format!("Git diff {from_ref}"),
                    _ => "Git diff".to_string(),
                }
            }
            GitToolKind::Restore => {
                let typed: GitRestoreArgs = parse_args(args)?;
                format!("Git restore {} file(s)", typed.paths.len())
            }
            GitToolKind::Add => {
                let typed: GitAddArgs = parse_args(args)?;
                if typed.all {
                    "Git add -A".to_string()
                } else if typed.update {
                    "Git add -u".to_string()
                } else {
                    format!("Git add {} file(s)", typed.paths.unwrap_or_default().len())
                }
            }
            GitToolKind::Commit => {
                let typed: GitCommitArgs = parse_args(args)?;
                format!("Git commit {}", typed.message)
            }
            GitToolKind::Log => "Git log".to_string(),
            GitToolKind::Branch => "Git branch".to_string(),
            GitToolKind::Checkout => "Git checkout".to_string(),
            GitToolKind::Stash => {
                let typed: GitStashArgs = parse_args(args)?;
                format!(
                    "Git stash {}",
                    typed.action.unwrap_or_else(|| "push".to_string())
                )
            }
            GitToolKind::Show => "Git show".to_string(),
            GitToolKind::Blame => {
                let typed: GitBlameArgs = parse_args(args)?;
                format!("Git blame {}", typed.path)
            }
        };

        Ok(redact_distillate(&distillate))
    }

    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(MAX_GIT_TIMEOUT_MS))
    }

    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let kind = Self::parse_kind(&args)?;

            // Disable generic truncation - we handle it ourselves to preserve JSON validity
            ctx.allow_truncation = false;

            let mut payload = match kind {
                GitToolKind::Status => handle_git_status(ctx, args).await?,
                GitToolKind::Diff => handle_git_diff(ctx, args).await?,
                GitToolKind::Restore => handle_git_restore(ctx, args).await?,
                GitToolKind::Add => handle_git_add(ctx, args).await?,
                GitToolKind::Commit => handle_git_commit(ctx, args).await?,
                GitToolKind::Log => handle_git_log(ctx, args).await?,
                GitToolKind::Branch => handle_git_branch(ctx, args).await?,
                GitToolKind::Checkout => handle_git_checkout(ctx, args).await?,
                GitToolKind::Stash => handle_git_stash(ctx, args).await?,
                GitToolKind::Show => handle_git_show(ctx, args).await?,
                GitToolKind::Blame => handle_git_blame(ctx, args).await?,
            };

            // Ensure JSON output fits within capacity by shrinking large fields
            let max_bytes = effective_max_bytes(ctx);
            truncate_json_payload(&mut payload, max_bytes);

            let json = serde_json::to_string(&payload).map_err(|e| ToolError::ExecutionFailed {
                tool: format!("Git:{}", kind.command_str()),
                message: e.to_string(),
            })?;

            Ok(sanitize_output(&json))
        })
    }
}

/// Truncate large string fields in a JSON payload to fit within a byte budget.
///
/// This preserves JSON validity by shrinking `stdout`, `stderr`, and `text` fields
/// (inside `content` array) rather than cutting the JSON string mid-token.
fn truncate_json_payload(payload: &mut Value, max_bytes: usize) {
    const TRUNCATION_MARKER: &str = "\n... [truncated for size]";

    // First, check if we're already within budget
    if let Ok(json) = serde_json::to_string(payload)
        && json.len() <= max_bytes
    {
        return;
    }

    // Iteratively shrink large fields until we fit
    // Priority: stdout (largest), then stderr, then content text
    let fields_to_shrink = ["stdout", "stderr"];

    for _ in 0..10 {
        // Max 10 iterations to prevent infinite loop
        let current_size = serde_json::to_string(payload).map(|s| s.len()).unwrap_or(0);

        if current_size <= max_bytes {
            return;
        }

        let excess = current_size.saturating_sub(max_bytes);

        // Find the largest shrinkable field and reduce it
        let obj = match payload.as_object_mut() {
            Some(obj) => obj,
            None => return,
        };

        let mut best_field: Option<(&str, usize)> = None;
        for field in fields_to_shrink {
            if let Some(Value::String(s)) = obj.get(field) {
                let len = s.len();
                if len > TRUNCATION_MARKER.len() + 100 {
                    // Only consider if meaningful content
                    if best_field.is_none() || len > best_field.unwrap().1 {
                        best_field = Some((field, len));
                    }
                }
            }
        }

        match best_field {
            Some((field, current_len)) => {
                // Shrink by at least the excess, but leave some meaningful content
                let reduction = excess.max(current_len / 4);
                let new_len = current_len
                    .saturating_sub(reduction)
                    .max(TRUNCATION_MARKER.len() + 100);

                if let Some(Value::String(s)) = obj.get_mut(field) {
                    // Truncate at a safe UTF-8 boundary
                    let truncate_at = s
                        .char_indices()
                        .take_while(|(i, _)| *i < new_len.saturating_sub(TRUNCATION_MARKER.len()))
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(0);

                    s.truncate(truncate_at);
                    s.push_str(TRUNCATION_MARKER);
                }
            }
            None => {
                // No more fields to shrink, we've done our best
                return;
            }
        }
    }
}

pub fn register_git_tool(registry: &mut super::ToolRegistry) -> Result<(), ToolError> {
    registry.register(Box::new(GitTool))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct GitExecResult {
    git_bin: String,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    exit_code: Option<i32>,
    success: bool,
    stdout: String,
    stderr: String,
    truncated_stdout: bool,
    truncated_stderr: bool,
    timed_out: bool,
}

fn build_git_response(
    exec: &GitExecResult,
    text: String,
    extra_fields: Option<HashMap<&str, Value>>,
) -> Value {
    let mut payload = json!({
        "content": [{"type": "text", "text": text}],
        "isError": !exec.success,
        "git_bin": exec.git_bin,
        "args": exec.args,
        "working_dir": exec.working_dir.as_ref().map(|p| p.display().to_string()),
        "exit_code": exec.exit_code,
        "timed_out": exec.timed_out,
        "truncated_stdout": exec.truncated_stdout,
        "truncated_stderr": exec.truncated_stderr,
        "stdout": exec.stdout,
        "stderr": exec.stderr,
    });

    if let Some(extra) = extra_fields
        && let Some(obj) = payload.as_object_mut()
    {
        for (key, value) in extra {
            obj.insert(key.to_string(), value);
        }
    }

    payload
}

fn clamp_bytes(requested: Option<usize>, default: usize, max: usize) -> usize {
    let value = requested.unwrap_or(default);
    value.clamp(1, max)
}

fn effective_max_bytes(ctx: &ToolCtx) -> usize {
    ctx.max_output_bytes
        .min(ctx.available_capacity_bytes)
        .max(1)
}

async fn run_git(
    ctx: &ToolCtx,
    working_dir: &Path,
    subcommand_args: Vec<String>,
    timeout_ms: u64,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<GitExecResult, ToolError> {
    let timeout_ms = timeout_ms.clamp(100, MAX_GIT_TIMEOUT_MS);
    let max_stdout_bytes = max_stdout_bytes.clamp(1, MAX_OUTPUT_BYTES);
    let max_stderr_bytes = max_stderr_bytes.clamp(1, MAX_OUTPUT_BYTES);

    let git_bin = if cfg!(windows) { "git.exe" } else { "git" }.to_string();

    let mut args: Vec<String> = vec!["--no-pager".into(), "-c".into(), "color.ui=false".into()];
    args.extend(subcommand_args);

    let mut cmd = Command::new(&git_bin);
    cmd.args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(working_dir);

    let env: Vec<(String, String)> = std::env::vars().collect();
    let sanitized = ctx.env_sanitizer.sanitize_env(&env);
    cmd.env_clear();
    cmd.envs(sanitized);

    #[cfg(unix)]
    super::process::set_new_session(&mut cmd);

    let child = cmd.spawn().map_err(|e| ToolError::ExecutionFailed {
        tool: "git".to_string(),
        message: format!("failed to spawn git: {e}"),
    })?;
    let mut guard = super::process::ChildGuard::new(child);

    let stdout = guard
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: "failed to capture git stdout".to_string(),
        })?;
    let stderr = guard
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: "failed to capture git stderr".to_string(),
        })?;

    let stdout_task = tokio::spawn(read_to_end_limited(stdout, max_stdout_bytes));
    let stderr_task = tokio::spawn(read_to_end_limited(stderr, max_stderr_bytes));

    let mut timed_out = false;
    let status = if let Ok(res) =
        time::timeout(Duration::from_millis(timeout_ms), guard.child_mut().wait()).await
    {
        res.map_err(|e| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: e.to_string(),
        })?
    } else {
        timed_out = true;
        let _ = guard.child_mut().kill().await;
        match time::timeout(Duration::from_millis(2_000), guard.child_mut().wait()).await {
            Ok(res) => res.map_err(|e| ToolError::ExecutionFailed {
                tool: "git".to_string(),
                message: e.to_string(),
            })?,
            Err(_) => {
                return Err(ToolError::ExecutionFailed {
                    tool: "git".to_string(),
                    message: format!(
                        "git command timed out after {timeout_ms} ms and did not terminate"
                    ),
                });
            }
        }
    };

    guard.disarm();
    let exit_code = status.code();

    let (stdout_bytes, truncated_stdout) =
        stdout_task.await.unwrap_or_else(|_| (Vec::new(), false));
    let (stderr_bytes, truncated_stderr) =
        stderr_task.await.unwrap_or_else(|_| (Vec::new(), false));

    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    Ok(GitExecResult {
        git_bin,
        args,
        working_dir: Some(working_dir.to_path_buf()),
        exit_code,
        success: status.success() && !timed_out,
        stdout,
        stderr,
        truncated_stdout,
        truncated_stderr,
        timed_out,
    })
}

async fn read_to_end_limited<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    max_bytes: usize,
) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let mut truncated = false;

    loop {
        let n = match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        let take = remaining.min(n);
        buf.extend_from_slice(&tmp[..take]);
        if take < n {
            truncated = true;
            break;
        }
    }

    (buf, truncated)
}

fn trim_output(output: &str) -> String {
    output.trim_end_matches(&['\r', '\n'][..]).to_string()
}

// ===== Git diff patch Distillate helpers =====

fn sanitize_path_for_filename(path: &str) -> String {
    path.replace(['/', '\\'], "__")
}

#[derive(Serialize)]
struct FileDiffEntry {
    path: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    insertions: u32,
    deletions: u32,
    patch_file: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    binary: bool,
}

#[derive(Serialize)]
struct DiffSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    from_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_ref: Option<String>,
    generated_at: String,
    files: Vec<FileDiffEntry>,
    stats: DiffStats,
}

#[derive(Serialize)]
struct DiffStats {
    files_changed: usize,
    insertions: u32,
    deletions: u32,
}

fn parse_numstat_line(line: &str) -> Option<(u32, u32, String, bool)> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return None;
    }
    let path = parts[2..].join("\t");
    if parts[0] == "-" && parts[1] == "-" {
        Some((0, 0, path, true))
    } else {
        let ins = parts[0].parse().ok()?;
        let del = parts[1].parse().ok()?;
        Some((ins, del, path, false))
    }
}

async fn write_patches_to_dir(
    ctx: &ToolCtx,
    working_dir: &Path,
    from_ref: Option<&str>,
    to_ref: Option<&str>,
    output_dir: &Path,
    timeout_ms: u64,
) -> Result<Value, ToolError> {
    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "Git:diff".to_string(),
            message: format!("Failed to create output directory: {e}"),
        })?;
    // TOCTOU mitigation: revalidate after directory creation
    // Use a dummy child path since validate_created_parent checks the parent
    ctx.sandbox
        .validate_created_parent(&output_dir.join("_check"))?;

    // Build the ref range arg: "from..to", just "from", or omit entirely.
    let ref_range: Option<String> = match (from_ref, to_ref) {
        (Some(f), Some(t)) => Some(format!("{f}..{t}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    };

    let mut numstat_args: Vec<String> = vec!["diff".into()];
    if let Some(r) = &ref_range {
        numstat_args.push(r.clone());
    }
    numstat_args.push("--numstat".into());

    let numstat_exec = run_git(
        ctx,
        working_dir,
        numstat_args,
        timeout_ms,
        MAX_OUTPUT_BYTES,
        DEFAULT_GIT_STDERR_BYTES,
    )
    .await?;

    if !numstat_exec.success {
        return Err(ToolError::ExecutionFailed {
            tool: "Git:diff".to_string(),
            message: format!("git diff --numstat failed: {}", numstat_exec.stderr.trim()),
        });
    }

    let mut files: Vec<FileDiffEntry> = Vec::new();
    let mut total_insertions = 0u32;
    let mut total_deletions = 0u32;

    for line in numstat_exec.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((ins, del, path, is_binary)) = parse_numstat_line(line) else {
            continue;
        };

        total_insertions = total_insertions.saturating_add(ins);
        total_deletions = total_deletions.saturating_add(del);

        let patch_filename = format!("{}.patch", sanitize_path_for_filename(&path));
        let patch_path = output_dir.join(&patch_filename);

        let mut patch_args: Vec<String> = vec!["diff".into()];
        if let Some(r) = &ref_range {
            patch_args.push(r.clone());
        }
        patch_args.push("--".into());
        patch_args.push(path.clone());
        let patch_exec = run_git(
            ctx,
            working_dir,
            patch_args,
            timeout_ms,
            MAX_OUTPUT_BYTES,
            DEFAULT_GIT_STDERR_BYTES,
        )
        .await?;

        let patch_content = if is_binary {
            format!("Binary file: {path}\n")
        } else {
            patch_exec.stdout.clone()
        };

        tokio::fs::write(&patch_path, &patch_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "Git:diff".to_string(),
                message: format!("Failed to write {}: {e}", patch_path.display()),
            })?;

        let status = if patch_exec.stdout.contains("new file mode") {
            "added"
        } else if patch_exec.stdout.contains("deleted file mode") {
            "deleted"
        } else if patch_exec.stdout.contains("rename from") {
            "renamed"
        } else {
            "modified"
        };

        let old_path = if status == "renamed" {
            patch_exec
                .stdout
                .lines()
                .find(|l| l.starts_with("rename from "))
                .map(|l| l.strip_prefix("rename from ").unwrap_or("").to_string())
        } else {
            None
        };

        files.push(FileDiffEntry {
            path,
            status: status.to_string(),
            old_path,
            insertions: ins,
            deletions: del,
            patch_file: patch_filename,
            binary: is_binary,
        });
    }

    let summary = DiffSummary {
        from_ref: from_ref.map(ToString::to_string),
        to_ref: to_ref.map(ToString::to_string),
        generated_at: chrono::Utc::now().to_rfc3339(),
        stats: DiffStats {
            files_changed: files.len(),
            insertions: total_insertions,
            deletions: total_deletions,
        },
        files,
    };

    let summary_json =
        serde_json::to_string_pretty(&summary).map_err(|e| ToolError::ExecutionFailed {
            tool: "Git:diff".to_string(),
            message: format!("Failed to serialize diff summary: {e}"),
        })?;
    let summary_path = output_dir.join("_summary.json");
    tokio::fs::write(&summary_path, &summary_json)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "Git:diff".to_string(),
            message: format!("Failed to write diff summary: {e}"),
        })?;

    Ok(json!(summary))
}

// ===== Argument types =====

#[derive(Deserialize)]
struct GitStatusArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default = "default_true")]
    porcelain: bool,
    #[serde(default = "default_true")]
    branch: bool,
    #[serde(default = "default_true")]
    untracked: bool,
}

#[derive(Deserialize)]
struct GitDiffArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    cached: bool,
    #[serde(default)]
    stat: bool,
    #[serde(default)]
    name_only: bool,
    #[serde(default)]
    unified: Option<i64>,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    from_ref: Option<String>,
    #[serde(default)]
    to_ref: Option<String>,
    #[serde(default)]
    output_dir: Option<String>,
}

#[derive(Deserialize)]
struct GitRestoreArgs {
    paths: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    staged: bool,
    #[serde(default = "default_true")]
    worktree: bool,
}

#[derive(Deserialize)]
struct GitAddArgs {
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    update: bool,
}

#[derive(Deserialize)]
struct GitCommitArgs {
    #[serde(rename = "type")]
    commit_type: String,
    #[serde(default)]
    scope: Option<String>,
    message: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct GitLogArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_count: Option<u32>,
    #[serde(default)]
    oneline: bool,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    grep: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct GitBranchArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    list_all: bool,
    #[serde(default)]
    list_remote: bool,
    #[serde(default)]
    create: Option<String>,
    #[serde(default)]
    delete: Option<String>,
    #[serde(default)]
    force_delete: Option<String>,
    #[serde(default)]
    rename: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
}

#[derive(Deserialize)]
struct GitCheckoutArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    create_branch: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GitStashArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    include_untracked: bool,
}

#[derive(Deserialize)]
struct GitShowArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    stat: bool,
    #[serde(default)]
    name_only: bool,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct GitBlameArgs {
    path: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

// ===== Handlers =====

async fn handle_git_status(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitStatusArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let porcelain = req.porcelain;
    let branch = req.branch;
    let untracked = req.untracked;

    let working_dir = ctx.working_dir.clone();

    let mut cmd_args: Vec<String> = vec!["status".into()];
    if porcelain {
        cmd_args.push("--porcelain=1".into());
        if branch {
            cmd_args.push("-b".into());
        }
        if !untracked {
            cmd_args.push("-uno".into());
        }
    }

    let max_cap = effective_max_bytes(ctx);
    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let clean = exec.success && exec.stdout.trim().is_empty();
    let text = if exec.success {
        if clean {
            "clean".to_string()
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("clean", json!(clean));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_diff(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let mut req: GitDiffArgs = parse_args(&args)?;

    // Treat empty/whitespace-only strings as absent — models sometimes pass ""
    // instead of omitting the field, which causes git to interpret them as paths.
    for opt in [&mut req.from_ref, &mut req.to_ref, &mut req.output_dir] {
        if opt.as_ref().is_some_and(|s| s.trim().is_empty()) {
            *opt = None;
        }
    }

    // Reject refs starting with '-' to prevent flag injection (e.g. --output=).
    if req.from_ref.as_ref().is_some_and(|s| s.starts_with('-')) {
        return Err(ToolError::BadArgs {
            message: "from_ref cannot start with '-'".to_string(),
        });
    }
    if req.to_ref.as_ref().is_some_and(|s| s.starts_with('-')) {
        return Err(ToolError::BadArgs {
            message: "to_ref cannot start with '-'".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();

    // When output_dir is provided, write per-file patches to disk.
    // Refs are optional: omit both for working-tree diff, provide from_ref alone
    // to diff against working tree, or both for ref-to-ref comparison.
    if let Some(output_dir) = req.output_dir.as_ref() {
        let output_dir = ctx
            .sandbox
            .resolve_path_for_create(output_dir, &working_dir)?;
        let summary = write_patches_to_dir(
            ctx,
            &working_dir,
            req.from_ref.as_deref(),
            req.to_ref.as_deref(),
            &output_dir,
            timeout_ms,
        )
        .await?;

        let files_changed = summary["stats"]["files_changed"].as_u64().unwrap_or(0);
        let desc = match (req.from_ref.as_ref(), req.to_ref.as_ref()) {
            (Some(f), Some(t)) => format!("Diff between {f} and {t}"),
            (Some(f), None) => format!("Diff of {f} vs working tree"),
            _ => "Working tree diff".to_string(),
        };
        let text = format!(
            "{desc}: {files_changed} files changed. Patches written to {}",
            output_dir.display()
        );

        let mut response = serde_json::Map::new();
        response.insert(
            "content".to_string(),
            json!([{"type": "text", "text": text}]),
        );
        response.insert("isError".to_string(), json!(false));
        if let Some(from_ref) = &req.from_ref {
            response.insert("from_ref".to_string(), json!(from_ref));
        }
        if let Some(to_ref) = &req.to_ref {
            response.insert("to_ref".to_string(), json!(to_ref));
        }
        response.insert(
            "output_dir".to_string(),
            json!(output_dir.display().to_string()),
        );
        response.insert("stats".to_string(), summary["stats"].clone());
        response.insert("files".to_string(), summary["files"].clone());

        return Ok(Value::Object(response));
    }

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["diff".into()];

    if req.cached {
        cmd_args.push("--cached".into());
    }
    if req.stat {
        cmd_args.push("--stat".into());
    }
    if req.name_only {
        cmd_args.push("--name-only".into());
    }
    if let Some(u) = req.unified
        && u >= 0
    {
        cmd_args.push(format!("-U{u}"));
    }

    // Ref-to-ref inline comparison (without output_dir).
    // `from_ref` alone → `git diff <from_ref>` (ref vs working tree).
    // Both refs → `git diff <from_ref> <to_ref>`.
    // Note: '-' prefix validation already done above.
    if let Some(from_ref) = &req.from_ref {
        cmd_args.push(from_ref.clone());
        if let Some(to_ref) = &req.to_ref {
            cmd_args.push(to_ref.clone());
        }
    }

    if let Some(paths) = &req.paths
        && !paths.is_empty()
    {
        cmd_args.push("--".into());
        for p in paths {
            if !p.trim().is_empty() {
                cmd_args.push(p.clone());
            }
        }
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        max_bytes,
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() {
            "no diff".to_string()
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("max_bytes", json!(max_bytes));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_restore(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitRestoreArgs = parse_args(&args)?;

    if req.paths.is_empty() {
        return Err(ToolError::BadArgs {
            message: "paths must be non-empty".to_string(),
        });
    }

    let staged = req.staged;
    let worktree = req.worktree;

    if !staged && !worktree {
        return Err(ToolError::BadArgs {
            message: "at least one of staged/worktree must be true".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["restore".into()];
    if staged {
        cmd_args.push("--staged".into());
    }
    if worktree {
        cmd_args.push("--worktree".into());
    }

    cmd_args.push("--".into());
    for p in &req.paths {
        if !p.trim().is_empty() {
            cmd_args.push(p.clone());
        }
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() && exec.stderr.trim().is_empty() {
            "ok".to_string()
        } else if exec.stdout.trim().is_empty() {
            trim_output(&exec.stderr)
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    Ok(build_git_response(&exec, text, None))
}

async fn handle_git_add(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitAddArgs = parse_args(&args)?;

    let use_all = req.all;
    let use_update = req.update;
    let paths = req.paths.unwrap_or_default();

    if !use_all && !use_update && paths.is_empty() {
        return Err(ToolError::BadArgs {
            message: "paths required unless 'all' or 'update' is true".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["add".into()];

    if use_all {
        cmd_args.push("-A".into());
    } else if use_update {
        cmd_args.push("-u".into());
    }

    if !paths.is_empty() {
        cmd_args.push("--".into());
        for p in &paths {
            if !p.trim().is_empty() {
                cmd_args.push(p.clone());
            }
        }
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        "ok".to_string()
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    Ok(build_git_response(&exec, text, None))
}

async fn handle_git_commit(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitCommitArgs = parse_args(&args)?;

    if req.commit_type.trim().is_empty() {
        return Err(ToolError::BadArgs {
            message: "type must not be empty".to_string(),
        });
    }
    if req.message.trim().is_empty() {
        return Err(ToolError::BadArgs {
            message: "message must not be empty".to_string(),
        });
    }

    let commit_msg = match &req.scope {
        Some(scope) if !scope.trim().is_empty() => {
            format!(
                "{}({}): {}",
                req.commit_type.trim(),
                scope.trim(),
                req.message.trim()
            )
        }
        _ => format!("{}: {}", req.commit_type.trim(), req.message.trim()),
    };

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let cmd_args: Vec<String> = vec!["commit".into(), "-m".into(), commit_msg.clone()];

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let commit_hash = exec
        .stdout
        .split_whitespace()
        .find(|s| s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit() || c == ']'))
        .map(|s| s.trim_end_matches(']').to_string());

    let text = if exec.success {
        trim_output(&exec.stdout)
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("commit_message", json!(commit_msg));
    extra_fields.insert("commit_hash", json!(commit_hash));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_log(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitLogArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["log".into()];

    if let Some(n) = req.max_count {
        cmd_args.push(format!("-{n}"));
    }
    if req.oneline {
        cmd_args.push("--oneline".into());
    }
    if let Some(fmt) = &req.format {
        cmd_args.push(format!("--format={fmt}"));
    }
    if let Some(author) = &req.author {
        cmd_args.push(format!("--author={author}"));
    }
    if let Some(since) = &req.since {
        cmd_args.push(format!("--since={since}"));
    }
    if let Some(until) = &req.until {
        cmd_args.push(format!("--until={until}"));
    }
    if let Some(grep) = &req.grep {
        cmd_args.push(format!("--grep={grep}"));
    }
    if let Some(path) = &req.path {
        cmd_args.push("--".into());
        cmd_args.push(path.clone());
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        max_bytes,
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() {
            "no commits".to_string()
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("max_bytes", json!(max_bytes));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_branch(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitBranchArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["branch".into()];

    if let Some(name) = &req.create {
        cmd_args.push("--".into()); // Prevent flag injection
        cmd_args.push(name.clone());
    } else if let Some(name) = &req.delete {
        cmd_args.push("-d".into());
        cmd_args.push("--".into()); // Prevent flag injection
        cmd_args.push(name.clone());
    } else if let Some(name) = &req.force_delete {
        cmd_args.push("-D".into());
        cmd_args.push("--".into()); // Prevent flag injection
        cmd_args.push(name.clone());
    } else if let Some(old_name) = &req.rename {
        cmd_args.push("-m".into());
        cmd_args.push("--".into()); // Prevent flag injection
        cmd_args.push(old_name.clone());
        if let Some(new_name) = &req.new_name {
            cmd_args.push(new_name.clone());
        } else {
            return Err(ToolError::BadArgs {
                message: "new_name required when renaming a branch".to_string(),
            });
        }
    } else {
        if req.list_all {
            cmd_args.push("-a".into());
        } else if req.list_remote {
            cmd_args.push("-r".into());
        }
        cmd_args.push("-v".into());
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() {
            "ok".to_string()
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    Ok(build_git_response(&exec, text, None))
}

async fn handle_git_checkout(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitCheckoutArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["checkout".into()];

    if let Some(branch) = &req.create_branch {
        if branch.starts_with('-') {
            return Err(ToolError::BadArgs {
                message: "branch name cannot start with '-'".to_string(),
            });
        }
        cmd_args.push("-b".into());
        cmd_args.push(branch.clone());
    } else if let Some(branch) = &req.branch {
        if branch.starts_with('-') {
            return Err(ToolError::BadArgs {
                message: "branch name cannot start with '-'".to_string(),
            });
        }
        cmd_args.push(branch.clone());
    } else if let Some(commit) = &req.commit {
        if commit.starts_with('-') {
            return Err(ToolError::BadArgs {
                message: "commit ref cannot start with '-'".to_string(),
            });
        }
        cmd_args.push(commit.clone());
    }

    if let Some(paths) = &req.paths
        && !paths.is_empty()
    {
        cmd_args.push("--".into());
        for p in paths {
            if !p.trim().is_empty() {
                cmd_args.push(p.clone());
            }
        }
    }

    if cmd_args.len() == 1 {
        return Err(ToolError::BadArgs {
            message: "at least one of branch, create_branch, commit, or paths is required"
                .to_string(),
        });
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() && exec.stderr.trim().is_empty() {
            "ok".to_string()
        } else if !exec.stderr.trim().is_empty() {
            trim_output(&exec.stderr)
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    Ok(build_git_response(&exec, text, None))
}

async fn handle_git_stash(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitStashArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();
    let max_cap = effective_max_bytes(ctx);

    let action = req.action.as_deref().unwrap_or("push");

    let mut cmd_args: Vec<String> = vec!["stash".into()];

    match action {
        "push" | "save" => {
            cmd_args.push("push".into());
            if req.include_untracked {
                cmd_args.push("-u".into());
            }
            if let Some(msg) = &req.message {
                cmd_args.push("-m".into());
                cmd_args.push(msg.clone());
            }
        }
        "pop" => {
            cmd_args.push("pop".into());
            if let Some(idx) = req.index {
                cmd_args.push(format!("stash@{{{idx}}}"));
            }
        }
        "apply" => {
            cmd_args.push("apply".into());
            if let Some(idx) = req.index {
                cmd_args.push(format!("stash@{{{idx}}}"));
            }
        }
        "drop" => {
            cmd_args.push("drop".into());
            if let Some(idx) = req.index {
                cmd_args.push(format!("stash@{{{idx}}}"));
            }
        }
        "list" => {
            cmd_args.push("list".into());
        }
        "show" => {
            cmd_args.push("show".into());
            cmd_args.push("-p".into());
            if let Some(idx) = req.index {
                cmd_args.push(format!("stash@{{{idx}}}"));
            }
        }
        "clear" => {
            cmd_args.push("clear".into());
        }
        _ => {
            return Err(ToolError::BadArgs {
                message: format!(
                    "unknown stash action '{action}'. Valid: push, pop, apply, drop, list, show, clear"
                ),
            });
        }
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        DEFAULT_GIT_STDOUT_BYTES.min(max_cap),
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        if exec.stdout.trim().is_empty() {
            match action {
                "list" => "no stashes".to_string(),
                _ => "ok".to_string(),
            }
        } else {
            trim_output(&exec.stdout)
        }
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("action", json!(action));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_show(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitShowArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["show".into()];

    if let Some(commit) = &req.commit {
        if commit.starts_with('-') {
            return Err(ToolError::BadArgs {
                message: "commit ref cannot start with '-'".to_string(),
            });
        }
        cmd_args.push(commit.clone());
    }
    if req.stat {
        cmd_args.push("--stat".into());
    }
    if req.name_only {
        cmd_args.push("--name-only".into());
    }
    if let Some(fmt) = &req.format {
        cmd_args.push(format!("--format={fmt}"));
    }

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        max_bytes,
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        trim_output(&exec.stdout)
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("max_bytes", json!(max_bytes));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

async fn handle_git_blame(ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    let req: GitBlameArgs = parse_args(&args)?;

    if req.path.trim().is_empty() {
        return Err(ToolError::BadArgs {
            message: "path must not be empty".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = ctx.working_dir.clone();

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["blame".into()];

    if let (Some(start), Some(end)) = (req.start_line, req.end_line) {
        cmd_args.push(format!("-L{start},{end}"));
    } else if let Some(start) = req.start_line {
        cmd_args.push(format!("-L{start},"));
    } else if let Some(end) = req.end_line {
        cmd_args.push(format!("-L1,{end}"));
    }

    if let Some(commit) = &req.commit {
        if commit.starts_with('-') {
            return Err(ToolError::BadArgs {
                message: "commit ref cannot start with '-'".to_string(),
            });
        }
        cmd_args.push(commit.clone());
    }

    cmd_args.push("--".into());
    cmd_args.push(req.path.clone());

    let exec = run_git(
        ctx,
        &working_dir,
        cmd_args,
        timeout_ms,
        max_bytes,
        DEFAULT_GIT_STDERR_BYTES.min(max_cap),
    )
    .await?;

    let text = if exec.success {
        trim_output(&exec.stdout)
    } else if !exec.stderr.trim().is_empty() {
        trim_output(&exec.stderr)
    } else {
        trim_output(&exec.stdout)
    };

    let mut extra_fields = HashMap::new();
    extra_fields.insert("path", json!(req.path));
    extra_fields.insert("max_bytes", json!(max_bytes));

    Ok(build_git_response(&exec, text, Some(extra_fields)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diff_without_output_dir_is_not_side_effecting() {
        let tool = GitTool;
        let args = json!({"command": "diff"});
        assert!(!tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::Low);
    }

    #[test]
    fn diff_with_output_dir_is_side_effecting() {
        let tool = GitTool;
        let args = json!({"command": "diff", "output_dir": "patches"});
        assert!(tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::Medium);
    }

    #[test]
    fn diff_with_empty_output_dir_is_not_side_effecting() {
        let tool = GitTool;
        let args = json!({"command": "diff", "output_dir": ""});
        assert!(!tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::Low);
    }

    #[test]
    fn diff_with_whitespace_output_dir_is_not_side_effecting() {
        let tool = GitTool;
        let args = json!({"command": "diff", "output_dir": "  "});
        assert!(!tool.is_side_effecting(&args));
    }

    #[test]
    fn status_is_not_side_effecting() {
        let tool = GitTool;
        let args = json!({"command": "status"});
        assert!(!tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::Low);
    }

    #[test]
    fn restore_is_side_effecting_high_risk() {
        let tool = GitTool;
        let args = json!({"command": "restore", "paths": ["file.rs"]});
        assert!(tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::High);
    }

    #[test]
    fn unknown_command_is_side_effecting_high_risk() {
        let tool = GitTool;
        let args = json!({"command": "unknown"});
        assert!(tool.is_side_effecting(&args));
        assert_eq!(tool.risk_level(&args), RiskLevel::High);
    }

    #[test]
    fn status_reads_user_data() {
        let tool = GitTool;
        assert!(tool.reads_user_data(&json!({"command": "status"})));
    }

    #[test]
    fn diff_reads_user_data() {
        let tool = GitTool;
        assert!(tool.reads_user_data(&json!({"command": "diff"})));
    }

    #[test]
    fn commit_does_not_read_user_data() {
        let tool = GitTool;
        assert!(!tool.reads_user_data(&json!({"command": "commit", "message": "test"})));
    }

    #[test]
    fn restore_does_not_read_user_data() {
        let tool = GitTool;
        assert!(!tool.reads_user_data(&json!({"command": "restore", "paths": ["f.rs"]})));
    }

    #[test]
    fn diff_args_rejects_from_ref_starting_with_dash() {
        let args: GitDiffArgs = serde_json::from_value(json!({
            "from_ref": "--output=/tmp/exfil"
        }))
        .unwrap();
        assert!(args.from_ref.as_ref().unwrap().starts_with('-'));
    }

    #[test]
    fn diff_args_rejects_to_ref_starting_with_dash() {
        let args: GitDiffArgs = serde_json::from_value(json!({
            "to_ref": "--work-tree=/tmp"
        }))
        .unwrap();
        assert!(args.to_ref.as_ref().unwrap().starts_with('-'));
    }

    #[test]
    fn diff_args_accepts_valid_refs() {
        let args: GitDiffArgs = serde_json::from_value(json!({
            "from_ref": "main",
            "to_ref": "HEAD~3"
        }))
        .unwrap();
        assert!(!args.from_ref.as_ref().unwrap().starts_with('-'));
        assert!(!args.to_ref.as_ref().unwrap().starts_with('-'));
    }
}

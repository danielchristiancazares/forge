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
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, redact_summary, sanitize_output,
};

const DEFAULT_GIT_TIMEOUT_MS: u64 = 30_000;
const MAX_GIT_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_GIT_STDOUT_BYTES: usize = 200_000;
const DEFAULT_GIT_STDERR_BYTES: usize = 100_000;
const MAX_OUTPUT_BYTES: usize = 5_000_000;

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
    fn name(self) -> &'static str {
        match self {
            GitToolKind::Status => "git_status",
            GitToolKind::Diff => "git_diff",
            GitToolKind::Restore => "git_restore",
            GitToolKind::Add => "git_add",
            GitToolKind::Commit => "git_commit",
            GitToolKind::Log => "git_log",
            GitToolKind::Branch => "git_branch",
            GitToolKind::Checkout => "git_checkout",
            GitToolKind::Stash => "git_stash",
            GitToolKind::Show => "git_show",
            GitToolKind::Blame => "git_blame",
        }
    }

    fn description(self) -> &'static str {
        match self {
            GitToolKind::Status => {
                "Show working tree status: staged, modified, and untracked files."
            }
            GitToolKind::Diff => {
                "Show file changes in the working tree or staging area. When from_ref, to_ref, and output_dir are provided, writes per-file patches to the directory."
            }
            GitToolKind::Restore => {
                "Discard uncommitted changes to specific files. WARNING: destructive."
            }
            GitToolKind::Add => "Stage files for commit.",
            GitToolKind::Commit => "Create a conventional commit (type(scope): message).",
            GitToolKind::Log => "Show commit history with configurable format and filters.",
            GitToolKind::Branch => "List, create, rename, or delete branches.",
            GitToolKind::Checkout => "Switch branches or restore working tree files.",
            GitToolKind::Stash => "Stash changes in a dirty working directory.",
            GitToolKind::Show => "Show commit details and diff.",
            GitToolKind::Blame => {
                "Show what revision and author last modified each line of a file."
            }
        }
    }

    fn schema(self) -> Value {
        match self {
            GitToolKind::Status => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory for the git command"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds before the command is aborted"},
                    "porcelain": {"type": "boolean", "default": true, "description": "Use porcelain output (`--porcelain=1`) when true"},
                    "branch": {"type": "boolean", "default": true, "description": "Include branch info (`-b`) in porcelain mode"},
                    "untracked": {"type": "boolean", "default": true, "description": "Include untracked files in porcelain mode (when false, uses `-uno`)"}
                },
                "required": []
            }),
            GitToolKind::Diff => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory for the git command"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds before the command is aborted"},
                    "cached": {"type": "boolean", "default": false, "description": "Diff staged changes (`--cached`)"},
                    "stat": {"type": "boolean", "default": false, "description": "Show diffstat only (`--stat`)"},
                    "name_only": {"type": "boolean", "default": false, "description": "Show only changed file names (`--name-only`)"},
                    "unified": {"type": "integer", "minimum": 0, "description": "Number of context lines (`-U<N>`)"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Optional path list to diff (passed after `--`)"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Maximum bytes captured from stdout before truncation"},
                    "from_ref": {"type": "string", "description": "Starting ref (tag/branch/commit) for ref-to-ref comparison"},
                    "to_ref": {"type": "string", "description": "Ending ref (tag/branch/commit) for ref-to-ref comparison"},
                    "output_dir": {"type": "string", "description": "Directory to write per-file patches (creates if missing). Required with from_ref/to_ref."}
                },
                "required": []
            }),
            GitToolKind::Restore => json!({
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Paths to restore (passed after `--`)"},
                    "working_dir": {"type": "string", "description": "Optional working directory for the git command"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds before the command is aborted"},
                    "staged": {"type": "boolean", "default": false, "description": "Restore the index/staging area (`--staged`)"},
                    "worktree": {"type": "boolean", "default": true, "description": "Restore the working tree (`--worktree`) (default true)"}
                },
                "required": ["paths"]
            }),
            GitToolKind::Add => json!({
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Files to stage"},
                    "all": {"type": "boolean", "default": false, "description": "Stage all changes (`-A`)"},
                    "update": {"type": "boolean", "default": false, "description": "Stage modified/deleted only (`-u`)"},
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"}
                },
                "required": []
            }),
            GitToolKind::Commit => json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "description": "Commit type: feat, fix, docs, style, refactor, test, chore, etc."},
                    "scope": {"type": "string", "description": "Optional scope/area of change"},
                    "message": {"type": "string", "description": "Commit description"},
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"}
                },
                "required": ["type", "message"]
            }),
            GitToolKind::Log => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "max_count": {"type": "integer", "minimum": 1, "description": "Limit number of commits to show"},
                    "oneline": {"type": "boolean", "default": false, "description": "Show each commit on a single line"},
                    "format": {"type": "string", "description": "Pretty-print format (e.g., '%H %s' for hash and subject)"},
                    "author": {"type": "string", "description": "Filter commits by author"},
                    "since": {"type": "string", "description": "Show commits after date (e.g., '2024-01-01', '2 weeks ago')"},
                    "until": {"type": "string", "description": "Show commits before date"},
                    "grep": {"type": "string", "description": "Filter commits by message pattern"},
                    "path": {"type": "string", "description": "Show commits affecting this path"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Maximum output bytes"}
                },
                "required": []
            }),
            GitToolKind::Branch => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "list_all": {"type": "boolean", "default": false, "description": "List both local and remote branches (`-a`)"},
                    "list_remote": {"type": "boolean", "default": false, "description": "List only remote branches (`-r`)"},
                    "create": {"type": "string", "description": "Create a new branch with this name"},
                    "delete": {"type": "string", "description": "Delete this branch (`-d`, must be merged)"},
                    "force_delete": {"type": "string", "description": "Force delete this branch (`-D`)"},
                    "rename": {"type": "string", "description": "Rename this branch (requires new_name)"},
                    "new_name": {"type": "string", "description": "New name when renaming a branch"}
                },
                "required": []
            }),
            GitToolKind::Checkout => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "branch": {"type": "string", "description": "Branch to switch to"},
                    "create_branch": {"type": "string", "description": "Create and switch to a new branch (`-b`)"},
                    "commit": {"type": "string", "description": "Checkout a specific commit (detached HEAD)"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Restore these paths from HEAD or specified commit"}
                },
                "required": []
            }),
            GitToolKind::Stash => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "action": {"type": "string", "enum": ["push", "pop", "apply", "drop", "list", "show", "clear"], "default": "push", "description": "Stash action to perform"},
                    "message": {"type": "string", "description": "Message for the stash (with push)"},
                    "index": {"type": "integer", "minimum": 0, "description": "Stash index for pop/apply/drop/show"},
                    "include_untracked": {"type": "boolean", "default": false, "description": "Include untracked files (with push)"}
                },
                "required": []
            }),
            GitToolKind::Show => json!({
                "type": "object",
                "properties": {
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "commit": {"type": "string", "description": "Commit to show (default: HEAD)"},
                    "stat": {"type": "boolean", "default": false, "description": "Show diffstat only"},
                    "name_only": {"type": "boolean", "default": false, "description": "Show only names of changed files"},
                    "format": {"type": "string", "description": "Pretty-print format for commit info"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Maximum output bytes"}
                },
                "required": []
            }),
            GitToolKind::Blame => json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to blame"},
                    "working_dir": {"type": "string", "description": "Optional working directory"},
                    "timeout_ms": {"type": "integer", "minimum": 100, "default": 30000, "description": "Timeout in milliseconds"},
                    "start_line": {"type": "integer", "minimum": 1, "description": "Start line number for range"},
                    "end_line": {"type": "integer", "minimum": 1, "description": "End line number for range"},
                    "commit": {"type": "string", "description": "Blame at specific commit instead of HEAD"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 5000000, "default": 200000, "description": "Maximum output bytes"}
                },
                "required": ["path"]
            }),
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

struct GitTool {
    kind: GitToolKind,
}

impl GitTool {
    fn new(kind: GitToolKind) -> Self {
        Self { kind }
    }
}

impl ToolExecutor for GitTool {
    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn description(&self) -> &'static str {
        self.kind.description()
    }

    fn schema(&self) -> Value {
        self.kind.schema()
    }

    fn is_side_effecting(&self) -> bool {
        self.kind.is_side_effecting()
    }

    fn requires_approval(&self) -> bool {
        self.kind.is_side_effecting()
    }

    fn risk_level(&self) -> RiskLevel {
        self.kind.risk_level()
    }

    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let summary = match self.kind {
            GitToolKind::Status => {
                let typed: GitStatusArgs = parse_args(args)?;
                format!(
                    "Git status in {}",
                    typed.working_dir.unwrap_or_else(|| ".".to_string())
                )
            }
            GitToolKind::Diff => {
                let typed: GitDiffArgs = parse_args(args)?;
                if let (Some(from_ref), Some(to_ref)) = (typed.from_ref, typed.to_ref) {
                    format!("Git diff {from_ref}..{to_ref}")
                } else {
                    "Git diff".to_string()
                }
            }
            GitToolKind::Restore => {
                let typed: GitRestoreArgs = parse_args(args)?;
                format!("Git restore {} file(s)", typed.paths.len())
            }
            GitToolKind::Add => {
                let typed: GitAddArgs = parse_args(args)?;
                if typed.all.unwrap_or(false) {
                    "Git add -A".to_string()
                } else if typed.update.unwrap_or(false) {
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

        Ok(redact_summary(&summary))
    }

    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(MAX_GIT_TIMEOUT_MS))
    }

    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let payload = match self.kind {
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

            let json = serde_json::to_string(&payload).map_err(|e| ToolError::ExecutionFailed {
                tool: self.kind.name().to_string(),
                message: e.to_string(),
            })?;

            Ok(sanitize_output(&json))
        })
    }
}

pub fn register_git_tools(registry: &mut super::ToolRegistry) -> Result<(), ToolError> {
    registry.register(Box::new(GitTool::new(GitToolKind::Status)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Diff)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Restore)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Add)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Commit)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Log)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Branch)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Checkout)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Stash)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Show)))?;
    registry.register(Box::new(GitTool::new(GitToolKind::Blame)))?;
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

fn parse_args<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
        message: e.to_string(),
    })
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

fn resolve_working_dir(ctx: &ToolCtx, working_dir: Option<String>) -> Result<PathBuf, ToolError> {
    let base = ctx.working_dir.clone();
    let dir = if let Some(raw) = working_dir {
        ctx.sandbox.resolve_path(&raw, &base)?
    } else {
        base
    };
    Ok(dir)
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

    let mut child = cmd.spawn().map_err(|e| ToolError::ExecutionFailed {
        tool: "git".to_string(),
        message: format!("failed to spawn git: {e}"),
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: "failed to capture git stdout".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: "failed to capture git stderr".to_string(),
        })?;

    let stdout_task = tokio::spawn(read_to_end_limited(stdout, max_stdout_bytes));
    let stderr_task = tokio::spawn(read_to_end_limited(stderr, max_stderr_bytes));

    let mut timed_out = false;
    let status = if let Ok(res) = time::timeout(Duration::from_millis(timeout_ms), child.wait()).await { res.map_err(|e| ToolError::ExecutionFailed {
        tool: "git".to_string(),
        message: e.to_string(),
    })? } else {
        timed_out = true;
        let _ = child.kill().await;
        match time::timeout(Duration::from_millis(2_000), child.wait()).await {
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
            continue;
        }
        let take = remaining.min(n);
        buf.extend_from_slice(&tmp[..take]);
        if take < n {
            truncated = true;
        }
    }

    (buf, truncated)
}

fn trim_output(output: &str) -> String {
    output.trim_end_matches(&['\r', '\n'][..]).to_string()
}

// ===== Git diff patch summary helpers =====

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
    from_ref: String,
    to_ref: String,
    generated_at: String,
    files: Vec<FileDiffEntry>,
    summary: DiffStats,
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
    from_ref: &str,
    to_ref: &str,
    output_dir: &Path,
    timeout_ms: u64,
) -> Result<Value, ToolError> {
    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git_diff".to_string(),
            message: format!("Failed to create output directory: {e}"),
        })?;

    let numstat_args = vec![
        "diff".into(),
        format!("{from_ref}..{to_ref}"),
        "--numstat".into(),
    ];

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
            tool: "git_diff".to_string(),
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

        total_insertions += ins;
        total_deletions += del;

        let patch_filename = format!("{}.patch", sanitize_path_for_filename(&path));
        let patch_path = output_dir.join(&patch_filename);

        let patch_args = vec![
            "diff".into(),
            format!("{from_ref}..{to_ref}"),
            "--".into(),
            path.clone(),
        ];
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
                tool: "git_diff".to_string(),
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
        from_ref: from_ref.to_string(),
        to_ref: to_ref.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        summary: DiffStats {
            files_changed: files.len(),
            insertions: total_insertions,
            deletions: total_deletions,
        },
        files,
    };

    let summary_json =
        serde_json::to_string_pretty(&summary).map_err(|e| ToolError::ExecutionFailed {
            tool: "git_diff".to_string(),
            message: format!("Failed to serialize summary: {e}"),
        })?;
    let summary_path = output_dir.join("_summary.json");
    tokio::fs::write(&summary_path, &summary_json)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git_diff".to_string(),
            message: format!("Failed to write summary: {e}"),
        })?;

    Ok(json!(summary))
}

// ===== Argument types =====

#[derive(Deserialize)]
struct GitStatusArgs {
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    porcelain: Option<bool>,
    #[serde(default)]
    branch: Option<bool>,
    #[serde(default)]
    untracked: Option<bool>,
}

#[derive(Deserialize)]
struct GitDiffArgs {
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    cached: Option<bool>,
    #[serde(default)]
    stat: Option<bool>,
    #[serde(default)]
    name_only: Option<bool>,
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
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    staged: Option<bool>,
    #[serde(default)]
    worktree: Option<bool>,
}

#[derive(Deserialize)]
struct GitAddArgs {
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    all: Option<bool>,
    #[serde(default)]
    update: Option<bool>,
}

#[derive(Deserialize)]
struct GitCommitArgs {
    #[serde(rename = "type")]
    commit_type: String,
    #[serde(default)]
    scope: Option<String>,
    message: String,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct GitLogArgs {
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_count: Option<u32>,
    #[serde(default)]
    oneline: Option<bool>,
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
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    list_all: Option<bool>,
    #[serde(default)]
    list_remote: Option<bool>,
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
    working_dir: Option<String>,
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
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    include_untracked: Option<bool>,
}

#[derive(Deserialize)]
struct GitShowArgs {
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    stat: Option<bool>,
    #[serde(default)]
    name_only: Option<bool>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct GitBlameArgs {
    path: String,
    #[serde(default)]
    working_dir: Option<String>,
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
    let porcelain = req.porcelain.unwrap_or(true);
    let branch = req.branch.unwrap_or(true);
    let untracked = req.untracked.unwrap_or(true);

    let working_dir = resolve_working_dir(ctx, req.working_dir)?;

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
    let req: GitDiffArgs = parse_args(&args)?;

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;

    if let (Some(from_ref), Some(to_ref), Some(output_dir)) = (
        req.from_ref.as_ref(),
        req.to_ref.as_ref(),
        req.output_dir.as_ref(),
    ) {
        let output_dir = ctx.sandbox.resolve_path(output_dir, &working_dir)?;
        let summary =
            write_patches_to_dir(ctx, &working_dir, from_ref, to_ref, &output_dir, timeout_ms)
                .await?;

        let files_changed = summary["summary"]["files_changed"].as_u64().unwrap_or(0);
        let text = format!(
            "Diff between {} and {}: {} files changed. Patches written to {}",
            from_ref,
            to_ref,
            files_changed,
            output_dir.display()
        );

        let mut response = serde_json::Map::new();
        response.insert(
            "content".to_string(),
            json!([{"type": "text", "text": text}]),
        );
        response.insert("isError".to_string(), json!(false));
        response.insert("from_ref".to_string(), json!(from_ref));
        response.insert("to_ref".to_string(), json!(to_ref));
        response.insert(
            "output_dir".to_string(),
            json!(output_dir.display().to_string()),
        );
        response.insert("summary".to_string(), summary["summary"].clone());
        response.insert("files".to_string(), summary["files"].clone());

        return Ok(Value::Object(response));
    }

    if req.from_ref.is_some() || req.to_ref.is_some() {
        if req.output_dir.is_none() {
            return Err(ToolError::BadArgs {
                message: "output_dir is required when using from_ref and to_ref".to_string(),
            });
        }
        if req.from_ref.is_none() || req.to_ref.is_none() {
            return Err(ToolError::BadArgs {
                message: "both from_ref and to_ref are required together".to_string(),
            });
        }
    }

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["diff".into()];

    if req.cached.unwrap_or(false) {
        cmd_args.push("--cached".into());
    }
    if req.stat.unwrap_or(false) {
        cmd_args.push("--stat".into());
    }
    if req.name_only.unwrap_or(false) {
        cmd_args.push("--name-only".into());
    }
    if let Some(u) = req.unified
        && u >= 0
    {
        cmd_args.push(format!("-U{u}"));
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

    let staged = req.staged.unwrap_or(false);
    let worktree = req.worktree.unwrap_or(true);

    if !staged && !worktree {
        return Err(ToolError::BadArgs {
            message: "at least one of staged/worktree must be true".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
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

    let use_all = req.all.unwrap_or(false);
    let use_update = req.update.unwrap_or(false);
    let paths = req.paths.unwrap_or_default();

    if !use_all && !use_update && paths.is_empty() {
        return Err(ToolError::BadArgs {
            message: "paths required unless 'all' or 'update' is true".to_string(),
        });
    }

    let timeout_ms = req.timeout_ms.unwrap_or(DEFAULT_GIT_TIMEOUT_MS);
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["log".into()];

    if let Some(n) = req.max_count {
        cmd_args.push(format!("-{n}"));
    }
    if req.oneline.unwrap_or(false) {
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["branch".into()];

    if let Some(name) = &req.create {
        cmd_args.push(name.clone());
    } else if let Some(name) = &req.delete {
        cmd_args.push("-d".into());
        cmd_args.push(name.clone());
    } else if let Some(name) = &req.force_delete {
        cmd_args.push("-D".into());
        cmd_args.push(name.clone());
    } else if let Some(old_name) = &req.rename {
        cmd_args.push("-m".into());
        cmd_args.push(old_name.clone());
        if let Some(new_name) = &req.new_name {
            cmd_args.push(new_name.clone());
        } else {
            return Err(ToolError::BadArgs {
                message: "new_name required when renaming a branch".to_string(),
            });
        }
    } else {
        if req.list_all.unwrap_or(false) {
            cmd_args.push("-a".into());
        } else if req.list_remote.unwrap_or(false) {
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
    let max_cap = effective_max_bytes(ctx);

    let mut cmd_args: Vec<String> = vec!["checkout".into()];

    if let Some(branch) = &req.create_branch {
        cmd_args.push("-b".into());
        cmd_args.push(branch.clone());
    } else if let Some(branch) = &req.branch {
        cmd_args.push(branch.clone());
    } else if let Some(commit) = &req.commit {
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;
    let max_cap = effective_max_bytes(ctx);

    let action = req.action.as_deref().unwrap_or("push");

    let mut cmd_args: Vec<String> = vec!["stash".into()];

    match action {
        "push" | "save" => {
            cmd_args.push("push".into());
            if req.include_untracked.unwrap_or(false) {
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;

    let max_cap = effective_max_bytes(ctx).min(MAX_OUTPUT_BYTES);
    let max_bytes = clamp_bytes(req.max_bytes, DEFAULT_GIT_STDOUT_BYTES, max_cap);

    let mut cmd_args: Vec<String> = vec!["show".into()];

    if let Some(commit) = &req.commit {
        cmd_args.push(commit.clone());
    }
    if req.stat.unwrap_or(false) {
        cmd_args.push("--stat".into());
    }
    if req.name_only.unwrap_or(false) {
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
    let working_dir = resolve_working_dir(ctx, req.working_dir)?;

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

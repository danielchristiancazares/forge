//! Built-in tool executors.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use globset::GlobBuilder;
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;

use similar::{ChangeTag, TextDiff};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{
    FileCacheEntry, ObservedRegion, PatchLimits, ReadFileLimits, RiskLevel, RunSandboxPolicy,
    SearchToolConfig, ToolCtx, ToolError, ToolExecutor, ToolFut, ToolRegistry, WebFetchToolConfig,
    parse_args, redact_distillate, sanitize_output,
};
use crate::config::default_true;
use crate::git;
use crate::normalize_cache_key;
use crate::region_hash;

/// Display a path without the Windows extended-length prefix (`\\?\`).
fn display_path(path: &Path) -> String {
    path_string_without_verbatim_prefix(path)
}

fn display_path_relative(path: &Path, root: &Path) -> String {
    let path = PathBuf::from(path_string_without_verbatim_prefix(path));
    let root = PathBuf::from(path_string_without_verbatim_prefix(root));
    if let Ok(rel) = path.strip_prefix(&root) {
        return rel.to_string_lossy().to_string();
    }
    path.to_string_lossy().to_string()
}

fn path_string_without_verbatim_prefix(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        return stripped.to_string();
    }
    s.to_string()
}
use crate::lp1::{self, FileContent};
use crate::memory::MemoryTool;
use crate::phase_gate::GeminiGateTool;
use crate::recall::RecallTool;
use crate::search::SearchTool;
use crate::webfetch::WebFetchTool;

#[derive(Debug)]
pub struct ReadFileTool {
    limits: ReadFileLimits,
}

#[derive(Debug)]
pub struct ApplyPatchTool {
    limits: PatchLimits,
}

#[derive(Debug, Default)]
pub struct WriteFileTool;

#[derive(Debug, Clone)]
pub struct RunCommandTool {
    shell: super::DetectedShell,
    run_policy: RunSandboxPolicy,
}

impl RunCommandTool {
    #[must_use]
    pub fn new(shell: super::DetectedShell, run_policy: RunSandboxPolicy) -> Self {
        Self { shell, run_policy }
    }
}

#[derive(Debug, Default)]
pub struct GlobTool;

impl ReadFileTool {
    #[must_use]
    pub fn new(limits: ReadFileLimits) -> Self {
        Self { limits }
    }
}

impl ApplyPatchTool {
    #[must_use]
    pub fn new(limits: PatchLimits) -> Self {
        Self { limits }
    }
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    start_line: Option<u32>,
    end_line: Option<u32>,
    #[serde(default = "default_true")]
    line_numbers: bool,
}

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    patch: String,
}

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RunCommandArgs {
    command: String,
    // Model-provided justification shown to the user during tool approval (plumbed through
    // the tool loop). The `Run` executor intentionally does not read this value.
    #[allow(dead_code)]
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    unsafe_allow_unsandboxed: bool,
}

#[derive(Debug, Deserialize)]
struct GlobArgs {
    pattern: String,
    path: Option<String>,
    #[serde(default)]
    hidden: bool,
    limit: Option<usize>,
}

const DEFAULT_GLOB_LIMIT: usize = 1000;
const MAX_GLOB_LIMIT: usize = 10_000;

/// Format a unified diff between old and new file content.
///
/// Produces output with:
/// - 1 line of context around each change
/// - `...` between changes separated by >3 unchanged lines
/// - Red (`-`) for deletions, green (`+`) for additions
#[must_use]
pub fn format_unified_diff(
    path: &str,
    old_bytes: &[u8],
    new_bytes: &[u8],
    existed: bool,
) -> String {
    format_unified_diff_width(path, old_bytes, new_bytes, existed, 0)
}

/// Like `format_unified_diff`, but accepts a minimum line-number column width.
///
/// When multiple files are displayed together, callers should pre-compute the
/// max line count across all files and pass the resulting digit width so that
/// every section aligns consistently.  Pass `0` to auto-detect from the file.
#[must_use]
pub fn format_unified_diff_width(
    _path: &str,
    old_bytes: &[u8],
    new_bytes: &[u8],
    _existed: bool,
    min_line_num_width: usize,
) -> String {
    let old_text = std::str::from_utf8(old_bytes).unwrap_or("");
    let new_text = std::str::from_utf8(new_bytes).unwrap_or("");

    let diff = TextDiff::from_lines(old_text, new_text);

    let mut out = String::new();

    let changes: Vec<_> = diff.iter_all_changes().collect();
    if changes.is_empty() {
        return String::new();
    }

    let max_line = old_text.lines().count().max(new_text.lines().count());
    let auto_width = if max_line == 0 {
        1
    } else {
        ((max_line as f64).log10().floor() as usize) + 1
    };
    let line_num_width = auto_width.max(min_line_num_width);

    let gap_marker = format!("{:>line_num_width$}\n", "...");

    let mut i = 0;
    let mut last_output_idx: Option<usize> = None;

    while i < changes.len() {
        let change = &changes[i];

        match change.tag() {
            ChangeTag::Equal => {
                let near_prev_change = i > 0 && changes[i - 1].tag() != ChangeTag::Equal;
                let near_next_change = changes
                    .get(i + 1)
                    .is_some_and(|c| c.tag() != ChangeTag::Equal);

                if near_prev_change || near_next_change {
                    if let Some(last_idx) = last_output_idx {
                        let gap = i - last_idx - 1;
                        if gap > 3 {
                            out.push_str(&gap_marker);
                        }
                    }
                    let line_no = change.old_index().unwrap_or(0) + 1;
                    write!(out, "{line_no:>line_num_width$}  ").unwrap();
                    out.push_str(change.value().trim_end_matches('\n'));
                    out.push('\n');
                    last_output_idx = Some(i);
                }
            }
            ChangeTag::Delete => {
                if let Some(last_idx) = last_output_idx {
                    let gap = i - last_idx - 1;
                    if gap > 3 {
                        out.push_str(&gap_marker);
                    }
                }
                let line_no = change.old_index().unwrap_or(0) + 1;
                write!(out, "{line_no:>line_num_width$} -").unwrap();
                out.push_str(change.value().trim_end_matches('\n'));
                out.push('\n');
                last_output_idx = Some(i);
            }
            ChangeTag::Insert => {
                if let Some(last_idx) = last_output_idx {
                    let gap = i - last_idx - 1;
                    if gap > 3 {
                        out.push_str(&gap_marker);
                    }
                }
                let line_no = change.new_index().unwrap_or(0) + 1;
                write!(out, "{line_no:>line_num_width$} +").unwrap();
                out.push_str(change.value().trim_end_matches('\n'));
                out.push('\n');
                last_output_idx = Some(i);
            }
        }

        i += 1;
    }

    out
}

/// Compute diff stats (additions and deletions) between old and new content.
#[must_use]
pub fn compute_diff_stats(old_bytes: &[u8], new_bytes: &[u8]) -> (u32, u32) {
    use similar::ChangeTag;

    let old_text = std::str::from_utf8(old_bytes).unwrap_or("");
    let new_text = std::str::from_utf8(new_bytes).unwrap_or("");

    let diff = TextDiff::from_lines(old_text, new_text);

    let mut additions: u32 = 0;
    let mut deletions: u32 = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }

    (additions, deletions)
}

impl ToolExecutor for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Find file paths by filename/path pattern only (not file contents)"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match file paths (e.g., '**/*.rs', 'src/**/*.{ts,tsx}'). Matches path names only, never file contents. Supports brace expansion."
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search from. Defaults to working directory."
                },
                "hidden": {
                    "type": "boolean",
                    "description": "Include hidden files (starting with '.'). Defaults to false."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of matches to return. Defaults to 1000, max 10000."
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn reads_user_data(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: GlobArgs = parse_args(args)?;
        let base = typed.path.as_deref().unwrap_or(".");
        Ok(redact_distillate(&format!(
            "Glob {} in {}",
            typed.pattern, base
        )))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: GlobArgs = parse_args(&args)?;

            if typed.pattern.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "pattern must not be empty".to_string(),
                });
            }

            let base_path = typed.path.as_deref().unwrap_or(".");
            let include_hidden = typed.hidden;
            let limit = typed
                .limit
                .unwrap_or(DEFAULT_GLOB_LIMIT)
                .clamp(1, MAX_GLOB_LIMIT);

            let base = ctx.sandbox.resolve_path(base_path, &ctx.working_dir)?;

            if !base.exists() {
                return Err(ToolError::ExecutionFailed {
                    tool: "Glob".to_string(),
                    message: format!("base path does not exist: {}", base.display()),
                });
            }
            if !base.is_dir() {
                return Err(ToolError::ExecutionFailed {
                    tool: "Glob".to_string(),
                    message: format!("base path is not a directory: {}", base.display()),
                });
            }

            let expanded = expand_braces(&typed.pattern);
            let mut builder = globset::GlobSetBuilder::new();
            for pat in &expanded {
                let glob = GlobBuilder::new(pat)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| ToolError::BadArgs {
                        message: format!("invalid glob pattern '{pat}': {e}"),
                    })?;
                builder.add(glob);
            }
            let glob_set = builder.build().map_err(|e| ToolError::BadArgs {
                message: format!("failed to compile glob patterns: {e}"),
            })?;

            let walker = WalkBuilder::new(&base)
                .hidden(!include_hidden)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .filter_entry(|entry| entry.file_name() != ".git")
                .build();

            let mut files: Vec<String> = Vec::new();
            let mut truncated = false;

            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue, // Skip unreadable entries
                };

                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    continue;
                }

                let path = entry.path();
                let rel_path = path.strip_prefix(&base).unwrap_or(path);

                if !glob_set.is_match(rel_path) {
                    continue;
                }

                // Check for sandbox deny patterns (e.g. .env, .ssh/) using the full path
                if ctx.sandbox.is_path_denied(path) {
                    continue;
                }

                files.push(display_path(path));

                if files.len() >= limit {
                    truncated = true;
                    break;
                }
            }

            files.sort();

            let output = if files.is_empty() {
                format!("No files match pattern: {}", typed.pattern)
            } else {
                let mut out = files.join("\n");
                if truncated {
                    out.push_str(&format!("\n\n[truncated at {limit} matches]"));
                }
                out
            };

            Ok(sanitize_output(&output))
        })
    }
}

/// Expands brace patterns like `{a,b,c}` into multiple alternatives.
/// Handles nested braces and multiple brace groups.
/// Example: `**/*.{cpp,h}` -> `["**/*.cpp", "**/*.h"]`
fn expand_braces(pattern: &str) -> Vec<String> {
    let mut results = vec![pattern.to_string()];

    loop {
        let mut expanded = false;
        let mut new_results = Vec::new();

        for pat in &results {
            if let Some(expansion) = expand_single_brace(pat) {
                new_results.extend(expansion);
                expanded = true;
            } else {
                new_results.push(pat.clone());
            }
        }

        results = new_results;
        if !expanded {
            break;
        }
    }

    results
}

/// Expands the first (innermost) brace group found in the pattern.
/// Returns None if no braces found.
fn expand_single_brace(pattern: &str) -> Option<Vec<String>> {
    let bytes = pattern.as_bytes();
    let mut brace_start = None;

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'{' {
            brace_start = Some(i);
        } else if b == b'}'
            && let Some(start) = brace_start
        {
            let prefix = &pattern[..start];
            let suffix = &pattern[i + 1..];
            let alternatives = &pattern[start + 1..i];

            let parts: Vec<&str> = alternatives.split(',').collect();

            if parts.len() > 1 {
                return Some(
                    parts
                        .into_iter()
                        .map(|p| format!("{prefix}{p}{suffix}"))
                        .collect(),
                );
            }
            // Single item in braces, just remove the braces
            return Some(vec![format!("{prefix}{}{suffix}", &pattern[start + 1..i])]);
        }
    }

    None
}

impl ToolExecutor for ReadFileTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read file contents, optionally by line range"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or relative file path to read" },
                "start_line": { "type": "integer", "minimum": 1, "description": "First line to read (1-indexed). Omit to start from the beginning." },
                "end_line": { "type": "integer", "minimum": 1, "description": "Last line to read, inclusive (1-indexed). Omit to read to end of file." },
                "line_numbers": { "type": "boolean", "default": true, "description": "Show line numbers (default: true)" }
            },
            "required": ["path"]
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn reads_user_data(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: ReadFileArgs = parse_args(args)?;
        let mut distillate = format!("Read {}", typed.path);
        if let Some(start) = typed.start_line {
            if let Some(end) = typed.end_line {
                distillate.push_str(&format!(" lines {start}-{end}"));
            } else {
                distillate.push_str(&format!(" lines {start}-"));
            }
        }
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        // Optimization threshold: skip hashing for range reads on files larger than this
        const HASH_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB

        Box::pin(async move {
            let typed: ReadFileArgs = parse_args(&args)?;
            if typed.path.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "path must not be empty".to_string(),
                });
            }

            if let Some(start) = typed.start_line
                && start == 0
            {
                return Err(ToolError::BadArgs {
                    message: "start_line must be >= 1".to_string(),
                });
            }
            if let Some(end) = typed.end_line
                && end == 0
            {
                return Err(ToolError::BadArgs {
                    message: "end_line must be >= 1".to_string(),
                });
            }
            if let (Some(start), Some(end)) = (typed.start_line, typed.end_line)
                && start > end
            {
                return Err(ToolError::BadArgs {
                    message: "start_line must be <= end_line".to_string(),
                });
            }

            let resolved = ctx.sandbox.resolve_path(&typed.path, &ctx.working_dir)?;
            let meta = std::fs::metadata(&resolved).map_err(|e| ToolError::ExecutionFailed {
                tool: "Read".to_string(),
                message: e.to_string(),
            })?;
            if meta.is_dir() {
                return Err(ToolError::ExecutionFailed {
                    tool: "Read".to_string(),
                    message: "path is a directory".to_string(),
                });
            }
            if is_crash_dump_artifact(&resolved) {
                return Err(ToolError::ExecutionFailed {
                    tool: "Read".to_string(),
                    message: "reading crash-dump artifacts is blocked".to_string(),
                });
            }

            let output_limit = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
            let read_limit = self
                .limits
                .max_file_read_bytes
                .min(ctx.available_capacity_bytes);

            let is_binary = sniff_binary(&resolved).map_err(|e| ToolError::ExecutionFailed {
                tool: "Read".to_string(),
                message: e.to_string(),
            })?;

            let show_line_numbers = typed.line_numbers;

            // Track the observed line range for surgical hashing
            let (output, observed_range): (String, Option<(u32, u32)>) = if is_binary {
                if typed.start_line.is_some() || typed.end_line.is_some() {
                    return Err(ToolError::BadArgs {
                        message: "Line ranges are not supported for binary files".to_string(),
                    });
                }
                ctx.allow_truncation = false;
                (read_binary(&resolved, output_limit)?, None)
            } else if typed.start_line.is_none() && typed.end_line.is_none() {
                if meta.len() as usize > read_limit {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Read".to_string(),
                        message: "File too large; use start_line/end_line".to_string(),
                    });
                }
                let content = read_text_lossy(&resolved)?;
                let line_count = content.lines().count().max(1) as u32;
                let formatted = if show_line_numbers {
                    format_with_line_numbers(&content, 1)
                } else {
                    content
                };
                (formatted, Some((1, line_count)))
            } else {
                let start = typed.start_line.unwrap_or(1) as usize;
                let end = typed.end_line.unwrap_or(u32::MAX) as usize;
                let content = read_text_range(&resolved, start, end, self.limits.max_scan_bytes)?;
                let lines_read = content.lines().count().max(1) as u32;
                let actual_end = (start as u32).saturating_add(lines_read).saturating_sub(1);
                let formatted = if show_line_numbers {
                    format_with_line_numbers(&content, start)
                } else {
                    content
                };
                (formatted, Some((start as u32, actual_end)))
            };

            // Update file cache with observed region for stale-file protection.
            // Skip for binary files (no line-based region concept).
            // Skip for range reads on large files (> 10MB) to avoid expensive hashing.
            if let Some((start_line, end_line)) = observed_range {
                let is_range_read = typed.start_line.is_some() || typed.end_line.is_some();
                let should_hash = !is_range_read || meta.len() <= HASH_THRESHOLD_BYTES;

                if should_hash {
                    let mut cache = ctx.file_cache.lock().await;
                    let key = normalize_cache_key(&resolved);

                    let new_entry = match cache.get(&key) {
                        Some(existing) => {
                            // Merge with existing observed region
                            if let Ok(merged) = region_hash::merge_regions(
                                &resolved,
                                &existing.observed,
                                start_line,
                                end_line,
                            ) {
                                FileCacheEntry {
                                    observed: merged,
                                    read_at: SystemTime::now(),
                                }
                            } else {
                                // If merge fails (IO error), create fresh region
                                if let Ok(region) =
                                    region_hash::create_region(&resolved, start_line, end_line)
                                {
                                    FileCacheEntry {
                                        observed: region,
                                        read_at: SystemTime::now(),
                                    }
                                } else {
                                    return Ok(sanitize_output(&output));
                                }
                            }
                        }
                        None => {
                            // First read: create initial region
                            if let Ok(region) =
                                region_hash::create_region(&resolved, start_line, end_line)
                            {
                                FileCacheEntry {
                                    observed: region,
                                    read_at: SystemTime::now(),
                                }
                            } else {
                                return Ok(sanitize_output(&output));
                            }
                        }
                    };
                    cache.insert(key, new_entry);
                }
            }

            Ok(sanitize_output(&output))
        })
    }
}

impl ToolExecutor for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Apply an LP1 patch to files"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string", "description": "An LP1-format patch string." }
            },
            "required": ["patch"]
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: ApplyPatchArgs = parse_args(args)?;
        let patch = lp1::parse_patch(&typed.patch).map_err(|e| ToolError::BadArgs {
            message: e.to_string(),
        })?;
        let files: Vec<String> = patch.files.iter().map(|f| f.path.clone()).collect();
        let distillate = if files.is_empty() {
            "Apply patch (no files)".to_string()
        } else {
            format!(
                "Apply patch to {} file(s): {}",
                files.len(),
                files.join(", ")
            )
        };
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: ApplyPatchArgs = parse_args(&args)?;
            if typed.patch.len() > self.limits.max_patch_bytes {
                return Err(ToolError::BadArgs {
                    message: "Patch exceeds max_patch_bytes".to_string(),
                });
            }

            let patch = lp1::parse_patch(&typed.patch).map_err(|e| ToolError::PatchFailed {
                file: PathBuf::from("<patch>"),
                message: e.to_string(),
            })?;

            let mut staged: Vec<StagedFile> = Vec::new();
            for file_patch in &patch.files {
                let resolved = ctx
                    .sandbox
                    .resolve_path(&file_patch.path, &ctx.working_dir)?;

                // Check if file exists FIRST (before stale check)
                let (existed, permissions) = match std::fs::metadata(&resolved) {
                    Ok(meta) => {
                        if meta.is_dir() {
                            return Err(ToolError::PatchFailed {
                                file: resolved.clone(),
                                message: "Path is a directory".to_string(),
                            });
                        }
                        (true, Some(meta.permissions()))
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => (false, None),
                    Err(err) => {
                        return Err(ToolError::PatchFailed {
                            file: resolved.clone(),
                            message: err.to_string(),
                        });
                    }
                };

                // Read file once to avoid TOCTOU between hash check and patch application
                let original_bytes = if existed {
                    std::fs::read(&resolved).map_err(|e| ToolError::PatchFailed {
                        file: resolved.clone(),
                        message: e.to_string(),
                    })?
                } else {
                    Vec::new()
                };

                // Stale file protection - only for EXISTING files
                // New files don't need stale check (there's nothing to go stale)
                if existed {
                    let entry = {
                        let cache = ctx.file_cache.lock().await;
                        cache.get(&normalize_cache_key(&resolved)).cloned()
                    };
                    let Some(entry) = entry else {
                        return Err(ToolError::StaleFile {
                            file: resolved.clone(),
                            reason: "File was not read before editing, or has been modified since last read".to_string(),
                        });
                    };

                    // Validate using surgical region hashing.
                    // Find the minimum line targeted by any operation for the range check.
                    // Operations without explicit targets (Append, Prepend, SetFinalNewline)
                    // use sentinel values that make sense for their behavior.
                    let parsed_for_check =
                        lp1::parse_file(&original_bytes).map_err(|e| ToolError::PatchFailed {
                            file: resolved.clone(),
                            message: e.to_string(),
                        })?;
                    let min_target_line = file_patch
                        .ops
                        .iter()
                        .filter_map(|op| {
                            match op {
                                lp1::Op::Replace { find, occ, .. }
                                | lp1::Op::InsertAfter { find, occ, .. }
                                | lp1::Op::InsertBefore { find, occ, .. }
                                | lp1::Op::Erase { find, occ } => {
                                    // Try to find the match line (0-indexed from find_match)
                                    lp1::find_match_line(&parsed_for_check.lines, find, *occ)
                                        .ok()
                                        .map(|idx| (idx + 1) as u32) // Convert to 1-indexed
                                }
                                lp1::Op::Prepend { .. } => Some(1), // Prepend targets line 1
                                lp1::Op::Append { .. } | lp1::Op::SetFinalNewline(_) => None,
                            }
                        })
                        .min();

                    // Validate the edit against the observed region
                    if let Some(target_line) = min_target_line {
                        region_hash::validate_edit(&original_bytes, target_line, &entry.observed)
                            .map_err(|e| ToolError::StaleFile {
                            file: resolved.clone(),
                            reason: e.to_string(),
                        })?;
                    } else {
                        // No line-targeted ops (only Append/SetFinalNewline) - still validate hashes
                        // to ensure the observed content hasn't changed
                        region_hash::validate_edit(
                            &original_bytes,
                            entry.observed.start_line, // Use start of observed region
                            &entry.observed,
                        )
                        .map_err(|e| ToolError::StaleFile {
                            file: resolved.clone(),
                            reason: e.to_string(),
                        })?;
                    }
                }

                let mut content = if existed {
                    lp1::parse_file(&original_bytes).map_err(|e| ToolError::PatchFailed {
                        file: resolved.clone(),
                        message: e.to_string(),
                    })?
                } else {
                    FileContent {
                        lines: Vec::new(),
                        final_newline: false,
                        eol_kind: None,
                    }
                };

                if !existed
                    && file_patch.ops.iter().any(|op| {
                        matches!(
                            op,
                            lp1::Op::Replace { .. }
                                | lp1::Op::InsertAfter { .. }
                                | lp1::Op::InsertBefore { .. }
                                | lp1::Op::Erase { .. }
                        )
                    })
                {
                    return Err(ToolError::PatchFailed {
                        file: resolved.clone(),
                        message: "File does not exist for match-based operation".to_string(),
                    });
                }

                lp1::apply_ops(&mut content, &file_patch.ops).map_err(|e| {
                    ToolError::PatchFailed {
                        file: resolved.clone(),
                        message: e.to_string(),
                    }
                })?;

                let new_bytes = lp1::emit_file(&content);
                let changed = new_bytes != original_bytes;

                staged.push(StagedFile {
                    path: resolved,
                    existed,
                    changed,
                    bytes: new_bytes,
                    original_bytes,
                    permissions,
                });
            }

            let any_changed = staged.iter().any(|s| s.changed);

            // Compute a global line-number column width so multi-file diffs
            // align consistently.  Each file's max(old_lines, new_lines) is
            // considered; the widest across all changed files wins.
            let global_max_line = staged
                .iter()
                .filter(|s| s.changed)
                .map(|s| {
                    let old_lines = std::str::from_utf8(&s.original_bytes)
                        .unwrap_or("")
                        .lines()
                        .count();
                    let new_lines = std::str::from_utf8(&s.bytes).unwrap_or("").lines().count();
                    old_lines.max(new_lines)
                })
                .max()
                .unwrap_or(0);
            let global_line_num_width = if global_max_line == 0 {
                1
            } else {
                ((global_max_line as f64).log10().floor() as usize) + 1
            };

            // Human-visible diff (unified-diff-style) derived from the LP1 ops.
            let mut diff_sections: Vec<String> = Vec::new();
            for file in &staged {
                if !file.changed {
                    continue;
                }
                let diff = format_unified_diff_width(
                    &file.path.to_string_lossy(),
                    &file.original_bytes,
                    &file.bytes,
                    file.existed,
                    global_line_num_width,
                );
                if !diff.is_empty() {
                    diff_sections.push(diff);
                }
            }
            let changed_count = staged.iter().filter(|s| s.changed).count();

            if any_changed {
                apply_staged_files(&staged, &ctx.sandbox)?;
                for file in &staged {
                    if !file.changed {
                        continue;
                    }
                    if file.existed {
                        ctx.turn_changes.record_modified(file.path.clone());
                    } else {
                        ctx.turn_changes.record_created(file.path.clone());
                    }
                    let (additions, deletions) =
                        compute_diff_stats(&file.original_bytes, &file.bytes);
                    ctx.turn_changes
                        .record_stats(file.path.clone(), additions, deletions);

                    // Update cache with rehashed region
                    let mut cache = ctx.file_cache.lock().await;
                    let key = normalize_cache_key(&file.path);
                    let new_entry = if let Some(existing) = cache.get(&key) {
                        // Existing file: rehash the same region bounds
                        FileCacheEntry {
                            observed: region_hash::rehash_region_bytes(
                                &file.bytes,
                                &existing.observed,
                            ),
                            read_at: SystemTime::now(),
                        }
                    } else {
                        // New file: create region covering entire file
                        let line_count = std::io::BufRead::lines(file.bytes.as_slice())
                            .count()
                            .max(1) as u32;
                        FileCacheEntry {
                            observed: ObservedRegion {
                                start_line: 1,
                                end_line: line_count,
                                prefix_hash: ObservedRegion::EMPTY_HASH,
                                region_hash: region_hash::hash_line_range_bytes(
                                    &file.bytes,
                                    1,
                                    line_count,
                                ),
                            },
                            read_at: SystemTime::now(),
                        }
                    };
                    cache.insert(key, new_entry);
                }
            }

            let output = if !any_changed {
                "No changes applied.".to_string()
            } else if changed_count == 1 && !diff_sections.is_empty() {
                diff_sections.join("\n\n")
            } else {
                let mut distillate_lines: Vec<String> = Vec::new();
                for file in &staged {
                    if !file.changed {
                        continue;
                    }
                    let rel_path = display_path_relative(&file.path, &ctx.working_dir);
                    if file.existed {
                        distillate_lines.push(format!("modified: {rel_path}"));
                    } else {
                        distillate_lines.push(format!("created: {rel_path}"));
                    }
                }
                let mut out = distillate_lines.join("\n");
                if !diff_sections.is_empty() {
                    out.push_str("\n\n");
                    out.push_str(&diff_sections.join("\n\n"));
                }
                out
            };

            Ok(sanitize_output(&output))
        })
    }
}

impl ToolExecutor for WriteFileTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write content to a new file, creating directories as needed"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to create. Parent directories are created automatically." },
                "content": { "type": "string", "description": "Full file content to write." }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: WriteFileArgs = parse_args(args)?;
        let distillate = format!(
            "Write new file: {} ({} bytes)",
            typed.path,
            typed.content.len()
        );
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: WriteFileArgs = parse_args(&args)?;

            if typed.path.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "path must not be empty".to_string(),
                });
            }

            let resolved = ctx
                .sandbox
                .resolve_path_for_create(&typed.path, &ctx.working_dir)?;
            if let Some(parent) = resolved.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!(
                            "failed to create parent directories for {}: {e}",
                            resolved.display()
                        ),
                    }
                })?;
                // TOCTOU mitigation: revalidate after directory creation
                ctx.sandbox.validate_created_parent(&resolved)?;
            }

            let bytes = typed.content.into_bytes();
            let byte_len = bytes.len();
            // Record stats: all lines are additions for a new file
            // PERF: use bytecount crate if this becomes a hot path.
            #[allow(clippy::naive_bytecount)]
            let line_count = bytes.iter().filter(|&&b| b == b'\n').count() as u32
                + u32::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));

            let write_path = resolved.clone();
            let write_result = tokio::task::spawn_blocking(move || {
                forge_context::atomic_write_new_with_options(
                    &write_path,
                    &bytes,
                    forge_context::AtomicWriteOptions {
                        sync_all: true,
                        dir_sync: true,
                        unix_mode: None,
                    },
                )
            })
            .await;

            match write_result {
                Ok(Ok(())) => {}
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!(
                            "file already exists: {}. Use Edit to modify existing files.",
                            display_path(&resolved)
                        ),
                    });
                }
                Ok(Err(err)) => {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!("failed to write {}: {err}", resolved.display()),
                    });
                }
                Err(err) => {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!("failed to write {}: {err}", resolved.display()),
                    });
                }
            }

            // Create observed region covering entire new file
            if let Ok(region) = region_hash::create_region(&resolved, 1, line_count as u32) {
                let mut cache = ctx.file_cache.lock().await;
                cache.insert(
                    normalize_cache_key(&resolved),
                    FileCacheEntry {
                        observed: region,
                        read_at: SystemTime::now(),
                    },
                );
            }

            ctx.turn_changes.record_created(resolved.clone());

            ctx.turn_changes
                .record_stats(resolved.clone(), line_count, 0);

            let output = format!("Created {} ({} bytes)", display_path(&resolved), byte_len);
            Ok(sanitize_output(&output))
        })
    }
}

impl ToolExecutor for RunCommandTool {
    fn name(&self) -> &'static str {
        "Run"
    }

    fn description(&self) -> &'static str {
        "Run a shell command"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute." },
                "reason": { "type": "string", "description": "Brief explanation of why this command needs to run." },
                "unsafe_allow_unsandboxed": { "type": "boolean", "description": "If true, allow unsandboxed execution when the sandbox is unavailable." }
            },
            "required": ["command"]
        })
    }

    fn is_side_effecting(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn risk_level(&self, _args: &serde_json::Value) -> RiskLevel {
        RiskLevel::High
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: RunCommandArgs = parse_args(args)?;
        let distillate = if typed.unsafe_allow_unsandboxed {
            format!(
                "Run command (unsandboxed override requested): {}",
                typed.command
            )
        } else {
            format!("Run command: {}", typed.command)
        };
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: RunCommandArgs = parse_args(&args)?;
            if typed.command.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "command must not be empty".to_string(),
                });
            }

            let policy_text = if cfg!(windows)
                && self.run_policy.windows.enabled
                && super::windows_run::is_powershell_shell(&self.shell)
            {
                Some(
                    super::powershell_ast::policy_text_for_command(
                        &self.shell.binary,
                        &typed.command,
                        &ctx.env_sanitizer,
                    )
                    .await?,
                )
            } else {
                None
            };
            let policy_text = policy_text
                .as_ref()
                .map(super::powershell_ast::PowerShellPolicyText::as_str)
                .unwrap_or(typed.command.as_str());

            // Validate against command blacklist BEFORE any execution
            ctx.command_blacklist.validate(policy_text)?;

            let prepared = super::windows_run::prepare_run_command(
                super::windows_run::RunCommandText::new(&typed.command, policy_text),
                &self.shell,
                self.run_policy,
                typed.unsafe_allow_unsandboxed,
                &ctx.working_dir,
            )?;

            let mut command = Command::new(prepared.program());
            for arg in prepared.args() {
                command.arg(arg);
            }
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .current_dir(&ctx.working_dir);

            super::process::apply_sanitized_env(&mut command, &ctx.env_sanitizer);

            let requires_host_sandbox = prepared.requires_host_sandbox();

            #[cfg(windows)]
            if requires_host_sandbox {
                use std::os::windows::process::CommandExt;
                command
                    .as_std_mut()
                    .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }

            #[cfg(unix)]
            {
                let _ = requires_host_sandbox;
                super::process::set_new_session(&mut command);
            }

            let child = command.spawn().map_err(|e| ToolError::ExecutionFailed {
                tool: "Run".to_string(),
                message: e.to_string(),
            })?;

            let mut guard = ChildGuard::new(child);

            if let Some(pid) = guard.child_mut().id() {
                match super::process::process_started_at_unix_ms(pid) {
                    Ok(process_started_at_unix_ms) => {
                        let _ = ctx.output_tx.try_send(super::ToolEvent::ProcessSpawned {
                            tool_call_id: ctx.tool_call_id.clone(),
                            pid,
                            process_started_at_unix_ms,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(pid, "failed to read process start time: {e}");
                    }
                }
            }

            #[cfg(windows)]
            let _windows_host_sandbox_guard = if requires_host_sandbox {
                Some(
                    super::windows_run_host::attach_process_to_sandbox(guard.child_mut()).map_err(
                        |e| ToolError::ExecutionFailed {
                            tool: "Run".to_string(),
                            message: format!(
                                "failed to attach Windows host isolation boundary: {e}"
                            ),
                        },
                    )?,
                )
            } else {
                // Best-effort orphan prevention: attach to a kill-on-close job so that if Forge
                // crashes, Windows tears down the child process.
                match super::windows_run_host::attach_process_to_kill_on_close(guard.child_mut()) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to attach `Run` child to kill-on-close job; crash cleanup may be incomplete: {e}"
                        );
                        None
                    }
                }
            };

            let stdout =
                guard
                    .child_mut()
                    .stdout
                    .take()
                    .ok_or_else(|| ToolError::ExecutionFailed {
                        tool: "Run".to_string(),
                        message: "Failed to capture stdout".to_string(),
                    })?;
            let stderr =
                guard
                    .child_mut()
                    .stderr
                    .take()
                    .ok_or_else(|| ToolError::ExecutionFailed {
                        tool: "Run".to_string(),
                        message: "Failed to capture stderr".to_string(),
                    })?;

            let max_collect = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
            let stdout_task = tokio::spawn(read_stream(
                stdout,
                ctx.output_tx.clone(),
                ctx.tool_call_id.clone(),
                true,
                max_collect,
            ));
            let stderr_task = tokio::spawn(read_stream(
                stderr,
                ctx.output_tx.clone(),
                ctx.tool_call_id.clone(),
                false,
                max_collect,
            ));

            let status =
                guard
                    .child_mut()
                    .wait()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "Run".to_string(),
                        message: e.to_string(),
                    })?;
            guard.disarm();

            let stdout_content = stdout_task.await.unwrap_or_default();
            let stderr_content = stderr_task.await.unwrap_or_default();

            // Build combined output (stdout + stderr if present)
            let mut output = String::new();
            if let Some(warning) = prepared.warning() {
                output.push_str("[sandbox warning]\n");
                output.push_str(warning);
                if !stdout_content.trim().is_empty() || !stderr_content.trim().is_empty() {
                    output.push_str("\n\n");
                }
            }
            output.push_str(&stdout_content);
            if !stderr_content.trim().is_empty() {
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str("[stderr]\n");
                output.push_str(&stderr_content);
            }

            if !status.success() {
                // Include the output in the error so the model can see what went wrong
                let exit_code = status.code().unwrap_or(-1);
                let message = if output.trim().is_empty() {
                    format!("exit code {exit_code}")
                } else {
                    format!("exit code {exit_code}\n\n{output}")
                };
                return Err(ToolError::ExecutionFailed {
                    tool: "Run".to_string(),
                    message,
                });
            }

            Ok(sanitize_output(&output))
        })
    }
}

/// Register built-in tools into the registry.
/// Schema-only tool definition for the Plan tool.
///
/// The Plan tool is intercepted by the engine before executor dispatch.
/// This definition exists solely for LLM visibility in the tool manifest.
fn plan_tool_definition() -> forge_types::ToolDefinition {
    forge_types::ToolDefinition::new(
        "Plan",
        "Create and manage a phased execution plan. The plan organizes work into phases \
         with steps, enforces ordering constraints, and tracks progress. Use 'create' at \
         the start of complex tasks, then 'advance'/'skip'/'fail' as you complete steps.",
        json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": ["create", "advance", "skip", "fail", "edit", "status"],
                    "description": "The plan operation to perform."
                },
                "phases": {
                    "type": "array",
                    "description": "Required for 'create'. Ordered list of phases.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Phase name."
                            },
                            "steps": {
                                "type": "array",
                                "description": "Steps in this phase.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "description": {
                                            "type": "string",
                                            "description": "What this step accomplishes."
                                        },
                                        "depends_on": {
                                            "type": "array",
                                            "items": { "type": "integer" },
                                            "description": "Step IDs from earlier phases that must complete first."
                                        }
                                    },
                                    "required": ["description"]
                                }
                            }
                        },
                        "required": ["name", "steps"]
                    }
                },
                "step_id": {
                    "type": "integer",
                    "description": "Required for 'advance', 'skip', 'fail'. The step ID to operate on."
                },
                "outcome": {
                    "type": "string",
                    "description": "Required for 'advance'. What was accomplished."
                },
                "reason": {
                    "type": "string",
                    "description": "Required for 'skip' and 'fail'. Why the step was skipped or failed."
                },
                "edit_op": {
                    "type": "object",
                    "description": "Required for 'edit'. The edit operation to apply.",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": [
                                "add_step", "remove_step", "reorder_step",
                                "update_description", "add_phase", "remove_phase"
                            ],
                            "description": "The type of edit."
                        },
                        "phase_index": {
                            "type": "integer",
                            "description": "Target phase index (for add_step, add_phase, remove_phase)."
                        },
                        "step_id": {
                            "type": "integer",
                            "description": "Target step ID (for remove_step, reorder_step, update_description)."
                        },
                        "new_phase": {
                            "type": "integer",
                            "description": "Destination phase index (for reorder_step)."
                        },
                        "description": {
                            "type": "string",
                            "description": "New description (for update_description)."
                        },
                        "step": {
                            "type": "object",
                            "description": "Step to add (for add_step).",
                            "properties": {
                                "description": { "type": "string" },
                                "depends_on": {
                                    "type": "array",
                                    "items": { "type": "integer" }
                                }
                            },
                            "required": ["description"]
                        },
                        "phase": {
                            "type": "object",
                            "description": "Phase to add (for add_phase).",
                            "properties": {
                                "name": { "type": "string" },
                                "steps": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "description": { "type": "string" },
                                            "depends_on": {
                                                "type": "array",
                                                "items": { "type": "integer" }
                                            }
                                        },
                                        "required": ["description"]
                                    }
                                }
                            },
                            "required": ["name", "steps"]
                        }
                    },
                    "required": ["type"]
                },
                "justification": {
                    "type": "string",
                    "description": "Required for 'edit'. Why this edit is needed."
                }
            },
            "required": ["subcommand"]
        }),
    )
}

pub fn register_builtins(
    registry: &mut ToolRegistry,
    read_limits: ReadFileLimits,
    patch_limits: PatchLimits,
    search_config: SearchToolConfig,
    webfetch_config: WebFetchToolConfig,
    shell: super::DetectedShell,
    run_policy: RunSandboxPolicy,
) -> Result<(), ToolError> {
    registry.register(Box::new(ReadFileTool::new(read_limits)))?;
    registry.register(Box::new(ApplyPatchTool::new(patch_limits)))?;
    registry.register(Box::new(WriteFileTool))?;
    registry.register(Box::new(RunCommandTool::new(shell, run_policy)))?;
    registry.register(Box::new(GlobTool))?;
    git::register_git_tool(registry)?;
    registry.register(Box::new(SearchTool::new(search_config)))?;
    registry.register(Box::new(WebFetchTool::new(webfetch_config)))?;
    registry.register(Box::new(RecallTool))?;
    registry.register(Box::new(MemoryTool))?;
    registry.register(Box::new(GeminiGateTool))?;
    registry.register_schema(plan_tool_definition())?;
    Ok(())
}

fn sniff_binary(path: &Path) -> Result<bool, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 8192];
    let n = file.read(&mut buf)?;
    if n == 0 {
        return Ok(false);
    }
    if buf[..n].contains(&0) {
        return Ok(true);
    }
    // Treat only NUL-containing files as binary; non-UTF-8 text is decoded lossily.
    Ok(false)
}

fn is_crash_dump_artifact(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower == "core" || lower.starts_with("core.")
        })
    {
        return true;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "core" | "dmp" | "mdmp" | "stackdump"
            )
        })
}

fn read_binary(path: &Path, output_limit: usize) -> Result<String, ToolError> {
    let mut header = "[binary:base64]".to_string();
    let meta = std::fs::metadata(path).map_err(|e| ToolError::ExecutionFailed {
        tool: "Read".to_string(),
        message: e.to_string(),
    })?;

    let mut truncated = false;
    let mut available = output_limit.saturating_sub(header.len() + 1); // +1 for newline
    if available == 0 {
        return Ok(header.chars().take(output_limit).collect());
    }

    let max_raw = (available / 4) * 3;
    if meta.len() as usize > max_raw {
        truncated = true;
    }

    if truncated {
        header.push_str("[truncated]");
        available = output_limit.saturating_sub(header.len() + 1);
    }

    let max_raw = (available / 4) * 3;
    let mut file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
        tool: "Read".to_string(),
        message: e.to_string(),
    })?;
    let mut buf = vec![0u8; max_raw];
    let n = file
        .read(&mut buf)
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "Read".to_string(),
            message: e.to_string(),
        })?;
    buf.truncate(n);

    let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
    let mut out = String::new();
    out.push_str(&header);
    out.push('\n');
    out.push_str(&encoded);
    Ok(out)
}

/// Format text content with line numbers.
/// Uses right-aligned line numbers with a separator: "  1| content"
fn format_with_line_numbers(content: &str, start_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let max_line_num = start_line + lines.len() - 1;
    let width = max_line_num.to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line_num = start_line + i;
        out.push_str(&format!("{line_num:>width$}| {line}\n"));
    }
    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn read_text_range(
    path: &Path,
    start: usize,
    end: usize,
    max_scan_bytes: usize,
) -> Result<String, ToolError> {
    let file = std::fs::File::open(path).map_err(|e| ToolError::ExecutionFailed {
        tool: "Read".to_string(),
        message: e.to_string(),
    })?;
    let mut reader = BufReader::new(file);
    let mut output = String::new();
    let mut line_num = 1usize;
    let mut scanned = 0usize;
    let mut line = Vec::new();

    loop {
        line.clear();
        let bytes =
            reader
                .read_until(b'\n', &mut line)
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "Read".to_string(),
                    message: e.to_string(),
                })?;
        if bytes == 0 {
            break;
        }
        scanned += bytes;
        if scanned > max_scan_bytes {
            return Err(ToolError::ExecutionFailed {
                tool: "Read".to_string(),
                message: "Scan limit exceeded; narrow the range".to_string(),
            });
        }
        if line_num >= start && line_num <= end {
            output.push_str(&String::from_utf8_lossy(&line));
        }
        if line_num >= end {
            break;
        }
        line_num += 1;
    }

    Ok(output)
}

fn read_text_lossy(path: &Path) -> Result<String, ToolError> {
    let bytes = std::fs::read(path).map_err(|e| ToolError::ExecutionFailed {
        tool: "Read".to_string(),
        message: e.to_string(),
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

struct StagedFile {
    path: PathBuf,
    existed: bool,
    changed: bool,
    bytes: Vec<u8>,
    original_bytes: Vec<u8>,
    /// Original file permissions, used to preserve mode on Unix after atomic write.
    #[cfg_attr(not(unix), allow(dead_code))]
    permissions: Option<std::fs::Permissions>,
}

fn apply_staged_files(
    staged: &[StagedFile],
    sandbox: &crate::sandbox::Sandbox,
) -> Result<(), ToolError> {
    for file in staged.iter().filter(|s| s.changed) {
        let parent = file.path.parent().ok_or_else(|| ToolError::PatchFailed {
            file: file.path.clone(),
            message: "Invalid path".to_string(),
        })?;

        // Ensure parent directories exist for new files
        if !file.existed && !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| ToolError::PatchFailed {
                file: file.path.clone(),
                message: format!("Failed to create parent directories: {e}"),
            })?;
            // Validate that no symlink was injected between create_dir_all and write.
            // This mirrors WriteFileTool's post-create validation.
            sandbox.validate_created_parent(&file.path)?;
        }

        // Extract unix mode from existing permissions to preserve across atomic write
        #[cfg(unix)]
        let unix_mode = file.permissions.as_ref().map(|p| {
            use std::os::unix::fs::PermissionsExt;
            p.mode()
        });
        #[cfg(not(unix))]
        let unix_mode: Option<u32> = None;

        let options = forge_context::AtomicWriteOptions {
            sync_all: true,
            dir_sync: true,
            unix_mode,
        };

        let result = if file.existed {
            forge_context::atomic_write_with_options(&file.path, &file.bytes, options)
        } else {
            forge_context::atomic_write_new_with_options(&file.path, &file.bytes, options)
        };

        result.map_err(|e| ToolError::PatchFailed {
            file: file.path.clone(),
            message: e.to_string(),
        })?;
    }

    Ok(())
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    tx: tokio::sync::mpsc::Sender<super::ToolEvent>,
    tool_call_id: String,
    is_stdout: bool,
    max_collect: usize,
) -> String {
    let mut buf = [0u8; 4096];
    let mut collected = String::new();
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
        if collected.len() < max_collect {
            let remaining = max_collect - collected.len();
            let mut take = remaining.min(chunk.len());
            while take > 0 && !chunk.is_char_boundary(take) {
                take -= 1;
            }
            collected.push_str(&chunk[..take]);
        }
        let event = if is_stdout {
            super::ToolEvent::StdoutChunk {
                tool_call_id: tool_call_id.clone(),
                chunk: super::sanitize_output(&chunk),
            }
        } else {
            super::ToolEvent::StderrChunk {
                tool_call_id: tool_call_id.clone(),
                chunk: super::sanitize_output(&chunk),
            }
        };
        let _ = tx.try_send(event);
    }
    collected
}

use super::process::ChildGuard;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DetectedShell, RunSandboxPolicy};

    fn run_tool() -> RunCommandTool {
        RunCommandTool::new(
            DetectedShell {
                binary: std::path::PathBuf::from("pwsh"),
                args: vec!["-NoProfile".to_string(), "-Command".to_string()],
                name: "pwsh".to_string(),
            },
            RunSandboxPolicy::default(),
        )
    }

    #[test]
    fn detects_crash_dump_artifacts() {
        assert!(is_crash_dump_artifact(std::path::Path::new("core")));
        assert!(is_crash_dump_artifact(std::path::Path::new("core.1234")));
        assert!(is_crash_dump_artifact(std::path::Path::new("dump.dmp")));
        assert!(is_crash_dump_artifact(std::path::Path::new("dump.mdmp")));
        assert!(is_crash_dump_artifact(std::path::Path::new(
            "panic.stackdump"
        )));
        assert!(is_crash_dump_artifact(std::path::Path::new(
            "snapshot.core"
        )));
    }

    #[test]
    fn allows_non_dump_file_names() {
        assert!(!is_crash_dump_artifact(std::path::Path::new("src/main.rs")));
        assert!(!is_crash_dump_artifact(std::path::Path::new(
            "docs/core-concepts.md"
        )));
        assert!(!is_crash_dump_artifact(std::path::Path::new(
            "notes.dmp.txt"
        )));
    }

    #[test]
    fn expand_braces_no_braces() {
        assert_eq!(expand_braces("**/*.rs"), vec!["**/*.rs"]);
    }

    #[test]
    fn expand_braces_single_group() {
        let mut result = expand_braces("**/*.{rs,toml}");
        result.sort();
        assert_eq!(result, vec!["**/*.rs", "**/*.toml"]);
    }

    #[test]
    fn expand_braces_multiple_alternatives() {
        let mut result = expand_braces("src/*.{ts,tsx,js,jsx}");
        result.sort();
        assert_eq!(
            result,
            vec!["src/*.js", "src/*.jsx", "src/*.ts", "src/*.tsx"]
        );
    }

    #[test]
    fn expand_braces_single_item_removes_braces() {
        assert_eq!(expand_braces("**/*.{rs}"), vec!["**/*.rs"]);
    }

    #[test]
    fn expand_braces_nested() {
        // Nested braces expand innermost first
        let result = expand_braces("{a{b,c}}");
        // {a{b,c}} -> {ab}, {ac} -> ab, ac
        assert!(result.contains(&"ab".to_string()));
        assert!(result.contains(&"ac".to_string()));
    }

    #[test]
    fn expand_braces_empty_pattern() {
        assert_eq!(expand_braces(""), vec![""]);
    }

    #[test]
    fn expand_single_brace_no_braces() {
        assert_eq!(expand_single_brace("**/*.rs"), None);
    }

    #[test]
    fn expand_single_brace_simple() {
        let result = expand_single_brace("{a,b}").unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn expand_single_brace_with_prefix_suffix() {
        let result = expand_single_brace("pre{a,b}post").unwrap();
        assert_eq!(result, vec!["preapost", "prebpost"]);
    }

    #[test]
    fn glob_args_deserialize() {
        let json = serde_json::json!({
            "pattern": "**/*.rs",
            "path": "src",
            "hidden": true,
            "limit": 500
        });
        let args: GlobArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.pattern, "**/*.rs");
        assert_eq!(args.path, Some("src".to_string()));
        assert!(args.hidden);
        assert_eq!(args.limit, Some(500));
    }

    #[test]
    fn glob_args_deserialize_minimal() {
        let json = serde_json::json!({ "pattern": "*.txt" });
        let args: GlobArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.pattern, "*.txt");
        assert_eq!(args.path, None);
        assert!(!args.hidden);
        assert_eq!(args.limit, None);
    }

    #[test]
    fn glob_tool_schema_has_required_pattern() {
        let tool = GlobTool;
        let schema = tool.schema();
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("pattern")));
    }

    #[test]
    fn glob_tool_is_not_side_effecting() {
        let tool = GlobTool;
        assert!(!tool.is_side_effecting(&serde_json::json!({})));
    }

    #[test]
    fn glob_tool_does_not_require_approval() {
        let tool = GlobTool;
        assert!(!tool.requires_approval());
    }

    #[test]
    fn glob_tool_reads_user_data() {
        let tool = GlobTool;
        assert!(tool.reads_user_data(&serde_json::json!({})));
    }

    #[test]
    fn glob_tool_risk_level_is_low() {
        let tool = GlobTool;
        assert_eq!(tool.risk_level(&serde_json::json!({})), RiskLevel::Low);
    }

    #[test]
    fn run_tool_schema_exposes_unsandboxed_override_flag() {
        let tool = run_tool();
        let schema = tool.schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .expect("properties object");
        assert!(props.contains_key("unsafe_allow_unsandboxed"));
    }

    #[test]
    fn run_tool_approval_summary_marks_unsandboxed_override() {
        let tool = run_tool();
        let summary = tool
            .approval_summary(&serde_json::json!({
                "command": "Get-ChildItem",
                "unsafe_allow_unsandboxed": true
            }))
            .expect("summary");
        assert!(summary.contains("unsandboxed override requested"));
    }

    #[test]
    fn format_with_line_numbers_basic() {
        let content = "line one\nline two\nline three";
        let result = format_with_line_numbers(content, 1);
        assert_eq!(result, "1| line one\n2| line two\n3| line three");
    }

    #[test]
    fn format_with_line_numbers_with_trailing_newline() {
        let content = "line one\nline two\n";
        let result = format_with_line_numbers(content, 1);
        assert_eq!(result, "1| line one\n2| line two\n");
    }

    #[test]
    fn format_with_line_numbers_start_offset() {
        let content = "middle\nend";
        let result = format_with_line_numbers(content, 50);
        assert_eq!(result, "50| middle\n51| end");
    }

    #[test]
    fn format_with_line_numbers_wide_numbers() {
        let content = "a\nb";
        let result = format_with_line_numbers(content, 99);
        // Both lines should be padded to same width (3 digits for 100)
        assert_eq!(result, " 99| a\n100| b");
    }

    #[test]
    fn format_with_line_numbers_empty() {
        let content = "";
        let result = format_with_line_numbers(content, 1);
        assert_eq!(result, "");
    }

    #[test]
    fn format_with_line_numbers_single_line() {
        let content = "only line";
        let result = format_with_line_numbers(content, 1);
        assert_eq!(result, "1| only line");
    }

    #[test]
    fn read_tool_reads_user_data() {
        let tool = ReadFileTool::new(ReadFileLimits {
            max_file_read_bytes: 1024,
            max_scan_bytes: 4096,
        });
        assert!(tool.reads_user_data(&serde_json::json!({"path": "foo.rs"})));
    }

    #[test]
    fn write_tool_does_not_read_user_data() {
        let tool = WriteFileTool;
        assert!(!tool.reads_user_data(&serde_json::json!({})));
    }
}

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
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::{
    FileCacheEntry, PatchLimits, ReadFileLimits, RiskLevel, RunSandboxPolicy, SearchToolConfig,
    ToolCtx, ToolError, ToolExecutor, ToolFut, ToolRegistry, WebFetchToolConfig, redact_distillate,
    sanitize_output,
};
use crate::tools::git;

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
use crate::tools::lp1::{self, FileContent};
use crate::tools::memory::MemoryTool;
use crate::tools::recall::RecallTool;
use crate::tools::search::SearchTool;
use crate::tools::webfetch::WebFetchTool;

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
    pub fn new(shell: super::DetectedShell, run_policy: RunSandboxPolicy) -> Self {
        Self { shell, run_policy }
    }
}

#[derive(Debug, Default)]
pub struct GlobTool;

impl ReadFileTool {
    pub fn new(limits: ReadFileLimits) -> Self {
        Self { limits }
    }
}

impl ApplyPatchTool {
    pub fn new(limits: PatchLimits) -> Self {
        Self { limits }
    }
}

fn default_true() -> bool {
    true
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
pub(crate) fn format_unified_diff(
    _path: &str,
    old_bytes: &[u8],
    new_bytes: &[u8],
    _existed: bool,
) -> String {
    let old_text = std::str::from_utf8(old_bytes).unwrap_or("");
    let new_text = std::str::from_utf8(new_bytes).unwrap_or("");

    let diff = TextDiff::from_lines(old_text, new_text);

    let mut out = String::new();

    // Collect all changes with their line indices
    let changes: Vec<_> = diff.iter_all_changes().collect();
    if changes.is_empty() {
        return String::new();
    }

    // Determine the width needed for line numbers (based on max line count)
    let max_line = old_text.lines().count().max(new_text.lines().count());
    let line_num_width = if max_line == 0 {
        1
    } else {
        ((max_line as f64).log10().floor() as usize) + 1
    };

    // Group changes into hunks with 1 line of context, collapsing gaps >3 lines
    let mut i = 0;
    let mut last_output_idx: Option<usize> = None;

    while i < changes.len() {
        let change = &changes[i];

        match change.tag() {
            ChangeTag::Equal => {
                // Check if this context line is near a change
                let near_prev_change = i > 0 && changes[i - 1].tag() != ChangeTag::Equal;
                let near_next_change = changes
                    .get(i + 1)
                    .is_some_and(|c| c.tag() != ChangeTag::Equal);

                if near_prev_change || near_next_change {
                    // Check if we need a gap marker
                    if let Some(last_idx) = last_output_idx {
                        let gap = i - last_idx - 1;
                        if gap > 3 {
                            out.push_str("...\n");
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
                        out.push_str("...\n");
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
                        out.push_str("...\n");
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
pub(crate) fn compute_diff_stats(old_bytes: &[u8], new_bytes: &[u8]) -> (u32, u32) {
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

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: GlobArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
        let base = typed.path.as_deref().unwrap_or(".");
        Ok(redact_distillate(&format!(
            "Glob {} in {}",
            typed.pattern, base
        )))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: GlobArgs = serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;

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

            // Resolve base path through sandbox
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

            // Build glob matcher - expand braces and compile patterns
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

            // Walk directory tree, respecting .gitignore
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

                // Skip directories
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    continue;
                }

                let path = entry.path();
                let rel_path = path.strip_prefix(&base).unwrap_or(path);

                // Check if path matches any pattern
                if !glob_set.is_match(rel_path) {
                    continue;
                }

                files.push(display_path(path));

                if files.len() >= limit {
                    truncated = true;
                    break;
                }
            }

            // Sort for consistent output
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
                "path": { "type": "string" },
                "start_line": { "type": "integer", "minimum": 1 },
                "end_line": { "type": "integer", "minimum": 1 },
                "line_numbers": { "type": "boolean", "default": true, "description": "Show line numbers (default: true)" }
            },
            "required": ["path"]
        })
    }

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Low
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: ReadFileArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
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
        const HASH_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024; // 10MB

        Box::pin(async move {
            let typed: ReadFileArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;
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

            let resolved = ctx
                .sandbox
                .resolve_path_for_create(&typed.path, &ctx.working_dir)?;
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

            let output = if is_binary {
                if typed.start_line.is_some() || typed.end_line.is_some() {
                    return Err(ToolError::BadArgs {
                        message: "Line ranges are not supported for binary files".to_string(),
                    });
                }
                ctx.allow_truncation = false;
                read_binary(&resolved, output_limit)?
            } else if typed.start_line.is_none() && typed.end_line.is_none() {
                if meta.len() as usize > read_limit {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Read".to_string(),
                        message: "File too large; use start_line/end_line".to_string(),
                    });
                }
                let content = read_text_lossy(&resolved)?;
                if show_line_numbers {
                    format_with_line_numbers(&content, 1)
                } else {
                    content
                }
            } else {
                let start = typed.start_line.unwrap_or(1) as usize;
                let end = typed.end_line.unwrap_or(u32::MAX) as usize;
                let content = read_text_range(&resolved, start, end, self.limits.max_scan_bytes)?;
                if show_line_numbers {
                    format_with_line_numbers(&content, start)
                } else {
                    content
                }
            };

            // Update file cache with SHA-256 for stale-file protection in apply_patch.
            // Optimization: skip hashing for range reads on large files (> 10MB) to avoid
            // expensive O(file_size) work. Full-file reads and smaller files are always hashed.
            let is_range_read = typed.start_line.is_some() || typed.end_line.is_some();
            let should_hash = !is_range_read || meta.len() <= HASH_THRESHOLD_BYTES;

            if should_hash && let Ok(sha) = compute_sha256(&resolved) {
                let mut cache = ctx.file_cache.lock().await;
                cache.insert(
                    resolved.clone(),
                    FileCacheEntry {
                        sha256: sha,
                        read_at: SystemTime::now(),
                    },
                );
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
                "patch": { "type": "string" }
            },
            "required": ["patch"]
        })
    }

    fn is_side_effecting(&self) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: ApplyPatchArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
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
            let typed: ApplyPatchArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;
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
            // Human-visible diff (unified-diff-style) derived from the LP1 ops.
            // We only include diffs for files that actually changed on disk.
            let mut diff_sections: Vec<String> = Vec::new();
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
                        cache.get(&resolved).cloned()
                    };
                    let Some(entry) = entry else {
                        return Err(ToolError::StaleFile {
                            file: resolved.clone(),
                            reason: "File was not read before editing, or has been modified since last read".to_string(),
                        });
                    };

                    // Hash the already-read bytes to avoid TOCTOU race
                    let current_sha = compute_sha256_bytes(&original_bytes);
                    if current_sha != entry.sha256 {
                        return Err(ToolError::StaleFile {
                            file: resolved.clone(),
                            reason: "File content changed since last read".to_string(),
                        });
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

                if changed {
                    let diff =
                        format_unified_diff(&file_patch.path, &original_bytes, &new_bytes, existed);
                    if !diff.is_empty() {
                        diff_sections.push(diff);
                    }
                }

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
            let changed_count = staged.iter().filter(|s| s.changed).count();

            if any_changed {
                apply_staged_files(&staged)?;
                for file in &staged {
                    if !file.changed {
                        continue;
                    }
                    if file.existed {
                        ctx.turn_changes.record_modified(file.path.clone());
                    } else {
                        ctx.turn_changes.record_created(file.path.clone());
                    }
                    // Record diff stats for the turn Distillate
                    let (additions, deletions) =
                        compute_diff_stats(&file.original_bytes, &file.bytes);
                    ctx.turn_changes
                        .record_stats(file.path.clone(), additions, deletions);

                    // Update file cache so subsequent edits don't fail staleness check
                    let sha = compute_sha256_bytes(&file.bytes);
                    let mut cache = ctx.file_cache.lock().await;
                    cache.insert(
                        file.path.clone(),
                        FileCacheEntry {
                            sha256: sha,
                            read_at: SystemTime::now(),
                        },
                    );
                }
            }

            // Build output: skip Distillate for single-file edits (redundant with tool header)
            let output = if !any_changed {
                "No changes applied.".to_string()
            } else if changed_count == 1 && !diff_sections.is_empty() {
                // Single file: just show the diff
                diff_sections.join("\n\n")
            } else {
                // Multiple files: show Distillate then diffs
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
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self) -> bool {
        true
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: WriteFileArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
        let distillate = format!(
            "Write new file: {} ({} bytes)",
            typed.path,
            typed.content.len()
        );
        Ok(redact_distillate(&distillate))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let typed: WriteFileArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;

            if typed.path.trim().is_empty() {
                return Err(ToolError::BadArgs {
                    message: "path must not be empty".to_string(),
                });
            }

            // Use resolve_path_for_create to allow creating files in new directories
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
            }

            let bytes = typed.content.as_bytes();
            let mut file = match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&resolved)
                .await
            {
                Ok(f) => f,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!(
                            "file already exists: {}. Use apply_patch to modify existing files.",
                            resolved.display()
                        ),
                    });
                }
                Err(err) => {
                    return Err(ToolError::ExecutionFailed {
                        tool: "Write".to_string(),
                        message: format!("failed to create {}: {err}", resolved.display()),
                    });
                }
            };

            file.write_all(bytes)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "Write".to_string(),
                    message: format!("failed to write {}: {e}", resolved.display()),
                })?;
            file.flush().await.map_err(|e| ToolError::ExecutionFailed {
                tool: "Write".to_string(),
                message: format!("failed to flush {}: {e}", resolved.display()),
            })?;

            if let Ok(sha) = compute_sha256(&resolved) {
                let mut cache = ctx.file_cache.lock().await;
                cache.insert(
                    resolved.clone(),
                    FileCacheEntry {
                        sha256: sha,
                        read_at: SystemTime::now(),
                    },
                );
            }

            ctx.turn_changes.record_created(resolved.clone());

            // Record stats: all lines are additions for a new file
            // FIXME: use bytecount crate if this becomes a hot path
            #[allow(clippy::naive_bytecount)]
            let line_count = bytes.iter().filter(|&&b| b == b'\n').count() as u32
                + u32::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
            ctx.turn_changes
                .record_stats(resolved.clone(), line_count, 0);

            let output = format!(
                "Created {} ({} bytes)",
                display_path(&resolved),
                bytes.len()
            );
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
                "command": { "type": "string" },
                "reason": { "type": "string" },
                "unsafe_allow_unsandboxed": { "type": "boolean" }
            },
            "required": ["command"]
        })
    }

    fn is_side_effecting(&self) -> bool {
        true
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn risk_level(&self) -> super::RiskLevel {
        super::RiskLevel::High
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: RunCommandArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
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
            let typed: RunCommandArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;
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
            )?;

            let mut command = Command::new(&self.shell.binary);
            for arg in &self.shell.args {
                command.arg(arg);
            }
            command.arg(prepared.command());
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .current_dir(&ctx.working_dir);

            let env: Vec<(String, String)> = std::env::vars().collect();
            let sanitized = ctx.env_sanitizer.sanitize_env(&env);
            command.env_clear();
            command.envs(sanitized);

            let requires_host_sandbox = prepared.requires_windows_host_sandbox();

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
                use std::os::unix::process::CommandExt;
                unsafe {
                    command.as_std_mut().pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }

            let child = command.spawn().map_err(|e| ToolError::ExecutionFailed {
                tool: "Run".to_string(),
                message: e.to_string(),
            })?;

            let mut guard = ChildGuard::new(child);

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
                None
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
    git::register_git_tools(registry)?;
    for name in SearchTool::aliases() {
        registry.register(Box::new(SearchTool::with_name(name, search_config.clone())))?;
    }
    registry.register(Box::new(WebFetchTool::new(webfetch_config)))?;
    registry.register(Box::new(RecallTool))?;
    registry.register(Box::new(MemoryTool))?;
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

fn compute_sha256(path: &Path) -> Result<[u8; 32], std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    Ok(out)
}

fn compute_sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    out
}

struct StagedFile {
    path: PathBuf,
    existed: bool,
    changed: bool,
    bytes: Vec<u8>,
    original_bytes: Vec<u8>,
    permissions: Option<std::fs::Permissions>,
}

struct PreparedFile {
    target: PathBuf,
    temp: PathBuf,
    backup: Option<PathBuf>,
    existed: bool,
}

fn unique_backup_path(target: &Path) -> Result<PathBuf, ToolError> {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for attempt in 0..1000 {
        let suffix = format!("forge_patch_bak_{stamp}_{attempt}");
        let candidate = target.with_extension(suffix);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ToolError::PatchFailed {
        file: target.to_path_buf(),
        message: "Failed to allocate backup path".to_string(),
    })
}

fn apply_staged_files(staged: &[StagedFile]) -> Result<(), ToolError> {
    if staged.is_empty() {
        return Ok(());
    }

    let mut prepared: Vec<PreparedFile> = Vec::new();
    for file in staged.iter().filter(|s| s.changed) {
        let parent = file.path.parent().ok_or_else(|| ToolError::PatchFailed {
            file: file.path.clone(),
            message: "Invalid path".to_string(),
        })?;
        let temp = tempfile::Builder::new()
            .prefix("forge_patch_")
            .tempfile_in(parent)
            .map_err(|e| ToolError::PatchFailed {
                file: file.path.clone(),
                message: e.to_string(),
            })?;
        std::fs::write(temp.path(), &file.bytes).map_err(|e| ToolError::PatchFailed {
            file: file.path.clone(),
            message: e.to_string(),
        })?;
        if let Some(perms) = &file.permissions {
            std::fs::set_permissions(temp.path(), perms.clone()).map_err(|e| {
                ToolError::PatchFailed {
                    file: file.path.clone(),
                    message: e.to_string(),
                }
            })?;
        }
        let temp_path = temp
            .into_temp_path()
            .keep()
            .map_err(|e| ToolError::PatchFailed {
                file: file.path.clone(),
                message: e.to_string(),
            })?;
        let backup_path = if file.existed {
            Some(unique_backup_path(&file.path)?)
        } else {
            None
        };
        prepared.push(PreparedFile {
            target: file.path.clone(),
            temp: temp_path,
            backup: backup_path,
            existed: file.existed,
        });
    }

    let mut backed_up: Vec<&PreparedFile> = Vec::new();
    for entry in &prepared {
        let Some(backup) = &entry.backup else {
            continue;
        };
        if let Err(e) = std::fs::rename(&entry.target, backup) {
            for restored in backed_up {
                let Some(backup) = &restored.backup else {
                    continue;
                };
                let _ = std::fs::remove_file(&restored.target);
                let _ = std::fs::rename(backup, &restored.target);
            }
            for cleanup in &prepared {
                let _ = std::fs::remove_file(&cleanup.temp);
            }
            return Err(ToolError::PatchFailed {
                file: entry.target.clone(),
                message: format!("Failed to backup original: {e}"),
            });
        }
        backed_up.push(entry);
    }

    for entry in &prepared {
        if let Err(e) = std::fs::rename(&entry.temp, &entry.target) {
            for restore in &prepared {
                if let Some(backup) = &restore.backup {
                    if backup.exists() {
                        let _ = std::fs::remove_file(&restore.target);
                        let _ = std::fs::rename(backup, &restore.target);
                    }
                } else if !restore.existed {
                    let _ = std::fs::remove_file(&restore.target);
                }
            }
            for cleanup in &prepared {
                let _ = std::fs::remove_file(&cleanup.temp);
            }
            return Err(ToolError::PatchFailed {
                file: entry.target.clone(),
                message: format!("Failed to apply patch: {e}"),
            });
        }
    }

    for entry in &prepared {
        if let Some(backup) = &entry.backup {
            let _ = std::fs::remove_file(backup);
        }
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

struct ChildGuard {
    child: Option<tokio::process::Child>,
}

impl ChildGuard {
    fn new(child: tokio::process::Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child.as_mut().expect("child present")
    }

    fn disarm(&mut self) {
        self.child = None;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                unsafe {
                    if libc::killpg(pid as i32, libc::SIGKILL) == -1 {
                        let _ = child.start_kill();
                    }
                }
            }
            // Reap the zombie process synchronously to prevent zombie accumulation.
            // This is best-effort - if it fails, the process table entry will be
            // cleaned up when the parent exits.
            let _ = child.try_wait();
        }
        #[cfg(windows)]
        {
            let _ = child.start_kill();
            // Windows doesn't have the same zombie issue, but try_wait is good practice
            let _ = child.try_wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{DetectedShell, RunSandboxPolicy};

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
        assert!(!tool.is_side_effecting());
    }

    #[test]
    fn glob_tool_does_not_require_approval() {
        let tool = GlobTool;
        assert!(!tool.requires_approval());
    }

    #[test]
    fn glob_tool_risk_level_is_low() {
        let tool = GlobTool;
        assert_eq!(tool.risk_level(), RiskLevel::Low);
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
}

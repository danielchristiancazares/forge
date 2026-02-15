//! Tool result distillation and render decision logic.
//!
//! Determines whether tool results should be rendered in full or as compact summarys.
//! The decision depends on:
//!
//! - **Tool type**: `Edit` and `Write` always render full (never Distilled)
//! - **Content analysis**: Diff-like content (with `@@`, `---`, `+++`) renders full
//! - **Tool-specific parsing**: Each tool type has custom Summary logic
//!
//! # Summary Formats
//!
//! | Tool | Format |
//! |------|--------|
//! | Read | "42 lines" or "lines 1-50" |
//! | Search | "3 matches in 2 files" |
//! | Glob | "5 files" |
//! | Run/Pwsh | "exit 0: first line" |
//! | Git:status | "1 staged, 2 modified" |

use std::collections::HashSet;

use forge_types::ToolCall;
use serde::Deserialize;
use serde_json::Value;

use crate::tool_display::canonical_tool_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolKind {
    Read,
    Search,
    Glob,
    Shell,
    Edit,
    Write,
    GitStatus,
    GitCommit,
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolCallMeta {
    pub(crate) kind: ToolKind,
    read_range: Option<ReadRange>,
}

#[derive(Debug, Clone, Copy)]
struct ReadRange {
    start_line: Option<u64>,
    end_line: Option<u64>,
}

impl ToolCallMeta {
    pub(crate) fn from_call(call: &ToolCall) -> Self {
        let canonical = canonical_tool_name(&call.name);
        let kind = if canonical == "Git" {
            match call.arguments.get("command").and_then(|v| v.as_str()) {
                Some("status") => ToolKind::GitStatus,
                Some("commit") => ToolKind::GitCommit,
                _ => ToolKind::Other,
            }
        } else {
            ToolKind::from_canonical(canonical.as_ref())
        };
        let read_range = if matches!(kind, ToolKind::Read) {
            extract_read_range(&call.arguments)
        } else {
            None
        };

        Self { kind, read_range }
    }
}

impl ToolKind {
    fn from_canonical(name: &str) -> Self {
        match name {
            "Read" => ToolKind::Read,
            "Search" => ToolKind::Search,
            "Glob" => ToolKind::Glob,
            "Run" | "Pwsh" => ToolKind::Shell,
            "Edit" => ToolKind::Edit,
            "Write" => ToolKind::Write,
            _ => ToolKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolResultRender {
    /// Show full output (never distill).
    Full {
        /// Apply diff-aware styling (colored +/- lines).
        diff_aware: bool,
    },
    /// Show compact Summary line.
    Summary(String),
}

/// Write/Edit always return `Full` (never Distilled).
/// Other tools may return `Summary` or `Full` based on content.
pub(crate) fn tool_result_render_decision(
    tool_meta: Option<&ToolCallMeta>,
    content: &str,
    is_error: bool,
    max_width: usize,
) -> ToolResultRender {
    let kind = tool_meta.map(|m| m.kind);

    // Invariant: Write and Edit are NEVER Distilled
    match kind {
        Some(ToolKind::Edit) => return ToolResultRender::Full { diff_aware: true },
        Some(ToolKind::Write) => return ToolResultRender::Full { diff_aware: false },
        _ => {}
    }

    // For other tools, check if content looks like a diff
    if looks_like_diff(content) {
        return ToolResultRender::Full { diff_aware: true };
    }

    // Otherwise, generate tool-specific Summary
    let summary = format_tool_result_summary(tool_meta, content, is_error, max_width);
    ToolResultRender::Summary(summary)
}

fn looks_like_diff(content: &str) -> bool {
    content.lines().take(10).any(|line| {
        line.starts_with("diff --git")
            || line.starts_with("@@")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
    })
}

pub(crate) fn format_tool_result_summary(
    tool_call_meta: Option<&ToolCallMeta>,
    content: &str,
    is_error: bool,
    max_width: usize,
) -> String {
    if is_error {
        return truncate_to(first_line(content).unwrap_or_default(), max_width);
    }

    match tool_call_meta.map(|meta| meta.kind) {
        Some(ToolKind::Read) => {
            distill_read(tool_call_meta.and_then(|meta| meta.read_range), content)
        }
        Some(ToolKind::Search) => {
            distill_search(content).unwrap_or_else(|| distill_generic(content, max_width))
        }
        Some(ToolKind::Glob) => distill_glob(content),
        Some(ToolKind::Shell) => distill_shell(content, is_error, max_width),
        Some(ToolKind::GitStatus) => {
            distill_git_status(content).unwrap_or_else(|| distill_generic(content, max_width))
        }
        Some(ToolKind::GitCommit) => distill_git_commit(content, max_width)
            .unwrap_or_else(|| distill_generic(content, max_width)),
        _ => distill_generic(content, max_width),
    }
}

fn distill_read(range: Option<ReadRange>, content: &str) -> String {
    if let Some(summary) = range.and_then(format_read_range) {
        return summary;
    }

    let line_count = content.lines().count();
    if line_count == 1 {
        "1 line".to_string()
    } else {
        format!("{line_count} lines")
    }
}

fn distill_search(content: &str) -> Option<String> {
    let (match_count, file_count) = parse_search_counts(content)?;
    let match_label = pluralize(match_count, "match", "matches");
    if file_count > 0 {
        let file_label = pluralize(file_count, "file", "files");
        return Some(format!(
            "{match_count} {match_label} in {file_count} {file_label}"
        ));
    }

    Some(format!("{match_count} {match_label}"))
}

fn distill_glob(content: &str) -> String {
    let count = parse_glob_count(content).unwrap_or_else(|| count_non_empty_lines(content));
    let label = pluralize(count, "file", "files");
    format!("{count} {label}")
}

fn distill_shell(content: &str, is_error: bool, max_width: usize) -> String {
    let (stdout, exit_code) = parse_command_output(content);
    let first = first_non_empty_line(stdout.as_deref().unwrap_or(content));
    let fallback_line = first_line(content);
    let exit_code = exit_code.or_else(|| extract_exit_code_from_text(content));

    let summary = if is_error {
        if let Some(code) = exit_code {
            match first.or(fallback_line) {
                Some(line) if !line.is_empty() && !is_exit_code_message(line) => {
                    format!("exit {code}: {line}")
                }
                _ => format!("exit {code}"),
            }
        } else {
            truncate_to(fallback_line.unwrap_or_default(), max_width)
        }
    } else {
        let code = exit_code.unwrap_or(0);
        match first.or(fallback_line) {
            Some(line) if !line.is_empty() => format!("exit {code}: {line}"),
            _ => format!("exit {code}"),
        }
    };

    truncate_to(&summary, max_width)
}

fn distill_git_commit(content: &str, max_width: usize) -> Option<String> {
    let value: Value = serde_json::from_str(content).ok()?;
    let commit_msg = value.get("commit_message").and_then(Value::as_str)?;
    let subject = commit_msg.lines().next().unwrap_or(commit_msg);

    let summary = match value.get("commit_hash").and_then(Value::as_str) {
        Some(h) => {
            let short = if h.len() > 7 { &h[..7] } else { h };
            format!("{short} {subject}")
        }
        None => format!("failed: {subject}"),
    };

    Some(truncate_to(&summary, max_width))
}

fn distill_git_status(content: &str) -> Option<String> {
    let stdout = parse_stdout_from_json(content).unwrap_or_else(|| content.to_string());
    let counts = parse_git_porcelain_counts(&stdout)?;
    Some(render_git_status_counts(counts))
}

fn distill_generic(content: &str, max_width: usize) -> String {
    let line_count = content.lines().count();
    if line_count <= 1 {
        truncate_to(first_line(content).unwrap_or_default(), max_width)
    } else {
        format!("{line_count} lines")
    }
}

fn format_read_range(range: ReadRange) -> Option<String> {
    let start = range.start_line;
    let end = range.end_line;

    match (start, end) {
        (Some(start), Some(end)) if start == end => Some(format!("line {start}")),
        (Some(start), Some(end)) => Some(format!("lines {start}-{end}")),
        (Some(start), None) => Some(format!("from line {start}")),
        (None, Some(end)) => Some(format!("up to line {end}")),
        _ => None,
    }
}

fn extract_read_range(args: &Value) -> Option<ReadRange> {
    let obj = args.as_object()?;
    let start_line = obj.get("start_line").and_then(parse_line_number);
    let end_line = obj.get("end_line").and_then(parse_line_number);

    if start_line.is_none() && end_line.is_none() {
        return None;
    }

    Some(ReadRange {
        start_line,
        end_line,
    })
}

fn parse_line_number(value: &Value) -> Option<u64> {
    let number = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }?;

    if number == 0 { None } else { Some(number) }
}

fn parse_search_counts(content: &str) -> Option<(usize, usize)> {
    let value: Value = serde_json::from_str(content).ok()?;
    let matches_value = value.get("matches").and_then(Value::as_array);
    let match_count = value
        .get("count")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
        .or_else(|| matches_value.map(Vec::len))?;

    let file_count = matches_value
        .map(|matches| {
            let mut files = HashSet::new();
            for entry in matches {
                if let Some(path) = extract_path(entry) {
                    files.insert(path);
                }
            }
            files.len()
        })
        .unwrap_or_default();

    Some((match_count, file_count))
}

fn extract_path(value: &Value) -> Option<String> {
    if let Some(obj) = value.as_object() {
        if let Some(data) = obj.get("data")
            && let Some(path) = extract_path(data)
        {
            return Some(path);
        }

        if let Some(path) = obj.get("path")
            && let Some(path) = extract_path(path)
        {
            return Some(path);
        }

        if let Some(text) = obj.get("text").and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }

    value.as_str().map(str::to_string)
}

fn parse_glob_count(content: &str) -> Option<usize> {
    let value: Value = serde_json::from_str(content).ok()?;
    if let Some(array) = value.as_array() {
        return Some(array.len());
    }

    let obj = value.as_object()?;
    for key in ["paths", "matches", "results", "files"] {
        if let Some(array) = obj.get(key).and_then(Value::as_array) {
            return Some(array.len());
        }
    }

    None
}

fn parse_command_output(content: &str) -> (Option<String>, Option<i64>) {
    let parsed: CommandOutput = match serde_json::from_str(content) {
        Ok(parsed) => parsed,
        Err(_) => return (None, None),
    };

    (parsed.stdout_or_output(), parsed.exit_code())
}

fn parse_stdout_from_json(content: &str) -> Option<String> {
    let parsed: CommandOutput = serde_json::from_str(content).ok()?;
    parsed.stdout_or_output()
}

fn extract_exit_code_from_text(content: &str) -> Option<i64> {
    let lower = content.to_lowercase();
    let marker = "exit code";
    let start = lower.find(marker)? + marker.len();
    let mut digits = String::new();

    for ch in content[start..].chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }

    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn is_exit_code_message(line: &str) -> bool {
    line.trim_start()
        .to_ascii_lowercase()
        .starts_with("exit code")
}

#[derive(Debug, Default, Clone, Copy)]
struct GitStatusCounts {
    staged: usize,
    modified: usize,
    untracked: usize,
    conflicted: usize,
}

fn parse_git_porcelain_counts(content: &str) -> Option<GitStatusCounts> {
    let mut counts = GitStatusCounts::default();
    let mut has_entries = false;

    for line in content.lines() {
        if line.starts_with("##") || line.starts_with("!!") {
            continue;
        }
        if line.len() < 2 {
            continue;
        }
        let mut chars = line.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');

        if x == '?' && y == '?' {
            counts.untracked += 1;
            has_entries = true;
            continue;
        }

        if x == 'U' || y == 'U' {
            counts.conflicted += 1;
        }

        if x != ' ' {
            counts.staged += 1;
        }

        if y != ' ' {
            counts.modified += 1;
        }

        if x != ' ' || y != ' ' {
            has_entries = true;
        }
    }

    if has_entries { Some(counts) } else { None }
}

fn render_git_status_counts(counts: GitStatusCounts) -> String {
    let mut parts = Vec::new();

    if counts.staged > 0 {
        let staged = counts.staged;
        let label = pluralize(staged, "staged", "staged");
        parts.push(format!("{staged} {label}"));
    }

    if counts.modified > 0 {
        let modified = counts.modified;
        let label = pluralize(modified, "modified", "modified");
        parts.push(format!("{modified} {label}"));
    }

    if counts.untracked > 0 {
        let untracked = counts.untracked;
        let label = pluralize(untracked, "untracked", "untracked");
        parts.push(format!("{untracked} {label}"));
    }

    if counts.conflicted > 0 {
        let conflicted = counts.conflicted;
        let label = pluralize(conflicted, "conflict", "conflicts");
        parts.push(format!("{conflicted} {label}"));
    }

    if parts.is_empty() {
        "clean".to_string()
    } else {
        parts.join(", ")
    }
}

fn first_line(content: &str) -> Option<&str> {
    content.lines().next()
}

fn first_non_empty_line(content: &str) -> Option<&str> {
    content.lines().find(|line| !line.trim().is_empty())
}

fn count_non_empty_lines(content: &str) -> usize {
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn truncate_to(content: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    content.chars().take(max_width).collect()
}

#[derive(Debug, Deserialize)]
struct CommandOutput {
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    exit_code: Option<i64>,
    #[serde(default, rename = "exitCode")]
    exit_code_camel: Option<i64>,
    #[serde(default)]
    status: Option<i64>,
}

impl CommandOutput {
    fn stdout_or_output(&self) -> Option<String> {
        self.stdout.clone().or_else(|| self.output.clone())
    }

    fn exit_code(&self) -> Option<i64> {
        self.exit_code.or(self.exit_code_camel).or(self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_read_range_prefers_call_args() {
        let call = ToolCall::new(
            "call_1",
            "Read",
            json!({"path": "src/lib.rs", "start_line": 1, "end_line": 50}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let summary = format_tool_result_summary(Some(&meta), "", false, 80);
        assert_eq!(summary, "lines 1-50");
    }

    #[test]
    fn summary_search_counts_matches_and_files() {
        let call = ToolCall::new("call_2", "Search", json!({"pattern": "foo"}));
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"count":3,"matches":[{"type":"match","data":{"path":{"text":"a.rs"}}},{"type":"context","data":{"path":{"text":"b.rs"}}},{"type":"match","data":{"path":{"text":"a.rs"}}}]}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "3 matches in 2 files");
    }

    #[test]
    fn summary_glob_counts_files() {
        let call = ToolCall::new("call_3", "Glob", json!({"pattern": "*.rs"}));
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"["a.rs","b.rs"]"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "2 files");
    }

    #[test]
    fn summary_bash_reports_exit_and_first_line() {
        let call = ToolCall::new("call_4", "Run", json!({"command": "echo hi"}));
        let meta = ToolCallMeta::from_call(&call);
        let summary = format_tool_result_summary(Some(&meta), "hello\nworld", false, 80);
        assert_eq!(summary, "exit 0: hello");
    }

    #[test]
    fn summary_git_status_counts_porcelain() {
        let call = ToolCall::new(
            "call_5",
            "Git",
            json!({"command": "status", "porcelain": true}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"stdout":" M file1\nA  file2\n?? file3\n"}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "1 staged, 1 modified, 1 untracked");
    }

    // --- Render decision tests ---

    #[test]
    fn edit_is_always_full_even_without_diff_markers() {
        let call = ToolCall::new("call_1", "Edit", json!({}));
        let meta = ToolCallMeta::from_call(&call);
        // Content has no diff markers (Summary lines only)
        let content = "modified: a\nmodified: b\n";
        let result = tool_result_render_decision(Some(&meta), content, false, 80);
        assert!(matches!(
            result,
            ToolResultRender::Full { diff_aware: true }
        ));
    }

    #[test]
    fn write_is_always_full() {
        let call = ToolCall::new("call_2", "Write", json!({}));
        let meta = ToolCallMeta::from_call(&call);
        let content = "Created /path/to/file.rs (1024 bytes)";
        let result = tool_result_render_decision(Some(&meta), content, false, 80);
        assert!(matches!(
            result,
            ToolResultRender::Full { diff_aware: false }
        ));
    }

    #[test]
    fn other_tools_with_diff_content_get_full_diff_aware() {
        let call = ToolCall::new("call_3", "Run", json!({}));
        let meta = ToolCallMeta::from_call(&call);
        let content = "--- old\n+++ new\n@@ -1 +1 @@\n-foo\n+bar";
        let result = tool_result_render_decision(Some(&meta), content, false, 80);
        assert!(matches!(
            result,
            ToolResultRender::Full { diff_aware: true }
        ));
    }

    #[test]
    fn other_tools_without_diff_get_summary() {
        let call = ToolCall::new("call_4", "Run", json!({}));
        let meta = ToolCallMeta::from_call(&call);
        let content = "hello\nworld";
        let result = tool_result_render_decision(Some(&meta), content, false, 80);
        assert!(matches!(result, ToolResultRender::Summary(_)));
    }

    #[test]
    fn edit_with_many_files_still_gets_full_output() {
        let call = ToolCall::new("call_5", "Edit", json!({}));
        let meta = ToolCallMeta::from_call(&call);
        // 15 modified lines before diff markers (exceeds 10-line heuristic window)
        let content = "modified: f1\nmodified: f2\nmodified: f3\nmodified: f4\n\
                       modified: f5\nmodified: f6\nmodified: f7\nmodified: f8\n\
                       modified: f9\nmodified: f10\nmodified: f11\nmodified: f12\n\
                       modified: f13\nmodified: f14\nmodified: f15\n\n\
                       --- old\n+++ new\n@@ -1 +1 @@\n-foo\n+bar";
        let result = tool_result_render_decision(Some(&meta), content, false, 80);
        // Even though diff markers are past line 10, Edit always gets Full
        assert!(matches!(
            result,
            ToolResultRender::Full { diff_aware: true }
        ));
    }

    #[test]
    fn summary_git_commit_shows_hash_and_message() {
        let call = ToolCall::new(
            "call_6",
            "Git",
            json!({"command": "commit", "type": "feat", "message": "add feature"}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"commit_hash":"abc1234","commit_message":"feat: add feature","exit_code":0,"stdout":"[main abc1234] feat: add feature\n 1 file changed","stderr":"","isError":false}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "abc1234 feat: add feature");
    }

    #[test]
    fn summary_git_commit_truncates_long_hash() {
        let call = ToolCall::new(
            "call_7",
            "Git",
            json!({"command": "commit", "type": "fix", "message": "bug"}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"commit_hash":"abc1234def5678","commit_message":"fix: bug","exit_code":0,"stdout":"","stderr":"","isError":false}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "abc1234 fix: bug");
    }

    #[test]
    fn summary_git_commit_missing_hash_shows_placeholder() {
        let call = ToolCall::new(
            "call_8",
            "Git",
            json!({"command": "commit", "type": "fix", "message": "bug"}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"commit_hash":null,"commit_message":"fix: bug","exit_code":0,"stdout":"","stderr":"","isError":false}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "failed: fix: bug");
    }

    #[test]
    fn summary_git_commit_error_falls_through() {
        let call = ToolCall::new(
            "call_9",
            "Git",
            json!({"command": "commit", "type": "feat", "message": "x"}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = "nothing to commit, working tree clean";
        let summary = format_tool_result_summary(Some(&meta), content, true, 80);
        assert_eq!(summary, "nothing to commit, working tree clean");
    }

    #[test]
    fn summary_git_commit_multiline_uses_subject_only() {
        let call = ToolCall::new(
            "call_10",
            "Git",
            json!({"command": "commit", "type": "refactor", "scope": "docs", "message": "update documentation"}),
        );
        let meta = ToolCallMeta::from_call(&call);
        let content = r#"{"commit_hash":"9562496","commit_message":"refactor(docs): update documentation\n\nThis commit contains:\n- Comprehensive rewrite","exit_code":0,"stdout":"","stderr":"","isError":false}"#;
        let summary = format_tool_result_summary(Some(&meta), content, false, 80);
        assert_eq!(summary, "9562496 refactor(docs): update documentation");
    }
}

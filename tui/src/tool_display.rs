//! Compact tool call display formatting.
//!
//! Converts tool calls from verbose JSON to function-call style:
//! `Search("pattern value")` instead of multi-line JSON.

use serde_json::Value;

/// Format a tool call in compact function-call style.
///
/// Returns format like `Read(src/main.rs)` or just `GitStatus` for tools without
/// a displayable primary argument.
pub fn format_tool_call_compact(name: &str, args: &Value) -> String {
    let display_name = canonical_tool_name(name);

    // Special case: Edit displays as Edit(path) showing patch file Summary
    if name == "Edit" {
        if let Some(obj) = args.as_object()
            && let Some(path) = format_patch_summary(obj)
        {
            return format!("Edit({})", truncate(&path, 60));
        }
        return "Edit".to_string();
    }

    match extract_primary_arg(name, args) {
        Some(val) => format!("{}({})", display_name, truncate(&val, 60)),
        None => display_name.to_string(),
    }
}

pub(crate) fn canonical_tool_name(name: &str) -> std::borrow::Cow<'static, str> {
    use std::borrow::Cow;

    match name {
        // File tools
        "Read" => Cow::Borrowed("Read"),
        "Write" => Cow::Borrowed("Write"),
        "Edit" => Cow::Borrowed("Edit"),
        "Delete" => Cow::Borrowed("Delete"),
        "Move" => Cow::Borrowed("Move"),
        "Copy" => Cow::Borrowed("Copy"),
        "ListDir" => Cow::Borrowed("ListDir"),
        "Outline" => Cow::Borrowed("Outline"),

        // Search tools
        "Glob" => Cow::Borrowed("Glob"),
        "Search" => Cow::Borrowed("Search"),

        // Git tools
        "GitStatus" => Cow::Borrowed("GitStatus"),
        "GitDiff" => Cow::Borrowed("GitDiff"),
        "GitAdd" => Cow::Borrowed("GitAdd"),
        "GitCommit" => Cow::Borrowed("GitCommit"),
        "GitStash" => Cow::Borrowed("GitStash"),
        "GitRestore" => Cow::Borrowed("GitRestore"),
        "GitBranch" => Cow::Borrowed("GitBranch"),
        "GitCheckout" => Cow::Borrowed("GitCheckout"),
        "GitShow" => Cow::Borrowed("GitShow"),
        "GitLog" => Cow::Borrowed("GitLog"),
        "GitBlame" => Cow::Borrowed("GitBlame"),

        // Shell/command tools
        "Pwsh" => Cow::Borrowed("Pwsh"),
        "Run" => Cow::Borrowed("Run"),

        // Web tools
        "WebFetch" => Cow::Borrowed("WebFetch"),

        // Memory tools
        "Recall" => Cow::Borrowed("Recall"),

        // Build tools
        "Build" => Cow::Borrowed("Build"),
        "Test" => Cow::Borrowed("Test"),

        // Pass through unknown tools as-is
        _ => Cow::Owned(name.to_string()),
    }
}

/// Extract the primary displayable argument based on tool name.
fn extract_primary_arg(name: &str, args: &Value) -> Option<String> {
    let obj = args.as_object()?;

    let key = match name {
        "Glob" | "Search" => "pattern",
        "Read" | "Write" | "Edit" | "Delete" | "ListDir" | "Outline" | "GitBlame" => "path",
        "Move" | "Copy" => "source",
        "GitCommit" => return format_git_commit(obj),
        "GitDiff" => return format_git_diff(obj),
        "GitAdd" => return format_git_add(obj),
        "GitStash" => return format_git_stash(obj),
        "GitRestore" => return format_git_restore(obj),
        "GitBranch" => return format_git_branch(obj),
        "GitCheckout" => return format_git_checkout(obj),
        "GitShow" => return format_git_show(obj),
        "GitLog" => return format_git_log(obj),
        "GitStatus" => return None,
        "Pwsh" | "Run" => "command",
        "WebFetch" => "url",
        "Build" | "Test" => return format_build_test(obj),
        _ => return try_common_keys(obj),
    };

    obj.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Format git commit: `type(scope): message` or `type: message`.
fn format_git_commit(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let commit_type = obj.get("type")?.as_str()?;
    let message = obj.get("message")?.as_str()?;

    let result = if let Some(scope) = obj.get("scope").and_then(|v| v.as_str()) {
        format!("{commit_type}({scope}): {message}")
    } else {
        format!("{commit_type}: {message}")
    };
    Some(result)
}

/// Format git diff: `from..to` or flags/paths.
fn format_git_diff(obj: &serde_json::Map<String, Value>) -> Option<String> {
    // Check for ref-to-ref comparison
    if let (Some(from), Some(to)) = (
        obj.get("from_ref").and_then(|v| v.as_str()),
        obj.get("to_ref").and_then(|v| v.as_str()),
    ) {
        return Some(format!("{from}..{to}"));
    }

    // Check for flags
    let mut parts = Vec::new();
    if obj.get("cached").and_then(serde_json::Value::as_bool) == Some(true) {
        parts.push("--cached");
    }
    if obj.get("stat").and_then(serde_json::Value::as_bool) == Some(true) {
        parts.push("--stat");
    }
    if obj.get("name_only").and_then(serde_json::Value::as_bool) == Some(true) {
        parts.push("--name-only");
    }

    if !parts.is_empty() {
        return Some(parts.join(" "));
    }

    // Check for paths
    if let Some(paths) = obj.get("paths").and_then(|v| v.as_array()) {
        let path_strs: Vec<&str> = paths.iter().filter_map(|p| p.as_str()).collect();
        if !path_strs.is_empty() {
            return Some(format!("{} file(s)", path_strs.len()));
        }
    }

    None
}

/// Format git add: `-A`, `-u`, or file count.
fn format_git_add(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if obj.get("all").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-A".to_string());
    }
    if obj.get("update").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-u".to_string());
    }
    if let Some(paths) = obj.get("paths").and_then(|v| v.as_array())
        && !paths.is_empty()
    {
        return Some(format!("{} file(s)", paths.len()));
    }
    None
}

/// Format git stash: action name.
fn format_git_stash(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("action").and_then(|v| v.as_str()).map(String::from)
}

/// Format git restore: file count.
fn format_git_restore(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(paths) = obj.get("paths").and_then(|v| v.as_array())
        && !paths.is_empty()
    {
        return Some(format!("{} file(s)", paths.len()));
    }
    None
}

/// Format git branch: create/delete/rename info.
fn format_git_branch(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(name) = obj.get("create").and_then(|v| v.as_str()) {
        return Some(format!("create {name}"));
    }
    if let Some(name) = obj.get("delete").and_then(|v| v.as_str()) {
        return Some(format!("delete {name}"));
    }
    if let Some(name) = obj.get("force_delete").and_then(|v| v.as_str()) {
        return Some(format!("delete -D {name}"));
    }
    if let Some(old) = obj.get("rename").and_then(|v| v.as_str())
        && let Some(new) = obj.get("new_name").and_then(|v| v.as_str())
    {
        return Some(format!("{old} -> {new}"));
    }
    // List mode
    if obj.get("list_all").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-a".to_string());
    }
    if obj.get("list_remote").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-r".to_string());
    }
    None
}

/// Format git checkout: branch/commit/paths.
fn format_git_checkout(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(branch) = obj.get("create_branch").and_then(|v| v.as_str()) {
        return Some(format!("-b {branch}"));
    }
    if let Some(branch) = obj.get("branch").and_then(|v| v.as_str()) {
        return Some(branch.to_string());
    }
    if let Some(commit) = obj.get("commit").and_then(|v| v.as_str()) {
        return Some(commit.to_string());
    }
    if let Some(paths) = obj.get("paths").and_then(|v| v.as_array())
        && !paths.is_empty()
    {
        return Some(format!("{} file(s)", paths.len()));
    }
    None
}

/// Format git show: commit ref.
fn format_git_show(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("commit").and_then(|v| v.as_str()).map(String::from)
}

/// Format git log: path or filters.
fn format_git_log(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(path) = obj.get("path").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    if let Some(author) = obj.get("author").and_then(|v| v.as_str()) {
        return Some(format!("--author={author}"));
    }
    if let Some(since) = obj.get("since").and_then(|v| v.as_str()) {
        return Some(format!("--since={since}"));
    }
    if let Some(n) = obj.get("max_count").and_then(serde_json::Value::as_u64) {
        return Some(format!("-{n}"));
    }
    None
}

/// Format `apply_patch`: extract file path(s) from LP1 patch content.
fn format_patch_summary(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(patch) = obj.get("patch").and_then(|v| v.as_str()) {
        // Extract file paths from LP1 format (lines starting with "F ")
        let files: Vec<&str> = patch
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if let Some(path) = trimmed.strip_prefix("F ") {
                    let path = path.trim();
                    // Handle quoted paths
                    if let Some(inner) = path.strip_prefix('"') {
                        // Find closing quote, handle escaped quotes
                        let mut end = 0;
                        let mut chars = inner.chars().peekable();
                        while let Some(c) = chars.next() {
                            if c == '\\' {
                                chars.next(); // skip escaped char
                                end += 2;
                            } else if c == '"' {
                                break;
                            } else {
                                end += c.len_utf8();
                            }
                        }
                        if end > 0 {
                            return Some(&inner[..end]);
                        }
                    }
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        match files.len() {
            0 => None,
            1 => Some(files[0].to_string()),
            n => Some(format!("{n} files")),
        }
    } else {
        // Fallback to path if present
        obj.get("path").and_then(|v| v.as_str()).map(String::from)
    }
}

/// Format build/test: working dir or build system.
fn format_build_test(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(system) = obj.get("build_system").and_then(|v| v.as_str()) {
        return Some(system.to_string());
    }
    if let Some(dir) = obj.get("working_dir").and_then(|v| v.as_str()) {
        return Some(dir.to_string());
    }
    None
}

/// Fallback: try common argument names.
fn try_common_keys(obj: &serde_json::Map<String, Value>) -> Option<String> {
    const COMMON_KEYS: &[&str] = &["pattern", "path", "query", "command", "url", "file", "name"];

    for key in COMMON_KEYS {
        if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(val.to_string());
        }
    }
    None
}

/// Truncate a string to max character count, adding ellipsis if needed.
fn truncate(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }

    // Take max_chars - 1 characters, then add ellipsis
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_search_tool() {
        let args = json!({"pattern": "foo.*bar"});
        assert_eq!(
            format_tool_call_compact("Search", &args),
            "Search(foo.*bar)"
        );
    }

    #[test]
    fn test_glob_tool() {
        let args = json!({"pattern": "**/*.rs", "path": "/src"});
        assert_eq!(format_tool_call_compact("Glob", &args), "Glob(**/*.rs)");
    }

    #[test]
    fn test_read_file() {
        let args = json!({"path": "src/main.rs"});
        assert_eq!(format_tool_call_compact("Read", &args), "Read(src/main.rs)");
    }

    #[test]
    fn test_git_status_no_args() {
        let args = json!({});
        assert_eq!(format_tool_call_compact("GitStatus", &args), "GitStatus");
    }

    #[test]
    fn test_git_commit_with_scope() {
        let args = json!({
            "type": "feat",
            "scope": "tui",
            "message": "add compact display"
        });
        assert_eq!(
            format_tool_call_compact("GitCommit", &args),
            "GitCommit(feat(tui): add compact display)"
        );
    }

    #[test]
    fn test_git_commit_without_scope() {
        let args = json!({
            "type": "fix",
            "message": "resolve bug"
        });
        assert_eq!(
            format_tool_call_compact("GitCommit", &args),
            "GitCommit(fix: resolve bug)"
        );
    }

    #[test]
    fn test_git_diff_refs() {
        let args = json!({
            "from_ref": "main",
            "to_ref": "feature"
        });
        assert_eq!(
            format_tool_call_compact("GitDiff", &args),
            "GitDiff(main..feature)"
        );
    }

    #[test]
    fn test_git_diff_cached() {
        let args = json!({"cached": true});
        assert_eq!(
            format_tool_call_compact("GitDiff", &args),
            "GitDiff(--cached)"
        );
    }

    #[test]
    fn test_git_add_all() {
        let args = json!({"all": true});
        assert_eq!(format_tool_call_compact("GitAdd", &args), "GitAdd(-A)");
    }

    #[test]
    fn test_git_add_files() {
        let args = json!({"paths": ["a.rs", "b.rs", "c.rs"]});
        assert_eq!(
            format_tool_call_compact("GitAdd", &args),
            "GitAdd(3 file(s))"
        );
    }

    #[test]
    fn test_git_stash_action() {
        let args = json!({"action": "pop"});
        assert_eq!(format_tool_call_compact("GitStash", &args), "GitStash(pop)");
    }

    #[test]
    fn test_git_branch_create() {
        let args = json!({"create": "feature-x"});
        assert_eq!(
            format_tool_call_compact("GitBranch", &args),
            "GitBranch(create feature-x)"
        );
    }

    #[test]
    fn test_git_checkout_branch() {
        let args = json!({"branch": "main"});
        assert_eq!(
            format_tool_call_compact("GitCheckout", &args),
            "GitCheckout(main)"
        );
    }

    #[test]
    fn test_webfetch() {
        let args = json!({"url": "https://example.com/api"});
        assert_eq!(
            format_tool_call_compact("WebFetch", &args),
            "WebFetch(https://example.com/api)"
        );
    }

    #[test]
    fn test_pwsh_command() {
        let args = json!({"command": "Get-Process"});
        assert_eq!(format_tool_call_compact("Pwsh", &args), "Pwsh(Get-Process)");
    }

    #[test]
    fn test_unknown_tool_fallback() {
        let args = json!({"query": "SELECT * FROM users"});
        assert_eq!(
            format_tool_call_compact("CustomTool", &args),
            "CustomTool(SELECT * FROM users)"
        );
    }

    #[test]
    fn test_unknown_tool_no_match() {
        let args = json!({"foo": "bar"});
        assert_eq!(format_tool_call_compact("CustomTool", &args), "CustomTool");
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(100);
        let result = truncate(&long, 60);
        assert!(result.chars().count() <= 60);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_utf8_boundary() {
        // "héllo" has multi-byte char
        let s = "héllo world this is a long string";
        let result = truncate(s, 10);
        assert!(result.is_char_boundary(result.len()));
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_empty_args() {
        let args = json!({});
        assert_eq!(format_tool_call_compact("Search", &args), "Search");
    }

    #[test]
    fn test_null_args() {
        let args = Value::Null;
        assert_eq!(format_tool_call_compact("Search", &args), "Search");
    }

    #[test]
    fn test_edit_single_file() {
        let args = json!({"patch": "LP1\nF src/main.rs\nR\nold\n.\nnew\n.\nEND\n"});
        assert_eq!(format_tool_call_compact("Edit", &args), "Edit(src/main.rs)");
    }

    #[test]
    fn test_edit_multiple_files() {
        let args = json!({"patch": "LP1\nF src/a.rs\nR\nold\n.\nnew\n.\nF src/b.rs\nR\nold\n.\nnew\n.\nEND\n"});
        assert_eq!(format_tool_call_compact("Edit", &args), "Edit(2 files)");
    }

    #[test]
    fn test_edit_quoted_path() {
        let args = json!({"patch": "LP1\nF \"path with spaces.rs\"\nT\nhello\n.\nEND\n"});
        assert_eq!(
            format_tool_call_compact("Edit", &args),
            "Edit(path with spaces.rs)"
        );
    }

    #[test]
    fn test_edit_no_files() {
        let args = json!({"patch": "LP1\nEND\n"});
        assert_eq!(format_tool_call_compact("Edit", &args), "Edit");
    }

    #[test]
    fn test_edit_empty_args() {
        let args = json!({});
        assert_eq!(format_tool_call_compact("Edit", &args), "Edit");
    }
}

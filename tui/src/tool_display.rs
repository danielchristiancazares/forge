//! Compact tool call display formatting.
//!
//! Converts tool calls from verbose JSON to function-call style:
//! `Search("pattern value")` instead of multi-line JSON.

use serde_json::Value;

/// Format a tool call in compact function-call style.
///
/// Returns format like `Search("pattern")` or just `git_status` for tools without
/// a displayable primary argument.
pub fn format_tool_call_compact(name: &str, args: &Value) -> String {
    match extract_primary_arg(name, args) {
        Some(val) => format!("{}(\"{}\")", name, truncate(&val, 60)),
        None => name.to_string(),
    }
}

/// Extract the primary displayable argument based on tool name.
fn extract_primary_arg(name: &str, args: &Value) -> Option<String> {
    let obj = args.as_object()?;

    // Tool-specific primary argument mapping
    let key = match name.to_lowercase().as_str() {
        // Search tools
        "glob" | "search" | "rg" | "ripgrep" | "ugrep" | "ug" => "pattern",

        // File tools
        "read" | "read_file" | "readfile" => "path",
        "write" | "write_file" | "writefile" => "path",
        "edit" => "path",
        "delete" => "path",
        "move" => "source",
        "copy" => "source",
        "listdir" => "path",
        "outline" => "path",

        // Git tools with primary args
        "gitblame" | "git_blame" => "path",
        "gitcommit" | "git_commit" => return format_git_commit(obj),
        "gitdiff" | "git_diff" => return format_git_diff(obj),
        "gitadd" | "git_add" => return format_git_add(obj),
        "gitstash" | "git_stash" => return format_git_stash(obj),
        "gitrestore" | "git_restore" => return format_git_restore(obj),
        "gitbranch" | "git_branch" => return format_git_branch(obj),
        "gitcheckout" | "git_checkout" => return format_git_checkout(obj),
        "gitshow" | "git_show" => return format_git_show(obj),
        "gitlog" | "git_log" => return format_git_log(obj),

        // Git tools with no required args - show tool name only
        "gitstatus" | "git_status" => return None,

        // Shell/command tools
        "pwsh" | "bash" | "run_command" | "runcommand" => "command",

        // Web tools
        "webfetch" => "url",

        // Patch tool
        "apply_patch" | "applypatch" => return format_patch_summary(obj),

        // Build/test tools
        "build" | "test" => return format_build_test(obj),

        // Unknown tool - try common keys
        _ => return try_common_keys(obj),
    };

    obj.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Format git commit: `type(scope): message` or `type: message`.
fn format_git_commit(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let commit_type = obj.get("type")?.as_str()?;
    let message = obj.get("message")?.as_str()?;

    let result = if let Some(scope) = obj.get("scope").and_then(|v| v.as_str()) {
        format!("{}({}): {}", commit_type, scope, message)
    } else {
        format!("{}: {}", commit_type, message)
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
        return Some(format!("{}..{}", from, to));
    }

    // Check for flags
    let mut parts = Vec::new();
    if obj.get("cached").and_then(|v| v.as_bool()) == Some(true) {
        parts.push("--cached");
    }
    if obj.get("stat").and_then(|v| v.as_bool()) == Some(true) {
        parts.push("--stat");
    }
    if obj.get("name_only").and_then(|v| v.as_bool()) == Some(true) {
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
    if obj.get("all").and_then(|v| v.as_bool()) == Some(true) {
        return Some("-A".to_string());
    }
    if obj.get("update").and_then(|v| v.as_bool()) == Some(true) {
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
        return Some(format!("create {}", name));
    }
    if let Some(name) = obj.get("delete").and_then(|v| v.as_str()) {
        return Some(format!("delete {}", name));
    }
    if let Some(name) = obj.get("force_delete").and_then(|v| v.as_str()) {
        return Some(format!("delete -D {}", name));
    }
    if let Some(old) = obj.get("rename").and_then(|v| v.as_str())
        && let Some(new) = obj.get("new_name").and_then(|v| v.as_str())
    {
        return Some(format!("{} -> {}", old, new));
    }
    // List mode
    if obj.get("list_all").and_then(|v| v.as_bool()) == Some(true) {
        return Some("-a".to_string());
    }
    if obj.get("list_remote").and_then(|v| v.as_bool()) == Some(true) {
        return Some("-r".to_string());
    }
    None
}

/// Format git checkout: branch/commit/paths.
fn format_git_checkout(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(branch) = obj.get("create_branch").and_then(|v| v.as_str()) {
        return Some(format!("-b {}", branch));
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
        return Some(format!("--author={}", author));
    }
    if let Some(since) = obj.get("since").and_then(|v| v.as_str()) {
        return Some(format!("--since={}", since));
    }
    if let Some(n) = obj.get("max_count").and_then(|v| v.as_u64()) {
        return Some(format!("-{}", n));
    }
    None
}

/// Format apply_patch: file count from patch content.
fn format_patch_summary(obj: &serde_json::Map<String, Value>) -> Option<String> {
    // Try to count files from patch content
    if let Some(patch) = obj.get("patch").and_then(|v| v.as_str()) {
        let file_count = patch.matches("--- a/").count();
        if file_count > 0 {
            return Some(format!("{} file(s)", file_count));
        }
    }
    // Fallback to path if present
    obj.get("path").and_then(|v| v.as_str()).map(String::from)
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
    format!("{}…", truncated)
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
            r#"Search("foo.*bar")"#
        );
    }

    #[test]
    fn test_glob_tool() {
        let args = json!({"pattern": "**/*.rs", "path": "/src"});
        assert_eq!(
            format_tool_call_compact("Glob", &args),
            r#"Glob("**/*.rs")"#
        );
    }

    #[test]
    fn test_read_file() {
        let args = json!({"path": "src/main.rs"});
        assert_eq!(
            format_tool_call_compact("Read", &args),
            r#"Read("src/main.rs")"#
        );
    }

    #[test]
    fn test_git_status_no_args() {
        let args = json!({});
        assert_eq!(format_tool_call_compact("git_status", &args), "git_status");
    }

    #[test]
    fn test_git_commit_with_scope() {
        let args = json!({
            "type": "feat",
            "scope": "tui",
            "message": "add compact display"
        });
        assert_eq!(
            format_tool_call_compact("git_commit", &args),
            r#"git_commit("feat(tui): add compact display")"#
        );
    }

    #[test]
    fn test_git_commit_without_scope() {
        let args = json!({
            "type": "fix",
            "message": "resolve bug"
        });
        assert_eq!(
            format_tool_call_compact("git_commit", &args),
            r#"git_commit("fix: resolve bug")"#
        );
    }

    #[test]
    fn test_git_diff_refs() {
        let args = json!({
            "from_ref": "main",
            "to_ref": "feature"
        });
        assert_eq!(
            format_tool_call_compact("git_diff", &args),
            r#"git_diff("main..feature")"#
        );
    }

    #[test]
    fn test_git_diff_cached() {
        let args = json!({"cached": true});
        assert_eq!(
            format_tool_call_compact("git_diff", &args),
            r#"git_diff("--cached")"#
        );
    }

    #[test]
    fn test_git_add_all() {
        let args = json!({"all": true});
        assert_eq!(
            format_tool_call_compact("git_add", &args),
            r#"git_add("-A")"#
        );
    }

    #[test]
    fn test_git_add_files() {
        let args = json!({"paths": ["a.rs", "b.rs", "c.rs"]});
        assert_eq!(
            format_tool_call_compact("git_add", &args),
            r#"git_add("3 file(s)")"#
        );
    }

    #[test]
    fn test_git_stash_action() {
        let args = json!({"action": "pop"});
        assert_eq!(
            format_tool_call_compact("git_stash", &args),
            r#"git_stash("pop")"#
        );
    }

    #[test]
    fn test_git_branch_create() {
        let args = json!({"create": "feature-x"});
        assert_eq!(
            format_tool_call_compact("git_branch", &args),
            r#"git_branch("create feature-x")"#
        );
    }

    #[test]
    fn test_git_checkout_branch() {
        let args = json!({"branch": "main"});
        assert_eq!(
            format_tool_call_compact("git_checkout", &args),
            r#"git_checkout("main")"#
        );
    }

    #[test]
    fn test_webfetch() {
        let args = json!({"url": "https://example.com/api"});
        assert_eq!(
            format_tool_call_compact("WebFetch", &args),
            r#"WebFetch("https://example.com/api")"#
        );
    }

    #[test]
    fn test_pwsh_command() {
        let args = json!({"command": "Get-Process"});
        assert_eq!(
            format_tool_call_compact("Pwsh", &args),
            r#"Pwsh("Get-Process")"#
        );
    }

    #[test]
    fn test_unknown_tool_fallback() {
        let args = json!({"query": "SELECT * FROM users"});
        assert_eq!(
            format_tool_call_compact("CustomTool", &args),
            r#"CustomTool("SELECT * FROM users")"#
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
}

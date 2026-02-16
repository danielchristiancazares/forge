//! Compact tool call display formatting.

use forge_types::truncate_to_fit;
use serde_json::Value;

pub fn format_tool_call_compact(name: &str, args: &Value) -> String {
    let display_name = canonical_tool_name(name);

    // Special case: Edit displays as Edit(path) showing patch file Summary
    if name == "Edit" {
        if let Some(obj) = args.as_object()
            && let Some(path) = format_patch_summary(obj)
        {
            return format!("Edit({})", truncate_to_fit(&path, 60, "…"));
        }
        return "Edit".to_string();
    }

    match extract_primary_arg(name, args) {
        Some(val) => format!("{}({})", display_name, truncate_to_fit(&val, 60, "…")),
        None => display_name.to_string(),
    }
}

pub(crate) fn canonical_tool_name(name: &str) -> std::borrow::Cow<'static, str> {
    use std::borrow::Cow;

    match name {
        "Read" => Cow::Borrowed("Read"),
        "Write" => Cow::Borrowed("Write"),
        "Edit" => Cow::Borrowed("Edit"),
        "Delete" => Cow::Borrowed("Delete"),
        "Move" => Cow::Borrowed("Move"),
        "Copy" => Cow::Borrowed("Copy"),
        "ListDir" => Cow::Borrowed("ListDir"),
        "Outline" => Cow::Borrowed("Outline"),

        "Glob" => Cow::Borrowed("Glob"),
        "Search" => Cow::Borrowed("Search"),

        "Git" => Cow::Borrowed("Git"),

        "Pwsh" => Cow::Borrowed("Pwsh"),
        "Run" => Cow::Borrowed("Run"),

        "WebFetch" => Cow::Borrowed("WebFetch"),

        "Recall" => Cow::Borrowed("Recall"),

        "Build" => Cow::Borrowed("Build"),
        "Test" => Cow::Borrowed("Test"),

        // Pass through unknown tools as-is
        _ => Cow::Owned(name.to_string()),
    }
}

fn extract_primary_arg(name: &str, args: &Value) -> Option<String> {
    let obj = args.as_object()?;

    let key = match name {
        "Glob" | "Search" => "pattern",
        "Read" | "Write" | "Edit" | "Delete" | "ListDir" | "Outline" => "path",
        "Move" | "Copy" => "source",
        "Git" => return format_git_tool(obj),
        "Pwsh" | "Run" => "command",
        "WebFetch" => "url",
        "Build" | "Test" => return format_build_test(obj),
        _ => return try_common_keys(obj),
    };

    obj.get(key).and_then(|v| v.as_str()).map(String::from)
}

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

fn format_git_stash(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("action").and_then(|v| v.as_str()).map(String::from)
}

fn format_git_restore(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(paths) = obj.get("paths").and_then(|v| v.as_array())
        && !paths.is_empty()
    {
        return Some(format!("{} file(s)", paths.len()));
    }
    None
}

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
    if obj.get("list_all").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-a".to_string());
    }
    if obj.get("list_remote").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some("-r".to_string());
    }
    None
}

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

fn format_git_show(obj: &serde_json::Map<String, Value>) -> Option<String> {
    obj.get("commit").and_then(|v| v.as_str()).map(String::from)
}

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

fn format_git_tool(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let cmd = obj.get("command").and_then(|v| v.as_str()).unwrap_or("?");
    let detail = match cmd {
        "commit" => format_git_commit(obj),
        "diff" => format_git_diff(obj),
        "add" => format_git_add(obj),
        "stash" => format_git_stash(obj),
        "restore" => format_git_restore(obj),
        "branch" => format_git_branch(obj),
        "checkout" => format_git_checkout(obj),
        "show" => format_git_show(obj),
        "log" => format_git_log(obj),
        "blame" => obj.get("path").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    };
    match detail {
        Some(d) => Some(format!("{cmd}({d})")),
        None => Some(cmd.to_string()),
    }
}

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
                                if let Some(escaped) = chars.next() {
                                    end += 1 + escaped.len_utf8();
                                } else {
                                    end += 1; // trailing backslash
                                    break;
                                }
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

fn format_build_test(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(system) = obj.get("build_system").and_then(|v| v.as_str()) {
        return Some(system.to_string());
    }
    if let Some(dir) = obj.get("working_dir").and_then(|v| v.as_str()) {
        return Some(dir.to_string());
    }
    None
}

fn try_common_keys(obj: &serde_json::Map<String, Value>) -> Option<String> {
    const COMMON_KEYS: &[&str] = &["pattern", "path", "query", "command", "url", "file", "name"];

    for key in COMMON_KEYS {
        if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(val.to_string());
        }
    }
    None
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
    fn test_git_status() {
        let args = json!({"command": "status"});
        assert_eq!(format_tool_call_compact("Git", &args), "Git(status)");
    }

    #[test]
    fn test_git_commit_with_scope() {
        let args = json!({
            "command": "commit",
            "type": "feat",
            "scope": "tui",
            "message": "add compact display"
        });
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(commit(feat(tui): add compact display))"
        );
    }

    #[test]
    fn test_git_commit_without_scope() {
        let args = json!({
            "command": "commit",
            "type": "fix",
            "message": "resolve bug"
        });
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(commit(fix: resolve bug))"
        );
    }

    #[test]
    fn test_git_diff_refs() {
        let args = json!({
            "command": "diff",
            "from_ref": "main",
            "to_ref": "feature"
        });
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(diff(main..feature))"
        );
    }

    #[test]
    fn test_git_diff_cached() {
        let args = json!({"command": "diff", "cached": true});
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(diff(--cached))"
        );
    }

    #[test]
    fn test_git_add_all() {
        let args = json!({"command": "add", "all": true});
        assert_eq!(format_tool_call_compact("Git", &args), "Git(add(-A))");
    }

    #[test]
    fn test_git_add_files() {
        let args = json!({"command": "add", "paths": ["a.rs", "b.rs", "c.rs"]});
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(add(3 file(s)))"
        );
    }

    #[test]
    fn test_git_stash_action() {
        let args = json!({"command": "stash", "action": "pop"});
        assert_eq!(format_tool_call_compact("Git", &args), "Git(stash(pop))");
    }

    #[test]
    fn test_git_branch_create() {
        let args = json!({"command": "branch", "create": "feature-x"});
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(branch(create feature-x))"
        );
    }

    #[test]
    fn test_git_checkout_branch() {
        let args = json!({"command": "checkout", "branch": "main"});
        assert_eq!(
            format_tool_call_compact("Git", &args),
            "Git(checkout(main))"
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
        assert_eq!(truncate_to_fit("hello", 10, "…"), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(100);
        let result = truncate_to_fit(&long, 60, "…");
        assert!(result.chars().count() <= 60);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_utf8_boundary() {
        // "héllo" has multi-byte char
        let s = "héllo world this is a long string";
        let result = truncate_to_fit(s, 10, "…");
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

use std::fmt::Write;
use std::path::PathBuf;

/// Runtime environment facts gathered once at application startup.
///
/// All fields are required (IFA: no optionality in core interfaces).
/// The boundary (`gather()`) resolves all platform queries; core code
/// consumes this struct without conditional checks.
pub struct EnvironmentContext {
    /// ISO 8601 date, e.g. "2026-02-11".
    date: String,
    /// Platform identifier from `std::env::consts::OS` (e.g. "macos", "linux", "windows").
    platform: &'static str,
    /// CPU architecture from `std::env::consts::ARCH` (e.g. "aarch64", "`x86_64`").
    arch: &'static str,
    /// Display string for the working directory.
    cwd: String,
    /// Whether the working directory is inside a git repository.
    is_git_repo: bool,
    /// Concatenated content from discovered AGENTS.md files.
    /// Consumed (taken) on first user message; empty string means nothing to inject.
    agents_md: String,
}

impl EnvironmentContext {
    /// Gathers environment facts from the OS. Called once at `App::new()`.
    ///
    /// This is boundary code: it queries external state and produces a strict
    /// representation. All fallbacks are resolved here; the returned struct
    /// has no conditional fields.
    #[must_use]
    pub fn gather() -> Self {
        let now = chrono::Local::now();
        let date = now.format("%Y-%m-%d").to_string();

        let cwd_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let is_git_repo = has_git_ancestor(&cwd_path);
        let cwd = cwd_path.display().to_string();
        let agents_md = discover_agents_md(&cwd_path);

        Self {
            date,
            platform: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            cwd,
            is_git_repo,
            agents_md,
        }
    }

    /// Takes the AGENTS.md content, leaving an empty string behind.
    /// Empty after first call — the content is consumed on first user message.
    pub fn take_agents_md(&mut self) -> String {
        std::mem::take(&mut self.agents_md)
    }

    /// Restores AGENTS.md content after a rollback (e.g. stream cancel on first message).
    pub fn restore_agents_md(&mut self, content: String) {
        if self.agents_md.is_empty() && !content.is_empty() {
            self.agents_md = content;
        }
    }

    /// Used by tests to avoid picking up real filesystem state.
    #[must_use]
    pub fn gather_without_agents_md() -> Self {
        let mut ctx = Self::gather();
        ctx.agents_md = String::new();
        ctx
    }

    /// Renders the environment block as markdown for system prompt injection.
    ///
    /// `model` is passed per-request because it can change via `/model`.
    #[must_use]
    pub fn render(&self, model: &str) -> String {
        let mut buf = String::with_capacity(256);
        let _ = writeln!(buf, "## Environment");
        let _ = writeln!(buf);
        let _ = writeln!(buf, "- Date: {}", self.date);
        let _ = writeln!(buf, "- Platform: {} ({})", self.platform, self.arch);
        let _ = writeln!(buf, "- Working directory: {}", self.cwd);
        let _ = writeln!(
            buf,
            "- Git repository: {}",
            if self.is_git_repo { "yes" } else { "no" }
        );
        let _ = writeln!(buf, "- Model: {model}");
        buf
    }
}

/// Discovers and concatenates AGENTS.md files from the user's environment.
///
/// Search order (all concatenated, global first, most-specific last):
/// 1. `~/.forge/AGENTS.md` — global user-level instructions
/// 2. Ancestor directories from root down to `cwd`, each `<dir>/AGENTS.md`
///
/// This is boundary code: filesystem I/O happens here, the result is a plain String.
fn discover_agents_md(cwd: &std::path::Path) -> String {
    let mut sections = Vec::new();

    // Global: ~/.forge/AGENTS.md
    if let Some(home) = dirs::home_dir() {
        let global_path = home.join(".forge").join("AGENTS.md");
        if let Ok(content) = std::fs::read_to_string(&global_path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                sections.push(trimmed.to_string());
            }
        }
    }

    // Ancestor walk: collect from cwd upward, then reverse for root-first order
    let mut ancestors = Vec::new();
    let mut dir = cwd.to_path_buf();
    loop {
        let agents_path = dir.join("AGENTS.md");
        if agents_path.is_file()
            && let Ok(content) = std::fs::read_to_string(&agents_path)
        {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                ancestors.push(trimmed.to_string());
            }
        }
        if !dir.pop() {
            break;
        }
    }
    ancestors.reverse();
    sections.extend(ancestors);

    if sections.is_empty() {
        String::new()
    } else {
        sections.join("\n\n")
    }
}

fn has_git_ancestor(start: &std::path::Path) -> bool {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return true;
        }
        if !dir.pop() {
            return false;
        }
    }
}

const PLACEHOLDER: &str = "{environment_context}";

/// Replaces `{environment_context}` in the base prompt with the rendered
/// environment block. Falls back to appending if the placeholder is absent.
#[must_use]
pub fn assemble_prompt(base: &str, env: &EnvironmentContext, model: &str) -> String {
    let rendered = env.render(model);
    if let Some(pos) = base.find(PLACEHOLDER) {
        let mut assembled = String::with_capacity(base.len() + rendered.len());
        assembled.push_str(&base[..pos]);
        assembled.push_str(&rendered);
        assembled.push_str(&base[pos + PLACEHOLDER.len()..]);
        assembled
    } else {
        let mut assembled = String::with_capacity(base.len() + rendered.len() + 2);
        assembled.push_str(base);
        assembled.push_str("\n\n");
        assembled.push_str(&rendered);
        assembled
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvironmentContext, assemble_prompt, discover_agents_md, has_git_ancestor};

    #[test]
    fn gather_produces_valid_context() {
        let ctx = EnvironmentContext::gather();
        assert!(!ctx.date.is_empty());
        assert!(!ctx.platform.is_empty());
        assert!(!ctx.cwd.is_empty());
    }

    #[test]
    fn discover_agents_md_reads_from_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let agents_path = dir.path().join("AGENTS.md");
        std::fs::write(&agents_path, "project rules here").unwrap();

        let result = discover_agents_md(dir.path());
        assert!(result.contains("project rules here"));
    }

    #[test]
    fn discover_agents_md_walks_ancestors() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("subdir");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(parent.path().join("AGENTS.md"), "parent rules").unwrap();
        std::fs::write(child.join("AGENTS.md"), "child rules").unwrap();

        let result = discover_agents_md(&child);
        assert!(result.contains("parent rules"));
        assert!(result.contains("child rules"));
        // Parent comes before child (general-to-specific)
        let parent_pos = result.find("parent rules").unwrap();
        let child_pos = result.find("child rules").unwrap();
        assert!(parent_pos < child_pos);
    }

    #[test]
    fn discover_agents_md_empty_when_none_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_agents_md(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn discover_agents_md_skips_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "   \n  \n  ").unwrap();

        let result = discover_agents_md(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn take_agents_md_consumes_content() {
        let mut ctx = EnvironmentContext {
            date: String::new(),
            platform: "test",
            arch: "test",
            cwd: String::new(),
            is_git_repo: false,
            agents_md: "some rules".to_string(),
        };
        let first = ctx.take_agents_md();
        assert_eq!(first, "some rules");
        let second = ctx.take_agents_md();
        assert!(second.is_empty());
    }

    #[test]
    fn render_contains_all_fields() {
        let ctx = EnvironmentContext {
            date: "2026-02-11".to_string(),
            platform: "macos",
            arch: "aarch64",
            cwd: "/home/user/project".to_string(),
            is_git_repo: true,
            agents_md: String::new(),
        };
        let rendered = ctx.render("claude-opus-4-6");
        assert!(rendered.contains("2026-02-11"));
        assert!(rendered.contains("macos"));
        assert!(rendered.contains("aarch64"));
        assert!(rendered.contains("/home/user/project"));
        assert!(rendered.contains("yes"));
        assert!(rendered.contains("claude-opus-4-6"));
    }

    #[test]
    fn assemble_replaces_placeholder() {
        let base = "Rules here.\n\n{environment_context}\n\n## Style";
        let ctx = EnvironmentContext {
            date: "2026-02-11".to_string(),
            platform: "linux",
            arch: "x86_64",
            cwd: "/tmp".to_string(),
            is_git_repo: false,
            agents_md: String::new(),
        };
        let assembled = assemble_prompt(base, &ctx, "gpt-5.2");
        assert!(!assembled.contains("{environment_context}"));
        assert!(assembled.contains("## Environment"));
        assert!(assembled.contains("## Style"));
        assert!(assembled.contains("gpt-5.2"));
    }

    #[test]
    fn assemble_appends_when_no_placeholder() {
        let base = "Rules here.";
        let ctx = EnvironmentContext {
            date: "2026-02-11".to_string(),
            platform: "linux",
            arch: "x86_64",
            cwd: "/tmp".to_string(),
            is_git_repo: false,
            agents_md: String::new(),
        };
        let assembled = assemble_prompt(base, &ctx, "gpt-5.2");
        assert!(assembled.starts_with("Rules here."));
        assert!(assembled.contains("## Environment"));
    }

    #[test]
    fn has_git_ancestor_finds_repo() {
        let cwd = std::env::current_dir().unwrap();
        assert!(has_git_ancestor(&cwd) || !cwd.join(".git").exists());
    }
}

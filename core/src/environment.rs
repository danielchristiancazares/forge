//! Boundary: gathers runtime environment facts from the OS.
//!
//! All filesystem, clock, and env-var access lives here.
//! The pure `EnvironmentContext` struct and its rendering are in `env_context`.

use std::path::PathBuf;

use crate::env_context::EnvironmentContext;

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

        Self::new(
            date,
            std::env::consts::OS,
            std::env::consts::ARCH,
            cwd,
            is_git_repo,
            agents_md,
        )
    }

    /// Used by tests to avoid picking up real filesystem state.
    #[must_use]
    pub fn gather_without_agents_md() -> Self {
        let mut ctx = Self::gather();
        let _ = ctx.take_agents_md();
        ctx
    }
}

const MAX_AGENTS_MD_BYTES: usize = 64 * 1024;

/// Discovers and concatenates AGENTS.md files from the user's environment.
///
/// Search order (all concatenated, global first, most-specific last):
/// 1. `~/.forge/AGENTS.md` — global user-level instructions
/// 2. Ancestor directories from root down to `cwd`, each `<dir>/AGENTS.md`
///
/// Total injected content is capped at [`MAX_AGENTS_MD_BYTES`] (64 KB).
fn discover_agents_md(cwd: &std::path::Path) -> String {
    let mut sections = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let mut total_bytes: usize = 0;

    if let Some(home) = dirs::home_dir() {
        let global_path = home.join(".forge").join("AGENTS.md");
        if let Ok(content) = std::fs::read_to_string(&global_path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                total_bytes += trimmed.len();
                sources.push(global_path.display().to_string());
                sections.push(trimmed.to_string());
            }
        }
    }

    let mut ancestors = Vec::new();
    let mut ancestor_sources = Vec::new();
    let mut dir = cwd.to_path_buf();
    loop {
        let agents_path = dir.join("AGENTS.md");
        if agents_path.is_file()
            && let Ok(content) = std::fs::read_to_string(&agents_path)
        {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                total_bytes += trimmed.len();
                ancestor_sources.push(agents_path.display().to_string());
                ancestors.push(trimmed.to_string());
            }
        }
        if !dir.pop() {
            break;
        }
    }
    ancestors.reverse();
    ancestor_sources.reverse();
    sections.extend(ancestors);
    sources.extend(ancestor_sources);

    if !sources.is_empty() {
        tracing::info!(
            count = sources.len(),
            total_bytes,
            sources = ?sources,
            "Discovered AGENTS.md instruction files"
        );
    }

    if sections.is_empty() {
        return String::new();
    }

    let mut result = sections.join("\n\n");
    if result.len() > MAX_AGENTS_MD_BYTES {
        tracing::warn!(
            total_bytes = result.len(),
            cap = MAX_AGENTS_MD_BYTES,
            "AGENTS.md content exceeds {MAX_AGENTS_MD_BYTES} byte cap; truncating"
        );
        result.truncate(result.floor_char_boundary(MAX_AGENTS_MD_BYTES));
    }
    result
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

#[cfg(test)]
mod tests {
    use super::{discover_agents_md, has_git_ancestor};
    use crate::env_context::EnvironmentContext;

    #[test]
    fn gather_produces_valid_context() {
        let ctx = EnvironmentContext::gather();
        let rendered = ctx.render("test-model");
        assert!(rendered.contains("Date:"));
        assert!(rendered.contains("Platform:"));
        assert!(rendered.contains("Working directory:"));
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
    fn has_git_ancestor_finds_repo() {
        let cwd = std::env::current_dir().unwrap();
        assert!(has_git_ancestor(&cwd) || !cwd.join(".git").exists());
    }
}

//! Core domain struct for runtime environment facts.
//!
//! Pure data + rendering. All I/O happens in the boundary `environment` module.

use std::fmt::Write;

/// Runtime environment facts gathered once at application startup.
///
/// All fields are required (IFA: no optionality in core interfaces).
/// The boundary (`gather()`) resolves all platform queries; core code
/// consumes this struct without conditional checks.
pub struct EnvironmentContext {
    date: String,
    platform: &'static str,
    arch: &'static str,
    cwd: String,
    is_git_repo: bool,
    agents_md: String,
}

impl EnvironmentContext {
    pub(crate) fn new(
        date: String,
        platform: &'static str,
        arch: &'static str,
        cwd: String,
        is_git_repo: bool,
        agents_md: String,
    ) -> Self {
        Self {
            date,
            platform,
            arch,
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
    use super::{EnvironmentContext, assemble_prompt};

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
}

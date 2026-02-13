//! Input mode wrappers for type-safe mode-specific operations.
//!
//! This module provides proof-token types and mode wrappers that ensure
//! operations are only performed when the app is in the correct mode.

use super::ui::{DraftInput, InputState};
use super::{ApiConfig, App, Message, NonEmptyString, OperationState};

pub(crate) use forge_tools::change_recording::{ChangeRecorder, TurnChangeReport, TurnContext};

/// Proof that a user message was validated and queued for sending.
#[derive(Debug)]
pub struct QueuedUserMessage {
    pub(crate) config: ApiConfig,
    pub(crate) turn: TurnContext,
}

/// Proof that a command line was entered in Command mode.
#[derive(Debug)]
pub struct EnteredCommand {
    pub(crate) raw: String,
}

/// Proof token for Insert mode operations.
#[derive(Debug)]
pub struct InsertToken(());

/// Proof token for Command mode operations.
#[derive(Debug)]
pub struct CommandToken(());

/// Mode wrapper for safe insert operations.
pub struct InsertMode<'a> {
    pub(crate) app: &'a mut App,
}

/// Mode wrapper for safe command operations.
pub struct CommandMode<'a> {
    pub(crate) app: &'a mut App,
}

impl App {
    /// Get proof token if currently in Insert mode.
    pub fn insert_token(&self) -> Option<InsertToken> {
        matches!(&self.input, InputState::Insert(_)).then_some(InsertToken(()))
    }

    /// Get proof token if currently in Command mode.
    pub fn command_token(&self) -> Option<CommandToken> {
        matches!(&self.input, InputState::Command { .. }).then_some(CommandToken(()))
    }

    /// Get insert mode wrapper (requires proof token).
    pub fn insert_mode(&mut self, _token: InsertToken) -> InsertMode<'_> {
        InsertMode { app: self }
    }

    /// Get command mode wrapper (requires proof token).
    pub fn command_mode(&mut self, _token: CommandToken) -> CommandMode<'_> {
        CommandMode { app: self }
    }
}

impl InsertMode<'_> {
    fn draft_mut(&mut self) -> &mut DraftInput {
        self.app.input.draft_mut()
    }

    pub fn move_cursor_left(&mut self) {
        self.draft_mut().move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        self.draft_mut().move_cursor_right();
    }

    pub fn enter_char(&mut self, new_char: char) {
        self.draft_mut().enter_char(new_char);
    }

    pub fn enter_newline(&mut self) {
        self.draft_mut().enter_newline();
    }

    pub fn enter_text(&mut self, text: &str) {
        self.draft_mut().enter_text(text);
    }

    pub fn delete_char(&mut self) {
        self.draft_mut().delete_char();
    }

    pub fn delete_char_forward(&mut self) {
        self.draft_mut().delete_char_forward();
    }

    pub fn delete_word_backwards(&mut self) {
        self.draft_mut().delete_word_backwards();
    }

    pub fn reset_cursor(&mut self) {
        self.draft_mut().reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        self.draft_mut().move_cursor_end();
    }

    pub fn clear_line(&mut self) {
        self.draft_mut().clear();
    }

    /// Queue the current draft as a user message.
    ///
    /// Returns a token proving that a non-empty user message was queued, and that it is valid to
    /// begin a new stream.
    #[must_use]
    pub fn queue_message(self) -> Option<QueuedUserMessage> {
        if !matches!(
            self.app.state,
            OperationState::Idle | OperationState::ToolsDisabled(_)
        ) {
            return None;
        }

        self.app.apply_pending_turn_settings();

        if self.app.draft_text().trim().is_empty() {
            return None;
        }

        // Preflight crash recovery BEFORE mutating history.
        //
        // If we recover after pushing a new user message, recovered assistant/tool
        // content would be appended after the new prompt (wrong chronology).
        let recovered = self.app.check_crash_recovery();
        if recovered.is_some()
            || !matches!(
                self.app.state,
                OperationState::Idle | OperationState::ToolsDisabled(_)
            )
        {
            // Recovery may have appended messages and/or entered ToolRecovery.
            // Don't consume the draft; let the user resume/discard, then re-send.
            return None;
        }

        let api_key = if let Some(key) = self.app.current_api_key().cloned() {
            key
        } else {
            self.app.push_notification(format!(
                "No API key configured. Set {} environment variable.",
                self.app.provider().env_var()
            ));
            return None;
        };

        let draft_text = self.app.input.draft_mut().take_text();
        // Record prompt to history for Up/Down navigation
        self.app.record_prompt(&draft_text);

        // Prepend AGENTS.md content to the first user message.
        // take_agents_md() consumes the content — subsequent messages get an empty string.
        let agents_md = self.app.environment.take_agents_md();
        let expanded = if agents_md.is_empty() {
            draft_text.clone()
        } else {
            format!("## AGENTS.md\n\n{agents_md}\n\n---\n\n{draft_text}")
        };

        // Expand @path file references: read file contents and prepend them.
        let expanded = expand_file_references(expanded);

        let content = if let Ok(content) = NonEmptyString::new(expanded) {
            content
        } else {
            return None;
        };

        // Automatic per-turn checkpoint (conversation rewind). Enables /undo and /retry.
        // Must happen before we append the user message to history.
        self.app.create_turn_checkpoint();

        // Track user message in context manager (also adds to display)
        let msg_id = self.app.push_history_message(Message::user(content));

        // Store original draft text (not expanded) for rollback on cancel.
        // Also stash consumed agents_md so it can be restored on rollback.
        self.app.pending_user_message = Some((msg_id, draft_text, agents_md));

        // Persist user message immediately for crash durability.
        // If persistence fails, rollback — streaming without a durable user
        // message violates the crash-recovery invariant (IFA §11.1).
        if !self.app.autosave_history() {
            self.app.rollback_pending_user_message();
            self.app.push_notification(
                "Cannot send: history save failed. Check disk space/permissions.",
            );
            return None;
        }

        self.app.scroll_to_bottom();
        self.app.tool_iterations = 0;

        let api_key = crate::util::wrap_api_key(self.app.model.provider(), api_key);

        let config = match ApiConfig::new(api_key, self.app.model.clone()) {
            Ok(config) => config
                .with_openai_options(self.app.openai_options_for_model(&self.app.model))
                .with_gemini_thinking_enabled(self.app.provider_runtime.gemini_thinking_enabled)
                .with_anthropic_thinking(
                    self.app.provider_runtime.anthropic_thinking_mode.as_str(),
                    self.app.provider_runtime.anthropic_thinking_effort.as_str(),
                ),
            Err(e) => {
                self.app
                    .push_notification(format!("Cannot queue request: {e}"));
                return None;
            }
        };

        Some(QueuedUserMessage {
            config,
            turn: TurnContext::new(),
        })
    }
}

// CommandMode operations

impl CommandMode<'_> {
    fn command_mut(&mut self) -> Option<&mut DraftInput> {
        self.app.input.command_mut()
    }

    pub fn move_cursor_left(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_right();
    }

    pub fn reset_cursor(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.move_cursor_end();
    }

    pub fn push_char(&mut self, c: char) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.enter_char(c);
    }

    pub fn backspace(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.delete_char();
    }

    pub fn delete_word_backwards(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.delete_word_backwards();
    }

    pub fn clear_line(&mut self) {
        let Some(command) = self.command_mut() else {
            return;
        };
        command.clear();
    }

    /// Perform shell-style tab completion on the command line.
    ///
    /// Completion is conservative: it only inserts additional characters when
    /// there is a single match or the matches share a longer common prefix.
    ///
    /// Supported completions:
    /// - Command names/aliases (e.g. `cl` -> `clear`)
    /// - Model argument for `/model` (current provider)
    /// - Rewind targets/scopes for `/rewind`
    pub fn tab_complete(&mut self) {
        let Some(line) = self.app.command_text().map(str::to_owned) else {
            return;
        };
        let cursor_byte = self
            .app
            .command_cursor_byte_index()
            .unwrap_or(line.len())
            .min(line.len());

        let Some(insert) = compute_command_tab_completion(self.app, &line, cursor_byte) else {
            return;
        };
        if insert.is_empty() {
            return;
        }

        let Some(command) = self.command_mut() else {
            return;
        };
        command.enter_text(&insert);
    }

    #[must_use]
    pub fn take_command(self) -> Option<EnteredCommand> {
        let input = std::mem::take(&mut self.app.input);
        let InputState::Command { draft, mut command } = input else {
            self.app.input = input;
            return None;
        };

        self.app.input = InputState::Normal(draft);
        Some(EnteredCommand {
            raw: command.take_text(),
        })
    }
}

// Command tab-completion helpers

fn compute_command_tab_completion(app: &App, line: &str, cursor_byte: usize) -> Option<String> {
    let cursor_byte = cursor_byte.min(line.len());
    let before_cursor = &line[..cursor_byte];

    // Identify the current token fragment (from last whitespace to cursor).
    let token_start = before_cursor
        .rfind(|c: char| c.is_whitespace())
        .map(|idx| {
            idx + before_cursor[idx..]
                .chars()
                .next()
                .unwrap_or(' ')
                .len_utf8()
        })
        .unwrap_or(0);

    let token_index = before_cursor[..token_start].split_whitespace().count();
    let fragment = &line[token_start..cursor_byte];

    if token_index == 0 {
        return complete_command_name(fragment, cursor_byte == line.len());
    }

    // Argument completion depends on the (normalized) command name.
    let first = line.split_whitespace().next()?;
    let kind = crate::commands::normalize_command_name(first)?;
    let arg_index = token_index.saturating_sub(1);

    complete_command_arg(app, kind, arg_index, fragment)
}

fn complete_command_name(fragment: &str, at_end_of_line: bool) -> Option<String> {
    let (_prefix, core) = fragment
        .strip_prefix('/')
        .map_or(("", fragment), |rest| ("/", rest));
    let core_lower = core.to_ascii_lowercase();
    let core_chars = core.chars().count();

    let matches: Vec<&crate::commands::CommandAlias> = crate::commands::command_aliases()
        .iter()
        .filter(|alias| alias.name.starts_with(&core_lower))
        .collect();

    if matches.is_empty() {
        return None;
    }

    if matches.len() == 1 {
        let m = matches[0];
        let mut insert: String = m.name.chars().skip(core_chars).collect();
        if m.kind.expects_arg() && at_end_of_line {
            insert.push(' ');
        }
        return Some(insert);
    }

    // Multiple matches: extend to the longest common prefix if it advances the cursor.
    let names: Vec<&str> = matches.iter().map(|m| m.name).collect();
    let lcp = longest_common_prefix(&names);
    if lcp.chars().count() <= core_chars {
        return None;
    }

    Some(lcp.chars().skip(core_chars).collect())
}

fn complete_command_arg(
    app: &App,
    kind: crate::commands::CommandKind,
    arg_index: usize,
    fragment: &str,
) -> Option<String> {
    use crate::commands::CommandKind;

    // Rewind targets accept optional leading '#', because checkpoint lists format ids as #<id>.
    let (_prefix, core) = if matches!(kind, CommandKind::Rewind) && arg_index == 0 {
        fragment
            .strip_prefix('#')
            .map_or(("", fragment), |rest| ("#", rest))
    } else {
        ("", fragment)
    };

    let core_lower = core.to_ascii_lowercase();
    let core_chars = core.chars().count();

    let candidates: Vec<String> = match (kind, arg_index) {
        (CommandKind::Model, 0) => app
            .provider()
            .available_models()
            .into_iter()
            .map(|model| model.model_id().to_string())
            .collect(),
        (CommandKind::Rewind, 0) => {
            let mut v = vec![
                "list".to_string(),
                "ls".to_string(),
                "last".to_string(),
                "latest".to_string(),
            ];
            // Checkpoints are cheap (<= 50). Offer all numeric ids for completion.
            for s in app.checkpoints.summaries() {
                v.push(s.id.to_string());
            }
            v
        }
        (CommandKind::Rewind, 1) => vec![
            "code".to_string(),
            "conversation".to_string(),
            "chat".to_string(),
            "both".to_string(),
        ],
        _ => Vec::new(),
    };

    if candidates.is_empty() {
        return None;
    }

    let matches: Vec<&str> = candidates
        .iter()
        .map(String::as_str)
        .filter(|c| c.to_ascii_lowercase().starts_with(&core_lower))
        .collect();

    if matches.is_empty() {
        return None;
    }

    if matches.len() == 1 {
        let chosen = matches[0];
        return Some(chosen.chars().skip(core_chars).collect());
    }

    let lcp = longest_common_prefix(&matches);
    if lcp.chars().count() <= core_chars {
        return None;
    }

    Some(lcp.chars().skip(core_chars).collect())
}

const MAX_FILE_REF_BYTES: usize = 200 * 1024;

/// Expand `@path` file references in user message text.
///
/// Scans for `@` followed by non-whitespace, checks if the path exists as a file,
/// and prepends file contents to the message. The `@path` tokens remain in the
/// user's text as anchors.
fn expand_file_references(text: String) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut file_sections = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for word in text.split_whitespace() {
        let Some(path_str) = word.strip_prefix('@') else {
            continue;
        };
        if path_str.is_empty() || seen.contains(path_str) {
            continue;
        }

        let path = cwd.join(path_str);
        if path.is_file()
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            seen.insert(path_str.to_string());
            let content = if content.len() > MAX_FILE_REF_BYTES {
                let mut end = MAX_FILE_REF_BYTES;
                while end > 0 && !content.is_char_boundary(end) {
                    end -= 1;
                }
                format!(
                    "{}...\n[truncated at {}KB]",
                    &content[..end],
                    MAX_FILE_REF_BYTES / 1024
                )
            } else {
                content
            };
            file_sections.push(format!("`{path_str}`:\n```\n{content}\n```"));
        }
    }

    if file_sections.is_empty() {
        text
    } else {
        let files_block = file_sections.join("\n\n");
        format!("{files_block}\n\n---\n\n{text}")
    }
}

fn longest_common_prefix(strings: &[&str]) -> String {
    let Some(first) = strings.first() else {
        return String::new();
    };

    let mut prefix: Vec<char> = first.chars().collect();
    for s in &strings[1..] {
        let mut new_len = 0usize;
        for (a, b) in prefix.iter().zip(s.chars()) {
            if *a == b {
                new_len += 1;
            } else {
                break;
            }
        }
        prefix.truncate(new_len);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_common_prefix_empty() {
        assert_eq!(longest_common_prefix(&[]), "");
    }

    #[test]
    fn longest_common_prefix_single() {
        assert_eq!(longest_common_prefix(&["hello"]), "hello");
    }

    #[test]
    fn longest_common_prefix_multiple_full_match() {
        assert_eq!(longest_common_prefix(&["test", "test"]), "test");
    }

    #[test]
    fn longest_common_prefix_partial() {
        assert_eq!(longest_common_prefix(&["provider", "provide"]), "provide");
        assert_eq!(longest_common_prefix(&["clear", "context"]), "c");
        assert_eq!(longest_common_prefix(&["quit", "query"]), "qu");
    }

    #[test]
    fn longest_common_prefix_no_match() {
        assert_eq!(longest_common_prefix(&["abc", "xyz"]), "");
    }

    #[test]
    fn complete_command_name_unique_match() {
        // "cle" uniquely matches "clear"
        assert_eq!(complete_command_name("cle", true), Some("ar".to_string()));
        // "mo" uniquely matches "model", adds space since it expects arg
        assert_eq!(complete_command_name("mo", true), Some("del ".to_string()));
    }

    #[test]
    fn complete_command_name_with_slash_prefix() {
        // Leading slash should be stripped for matching
        assert_eq!(complete_command_name("/cle", true), Some("ar".to_string()));
    }

    #[test]
    fn complete_command_name_ambiguous_extends_to_lcp() {
        // Ambiguous: "c" matches multiple commands, so no completion is applied.
        assert_eq!(complete_command_name("c", true), None);
    }

    #[test]
    fn complete_command_name_no_match() {
        assert_eq!(complete_command_name("xyz", true), None);
    }

    #[test]
    fn complete_command_name_exact_match() {
        // Already complete, nothing to add (except space for commands expecting args)
        assert_eq!(complete_command_name("clear", true), Some(String::new()));
    }

    #[test]
    fn complete_command_name_case_insensitive() {
        assert_eq!(complete_command_name("CLE", true), Some("ar".to_string()));
        assert_eq!(complete_command_name("QU", true), Some("it".to_string()));
    }
}

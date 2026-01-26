//! Input mode wrappers for type-safe mode-specific operations.
//!
//! This module provides proof-token types and mode wrappers that ensure
//! operations are only performed when the app is in the correct mode.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ui::{DraftInput, InputState};
use super::{ApiConfig, ApiKey, App, Message, NonEmptyString, OperationState, Provider};

// ============================================================================
// Turn change tracking
// ============================================================================

/// Proof that a user turn is active.
#[derive(Debug)]
pub(crate) struct TurnContext {
    changes: Arc<Mutex<TurnChangeLog>>,
}

#[derive(Debug, Default)]
struct TurnChangeLog {
    created: BTreeSet<PathBuf>,
    modified: BTreeSet<PathBuf>,
}

/// Capability token that allows tool executors to record changes.
#[derive(Debug, Clone)]
pub(crate) struct ChangeRecorder {
    changes: Arc<Mutex<TurnChangeLog>>,
}

#[derive(Debug)]
pub(crate) enum TurnChangeReport {
    NoChanges,
    Changes(TurnChangeSummary),
}

#[derive(Debug)]
pub(crate) struct TurnChangeSummary {
    content: NonEmptyString,
}

impl TurnContext {
    fn new() -> Self {
        Self {
            changes: Arc::new(Mutex::new(TurnChangeLog::default())),
        }
    }

    pub(crate) fn new_for_recovery() -> Self {
        Self::new()
    }

    #[cfg(test)]
    pub(crate) fn new_for_tests() -> Self {
        Self::new()
    }

    pub(crate) fn recorder(&self) -> ChangeRecorder {
        ChangeRecorder {
            changes: Arc::clone(&self.changes),
        }
    }

    /// Finish the turn and return both the report and the raw path sets.
    ///
    /// The raw sets are used for session-wide aggregation in the files panel.
    pub(crate) fn finish(
        self,
        working_dir: &Path,
    ) -> (TurnChangeReport, BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
        let mut log = self.changes.lock().expect("mutex poisoned");
        let log = std::mem::take(&mut *log);
        let created = log.created.clone();
        let modified = log.modified.clone();
        let report = log.into_report(working_dir);
        (report, created, modified)
    }
}

impl ChangeRecorder {
    pub(crate) fn record_created(&self, path: PathBuf) {
        let mut log = self.changes.lock().expect("mutex poisoned");
        log.record_created(path);
    }

    pub(crate) fn record_modified(&self, path: PathBuf) {
        let mut log = self.changes.lock().expect("mutex poisoned");
        log.record_modified(path);
    }
}

impl TurnChangeSummary {
    pub(crate) fn into_message(self) -> NonEmptyString {
        self.content
    }
}

impl TurnChangeLog {
    fn record_created(&mut self, path: PathBuf) {
        self.modified.remove(&path);
        self.created.insert(path);
    }

    fn record_modified(&mut self, path: PathBuf) {
        if !self.created.contains(&path) {
            self.modified.insert(path);
        }
    }

    fn into_report(self, working_dir: &Path) -> TurnChangeReport {
        if self.created.is_empty() && self.modified.is_empty() {
            return TurnChangeReport::NoChanges;
        }

        let created = format_paths(&self.created, working_dir);
        let modified = format_paths(&self.modified, working_dir);
        TurnChangeReport::Changes(TurnChangeSummary::new(created, modified))
    }
}

fn format_paths(paths: &BTreeSet<PathBuf>, working_dir: &Path) -> Vec<String> {
    paths
        .iter()
        .map(|path| format_path(path, working_dir))
        .collect()
}

fn format_path(path: &Path, working_dir: &Path) -> String {
    path.strip_prefix(working_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

impl TurnChangeSummary {
    fn new(created: Vec<String>, modified: Vec<String>) -> Self {
        let mut lines: Vec<String> = Vec::new();

        if !created.is_empty() {
            lines.push(format!("Created files ({}):", created.len()));
            lines.extend(created.into_iter().map(|path| format!("- {path}")));
        }

        if !modified.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("Modified files ({}):", modified.len()));
            lines.extend(modified.into_iter().map(|path| format!("- {path}")));
        }

        let content = NonEmptyString::new(lines.join("\n"))
            .expect("summary must be non-empty when changes exist");
        Self { content }
    }
}

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

// ============================================================================
// Token factory methods (called from App)
// ============================================================================

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

// ============================================================================
// InsertMode operations
// ============================================================================

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
        // Can't send while busy with another operation
        if !matches!(self.app.state, OperationState::Idle) {
            return None;
        }

        // Ignore empty input
        if self.app.draft_text().trim().is_empty() {
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

        let raw_content = self.app.input.draft_mut().take_text();
        // Record prompt to history for Up/Down navigation
        self.app.record_prompt(&raw_content);

        let content = if let Ok(content) = NonEmptyString::new(raw_content.clone()) {
            content
        } else {
            return None;
        };

        // Automatic per-turn checkpoint (conversation rewind). Enables /undo and /retry.
        // Must happen before we append the user message to history.
        self.app.create_turn_checkpoint();

        // Track user message in context manager (also adds to display)
        let msg_id = self.app.push_history_message(Message::user(content));
        self.app.autosave_history(); // Persist user message immediately for crash durability

        // Store pending message for potential rollback if stream fails with no content
        self.app.pending_user_message = Some((msg_id, raw_content));

        self.app.scroll_to_bottom();
        self.app.tool_iterations = 0;

        let api_key = match self.app.model.provider() {
            Provider::Claude => ApiKey::Claude(api_key),
            Provider::OpenAI => ApiKey::OpenAI(api_key),
            Provider::Gemini => ApiKey::Gemini(api_key),
        };

        let config = match ApiConfig::new(api_key, self.app.model.clone()) {
            Ok(config) => config.with_openai_options(self.app.openai_options),
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

// ============================================================================
// CommandMode operations
// ============================================================================

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
    /// - Provider argument for `/provider`
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

// ============================================================================
// Command tab-completion helpers
// ============================================================================

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
        (CommandKind::Provider, 0) => vec![
            "claude".to_string(),
            "anthropic".to_string(),
            "gpt".to_string(),
            "openai".to_string(),
            "chatgpt".to_string(),
            "gemini".to_string(),
            "google".to_string(),
        ],
        (CommandKind::Model, 0) => app
            .provider()
            .available_models()
            .iter()
            .map(ToString::to_string)
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
        // "con" matches "context" and "cancel" - no common prefix beyond "c"
        // Actually "con" only matches "context", let me check...
        // "c" matches "clear", "cancel", "context", "ctx" - lcp is "c", no extension
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

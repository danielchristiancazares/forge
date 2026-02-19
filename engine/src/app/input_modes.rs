//! Input mode wrappers for type-safe mode-specific operations.
//!
//! This module provides borrow-scoped mode guards that ensure operations
//! are only performed when the app is in the correct mode.

use super::ui::{DraftInput, InputState};
use super::{ApiConfig, App, Message, NonEmptyString, OperationState};

pub(crate) use forge_tools::change_recording::{ChangeRecorder, TurnChangeReport, TurnContext};

/// Proof that a user message was validated and queued for sending.
#[derive(Debug)]
pub struct QueuedUserMessage {
    pub(crate) config: ApiConfig,
    pub(crate) turn: TurnContext,
}

#[derive(Debug)]
pub enum QueueMessageResult {
    Queued(QueuedUserMessage),
    Skipped,
}

/// Proof that a command line was entered in Command mode.
#[derive(Debug)]
pub struct EnteredCommand {
    pub(crate) raw: String,
}

pub struct InsertMode<'a> {
    pub(crate) app: &'a mut App,
}

pub struct CommandMode<'a> {
    pub(crate) app: &'a mut App,
}

pub enum InsertModeAccess<'a> {
    InInsert(InsertMode<'a>),
    NotInsert,
}

pub enum CommandModeAccess<'a> {
    InCommand(CommandMode<'a>),
    NotCommand,
}

impl App {
    /// Borrow-scoped access to Insert-mode operations.
    ///
    /// The returned guard holds `&mut App`, so the input mode cannot be
    /// changed while the guard exists.
    pub fn insert_mode_mut(&mut self) -> InsertModeAccess<'_> {
        match &self.ui.input {
            InputState::Insert(_) => InsertModeAccess::InInsert(InsertMode { app: self }),
            _ => InsertModeAccess::NotInsert,
        }
    }

    /// Borrow-scoped access to Command-mode operations.
    ///
    /// The returned guard holds `&mut App`, so the input mode cannot be
    /// changed while the guard exists.
    pub fn command_mode_mut(&mut self) -> CommandModeAccess<'_> {
        match &self.ui.input {
            InputState::Command { .. } => CommandModeAccess::InCommand(CommandMode { app: self }),
            _ => CommandModeAccess::NotCommand,
        }
    }
}

impl InsertMode<'_> {
    fn draft_mut(&mut self) -> &mut DraftInput {
        self.app.ui.input.draft_mut()
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
    pub fn queue_message(self) -> QueueMessageResult {
        if !matches!(self.app.core.state, OperationState::Idle) {
            return QueueMessageResult::Skipped;
        }

        self.app.apply_pending_turn_settings();

        if self.app.draft_text().trim().is_empty() {
            return QueueMessageResult::Skipped;
        }

        // Preflight crash recovery BEFORE mutating history.
        //
        // If we recover after pushing a new user message, recovered assistant/tool
        // content would be appended after the new prompt (wrong chronology).
        let recovered = self.app.check_crash_recovery();
        if recovered.is_some() || !matches!(self.app.core.state, OperationState::Idle) {
            // Recovery may have appended messages and/or entered ToolRecovery.
            // Don't consume the draft; let the user resume/discard, then re-send.
            return QueueMessageResult::Skipped;
        }

        let api_key = if let Some(key) = self.app.current_api_key().cloned() {
            key
        } else {
            self.app.push_notification(format!(
                "No API key configured. Set {} environment variable.",
                self.app.provider().env_var()
            ));
            return QueueMessageResult::Skipped;
        };

        let draft_text = self.app.ui.input.draft_mut().take_text();
        // Record prompt to history for Up/Down navigation
        self.app.record_prompt(&draft_text);

        // Prepend AGENTS.md content to the first user message.
        // take_agents_md() consumes the content — subsequent messages get an empty string.
        let agents_md = self.app.core.environment.take_agents_md();
        let expanded = if agents_md.is_empty() {
            draft_text.clone()
        } else {
            format!("## AGENTS.md\n\n{agents_md}\n\n---\n\n{draft_text}")
        };

        // Expand @path file references: read file contents and prepend them.
        let expansion = expand_file_references(
            &expanded,
            &self.app.runtime.tool_settings.sandbox,
            self.app
                .runtime
                .tool_settings
                .read_limits
                .max_file_read_bytes,
            &self.app.runtime.tool_file_cache,
        );

        for (path, reason) in &expansion.denied {
            self.app
                .push_notification(format!("@{path}: denied ({reason})"));
        }
        for (path, error) in &expansion.failed {
            self.app.push_notification(format!("@{path}: {error}"));
        }

        let expanded = if expansion.file_sections.is_empty() {
            expanded
        } else {
            let files_block = expansion.file_sections.join("\n\n");
            format!("{files_block}\n\n---\n\n{expanded}")
        };

        let content = if let Ok(content) = NonEmptyString::new(expanded) {
            content
        } else {
            return QueueMessageResult::Skipped;
        };

        // Automatic per-turn checkpoint (conversation rewind). Enables /undo and /retry.
        // Must happen before we append the user message to history.
        self.app.create_turn_checkpoint();

        // Track user message in context manager (also adds to display).
        // When AGENTS.md or @file refs were prepended, store a display version
        // that includes an attachment summary so the user can see what was sent.
        let message = match NonEmptyString::new(draft_text.clone()) {
            Ok(display) if display.as_str() != content.as_str() => {
                let display_with_summary =
                    if expansion.expanded_files.is_empty() && agents_md.is_empty() {
                        display
                    } else {
                        let mut summary_parts = Vec::new();
                        if !agents_md.is_empty() {
                            summary_parts.push("AGENTS.md".to_string());
                        }
                        for (path, bytes) in &expansion.expanded_files {
                            let kb = *bytes as f64 / 1024.0;
                            summary_parts.push(format!("@{path} ({kb:.1}KB)"));
                        }
                        let summary = format!("[Attached: {}]", summary_parts.join(", "));
                        NonEmptyString::new(format!("{summary}\n\n{}", display.as_str()))
                            .unwrap_or(display)
                    };
                Message::user_with_display(content, display_with_summary)
            }
            _ => Message::user(content),
        };
        let msg_id = self.app.push_history_message(message);

        // Store original draft text (not expanded) for rollback on cancel.
        // Also stash consumed agents_md so it can be restored on rollback.
        self.app.core.pending_user_message = Some((msg_id, draft_text, agents_md));

        // Persist user message immediately for crash durability.
        // If persistence fails, rollback — streaming without a durable user
        // message violates the crash-recovery invariant (IFA §11.1).
        if !self.app.autosave_history() {
            self.app.rollback_pending_user_message();
            self.app.push_notification(
                "Cannot send: history save failed. Check disk space/permissions.",
            );
            return QueueMessageResult::Skipped;
        }

        self.app.scroll_to_bottom();
        self.app.core.tool_iterations = 0;

        let api_key = crate::util::wrap_api_key(self.app.core.model.provider(), api_key);

        let config = match ApiConfig::new(api_key, self.app.core.model.clone()) {
            Ok(config) => config
                .with_openai_options(self.app.openai_options_for_model(&self.app.core.model))
                .with_gemini_thinking_enabled(
                    self.app.runtime.provider_runtime.gemini_thinking_enabled,
                )
                .with_anthropic_thinking(
                    self.app
                        .runtime
                        .provider_runtime
                        .anthropic_thinking_mode
                        .as_str(),
                    self.app
                        .runtime
                        .provider_runtime
                        .anthropic_thinking_effort
                        .as_str(),
                ),
            Err(e) => {
                self.app
                    .push_notification(format!("Cannot queue request: {e}"));
                return QueueMessageResult::Skipped;
            }
        };

        QueueMessageResult::Queued(QueuedUserMessage {
            config,
            turn: TurnContext::new(),
        })
    }
}

// CommandMode operations

impl CommandMode<'_> {
    fn command_mut(&mut self) -> &mut DraftInput {
        self.app
            .ui
            .input
            .command_mut()
            .expect("CommandMode must hold Command input state")
    }

    pub fn move_cursor_left(&mut self) {
        self.command_mut().move_cursor_left();
    }

    pub fn move_cursor_right(&mut self) {
        self.command_mut().move_cursor_right();
    }

    pub fn reset_cursor(&mut self) {
        self.command_mut().reset_cursor();
    }

    pub fn move_cursor_end(&mut self) {
        self.command_mut().move_cursor_end();
    }

    pub fn push_char(&mut self, c: char) {
        self.command_mut().enter_char(c);
    }

    pub fn backspace(&mut self) {
        self.command_mut().delete_char();
    }

    pub fn delete_word_backwards(&mut self) {
        self.command_mut().delete_word_backwards();
    }

    pub fn clear_line(&mut self) {
        self.command_mut().clear();
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

        let insert = match compute_command_tab_completion(self.app, &line, cursor_byte) {
            CommandCompletion::Available(insert) => insert,
            CommandCompletion::Unavailable => return,
        };
        if insert.is_empty() {
            return;
        }

        self.command_mut().enter_text(&insert);
    }

    #[must_use]
    pub fn take_command(self) -> EnteredCommand {
        let input = std::mem::take(&mut self.app.ui.input);
        let InputState::Command { draft, mut command } = input else {
            panic!("CommandMode must hold Command input state");
        };

        self.app.ui.input = InputState::Normal(draft);
        EnteredCommand {
            raw: command.take_text(),
        }
    }
}

// Command tab-completion helpers

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandCompletion {
    Available(String),
    Unavailable,
}

fn compute_command_tab_completion(app: &App, line: &str, cursor_byte: usize) -> CommandCompletion {
    let cursor_byte = cursor_byte.min(line.len());
    let before_cursor = &line[..cursor_byte];

    // Identify the current token fragment (from last whitespace to cursor).
    let token_start = before_cursor
        .rfind(|c: char| c.is_whitespace())
        .map_or(0, |idx| {
            idx + before_cursor[idx..]
                .chars()
                .next()
                .unwrap_or(' ')
                .len_utf8()
        });

    let token_index = before_cursor[..token_start].split_whitespace().count();
    let fragment = &line[token_start..cursor_byte];

    if token_index == 0 {
        return complete_command_name(fragment, cursor_byte == line.len());
    }

    // Argument completion depends on the (normalized) command name.
    let first = match line.split_whitespace().next() {
        Some(first) => first,
        None => return CommandCompletion::Unavailable,
    };
    let kind = match super::commands::normalize_command_name(first) {
        super::commands::NormalizedCommandName::Known(kind) => kind,
        super::commands::NormalizedCommandName::Blank
        | super::commands::NormalizedCommandName::Unrecognized(_) => {
            return CommandCompletion::Unavailable;
        }
    };
    let arg_index = token_index.saturating_sub(1);

    complete_command_arg(app, kind, arg_index, fragment)
}

fn complete_command_name(fragment: &str, at_end_of_line: bool) -> CommandCompletion {
    let (_prefix, core) = fragment
        .strip_prefix('/')
        .map_or(("", fragment), |rest| ("/", rest));
    let core_lower = core.to_ascii_lowercase();
    let core_chars = core.chars().count();

    let matches: Vec<&super::commands::CommandAlias> = super::commands::command_aliases()
        .iter()
        .filter(|alias| alias.name.starts_with(&core_lower))
        .collect();

    if matches.is_empty() {
        return CommandCompletion::Unavailable;
    }

    if matches.len() == 1 {
        let m = matches[0];
        let mut insert: String = m.name.chars().skip(core_chars).collect();
        if m.kind.expects_arg() && at_end_of_line {
            insert.push(' ');
        }
        return CommandCompletion::Available(insert);
    }

    // Multiple matches: extend to the longest common prefix if it advances the cursor.
    let names: Vec<&str> = matches.iter().map(|m| m.name).collect();
    let lcp = longest_common_prefix(&names);
    if lcp.chars().count() <= core_chars {
        return CommandCompletion::Unavailable;
    }

    CommandCompletion::Available(lcp.chars().skip(core_chars).collect())
}

fn complete_command_arg(
    app: &App,
    kind: super::commands::CommandKind,
    arg_index: usize,
    fragment: &str,
) -> CommandCompletion {
    use super::commands::CommandKind;

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
            for s in app.core.checkpoints.summaries() {
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
        return CommandCompletion::Unavailable;
    }

    let matches: Vec<&str> = candidates
        .iter()
        .map(String::as_str)
        .filter(|c| c.to_ascii_lowercase().starts_with(&core_lower))
        .collect();

    if matches.is_empty() {
        return CommandCompletion::Unavailable;
    }

    if matches.len() == 1 {
        let chosen = matches[0];
        return CommandCompletion::Available(chosen.chars().skip(core_chars).collect());
    }

    let lcp = longest_common_prefix(&matches);
    if lcp.chars().count() <= core_chars {
        return CommandCompletion::Unavailable;
    }

    CommandCompletion::Available(lcp.chars().skip(core_chars).collect())
}

struct FileExpansionResult {
    file_sections: Vec<String>,
    /// (path, byte_count) for each successfully expanded file.
    expanded_files: Vec<(String, usize)>,
    denied: Vec<(String, String)>,
    failed: Vec<(String, String)>,
}

/// Expand `@path` file references in user message text.
///
/// Routes each path through the sandbox for policy enforcement (deny patterns,
/// root constraints, unsafe char rejection) and populates the file cache so
/// that subsequent edits get stale-file protection.
fn expand_file_references(
    text: &str,
    sandbox: &forge_tools::sandbox::Sandbox,
    max_read_bytes: usize,
    file_cache: &std::sync::Arc<tokio::sync::Mutex<forge_tools::ToolFileCache>>,
) -> FileExpansionResult {
    let working_dir = sandbox.working_dir();
    let mut file_sections = Vec::new();
    let mut expanded_files = Vec::new();
    let mut denied = Vec::new();
    let mut failed = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for path_str in parse_file_references(text) {
        if seen.contains(&path_str) {
            continue;
        }
        seen.insert(path_str.clone());

        let resolved = match sandbox.resolve_path(&path_str, &working_dir) {
            Ok(r) => r,
            Err(e) => {
                denied.push((path_str, e.to_string()));
                continue;
            }
        };

        let meta = match std::fs::metadata(&resolved) {
            Ok(m) if m.is_file() => m,
            Ok(_) => {
                failed.push((path_str, "not a file".to_string()));
                continue;
            }
            Err(e) => {
                failed.push((path_str, e.to_string()));
                continue;
            }
        };

        if meta.len() > max_read_bytes as u64 {
            failed.push((path_str, "file too large".to_string()));
            continue;
        }

        match read_file_reference_content(&resolved, max_read_bytes) {
            FileReferenceContent::Text(content) => {
                let byte_count = content.len();
                let line_count = content.lines().count() as u32;
                if let Ok(mut cache) = file_cache.try_lock() {
                    let _ = forge_tools::record_file_read(&mut cache, &resolved, line_count);
                }
                file_sections.push(format!("`{path_str}`:\n```\n{content}\n```"));
                expanded_files.push((path_str, byte_count));
            }
            FileReferenceContent::NonTextOrUnreadable => {
                failed.push((path_str, "binary or non-UTF-8 file".to_string()));
            }
        }
    }

    FileExpansionResult {
        file_sections,
        expanded_files,
        denied,
        failed,
    }
}

/// Parse `@path` references from text.
///
/// Supported forms:
/// - `@path/to/file.rs`
/// - `@"path with spaces/file.md"`
/// - `@path/with\ spaces/file.md`
fn parse_file_references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut prev_char = PreviousChar::Start;
    let mut cursor = 0usize;

    while cursor < text.len() {
        let mut chars = text[cursor..].chars();
        let ch = chars.next().expect("cursor is always on a char boundary");
        let ch_len = ch.len_utf8();

        if ch == '@' {
            let at_token_start = prev_char.is_token_boundary();
            if at_token_start {
                let path_start = cursor + ch_len;
                let (parsed, next_cursor) = parse_file_reference_path(text, path_start);
                if let ParsedReference::Path(path) = parsed {
                    references.push(path);
                }
                if next_cursor > cursor {
                    prev_char = text[..next_cursor]
                        .chars()
                        .next_back()
                        .map_or(PreviousChar::Start, PreviousChar::Char);
                    cursor = next_cursor;
                    continue;
                }
            }
        }

        prev_char = PreviousChar::Char(ch);
        cursor += ch_len;
    }

    references
}

#[derive(Debug, Clone, Copy)]
enum PreviousChar {
    Start,
    Char(char),
}

impl PreviousChar {
    fn is_token_boundary(self) -> bool {
        match self {
            Self::Start => true,
            Self::Char(ch) => ch.is_whitespace(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedReference {
    Path(String),
    Missing,
}

fn parse_file_reference_path(text: &str, start: usize) -> (ParsedReference, usize) {
    if start >= text.len() {
        return (ParsedReference::Missing, start);
    }

    match text[start..].chars().next() {
        Some('"') => parse_quoted_file_reference(text, start),
        Some(ch) if ch.is_whitespace() => (ParsedReference::Missing, start),
        Some(_) => parse_unquoted_file_reference(text, start),
        None => (ParsedReference::Missing, start),
    }
}

fn parse_quoted_file_reference(text: &str, quote_start: usize) -> (ParsedReference, usize) {
    let mut path = String::new();
    let mut chars = text[quote_start + 1..].char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        let abs_idx = quote_start + 1 + offset;
        if ch == '"' {
            if path.is_empty() {
                return (ParsedReference::Missing, abs_idx + 1);
            }
            return (ParsedReference::Path(path), abs_idx + 1);
        }

        if ch == '\\'
            && let Some((_, next_ch)) = chars.peek()
            && matches!(*next_ch, '"' | '\\')
        {
            path.push(*next_ch);
            chars.next();
            continue;
        }

        path.push(ch);
    }

    (ParsedReference::Missing, text.len())
}

fn parse_unquoted_file_reference(text: &str, start: usize) -> (ParsedReference, usize) {
    let mut path = String::new();
    let mut end = text.len();
    let mut chars = text[start..].char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        let abs_idx = start + offset;
        if ch.is_whitespace() {
            end = abs_idx;
            break;
        }

        if ch == '\\'
            && let Some((_, next_ch)) = chars.peek()
            && next_ch.is_whitespace()
        {
            path.push(*next_ch);
            chars.next();
            continue;
        }

        path.push(ch);
    }

    if path.is_empty() {
        (ParsedReference::Missing, end)
    } else {
        (ParsedReference::Path(path), end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileReferenceContent {
    Text(String),
    NonTextOrUnreadable,
}

fn read_file_reference_content(path: &std::path::Path, max_bytes: usize) -> FileReferenceContent {
    use std::io::Read;

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return FileReferenceContent::NonTextOrUnreadable,
    };
    let mut limited = file.take((max_bytes + 1) as u64);
    let mut bytes = Vec::with_capacity(max_bytes + 1);
    if limited.read_to_end(&mut bytes).is_err() {
        return FileReferenceContent::NonTextOrUnreadable;
    }

    let truncated = bytes.len() > max_bytes;
    let bytes = if truncated {
        &bytes[..max_bytes]
    } else {
        bytes.as_slice()
    };

    let content = match std::str::from_utf8(bytes) {
        Ok(content) => content.to_string(),
        Err(err) if truncated && err.error_len().is_none() => {
            let valid_up_to = err.valid_up_to();
            match std::str::from_utf8(&bytes[..valid_up_to]) {
                Ok(content) => content.to_string(),
                Err(_) => return FileReferenceContent::NonTextOrUnreadable,
            }
        }
        Err(_) => return FileReferenceContent::NonTextOrUnreadable,
    };

    if truncated {
        FileReferenceContent::Text(format!(
            "{content}...\n[truncated at {}KB]",
            max_bytes / 1024
        ))
    } else {
        FileReferenceContent::Text(content)
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
    use super::{
        CommandCompletion, FileReferenceContent, complete_command_name, expand_file_references,
        longest_common_prefix, parse_file_references, read_file_reference_content,
    };

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
        assert_eq!(
            complete_command_name("cle", true),
            CommandCompletion::Available("ar".to_string())
        );
        // "mo" uniquely matches "model", adds space since it expects arg
        assert_eq!(
            complete_command_name("mo", true),
            CommandCompletion::Available("del ".to_string())
        );
    }

    #[test]
    fn complete_command_name_with_slash_prefix() {
        // Leading slash should be stripped for matching
        assert_eq!(
            complete_command_name("/cle", true),
            CommandCompletion::Available("ar".to_string())
        );
    }

    #[test]
    fn complete_command_name_ambiguous_extends_to_lcp() {
        // Ambiguous: "c" matches multiple commands, so no completion is applied.
        assert_eq!(
            complete_command_name("c", true),
            CommandCompletion::Unavailable
        );
    }

    #[test]
    fn complete_command_name_no_match() {
        assert_eq!(
            complete_command_name("xyz", true),
            CommandCompletion::Unavailable
        );
    }

    #[test]
    fn complete_command_name_exact_match() {
        // Already complete, nothing to add (except space for commands expecting args)
        assert_eq!(
            complete_command_name("clear", true),
            CommandCompletion::Available(String::new())
        );
    }

    #[test]
    fn complete_command_name_case_insensitive() {
        assert_eq!(
            complete_command_name("CLE", true),
            CommandCompletion::Available("ar".to_string())
        );
        assert_eq!(
            complete_command_name("QU", true),
            CommandCompletion::Available("it".to_string())
        );
    }

    #[test]
    fn parse_file_references_supports_quoted_paths_with_spaces() {
        let refs = parse_file_references(r#"read @"docs/My File.md" and @src/lib.rs"#);
        assert_eq!(refs, vec!["docs/My File.md", "src/lib.rs"]);
    }

    #[test]
    fn parse_file_references_supports_escaped_spaces() {
        let refs = parse_file_references(r"read @docs/My\ File.md and @src/lib.rs");
        assert_eq!(refs, vec!["docs/My File.md", "src/lib.rs"]);
    }

    #[test]
    fn parse_file_references_ignores_embedded_at_signs() {
        let refs = parse_file_references("email me at user@example.com");
        assert_eq!(refs, Vec::<String>::new());
    }

    const TEST_MAX_BYTES: usize = 200 * 1024;

    #[test]
    fn read_file_reference_content_truncates_at_utf8_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("utf8.txt");

        let mut content = "a".repeat(TEST_MAX_BYTES - 1);
        content.push('é');
        content.push('b');
        std::fs::write(&path, content).expect("write file");

        let loaded = match read_file_reference_content(&path, TEST_MAX_BYTES) {
            FileReferenceContent::Text(content) => content,
            FileReferenceContent::NonTextOrUnreadable => panic!("load content"),
        };
        let marker = format!("...\n[truncated at {}KB]", TEST_MAX_BYTES / 1024);

        assert!(loaded.ends_with(&marker));
        let prefix = loaded.strip_suffix(&marker).expect("truncate marker");
        assert_eq!(prefix.len(), TEST_MAX_BYTES - 1);
        assert!(prefix.chars().all(|c| c == 'a'));
    }

    fn test_sandbox(dir: &std::path::Path) -> forge_tools::sandbox::Sandbox {
        forge_tools::sandbox::Sandbox::new(
            vec![dir.to_path_buf()],
            vec![
                "**/.env".to_string(),
                "**/.env.*".to_string(),
                "**/*.pem".to_string(),
            ],
            false,
        )
        .unwrap()
    }

    fn test_file_cache() -> std::sync::Arc<tokio::sync::Mutex<forge_tools::ToolFileCache>> {
        std::sync::Arc::new(tokio::sync::Mutex::new(
            forge_tools::ToolFileCache::default(),
        ))
    }

    #[test]
    fn expand_file_references_reads_quoted_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("My File.md");
        std::fs::write(&path, "hello world").expect("write file");

        let sandbox = test_sandbox(dir.path());
        let cache = test_file_cache();

        let input = r#"inspect @"My File.md""#;

        let result = expand_file_references(input, &sandbox, TEST_MAX_BYTES, &cache);
        assert!(result.denied.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(result.file_sections.len(), 1);
        assert!(result.file_sections[0].contains("hello world"));
    }

    #[test]
    fn expand_file_references_denies_env_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".env"), "SECRET=123").expect("write file");

        let sandbox = test_sandbox(dir.path());
        let cache = test_file_cache();

        let env_path = dir.path().join(".env");
        let raw_path = env_path.to_string_lossy().to_string();
        let escaped_path = raw_path.replace('\\', "\\\\").replace('"', "\\\"");
        let input = format!(r#"read @"{escaped_path}""#);

        let result = expand_file_references(&input, &sandbox, TEST_MAX_BYTES, &cache);
        assert_eq!(result.denied.len(), 1);
        assert!(result.file_sections.is_empty());
    }

    #[test]
    fn expand_file_references_denies_parent_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sandbox = test_sandbox(dir.path());
        let cache = test_file_cache();

        let input = "@../../../etc/passwd";
        let result = expand_file_references(input, &sandbox, TEST_MAX_BYTES, &cache);
        assert_eq!(result.denied.len(), 1);
        assert!(result.file_sections.is_empty());
    }

    #[test]
    fn expand_file_references_populates_file_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cached.rs");
        std::fs::write(&path, "fn main() {}\n").expect("write file");

        let sandbox = test_sandbox(dir.path());
        let cache = test_file_cache();

        let input = r#"read @"cached.rs""#;

        let result = expand_file_references(input, &sandbox, TEST_MAX_BYTES, &cache);
        assert!(result.denied.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(result.file_sections.len(), 1);

        let cache_guard = cache.try_lock().unwrap();
        assert!(!cache_guard.is_empty());
    }
}

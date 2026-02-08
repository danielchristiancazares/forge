//! Command processing for the App.

use super::{
    ContextManager, ContextUsageStatus, EnteredCommand, ModelLimitsSource, SessionChangeLog,
    state::{
        ActiveStream, DistillationStart, JournalStatus, OperationState, ToolLoopPhase,
        ToolLoopState, ToolRecoveryDecision,
    },
};

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub palette_label: &'static str,
    pub help_label: &'static str,
    pub description: &'static str,
}

const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec {
        palette_label: "q, quit",
        help_label: "q(uit)",
        description: "Exit the application",
    },
    CommandSpec {
        palette_label: "clear",
        help_label: "clear",
        description: "Clear conversation history",
    },
    CommandSpec {
        palette_label: "cancel",
        help_label: "cancel",
        description: "Cancel streaming, tool execution, or distillation",
    },
    CommandSpec {
        palette_label: "tool <id> <result>",
        help_label: "tool",
        description: "Submit a tool result",
    },
    CommandSpec {
        palette_label: "model <name>",
        help_label: "model",
        description: "Change the model",
    },
    CommandSpec {
        palette_label: "ctx",
        help_label: "ctx",
        description: "Show context usage",
    },
    CommandSpec {
        palette_label: "jrnl",
        help_label: "jrnl",
        description: "Show stream journal stats",
    },
    CommandSpec {
        palette_label: "distill",
        help_label: "distill",
        description: "Distill older messages",
    },
    CommandSpec {
        palette_label: "screen",
        help_label: "screen",
        description: "Toggle fullscreen/inline mode",
    },
    CommandSpec {
        palette_label: "rewind <id|last> [code|conversation|both]",
        help_label: "rewind",
        description: "Rewind to an automatic checkpoint",
    },
    CommandSpec {
        palette_label: "undo",
        help_label: "undo",
        description: "Undo the last user turn (rewind to the last turn checkpoint)",
    },
    CommandSpec {
        palette_label: "retry",
        help_label: "retry",
        description: "Undo the last user turn and restore its prompt into the input box",
    },
    CommandSpec {
        palette_label: "problems, diag",
        help_label: "problems",
        description: "Show LSP diagnostics (compiler errors/warnings)",
    },
];

#[must_use]
pub fn command_specs() -> &'static [CommandSpec] {
    COMMAND_SPECS
}

// ============================================================================
// Command name normalization / aliasing (used by parsing + tab completion)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Quit,
    Clear,
    Model,
    Context,
    Journal,
    Distill,
    Cancel,
    Screen,
    Rewind,
    Undo,
    Retry,
    Problems,
}

impl CommandKind {
    pub(crate) fn expects_arg(self) -> bool {
        matches!(self, Self::Model | Self::Rewind)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandAlias {
    pub name: &'static str,
    pub kind: CommandKind,
}

const COMMAND_ALIASES: &[CommandAlias] = &[
    CommandAlias {
        name: "q",
        kind: CommandKind::Quit,
    },
    CommandAlias {
        name: "quit",
        kind: CommandKind::Quit,
    },
    CommandAlias {
        name: "clear",
        kind: CommandKind::Clear,
    },
    CommandAlias {
        name: "model",
        kind: CommandKind::Model,
    },
    CommandAlias {
        name: "ctx",
        kind: CommandKind::Context,
    },
    CommandAlias {
        name: "context",
        kind: CommandKind::Context,
    },
    CommandAlias {
        name: "jrnl",
        kind: CommandKind::Journal,
    },
    CommandAlias {
        name: "journal",
        kind: CommandKind::Journal,
    },
    CommandAlias {
        name: "distill",
        kind: CommandKind::Distill,
    },
    CommandAlias {
        name: "cancel",
        kind: CommandKind::Cancel,
    },
    CommandAlias {
        name: "screen",
        kind: CommandKind::Screen,
    },
    CommandAlias {
        name: "rw",
        kind: CommandKind::Rewind,
    },
    CommandAlias {
        name: "rewind",
        kind: CommandKind::Rewind,
    },
    CommandAlias {
        name: "undo",
        kind: CommandKind::Undo,
    },
    CommandAlias {
        name: "retry",
        kind: CommandKind::Retry,
    },
    CommandAlias {
        name: "problems",
        kind: CommandKind::Problems,
    },
    CommandAlias {
        name: "diag",
        kind: CommandKind::Problems,
    },
];

pub(crate) fn command_aliases() -> &'static [CommandAlias] {
    COMMAND_ALIASES
}

pub(crate) fn normalize_command_name(raw: &str) -> Option<CommandKind> {
    let token = raw.trim().trim_start_matches('/');
    if token.is_empty() {
        return None;
    }
    COMMAND_ALIASES
        .iter()
        .find(|alias| alias.name.eq_ignore_ascii_case(token))
        .map(|alias| alias.kind)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command<'a> {
    Quit,
    Clear,
    Model(Option<&'a str>),
    Context,
    Journal,
    Distill,
    Cancel,
    Screen,
    Rewind {
        target: Option<&'a str>,
        scope: Option<&'a str>,
    },
    Undo,
    Retry,
    Problems,
    Unknown(&'a str),
    Empty,
}

impl<'a> Command<'a> {
    /// Accepts optional leading `/` and is case-insensitive (e.g., `/Clear`, `MODEL`).
    pub(crate) fn parse(raw: &'a str) -> Self {
        let parts: Vec<&str> = raw.split_whitespace().collect();

        let Some(cmd_raw) = parts.first().copied() else {
            return Command::Empty;
        };

        // Treat a bare "/" as empty, since the UI already renders the prefix.
        let trimmed = cmd_raw.trim();
        let token = trimmed.trim_start_matches('/');
        if token.is_empty() {
            return Command::Empty;
        }

        let Some(kind) = normalize_command_name(trimmed) else {
            return Command::Unknown(cmd_raw);
        };

        match kind {
            CommandKind::Quit => Command::Quit,
            CommandKind::Clear => Command::Clear,
            CommandKind::Model => Command::Model(parts.get(1).copied()),
            CommandKind::Context => Command::Context,
            CommandKind::Journal => Command::Journal,
            CommandKind::Distill => Command::Distill,
            CommandKind::Cancel => Command::Cancel,
            CommandKind::Screen => Command::Screen,
            CommandKind::Rewind => Command::Rewind {
                target: parts.get(1).copied(),
                scope: parts.get(2).copied(),
            },
            CommandKind::Undo => Command::Undo,
            CommandKind::Retry => Command::Retry,
            CommandKind::Problems => Command::Problems,
        }
    }
}

impl super::App {
    pub fn cancel_active_operation(&mut self) -> bool {
        match self.replace_with_idle() {
            OperationState::Streaming(active) => {
                active.abort_handle().abort();

                // Clean up tool batch if journaled
                if let ActiveStream::Journaled { tool_batch_id, .. } = &active
                    && let Err(e) = self.tool_journal.discard_batch(*tool_batch_id)
                {
                    tracing::warn!("Failed to discard tool batch on cancel: {e}");
                }

                // Clean up journal state
                if let Err(e) = active.into_journal().discard(&mut self.stream_journal) {
                    tracing::warn!("Failed to discard stream journal on cancel: {e}");
                }

                // Rollback the user message since no response was generated
                self.rollback_pending_user_message();

                self.push_notification("Streaming cancelled");
                true
            }
            OperationState::ToolLoop(state) => {
                let ToolLoopState { batch, phase } = *state;
                if let ToolLoopPhase::Executing(exec) = &phase {
                    exec.spawned.abort();
                }
                self.cancel_tool_batch(
                    batch.assistant_text,
                    batch.calls,
                    batch.results,
                    batch.model,
                    batch.step_id,
                    batch.journal_status,
                    batch.turn,
                    batch.thinking_message,
                );
                self.push_notification("Tool execution cancelled");
                true
            }
            OperationState::ToolRecovery(state) => {
                self.commit_recovered_tool_batch(state, ToolRecoveryDecision::Discard);
                true
            }
            OperationState::Distilling(state) => {
                state.task().handle.abort();
                if state.has_queued_message() {
                    self.rollback_pending_user_message();
                }
                self.push_notification("Distillation cancelled");
                true
            }
            OperationState::Idle => false,
        }
    }

    pub fn process_command(&mut self, command: EnteredCommand) {
        // Record command to history for Up/Down navigation
        if !command.raw.is_empty() {
            self.record_command(&command.raw);
        }

        let parsed = Command::parse(&command.raw);

        match parsed {
            Command::Quit => {
                self.request_quit();
            }
            Command::Clear => {
                let state = self.replace_with_idle();
                match state {
                    OperationState::Streaming(active) => {
                        active.abort_handle().abort();
                        // Clean up tool batch if journaled
                        if let ActiveStream::Journaled { tool_batch_id, .. } = &active
                            && let Err(e) = self.tool_journal.discard_batch(*tool_batch_id)
                        {
                            tracing::warn!("Failed to discard tool batch on clear: {e}");
                        }
                        if let Err(e) = active.into_journal().discard(&mut self.stream_journal) {
                            tracing::warn!("Failed to discard stream journal on clear: {e}");
                        }
                    }
                    OperationState::ToolLoop(state) => {
                        let ToolLoopState { batch, phase } = *state;
                        if let ToolLoopPhase::Executing(exec) = &phase {
                            exec.spawned.abort();
                        }
                        if let JournalStatus::Persisted(id) = batch.journal_status
                            && let Err(e) = self.tool_journal.discard_batch(id)
                        {
                            tracing::warn!("Failed to discard tool batch on clear: {e}");
                        }
                        self.discard_journal_step(batch.step_id);
                    }
                    OperationState::ToolRecovery(state) => {
                        // RecoveredToolBatch always has a valid batch_id from the journal
                        if let Err(e) = self.tool_journal.discard_batch(state.batch.batch_id) {
                            tracing::warn!("Failed to discard recovered tool batch on clear: {e}");
                        }
                        self.discard_journal_step(state.step_id);
                    }
                    OperationState::Distilling(state) => {
                        state.task().handle.abort();
                    }
                    OperationState::Idle => {}
                }

                self.display.clear();
                self.display_version = self.display_version.wrapping_add(1);
                self.pending_user_message = None; // Clear pending message tracking
                self.session_changes = SessionChangeLog::default();
                self.context_manager = ContextManager::new(self.model.clone());
                self.context_manager
                    .set_output_limit(self.output_limits.max_output_tokens());
                self.invalidate_usage_cache();
                self.autosave_history(); // Persist cleared state immediately
                self.push_notification("Conversation cleared");
                self.view.clear_transcript = true;
            }
            Command::Model(model_arg) => {
                if let Some(reason) = self.busy_reason() {
                    self.push_notification(format!(
                        "Cannot change model while {reason}. Cancel or wait for it to finish."
                    ));
                    return;
                }
                if let Some(model_name) = model_arg {
                    let provider = self.provider();
                    match provider.parse_model(model_name) {
                        Ok(model) => {
                            self.set_model(model);
                            self.push_notification(format!("Model set to: {}", self.model));
                        }
                        Err(e) => {
                            self.push_notification(format!("Invalid model: {e}"));
                        }
                    }
                } else {
                    // Enter model selection mode with TUI list
                    self.enter_model_select_mode();
                }
            }
            Command::Context => {
                let usage_status = self.context_usage_status();
                let (usage, needs_distillation, recent_too_large) = match &usage_status {
                    ContextUsageStatus::Ready(usage) => (usage, None, None),
                    ContextUsageStatus::NeedsDistillation { usage, needed } => {
                        (usage, Some(needed), None)
                    }
                    ContextUsageStatus::RecentMessagesTooLarge {
                        usage,
                        required_tokens,
                        budget_tokens,
                    } => (usage, None, Some((*required_tokens, *budget_tokens))),
                };
                let format_k = |n: u32| -> String {
                    if n >= 1_000_000 {
                        format!("{:.1}M", n as f32 / 1_000_000.0)
                    } else if n >= 1000 {
                        format!("{:.1}k", n as f32 / 1000.0)
                    } else {
                        n.to_string()
                    }
                };
                let limits = self.context_manager.current_limits();
                let limits_source = match self.context_manager.current_limits_source() {
                    ModelLimitsSource::Override => "override".to_string(),
                    ModelLimitsSource::Catalog(model) => model.model_id().to_string(),
                };
                let memory_flag = if self.memory_enabled() { "on" } else { "off" };
                let pct = usage.percentage();
                let remaining = (100.0_f32 - pct).clamp(0.0, 100.0);
                let status_suffix = if let Some((required, budget)) = recent_too_large {
                    format!(" │ ERROR: recent msgs ({required} tokens) > budget ({budget} tokens)")
                } else {
                    needs_distillation.map_or(String::new(), |needed| {
                        format!(
                            " │ Distill: {} msgs (~{} tokens)",
                            needed.messages_to_distill.len(),
                            needed.excess_tokens
                        )
                    })
                };
                self.push_notification(format!(
                    "Memory: {} │ Context: {remaining:.0}% left │ Used: {} │ Budget(effective): {} │ Window(raw): {} │ Output(reserved): {} │ Model: {} │ Limits: {}{}",
                    memory_flag,
                    format_k(usage.used_tokens),
                    format_k(usage.budget_tokens),
                    format_k(limits.context_window()),
                    format_k(self.output_limits.max_output_tokens()),
                    self.context_manager.current_model(),
                    limits_source,
                    status_suffix,
                ));
            }
            Command::Journal => match self.stream_journal.stats() {
                Ok(stats) => {
                    let streaming = matches!(self.state, OperationState::Streaming(_));
                    let state_desc = if streaming {
                        "streaming"
                    } else if stats.unsealed_entries > 0 {
                        "unsealed"
                    } else {
                        "idle"
                    };
                    self.push_notification(format!(
                        "Journal: {} │ Total: {} │ Sealed: {} │ Unsealed: {} │ Steps: {}",
                        state_desc,
                        stats.total_entries,
                        stats.sealed_entries,
                        stats.unsealed_entries,
                        stats.current_step_id,
                    ));
                }
                Err(e) => {
                    self.push_notification(format!("Journal error: {e}"));
                }
            },
            Command::Distill => {
                self.push_notification("Distilling older messages...");
                let result = self.try_start_distillation(None);
                if matches!(result, DistillationStart::NotNeeded) {
                    self.push_notification("No messages need distillation");
                }
                // If Failed, try_start_distillation already set status
            }
            Command::Cancel => {
                self.cancel_active_operation();
            }
            Command::Screen => {
                self.view.toggle_screen_mode = true;
            }
            Command::Rewind { target, scope } => {
                if let Some(reason) = self.busy_reason() {
                    self.push_notification(format!("Cannot rewind while {reason}."));
                    return;
                }

                // /rewind or /rewind list shows available checkpoints
                if matches!(target, Some("list" | "ls")) || target.is_none() {
                    self.show_checkpoint_list();
                    return;
                }

                let Some(scope) = crate::checkpoints::RewindScope::parse(scope) else {
                    self.push_notification("Invalid scope. Use: code | conversation | both");
                    return;
                };

                let Some(proof) = self.parse_checkpoint_target(target) else {
                    return;
                };

                if let Err(msg) = self.apply_rewind(proof, scope) {
                    self.push_notification(msg);
                }
            }
            Command::Undo => {
                if let Some(reason) = self.busy_reason() {
                    self.push_notification(format!("Cannot undo while {reason}."));
                    return;
                }

                let Some(proof) = self.prepare_latest_turn_checkpoint() else {
                    return;
                };

                if let Err(msg) =
                    self.apply_rewind(proof, crate::checkpoints::RewindScope::Conversation)
                {
                    self.push_notification(msg);
                }
            }
            Command::Retry => {
                if let Some(reason) = self.busy_reason() {
                    self.push_notification(format!("Cannot retry while {reason}."));
                    return;
                }

                let Some(proof) = self.prepare_latest_turn_checkpoint() else {
                    return;
                };

                // Capture the user prompt we are about to rewind away (best-effort).
                let conversation_len = {
                    let cp = self.checkpoints.checkpoint(proof);
                    cp.conversation_len()
                };
                let prompt = self
                    .context_manager
                    .history()
                    .entries()
                    .get(conversation_len..)
                    .and_then(|slice| {
                        slice.iter().find_map(|entry| match entry.message() {
                            forge_types::Message::User(_) => {
                                Some(entry.message().content().to_string())
                            }
                            _ => None,
                        })
                    });

                if let Err(msg) =
                    self.apply_rewind(proof, crate::checkpoints::RewindScope::Conversation)
                {
                    self.push_notification(msg);
                    return;
                }

                // Restore prompt into draft and drop the user into Insert mode.
                if let Some(text) = prompt {
                    self.input.draft_mut().set_text(text);
                    self.input = std::mem::take(&mut self.input).into_insert();
                }
            }
            Command::Problems => {
                let snapshot = self.lsp_snapshot.clone();
                if snapshot.is_empty() {
                    self.push_notification("No diagnostics");
                } else {
                    let mut lines = vec![format!(
                        "Diagnostics: {} error(s), {} warning(s)",
                        snapshot.error_count(),
                        snapshot.warning_count()
                    )];
                    for (path, diags) in snapshot.files() {
                        for diag in diags {
                            lines.push(format!("  {}", diag.display_with_path(path)));
                        }
                    }
                    self.push_notification(lines.join("\n"));
                }
            }
            Command::Unknown(cmd) => {
                self.push_notification(format!("Unknown command: {cmd}"));
            }
            Command::Empty => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Command parsing tests
    // ========================================================================

    #[test]
    fn parse_quit_commands() {
        assert_eq!(Command::parse("q"), Command::Quit);
        assert_eq!(Command::parse("quit"), Command::Quit);
        assert_eq!(Command::parse("q extra"), Command::Quit);
    }

    #[test]
    fn parse_clear_command() {
        assert_eq!(Command::parse("clear"), Command::Clear);
    }

    #[test]
    fn parse_model_command() {
        assert_eq!(Command::parse("model"), Command::Model(None));
        assert_eq!(
            Command::parse("model claude-opus-4-6"),
            Command::Model(Some("claude-opus-4-6"))
        );
        assert_eq!(
            Command::parse("model gpt-5.2"),
            Command::Model(Some("gpt-5.2"))
        );
    }

    #[test]
    fn parse_context_commands() {
        assert_eq!(Command::parse("context"), Command::Context);
        assert_eq!(Command::parse("ctx"), Command::Context);
    }

    #[test]
    fn parse_journal_commands() {
        assert_eq!(Command::parse("journal"), Command::Journal);
        assert_eq!(Command::parse("jrnl"), Command::Journal);
    }

    #[test]
    fn parse_distill_commands() {
        assert_eq!(Command::parse("distill"), Command::Distill);
    }

    #[test]
    fn parse_cancel_command() {
        assert_eq!(Command::parse("cancel"), Command::Cancel);
    }

    #[test]
    fn parse_screen_command() {
        assert_eq!(Command::parse("screen"), Command::Screen);
    }

    #[test]
    fn parse_empty_command() {
        assert_eq!(Command::parse(""), Command::Empty);
        assert_eq!(Command::parse("   "), Command::Empty);
    }

    #[test]
    fn parse_unknown_command() {
        assert_eq!(Command::parse("foobar"), Command::Unknown("foobar"));
        assert_eq!(Command::parse("xyz 123"), Command::Unknown("xyz"));
    }

    #[test]
    fn parse_case_insensitive_and_slash_prefix() {
        // Commands should be case-insensitive
        assert_eq!(Command::parse("QUIT"), Command::Quit);
        assert_eq!(Command::parse("Clear"), Command::Clear);
        assert_eq!(Command::parse("MODEL"), Command::Model(None));

        // Leading slash should be accepted
        assert_eq!(Command::parse("/quit"), Command::Quit);
        assert_eq!(Command::parse("/clear"), Command::Clear);
        assert_eq!(
            Command::parse("/model gpt-5"),
            Command::Model(Some("gpt-5"))
        );

        // Bare "/" should be treated as empty
        assert_eq!(Command::parse("/"), Command::Empty);
    }

    #[test]
    fn parse_rewind_command() {
        assert_eq!(
            Command::parse("rewind"),
            Command::Rewind {
                target: None,
                scope: None
            }
        );
        assert_eq!(
            Command::parse("rewind last code"),
            Command::Rewind {
                target: Some("last"),
                scope: Some("code")
            }
        );
        assert_eq!(
            Command::parse("rw 3 conversation"),
            Command::Rewind {
                target: Some("3"),
                scope: Some("conversation")
            }
        );
        assert_eq!(
            Command::parse("rewind list"),
            Command::Rewind {
                target: Some("list"),
                scope: None
            }
        );
    }

    #[test]
    fn parse_undo_and_retry_commands() {
        assert_eq!(Command::parse("undo"), Command::Undo);
        assert_eq!(Command::parse("retry"), Command::Retry);
    }
}

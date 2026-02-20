//! Command processing for the App.

use super::{
    ContextManager, ContextUsageStatus, EnteredCommand, ModelLimitsSource, PlanState,
    state::{
        ActiveStream, DistillationStart, OperationState, ToolLoopPhase, ToolLoopState,
        ToolRecoveryDecision,
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
        palette_label: "settings, config",
        help_label: "settings",
        description: "Open the settings modal",
    },
    CommandSpec {
        palette_label: "runtime",
        help_label: "runtime",
        description: "Show active runtime configuration",
    },
    CommandSpec {
        palette_label: "resolve",
        help_label: "resolve",
        description: "Show resolved configuration cascade",
    },
    CommandSpec {
        palette_label: "validate",
        help_label: "validate",
        description: "Show configuration validation dashboard",
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
    CommandSpec {
        palette_label: "plan [clear]",
        help_label: "plan",
        description: "Show plan status or clear active plan",
    },
    CommandSpec {
        palette_label: "export [path]",
        help_label: "export",
        description: "Export conversation to JSON",
    },
];

#[must_use]
pub fn command_specs() -> &'static [CommandSpec] {
    COMMAND_SPECS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Quit,
    Clear,
    Model,
    Settings,
    Runtime,
    Resolve,
    Validate,
    Context,
    Journal,
    Distill,
    Cancel,
    Rewind,
    Undo,
    Retry,
    Problems,
    Plan,
    Export,
}

impl CommandKind {
    pub(crate) fn expects_arg(self) -> bool {
        matches!(self, Self::Model | Self::Rewind | Self::Export)
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
        name: "settings",
        kind: CommandKind::Settings,
    },
    CommandAlias {
        name: "config",
        kind: CommandKind::Settings,
    },
    CommandAlias {
        name: "runtime",
        kind: CommandKind::Runtime,
    },
    CommandAlias {
        name: "resolve",
        kind: CommandKind::Resolve,
    },
    CommandAlias {
        name: "validate",
        kind: CommandKind::Validate,
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
    CommandAlias {
        name: "plan",
        kind: CommandKind::Plan,
    },
    CommandAlias {
        name: "export",
        kind: CommandKind::Export,
    },
];

pub(crate) fn command_aliases() -> &'static [CommandAlias] {
    COMMAND_ALIASES
}

pub(crate) enum NormalizedCommandName<'a> {
    Blank,
    Known(CommandKind),
    Unrecognized(&'a str),
}

pub(crate) fn normalize_command_name(raw: &str) -> NormalizedCommandName<'_> {
    let token = raw.trim().trim_start_matches('/');
    if token.is_empty() {
        return NormalizedCommandName::Blank;
    }
    if let Some(kind) = COMMAND_ALIASES
        .iter()
        .find(|alias| alias.name.eq_ignore_ascii_case(token))
        .map(|alias| alias.kind)
    {
        NormalizedCommandName::Known(kind)
    } else {
        NormalizedCommandName::Unrecognized(token)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ModelCommand<'a> {
    OpenSelector,
    SetByName(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RewindTarget<'a> {
    List,
    Latest,
    Id(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RewindScopeInput<'a> {
    DefaultBoth,
    Named(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PlanCommand<'a> {
    Show,
    Clear,
    InvalidSubcommand(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExportDestination<'a> {
    TimestampedDefault,
    ExplicitPath(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextUsageCondition {
    Ready,
    NeedsCompaction,
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetryPromptCapture {
    Found(String),
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParseIssue<'a> {
    BlankInput,
    UnrecognizedCommand(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command<'a> {
    Quit,
    Clear,
    Model(ModelCommand<'a>),
    Settings,
    Runtime,
    Resolve,
    Validate,
    Context,
    Journal,
    Distill,
    Cancel,
    Rewind {
        target: RewindTarget<'a>,
        scope: RewindScopeInput<'a>,
    },
    Undo,
    Retry,
    Problems,
    Plan(PlanCommand<'a>),
    Export(ExportDestination<'a>),
    ParseIssue(ParseIssue<'a>),
}

impl<'a> Command<'a> {
    /// Accepts optional leading `/` and is case-insensitive (e.g., `/Clear`, `MODEL`).
    pub(crate) fn parse(raw: &'a str) -> Self {
        let parts: Vec<&str> = raw.split_whitespace().collect();

        let Some(cmd_raw) = parts.first().copied() else {
            return Command::ParseIssue(ParseIssue::BlankInput);
        };

        // Treat a bare "/" as empty, since the UI already renders the prefix.
        let trimmed = cmd_raw.trim();
        let kind = match normalize_command_name(trimmed) {
            NormalizedCommandName::Blank => return Command::ParseIssue(ParseIssue::BlankInput),
            NormalizedCommandName::Known(kind) => kind,
            NormalizedCommandName::Unrecognized(token) => {
                return Command::ParseIssue(ParseIssue::UnrecognizedCommand(token));
            }
        };

        match kind {
            CommandKind::Quit => Command::Quit,
            CommandKind::Clear => Command::Clear,
            CommandKind::Model => {
                if let Some(model_name) = parts.get(1).copied() {
                    Command::Model(ModelCommand::SetByName(model_name))
                } else {
                    Command::Model(ModelCommand::OpenSelector)
                }
            }
            CommandKind::Settings => Command::Settings,
            CommandKind::Runtime => Command::Runtime,
            CommandKind::Resolve => Command::Resolve,
            CommandKind::Validate => Command::Validate,
            CommandKind::Context => Command::Context,
            CommandKind::Journal => Command::Journal,
            CommandKind::Distill => Command::Distill,
            CommandKind::Cancel => Command::Cancel,
            CommandKind::Rewind => Command::Rewind {
                target: if let Some(target) = parts.get(1).copied() {
                    match target {
                        "list" | "ls" => RewindTarget::List,
                        "last" | "latest" => RewindTarget::Latest,
                        other => RewindTarget::Id(other),
                    }
                } else {
                    RewindTarget::List
                },
                scope: if let Some(scope) = parts.get(2).copied() {
                    RewindScopeInput::Named(scope)
                } else {
                    RewindScopeInput::DefaultBoth
                },
            },
            CommandKind::Undo => Command::Undo,
            CommandKind::Retry => Command::Retry,
            CommandKind::Problems => Command::Problems,
            CommandKind::Plan => {
                if let Some("clear") = parts.get(1).copied() {
                    Command::Plan(PlanCommand::Clear)
                } else if let Some(other) = parts.get(1).copied() {
                    Command::Plan(PlanCommand::InvalidSubcommand(other))
                } else {
                    Command::Plan(PlanCommand::Show)
                }
            }
            CommandKind::Export => {
                if let Some(path) = parts.get(1).copied() {
                    Command::Export(ExportDestination::ExplicitPath(path))
                } else {
                    Command::Export(ExportDestination::TimestampedDefault)
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
struct ConversationExport<'a> {
    model: &'a str,
    exported_at: String,
    messages: Vec<&'a forge_types::Message>,
}

impl super::App {
    pub fn cancel_active_operation(&mut self) -> bool {
        match self.op_take() {
            OperationState::Streaming(active) => {
                active.abort_handle().abort();

                if let ActiveStream::Journaled { tool_batch_id, .. } = &active
                    && let Err(e) = self.runtime.tool_journal.discard_batch(*tool_batch_id)
                {
                    tracing::warn!("Failed to discard tool batch on cancel: {e}");
                }

                if let Err(e) = active
                    .into_journal()
                    .discard(&mut self.runtime.stream_journal)
                {
                    tracing::warn!("Failed to discard stream journal on cancel: {e}");
                }

                // Rollback the user message since no response was generated.
                self.rollback_pending_user_message();

                // Discard any partial usage from the cancelled stream so it
                // doesn't leak into the next turn's accounting.
                self.core.turn_usage = None;

                self.push_notification("Streaming cancelled");
                true
            }
            OperationState::ToolLoop(state) => {
                let ToolLoopState { batch, phase } = *state;
                if let ToolLoopPhase::Executing(exec) = &phase {
                    exec.spawned.abort();
                }
                self.cancel_tool_batch(batch);
                self.push_notification("Tool execution cancelled");
                true
            }
            OperationState::PlanApproval(state) => {
                match &state.kind {
                    crate::state::PlanApprovalKind::Create => {
                        self.core.plan_state = PlanState::Inactive;
                    }
                    crate::state::PlanApprovalKind::Edit { .. } => {}
                }
                self.cancel_tool_batch(state.batch);
                self.push_notification("Plan approval cancelled");
                true
            }
            OperationState::ToolRecovery(state) => {
                self.commit_recovered_tool_batch(state, ToolRecoveryDecision::Discard);
                true
            }
            OperationState::RecoveryBlocked(state) => {
                // Safety state: don't allow "cancel" to silently clear recovery blocks.
                self.op_restore(OperationState::RecoveryBlocked(state));
                self.push_notification(
                    "Recovery is blocked due to journal errors. Run /clear to reset.",
                );
                false
            }
            OperationState::Distilling(state) => {
                state.task().handle.abort();
                if state.has_queued_message() {
                    self.rollback_pending_user_message();
                }
                self.push_notification("Compaction cancelled");
                true
            }
            OperationState::Idle => false,
        }
    }

    pub fn process_command(&mut self, command: EnteredCommand) {
        if !command.raw.is_empty() {
            self.record_command(&command.raw);
        }

        let parsed = Command::parse(&command.raw);

        match parsed {
            Command::Quit => {
                self.request_quit();
            }
            Command::Clear => {
                let state = self.op_take();
                match state {
                    OperationState::Streaming(active) => {
                        active.abort_handle().abort();
                        if let ActiveStream::Journaled { tool_batch_id, .. } = &active
                            && let Err(e) = self.runtime.tool_journal.discard_batch(*tool_batch_id)
                        {
                            tracing::warn!("Failed to discard tool batch on clear: {e}");
                        }
                        if let Err(e) = active
                            .into_journal()
                            .discard(&mut self.runtime.stream_journal)
                        {
                            tracing::warn!("Failed to discard stream journal on clear: {e}");
                        }
                    }
                    OperationState::ToolLoop(state) => {
                        let ToolLoopState { batch, phase } = *state;
                        if let ToolLoopPhase::Executing(exec) = &phase {
                            exec.spawned.abort();
                        }
                        if let Err(e) = self
                            .runtime
                            .tool_journal
                            .discard_batch(batch.journal_status.batch_id())
                        {
                            tracing::warn!("Failed to discard tool batch on clear: {e}");
                        }
                        self.discard_journal_step(batch.step_id);
                    }
                    OperationState::PlanApproval(state) => {
                        match &state.kind {
                            crate::state::PlanApprovalKind::Create => {
                                self.core.plan_state = PlanState::Inactive;
                            }
                            crate::state::PlanApprovalKind::Edit { .. } => {}
                        }
                        if let Err(e) = self
                            .runtime
                            .tool_journal
                            .discard_batch(state.batch.journal_status.batch_id())
                        {
                            tracing::warn!("Failed to discard tool batch on clear: {e}");
                        }
                        self.discard_journal_step(state.batch.step_id);
                    }
                    OperationState::ToolRecovery(state) => {
                        // RecoveredToolBatch always has a valid batch_id from the journal
                        if let Err(e) = self
                            .runtime
                            .tool_journal
                            .discard_batch(state.batch.batch_id)
                        {
                            tracing::warn!("Failed to discard recovered tool batch on clear: {e}");
                        }
                        self.discard_journal_step(state.step_id);
                    }
                    OperationState::RecoveryBlocked(_) | OperationState::Idle => {
                        // No in-flight async work; /clear below resets history/display state.
                    }
                    OperationState::Distilling(state) => {
                        state.task().handle.abort();
                    }
                }

                self.ui.display.clear();
                self.core.pending_user_message = None;
                self.core.session_changes = crate::SessionChangeLog::default();
                self.core.context_manager = ContextManager::new(self.core.model.clone());
                self.core
                    .context_manager
                    .set_output_limit(self.core.output_limits.max_output_tokens());
                if let Some(step_id) = self.runtime.pending_stream_cleanup.take() {
                    self.discard_journal_step(step_id);
                }
                self.runtime.pending_stream_cleanup_failures = 0;
                if let Some(batch_id) = self.runtime.pending_tool_cleanup.take()
                    && let Err(e) = self.runtime.tool_journal.discard_batch(batch_id)
                {
                    tracing::warn!("Failed to discard pending tool batch on clear: {e}");
                }
                self.runtime.pending_tool_cleanup_failures = 0;
                self.core.tool_gate.clear();
                self.runtime.provider_runtime.openai_previous_response_id = None;
                self.invalidate_usage_cache();
                self.autosave_history();
                self.ui.view.clear_transcript = true;
                self.op_transition(self.idle_state());
            }
            Command::Model(model_cmd) => {
                match self.busy_state() {
                    super::BusyState::Idle => {}
                    busy => {
                        let reason = busy.reason();
                        self.push_notification(format!(
                            "Cannot change model while {reason}. Cancel or wait for it to finish."
                        ));
                        return;
                    }
                }
                match model_cmd {
                    ModelCommand::SetByName(model_name) => {
                        let provider = self.provider();
                        match provider.parse_model(model_name) {
                            Ok(model) => {
                                self.set_model(model);
                                self.push_notification(format!(
                                    "Model set to: {}",
                                    self.core.model
                                ));
                            }
                            Err(e) => {
                                self.push_notification(format!("Invalid model: {e}"));
                            }
                        }
                    }
                    ModelCommand::OpenSelector => {
                        self.enter_model_select_mode();
                    }
                }
            }
            Command::Settings => {
                self.enter_settings_mode();
                self.push_settings_next_turn_guardrail();
            }
            Command::Runtime => {
                self.enter_runtime_mode();
            }
            Command::Resolve => {
                self.enter_resolve_mode();
            }
            Command::Validate => {
                self.enter_validate_mode();
            }
            Command::Context => {
                let usage_status = self.context_usage_status();
                let (usage, condition) = match &usage_status {
                    ContextUsageStatus::Ready(usage) => (usage, ContextUsageCondition::Ready),
                    ContextUsageStatus::NeedsCompaction { usage } => {
                        (usage, ContextUsageCondition::NeedsCompaction)
                    }
                    ContextUsageStatus::RecentMessagesTooLarge {
                        usage,
                        required_tokens,
                        budget_tokens,
                    } => (
                        usage,
                        ContextUsageCondition::RecentMessagesTooLarge {
                            required_tokens: *required_tokens,
                            budget_tokens: *budget_tokens,
                        },
                    ),
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
                let limits = self.core.context_manager.current_limits();
                let limits_source = match self.core.context_manager.current_limits_source() {
                    ModelLimitsSource::Override => "override".to_string(),
                    ModelLimitsSource::Catalog(model) => model.model_id().to_string(),
                };
                let memory_flag = if self.memory_enabled() { "on" } else { "off" };
                let pct = usage.percentage();
                let remaining = (100.0_f32 - pct).clamp(0.0, 100.0);
                let status_suffix = match condition {
                    ContextUsageCondition::RecentMessagesTooLarge {
                        required_tokens,
                        budget_tokens,
                    } => format!(
                        " │ ERROR: recent msgs ({required_tokens} tokens) > budget ({budget_tokens} tokens)"
                    ),
                    ContextUsageCondition::NeedsCompaction => " │ Compaction needed".to_string(),
                    ContextUsageCondition::Ready => String::new(),
                };
                self.push_notification(format!(
                    "Memory: {} │ Context: {remaining:.0}% left │ Used: {} │ Budget(effective): {} │ Window(raw): {} │ Output(reserved): {} │ Model: {} │ Limits: {}{}",
                    memory_flag,
                    format_k(usage.used_tokens),
                    format_k(usage.budget_tokens),
                    format_k(limits.context_window()),
                    format_k(self.core.output_limits.max_output_tokens()),
                    self.core.context_manager.current_model(),
                    limits_source,
                    status_suffix,
                ));
            }
            Command::Journal => match self.runtime.stream_journal.stats() {
                Ok(stats) => {
                    let streaming = self.streaming().is_some();
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
                    self.push_notification("Context fits — no compaction needed");
                }
            }
            Command::Cancel => {
                self.cancel_active_operation();
            }
            Command::Rewind { target, scope } => {
                match self.busy_state() {
                    super::BusyState::Idle => {}
                    busy => {
                        let reason = busy.reason();
                        self.push_notification(format!("Cannot rewind while {reason}."));
                        return;
                    }
                }

                let scope = match scope {
                    RewindScopeInput::DefaultBoth => super::checkpoints::RewindScope::Both,
                    RewindScopeInput::Named(raw) => {
                        match super::checkpoints::RewindScope::parse(raw) {
                            super::checkpoints::RewindScopeParse::Valid(scope) => scope,
                            super::checkpoints::RewindScopeParse::Invalid => {
                                self.push_notification(
                                    "Invalid scope. Use: code | conversation | both",
                                );
                                return;
                            }
                        }
                    }
                };

                let target = match target {
                    RewindTarget::List => {
                        self.show_checkpoint_list();
                        return;
                    }
                    RewindTarget::Latest => super::checkpoints::CheckpointTarget::Latest,
                    RewindTarget::Id(raw_id) => super::checkpoints::CheckpointTarget::Id(raw_id),
                };

                let proof = match self.parse_checkpoint_target(target) {
                    super::checkpoints::CheckpointTargetResolution::Resolved(proof) => proof,
                    super::checkpoints::CheckpointTargetResolution::Rejected => return,
                };

                if let Err(msg) = self.apply_rewind(proof, scope) {
                    self.push_notification(msg);
                }
            }
            Command::Undo => {
                match self.busy_state() {
                    super::BusyState::Idle => {}
                    busy => {
                        let reason = busy.reason();
                        self.push_notification(format!("Cannot undo while {reason}."));
                        return;
                    }
                }

                let proof = match self.prepare_latest_turn_checkpoint() {
                    super::checkpoints::PreparedRewindLookup::Prepared(proof) => proof,
                    super::checkpoints::PreparedRewindLookup::Missing => return,
                };

                if let Err(msg) =
                    self.apply_rewind(proof, super::checkpoints::RewindScope::Conversation)
                {
                    self.push_notification(msg);
                }
            }
            Command::Retry => {
                match self.busy_state() {
                    super::BusyState::Idle => {}
                    busy => {
                        let reason = busy.reason();
                        self.push_notification(format!("Cannot retry while {reason}."));
                        return;
                    }
                }

                let proof = match self.prepare_latest_turn_checkpoint() {
                    super::checkpoints::PreparedRewindLookup::Prepared(proof) => proof,
                    super::checkpoints::PreparedRewindLookup::Missing => return,
                };

                // Capture the user prompt we are about to rewind away (best-effort).
                let conversation_len = {
                    let cp = self.core.checkpoints.checkpoint(proof);
                    cp.conversation_len()
                };
                let mut prompt = RetryPromptCapture::Missing;
                if let Some(slice) = self
                    .core
                    .context_manager
                    .history()
                    .entries()
                    .get(conversation_len..)
                {
                    for entry in slice {
                        if let forge_types::Message::User(_) = entry.message() {
                            prompt =
                                RetryPromptCapture::Found(entry.message().content().to_string());
                            break;
                        }
                    }
                }

                if let Err(msg) =
                    self.apply_rewind(proof, super::checkpoints::RewindScope::Conversation)
                {
                    self.push_notification(msg);
                    return;
                }

                if let RetryPromptCapture::Found(text) = prompt {
                    self.ui.input.draft_mut().set_text(text);
                    self.ui.input = std::mem::take(&mut self.ui.input).into_insert();
                }
            }
            Command::Problems => {
                let snapshot = self.runtime.lsp_runtime.snapshot.clone();
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
            Command::Plan(plan_cmd) => match plan_cmd {
                PlanCommand::Show => {
                    let msg = match &self.core.plan_state {
                        PlanState::Inactive => "No active plan.".to_string(),
                        PlanState::Proposed(plan) => {
                            format!("[Proposed — awaiting approval]\n\n{}", plan.render())
                        }
                        PlanState::Active(plan) => plan.render(),
                    };
                    self.push_notification(msg);
                }
                PlanCommand::Clear => {
                    self.core.plan_state = PlanState::Inactive;
                    self.save_plan();
                    self.core.tool_gate.clear();
                    self.push_notification("Plan cleared.");
                }
                PlanCommand::InvalidSubcommand(other) => {
                    self.push_notification(format!(
                        "Unknown /plan subcommand: {other}. Usage: /plan [clear]"
                    ));
                }
            },
            Command::Export(export_destination) => {
                let path = match export_destination {
                    ExportDestination::ExplicitPath(path) => std::path::PathBuf::from(path),
                    ExportDestination::TimestampedDefault => {
                        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
                        std::path::PathBuf::from(format!("forge-export-{ts}.json"))
                    }
                };

                let messages: Vec<_> = self
                    .core
                    .context_manager
                    .history()
                    .entries()
                    .iter()
                    .map(forge_context::HistoryEntry::message)
                    .collect();

                let export = ConversationExport {
                    model: self.core.model.as_str(),
                    exported_at: chrono::Utc::now().to_rfc3339(),
                    messages,
                };

                match serde_json::to_string_pretty(&export) {
                    Ok(json) => {
                        let opts = forge_utils::AtomicWriteOptions {
                            sync_all: true,
                            dir_sync: false,
                            unix_mode: Some(0o600),
                        };
                        match forge_utils::atomic_write_with_options(&path, json.as_bytes(), opts) {
                            Ok(()) => {
                                self.push_notification(format!("Exported to {}", path.display()));
                            }
                            Err(e) => {
                                self.push_notification(format!("Export failed: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        self.push_notification(format!("Export serialization failed: {e}"));
                    }
                }
            }
            Command::ParseIssue(parse_issue) => match parse_issue {
                ParseIssue::BlankInput => {}
                ParseIssue::UnrecognizedCommand(cmd) => {
                    self.push_notification(format!("Unknown command: {cmd}"));
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Command, ExportDestination, ModelCommand, ParseIssue, PlanCommand, RewindScopeInput,
        RewindTarget,
    };

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
        assert_eq!(
            Command::parse("model"),
            Command::Model(ModelCommand::OpenSelector)
        );
        assert_eq!(
            Command::parse("model claude-opus-4-6"),
            Command::Model(ModelCommand::SetByName("claude-opus-4-6"))
        );
        assert_eq!(
            Command::parse("model gpt-5.2"),
            Command::Model(ModelCommand::SetByName("gpt-5.2"))
        );
    }

    #[test]
    fn parse_settings_commands() {
        assert_eq!(Command::parse("settings"), Command::Settings);
        assert_eq!(Command::parse("config"), Command::Settings);
    }

    #[test]
    fn parse_observability_commands() {
        assert_eq!(Command::parse("runtime"), Command::Runtime);
        assert_eq!(Command::parse("resolve"), Command::Resolve);
        assert_eq!(Command::parse("validate"), Command::Validate);
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
    fn parse_empty_command() {
        assert_eq!(
            Command::parse(""),
            Command::ParseIssue(ParseIssue::BlankInput)
        );
        assert_eq!(
            Command::parse("   "),
            Command::ParseIssue(ParseIssue::BlankInput)
        );
    }

    #[test]
    fn parse_unknown_command() {
        assert_eq!(
            Command::parse("foobar"),
            Command::ParseIssue(ParseIssue::UnrecognizedCommand("foobar"))
        );
        assert_eq!(
            Command::parse("xyz 123"),
            Command::ParseIssue(ParseIssue::UnrecognizedCommand("xyz"))
        );
    }

    #[test]
    fn parse_case_insensitive_and_slash_prefix() {
        assert_eq!(Command::parse("QUIT"), Command::Quit);
        assert_eq!(Command::parse("Clear"), Command::Clear);
        assert_eq!(
            Command::parse("MODEL"),
            Command::Model(ModelCommand::OpenSelector)
        );
        assert_eq!(Command::parse("Settings"), Command::Settings);
        assert_eq!(Command::parse("Validate"), Command::Validate);

        assert_eq!(Command::parse("/quit"), Command::Quit);
        assert_eq!(Command::parse("/clear"), Command::Clear);
        assert_eq!(Command::parse("/config"), Command::Settings);
        assert_eq!(Command::parse("/runtime"), Command::Runtime);
        assert_eq!(Command::parse("/resolve"), Command::Resolve);
        assert_eq!(
            Command::parse("/model gpt-5"),
            Command::Model(ModelCommand::SetByName("gpt-5"))
        );

        assert_eq!(
            Command::parse("/"),
            Command::ParseIssue(ParseIssue::BlankInput)
        );
    }

    #[test]
    fn parse_rewind_command() {
        assert_eq!(
            Command::parse("rewind"),
            Command::Rewind {
                target: RewindTarget::List,
                scope: RewindScopeInput::DefaultBoth
            }
        );
        assert_eq!(
            Command::parse("rewind last code"),
            Command::Rewind {
                target: RewindTarget::Latest,
                scope: RewindScopeInput::Named("code")
            }
        );
        assert_eq!(
            Command::parse("rw 3 conversation"),
            Command::Rewind {
                target: RewindTarget::Id("3"),
                scope: RewindScopeInput::Named("conversation")
            }
        );
        assert_eq!(
            Command::parse("rewind list"),
            Command::Rewind {
                target: RewindTarget::List,
                scope: RewindScopeInput::DefaultBoth
            }
        );
    }

    #[test]
    fn parse_plan_commands() {
        assert_eq!(Command::parse("plan"), Command::Plan(PlanCommand::Show));
        assert_eq!(Command::parse("/plan"), Command::Plan(PlanCommand::Show));
        assert_eq!(
            Command::parse("plan clear"),
            Command::Plan(PlanCommand::Clear)
        );
        assert_eq!(
            Command::parse("/plan clear"),
            Command::Plan(PlanCommand::Clear)
        );
        assert_eq!(
            Command::parse("plan bogus"),
            Command::Plan(PlanCommand::InvalidSubcommand("bogus"))
        );
    }

    #[test]
    fn parse_undo_and_retry_commands() {
        assert_eq!(Command::parse("undo"), Command::Undo);
        assert_eq!(Command::parse("retry"), Command::Retry);
    }

    #[test]
    fn parse_export_command() {
        assert_eq!(
            Command::parse("export"),
            Command::Export(ExportDestination::TimestampedDefault)
        );
        assert_eq!(
            Command::parse("/export"),
            Command::Export(ExportDestination::TimestampedDefault)
        );
    }

    #[test]
    fn parse_export_command_with_path() {
        assert_eq!(
            Command::parse("export out.json"),
            Command::Export(ExportDestination::ExplicitPath("out.json"))
        );
        assert_eq!(
            Command::parse("/export /tmp/dump.json"),
            Command::Export(ExportDestination::ExplicitPath("/tmp/dump.json"))
        );
    }
}

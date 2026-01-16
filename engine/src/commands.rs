//! Command processing for the App.
//!
//! This module handles slash commands like /quit, /clear, /model, etc.

use super::{
    ContextManager, ContextUsageStatus, EnteredCommand, ModelLimitsSource, ModelNameKind, Provider,
    state::{OperationState, SummarizationStart, ToolLoopPhase, ToolRecoveryDecision},
};

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub palette_label: &'static str,
    pub help_label: &'static str,
    pub description: &'static str,
    pub show_in_help: bool,
}

const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec {
        palette_label: "q, quit",
        help_label: "q(uit)",
        description: "Exit the application",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "clear",
        help_label: "clear",
        description: "Clear conversation history",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "cancel",
        help_label: "cancel",
        description: "Cancel streaming or tool execution",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "tool <id> <result>",
        help_label: "tool",
        description: "Submit a tool result",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "tools",
        help_label: "tools",
        description: "Show tool status",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "model <name>",
        help_label: "model",
        description: "Change the model",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "p, provider <name>",
        help_label: "p(rovider)",
        description: "Switch provider (claude/gpt)",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "ctx",
        help_label: "ctx",
        description: "Show context usage",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "jrnl",
        help_label: "jrnl",
        description: "Show stream journal stats",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "sum",
        help_label: "sum",
        description: "Summarize older messages",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "screen",
        help_label: "screen",
        description: "Toggle fullscreen/inline mode",
        show_in_help: true,
    },
    CommandSpec {
        palette_label: "help",
        help_label: "help",
        description: "Show available commands",
        show_in_help: false,
    },
];

#[must_use]
pub fn command_specs() -> &'static [CommandSpec] {
    COMMAND_SPECS
}

#[must_use]
pub fn command_help_summary() -> String {
    let labels: Vec<&str> = COMMAND_SPECS
        .iter()
        .filter(|spec| spec.show_in_help)
        .map(|spec| spec.help_label)
        .collect();
    format!("Commands: /{}", labels.join(", /"))
}

/// Parsed command with typed arguments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command<'a> {
    Quit,
    Clear,
    Model(Option<&'a str>),
    Provider(Option<&'a str>),
    Context,
    Journal,
    Summarize,
    Cancel,
    Screen,
    Tools,
    Help,
    Unknown(&'a str),
    Empty,
}

impl<'a> Command<'a> {
    /// Parse a raw command string into a typed Command.
    pub(crate) fn parse(raw: &'a str) -> Self {
        let parts: Vec<&str> = raw.split_whitespace().collect();

        match parts.first().copied() {
            Some("q" | "quit") => Command::Quit,
            Some("clear") => Command::Clear,
            Some("model") => Command::Model(parts.get(1).copied()),
            Some("provider" | "p") => Command::Provider(parts.get(1).copied()),
            Some("context" | "ctx") => Command::Context,
            Some("journal" | "jrnl") => Command::Journal,
            Some("summarize" | "sum") => Command::Summarize,
            Some("cancel") => Command::Cancel,
            Some("screen") => Command::Screen,
            Some("tools") => Command::Tools,
            Some("help") => Command::Help,
            Some(cmd) => Command::Unknown(cmd),
            None => Command::Empty,
        }
    }
}

impl super::App {
    /// Cancel the current active operation (streaming/tools), if any.
    /// Returns true if a cancellation happened.
    pub fn cancel_active_operation(&mut self) -> bool {
        match self.replace_with_idle() {
            OperationState::Streaming(active) => {
                active.abort_handle.abort();

                // Clean up journal state
                let _ = active.journal.discard(&mut self.stream_journal);
                self.set_status_warning("Streaming cancelled");
                true
            }
            OperationState::ToolLoop(state) => {
                if let ToolLoopPhase::Executing(exec) = &state.phase
                    && let Some(handle) = &exec.abort_handle
                {
                    handle.abort();
                }
                self.cancel_tool_batch(
                    state.batch.assistant_text,
                    state.batch.calls,
                    state.batch.results,
                    state.batch.model,
                    state.batch.step_id,
                    state.batch.batch_id,
                );
                self.set_status_warning("Tool execution cancelled");
                true
            }
            OperationState::ToolRecovery(state) => {
                self.commit_recovered_tool_batch(state, ToolRecoveryDecision::Discard);
                true
            }
            other => {
                self.state = other;
                self.set_status_warning("No active stream to cancel");
                false
            }
        }
    }

    /// Process a slash command entered by the user.
    pub fn process_command(&mut self, command: EnteredCommand) {
        let parsed = Command::parse(&command.raw);

        match parsed {
            Command::Quit => {
                self.request_quit();
            }
            Command::Clear => {
                let state = self.replace_with_idle();
                match state {
                    OperationState::Streaming(active) => {
                        active.abort_handle.abort();
                        let _ = active.journal.discard(&mut self.stream_journal);
                    }
                    OperationState::ToolLoop(state) => {
                        if let ToolLoopPhase::Executing(exec) = &state.phase
                            && let Some(handle) = &exec.abort_handle
                        {
                            handle.abort();
                        }
                        if state.batch.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(state.batch.batch_id);
                        }
                        self.discard_journal_step(state.batch.step_id);
                    }
                    OperationState::ToolRecovery(state) => {
                        if state.batch.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(state.batch.batch_id);
                        }
                        self.discard_journal_step(state.step_id);
                    }
                    OperationState::Summarizing(state) => {
                        state.task.handle.abort();
                    }
                    OperationState::SummarizingWithQueued(state) => {
                        state.task.handle.abort();
                    }
                    OperationState::SummarizationRetry(_)
                    | OperationState::SummarizationRetryWithQueued(_)
                    | OperationState::Idle => {}
                }

                self.display.clear();
                self.display_version = self.display_version.wrapping_add(1);
                self.context_manager = ContextManager::new(self.model.as_str());
                self.context_manager
                    .set_output_limit(self.output_limits.max_output_tokens());
                self.invalidate_usage_cache();
                self.autosave_history(); // Persist cleared state immediately
                self.set_status_success("Conversation cleared");
                self.view.clear_transcript = true;
            }
            Command::Model(model_arg) => {
                if let Some(model_name) = model_arg {
                    let provider = self.provider();
                    match provider.parse_model(model_name) {
                        Ok(model) => {
                            let kind = model.kind();
                            self.set_model(model);
                            let suffix = match kind {
                                ModelNameKind::Known => "",
                                ModelNameKind::Unverified => " (unverified; limits may fallback)",
                            };
                            self.set_status_success(format!(
                                "Model set to: {}{}",
                                self.model, suffix
                            ));
                        }
                        Err(e) => {
                            self.set_status_error(format!("Invalid model: {e}"));
                        }
                    }
                } else {
                    // Enter model selection mode with TUI list
                    self.enter_model_select_mode();
                }
            }
            Command::Provider(provider_arg) => {
                if let Some(provider_str) = provider_arg {
                    if let Some(provider) = Provider::parse(provider_str) {
                        self.set_provider(provider);
                        let has_key = self.current_api_key().is_some();
                        let status = if has_key {
                            format!("Switched to {} ({})", provider.display_name(), self.model)
                        } else {
                            format!(
                                "Switched to {} - No API key! Set {}",
                                provider.display_name(),
                                provider.env_var()
                            )
                        };
                        if has_key {
                            self.set_status_success(status);
                        } else {
                            self.set_status_warning(status);
                        }
                    } else {
                        self.set_status_error(format!("Unknown provider: {provider_str}"));
                    }
                } else {
                    let provider = self.provider();
                    let providers: Vec<&str> = Provider::all()
                        .iter()
                        .map(forge_types::Provider::as_str)
                        .collect();
                    self.set_status(format!(
                        "Current: {} ({}) │ Providers: {} │ Models: {}",
                        provider.display_name(),
                        self.model,
                        providers.join(", "),
                        provider.available_models().join(", ")
                    ));
                }
            }
            Command::Context => {
                let usage_status = self.context_usage_status();
                let (usage, needs_summary, recent_too_large) = match &usage_status {
                    ContextUsageStatus::Ready(usage) => (usage, None, None),
                    ContextUsageStatus::NeedsSummarization { usage, needed } => {
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
                    ModelLimitsSource::Prefix(prefix) => prefix.to_string(),
                    ModelLimitsSource::DefaultFallback => "fallback(default)".to_string(),
                };
                let context_flag = if self.context_infinity_enabled() {
                    "on"
                } else {
                    "off"
                };
                let pct = usage.percentage();
                let remaining = (100.0 - pct).clamp(0.0, 100.0);
                let status_suffix = if let Some((required, budget)) = recent_too_large {
                    format!(" │ ERROR: recent msgs ({required} tokens) > budget ({budget} tokens)")
                } else {
                    needs_summary.map_or(String::new(), |needed| {
                        format!(
                            " │ Summarize: {} msgs (~{} tokens)",
                            needed.messages_to_summarize.len(),
                            needed.excess_tokens
                        )
                    })
                };
                self.set_status(format!(
                    "ContextInfinity: {} │ Context: {remaining:.0}% left │ Used: {} │ Budget(effective): {} │ Window(raw): {} │ Max output: {} │ Model: {} │ Limits: {}{}",
                    context_flag,
                    format_k(usage.used_tokens),
                    format_k(usage.budget_tokens),
                    format_k(limits.context_window()),
                    format_k(limits.max_output()),
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
                    self.set_status(format!(
                        "Journal: {} │ Total: {} │ Sealed: {} │ Unsealed: {} │ Steps: {}",
                        state_desc,
                        stats.total_entries,
                        stats.sealed_entries,
                        stats.unsealed_entries,
                        stats.current_step_id,
                    ));
                }
                Err(e) => {
                    self.set_status_error(format!("Journal error: {e}"));
                }
            },
            Command::Summarize => {
                if self.context_infinity_enabled() {
                    self.set_status("Summarizing older messages...");
                    let result = self.start_summarization_with_attempt(None, 1);
                    if matches!(result, SummarizationStart::NotNeeded) {
                        self.set_status("No messages need summarization");
                    }
                    // If Failed, start_summarization_with_attempt already set status
                } else {
                    self.set_status_warning("ContextInfinity disabled: summarization unavailable");
                }
            }
            Command::Cancel => {
                self.cancel_active_operation();
            }
            Command::Screen => {
                self.view.toggle_screen_mode = true;
            }
            Command::Tools => {
                if self.tool_definitions.is_empty() {
                    self.set_status_warning("No tools configured. Add tools to config.toml");
                } else {
                    let tools_list: Vec<&str> = self
                        .tool_definitions
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect();
                    self.set_status(format!(
                        "Tools ({}): {}",
                        self.tool_definitions.len(),
                        tools_list.join(", ")
                    ));
                }
            }
            Command::Help => {
                self.set_status(command_help_summary());
            }
            Command::Unknown(cmd) => {
                self.set_status_warning(format!("Unknown command: {cmd}"));
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
            Command::parse("model claude-sonnet-4-5-20250929"),
            Command::Model(Some("claude-sonnet-4-5-20250929"))
        );
        assert_eq!(
            Command::parse("model gpt-5.2"),
            Command::Model(Some("gpt-5.2"))
        );
    }

    #[test]
    fn parse_provider_commands() {
        assert_eq!(Command::parse("provider"), Command::Provider(None));
        assert_eq!(Command::parse("p"), Command::Provider(None));
        assert_eq!(
            Command::parse("provider claude"),
            Command::Provider(Some("claude"))
        );
        assert_eq!(Command::parse("p gpt"), Command::Provider(Some("gpt")));
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
    fn parse_summarize_commands() {
        assert_eq!(Command::parse("summarize"), Command::Summarize);
        assert_eq!(Command::parse("sum"), Command::Summarize);
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
    fn parse_tools_command() {
        assert_eq!(Command::parse("tools"), Command::Tools);
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(Command::parse("help"), Command::Help);
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
    fn parse_case_sensitive() {
        // Commands should be case-sensitive
        assert_eq!(Command::parse("QUIT"), Command::Unknown("QUIT"));
        assert_eq!(Command::parse("Clear"), Command::Unknown("Clear"));
        assert_eq!(Command::parse("MODEL"), Command::Unknown("MODEL"));
    }
}

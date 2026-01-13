//! Command processing for the App.
//!
//! This module handles slash commands like /quit, /clear, /model, etc.

use super::{
    ContextManager, ContextUsageStatus, EnteredCommand, ModelLimitsSource, ModelNameKind, Provider,
    ToolResult,
    state::{OperationState, SummarizationStart, ToolLoopPhase, ToolRecoveryDecision},
};

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
    Tool(ToolCommand<'a>),
    Tools,
    Help,
    Unknown(&'a str),
    Empty,
}

/// Tool command variants.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ToolCommand<'a> {
    Success { id: &'a str, content: &'a str },
    Error { id: &'a str, message: &'a str },
    Usage,
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
            Some("tool") => {
                if parts.len() < 3 {
                    Command::Tool(ToolCommand::Usage)
                } else if parts[1] == "error" && parts.len() >= 4 {
                    // Find the position after "tool error <id> " to get the rest as content
                    let prefix_len = "tool error ".len() + parts[2].len() + 1;
                    let message = if raw.len() > prefix_len {
                        raw[prefix_len..].trim_start()
                    } else {
                        ""
                    };
                    Command::Tool(ToolCommand::Error {
                        id: parts[2],
                        message,
                    })
                } else {
                    // Find the position after "tool <id> " to get the rest as content
                    let prefix_len = "tool ".len() + parts[1].len() + 1;
                    let content = if raw.len() > prefix_len {
                        raw[prefix_len..].trim_start()
                    } else {
                        ""
                    };
                    Command::Tool(ToolCommand::Success {
                        id: parts[1],
                        content,
                    })
                }
            }
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
            OperationState::AwaitingToolResults(pending) => {
                self.cancel_tool_batch(
                    pending.assistant_text,
                    pending.pending_calls,
                    pending.results,
                    pending.model,
                    pending.step_id,
                    pending.batch_id,
                );
                self.set_status_warning("Tool results cancelled");
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
                    OperationState::AwaitingToolResults(pending) => {
                        // Clear pending tool execution and discard the journal step.
                        self.discard_journal_step(pending.step_id);
                        if pending.batch_id != 0 {
                            let _ = self.tool_journal.discard_batch(pending.batch_id);
                        }
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
                    "ContextInfinity: {} │ Context: {} │ Model: {} │ Limits: {} │ Window: {}k │ Budget: {}k │ Max output: {}k{}",
                    context_flag,
                    usage.format_compact(),
                    self.context_manager.current_model(),
                    limits_source,
                    limits.context_window() / 1000,
                    limits.effective_input_budget() / 1000,
                    limits.max_output() / 1000,
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
            Command::Tool(tool_cmd) => match tool_cmd {
                ToolCommand::Usage => {
                    self.set_status(
                        "Usage: /tool <call_id> <result> or /tool error <call_id> <message>",
                    );
                }
                ToolCommand::Error { id, message } => {
                    let result = ToolResult::error(id.to_string(), message.to_string());
                    match self.submit_tool_result(result) {
                        Ok(true) => self.set_status_success("All tool results submitted"),
                        Ok(false) => {} // Status already set by submit_tool_result
                        Err(e) => self.set_status_error(format!("Tool error: {e}")),
                    }
                }
                ToolCommand::Success { id, content } => {
                    let result = ToolResult::success(id.to_string(), content.to_string());
                    match self.submit_tool_result(result) {
                        Ok(true) => self.set_status_success("All tool results submitted"),
                        Ok(false) => {} // Status already set by submit_tool_result
                        Err(e) => self.set_status_error(format!("Tool error: {e}")),
                    }
                }
            },
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
                self.set_status(
                    "Commands: /q(uit), /clear, /cancel, /model, /p(rovider), /ctx, /jrnl, /sum, /screen, /tool, /tools",
                );
            }
            Command::Unknown(cmd) => {
                self.set_status_warning(format!("Unknown command: {cmd}"));
            }
            Command::Empty => {}
        }
    }
}

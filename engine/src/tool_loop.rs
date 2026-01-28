//! Tool execution loop for the App.
//!
//! This module contains all tool loop logic including:
//! - Planning and validation of tool calls
//! - Spawning and polling tool execution
//! - Approval workflow handling
//! - Tool batch commit and recovery

use std::collections::{HashSet, VecDeque};

use futures_util::future::Abortable;
use tokio::sync::mpsc;

use forge_context::{ContextUsageStatus, StepId, ToolBatchId};
use forge_types::{ModelName, ToolCall, ToolResult, sanitize_terminal_text};

use crate::input_modes::{ChangeRecorder, TurnChangeReport, TurnContext};
use crate::state::{
    ActiveToolExecution, ApprovalState, OperationState, ToolBatch, ToolLoopPhase, ToolLoopState,
    ToolPlan, ToolRecoveryDecision, ToolRecoveryState,
};
use crate::tools::{self, ConfirmationRequest};
use crate::util;
use crate::{
    ApiConfig, App, DEFAULT_TOOL_CAPACITY_BYTES, Message, NonEmptyString, QueuedUserMessage,
    TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

use futures_util::future::AbortHandle;

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_tool_calls(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        pre_resolved: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        tool_batch_id: Option<ToolBatchId>,
        turn: TurnContext,
    ) {
        if tool_calls.is_empty() {
            self.finish_turn(turn);
            return;
        }

        // Try to update existing batch or create new one
        let mut batch_id = tool_batch_id;
        if let Some(id) = batch_id
            && let Err(e) = self.tool_journal.update_assistant_text(id, &assistant_text)
        {
            tracing::warn!("Tool journal update failed: {e}");
            self.push_notification(format!("Tool journal error: {e}"));
            batch_id = None;
        }
        if batch_id.is_none() {
            batch_id =
                match self
                    .tool_journal
                    .begin_batch(model.as_str(), &assistant_text, &tool_calls)
                {
                    Ok(id) => Some(id),
                    Err(e) => {
                        tracing::warn!("Tool journal begin failed: {e}");
                        self.push_notification(format!("Tool journal error: {e}"));
                        None
                    }
                };
        }

        self.start_tool_loop(
            assistant_text,
            tool_calls,
            pre_resolved,
            model,
            step_id,
            batch_id,
            turn,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn start_tool_loop(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        pre_resolved: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        batch_id: Option<ToolBatchId>,
        turn: TurnContext,
    ) {
        let next_iteration = self.tool_iterations.saturating_add(1);
        if next_iteration > self.tool_settings.limits.max_tool_iterations_per_user_turn {
            let mut results = pre_resolved;
            results.extend(tool_calls.iter().map(|call| {
                ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    "Max tool iterations reached",
                )
            }));
            if let Some(id) = batch_id {
                for result in &results {
                    let _ = self.tool_journal.record_result(id, result);
                }
            }
            self.commit_tool_batch(
                assistant_text,
                tool_calls,
                results,
                model,
                step_id,
                batch_id,
                true,
                turn,
            );
            return;
        }
        self.tool_iterations = next_iteration;

        let plan = self.plan_tool_calls(&tool_calls, pre_resolved);
        if let Some(id) = batch_id {
            for result in &plan.pre_resolved {
                let _ = self.tool_journal.record_result(id, result);
            }
        }

        // Create an automatic checkpoint before any tool-driven file edits.
        // (QoL: enables /rewind <id> [code|conversation|both])
        self.maybe_create_checkpoint_for_tool_calls(
            plan.execute_now.iter().chain(plan.approval_calls.iter()),
        );

        let batch = ToolBatch {
            assistant_text,
            calls: tool_calls,
            results: plan.pre_resolved,
            model,
            step_id,
            batch_id,
            execute_now: plan.execute_now,
            approval_calls: plan.approval_calls,
            approval_requests: plan.approval_requests.clone(),
            turn,
        };
        let remaining_capacity_bytes = self.remaining_tool_capacity(&batch);

        if !plan.approval_requests.is_empty() {
            let approval = ApprovalState {
                requests: plan.approval_requests,
                selected: vec![true; batch.approval_requests.len()],
                cursor: 0,
                deny_confirm: false,
                expanded: None,
            };
            self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
                batch,
                phase: ToolLoopPhase::AwaitingApproval(approval),
            }));
            return;
        }

        let queue = batch.execute_now.clone();
        if queue.is_empty() {
            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                batch.results,
                batch.model,
                batch.step_id,
                batch.batch_id,
                true,
                batch.turn,
            );
            return;
        }

        let exec =
            self.spawn_tool_execution(queue, remaining_capacity_bytes, batch.turn.recorder());
        self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
            batch,
            phase: ToolLoopPhase::Executing(exec),
        }));
    }

    fn plan_tool_calls(&self, calls: &[ToolCall], mut pre_resolved: Vec<ToolResult>) -> ToolPlan {
        let mut execute_now = Vec::new();
        let mut approval_calls = Vec::new();
        let mut approval_requests = Vec::new();
        let mut pre_resolved_ids: HashSet<String> = pre_resolved
            .iter()
            .map(|result| result.tool_call_id.clone())
            .collect();
        let mut seen_ids = HashSet::new();
        let mut accepted = 0usize;

        for call in calls {
            if !seen_ids.insert(call.id.clone()) {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::DuplicateToolCallId {
                        id: call.id.clone(),
                    },
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }
            accepted += 1;
            if accepted > self.tool_settings.limits.max_tool_calls_per_batch {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::LimitsExceeded {
                        message: "Exceeded max tool calls per batch".to_string(),
                    }),
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }
            if pre_resolved_ids.contains(&call.id) {
                continue;
            }

            if self.tool_settings.policy.is_denylisted(&call.name) {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::Denylisted {
                        tool: call.name.clone(),
                    }),
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            let args_size = serde_json::to_vec(&call.arguments)
                .map(|v| v.len())
                .unwrap_or(0);
            if args_size > self.tool_settings.limits.max_tool_args_bytes {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::LimitsExceeded {
                        message: "Tool arguments too large".to_string(),
                    }),
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            if call.name == "apply_patch"
                && let Some(patch) = call.arguments.get("patch").and_then(|v| v.as_str())
                && patch.len() > self.tool_settings.patch_limits.max_patch_bytes
            {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::LimitsExceeded {
                        message: "Patch exceeds max_patch_bytes".to_string(),
                    }),
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            let exec = match self.tool_registry.lookup(&call.name) {
                Ok(exec) => exec,
                Err(err) => {
                    pre_resolved.push(tool_error_result(call, err));
                    pre_resolved_ids.insert(call.id.clone());
                    continue;
                }
            };

            if let Err(err) = tools::validate_args(&exec.schema(), &call.arguments) {
                pre_resolved.push(tool_error_result(call, err));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            if let Err(err) = preflight_sandbox(&self.tool_settings.sandbox, call) {
                pre_resolved.push(tool_error_result(call, err));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            if matches!(self.tool_settings.policy.mode, tools::ApprovalMode::Strict)
                && !self.tool_settings.policy.is_allowlisted(&call.name)
            {
                pre_resolved.push(tool_error_result(
                    call,
                    tools::ToolError::SandboxViolation(tools::DenialReason::Denylisted {
                        tool: call.name.clone(),
                    }),
                ));
                pre_resolved_ids.insert(call.id.clone());
                continue;
            }

            let allowlisted = self.tool_settings.policy.is_allowlisted(&call.name);
            let needs_confirmation = match self.tool_settings.policy.mode {
                tools::ApprovalMode::Permissive => exec.requires_approval(),
                tools::ApprovalMode::Strict => true, // All tools require approval
                tools::ApprovalMode::Default => {
                    exec.requires_approval() || (exec.is_side_effecting() && !allowlisted)
                }
            };

            if needs_confirmation {
                let summary = match exec.approval_summary(&call.arguments) {
                    Ok(summary) => summary,
                    Err(err) => {
                        pre_resolved.push(tool_error_result(call, err));
                        continue;
                    }
                };
                let summary = sanitize_terminal_text(&summary).into_owned();
                let summary = util::truncate_with_ellipsis(&summary, 200);
                approval_requests.push(ConfirmationRequest {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    summary,
                    risk_level: exec.risk_level(),
                    arguments: call.arguments.clone(),
                });
                approval_calls.push(call.clone());
            } else {
                execute_now.push(call.clone());
            }
        }

        ToolPlan {
            execute_now,
            approval_calls,
            approval_requests,
            pre_resolved,
        }
    }

    fn tool_capacity_bytes(&mut self) -> usize {
        let usage = match self.context_usage_status() {
            ContextUsageStatus::Ready(usage)
            | ContextUsageStatus::NeedsSummarization { usage, .. }
            | ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => usage,
        };

        if usage.budget_tokens == 0 {
            return DEFAULT_TOOL_CAPACITY_BYTES;
        }

        let available_tokens = usage
            .budget_tokens
            .saturating_sub(usage.used_tokens)
            .saturating_sub(TOOL_OUTPUT_SAFETY_MARGIN_TOKENS);
        if available_tokens == 0 {
            return 0;
        }

        (available_tokens as usize).saturating_mul(4)
    }

    fn remaining_tool_capacity(&mut self, batch: &ToolBatch) -> usize {
        let mut remaining = self.tool_capacity_bytes();
        for result in &batch.results {
            remaining = remaining.saturating_sub(result.content.len());
        }
        remaining
    }

    fn spawn_tool_execution(
        &self,
        queue: Vec<ToolCall>,
        initial_capacity_bytes: usize,
        turn_recorder: ChangeRecorder,
    ) -> ActiveToolExecution {
        let mut exec = ActiveToolExecution {
            queue: VecDeque::from(queue),
            current_call: None,
            join_handle: None,
            event_rx: None,
            abort_handle: None,
            output_lines: Vec::new(),
            remaining_capacity_bytes: initial_capacity_bytes,
            turn_recorder,
        };
        self.start_next_tool_call(&mut exec);
        exec
    }

    fn start_next_tool_call(&self, exec: &mut ActiveToolExecution) -> bool {
        let Some(call) = exec.queue.pop_front() else {
            return false;
        };

        exec.output_lines.clear();
        exec.current_call = Some(call.clone());

        let (event_tx, event_rx) = mpsc::channel(TOOL_EVENT_CHANNEL_CAPACITY);
        exec.event_rx = Some(event_rx);

        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        exec.abort_handle = Some(abort_handle.clone());

        let registry = self.tool_registry.clone();
        let settings = self.tool_settings.clone();
        let file_cache = self.tool_file_cache.clone();
        let librarian = self.librarian.clone();
        let working_dir = settings.sandbox.working_dir();
        let remaining_capacity = exec.remaining_capacity_bytes;
        let turn_recorder = exec.turn_recorder.clone();

        let handle = tokio::spawn(async move {
            use futures_util::FutureExt;
            let _ = event_tx
                .send(tools::ToolEvent::Started {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                })
                .await;

            let exec_ref = match registry.lookup(&call.name) {
                Ok(exec) => exec,
                Err(err) => {
                    let result = tool_error_result(&call, err);
                    let _ = event_tx
                        .send(tools::ToolEvent::Completed {
                            tool_call_id: call.id.clone(),
                        })
                        .await;
                    return result;
                }
            };

            let default_timeout = match call.name.as_str() {
                "read_file" | "apply_patch" => settings.timeouts.file_operations_timeout,
                "run_command" => settings.timeouts.shell_commands_timeout,
                _ => settings.timeouts.default_timeout,
            };

            let mut ctx = tools::ToolCtx {
                sandbox: settings.sandbox.clone(),
                abort: abort_handle,
                output_tx: event_tx.clone(),
                default_timeout,
                max_output_bytes: settings.max_output_bytes,
                available_capacity_bytes: remaining_capacity,
                tool_call_id: call.id.clone(),
                allow_truncation: true,
                working_dir,
                env_sanitizer: settings.env_sanitizer.clone(),
                file_cache,
                turn_changes: turn_recorder,
                librarian,
            };

            let timeout = exec_ref.timeout().unwrap_or(ctx.default_timeout);
            let exec_future = exec_ref.execute(call.arguments.clone(), &mut ctx);
            let exec_future = std::panic::AssertUnwindSafe(exec_future).catch_unwind();
            let exec_future = Abortable::new(exec_future, abort_registration);

            let result = match tokio::time::timeout(timeout, exec_future).await {
                Err(_) => tool_error_result(
                    &call,
                    tools::ToolError::Timeout {
                        tool: call.name.clone(),
                        elapsed: timeout,
                    },
                ),
                Ok(Err(_)) => tool_error_result(&call, tools::ToolError::Cancelled),
                Ok(Ok(Err(panic_payload))) => {
                    let panic_msg = panic_payload_to_string(&panic_payload);
                    let message = format!("Tool panicked: {panic_msg}");
                    ToolResult::error(
                        call.id.clone(),
                        call.name.clone(),
                        tools::sanitize_output(&message),
                    )
                }
                Ok(Ok(Ok(inner))) => match inner {
                    Ok(output) => {
                        let sanitized = tools::sanitize_output(&output);
                        let effective_max = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
                        let final_output = if ctx.allow_truncation {
                            tools::truncate_output(sanitized, effective_max)
                        } else {
                            sanitized
                        };
                        ToolResult::success(call.id.clone(), call.name.clone(), final_output)
                    }
                    Err(err) => tool_error_result(&call, err),
                },
            };

            let _ = event_tx
                .send(tools::ToolEvent::Completed {
                    tool_call_id: call.id.clone(),
                })
                .await;

            result
        });

        exec.join_handle = Some(handle);
        true
    }

    pub(crate) fn poll_tool_loop(&mut self) {
        use futures_util::future::FutureExt;

        let state = match std::mem::replace(&mut self.state, OperationState::Idle) {
            OperationState::ToolLoop(state) => *state,
            other => {
                self.state = other;
                return;
            }
        };
        let mut state = state;
        let mut completed: Option<ToolResult> = None;
        let mut should_commit = false;

        match &mut state.phase {
            ToolLoopPhase::AwaitingApproval(_) => {}
            ToolLoopPhase::Executing(exec) => {
                if let Some(rx) = exec.event_rx.as_mut() {
                    loop {
                        match rx.try_recv() {
                            Ok(event) => match event {
                                tools::ToolEvent::Started {
                                    tool_call_id,
                                    tool_name,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if is_current {
                                        exec.output_lines.push(format!(
                                            "▶ {} ({})",
                                            tools::sanitize_output(&tool_name),
                                            tool_call_id
                                        ));
                                    }
                                }
                                tools::ToolEvent::StdoutChunk {
                                    tool_call_id,
                                    chunk,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if !is_current {
                                        continue;
                                    }
                                    append_tool_output_lines(
                                        &mut exec.output_lines,
                                        &tools::sanitize_output(&chunk),
                                        None,
                                    );
                                }
                                tools::ToolEvent::StderrChunk {
                                    tool_call_id,
                                    chunk,
                                } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if !is_current {
                                        continue;
                                    }
                                    append_tool_output_lines(
                                        &mut exec.output_lines,
                                        &tools::sanitize_output(&chunk),
                                        Some("[stderr] "),
                                    );
                                }
                                tools::ToolEvent::Completed { tool_call_id } => {
                                    let is_current =
                                        exec.current_call.as_ref().map(|call| call.id.as_str())
                                            == Some(tool_call_id.as_str());
                                    if is_current {
                                        exec.output_lines
                                            .push(format!("✓ Tool completed ({tool_call_id})"));
                                    }
                                }
                            },
                            Err(mpsc::error::TryRecvError::Empty) => break,
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                exec.event_rx = None;
                                break;
                            }
                        }
                    }
                }

                if let Some(handle) = exec.join_handle.as_mut()
                    && let Some(joined) = handle.now_or_never()
                {
                    exec.join_handle = None;
                    exec.event_rx = None;
                    exec.abort_handle = None;

                    let result = match joined {
                        Ok(result) => result,
                        Err(err) => {
                            let (call_id, tool_name) = exec.current_call.as_ref().map_or_else(
                                || ("<unknown>".to_string(), "<unknown>".to_string()),
                                |c| (c.id.clone(), c.name.clone()),
                            );
                            let message = if err.is_cancelled() {
                                "Tool execution cancelled"
                            } else {
                                "Tool execution failed"
                            };
                            ToolResult::error(call_id, tool_name, message)
                        }
                    };
                    exec.current_call = None;
                    completed = Some(result);
                }

                if let Some(result) = completed.take() {
                    if let Some(id) = state.batch.batch_id {
                        let _ = self.tool_journal.record_result(id, &result);
                    }
                    exec.remaining_capacity_bytes = exec
                        .remaining_capacity_bytes
                        .saturating_sub(result.content.len());
                    state.batch.results.push(result);

                    if exec.queue.is_empty() {
                        should_commit = true;
                    } else {
                        self.start_next_tool_call(exec);
                    }
                }
            }
        }

        if should_commit {
            self.commit_tool_batch(
                state.batch.assistant_text,
                state.batch.calls,
                state.batch.results,
                state.batch.model,
                state.batch.step_id,
                state.batch.batch_id,
                true,
                state.batch.turn,
            );
        } else {
            self.state = OperationState::ToolLoop(Box::new(state));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cancel_tool_batch(
        &mut self,
        assistant_text: String,
        calls: Vec<ToolCall>,
        mut results: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        batch_id: Option<ToolBatchId>,
        turn: TurnContext,
    ) {
        let existing: HashSet<String> = results.iter().map(|r| r.tool_call_id.clone()).collect();
        for call in &calls {
            if existing.contains(&call.id) {
                continue;
            }
            let result = ToolResult::error(call.id.clone(), call.name.clone(), "Cancelled by user");
            if let Some(id) = batch_id {
                let _ = self.tool_journal.record_result(id, &result);
            }
            results.push(result);
        }

        self.commit_tool_batch(
            assistant_text,
            calls,
            results,
            model,
            step_id,
            batch_id,
            false,
            turn,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_tool_batch(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        results: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        batch_id: Option<ToolBatchId>,
        auto_resume: bool,
        turn: TurnContext,
    ) {
        self.state = self.idle_state();

        let mut step_id_recorded = false;
        if let Ok(content) = NonEmptyString::new(assistant_text.clone()) {
            let message = Message::assistant(model.clone(), content);
            self.push_history_message_with_step_id(message, step_id);
            step_id_recorded = true;
        }

        let mut result_map: std::collections::HashMap<String, ToolResult> =
            std::collections::HashMap::new();
        for result in results {
            result_map
                .entry(result.tool_call_id.clone())
                .or_insert(result);
        }

        let mut ordered_results: Vec<ToolResult> = Vec::new();
        for call in &tool_calls {
            if let Some(result) = result_map.remove(&call.id) {
                ordered_results.push(result);
            } else {
                ordered_results.push(ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    "Missing tool result",
                ));
            }
        }

        // Interleave tool calls with their results so each result appears right after
        // its corresponding call in the display list (for proper tree connector rendering)
        for (idx, (call, result)) in tool_calls.iter().zip(&ordered_results).enumerate() {
            if !step_id_recorded && idx == 0 {
                self.push_history_message_with_step_id(Message::tool_use(call.clone()), step_id);
                step_id_recorded = true;
            } else {
                self.push_history_message(Message::tool_use(call.clone()));
            }
            self.push_history_message(Message::tool_result(result.clone()));
        }

        let autosave_succeeded = self.autosave_history();
        if autosave_succeeded {
            self.finalize_journal_commit(step_id);
            if let Some(id) = batch_id
                && let Err(e) = self.tool_journal.commit_batch(id)
            {
                tracing::warn!("Failed to commit tool batch {id}: {e}");
            }
        }

        if !auto_resume {
            self.pending_user_message = None;
        }

        // Only auto_resume if autosave succeeded - otherwise the journal step
        // remains uncommitted and would cause recovery issues on restart
        if auto_resume && !autosave_succeeded {
            self.push_notification(
                "Cannot continue tool loop: history save failed. Stopping to prevent data loss.",
            );
            self.finish_turn(turn);
            return;
        }

        if auto_resume {
            let Some(api_key) = self.api_keys.get(&model.provider()).cloned() else {
                self.push_notification(format!(
                    "Cannot resume: no API key for {}",
                    model.provider().display_name()
                ));
                self.finish_turn(turn);
                return;
            };

            let api_key = crate::util::wrap_api_key(model.provider(), api_key);

            let config = match ApiConfig::new(api_key, model.clone()) {
                Ok(config) => config
                    .with_openai_options(self.openai_options_for_model(&model))
                    .with_gemini_thinking_enabled(self.gemini_thinking_enabled),
                Err(e) => {
                    self.push_notification(format!("Cannot resume after tools: {e}"));
                    self.finish_turn(turn);
                    return;
                }
            };

            self.start_streaming(QueuedUserMessage { config, turn });
            return;
        }

        self.finish_turn(turn);
    }

    pub(crate) fn resolve_tool_approval(&mut self, decision: tools::ApprovalDecision) {
        let state = match std::mem::replace(&mut self.state, OperationState::Idle) {
            OperationState::ToolLoop(state) => *state,
            other => {
                self.state = other;
                return;
            }
        };

        let ToolLoopState { mut batch, phase } = state;
        let ToolLoopPhase::AwaitingApproval(_approval) = phase else {
            self.state = OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase }));
            return;
        };

        let mut approved_ids: HashSet<String> = HashSet::new();
        match &decision {
            tools::ApprovalDecision::ApproveAll => {
                approved_ids = batch
                    .approval_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect();
            }
            tools::ApprovalDecision::ApproveSelected(ids) => {
                approved_ids.extend(ids.iter().cloned());
            }
            tools::ApprovalDecision::DenyAll => {}
        }

        let mut approved_calls = Vec::new();
        let mut denied_results = Vec::new();
        for call in batch.approval_calls.drain(..) {
            if approved_ids.contains(&call.id) {
                approved_calls.push(call);
            } else {
                denied_results.push(ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    "Tool call denied by user",
                ));
            }
        }

        // When DenyAll, also deny auto-approved tools - "deny all" means deny all
        if matches!(decision, tools::ApprovalDecision::DenyAll) {
            for call in batch.execute_now.drain(..) {
                denied_results.push(ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    "Tool call denied by user",
                ));
            }
        }

        if let Some(id) = batch.batch_id {
            for result in &denied_results {
                let _ = self.tool_journal.record_result(id, result);
            }
        }
        batch.results.extend(denied_results);

        let mut allowed_ids: HashSet<String> = batch
            .execute_now
            .iter()
            .map(|call| call.id.clone())
            .collect();
        for call in &approved_calls {
            allowed_ids.insert(call.id.clone());
        }
        let queue: Vec<ToolCall> = batch
            .calls
            .iter()
            .filter(|call| allowed_ids.contains(&call.id))
            .cloned()
            .collect();
        batch.execute_now = queue.clone();

        if queue.is_empty() {
            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                batch.results,
                batch.model,
                batch.step_id,
                batch.batch_id,
                true,
                batch.turn,
            );
            return;
        }

        let remaining_capacity = self.remaining_tool_capacity(&batch);
        let exec = self.spawn_tool_execution(queue, remaining_capacity, batch.turn.recorder());
        self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
            batch,
            phase: ToolLoopPhase::Executing(exec),
        }));
    }

    pub(crate) fn resolve_tool_recovery(&mut self, decision: ToolRecoveryDecision) {
        let state = match std::mem::replace(&mut self.state, OperationState::Idle) {
            OperationState::ToolRecovery(state) => state,
            other => {
                self.state = other;
                return;
            }
        };

        self.commit_recovered_tool_batch(state, decision);
    }

    pub(crate) fn commit_recovered_tool_batch(
        &mut self,
        state: ToolRecoveryState,
        decision: ToolRecoveryDecision,
    ) {
        let ToolRecoveryState {
            batch,
            step_id,
            model,
        } = state;

        let assistant_text = batch.assistant_text.clone();
        let results = match decision {
            ToolRecoveryDecision::Resume => {
                let mut merged = batch.results;
                let existing: HashSet<String> =
                    merged.iter().map(|r| r.tool_call_id.clone()).collect();
                for call in &batch.calls {
                    if !existing.contains(&call.id) {
                        merged.push(ToolResult::error(
                            call.id.clone(),
                            call.name.clone(),
                            "Tool result missing after crash",
                        ));
                    }
                }
                merged
            }
            ToolRecoveryDecision::Discard => batch
                .calls
                .iter()
                .map(|call| {
                    ToolResult::error(
                        call.id.clone(),
                        call.name.clone(),
                        "Tool results discarded after crash",
                    )
                })
                .collect(),
        };

        // RecoveredToolBatch always has a valid batch_id from the journal
        for result in &results {
            let _ = self.tool_journal.record_result(batch.batch_id, result);
        }

        let auto_resume = true;
        self.commit_tool_batch(
            assistant_text,
            batch.calls,
            results,
            model,
            step_id,
            Some(batch.batch_id),
            auto_resume,
            TurnContext::new_for_recovery(),
        );

        match decision {
            ToolRecoveryDecision::Resume => {
                self.push_notification("Recovered tool batch resumed");
            }
            ToolRecoveryDecision::Discard => {
                self.push_notification("Tool results discarded after crash");
            }
        }
    }

    /// Finish a user turn and report any file changes.
    pub(crate) fn finish_turn(&mut self, turn: TurnContext) {
        let working_dir = self.tool_settings.sandbox.working_dir();
        let (report, created, modified) = turn.finish(&working_dir);

        // Aggregate turn changes into session-wide log
        self.session_changes.merge_turn(&created, &modified);

        // Sync files panel selection after file list changes
        self.files_panel_sync_selection();

        if let TurnChangeReport::Changes(summary) = report {
            let msg = summary.into_message();
            self.push_local_message(Message::system(msg));
        }
        self.pending_user_message = None;
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn preflight_sandbox(
    sandbox: &tools::sandbox::Sandbox,
    call: &ToolCall,
) -> Result<(), tools::ToolError> {
    let working_dir = sandbox.working_dir();
    match call.name.as_str() {
        "read_file" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "path must be a string".to_string(),
                })?;
            let _ = sandbox.resolve_path(path, &working_dir)?;
        }
        "apply_patch" => {
            let patch_str = call
                .arguments
                .get("patch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "patch must be a string".to_string(),
                })?;
            let patch =
                tools::lp1::parse_patch(patch_str).map_err(|e| tools::ToolError::BadArgs {
                    message: e.to_string(),
                })?;
            for file in patch.files {
                let _ = sandbox.resolve_path(&file.path, &working_dir)?;
            }
        }
        "write_file" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "path must be a string".to_string(),
                })?;
            let _ = sandbox.resolve_path_for_create(path, &working_dir)?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn tool_error_result(call: &ToolCall, err: tools::ToolError) -> ToolResult {
    let message = match err {
        tools::ToolError::BadArgs { message } => format!("Bad args: {message}"),
        tools::ToolError::Timeout { tool, elapsed } => {
            format!("Tool '{tool}' timed out after {}s", elapsed.as_secs())
        }
        tools::ToolError::SandboxViolation(reason) => reason.to_string(),
        tools::ToolError::ExecutionFailed { tool, message } => {
            format!("{tool} failed: {message}")
        }
        tools::ToolError::Cancelled => "Cancelled by user".to_string(),
        tools::ToolError::UnknownTool { name } => format!("Unknown tool: {name}"),
        tools::ToolError::DuplicateTool { name } => format!("Duplicate tool: {name}"),
        tools::ToolError::DuplicateToolCallId { id } => {
            format!("Duplicate tool call id: {id}")
        }
        tools::ToolError::PatchFailed { file, message } => {
            format!("Patch failed for {}: {message}", file.display())
        }
        tools::ToolError::StaleFile { file, reason } => {
            format!("Stale file {}: {reason}", file.display())
        }
    };

    ToolResult::error(
        call.id.clone(),
        call.name.clone(),
        tools::sanitize_output(&message),
    )
}

pub(crate) fn append_tool_output_lines(lines: &mut Vec<String>, chunk: &str, prefix: Option<&str>) {
    let prefix = prefix.unwrap_or("");
    for line in chunk.lines() {
        let mut entry = String::new();
        entry.push_str(prefix);
        entry.push_str(line);
        lines.push(entry);
    }
    if lines.len() > 50 {
        let overflow = lines.len() - 50;
        lines.drain(0..overflow);
    }
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

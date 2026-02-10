//! Tool execution loop for the App.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::{AbortHandle, Abortable, FutureExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use forge_context::{ContextUsageStatus, StepId, ToolBatchId};
use forge_types::{ModelName, ToolCall, ToolResult};

use crate::input_modes::{ChangeRecorder, TurnChangeReport, TurnContext};
use crate::state::{
    ApprovalState, JournalStatus, OperationState, ToolBatch, ToolLoopPhase, ToolLoopState,
    ToolPlan, ToolRecoveryDecision, ToolRecoveryState,
};
use crate::tools::{self, ConfirmationRequest, analyze_tool_arguments};
use crate::util;
use crate::{
    ApiConfig, App, DEFAULT_TOOL_CAPACITY_BYTES, Message, NonEmptyString, QueuedUserMessage,
    SystemNotification, TOOL_EVENT_CHANNEL_CAPACITY, TOOL_OUTPUT_SAFETY_MARGIN_TOKENS,
};

fn run_escalation_reason(tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
    if tool_name != "Run" {
        return None;
    }

    let reason = arguments.get("reason")?.as_str()?.trim();
    if reason.is_empty() {
        return None;
    }

    let sanitized = crate::security::sanitize_display_text(reason);
    Some(util::truncate_with_ellipsis(&sanitized, 200))
}

fn now_unix_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(0)
}

// SpawnedTool: Proof object for spawned tool execution (IFA §8.1)

#[derive(Debug)]
pub(crate) struct SpawnedTool {
    call: ToolCall,
    join_handle: JoinHandle<ToolResult>,
    event_rx: mpsc::Receiver<tools::ToolEvent>,
    abort_handle: AbortHandle,
}

#[derive(Debug)]
pub(crate) struct CompletedTool {
    pub(crate) call: ToolCall,
    pub(crate) result: Result<ToolResult, tokio::task::JoinError>,
    pub(crate) final_events: Vec<tools::ToolEvent>,
}

impl SpawnedTool {
    pub(crate) fn spawn<F, Fut>(call: ToolCall, task_fn: F) -> Self
    where
        F: FnOnce(mpsc::Sender<tools::ToolEvent>, AbortHandle) -> Fut,
        Fut: Future<Output = ToolResult> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel(TOOL_EVENT_CHANNEL_CAPACITY);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        // Capture only what we need for the abort error path
        let call_id_for_abort = call.id.clone();
        let call_name_for_abort = call.name.clone();

        // Clone abort_handle for use inside the task (for ToolCtx.abort checking)
        let abort_handle_for_task = abort_handle.clone();
        let future = task_fn(tx, abort_handle_for_task);
        let abortable = Abortable::new(future, abort_registration);

        let join_handle = tokio::spawn(async move {
            match abortable.await {
                Ok(result) => result,
                Err(_aborted) => {
                    ToolResult::error(call_id_for_abort, call_name_for_abort, "Cancelled by user")
                }
            }
        });

        Self {
            call,
            join_handle,
            event_rx: rx,
            abort_handle,
        }
    }

    pub(crate) fn call(&self) -> &ToolCall {
        &self.call
    }

    pub(crate) fn try_recv_event(&mut self) -> Option<tools::ToolEvent> {
        self.event_rx.try_recv().ok()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }

    pub(crate) fn abort(&self) {
        self.abort_handle.abort();
    }

    pub(crate) fn try_complete_now(mut self) -> Result<CompletedTool, Self> {
        let result = (&mut self.join_handle).now_or_never();
        let Some(result) = result else {
            return Err(self);
        };

        let Self {
            call,
            join_handle: _,
            mut event_rx,
            abort_handle: _,
        } = self;

        // Drain events AFTER completion - events may have been produced while finishing
        let mut final_events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            final_events.push(event);
        }

        Ok(CompletedTool {
            call,
            result,
            final_events,
        })
    }
}

// ToolQueue: Queue state without active execution

#[derive(Debug)]
pub(crate) struct ToolQueue {
    pub(crate) queue: VecDeque<ToolCall>,
    pub(crate) output_lines: HashMap<String, Vec<String>>,
    pub(crate) remaining_capacity_bytes: usize,
    pub(crate) turn_recorder: ChangeRecorder,
}

impl ToolQueue {
    pub(crate) fn new(
        calls: Vec<ToolCall>,
        remaining_capacity_bytes: usize,
        turn_recorder: ChangeRecorder,
    ) -> Self {
        Self {
            queue: VecDeque::from(calls),
            output_lines: HashMap::new(),
            remaining_capacity_bytes,
            turn_recorder,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

// ActiveExecution: State with a spawned tool (requires SpawnedTool)

#[derive(Debug)]
pub(crate) struct ActiveExecution {
    pub(crate) spawned: SpawnedTool, // Invariant: always present; existence proves execution.
    pub(crate) queue: VecDeque<ToolCall>,
    pub(crate) output_lines: HashMap<String, Vec<String>>,
    pub(crate) turn_recorder: ChangeRecorder,
}

impl App {
    pub(crate) fn disable_tools_due_to_tool_journal_error(
        &mut self,
        context: &'static str,
        error: impl std::fmt::Display,
    ) {
        let error = error.to_string();
        tracing::warn!("Tool journal error during {context}: {error}");

        // Latch tools-disabled state so future tool calls are pre-resolved.
        let was_disabled = self.tool_journal_disabled_reason.is_some();
        self.tool_journal_disabled_reason = Some(error.clone());
        if !was_disabled {
            self.push_notification(format!(
                "Tool journal error during {context}; tool execution disabled for safety. ({error})"
            ));
        }
    }

    fn record_tool_result_or_disable(
        &mut self,
        batch_id: ToolBatchId,
        result: &ToolResult,
        context: &'static str,
    ) -> bool {
        if let Err(e) = self.tool_journal.record_result(batch_id, result) {
            self.disable_tools_due_to_tool_journal_error(context, e);
            return false;
        }
        true
    }

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
        thinking_message: Option<Message>,
    ) {
        if tool_calls.is_empty() {
            self.finish_turn(turn);
            return;
        }

        // Tool-call assistant text is untrusted external content (LLM output).
        // Sanitize ONCE before journaling and before it can reach persistence/display paths.
        let assistant_text = crate::security::sanitize_display_text(&assistant_text);

        // If tools are disabled due to journal health, fail closed: pre-resolve tool calls to errors.
        if let Some(reason) = self.tool_journal_disabled_reason.clone() {
            if let Some(batch_id) = tool_batch_id
                && let Err(e) = self.tool_journal.discard_batch(batch_id)
            {
                tracing::warn!("Failed to discard stale tool batch {batch_id}: {e}");
            }

            let max_iters = self.tool_settings.limits.max_tool_iterations_per_user_turn;
            let next_iteration = self.tool_iterations.saturating_add(1);
            let error_message = if next_iteration > max_iters {
                "Max tool iterations reached"
            } else {
                self.tool_iterations = next_iteration;
                "Tool execution disabled: tool journal unavailable"
            };

            let mut results = pre_resolved;
            let existing: HashSet<String> =
                results.iter().map(|r| r.tool_call_id.clone()).collect();
            for call in &tool_calls {
                if existing.contains(&call.id) {
                    continue;
                }
                let message = format!("{error_message} ({reason})");
                results.push(ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    &message,
                ));
            }

            self.commit_tool_batch_without_journal(
                assistant_text,
                tool_calls,
                results,
                model,
                step_id,
                true,
                turn,
                thinking_message,
            );
            return;
        }

        // Determine journal status BEFORE constructing ToolBatch (IFA §10.1).
        // Persisted(id) is the capability proof that crash recovery is possible.
        // Without it, tool execution is blocked — fail closed.
        let journal_status = if let Some(id) = tool_batch_id {
            // Try to update existing batch
            match self.tool_journal.update_assistant_text(id, &assistant_text) {
                Ok(()) => JournalStatus::new(id),
                Err(e) => {
                    tracing::warn!("Tool journal update failed: {e}");
                    self.push_notification(format!("Tool journal error: {e}"));
                    // Try to create a new batch instead
                    match self.tool_journal.begin_batch(
                        step_id,
                        model.as_str(),
                        &assistant_text,
                        &tool_calls,
                    ) {
                        Ok(new_id) => JournalStatus::new(new_id),
                        Err(e2) => {
                            tracing::warn!("Tool journal begin failed after retry: {e2}");
                            self.tool_journal_disabled_reason = Some(e2.to_string());
                            self.push_notification(format!(
                                "Tool execution disabled: cannot persist tool journal for crash recovery. ({e2})"
                            ));

                            let max_iters =
                                self.tool_settings.limits.max_tool_iterations_per_user_turn;
                            let next_iteration = self.tool_iterations.saturating_add(1);
                            let error_message = if next_iteration > max_iters {
                                "Max tool iterations reached"
                            } else {
                                self.tool_iterations = next_iteration;
                                "Tool execution disabled: tool journal unavailable"
                            };

                            let mut results = pre_resolved;
                            let existing: HashSet<String> =
                                results.iter().map(|r| r.tool_call_id.clone()).collect();
                            for call in &tool_calls {
                                if existing.contains(&call.id) {
                                    continue;
                                }
                                results.push(ToolResult::error(
                                    call.id.clone(),
                                    call.name.clone(),
                                    error_message,
                                ));
                            }

                            self.commit_tool_batch_without_journal(
                                assistant_text,
                                tool_calls,
                                results,
                                model,
                                step_id,
                                true,
                                turn,
                                thinking_message,
                            );
                            return;
                        }
                    }
                }
            }
        } else {
            // No existing batch, create new one
            match self.tool_journal.begin_batch(
                step_id,
                model.as_str(),
                &assistant_text,
                &tool_calls,
            ) {
                Ok(id) => JournalStatus::new(id),
                Err(e) => {
                    tracing::warn!("Tool journal begin failed: {e}");
                    self.tool_journal_disabled_reason = Some(e.to_string());
                    self.push_notification(format!(
                        "Tool execution disabled: cannot persist tool journal for crash recovery. ({e})"
                    ));

                    let max_iters = self.tool_settings.limits.max_tool_iterations_per_user_turn;
                    let next_iteration = self.tool_iterations.saturating_add(1);
                    let error_message = if next_iteration > max_iters {
                        "Max tool iterations reached"
                    } else {
                        self.tool_iterations = next_iteration;
                        "Tool execution disabled: tool journal unavailable"
                    };

                    let mut results = pre_resolved;
                    let existing: HashSet<String> =
                        results.iter().map(|r| r.tool_call_id.clone()).collect();
                    for call in &tool_calls {
                        if existing.contains(&call.id) {
                            continue;
                        }
                        results.push(ToolResult::error(
                            call.id.clone(),
                            call.name.clone(),
                            error_message,
                        ));
                    }

                    self.commit_tool_batch_without_journal(
                        assistant_text,
                        tool_calls,
                        results,
                        model,
                        step_id,
                        true,
                        turn,
                        thinking_message,
                    );
                    return;
                }
            }
        };

        self.start_tool_loop(
            assistant_text,
            tool_calls,
            pre_resolved,
            model,
            step_id,
            journal_status,
            turn,
            thinking_message,
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
        journal_status: JournalStatus,
        turn: TurnContext,
        thinking_message: Option<Message>,
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
            {
                let id = journal_status.batch_id();
                for result in &results {
                    let _ = self.record_tool_result_or_disable(id, result, "max-iterations result");
                }
            }
            self.commit_tool_batch(
                assistant_text,
                tool_calls,
                results,
                model,
                step_id,
                journal_status,
                true,
                turn,
                thinking_message,
            );
            return;
        }
        self.tool_iterations = next_iteration;

        let plan = self.plan_tool_calls(&tool_calls, pre_resolved);
        {
            let id = journal_status.batch_id();
            for result in &plan.pre_resolved {
                if !self.record_tool_result_or_disable(id, result, "pre-resolved tool result") {
                    let mut results = plan.pre_resolved.clone();
                    let existing: HashSet<String> =
                        results.iter().map(|r| r.tool_call_id.clone()).collect();
                    for call in &tool_calls {
                        if existing.contains(&call.id) {
                            continue;
                        }
                        results.push(ToolResult::error(
                            call.id.clone(),
                            call.name.clone(),
                            "Tool execution disabled: tool journal unavailable",
                        ));
                    }
                    self.commit_tool_batch(
                        assistant_text,
                        tool_calls,
                        results,
                        model,
                        step_id,
                        journal_status,
                        true,
                        turn,
                        thinking_message,
                    );
                    return;
                }
            }
        }

        // Create an automatic checkpoint before any tool-driven file edits.
        // (QoL: enables /rewind <id> [code|conversation|both])
        self.maybe_create_checkpoint_for_tool_calls(
            plan.execute_now.iter().chain(plan.approval_calls.iter()),
        );

        let batch = ToolBatch {
            assistant_text,
            thinking_message,
            calls: tool_calls,
            results: plan.pre_resolved,
            model,
            step_id,
            journal_status,
            execute_now: plan.execute_now,
            approval_calls: plan.approval_calls,
            turn,
        };
        let remaining_capacity_bytes = self.remaining_tool_capacity(&batch);

        if !plan.approval_requests.is_empty() {
            let approval = ApprovalState::new(plan.approval_requests);
            self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
                batch,
                phase: ToolLoopPhase::AwaitingApproval(approval),
            }));
            return;
        }

        let calls_to_execute = batch.execute_now.clone();
        if calls_to_execute.is_empty() {
            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                batch.results,
                batch.model,
                batch.step_id,
                batch.journal_status,
                true,
                batch.turn,
                batch.thinking_message,
            );
            return;
        }

        let phase = match self.start_tool_execution(
            batch.journal_status.batch_id(),
            calls_to_execute,
            remaining_capacity_bytes,
            batch.turn.recorder(),
        ) {
            Ok(phase) => phase,
            Err(e) => {
                self.disable_tools_due_to_tool_journal_error("mark tool call started", e);

                let mut results = batch.results;
                let existing: HashSet<String> =
                    results.iter().map(|r| r.tool_call_id.clone()).collect();
                for call in &batch.calls {
                    if existing.contains(&call.id) {
                        continue;
                    }
                    results.push(ToolResult::error(
                        call.id.clone(),
                        call.name.clone(),
                        "Tool execution stopped: tool journal error",
                    ));
                }

                self.commit_tool_batch(
                    batch.assistant_text,
                    batch.calls,
                    results,
                    batch.model,
                    batch.step_id,
                    batch.journal_status,
                    true,
                    batch.turn,
                    batch.thinking_message,
                );
                return;
            }
        };
        self.state = OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase }));
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

            if call.name == "Edit"
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
                    exec.requires_approval()
                        || (exec.is_side_effecting(&call.arguments) && !allowlisted)
                        || (exec.reads_user_data(&call.arguments) && !allowlisted)
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
                let summary = crate::security::sanitize_display_text(&summary);
                let summary = util::truncate_with_ellipsis(&summary, 200);
                let warnings = analyze_tool_arguments(&call.name, &call.arguments);
                approval_requests.push(ConfirmationRequest {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    summary,
                    reason: run_escalation_reason(&call.name, &call.arguments),
                    risk_level: exec.risk_level(&call.arguments),
                    arguments: call.arguments.clone(),
                    warnings,
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
            | ContextUsageStatus::NeedsDistillation { usage, .. }
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

    /// Create a tool queue and immediately try to spawn the first tool.
    /// Returns the appropriate phase (Processing if queue empty, Executing otherwise).
    fn start_tool_execution(
        &mut self,
        batch_id: ToolBatchId,
        calls: Vec<ToolCall>,
        initial_capacity_bytes: usize,
        turn_recorder: ChangeRecorder,
    ) -> anyhow::Result<ToolLoopPhase> {
        let queue = ToolQueue::new(calls, initial_capacity_bytes, turn_recorder);
        self.spawn_next_from_queue(batch_id, queue)
    }

    /// Spawn the next tool from the queue, transitioning to Executing if possible.
    ///
    /// # IFA Conformance
    /// - Call comes FROM the queue, preventing mismatch
    /// - Consumes queue, returns new phase
    /// - Returns Processing if queue empty
    fn spawn_next_from_queue(
        &mut self,
        batch_id: ToolBatchId,
        mut queue: ToolQueue,
    ) -> anyhow::Result<ToolLoopPhase> {
        let Some(call) = queue.queue.pop_front() else {
            return Ok(ToolLoopPhase::Processing(queue));
        };

        self.tool_journal
            .mark_call_started(batch_id, &call.id, now_unix_ms())?;

        let remaining_capacity = queue.remaining_capacity_bytes;
        let turn_recorder = queue.turn_recorder.clone();

        // Capture app state needed for tool execution
        let registry = self.tool_registry.clone();
        let settings = self.tool_settings.clone();
        let file_cache = self.tool_file_cache.clone();
        let librarian = self.librarian.clone();
        let working_dir = settings.sandbox.working_dir();

        let spawned = SpawnedTool::spawn(call.clone(), |event_tx, abort_handle| {
            let call = call.clone();

            async move {
                let _ = event_tx.try_send(tools::ToolEvent::Started {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                });

                let exec_ref = match registry.lookup(&call.name) {
                    Ok(exec) => exec,
                    Err(err) => {
                        let result = tool_error_result(&call, err);
                        let _ = event_tx.try_send(tools::ToolEvent::Completed {
                            tool_call_id: call.id.clone(),
                        });
                        return result;
                    }
                };

                let default_timeout = match call.name.as_str() {
                    "Read" | "Edit" => settings.timeouts.file_operations_timeout,
                    "Run" => settings.timeouts.shell_commands_timeout,
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
                    command_blacklist: settings.command_blacklist.clone(),
                };

                let timeout = exec_ref.timeout().unwrap_or(ctx.default_timeout);
                let exec_future = exec_ref.execute(call.arguments.clone(), &mut ctx);
                let exec_future = std::panic::AssertUnwindSafe(exec_future).catch_unwind();

                let result = match tokio::time::timeout(timeout, exec_future).await {
                    Err(_) => tool_error_result(
                        &call,
                        tools::ToolError::Timeout {
                            tool: call.name.clone(),
                            elapsed: timeout,
                        },
                    ),
                    Ok(Err(panic_payload)) => {
                        let panic_msg = panic_payload_to_string(&panic_payload);
                        let message = format!("Tool panicked: {panic_msg}");
                        ToolResult::error(
                            call.id.clone(),
                            call.name.clone(),
                            tools::sanitize_output(&message),
                        )
                    }
                    Ok(Ok(inner)) => match inner {
                        Ok(output) => {
                            let sanitized = tools::sanitize_output(&output);
                            let effective_max =
                                ctx.max_output_bytes.min(ctx.available_capacity_bytes);
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

                let _ = event_tx.try_send(tools::ToolEvent::Completed {
                    tool_call_id: call.id.clone(),
                });

                result
            }
        });

        Ok(ToolLoopPhase::Executing(ActiveExecution {
            spawned,
            queue: queue.queue,
            output_lines: queue.output_lines,
            turn_recorder: queue.turn_recorder,
        }))
    }

    pub(crate) fn poll_tool_loop(&mut self) {
        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
            OperationState::ToolLoop(state) => *state,
            other => {
                self.state = other;
                return;
            }
        };
        let ToolLoopState { batch, phase } = state;

        match phase {
            ToolLoopPhase::AwaitingApproval(approval) => {
                // No polling needed - wait for user input
                self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
                    batch,
                    phase: ToolLoopPhase::AwaitingApproval(approval),
                }));
            }

            ToolLoopPhase::Processing(queue) => {
                // If tool journaling was disabled mid-loop, fail closed and do not execute
                // any additional tools in this turn.
                if self.tool_journal_disabled_reason.is_some() {
                    let mut results = batch.results;
                    let existing: HashSet<String> =
                        results.iter().map(|r| r.tool_call_id.clone()).collect();
                    for call in &batch.calls {
                        if existing.contains(&call.id) {
                            continue;
                        }
                        results.push(ToolResult::error(
                            call.id.clone(),
                            call.name.clone(),
                            "Tool execution stopped: tool journal error",
                        ));
                    }

                    self.commit_tool_batch(
                        batch.assistant_text,
                        batch.calls,
                        results,
                        batch.model,
                        batch.step_id,
                        batch.journal_status,
                        true,
                        batch.turn,
                        batch.thinking_message,
                    );
                    return;
                }

                // Try to spawn the next tool from queue
                if queue.is_empty() {
                    // Queue empty - commit batch
                    self.commit_tool_batch(
                        batch.assistant_text,
                        batch.calls,
                        batch.results,
                        batch.model,
                        batch.step_id,
                        batch.journal_status,
                        true,
                        batch.turn,
                        batch.thinking_message,
                    );
                } else {
                    // Spawn next tool
                    let phase =
                        match self.spawn_next_from_queue(batch.journal_status.batch_id(), queue) {
                            Ok(phase) => phase,
                            Err(e) => {
                                self.disable_tools_due_to_tool_journal_error(
                                    "mark tool call started",
                                    e,
                                );

                                let mut results = batch.results;
                                let existing: HashSet<String> =
                                    results.iter().map(|r| r.tool_call_id.clone()).collect();
                                for call in &batch.calls {
                                    if existing.contains(&call.id) {
                                        continue;
                                    }
                                    results.push(ToolResult::error(
                                        call.id.clone(),
                                        call.name.clone(),
                                        "Tool execution stopped: tool journal error",
                                    ));
                                }

                                self.commit_tool_batch(
                                    batch.assistant_text,
                                    batch.calls,
                                    results,
                                    batch.model,
                                    batch.step_id,
                                    batch.journal_status,
                                    true,
                                    batch.turn,
                                    batch.thinking_message,
                                );
                                return;
                            }
                        };
                    self.state = OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase }));
                }
            }

            ToolLoopPhase::Executing(mut exec) => {
                // Poll for events from the spawned tool
                while let Some(event) = exec.spawned.try_recv_event() {
                    if let tools::ToolEvent::ProcessSpawned {
                        tool_call_id,
                        pid,
                        process_started_at_unix_ms,
                        ..
                    } = &event
                        && let Err(e) = self.tool_journal.record_call_process(
                            batch.journal_status.batch_id(),
                            tool_call_id,
                            i64::from(*pid),
                            *process_started_at_unix_ms,
                        )
                    {
                        self.disable_tools_due_to_tool_journal_error(
                            "record tool process metadata",
                            e,
                        );
                        // Fail closed: abort the tool so side effects stop as soon as possible.
                        exec.spawned.abort();
                    }
                    apply_tool_event_to_output_lines(&mut exec.output_lines, event);
                }

                // Check if the spawned tool has completed
                if exec.spawned.is_finished() {
                    let ActiveExecution {
                        spawned,
                        queue,
                        mut output_lines,
                        turn_recorder,
                    } = exec;

                    let completed = match spawned.try_complete_now() {
                        Ok(completed) => completed,
                        Err(spawned) => {
                            // Edge-case: is_finished() was true but join handle isn't ready yet.
                            // Keep state and retry next tick rather than aborting the turn.
                            self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
                                batch,
                                phase: ToolLoopPhase::Executing(ActiveExecution {
                                    spawned,
                                    queue,
                                    output_lines,
                                    turn_recorder,
                                }),
                            }));
                            return;
                        }
                    };

                    // Process final events from the completed tool
                    for event in completed.final_events {
                        if let tools::ToolEvent::ProcessSpawned {
                            tool_call_id,
                            pid,
                            process_started_at_unix_ms,
                            ..
                        } = &event
                            && let Err(e) = self.tool_journal.record_call_process(
                                batch.journal_status.batch_id(),
                                tool_call_id,
                                i64::from(*pid),
                                *process_started_at_unix_ms,
                            )
                        {
                            self.disable_tools_due_to_tool_journal_error(
                                "record tool process metadata",
                                e,
                            );
                        }
                        apply_tool_event_to_output_lines(&mut output_lines, event);
                    }

                    // Convert completed result to ToolResult
                    let result = match completed.result {
                        Ok(result) => result,
                        Err(err) => {
                            let message = if err.is_cancelled() {
                                "Tool execution cancelled"
                            } else {
                                "Tool execution failed"
                            };
                            ToolResult::error(
                                completed.call.id.clone(),
                                completed.call.name.clone(),
                                message,
                            )
                        }
                    };

                    // Record result to journal
                    let mut batch = batch;
                    if !self.record_tool_result_or_disable(
                        batch.journal_status.batch_id(),
                        &result,
                        "tool result",
                    ) {
                        // Fail closed: do not execute any further tools when we cannot
                        // durably persist results for crash recovery.
                        let mut results = batch.results;
                        results.push(result);
                        let existing: HashSet<String> =
                            results.iter().map(|r| r.tool_call_id.clone()).collect();
                        for call in &batch.calls {
                            if existing.contains(&call.id) {
                                continue;
                            }
                            results.push(ToolResult::error(
                                call.id.clone(),
                                call.name.clone(),
                                "Tool execution stopped: tool journal error",
                            ));
                        }
                        self.commit_tool_batch(
                            batch.assistant_text,
                            batch.calls,
                            results,
                            batch.model,
                            batch.step_id,
                            batch.journal_status,
                            true,
                            batch.turn,
                            batch.thinking_message,
                        );
                        return;
                    }
                    batch.results.push(result);

                    // Recompute capacity fresh from current context state
                    let new_capacity = self.remaining_tool_capacity(&batch);

                    // Create new queue with updated state
                    let new_queue = ToolQueue {
                        queue,
                        output_lines,
                        remaining_capacity_bytes: new_capacity,
                        turn_recorder,
                    };

                    if new_queue.is_empty() {
                        // All tools done - commit batch
                        self.commit_tool_batch(
                            batch.assistant_text,
                            batch.calls,
                            batch.results,
                            batch.model,
                            batch.step_id,
                            batch.journal_status,
                            true,
                            batch.turn,
                            batch.thinking_message,
                        );
                    } else {
                        // If tool journaling was disabled mid-loop, fail closed and do not execute
                        // any additional tools in this turn.
                        if self.tool_journal_disabled_reason.is_some() {
                            let mut results = batch.results;
                            let existing: HashSet<String> =
                                results.iter().map(|r| r.tool_call_id.clone()).collect();
                            for call in &batch.calls {
                                if existing.contains(&call.id) {
                                    continue;
                                }
                                results.push(ToolResult::error(
                                    call.id.clone(),
                                    call.name.clone(),
                                    "Tool execution stopped: tool journal error",
                                ));
                            }

                            self.commit_tool_batch(
                                batch.assistant_text,
                                batch.calls,
                                results,
                                batch.model,
                                batch.step_id,
                                batch.journal_status,
                                true,
                                batch.turn,
                                batch.thinking_message,
                            );
                            return;
                        }

                        // Spawn next tool
                        let phase = match self
                            .spawn_next_from_queue(batch.journal_status.batch_id(), new_queue)
                        {
                            Ok(phase) => phase,
                            Err(e) => {
                                self.disable_tools_due_to_tool_journal_error(
                                    "mark tool call started",
                                    e,
                                );

                                let mut results = batch.results;
                                let existing: HashSet<String> =
                                    results.iter().map(|r| r.tool_call_id.clone()).collect();
                                for call in &batch.calls {
                                    if existing.contains(&call.id) {
                                        continue;
                                    }
                                    results.push(ToolResult::error(
                                        call.id.clone(),
                                        call.name.clone(),
                                        "Tool execution stopped: tool journal error",
                                    ));
                                }

                                self.commit_tool_batch(
                                    batch.assistant_text,
                                    batch.calls,
                                    results,
                                    batch.model,
                                    batch.step_id,
                                    batch.journal_status,
                                    true,
                                    batch.turn,
                                    batch.thinking_message,
                                );
                                return;
                            }
                        };
                        self.state =
                            OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase }));
                    }
                } else {
                    // Still running - keep state
                    self.state = OperationState::ToolLoop(Box::new(ToolLoopState {
                        batch,
                        phase: ToolLoopPhase::Executing(exec),
                    }));
                }
            }
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
        journal_status: JournalStatus,
        turn: TurnContext,
        thinking_message: Option<Message>,
    ) {
        let existing: HashSet<String> = results.iter().map(|r| r.tool_call_id.clone()).collect();
        for call in &calls {
            if existing.contains(&call.id) {
                continue;
            }
            let result = ToolResult::error(call.id.clone(), call.name.clone(), "Cancelled by user");
            let _ = self.record_tool_result_or_disable(
                journal_status.batch_id(),
                &result,
                "cancelled tool result",
            );
            results.push(result);
        }

        self.commit_tool_batch(
            assistant_text,
            calls,
            results,
            model,
            step_id,
            journal_status,
            false,
            turn,
            thinking_message,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_tool_batch_messages(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        results: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        thinking_message: Option<Message>,
    ) -> bool {
        // Defensive: recovered batches and alternate entry points might bypass handle_tool_calls.
        // This is idempotent and ensures we never persist/display raw untrusted assistant text.
        let assistant_text = crate::security::sanitize_display_text(&assistant_text);

        let mut step_id_recorded = false;
        if let Some(thinking_message) = thinking_message {
            let requires_persistence = matches!(
                &thinking_message,
                Message::Thinking(thinking) if thinking.requires_persistence()
            );
            if requires_persistence {
                self.push_history_message(thinking_message);
            } else {
                self.push_local_message(thinking_message);
            }
        }
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

        // Group all tool calls first (so they appear as a single block in history/API).
        // This is critical for providers like Gemini that require thoughtSignature round-tripping
        // within the same turn structure, and for parallel tool call correctness.
        for (idx, call) in tool_calls.iter().enumerate() {
            if !step_id_recorded && idx == 0 {
                self.push_history_message_with_step_id(Message::tool_use(call.clone()), step_id);
                step_id_recorded = true;
            } else {
                self.push_history_message(Message::tool_use(call.clone()));
            }
        }

        // Then push all tool results (matching the call order)
        for result in ordered_results {
            self.push_history_message(Message::tool_result(result));
        }

        self.autosave_history()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_tool_batch(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        results: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        journal_status: JournalStatus,
        auto_resume: bool,
        turn: TurnContext,
        thinking_message: Option<Message>,
    ) {
        self.state = self.idle_state();

        let autosave_succeeded = self.commit_tool_batch_messages(
            assistant_text,
            tool_calls,
            results,
            model.clone(),
            step_id,
            thinking_message,
        );
        let mut tool_cleanup_failed = false;
        if autosave_succeeded {
            self.finalize_journal_commit(step_id);
            let id = journal_status.batch_id();
            if let Err(e) = self.tool_journal.commit_batch(id) {
                tracing::warn!("Failed to commit tool batch {id}: {e}");
                tool_cleanup_failed = true;
                self.disable_tools_due_to_tool_journal_error("tool batch commit", &e);

                if self.pending_tool_cleanup == Some(id) {
                    self.pending_tool_cleanup_failures =
                        self.pending_tool_cleanup_failures.saturating_add(1);
                } else {
                    self.pending_tool_cleanup = Some(id);
                    self.pending_tool_cleanup_failures = 1;
                    self.push_notification(format!(
                        "Tool journal cleanup failed; will retry. If tools get stuck, run /clear. ({e})"
                    ));
                }

                let after = Instant::now() + Duration::from_secs(1);
                if self.next_journal_cleanup_attempt < after {
                    self.next_journal_cleanup_attempt = after;
                }
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

        if auto_resume && tool_cleanup_failed {
            // If tools are already disabled due to journal errors, allow streaming to continue.
            // The pending batch will be cleaned up best-effort or cleared manually via /clear.
            if self.tool_journal_disabled_reason.is_none() {
                self.push_notification(
                    "Cannot continue tool loop: tool journal cleanup failed. Stopping to prevent stuck state.",
                );
                self.finish_turn(turn);
                return;
            }
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
                    .with_gemini_thinking_enabled(self.gemini_thinking_enabled)
                    .with_anthropic_thinking(
                        self.anthropic_thinking_mode.as_str(),
                        self.anthropic_thinking_effort.as_str(),
                    ),
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_tool_batch_without_journal(
        &mut self,
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        results: Vec<ToolResult>,
        model: ModelName,
        step_id: StepId,
        auto_resume: bool,
        turn: TurnContext,
        thinking_message: Option<Message>,
    ) {
        self.state = self.idle_state();

        let autosave_succeeded = self.commit_tool_batch_messages(
            assistant_text,
            tool_calls,
            results,
            model.clone(),
            step_id,
            thinking_message,
        );
        if autosave_succeeded {
            self.finalize_journal_commit(step_id);
        }

        if !auto_resume {
            self.pending_user_message = None;
        }

        // Only auto_resume if autosave succeeded - otherwise the journal step
        // remains uncommitted and would cause recovery issues on restart.
        if auto_resume && !autosave_succeeded {
            self.push_notification(
                "Cannot continue after tool failure: history save failed. Stopping to prevent data loss.",
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
                    .with_gemini_thinking_enabled(self.gemini_thinking_enabled)
                    .with_anthropic_thinking(
                        self.anthropic_thinking_mode.as_str(),
                        self.anthropic_thinking_effort.as_str(),
                    ),
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
        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
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

        // Queue system notifications for tool approval/denial
        let approved_count = approved_calls.len();
        let denied_count = denied_results.len();
        if approved_count > 0 {
            #[allow(clippy::cast_possible_truncation)]
            self.queue_notification(SystemNotification::ToolsApproved {
                count: approved_count.min(u8::MAX as usize) as u8,
            });
        }
        if denied_count > 0 {
            #[allow(clippy::cast_possible_truncation)]
            self.queue_notification(SystemNotification::ToolsDenied {
                count: denied_count.min(u8::MAX as usize) as u8,
            });
        }

        {
            let id = batch.journal_status.batch_id();
            for result in &denied_results {
                if !self.record_tool_result_or_disable(id, result, "denied tool result") {
                    break;
                }
            }
        }
        batch.results.extend(denied_results);

        // If tool journaling failed while recording denied results, fail closed and do not execute
        // any additional tools in this turn.
        if self.tool_journal_disabled_reason.is_some() {
            let mut results = batch.results;
            let existing: HashSet<String> =
                results.iter().map(|r| r.tool_call_id.clone()).collect();
            for call in &batch.calls {
                if existing.contains(&call.id) {
                    continue;
                }
                results.push(ToolResult::error(
                    call.id.clone(),
                    call.name.clone(),
                    "Tool execution disabled: tool journal unavailable",
                ));
            }

            self.commit_tool_batch(
                batch.assistant_text,
                batch.calls,
                results,
                batch.model,
                batch.step_id,
                batch.journal_status,
                true,
                batch.turn,
                batch.thinking_message,
            );
            return;
        }

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
                batch.journal_status,
                true,
                batch.turn,
                batch.thinking_message,
            );
            return;
        }

        let remaining_capacity = self.remaining_tool_capacity(&batch);
        let phase = match self.start_tool_execution(
            batch.journal_status.batch_id(),
            queue,
            remaining_capacity,
            batch.turn.recorder(),
        ) {
            Ok(phase) => phase,
            Err(e) => {
                self.disable_tools_due_to_tool_journal_error("mark tool call started", e);

                let mut results = batch.results;
                let existing: HashSet<String> =
                    results.iter().map(|r| r.tool_call_id.clone()).collect();
                for call in &batch.calls {
                    if existing.contains(&call.id) {
                        continue;
                    }
                    results.push(ToolResult::error(
                        call.id.clone(),
                        call.name.clone(),
                        "Tool execution stopped: tool journal error",
                    ));
                }

                self.commit_tool_batch(
                    batch.assistant_text,
                    batch.calls,
                    results,
                    batch.model,
                    batch.step_id,
                    batch.journal_status,
                    true,
                    batch.turn,
                    batch.thinking_message,
                );
                return;
            }
        };
        self.state = OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase }));
    }

    pub(crate) fn resolve_tool_recovery(&mut self, decision: ToolRecoveryDecision) {
        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
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
            let _ =
                self.record_tool_result_or_disable(batch.batch_id, result, "recovery tool result");
        }

        let auto_resume = true;
        self.commit_tool_batch(
            assistant_text,
            batch.calls,
            results,
            model,
            step_id,
            JournalStatus::new(batch.batch_id),
            auto_resume,
            TurnContext::new_for_recovery(),
            None,
        );

        match decision {
            ToolRecoveryDecision::Resume => {
                self.push_notification("Recovered tool batch finalized");
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

        // Notify LSP servers about file changes
        self.notify_lsp_file_changes(&created, &modified);

        // Aggregate turn changes into session-wide log
        self.session_changes.merge_turn(&created, &modified);

        // Sync files panel selection after file list changes
        self.files_panel_sync_selection();

        if let TurnChangeReport::Changes(distillate) = report {
            let msg = distillate.into_message();
            self.push_local_message(Message::system(msg));
        }
        self.pending_user_message = None;

        // Transfer turn usage to last_turn_usage for display, reset for next turn
        if self.turn_usage.is_some() {
            self.last_turn_usage = self.turn_usage.take();
        }
        self.tool_iterations = 0;
    }
}

fn preflight_sandbox(
    sandbox: &tools::sandbox::Sandbox,
    call: &ToolCall,
) -> Result<(), tools::ToolError> {
    let working_dir = sandbox.working_dir();
    match call.name.as_str() {
        "Read" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| tools::ToolError::BadArgs {
                    message: "path must be a string".to_string(),
                })?;
            let _ = sandbox.resolve_path(path, &working_dir)?;
        }
        "Edit" => {
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
        "Write" => {
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

/// Strip Windows extended path prefix (`\\?\`) for cleaner display.
fn strip_windows_prefix(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
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
        tools::ToolError::UnknownTool { name } => format!("Unknown tool: {name}"),
        tools::ToolError::DuplicateTool { name } => format!("Duplicate tool: {name}"),
        tools::ToolError::DuplicateToolCallId { id } => {
            format!("Duplicate tool call id: {id}")
        }
        tools::ToolError::PatchFailed { file, message } => {
            format!(
                "Patch failed for {}: {message}",
                strip_windows_prefix(&file)
            )
        }
        tools::ToolError::StaleFile { file, reason } => {
            format!("{}: {reason}", strip_windows_prefix(&file))
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

fn apply_tool_event_to_output_lines(
    output_lines: &mut HashMap<String, Vec<String>>,
    event: tools::ToolEvent,
) {
    match event {
        tools::ToolEvent::Started {
            tool_call_id,
            tool_name,
        } => {
            let lines = output_lines.entry(tool_call_id.clone()).or_default();
            lines.push(format!(
                "▶ {} ({})",
                tools::sanitize_output(&tool_name),
                tool_call_id
            ));
        }
        tools::ToolEvent::ProcessSpawned {
            tool_call_id, pid, ..
        } => {
            let lines = output_lines.entry(tool_call_id.clone()).or_default();
            lines.push(format!("  -> pid {pid}"));
        }
        tools::ToolEvent::StdoutChunk {
            tool_call_id,
            chunk,
        } => {
            let lines = output_lines.entry(tool_call_id).or_default();
            append_tool_output_lines(lines, &tools::sanitize_output(&chunk), None);
        }
        tools::ToolEvent::StderrChunk {
            tool_call_id,
            chunk,
        } => {
            let lines = output_lines.entry(tool_call_id).or_default();
            append_tool_output_lines(lines, &tools::sanitize_output(&chunk), Some("[stderr] "));
        }
        tools::ToolEvent::Completed { tool_call_id } => {
            let lines = output_lines.entry(tool_call_id.clone()).or_default();
            lines.push(format!("✓ Tool completed ({tool_call_id})"));
        }
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

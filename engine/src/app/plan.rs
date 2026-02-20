//! Plan tool dispatch — intercepts `Plan` tool calls before executor dispatch.
//!
//! The Plan tool is schema-only (no `ToolExecutor`). The engine resolves all
//! Plan subcommands here and returns `ToolResult` directly.

use std::mem::take;

use forge_types::plan::editor::StepTransitionError;
use forge_types::plan::{ActiveStepQuery, CompletionStatus, editor};
use forge_types::{
    EditOp, NonEmptyString, PhaseInput, Plan, PlanState, PlanStepId, StepInput, ToolCall,
    ToolResult,
};
use serde_json::Value;

use crate::App;
use crate::state::{OperationTag, PlanApprovalKind};

/// Name of the Plan tool (must match the schema registration in builtins).
pub(crate) const PLAN_TOOL_NAME: &str = "Plan";

pub(crate) enum PlanCallResult {
    Resolved(ToolResult),
    NeedsApproval { kind: PlanApprovalKind },
}

pub(crate) struct PlanResolution {
    pub(crate) pre_resolved: Vec<ToolResult>,
    pub(crate) pending_approval: Option<PendingPlanApproval>,
}

pub(crate) struct PendingPlanApproval {
    pub(crate) tool_call_id: String,
    pub(crate) kind: PlanApprovalKind,
}

impl App {
    /// Intercept Plan tool calls and resolve them before normal tool planning.
    ///
    /// Returns a `PlanResolution` with pre-resolved results and an optional
    /// pending approval (only one plan create/edit per batch may pend).
    pub(crate) fn resolve_plan_tool_calls(
        &mut self,
        calls: &[ToolCall],
        mut pre_resolved: Vec<ToolResult>,
    ) -> PlanResolution {
        let mut pending_approval: Option<PendingPlanApproval> = None;

        for call in calls {
            if call.name != PLAN_TOOL_NAME {
                continue;
            }
            let result = self.dispatch_plan_call(call);
            match result {
                PlanCallResult::Resolved(tool_result) => {
                    pre_resolved.push(tool_result);
                }
                PlanCallResult::NeedsApproval { kind } => {
                    if pending_approval.is_some() {
                        pre_resolved.push(ToolResult::error(
                            &call.id,
                            PLAN_TOOL_NAME,
                            "Only one plan create/edit per batch is allowed.",
                        ));
                    } else {
                        pending_approval = Some(PendingPlanApproval {
                            tool_call_id: call.id.clone(),
                            kind,
                        });
                    }
                }
            }
        }

        PlanResolution {
            pre_resolved,
            pending_approval,
        }
    }

    fn dispatch_plan_call(&mut self, call: &ToolCall) -> PlanCallResult {
        let subcommand = call
            .arguments
            .get("subcommand")
            .and_then(Value::as_str)
            .unwrap_or("");

        match subcommand {
            "create" => self.plan_create(call),
            "advance" => PlanCallResult::Resolved(self.plan_advance(call)),
            "skip" => PlanCallResult::Resolved(self.plan_skip(call)),
            "fail" => PlanCallResult::Resolved(self.plan_fail(call)),
            "edit" => self.plan_edit(call),
            "status" => PlanCallResult::Resolved(self.plan_status(call)),
            other => PlanCallResult::Resolved(ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                format!(
                    "Unknown subcommand '{other}'. \
                     Valid subcommands: create, advance, skip, fail, edit, status."
                ),
            )),
        }
    }

    fn plan_create(&mut self, call: &ToolCall) -> PlanCallResult {
        if matches!(self.core.plan_state, PlanState::Active(_)) {
            return PlanCallResult::Resolved(ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                "A plan is already active. Complete or clear the current plan before creating a new one.",
            ));
        }

        let phases_val = match call.arguments.get("phases") {
            Some(v) => v,
            None => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    "'create' requires a 'phases' array.",
                ));
            }
        };

        let phase_inputs: Vec<PhaseInput> = match serde_json::from_value(phases_val.clone()) {
            Ok(v) => v,
            Err(e) => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!("Invalid 'phases' format: {e}"),
                ));
            }
        };

        let plan = match Plan::from_input(phase_inputs) {
            Ok(p) => p,
            Err(e) => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!("Plan validation failed: {e}"),
                ));
            }
        };

        self.core.plan_state = PlanState::Proposed(plan);
        PlanCallResult::NeedsApproval {
            kind: PlanApprovalKind::Create,
        }
    }

    fn plan_advance(&mut self, call: &ToolCall) -> ToolResult {
        let (step_id, outcome) = match parse_step_id_and_text(call, "outcome") {
            Ok(v) => v,
            Err(result) => return result,
        };

        // Scope the plan borrow so we can call self.create_plan_step_checkpoint after.
        let (rendered, completion) = {
            let plan = match &mut self.core.plan_state {
                PlanState::Active(plan) => plan,
                PlanState::Proposed(_) => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        "Plan is proposed but not yet approved. Wait for user approval.",
                    );
                }
                PlanState::Inactive => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        "No active plan. Create one first with 'create'.",
                    );
                }
            };

            if let Err(err) = editor::complete_active_step(plan, step_id, outcome) {
                return match err {
                    StepTransitionError::StepNotFound { .. } => ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} not found in the plan."),
                    ),
                    StepTransitionError::StepNotActive { .. } => ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} is not Active. Only Active steps can be advanced."),
                    ),
                    StepTransitionError::InvalidTransition(e) => {
                        ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string())
                    }
                };
            }

            // Auto-activate next eligible step.
            editor::activate_next_eligible(plan);

            let rendered = plan.render();
            let completion = match plan.try_complete() {
                CompletionStatus::Complete(c) => Some((c.phase_count(), c.step_count())),
                CompletionStatus::Incomplete { .. } => None,
            };
            (rendered, completion)
        };

        self.create_plan_step_checkpoint(step_id);

        if let Some((phases, steps)) = completion {
            self.push_notification(format!("Plan complete! {phases} phases, {steps} steps."));
            return ToolResult::success(
                &call.id,
                PLAN_TOOL_NAME,
                format!("Plan complete! {phases} phases, {steps} steps.\n\n{rendered}"),
            );
        }

        if let PlanState::Active(plan) = &self.core.plan_state
            && let ActiveStepQuery::Active(next) = plan.active_step()
        {
            let step = next.step();
            self.push_notification(format!(
                "Step {step_id} complete \u{2192} Step {}: {}",
                step.id(),
                step.description()
            ));
        }

        ToolResult::success(
            &call.id,
            PLAN_TOOL_NAME,
            format!("Step {step_id} completed.\n\n{rendered}"),
        )
    }

    fn plan_skip(&mut self, call: &ToolCall) -> ToolResult {
        let (step_id, reason) = match parse_step_id_and_text(call, "reason") {
            Ok(v) => v,
            Err(result) => return result,
        };

        let rendered = {
            let plan = match &mut self.core.plan_state {
                PlanState::Active(plan) => plan,
                _ => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        "No active plan. 'skip' requires an active plan.",
                    );
                }
            };

            if let Err(err) = editor::skip_active_step(plan, step_id, reason) {
                return match err {
                    StepTransitionError::StepNotFound { .. } => ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} not found in the plan."),
                    ),
                    StepTransitionError::StepNotActive { .. } => ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} is not Active. Only Active steps can be skipped."),
                    ),
                    StepTransitionError::InvalidTransition(e) => {
                        ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string())
                    }
                };
            }

            editor::activate_next_eligible(plan);
            plan.render()
        };

        self.create_plan_step_checkpoint(step_id);

        if let PlanState::Active(plan) = &self.core.plan_state
            && let ActiveStepQuery::Active(next) = plan.active_step()
        {
            let step = next.step();
            self.push_notification(format!(
                "Step {step_id} skipped \u{2192} Step {}: {}",
                step.id(),
                step.description()
            ));
        }

        ToolResult::success(
            &call.id,
            PLAN_TOOL_NAME,
            format!("Step {step_id} skipped.\n\n{rendered}"),
        )
    }

    fn plan_fail(&mut self, call: &ToolCall) -> ToolResult {
        let plan = match &mut self.core.plan_state {
            PlanState::Active(plan) => plan,
            _ => {
                return ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    "No active plan. 'fail' requires an active plan.",
                );
            }
        };

        let (step_id, reason) = match parse_step_id_and_text(call, "reason") {
            Ok(v) => v,
            Err(result) => return result,
        };

        if let Err(err) = editor::fail_active_step(plan, step_id, reason) {
            return match err {
                StepTransitionError::StepNotFound { .. } => ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!("Step {step_id} not found in the plan."),
                ),
                StepTransitionError::StepNotActive { .. } => ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!(
                        "Step {step_id} is not Active. Only Active steps can be marked as failed."
                    ),
                ),
                StepTransitionError::InvalidTransition(e) => {
                    ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string())
                }
            };
        }

        let rendered = plan.render();
        ToolResult::success(
            &call.id,
            PLAN_TOOL_NAME,
            format!("Step {step_id} marked as failed. Awaiting user decision.\n\n{rendered}"),
        )
    }

    fn plan_edit(&mut self, call: &ToolCall) -> PlanCallResult {
        let plan = match &self.core.plan_state {
            PlanState::Active(plan) => plan,
            _ => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    "No active plan. 'edit' requires an active plan.",
                ));
            }
        };

        let Some(_justification) = call
            .arguments
            .get("justification")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return PlanCallResult::Resolved(ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                "'edit' requires a non-empty 'justification'.",
            ));
        };

        let edit_op_val = match call.arguments.get("edit_op") {
            Some(v) => v,
            None => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    "'edit' requires an 'edit_op' object.",
                ));
            }
        };

        let edit_op = match parse_edit_op(edit_op_val) {
            Ok(op) => op,
            Err(msg) => {
                return PlanCallResult::Resolved(ToolResult::error(&call.id, PLAN_TOOL_NAME, msg));
            }
        };

        let edited_plan = match editor::apply(plan.clone(), edit_op) {
            Ok(plan) => plan,
            Err(e) => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!("Edit failed: {e}"),
                ));
            }
        };

        PlanCallResult::NeedsApproval {
            kind: PlanApprovalKind::Edit { edited_plan },
        }
    }

    fn plan_status(&self, call: &ToolCall) -> ToolResult {
        let content = match &self.core.plan_state {
            PlanState::Inactive => "No active plan.".to_string(),
            PlanState::Proposed(plan) => {
                format!("[Proposed — awaiting approval]\n\n{}", plan.render())
            }
            PlanState::Active(plan) => plan.render(),
        };

        ToolResult::success(&call.id, PLAN_TOOL_NAME, content)
    }

    pub(crate) fn resolve_plan_approval(&mut self, approved: bool) {
        use crate::state::{
            ApprovalState, OperationState, PlanApprovalState, ToolLoopPhase, ToolLoopState,
        };

        let state = match self.op_take_plan_approval() {
            super::OperationTake::Taken(state) => *state,
            super::OperationTake::Skipped => return,
        };

        let PlanApprovalState {
            tool_call_id,
            kind,
            mut batch,
            pending_tool_approvals,
        } = state;

        let result = if approved {
            match kind {
                PlanApprovalKind::Create => {
                    let plan = match take(&mut self.core.plan_state) {
                        PlanState::Proposed(plan) => plan,
                        other => {
                            self.core.plan_state = other;
                            batch.results.push(ToolResult::error(
                                &tool_call_id,
                                PLAN_TOOL_NAME,
                                "Plan state inconsistency: expected Proposed.",
                            ));
                            self.cancel_tool_batch(batch);
                            return;
                        }
                    };
                    let rendered = plan.render();
                    let mut plan = plan;
                    editor::activate_next_eligible(&mut plan);
                    self.core.plan_state = PlanState::Active(plan);
                    ToolResult::success(
                        &tool_call_id,
                        PLAN_TOOL_NAME,
                        format!("Plan approved and activated.\n\n{rendered}"),
                    )
                }
                PlanApprovalKind::Edit { edited_plan } => {
                    let rendered = edited_plan.render();
                    self.core.plan_state = PlanState::Active(edited_plan);
                    ToolResult::success(
                        &tool_call_id,
                        PLAN_TOOL_NAME,
                        format!("Plan edit approved.\n\n{rendered}"),
                    )
                }
            }
        } else {
            match kind {
                PlanApprovalKind::Create => {
                    self.core.plan_state = PlanState::Inactive;
                    ToolResult::error(&tool_call_id, PLAN_TOOL_NAME, "Plan rejected by user.")
                }
                PlanApprovalKind::Edit { .. } => {
                    ToolResult::error(&tool_call_id, PLAN_TOOL_NAME, "Plan edit rejected by user.")
                }
            }
        };

        let batch_id = batch.journal_status.batch_id();
        if !self.record_tool_result_or_disable(batch_id, &result, "plan approval result") {
            batch.results.push(result);
            self.cancel_tool_batch(batch);
            return;
        }
        batch.results.push(result);

        if !pending_tool_approvals.is_empty() {
            let approval = ApprovalState::new(pending_tool_approvals);
            self.op_transition_from(
                OperationTag::PlanApproval,
                OperationState::ToolLoop(Box::new(ToolLoopState {
                    batch,
                    phase: ToolLoopPhase::AwaitingApproval(approval),
                })),
            );
            return;
        }

        let calls_to_execute = batch.execute_now.clone();
        if calls_to_execute.is_empty() {
            let journal_status = batch.journal_status.clone();
            self.commit_tool_batch(batch.into_commit(), journal_status, true);
            return;
        }

        let remaining_capacity = self.remaining_tool_capacity(&batch);
        let phase = match self.start_tool_execution(
            batch.journal_status.batch_id(),
            calls_to_execute,
            remaining_capacity,
            batch.turn.recorder(),
            batch.batch_start,
        ) {
            Ok(phase) => phase,
            Err(e) => {
                self.disable_tools_due_to_tool_journal_error("mark tool call started", e);
                self.cancel_tool_batch(batch);
                return;
            }
        };
        self.op_transition_from(
            OperationTag::PlanApproval,
            OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase })),
        );
    }
}

/// Parse `step_id` and a required text field from tool call arguments.
fn parse_step_id_and_text(
    call: &ToolCall,
    text_field: &str,
) -> Result<(PlanStepId, NonEmptyString), ToolResult> {
    let step_id_raw = call
        .arguments
        .get("step_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                format!("'{text_field}' subcommand requires an integer 'step_id'."),
            )
        })?;

    let step_id = PlanStepId::try_from(step_id_raw).map_err(|_err| {
        ToolResult::error(
            &call.id,
            PLAN_TOOL_NAME,
            "'step_id' must be a non-zero 32-bit integer.",
        )
    })?;

    let raw_text = call
        .arguments
        .get(text_field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                format!("'{text_field}' is required and must be non-empty."),
            )
        })?;

    let text = NonEmptyString::new(raw_text.to_string()).map_err(|_err| {
        ToolResult::error(
            &call.id,
            PLAN_TOOL_NAME,
            format!("'{text_field}' must not be empty or whitespace-only."),
        )
    })?;

    Ok((step_id, text))
}

/// Parse an `edit_op` JSON value into an `EditOp`.
fn parse_edit_op(val: &serde_json::Value) -> Result<EditOp, String> {
    let op_type = val
        .get("type")
        .and_then(Value::as_str)
        .ok_or("'edit_op.type' is required.")?;

    match op_type {
        "add_step" => {
            let phase_index = val
                .get("phase_index")
                .and_then(Value::as_u64)
                .ok_or("'add_step' requires 'phase_index' (integer).")?
                as usize;
            let step_val = val
                .get("step")
                .ok_or("'add_step' requires a 'step' object.")?;
            let step: StepInput = serde_json::from_value(step_val.clone())
                .map_err(|e| format!("Invalid 'step': {e}"))?;
            Ok(EditOp::AddStep { phase_index, step })
        }
        "remove_step" => {
            let step_id_raw = val
                .get("step_id")
                .and_then(Value::as_u64)
                .ok_or("'remove_step' requires 'step_id' (integer).")?;
            let step_id = PlanStepId::try_from(step_id_raw)
                .map_err(|_err| "'remove_step.step_id' must be a non-zero 32-bit integer.")?;
            Ok(EditOp::RemoveStep(step_id))
        }
        "reorder_step" => {
            let step_id_raw = val
                .get("step_id")
                .and_then(Value::as_u64)
                .ok_or("'reorder_step' requires 'step_id' (integer).")?;
            let step_id = PlanStepId::try_from(step_id_raw)
                .map_err(|_err| "'reorder_step.step_id' must be a non-zero 32-bit integer.")?;
            let new_phase = val
                .get("new_phase")
                .and_then(Value::as_u64)
                .ok_or("'reorder_step' requires 'new_phase' (integer).")?
                as usize;
            Ok(EditOp::ReorderStep { step_id, new_phase })
        }
        "update_description" => {
            let step_id_raw = val
                .get("step_id")
                .and_then(Value::as_u64)
                .ok_or("'update_description' requires 'step_id' (integer).")?;
            let step_id = PlanStepId::try_from(step_id_raw).map_err(
                |_err| "'update_description.step_id' must be a non-zero 32-bit integer.",
            )?;
            let description = val
                .get("description")
                .and_then(Value::as_str)
                .ok_or("'update_description' requires 'description' (string).")?
                .to_string();
            Ok(EditOp::UpdateDescription {
                step_id,
                description,
            })
        }
        "add_phase" => {
            let index = val
                .get("phase_index")
                .and_then(Value::as_u64)
                .ok_or("'add_phase' requires 'phase_index' (integer).")?
                as usize;
            let phase_val = val
                .get("phase")
                .ok_or("'add_phase' requires a 'phase' object.")?;
            let phase: PhaseInput = serde_json::from_value(phase_val.clone())
                .map_err(|e| format!("Invalid 'phase': {e}"))?;
            Ok(EditOp::AddPhase { index, phase })
        }
        "remove_phase" => {
            let index = val
                .get("phase_index")
                .and_then(Value::as_u64)
                .ok_or("'remove_phase' requires 'phase_index' (integer).")?
                as usize;
            Ok(EditOp::RemovePhase(index))
        }
        other => Err(format!(
            "Unknown edit_op type '{other}'. \
             Valid types: add_step, remove_step, reorder_step, update_description, add_phase, remove_phase."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{PLAN_TOOL_NAME, parse_edit_op, parse_step_id_and_text};
    use forge_types::plan::editor;
    use forge_types::{
        EditOp, NonEmptyString, PhaseInput, Plan, PlanStepId, StepInput, ThoughtSignatureState,
        ToolCall,
    };
    use serde_json::json;

    fn tool_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            name: PLAN_TOOL_NAME.to_string(),
            arguments: args,
            thought_signature: ThoughtSignatureState::Unsigned,
        }
    }

    #[test]
    fn parse_step_id_and_text_valid() {
        let call = tool_call(json!({ "step_id": 3, "outcome": "Did the thing" }));
        let (id, text) = parse_step_id_and_text(&call, "outcome").unwrap();
        assert_eq!(id, plan_step_id(3));
        assert_eq!(text.as_str(), "Did the thing");
    }

    #[test]
    fn parse_step_id_and_text_missing_step_id() {
        let call = tool_call(json!({ "outcome": "Did the thing" }));
        let err = parse_step_id_and_text(&call, "outcome").unwrap_err();
        assert!(err.is_error);
    }

    #[test]
    fn parse_step_id_and_text_empty_text() {
        let call = tool_call(json!({ "step_id": 1, "outcome": "" }));
        let err = parse_step_id_and_text(&call, "outcome").unwrap_err();
        assert!(err.is_error);
    }

    #[test]
    fn parse_step_id_and_text_whitespace_text() {
        let call = tool_call(json!({ "step_id": 1, "outcome": "   " }));
        let err = parse_step_id_and_text(&call, "outcome").unwrap_err();
        assert!(err.is_error);
    }

    #[test]
    fn parse_step_id_and_text_zero_step_id() {
        let call = tool_call(json!({ "step_id": 0, "outcome": "Did the thing" }));
        let err = parse_step_id_and_text(&call, "outcome").unwrap_err();
        assert!(err.is_error);
    }

    #[test]
    fn parse_edit_op_add_step() {
        let val = json!({
            "type": "add_step",
            "phase_index": 0,
            "step": { "description": "New step" }
        });
        let op = parse_edit_op(&val).unwrap();
        assert!(matches!(op, EditOp::AddStep { phase_index: 0, .. }));
    }

    #[test]
    fn parse_edit_op_remove_step() {
        let val = json!({ "type": "remove_step", "step_id": 5 });
        let op = parse_edit_op(&val).unwrap();
        assert!(matches!(op, EditOp::RemoveStep(id) if id == plan_step_id(5)));
    }

    #[test]
    fn parse_edit_op_reorder_step() {
        let val = json!({ "type": "reorder_step", "step_id": 2, "new_phase": 1 });
        let op = parse_edit_op(&val).unwrap();
        assert!(matches!(
            op,
            EditOp::ReorderStep {
                step_id,
                new_phase: 1
            } if step_id == plan_step_id(2)
        ));
    }

    #[test]
    fn parse_edit_op_update_description() {
        let val = json!({
            "type": "update_description",
            "step_id": 3,
            "description": "Updated text"
        });
        let op = parse_edit_op(&val).unwrap();
        assert!(
            matches!(op, EditOp::UpdateDescription { step_id, description }
                if step_id == plan_step_id(3) && description == "Updated text"
            )
        );
    }

    #[test]
    fn parse_edit_op_add_phase() {
        let val = json!({
            "type": "add_phase",
            "phase_index": 1,
            "phase": {
                "name": "New phase",
                "steps": [{ "description": "Step A" }]
            }
        });
        let op = parse_edit_op(&val).unwrap();
        assert!(matches!(op, EditOp::AddPhase { index: 1, .. }));
    }

    #[test]
    fn parse_edit_op_remove_phase() {
        let val = json!({ "type": "remove_phase", "phase_index": 2 });
        let op = parse_edit_op(&val).unwrap();
        assert!(matches!(op, EditOp::RemovePhase(2)));
    }

    #[test]
    fn parse_edit_op_unknown_type() {
        let val = json!({ "type": "destroy_everything" });
        let err = parse_edit_op(&val).unwrap_err();
        assert!(err.contains("Unknown edit_op type"));
    }

    #[test]
    fn parse_edit_op_missing_type() {
        let val = json!({ "phase_index": 0 });
        let err = parse_edit_op(&val).unwrap_err();
        assert!(err.contains("type"));
    }

    #[test]
    fn activate_next_eligible_activates_first() {
        let mut plan = Plan::from_input(vec![PhaseInput {
            name: "Phase 1".to_string(),
            steps: vec![
                StepInput {
                    description: "Step A".to_string(),
                    depends_on: vec![],
                },
                StepInput {
                    description: "Step B".to_string(),
                    depends_on: vec![],
                },
            ],
        }])
        .unwrap();

        editor::activate_next_eligible(&mut plan);
        let s = editor::resolve_step(&plan, plan_step_id(1)).unwrap();
        assert!(s.is_active());
        let s2 = editor::resolve_step(&plan, plan_step_id(2)).unwrap();
        assert!(s2.is_pending());
    }

    #[test]
    fn activate_next_eligible_noop_when_active_exists() {
        let mut plan = Plan::from_input(vec![PhaseInput {
            name: "Phase 1".to_string(),
            steps: vec![
                StepInput {
                    description: "Step A".to_string(),
                    depends_on: vec![],
                },
                StepInput {
                    description: "Step B".to_string(),
                    depends_on: vec![],
                },
            ],
        }])
        .unwrap();

        // Manually activate step 1.
        editor::activate_step(&mut plan, plan_step_id(1)).unwrap();

        editor::activate_next_eligible(&mut plan);
        // Step 2 should still be Pending (only one active at a time).
        let s2 = editor::resolve_step(&plan, plan_step_id(2)).unwrap();
        assert!(s2.is_pending());
    }

    #[test]
    fn activate_next_eligible_respects_dependencies() {
        let mut plan = Plan::from_input(vec![
            PhaseInput {
                name: "Phase 1".to_string(),
                steps: vec![StepInput {
                    description: "Step A".to_string(),
                    depends_on: vec![],
                }],
            },
            PhaseInput {
                name: "Phase 2".to_string(),
                steps: vec![StepInput {
                    description: "Step B".to_string(),
                    depends_on: vec![plan_step_id(1)],
                }],
            },
        ])
        .unwrap();

        // Phase 1 not complete yet — Phase 2 steps should not activate.
        editor::activate_next_eligible(&mut plan);
        // Should activate step 1 (Phase 1's eligible step).
        let s1 = editor::resolve_step(&plan, plan_step_id(1)).unwrap();
        assert!(s1.is_active());
        let s2 = editor::resolve_step(&plan, plan_step_id(2)).unwrap();
        assert!(s2.is_pending());
    }

    #[test]
    fn activate_next_eligible_advances_to_next_phase() {
        let mut plan = Plan::from_input(vec![
            PhaseInput {
                name: "Phase 1".to_string(),
                steps: vec![StepInput {
                    description: "Step A".to_string(),
                    depends_on: vec![],
                }],
            },
            PhaseInput {
                name: "Phase 2".to_string(),
                steps: vec![StepInput {
                    description: "Step B".to_string(),
                    depends_on: vec![],
                }],
            },
        ])
        .unwrap();

        // Complete phase 1.
        editor::activate_step(&mut plan, plan_step_id(1)).unwrap();
        editor::complete_active_step(&mut plan, plan_step_id(1), non_empty("done")).unwrap();

        editor::activate_next_eligible(&mut plan);
        let s2 = editor::resolve_step(&plan, plan_step_id(2)).unwrap();
        assert!(s2.is_active());
    }

    fn non_empty(value: &str) -> NonEmptyString {
        NonEmptyString::new(value).expect("test fixture must be non-empty")
    }

    fn plan_step_id(value: u32) -> PlanStepId {
        PlanStepId::try_new(value).expect("test fixture must use non-zero plan step ids")
    }
}

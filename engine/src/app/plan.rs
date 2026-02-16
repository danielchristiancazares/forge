//! Plan tool dispatch — intercepts `Plan` tool calls before executor dispatch.
//!
//! The Plan tool is schema-only (no `ToolExecutor`). The engine resolves all
//! Plan subcommands here and returns `ToolResult` directly.

use forge_types::{
    EditOp, PhaseInput, Plan, PlanState, PlanStepId, StepInput, StepStatus, ToolCall, ToolResult,
};
use serde_json::Value;

use crate::App;
use crate::state::PlanApprovalKind;

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
        if self.plan_state.is_active() {
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

        self.plan_state = PlanState::Proposed(plan);
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
            let plan = match &mut self.plan_state {
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

            // Validate: step must exist and be Active.
            let step = match plan.step(step_id) {
                Some(s) => s,
                None => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} not found in the plan."),
                    );
                }
            };

            if !matches!(step.status, StepStatus::Active) {
                return ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!(
                        "Step {step_id} is {:?}, not Active. Only Active steps can be advanced.",
                        step.status
                    ),
                );
            }

            // Transition to Complete.
            if let Err(e) = plan
                .step_mut(step_id)
                .expect("step existence verified")
                .transition(StepStatus::Complete(outcome))
            {
                return ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string());
            }

            // Auto-activate next eligible step.
            activate_next_eligible(plan);

            let rendered = plan.render();
            let completion = plan
                .try_complete()
                .map(|c| (c.phase_count(), c.step_count()));
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

        if let Some(plan) = self.plan_state.plan()
            && let Some(next) = plan.active_step()
        {
            self.push_notification(format!(
                "Step {step_id} complete \u{2192} Step {}: {}",
                next.id, next.description
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
            let plan = match &mut self.plan_state {
                PlanState::Active(plan) => plan,
                _ => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        "No active plan. 'skip' requires an active plan.",
                    );
                }
            };

            let step = match plan.step(step_id) {
                Some(s) => s,
                None => {
                    return ToolResult::error(
                        &call.id,
                        PLAN_TOOL_NAME,
                        format!("Step {step_id} not found in the plan."),
                    );
                }
            };

            if !matches!(step.status, StepStatus::Active) {
                return ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!(
                        "Step {step_id} is {:?}, not Active. Only Active steps can be skipped.",
                        step.status
                    ),
                );
            }

            if let Err(e) = plan
                .step_mut(step_id)
                .expect("step existence verified")
                .transition(StepStatus::Skipped(reason))
            {
                return ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string());
            }

            activate_next_eligible(plan);
            plan.render()
        };

        self.create_plan_step_checkpoint(step_id);

        if let Some(plan) = self.plan_state.plan()
            && let Some(next) = plan.active_step()
        {
            self.push_notification(format!(
                "Step {step_id} skipped \u{2192} Step {}: {}",
                next.id, next.description
            ));
        }

        ToolResult::success(
            &call.id,
            PLAN_TOOL_NAME,
            format!("Step {step_id} skipped.\n\n{rendered}"),
        )
    }

    fn plan_fail(&mut self, call: &ToolCall) -> ToolResult {
        let plan = match &mut self.plan_state {
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

        let step = match plan.step(step_id) {
            Some(s) => s,
            None => {
                return ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    format!("Step {step_id} not found in the plan."),
                );
            }
        };

        if !matches!(step.status, StepStatus::Active) {
            return ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                format!(
                    "Step {step_id} is {:?}, not Active. Only Active steps can be marked as failed.",
                    step.status
                ),
            );
        }

        if let Err(e) = plan
            .step_mut(step_id)
            .expect("step existence verified")
            .transition(StepStatus::Failed(reason))
        {
            return ToolResult::error(&call.id, PLAN_TOOL_NAME, e.to_string());
        }

        let rendered = plan.render();
        ToolResult::success(
            &call.id,
            PLAN_TOOL_NAME,
            format!("Step {step_id} marked as failed. Awaiting user decision.\n\n{rendered}"),
        )
    }

    fn plan_edit(&mut self, call: &ToolCall) -> PlanCallResult {
        let plan = match &mut self.plan_state {
            PlanState::Active(plan) => plan,
            _ => {
                return PlanCallResult::Resolved(ToolResult::error(
                    &call.id,
                    PLAN_TOOL_NAME,
                    "No active plan. 'edit' requires an active plan.",
                ));
            }
        };

        let justification = call
            .arguments
            .get("justification")
            .and_then(Value::as_str)
            .unwrap_or("");
        if justification.is_empty() {
            return PlanCallResult::Resolved(ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                "'edit' requires a non-empty 'justification'.",
            ));
        }

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

        let pre_edit_plan = plan.clone();

        if let Err(e) = plan.apply_edit(edit_op) {
            // Restore from clone — apply_edit mutates in place before validation,
            // so a validation failure leaves the plan in a corrupted state.
            *plan = pre_edit_plan;
            return PlanCallResult::Resolved(ToolResult::error(
                &call.id,
                PLAN_TOOL_NAME,
                format!("Edit failed: {e}"),
            ));
        }

        PlanCallResult::NeedsApproval {
            kind: PlanApprovalKind::Edit { pre_edit_plan },
        }
    }

    fn plan_status(&self, call: &ToolCall) -> ToolResult {
        let content = match &self.plan_state {
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

        let idle = self.idle_state();
        let state = match std::mem::replace(&mut self.state, idle) {
            OperationState::PlanApproval(state) => *state,
            other => {
                self.op_restore(other);
                return;
            }
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
                    let plan = match std::mem::take(&mut self.plan_state) {
                        PlanState::Proposed(plan) => plan,
                        other => {
                            self.plan_state = other;
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
                    activate_next_eligible(&mut plan);
                    self.plan_state = PlanState::Active(plan);
                    ToolResult::success(
                        &tool_call_id,
                        PLAN_TOOL_NAME,
                        format!("Plan approved and activated.\n\n{rendered}"),
                    )
                }
                PlanApprovalKind::Edit { .. } => {
                    let rendered = self.plan_state.plan().map(Plan::render).unwrap_or_default();
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
                    self.plan_state = PlanState::Inactive;
                    ToolResult::error(&tool_call_id, PLAN_TOOL_NAME, "Plan rejected by user.")
                }
                PlanApprovalKind::Edit { pre_edit_plan } => {
                    if let PlanState::Active(plan) = &mut self.plan_state {
                        *plan = pre_edit_plan;
                    }
                    ToolResult::error(
                        &tool_call_id,
                        PLAN_TOOL_NAME,
                        "Plan edit rejected by user. Plan reverted.",
                    )
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
                crate::state::OperationTag::PlanApproval,
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
        ) {
            Ok(phase) => phase,
            Err(e) => {
                self.disable_tools_due_to_tool_journal_error("mark tool call started", e);
                self.cancel_tool_batch(batch);
                return;
            }
        };
        self.op_transition_from(
            crate::state::OperationTag::PlanApproval,
            OperationState::ToolLoop(Box::new(ToolLoopState { batch, phase })),
        );
    }
}

/// Parse `step_id` and a required text field from tool call arguments.
fn parse_step_id_and_text(
    call: &ToolCall,
    text_field: &str,
) -> Result<(PlanStepId, String), ToolResult> {
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

    let step_id = PlanStepId::new(step_id_raw as u32);

    let text = call
        .arguments
        .get(text_field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        return Err(ToolResult::error(
            &call.id,
            PLAN_TOOL_NAME,
            format!("'{text_field}' is required and must be non-empty."),
        ));
    }

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
            let step_id =
                val.get("step_id")
                    .and_then(Value::as_u64)
                    .ok_or("'remove_step' requires 'step_id' (integer).")? as u32;
            Ok(EditOp::RemoveStep(PlanStepId::new(step_id)))
        }
        "reorder_step" => {
            let step_id =
                val.get("step_id")
                    .and_then(Value::as_u64)
                    .ok_or("'reorder_step' requires 'step_id' (integer).")? as u32;
            let new_phase = val
                .get("new_phase")
                .and_then(Value::as_u64)
                .ok_or("'reorder_step' requires 'new_phase' (integer).")?
                as usize;
            Ok(EditOp::ReorderStep {
                step_id: PlanStepId::new(step_id),
                new_phase,
            })
        }
        "update_description" => {
            let step_id = val
                .get("step_id")
                .and_then(Value::as_u64)
                .ok_or("'update_description' requires 'step_id' (integer).")?
                as u32;
            let description = val
                .get("description")
                .and_then(Value::as_str)
                .ok_or("'update_description' requires 'description' (string).")?
                .to_string();
            Ok(EditOp::UpdateDescription {
                step_id: PlanStepId::new(step_id),
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

/// Activate the first eligible pending step in the current phase.
///
/// Per spec: only one step may be `Active` at a time within a phase.
/// If no step is currently active, this activates the first eligible pending step.
fn activate_next_eligible(plan: &mut Plan) {
    if plan.active_step().is_some() {
        return;
    }

    let eligible = plan.eligible_steps();
    if let Some(next_id) = eligible.first()
        && let Some(step) = plan.step_mut(*next_id)
    {
        let _ = step.transition(StepStatus::Active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::{PhaseInput, StepInput, ThoughtSignatureState};
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
        assert_eq!(id, PlanStepId::new(3));
        assert_eq!(text, "Did the thing");
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
        assert!(matches!(op, EditOp::RemoveStep(id) if id == PlanStepId::new(5)));
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
            } if step_id == PlanStepId::new(2)
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
                if step_id == PlanStepId::new(3) && description == "Updated text"
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

        activate_next_eligible(&mut plan);
        assert!(matches!(
            plan.step(PlanStepId::new(1)).unwrap().status,
            StepStatus::Active
        ));
        assert!(matches!(
            plan.step(PlanStepId::new(2)).unwrap().status,
            StepStatus::Pending
        ));
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
        plan.step_mut(PlanStepId::new(1))
            .unwrap()
            .transition(StepStatus::Active)
            .unwrap();

        activate_next_eligible(&mut plan);
        // Step 2 should still be Pending (only one active at a time).
        assert!(matches!(
            plan.step(PlanStepId::new(2)).unwrap().status,
            StepStatus::Pending
        ));
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
                    depends_on: vec![PlanStepId::new(1)],
                }],
            },
        ])
        .unwrap();

        // Phase 1 not complete yet — Phase 2 steps should not activate.
        activate_next_eligible(&mut plan);
        // Should activate step 1 (Phase 1's eligible step).
        assert!(matches!(
            plan.step(PlanStepId::new(1)).unwrap().status,
            StepStatus::Active
        ));
        assert!(matches!(
            plan.step(PlanStepId::new(2)).unwrap().status,
            StepStatus::Pending
        ));
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
        plan.step_mut(PlanStepId::new(1))
            .unwrap()
            .transition(StepStatus::Active)
            .unwrap();
        plan.step_mut(PlanStepId::new(1))
            .unwrap()
            .transition(StepStatus::Complete("done".to_string()))
            .unwrap();

        activate_next_eligible(&mut plan);
        assert!(matches!(
            plan.step(PlanStepId::new(2)).unwrap().status,
            StepStatus::Active
        ));
    }
}

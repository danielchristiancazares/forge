//! Plan data model — DAG-backed, phased task plan.
//!
//! Pure domain types with no IO and no async. Invariants enforced at
//! construction time (IFA §2.1): invalid plans are unrepresentable.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Identifiers ──────────────────────────────────────────────

/// Unique identifier for a step within a plan.
///
/// Named `PlanStepId` to avoid collision with `context::StepId` (stream
/// recovery). Plan-scoped, monotonically assigned at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanStepId(u32);

impl PlanStepId {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PlanStepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Step Status ──────────────────────────────────────────────

/// Forward-only step lifecycle.
///
/// ```text
/// Pending ──► Active ──► Complete(outcome)
///                │
///                ├──────► Failed(reason)
///                │
///                └──────► Skipped(reason)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Active,
    Complete(String),
    Failed(String),
    Skipped(String),
}

impl StepStatus {
    /// Whether this status is terminal (no further transitions allowed).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StepStatus::Complete(_) | StepStatus::Failed(_) | StepStatus::Skipped(_)
        )
    }

    /// Whether this step counts as "satisfied" for dependency purposes.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        matches!(self, StepStatus::Complete(_) | StepStatus::Skipped(_))
    }
}

// ── Transition errors ────────────────────────────────────────

#[derive(Debug, Clone, Error)]
#[error("invalid step transition from {from} to {to}")]
pub struct PlanTransitionError {
    pub from: String,
    pub to: String,
}

// ── PlanStep ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: PlanStepId,
    pub description: String,
    pub status: StepStatus,
    pub depends_on: Vec<PlanStepId>,
}

impl PlanStep {
    /// Attempt a forward transition. Returns error on invalid transitions.
    ///
    /// Valid: `Pending → Active`, `Active → Complete|Failed|Skipped`.
    /// All other transitions are rejected.
    pub fn transition(&mut self, new_status: StepStatus) -> Result<(), PlanTransitionError> {
        let valid = matches!(
            (&self.status, &new_status),
            (StepStatus::Pending, StepStatus::Active)
                | (
                    StepStatus::Active,
                    StepStatus::Complete(_) | StepStatus::Failed(_) | StepStatus::Skipped(_)
                )
        );
        if valid {
            self.status = new_status;
            Ok(())
        } else {
            Err(PlanTransitionError {
                from: format!("{:?}", self.status),
                to: format!("{new_status:?}"),
            })
        }
    }
}

// ── Phase ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase {
    pub name: String,
    pub steps: Vec<PlanStep>,
}

impl Phase {
    /// Whether all steps in this phase are satisfied (Complete or Skipped).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.status.is_satisfied())
    }

    /// Whether all steps are terminal (Complete, Failed, or Skipped).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.steps.iter().all(|s| s.status.is_terminal())
    }

    /// Whether all steps are still Pending.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Pending))
    }
}

// ── Validation errors ────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum PlanValidationError {
    #[error("plan must contain at least one phase")]
    EmptyPlan,
    #[error("phase {index} has no steps")]
    EmptyPhase { index: usize },
    #[error("phase {index} has an empty name")]
    EmptyPhaseName { index: usize },
    #[error("duplicate step id {id}")]
    DuplicateStepId { id: PlanStepId },
    #[error("step {step_id} in phase {phase_index} depends on unknown step {dependency}")]
    UnknownDependency {
        step_id: PlanStepId,
        phase_index: usize,
        dependency: PlanStepId,
    },
    #[error(
        "step {step_id} in phase {phase_index} depends on step {dependency} which is not in an earlier phase"
    )]
    NonBackwardDependency {
        step_id: PlanStepId,
        phase_index: usize,
        dependency: PlanStepId,
    },
}

// ── Plan ─────────────────────────────────────────────────────

/// DAG-backed phased plan. Always non-empty (IFA §2.1).
///
/// Construction via [`Plan::new`] validates all invariants. Invalid plans
/// are unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    phases: Vec<Phase>,
}

impl Plan {
    ///
    /// Step IDs are auto-assigned (monotonically increasing from 1).
    /// This is the **only** public constructor (Authority Boundary).
    pub fn from_input(input: Vec<PhaseInput>) -> Result<Self, PlanValidationError> {
        if input.is_empty() {
            return Err(PlanValidationError::EmptyPlan);
        }

        // Assign step IDs monotonically.
        let mut next_id: u32 = 1;
        let phases: Vec<Phase> = input
            .into_iter()
            .map(|pi| {
                let steps = pi
                    .steps
                    .into_iter()
                    .map(|si| {
                        let id = PlanStepId::new(next_id);
                        next_id += 1;
                        PlanStep {
                            id,
                            description: si.description,
                            status: StepStatus::Pending,
                            depends_on: si.depends_on,
                        }
                    })
                    .collect();
                Phase {
                    name: pi.name,
                    steps,
                }
            })
            .collect();

        let plan = Self { phases };
        plan.validate()?;
        Ok(plan)
    }

    /// Reconstruct a plan from raw phases (deserialization / edit paths).
    ///
    /// Validates the DAG. Step IDs must already be assigned.
    pub fn new(phases: Vec<Phase>) -> Result<Self, PlanValidationError> {
        let plan = Self { phases };
        plan.validate()?;
        Ok(plan)
    }

    /// Validate all plan invariants.
    fn validate(&self) -> Result<(), PlanValidationError> {
        if self.phases.is_empty() {
            return Err(PlanValidationError::EmptyPlan);
        }

        // Collect all step IDs and their phase indices for dependency checking.
        let mut all_ids = HashSet::new();
        // Maps step_id → phase_index for backward-edge validation.
        let mut step_phase: Vec<(PlanStepId, usize)> = Vec::new();

        for (phase_idx, phase) in self.phases.iter().enumerate() {
            if phase.name.trim().is_empty() {
                return Err(PlanValidationError::EmptyPhaseName { index: phase_idx });
            }
            if phase.steps.is_empty() {
                return Err(PlanValidationError::EmptyPhase { index: phase_idx });
            }
            for step in &phase.steps {
                if !all_ids.insert(step.id) {
                    return Err(PlanValidationError::DuplicateStepId { id: step.id });
                }
                step_phase.push((step.id, phase_idx));
            }
        }

        // Build a fast lookup: step_id → phase_index.
        let phase_of: std::collections::HashMap<PlanStepId, usize> =
            step_phase.into_iter().collect();

        // Validate dependency edges.
        for (phase_idx, phase) in self.phases.iter().enumerate() {
            for step in &phase.steps {
                for dep in &step.depends_on {
                    let dep_phase =
                        phase_of
                            .get(dep)
                            .ok_or(PlanValidationError::UnknownDependency {
                                step_id: step.id,
                                phase_index: phase_idx,
                                dependency: *dep,
                            })?;
                    if *dep_phase >= phase_idx {
                        return Err(PlanValidationError::NonBackwardDependency {
                            step_id: step.id,
                            phase_index: phase_idx,
                            dependency: *dep,
                        });
                    }
                }
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn phases(&self) -> &[Phase] {
        &self.phases
    }

    pub fn phases_mut(&mut self) -> &mut Vec<Phase> {
        &mut self.phases
    }

    /// Find a step by ID, returning a reference.
    #[must_use]
    pub fn step(&self, id: PlanStepId) -> Option<&PlanStep> {
        self.phases
            .iter()
            .flat_map(|p| &p.steps)
            .find(|s| s.id == id)
    }

    /// Find a step by ID, returning a mutable reference.
    pub fn step_mut(&mut self, id: PlanStepId) -> Option<&mut PlanStep> {
        self.phases
            .iter_mut()
            .flat_map(|p| &mut p.steps)
            .find(|s| s.id == id)
    }

    /// Phase index containing the given step.
    #[must_use]
    pub fn phase_of(&self, id: PlanStepId) -> Option<usize> {
        self.phases
            .iter()
            .position(|p| p.steps.iter().any(|s| s.id == id))
    }

    /// The currently active step, if any.
    #[must_use]
    pub fn active_step(&self) -> Option<&PlanStep> {
        self.phases
            .iter()
            .flat_map(|p| &p.steps)
            .find(|s| matches!(s.status, StepStatus::Active))
    }

    /// The first eligible phase (all prior phases satisfied).
    #[must_use]
    pub fn eligible_phase_index(&self) -> Option<usize> {
        for (i, phase) in self.phases.iter().enumerate() {
            // Check if all prior phases are complete.
            let prior_complete = self.phases[..i].iter().all(Phase::is_complete);
            if prior_complete && !phase.is_terminal() {
                return Some(i);
            }
        }
        None
    }

    /// All steps in the eligible phase whose dependencies are satisfied
    /// and whose status is `Pending`.
    #[must_use]
    pub fn eligible_steps(&self) -> Vec<PlanStepId> {
        let Some(phase_idx) = self.eligible_phase_index() else {
            return Vec::new();
        };
        self.phases[phase_idx]
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Pending))
            .filter(|s| {
                s.depends_on
                    .iter()
                    .all(|dep| self.step(*dep).is_some_and(|d| d.status.is_satisfied()))
            })
            .map(|s| s.id)
            .collect()
    }

    /// Total step count.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.phases.iter().map(|p| p.steps.len()).sum()
    }

    /// Try to produce a `CompletedPlan` proof. Returns `Some` only when all
    /// steps are `Complete` or `Skipped`.
    #[must_use]
    pub fn try_complete(&self) -> Option<CompletedPlan> {
        if self.phases.iter().all(Phase::is_complete) {
            Some(CompletedPlan {
                phase_count: self.phases.len(),
                step_count: self.step_count(),
            })
        } else {
            None
        }
    }

    /// Render the plan as a UTF-8 status block.
    ///
    /// Format matches the spec:
    /// ```text
    /// [Active Plan — Phase 2: Implementation (2 of 3 phases)]
    ///
    /// Phase 1: Discovery ✓
    ///   ✓ 1. Audit existing config paths — outcome text
    ///
    /// Phase 2: Implementation →
    ///   → 3. Add helper function
    ///     4. Update error messages
    /// ```
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Find current phase info for header.
        let current_phase = self.eligible_phase_index();
        let completed_phases = self.phases.iter().filter(|p| p.is_complete()).count();

        // Header line.
        if let Some(idx) = current_phase {
            out.push_str(&format!(
                "[Active Plan — Phase {}: {} ({} of {} phases)]\n",
                idx + 1,
                self.phases[idx].name,
                completed_phases,
                self.phases.len()
            ));
        } else if self.phases.iter().all(Phase::is_complete) {
            out.push_str(&format!(
                "[Active Plan — Complete ({} phases)]\n",
                self.phases.len()
            ));
        } else {
            out.push_str(&format!(
                "[Active Plan — Blocked ({} of {} phases)]\n",
                completed_phases,
                self.phases.len()
            ));
        }

        for (i, phase) in self.phases.iter().enumerate() {
            out.push('\n');
            // Phase header with status indicator.
            let phase_indicator = if phase.is_complete() {
                " ✓"
            } else if Some(i) == current_phase {
                " →"
            } else {
                ""
            };
            out.push_str(&format!(
                "Phase {}: {}{}\n",
                i + 1,
                phase.name,
                phase_indicator
            ));

            for step in &phase.steps {
                let (icon, suffix) = match &step.status {
                    StepStatus::Pending => ("  ", String::new()),
                    StepStatus::Active => ("→ ", String::new()),
                    StepStatus::Complete(outcome) => ("✓ ", format!(" — {outcome}")),
                    StepStatus::Failed(reason) => ("✗ ", format!(" — FAILED: {reason}")),
                    StepStatus::Skipped(reason) => ("⊘ ", format!(" — skipped: {reason}")),
                };
                out.push_str(&format!(
                    "  {icon}{}. {}{}\n",
                    step.id, step.description, suffix
                ));
            }
        }

        out
    }
}

// ── Completed Plan (proof type) ──────────────────────────────

/// Proof that all steps in a plan reached a terminal satisfied state
/// (IFA §10.1). Produced only by [`Plan::try_complete`].
///
/// Private fields — cannot be forged outside this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPlan {
    phase_count: usize,
    step_count: usize,
}

impl CompletedPlan {
    #[must_use]
    pub fn phase_count(&self) -> usize {
        self.phase_count
    }

    #[must_use]
    pub fn step_count(&self) -> usize {
        self.step_count
    }
}

// ── Plan State ───────────────────────────────────────────────

/// Plan lifecycle state (IFA §9.1: State as Location).
///
/// Each variant is a named domain state with distinct valid operations.
/// No `Option<Plan>`, no empty sentinels, no tag fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum PlanState {
    /// Session is not plan-guided.
    #[default]
    Inactive,
    /// LLM created a plan, awaiting user approval.
    Proposed(Plan),
    /// User approved. Harness enforces constraints.
    Active(Plan),
}

impl PlanState {
    #[must_use]
    pub fn plan(&self) -> Option<&Plan> {
        match self {
            PlanState::Inactive => None,
            PlanState::Proposed(plan) | PlanState::Active(plan) => Some(plan),
        }
    }

    pub fn plan_mut(&mut self) -> Option<&mut Plan> {
        match self {
            PlanState::Inactive => None,
            PlanState::Proposed(plan) | PlanState::Active(plan) => Some(plan),
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, PlanState::Active(_))
    }
}

// ── Input types (for create/edit) ────────────────────────────

/// Input for creating a new phase (before IDs are assigned).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseInput {
    pub name: String,
    pub steps: Vec<StepInput>,
}

/// Input for creating a new step (before IDs are assigned).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInput {
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<PlanStepId>,
}

/// Edit operations for modifying an active plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditOp {
    AddStep {
        phase_index: usize,
        step: StepInput,
    },
    RemoveStep(PlanStepId),
    ReorderStep {
        step_id: PlanStepId,
        new_phase: usize,
    },
    UpdateDescription {
        step_id: PlanStepId,
        description: String,
    },
    AddPhase {
        index: usize,
        phase: PhaseInput,
    },
    RemovePhase(usize),
}

// ── Edit validation ──────────────────────────────────────────

#[derive(Debug, Clone, Error)]
pub enum EditValidationError {
    #[error("phase index {0} is out of range")]
    PhaseOutOfRange(usize),
    #[error("step {0} not found")]
    StepNotFound(PlanStepId),
    #[error("cannot remove step {0}: status is {1} (only Pending steps can be removed)")]
    StepNotPending(PlanStepId, String),
    #[error("cannot remove phase {0}: not all steps are Pending")]
    PhaseNotPending(usize),
    #[error("cannot modify completed phase {0}")]
    PhaseAlreadyComplete(usize),
    #[error("edit would produce an invalid plan: {0}")]
    InvalidResult(#[from] PlanValidationError),
}

impl Plan {
    /// Apply an edit operation, validating constraints.
    ///
    /// Returns the next available step ID (for `AddStep` / `AddPhase`).
    pub fn apply_edit(&mut self, op: EditOp) -> Result<(), EditValidationError> {
        match op {
            EditOp::AddStep { phase_index, step } => {
                if phase_index >= self.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(phase_index));
                }
                if self.phases[phase_index].is_complete() {
                    return Err(EditValidationError::PhaseAlreadyComplete(phase_index));
                }
                let next_id = self.next_step_id();
                self.phases[phase_index].steps.push(PlanStep {
                    id: next_id,
                    description: step.description,
                    status: StepStatus::Pending,
                    depends_on: step.depends_on,
                });
            }
            EditOp::RemoveStep(id) => {
                let step = self.step(id).ok_or(EditValidationError::StepNotFound(id))?;
                if !matches!(step.status, StepStatus::Pending) {
                    return Err(EditValidationError::StepNotPending(
                        id,
                        format!("{:?}", step.status),
                    ));
                }
                // Remove the step.
                for phase in &mut self.phases {
                    phase.steps.retain(|s| s.id != id);
                }
                // Remove from depends_on lists.
                for phase in &mut self.phases {
                    for s in &mut phase.steps {
                        s.depends_on.retain(|d| *d != id);
                    }
                }
            }
            EditOp::ReorderStep { step_id, new_phase } => {
                if new_phase >= self.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(new_phase));
                }
                let step = self
                    .step(step_id)
                    .ok_or(EditValidationError::StepNotFound(step_id))?;
                if !matches!(step.status, StepStatus::Pending) {
                    return Err(EditValidationError::StepNotPending(
                        step_id,
                        format!("{:?}", step.status),
                    ));
                }
                // Extract the step, place it in the new phase.
                let mut extracted = None;
                for phase in &mut self.phases {
                    if let Some(pos) = phase.steps.iter().position(|s| s.id == step_id) {
                        extracted = Some(phase.steps.remove(pos));
                        break;
                    }
                }
                if let Some(step) = extracted {
                    self.phases[new_phase].steps.push(step);
                }
            }
            EditOp::UpdateDescription {
                step_id,
                description,
            } => {
                let step = self
                    .step_mut(step_id)
                    .ok_or(EditValidationError::StepNotFound(step_id))?;
                step.description = description;
            }
            EditOp::AddPhase { index, phase } => {
                if index > self.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(index));
                }
                let mut next_id = self.next_step_id();
                let steps = phase
                    .steps
                    .into_iter()
                    .map(|si| {
                        let id = PlanStepId::new(next_id.value());
                        next_id = PlanStepId::new(next_id.value() + 1);
                        PlanStep {
                            id,
                            description: si.description,
                            status: StepStatus::Pending,
                            depends_on: si.depends_on,
                        }
                    })
                    .collect();
                self.phases.insert(
                    index,
                    Phase {
                        name: phase.name,
                        steps,
                    },
                );
            }
            EditOp::RemovePhase(index) => {
                if index >= self.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(index));
                }
                if !self.phases[index].is_pending() {
                    return Err(EditValidationError::PhaseNotPending(index));
                }
                // Collect step IDs being removed for dependency cleanup.
                let removed_ids: HashSet<PlanStepId> =
                    self.phases[index].steps.iter().map(|s| s.id).collect();
                self.phases.remove(index);
                // Clean up depends_on references.
                for phase in &mut self.phases {
                    for step in &mut phase.steps {
                        step.depends_on.retain(|d| !removed_ids.contains(d));
                    }
                }
            }
        }

        // Re-validate the DAG after mutation.
        self.validate()
            .map_err(EditValidationError::InvalidResult)?;

        // Ensure plan is still non-empty.
        if self.phases.is_empty() || self.phases.iter().all(|p| p.steps.is_empty()) {
            return Err(EditValidationError::InvalidResult(
                PlanValidationError::EmptyPlan,
            ));
        }

        Ok(())
    }

    /// Next available step ID (max existing + 1).
    fn next_step_id(&self) -> PlanStepId {
        let max = self
            .phases
            .iter()
            .flat_map(|p| &p.steps)
            .map(|s| s.id.value())
            .max()
            .unwrap_or(0);
        PlanStepId::new(max + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EditOp, EditValidationError, PhaseInput, Plan, PlanState, PlanStep, PlanStepId,
        PlanValidationError, StepInput, StepStatus,
    };

    fn simple_input() -> Vec<PhaseInput> {
        vec![
            PhaseInput {
                name: "Discovery".to_owned(),
                steps: vec![
                    StepInput {
                        description: "Audit config paths".to_owned(),
                        depends_on: vec![],
                    },
                    StepInput {
                        description: "Map dispatch flow".to_owned(),
                        depends_on: vec![],
                    },
                ],
            },
            PhaseInput {
                name: "Implementation".to_owned(),
                steps: vec![
                    StepInput {
                        description: "Replace hardcoded paths".to_owned(),
                        depends_on: vec![PlanStepId::new(1)],
                    },
                    StepInput {
                        description: "Add helper function".to_owned(),
                        depends_on: vec![],
                    },
                ],
            },
        ]
    }

    // ── Construction ─────────────────────────────────────────

    #[test]
    fn from_input_assigns_monotonic_ids() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let ids: Vec<u32> = plan
            .phases()
            .iter()
            .flat_map(|p| &p.steps)
            .map(|s| s.id.value())
            .collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn from_input_rejects_empty_plan() {
        let err = Plan::from_input(vec![]).unwrap_err();
        assert!(matches!(err, PlanValidationError::EmptyPlan));
    }

    #[test]
    fn from_input_rejects_empty_phase() {
        let input = vec![PhaseInput {
            name: "Empty".to_owned(),
            steps: vec![],
        }];
        let err = Plan::from_input(input).unwrap_err();
        assert!(matches!(err, PlanValidationError::EmptyPhase { index: 0 }));
    }

    #[test]
    fn from_input_rejects_empty_phase_name() {
        let input = vec![PhaseInput {
            name: "  ".to_owned(),
            steps: vec![StepInput {
                description: "Do thing".to_owned(),
                depends_on: vec![],
            }],
        }];
        let err = Plan::from_input(input).unwrap_err();
        assert!(matches!(
            err,
            PlanValidationError::EmptyPhaseName { index: 0 }
        ));
    }

    #[test]
    fn from_input_rejects_forward_dependency() {
        // Step 1 depends on step 2 (same phase) — forbidden.
        let input = vec![PhaseInput {
            name: "Phase 1".to_owned(),
            steps: vec![
                StepInput {
                    description: "First".to_owned(),
                    depends_on: vec![PlanStepId::new(2)],
                },
                StepInput {
                    description: "Second".to_owned(),
                    depends_on: vec![],
                },
            ],
        }];
        let err = Plan::from_input(input).unwrap_err();
        assert!(matches!(
            err,
            PlanValidationError::NonBackwardDependency { .. }
        ));
    }

    #[test]
    fn from_input_rejects_unknown_dependency() {
        let input = vec![PhaseInput {
            name: "Phase 1".to_owned(),
            steps: vec![StepInput {
                description: "First".to_owned(),
                depends_on: vec![PlanStepId::new(99)],
            }],
        }];
        let err = Plan::from_input(input).unwrap_err();
        assert!(matches!(err, PlanValidationError::UnknownDependency { .. }));
    }

    #[test]
    fn valid_cross_phase_dependency() {
        let plan = Plan::from_input(simple_input()).unwrap();
        // Step 3 depends on step 1 (earlier phase) — valid.
        assert_eq!(
            plan.phases()[1].steps[0].depends_on,
            vec![PlanStepId::new(1)]
        );
    }

    // ── Step transitions ─────────────────────────────────────

    #[test]
    fn valid_transitions() {
        let mut step = PlanStep {
            id: PlanStepId::new(1),
            description: "Test".to_owned(),
            status: StepStatus::Pending,
            depends_on: vec![],
        };
        step.transition(StepStatus::Active).unwrap();
        assert!(matches!(step.status, StepStatus::Active));

        step.transition(StepStatus::Complete("done".to_owned()))
            .unwrap();
        assert!(matches!(step.status, StepStatus::Complete(_)));
    }

    #[test]
    fn invalid_transition_pending_to_complete() {
        let mut step = PlanStep {
            id: PlanStepId::new(1),
            description: "Test".to_owned(),
            status: StepStatus::Pending,
            depends_on: vec![],
        };
        let err = step
            .transition(StepStatus::Complete("done".to_owned()))
            .unwrap_err();
        assert!(err.to_string().contains("invalid step transition"));
    }

    #[test]
    fn invalid_transition_from_terminal() {
        let mut step = PlanStep {
            id: PlanStepId::new(1),
            description: "Test".to_owned(),
            status: StepStatus::Complete("done".to_owned()),
            depends_on: vec![],
        };
        let err = step.transition(StepStatus::Active).unwrap_err();
        assert!(err.to_string().contains("invalid step transition"));
    }

    #[test]
    fn active_to_failed() {
        let mut step = PlanStep {
            id: PlanStepId::new(1),
            description: "Test".to_owned(),
            status: StepStatus::Active,
            depends_on: vec![],
        };
        step.transition(StepStatus::Failed("broken".to_owned()))
            .unwrap();
        assert!(matches!(step.status, StepStatus::Failed(_)));
    }

    #[test]
    fn active_to_skipped() {
        let mut step = PlanStep {
            id: PlanStepId::new(1),
            description: "Test".to_owned(),
            status: StepStatus::Active,
            depends_on: vec![],
        };
        step.transition(StepStatus::Skipped("not needed".to_owned()))
            .unwrap();
        assert!(matches!(step.status, StepStatus::Skipped(_)));
    }

    // ── Plan state lifecycle ─────────────────────────────────

    #[test]
    fn plan_state_default_is_inactive() {
        let state = PlanState::default();
        assert!(matches!(state, PlanState::Inactive));
        assert!(state.plan().is_none());
    }

    #[test]
    fn plan_state_proposed_has_plan() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let state = PlanState::Proposed(plan);
        assert!(state.plan().is_some());
        assert!(!state.is_active());
    }

    #[test]
    fn plan_state_active_is_active() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let state = PlanState::Active(plan);
        assert!(state.is_active());
    }

    // ── CompletedPlan proof ──────────────────────────────────

    #[test]
    fn try_complete_fails_when_not_all_terminal() {
        let plan = Plan::from_input(simple_input()).unwrap();
        assert!(plan.try_complete().is_none());
    }

    #[test]
    fn try_complete_succeeds_when_all_satisfied() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        // Transition all steps through Active → Complete.
        for phase in plan.phases_mut() {
            for step in &mut phase.steps {
                step.transition(StepStatus::Active).unwrap();
                step.transition(StepStatus::Complete("done".to_owned()))
                    .unwrap();
            }
        }
        let completed = plan.try_complete().unwrap();
        assert_eq!(completed.phase_count(), 2);
        assert_eq!(completed.step_count(), 4);
    }

    #[test]
    fn try_complete_with_skipped_steps() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        for phase in plan.phases_mut() {
            for step in &mut phase.steps {
                step.transition(StepStatus::Active).unwrap();
                step.transition(StepStatus::Skipped("not needed".to_owned()))
                    .unwrap();
            }
        }
        assert!(plan.try_complete().is_some());
    }

    #[test]
    fn try_complete_fails_with_failed_step() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        for phase in plan.phases_mut() {
            for step in &mut phase.steps {
                step.transition(StepStatus::Active).unwrap();
            }
        }
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Failed("broken".to_owned()))
            .unwrap();
        assert!(plan.try_complete().is_none());
    }

    // ── Eligible phase / steps ───────────────────────────────

    #[test]
    fn eligible_phase_starts_at_zero() {
        let plan = Plan::from_input(simple_input()).unwrap();
        assert_eq!(plan.eligible_phase_index(), Some(0));
    }

    #[test]
    fn eligible_steps_respects_dependencies() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        // Complete all phase 0 steps.
        for step in &mut plan.phases_mut()[0].steps {
            step.transition(StepStatus::Active).unwrap();
            step.transition(StepStatus::Complete("done".to_owned()))
                .unwrap();
        }
        assert_eq!(plan.eligible_phase_index(), Some(1));
        let eligible = plan.eligible_steps();
        // Step 3 depends on step 1 (satisfied), step 4 has no deps.
        // Both should be eligible.
        assert_eq!(eligible.len(), 2);
        assert!(eligible.contains(&PlanStepId::new(3)));
        assert!(eligible.contains(&PlanStepId::new(4)));
    }

    #[test]
    fn eligible_steps_blocked_by_unsatisfied_dep() {
        let plan = Plan::from_input(vec![
            PhaseInput {
                name: "Phase 1".to_owned(),
                steps: vec![StepInput {
                    description: "Step 1".to_owned(),
                    depends_on: vec![],
                }],
            },
            PhaseInput {
                name: "Phase 2".to_owned(),
                steps: vec![StepInput {
                    description: "Step 2".to_owned(),
                    depends_on: vec![PlanStepId::new(1)],
                }],
            },
        ])
        .unwrap();
        // Phase 0 not complete yet — phase 1 not eligible.
        assert_eq!(plan.eligible_phase_index(), Some(0));
        let eligible = plan.eligible_steps();
        assert_eq!(eligible, vec![PlanStepId::new(1)]);
    }

    // ── Render ───────────────────────────────────────────────

    #[test]
    fn render_fresh_plan() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("[Active Plan"));
        assert!(rendered.contains("Phase 1: Discovery"));
        assert!(rendered.contains("Phase 2: Implementation"));
        assert!(rendered.contains("1. Audit config paths"));
        assert!(rendered.contains("4. Add helper function"));
    }

    #[test]
    fn render_with_completed_steps() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Active)
            .unwrap();
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Complete("Found 3 refs".to_owned()))
            .unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("✓ 1. Audit config paths — Found 3 refs"));
    }

    #[test]
    fn render_with_active_step() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Active)
            .unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("→ 1. Audit config paths"));
    }

    // ── Serde round-trip ─────────────────────────────────────

    #[test]
    fn plan_serde_roundtrip() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        let deserialized: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, deserialized);
    }

    #[test]
    fn plan_state_serde_roundtrip() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let state = PlanState::Active(plan);
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: PlanState = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, PlanState::Active(_)));
    }

    #[test]
    fn plan_step_id_serde_transparent() {
        let id = PlanStepId::new(42);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "42");
        let deserialized: PlanStepId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    // ── Edit operations ──────────────────────────────────────

    #[test]
    fn add_step_to_phase() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.apply_edit(EditOp::AddStep {
            phase_index: 0,
            step: StepInput {
                description: "New step".to_owned(),
                depends_on: vec![],
            },
        })
        .unwrap();
        assert_eq!(plan.phases()[0].steps.len(), 3);
        assert_eq!(plan.phases()[0].steps[2].id, PlanStepId::new(5));
    }

    #[test]
    fn remove_pending_step() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.apply_edit(EditOp::RemoveStep(PlanStepId::new(4)))
            .unwrap();
        assert_eq!(plan.phases()[1].steps.len(), 1);
    }

    #[test]
    fn remove_non_pending_step_fails() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Active)
            .unwrap();
        let err = plan
            .apply_edit(EditOp::RemoveStep(PlanStepId::new(1)))
            .unwrap_err();
        assert!(matches!(err, EditValidationError::StepNotPending(_, _)));
    }

    #[test]
    fn add_phase() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.apply_edit(EditOp::AddPhase {
            index: 1,
            phase: PhaseInput {
                name: "New Phase".to_owned(),
                steps: vec![StepInput {
                    description: "Inserted step".to_owned(),
                    depends_on: vec![],
                }],
            },
        })
        .unwrap();
        assert_eq!(plan.phases().len(), 3);
        assert_eq!(plan.phases()[1].name, "New Phase");
    }

    #[test]
    fn remove_pending_phase() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        // Phase 1 (index 1) is all pending.
        plan.apply_edit(EditOp::RemovePhase(1)).unwrap();
        assert_eq!(plan.phases().len(), 1);
    }

    #[test]
    fn remove_non_pending_phase_fails() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.phases_mut()[0].steps[0]
            .transition(StepStatus::Active)
            .unwrap();
        let err = plan.apply_edit(EditOp::RemovePhase(0)).unwrap_err();
        assert!(matches!(err, EditValidationError::PhaseNotPending(_)));
    }

    #[test]
    fn update_description() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.apply_edit(EditOp::UpdateDescription {
            step_id: PlanStepId::new(1),
            description: "Updated description".to_owned(),
        })
        .unwrap();
        assert_eq!(
            plan.step(PlanStepId::new(1)).unwrap().description,
            "Updated description"
        );
    }

    #[test]
    fn edit_removes_dangling_deps() {
        // Remove step 1, which step 3 depends on. The dep should be cleaned up.
        let mut plan = Plan::from_input(simple_input()).unwrap();
        plan.apply_edit(EditOp::RemoveStep(PlanStepId::new(1)))
            .unwrap();
        assert!(plan.step(PlanStepId::new(3)).unwrap().depends_on.is_empty());
    }
}

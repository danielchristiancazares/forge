//! Plan data model — DAG-backed, phased task plan.
//!
//! Pure domain types with no IO and no async. Invariants enforced at
//! construction time (IFA §2.1): invalid plans are unrepresentable.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZeroU32;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::NonEmptyString;

// ── Identifiers ──────────────────────────────────────────────

/// Unique identifier for a step within a plan.
///
/// Named `PlanStepId` to avoid collision with `context::StepId` (stream
/// recovery). Plan-scoped, monotonically assigned at construction.
/// Zero is structurally unrepresentable via `NonZeroU32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanStepId(NonZeroU32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("plan step id must be a non-zero 32-bit integer")]
pub struct PlanStepIdError;

impl PlanStepId {
    pub fn try_new(value: u32) -> Result<Self, PlanStepIdError> {
        NonZeroU32::new(value).map(Self).ok_or(PlanStepIdError)
    }

    pub(crate) fn new_unchecked(value: u32) -> Self {
        Self(NonZeroU32::new(value).expect("PlanStepId::new_unchecked requires value > 0"))
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for PlanStepId {
    type Error = PlanStepIdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl TryFrom<u64> for PlanStepId {
    type Error = PlanStepIdError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let narrowed = u32::try_from(value).map_err(|_err| PlanStepIdError)?;
        Self::try_new(narrowed)
    }
}

impl fmt::Display for PlanStepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Step Typestates ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepData {
    id: PlanStepId,
    description: String,
    depends_on: Vec<PlanStepId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingStep(StepData);

impl PendingStep {
    #[must_use]
    fn activate(self) -> ActiveStep {
        ActiveStep(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveStep(StepData);

impl ActiveStep {
    #[must_use]
    fn complete(self, outcome: NonEmptyString) -> CompletedStep {
        CompletedStep {
            data: self.0,
            outcome,
        }
    }

    #[must_use]
    fn fail(self, reason: NonEmptyString) -> FailedStep {
        FailedStep {
            data: self.0,
            reason,
        }
    }

    #[must_use]
    fn skip(self, reason: NonEmptyString) -> SkippedStep {
        SkippedStep {
            data: self.0,
            reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletedStep {
    data: StepData,
    outcome: NonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedStep {
    data: StepData,
    reason: NonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedStep {
    data: StepData,
    reason: NonEmptyString,
}

// ── Transition errors ────────────────────────────────────────

#[derive(Debug, Clone, Error)]
#[error("invalid step transition from {from} to {to}")]
pub struct PlanTransitionError {
    pub from: String,
    pub to: String,
}

// ── PlanStep ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PlanStep {
    Pending(PendingStep),
    Active(ActiveStep),
    Complete(CompletedStep),
    Failed(FailedStep),
    Skipped(SkippedStep),
}

impl<'de> Deserialize<'de> for PlanStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StepDataWire {
            id: PlanStepId,
            description: String,
            #[serde(default)]
            depends_on: Vec<PlanStepId>,
        }

        #[derive(Deserialize)]
        struct CompleteWire {
            data: StepDataWire,
            outcome: NonEmptyString,
        }

        #[derive(Deserialize)]
        struct FailedWire {
            data: StepDataWire,
            reason: NonEmptyString,
        }

        #[derive(Deserialize)]
        struct SkippedWire {
            data: StepDataWire,
            reason: NonEmptyString,
        }

        #[derive(Deserialize)]
        enum StepWire {
            Pending(StepDataWire),
            Active(StepDataWire),
            Complete(CompleteWire),
            Failed(FailedWire),
            Skipped(SkippedWire),
        }

        match StepWire::deserialize(deserializer)? {
            StepWire::Pending(s) => Ok(Self::Pending(PendingStep(StepData {
                id: s.id,
                description: s.description,
                depends_on: s.depends_on,
            }))),
            StepWire::Active(s) => Ok(Self::Active(ActiveStep(StepData {
                id: s.id,
                description: s.description,
                depends_on: s.depends_on,
            }))),
            StepWire::Complete(s) => Ok(Self::Complete(CompletedStep {
                data: StepData {
                    id: s.data.id,
                    description: s.data.description,
                    depends_on: s.data.depends_on,
                },
                outcome: s.outcome,
            })),
            StepWire::Failed(s) => Ok(Self::Failed(FailedStep {
                data: StepData {
                    id: s.data.id,
                    description: s.data.description,
                    depends_on: s.data.depends_on,
                },
                reason: s.reason,
            })),
            StepWire::Skipped(s) => Ok(Self::Skipped(SkippedStep {
                data: StepData {
                    id: s.data.id,
                    description: s.data.description,
                    depends_on: s.data.depends_on,
                },
                reason: s.reason,
            })),
        }
    }
}

impl PlanStep {
    #[must_use]
    pub fn id(&self) -> PlanStepId {
        match self {
            Self::Pending(s) => s.0.id,
            Self::Active(s) => s.0.id,
            Self::Complete(s) => s.data.id,
            Self::Failed(s) => s.data.id,
            Self::Skipped(s) => s.data.id,
        }
    }

    #[must_use]
    pub fn description(&self) -> &str {
        match self {
            Self::Pending(s) => &s.0.description,
            Self::Active(s) => &s.0.description,
            Self::Complete(s) => &s.data.description,
            Self::Failed(s) => &s.data.description,
            Self::Skipped(s) => &s.data.description,
        }
    }

    fn description_mut(&mut self) -> &mut String {
        match self {
            Self::Pending(s) => &mut s.0.description,
            Self::Active(s) => &mut s.0.description,
            Self::Complete(s) => &mut s.data.description,
            Self::Failed(s) => &mut s.data.description,
            Self::Skipped(s) => &mut s.data.description,
        }
    }

    #[must_use]
    pub fn depends_on(&self) -> &[PlanStepId] {
        match self {
            Self::Pending(s) => &s.0.depends_on,
            Self::Active(s) => &s.0.depends_on,
            Self::Complete(s) => &s.data.depends_on,
            Self::Failed(s) => &s.data.depends_on,
            Self::Skipped(s) => &s.data.depends_on,
        }
    }

    fn depends_on_mut(&mut self) -> &mut Vec<PlanStepId> {
        match self {
            Self::Pending(s) => &mut s.0.depends_on,
            Self::Active(s) => &mut s.0.depends_on,
            Self::Complete(s) => &mut s.data.depends_on,
            Self::Failed(s) => &mut s.data.depends_on,
            Self::Skipped(s) => &mut s.data.depends_on,
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete(_) | Self::Failed(_) | Self::Skipped(_))
    }

    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        matches!(self, Self::Complete(_) | Self::Skipped(_))
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }

    fn state_name(&self) -> String {
        match self {
            Self::Pending(_) => "Pending".to_owned(),
            Self::Active(_) => "Active".to_owned(),
            Self::Complete(_) => "Complete".to_owned(),
            Self::Failed(_) => "Failed".to_owned(),
            Self::Skipped(_) => "Skipped".to_owned(),
        }
    }

    pub fn try_activate(self) -> Result<Self, (Self, PlanTransitionError)> {
        match self {
            Self::Pending(s) => Ok(Self::Active(s.activate())),
            other => {
                let name = other.state_name();
                Err((
                    other,
                    PlanTransitionError {
                        from: name,
                        to: "Active".to_owned(),
                    },
                ))
            }
        }
    }

    pub fn try_complete(
        self,
        outcome: NonEmptyString,
    ) -> Result<Self, (Self, PlanTransitionError)> {
        match self {
            Self::Active(s) => Ok(Self::Complete(s.complete(outcome))),
            other => {
                let name = other.state_name();
                Err((
                    other,
                    PlanTransitionError {
                        from: name,
                        to: "Complete".to_owned(),
                    },
                ))
            }
        }
    }

    pub fn try_fail(self, reason: NonEmptyString) -> Result<Self, (Self, PlanTransitionError)> {
        match self {
            Self::Active(s) => Ok(Self::Failed(s.fail(reason))),
            other => {
                let name = other.state_name();
                Err((
                    other,
                    PlanTransitionError {
                        from: name,
                        to: "Failed".to_owned(),
                    },
                ))
            }
        }
    }

    pub fn try_skip(self, reason: NonEmptyString) -> Result<Self, (Self, PlanTransitionError)> {
        match self {
            Self::Active(s) => Ok(Self::Skipped(s.skip(reason))),
            other => {
                let name = other.state_name();
                Err((
                    other,
                    PlanTransitionError {
                        from: name,
                        to: "Skipped".to_owned(),
                    },
                ))
            }
        }
    }
}

// ── Phase ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase {
    name: String,
    steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseCompletion {
    Complete,
    Incomplete,
}

impl Phase {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }

    /// Completion state for this phase based on step satisfaction.
    #[must_use]
    pub fn completion(&self) -> PhaseCompletion {
        if self.steps.iter().all(PlanStep::is_satisfied) {
            PhaseCompletion::Complete
        } else {
            PhaseCompletion::Incomplete
        }
    }

    /// Whether all steps are terminal (Complete, Failed, or Skipped).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.steps.iter().all(PlanStep::is_terminal)
    }

    /// Whether all steps are still Pending.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.steps.iter().all(PlanStep::is_pending)
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

// ── Domain Queries ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveStepContext<'a> {
    phase_index: usize,
    step: &'a PlanStep,
}

impl<'a> ActiveStepContext<'a> {
    #[must_use]
    pub const fn phase_index(self) -> usize {
        self.phase_index
    }

    #[must_use]
    pub const fn step(self) -> &'a PlanStep {
        self.step
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveStepQuery<'a> {
    Active(ActiveStepContext<'a>),
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseEligibility {
    Eligible(usize),
    BlockedByIncompletePriorPhase,
    AllPhasesComplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionStatus {
    Complete(CompletedPlan),
    Incomplete {
        pending_count: usize,
        active_count: usize,
        failed_count: usize,
    },
}

// ── Plan ─────────────────────────────────────────────────────

/// DAG-backed phased plan. Always non-empty (IFA §2.1).
///
/// Construction via [`Plan::new`] validates all invariants. Invalid plans
/// are unrepresentable. Deserialization validates on load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    phases: Vec<Phase>,
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PlanWire {
            phases: Vec<Phase>,
        }
        let wire = PlanWire::deserialize(deserializer)?;
        Plan::new(wire.phases).map_err(D::Error::custom)
    }
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
                        let id = PlanStepId::new_unchecked(next_id);
                        next_id += 1;
                        PlanStep::Pending(PendingStep(StepData {
                            id,
                            description: si.description,
                            depends_on: si.depends_on,
                        }))
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
                let id = step.id();
                if !all_ids.insert(id) {
                    return Err(PlanValidationError::DuplicateStepId { id });
                }
                step_phase.push((id, phase_idx));
            }
        }

        // Build a fast lookup: step_id → phase_index.
        let phase_of: HashMap<PlanStepId, usize> = step_phase.into_iter().collect();

        // Validate dependency edges.
        for (phase_idx, phase) in self.phases.iter().enumerate() {
            for step in &phase.steps {
                for dep in step.depends_on() {
                    let dep_phase =
                        phase_of
                            .get(dep)
                            .ok_or(PlanValidationError::UnknownDependency {
                                step_id: step.id(),
                                phase_index: phase_idx,
                                dependency: *dep,
                            })?;
                    if *dep_phase >= phase_idx {
                        return Err(PlanValidationError::NonBackwardDependency {
                            step_id: step.id(),
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

    /// The currently active step, if any.
    #[must_use]
    pub fn active_step(&self) -> ActiveStepQuery<'_> {
        for (phase_index, phase) in self.phases.iter().enumerate() {
            if let Some(step) = phase.steps.iter().find(|s| s.is_active()) {
                return ActiveStepQuery::Active(ActiveStepContext { phase_index, step });
            }
        }
        ActiveStepQuery::Idle
    }

    /// The first eligible phase.
    #[must_use]
    pub fn eligible_phase_index(&self) -> PhaseEligibility {
        for (i, phase) in self.phases.iter().enumerate() {
            // Check if all prior phases are complete.
            let prior_complete = self.phases[..i]
                .iter()
                .all(|p| matches!(p.completion(), PhaseCompletion::Complete));
            if !prior_complete {
                return PhaseEligibility::BlockedByIncompletePriorPhase;
            }
            if !phase.is_terminal() {
                return PhaseEligibility::Eligible(i);
            }
        }
        PhaseEligibility::AllPhasesComplete
    }

    /// All steps in the eligible phase whose dependencies are satisfied
    /// and whose status is `Pending`.
    #[must_use]
    pub fn eligible_steps(&self) -> Vec<PlanStepId> {
        let PhaseEligibility::Eligible(phase_idx) = self.eligible_phase_index() else {
            return Vec::new();
        };

        let satisfied_step_ids: HashSet<PlanStepId> = self
            .phases
            .iter()
            .flat_map(|phase| &phase.steps)
            .filter(|step| step.is_satisfied())
            .map(PlanStep::id)
            .collect();

        self.phases[phase_idx]
            .steps
            .iter()
            .filter(|s| s.is_pending())
            .filter(|s| {
                s.depends_on()
                    .iter()
                    .all(|dep| satisfied_step_ids.contains(dep))
            })
            .map(PlanStep::id)
            .collect()
    }

    /// Total step count.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.phases.iter().map(|p| p.steps.len()).sum()
    }

    /// Try to produce a `CompletedPlan` proof.
    #[must_use]
    pub fn try_complete(&self) -> CompletionStatus {
        if self
            .phases
            .iter()
            .all(|phase| matches!(phase.completion(), PhaseCompletion::Complete))
        {
            CompletionStatus::Complete(CompletedPlan {
                phase_count: self.phases.len(),
                step_count: self.step_count(),
            })
        } else {
            let mut pending_count = 0;
            let mut active_count = 0;
            let mut failed_count = 0;
            for p in &self.phases {
                for s in &p.steps {
                    match s {
                        PlanStep::Pending(_) => pending_count += 1,
                        PlanStep::Active(_) => active_count += 1,
                        PlanStep::Failed(_) => failed_count += 1,
                        PlanStep::Complete(_) | PlanStep::Skipped(_) => {}
                    }
                }
            }
            CompletionStatus::Incomplete {
                pending_count,
                active_count,
                failed_count,
            }
        }
    }

    /// Render the plan as a UTF-8 status block.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();

        let eligibility = self.eligible_phase_index();
        let completed_phases = self
            .phases
            .iter()
            .filter(|p| matches!(p.completion(), PhaseCompletion::Complete))
            .count();

        // Header line.
        match eligibility {
            PhaseEligibility::Eligible(idx) => {
                out.push_str(&format!(
                    "[Active Plan — Phase {}: {} ({} of {} phases)]\n",
                    idx + 1,
                    self.phases[idx].name,
                    completed_phases,
                    self.phases.len()
                ));
            }
            PhaseEligibility::AllPhasesComplete => {
                out.push_str(&format!(
                    "[Active Plan — Complete ({} phases)]\n",
                    self.phases.len()
                ));
            }
            PhaseEligibility::BlockedByIncompletePriorPhase => {
                out.push_str(&format!(
                    "[Active Plan — Blocked ({} of {} phases)]\n",
                    completed_phases,
                    self.phases.len()
                ));
            }
        }

        for (i, phase) in self.phases.iter().enumerate() {
            out.push('\n');
            let phase_indicator = match (
                phase.completion(),
                matches!(eligibility, PhaseEligibility::Eligible(idx) if idx == i),
            ) {
                (PhaseCompletion::Complete, _) => " ✓",
                (PhaseCompletion::Incomplete, true) => " →",
                (PhaseCompletion::Incomplete, false) => "",
            };
            out.push_str(&format!(
                "Phase {}: {}{}\n",
                i + 1,
                phase.name,
                phase_indicator
            ));

            for step in &phase.steps {
                let (icon, suffix) = match step {
                    PlanStep::Pending(_) => ("  ", String::new()),
                    PlanStep::Active(_) => ("→ ", String::new()),
                    PlanStep::Complete(s) => ("✓ ", format!(" — {}", s.outcome.as_str())),
                    PlanStep::Failed(s) => ("✗ ", format!(" — FAILED: {}", s.reason.as_str())),
                    PlanStep::Skipped(s) => ("⊘ ", format!(" — skipped: {}", s.reason.as_str())),
                };
                out.push_str(&format!(
                    "  {icon}{}. {}{}\n",
                    step.id(),
                    step.description(),
                    suffix
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

// ── Editor Boundary ──────────────────────────────────────────

pub mod editor {
    use super::{
        ActiveStepQuery, EditOp, EditValidationError, HashSet, NonEmptyString, PendingStep, Phase,
        PhaseCompletion, Plan, PlanStep, PlanStepId, PlanTransitionError, PlanValidationError,
        StepData,
    };
    use thiserror::Error;

    #[derive(Debug, Clone, Error, PartialEq, Eq)]
    #[error("step {requested} not found")]
    pub struct StepResolutionError {
        requested: PlanStepId,
    }

    impl StepResolutionError {
        #[must_use]
        pub const fn requested(&self) -> PlanStepId {
            self.requested
        }
    }

    #[derive(Debug, Clone, Error)]
    pub enum StepTransitionError {
        #[error("step {step_id} not found")]
        StepNotFound { step_id: PlanStepId },
        #[error("step {step_id} is not Active")]
        StepNotActive { step_id: PlanStepId },
        #[error(transparent)]
        InvalidTransition(PlanTransitionError),
    }

    #[must_use]
    fn not_found(step_id: PlanStepId) -> StepTransitionError {
        StepTransitionError::StepNotFound { step_id }
    }

    fn find_step_indices(
        plan: &Plan,
        step_id: PlanStepId,
    ) -> Result<(usize, usize), StepResolutionError> {
        for (phase_idx, phase) in plan.phases.iter().enumerate() {
            if let Some(step_idx) = phase.steps.iter().position(|step| step.id() == step_id) {
                return Ok((phase_idx, step_idx));
            }
        }
        Err(StepResolutionError { requested: step_id })
    }

    pub fn resolve_step(
        plan: &Plan,
        step_id: PlanStepId,
    ) -> Result<&PlanStep, StepResolutionError> {
        let (phase_idx, step_idx) = find_step_indices(plan, step_id)?;
        Ok(&plan.phases[phase_idx].steps[step_idx])
    }

    fn resolve_step_mut(
        plan: &mut Plan,
        step_id: PlanStepId,
    ) -> Result<&mut PlanStep, StepResolutionError> {
        let (phase_idx, step_idx) = find_step_indices(plan, step_id)?;
        Ok(&mut plan.phases[phase_idx].steps[step_idx])
    }

    pub fn phase_index_of_step(
        plan: &Plan,
        step_id: PlanStepId,
    ) -> Result<usize, StepResolutionError> {
        let (phase_idx, _step_idx) = find_step_indices(plan, step_id)?;
        Ok(phase_idx)
    }

    fn transition_step(
        plan: &mut Plan,
        step_id: PlanStepId,
        transition: impl FnOnce(PlanStep) -> Result<PlanStep, (PlanStep, PlanTransitionError)>,
    ) -> Result<(), StepTransitionError> {
        let (phase_idx, step_idx) =
            find_step_indices(plan, step_id).map_err(|_| not_found(step_id))?;
        let current = plan.phases[phase_idx].steps[step_idx].clone();
        let next = transition(current)
            .map_err(|(_original, err)| StepTransitionError::InvalidTransition(err))?;
        plan.phases[phase_idx].steps[step_idx] = next;
        Ok(())
    }

    pub fn activate_step(plan: &mut Plan, step_id: PlanStepId) -> Result<(), StepTransitionError> {
        transition_step(plan, step_id, super::PlanStep::try_activate)
    }

    pub fn complete_active_step(
        plan: &mut Plan,
        step_id: PlanStepId,
        outcome: NonEmptyString,
    ) -> Result<(), StepTransitionError> {
        let step = resolve_step(plan, step_id).map_err(|_| not_found(step_id))?;
        if !step.is_active() {
            return Err(StepTransitionError::StepNotActive { step_id });
        }
        transition_step(plan, step_id, |step| step.try_complete(outcome))
    }

    pub fn skip_active_step(
        plan: &mut Plan,
        step_id: PlanStepId,
        reason: NonEmptyString,
    ) -> Result<(), StepTransitionError> {
        let step = resolve_step(plan, step_id).map_err(|_| not_found(step_id))?;
        if !step.is_active() {
            return Err(StepTransitionError::StepNotActive { step_id });
        }
        transition_step(plan, step_id, |step| step.try_skip(reason))
    }

    pub fn fail_active_step(
        plan: &mut Plan,
        step_id: PlanStepId,
        reason: NonEmptyString,
    ) -> Result<(), StepTransitionError> {
        let step = resolve_step(plan, step_id).map_err(|_| not_found(step_id))?;
        if !step.is_active() {
            return Err(StepTransitionError::StepNotActive { step_id });
        }
        transition_step(plan, step_id, |step| step.try_fail(reason))
    }

    pub fn activate_next_eligible(plan: &mut Plan) {
        if matches!(plan.active_step(), ActiveStepQuery::Active(_)) {
            return;
        }
        if let Some(next_step_id) = plan.eligible_steps().first().copied() {
            let _ = activate_step(plan, next_step_id);
        }
    }

    /// Applies an edit operation as a pure transform.
    ///
    /// The input plan is consumed and a validated successor plan is returned.
    /// This avoids mutate-then-revert error handling in boundary callers.
    pub fn apply(plan: Plan, op: EditOp) -> Result<Plan, EditValidationError> {
        let mut plan = plan;
        match op {
            EditOp::AddStep { phase_index, step } => {
                if phase_index >= plan.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(phase_index));
                }
                if matches!(
                    plan.phases[phase_index].completion(),
                    PhaseCompletion::Complete
                ) {
                    return Err(EditValidationError::PhaseAlreadyComplete(phase_index));
                }
                let next_id = next_step_id(&plan);
                plan.phases[phase_index]
                    .steps
                    .push(PlanStep::Pending(PendingStep(StepData {
                        id: next_id,
                        description: step.description,
                        depends_on: step.depends_on,
                    })));
            }
            EditOp::RemoveStep(id) => {
                let (phase_idx, step_idx) = find_step_indices(&plan, id)
                    .map_err(|_| EditValidationError::StepNotFound(id))?;
                let step = &plan.phases[phase_idx].steps[step_idx];
                if !step.is_pending() {
                    return Err(EditValidationError::StepNotPending(id, step.state_name()));
                }

                // Remove the step.
                for phase in &mut plan.phases {
                    phase.steps.retain(|s| s.id() != id);
                }
                // Remove from depends_on lists.
                for phase in &mut plan.phases {
                    for s in &mut phase.steps {
                        s.depends_on_mut().retain(|d| *d != id);
                    }
                }
            }
            EditOp::ReorderStep { step_id, new_phase } => {
                if new_phase >= plan.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(new_phase));
                }
                let (phase_idx, step_idx) = find_step_indices(&plan, step_id)
                    .map_err(|_| EditValidationError::StepNotFound(step_id))?;
                let step = &plan.phases[phase_idx].steps[step_idx];
                if !step.is_pending() {
                    return Err(EditValidationError::StepNotPending(
                        step_id,
                        step.state_name(),
                    ));
                }

                // Extract the step, place it in the new phase.
                let mut extracted = None;
                for phase in &mut plan.phases {
                    if let Some(pos) = phase.steps.iter().position(|s| s.id() == step_id) {
                        extracted = Some(phase.steps.remove(pos));
                        break;
                    }
                }
                if let Some(step) = extracted {
                    plan.phases[new_phase].steps.push(step);
                }
            }
            EditOp::UpdateDescription {
                step_id,
                description,
            } => {
                let step = resolve_step_mut(&mut plan, step_id)
                    .map_err(|_| EditValidationError::StepNotFound(step_id))?;
                *step.description_mut() = description;
            }
            EditOp::AddPhase { index, phase } => {
                if index > plan.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(index));
                }
                let mut next_id = next_step_id(&plan);
                let steps = phase
                    .steps
                    .into_iter()
                    .map(|si| {
                        let id = PlanStepId::new_unchecked(next_id.value());
                        next_id = PlanStepId::new_unchecked(next_id.value() + 1);
                        PlanStep::Pending(PendingStep(StepData {
                            id,
                            description: si.description,
                            depends_on: si.depends_on,
                        }))
                    })
                    .collect();
                plan.phases.insert(
                    index,
                    Phase {
                        name: phase.name,
                        steps,
                    },
                );
            }
            EditOp::RemovePhase(index) => {
                if index >= plan.phases.len() {
                    return Err(EditValidationError::PhaseOutOfRange(index));
                }
                if !plan.phases[index].is_pending() {
                    return Err(EditValidationError::PhaseNotPending(index));
                }
                // Collect step IDs being removed for dependency cleanup.
                let removed_ids: HashSet<PlanStepId> =
                    plan.phases[index].steps.iter().map(PlanStep::id).collect();
                plan.phases.remove(index);
                // Clean up depends_on references.
                for phase in &mut plan.phases {
                    for step in &mut phase.steps {
                        step.depends_on_mut().retain(|d| !removed_ids.contains(d));
                    }
                }
            }
        }

        // Re-validate the DAG after mutation.
        plan.validate()
            .map_err(EditValidationError::InvalidResult)?;

        // Ensure plan is still non-empty.
        if plan.phases.is_empty() || plan.phases.iter().all(|p| p.steps.is_empty()) {
            return Err(EditValidationError::InvalidResult(
                PlanValidationError::EmptyPlan,
            ));
        }

        Ok(plan)
    }

    /// Next available step ID (max existing + 1).
    fn next_step_id(plan: &Plan) -> PlanStepId {
        let max = plan
            .phases
            .iter()
            .flat_map(|p| &p.steps)
            .map(|s| s.id().value())
            .max()
            .unwrap_or(0);
        PlanStepId::new_unchecked(max + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionStatus, EditOp, PhaseEligibility, PhaseInput, Plan, PlanStepId,
        PlanValidationError, StepInput, editor,
    };
    use crate::NonEmptyString;

    fn non_empty(value: &str) -> NonEmptyString {
        NonEmptyString::new(value).expect("test fixture must be non-empty")
    }

    fn step_id(value: u32) -> PlanStepId {
        PlanStepId::try_new(value).expect("test fixture must use non-zero plan step ids")
    }

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
                        depends_on: vec![step_id(1)],
                    },
                    StepInput {
                        description: "Add helper function".to_owned(),
                        depends_on: vec![],
                    },
                ],
            },
        ]
    }

    #[test]
    fn from_input_assigns_monotonic_ids() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let ids: Vec<u32> = plan
            .phases()
            .iter()
            .flat_map(|p| &p.steps)
            .map(|s| s.id().value())
            .collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn plan_step_id_rejects_zero() {
        assert!(PlanStepId::try_new(0).is_err());
    }

    #[test]
    fn plan_step_id_deserialize_rejects_zero() {
        let parsed: Result<PlanStepId, _> = serde_json::from_str("0");
        assert!(parsed.is_err());
    }

    #[test]
    fn from_input_rejects_empty_plan() {
        let err = Plan::from_input(vec![]).unwrap_err();
        assert!(matches!(err, PlanValidationError::EmptyPlan));
    }

    #[test]
    fn valid_transitions() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let mut step = plan.phases()[0].steps[0].clone();
        step = step.try_activate().unwrap();
        assert!(step.is_active());

        step = step.try_complete(non_empty("done")).unwrap();
        assert!(step.is_terminal());
    }

    #[test]
    fn invalid_transition_pending_to_complete() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let step = plan.phases()[0].steps[0].clone();
        let err = step.try_complete(non_empty("done")).unwrap_err();
        assert_eq!(err.1.from, "Pending");
    }

    #[test]
    fn try_complete_fails_when_not_all_terminal() {
        let plan = Plan::from_input(simple_input()).unwrap();
        assert!(matches!(
            plan.try_complete(),
            CompletionStatus::Incomplete { .. }
        ));
    }

    #[test]
    fn try_complete_succeeds_when_all_satisfied() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        for phase in &mut plan.phases {
            for step in &mut phase.steps {
                let s = step.clone();
                let active = s.try_activate().unwrap();
                *step = active.try_complete(non_empty("done")).unwrap();
            }
        }
        assert!(matches!(plan.try_complete(), CompletionStatus::Complete(_)));
    }

    #[test]
    fn eligible_phase_starts_at_zero() {
        let plan = Plan::from_input(simple_input()).unwrap();
        assert!(matches!(
            plan.eligible_phase_index(),
            PhaseEligibility::Eligible(0)
        ));
    }

    #[test]
    fn eligible_steps_respects_dependencies() {
        let mut plan = Plan::from_input(simple_input()).unwrap();
        for step in &mut plan.phases[0].steps {
            let s = step.clone();
            let active = s.try_activate().unwrap();
            *step = active.try_complete(non_empty("done")).unwrap();
        }
        assert!(matches!(
            plan.eligible_phase_index(),
            PhaseEligibility::Eligible(1)
        ));
        let eligible = plan.eligible_steps();
        assert_eq!(eligible.len(), 2);
    }

    #[test]
    fn render_fresh_plan() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let rendered = plan.render();
        assert!(rendered.contains("[Active Plan"));
        assert!(rendered.contains("Phase 1: Discovery"));
        assert!(rendered.contains("Phase 2: Implementation"));
    }

    #[test]
    fn add_step_to_phase() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let plan = editor::apply(
            plan,
            EditOp::AddStep {
                phase_index: 0,
                step: StepInput {
                    description: "New step".to_owned(),
                    depends_on: vec![],
                },
            },
        )
        .unwrap();
        assert_eq!(plan.phases()[0].steps.len(), 3);
    }

    #[test]
    fn current_format_roundtrips() {
        let plan = Plan::from_input(simple_input()).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        let roundtripped: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, roundtripped);
    }

    #[test]
    fn plan_deserialize_validates_on_load() {
        let invalid = r#"{"phases":[{"name":"Phase 1","steps":[{"Pending":{"id":1,"description":"x","depends_on":[99]}}]}]}"#;
        let err: Result<Plan, _> = serde_json::from_str(invalid);
        assert!(err.is_err());
    }
}

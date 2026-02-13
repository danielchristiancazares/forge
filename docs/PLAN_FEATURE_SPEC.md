# Plan Feature — PRD

**Status:** Draft v3
**Author:** Daniel Cazares
**Date:** 2026-02-13

---

## Problem Statement

When Forge's context window fills up, distillation compresses the conversation into an unstructured prose summary. The LLM loses its place in multi-step tasks — it forgets what's done, what's next, and what decisions were made along the way. Users have no way to rewind to a semantic milestone ("redo step 3") and no task-level visibility into what the LLM is doing beyond raw tool calls.

Most "plan" features in LLM tools are prompt wrappers: the LLM outputs a markdown list, then continues talking with no enforcement, no state, and no integration with context management. Even with a first-class data structure, if the plan is immutable and advisory, you've gold-plated the trenchcoat. Forge needs a Plan that is an **enforced scope-of-work contract** — a DAG-backed, user-approved, mutable structure that the harness holds the LLM to. Deviations require a plan edit with justification and user approval.

## Goals

1. **Enforced scope of work** — Once the user approves a plan, the LLM is locked into it. The harness enforces phase ordering and dependency satisfaction. To deviate, the LLM must propose a plan edit with justification, and the user must approve.

2. **Distillation-proof task tracking** — The Plan survives context compaction as a compact, re-rendered status block injected fresh into every API request. Size scales with plan structure, not conversation length.

3. **Semantic rewindability** — Each step completion creates a checkpoint. `/rewind step 3` restores files + history to the end of step 3, giving users milestone-based undo instead of turn-based undo.

4. **Task-level user control** — Users approve the plan before execution, approve plan edits mid-execution, see phase/step transitions in real-time, and manage *what* happens rather than *how* (individual tool calls).

5. **Plan-structured distillation** — When compaction runs with an active plan, the distillation prompt uses the plan as a skeleton, producing per-step outcomes instead of a chronological blob.

6. **Crash recovery** — Active plan state persists alongside conversation history. A crash mid-plan resumes at the correct step.

## Non-Goals

1. **Auto-detection of step completion** — The LLM explicitly calls `advance`. Heuristic completion detection is fragile and unpredictable.

2. **Nested plans** — One plan at a time. A step can be large, but nesting adds complexity without proportional value.

3. **Semantic tool-to-step mapping** — The harness does not parse tool arguments to determine if a tool call "relates to" the current step. Enforcement is structural (phase ordering, dependency gates, active step requirement) — individual tool calls within a step are unconstrained.

4. **Conditional branching / loops** — The DAG is acyclic. No conditional edges, no loops. If the plan needs restructuring, the LLM proposes an edit.

## User Stories

### Operator (Forge user)

- As an operator, I want to approve the plan before the LLM starts executing, so I can course-correct before wasted work.
- As an operator, I want to see which phase and step the LLM is on while it works, so I can gauge progress without reading every tool call.
- As an operator, I want the LLM to justify any deviation from the approved plan, so I maintain control over what's in scope.
- As an operator, I want to approve or reject plan edits mid-execution, so scope creep requires my explicit consent.
- As an operator, I want to rewind to a specific completed step, so I can retry from a known-good milestone instead of rewinding individual turns.
- As an operator, I want the LLM to stay on track after context compaction, so I don't have to re-explain the task when the conversation gets long.
- As an operator, I want to view the full plan with outcomes at any time (`/plan`), so I can review what was accomplished.
- As an operator, I want `/clear` to reset the conversation without losing my active plan, so I can free context space while keeping the scope of work intact.

### LLM (tool consumer)

- As the LLM, I want a structured plan state in every request, so I know what's done and what's next even after distillation erases the conversation history.
- As the LLM, I want to advance/fail/skip steps explicitly, so my progress is tracked without relying on context memory.
- As the LLM, I want to propose plan edits when I discover the plan is insufficient, so I can adapt without silently deviating.
- As the LLM, I want to query plan status after compaction, so I can reorient without guessing.

## State Machines

### Plan Lifecycle (`PlanState`)

```
                 create (LLM)
  Inactive ─────────────────────► Proposed(Plan)
     ▲                                │
     │                     ┌──────────┴──────────┐
     │                     │                      │
     │              user approves           user rejects
     │                     │                      │
     │                     ▼                      │
     │              Active(Plan) ◄────────────────┘
     │                     │            (plan cleared)
     │                     │
     │          ┌──────────┴──────────┐
     │          │                      │
     │   all steps terminal       /plan clear
     │          │                      │
     │          ▼                      │
     │     CompletedPlan (proof)       │
     │          │                      │
     └─────────┴──────────────────────┘
           (plan cleared)
```

Domain states (IFA §9.3):
- **Inactive**: Session is not plan-guided. No plan data exists.
- **Proposed**: LLM created a plan, awaiting user approval. Plan data exists but is not enforced.
- **Active**: User approved. Harness enforces phase ordering and dependency satisfaction.
- **CompletedPlan**: Proof type (IFA §10.1). All steps are terminal. Produced by `Plan::try_complete()`.

### Step Status Transitions

```
  Pending ──────► Active ──────► Complete(outcome)
                    │
                    ├──────────► Failed(reason)
                    │
                    └──────────► Skipped(reason)
```

Forward-only. Enforced at runtime via `PlanStep::transition() -> Result<(), PlanTransitionError>`. Invalid transitions return an error enumerating the attempted transition and current state.

### Step Activation Rules

Within an eligible phase, only one step may be `Active` at a time. The LLM must advance the active step before activating another. Parallel step execution is deferred to a future iteration.

A phase is eligible when all steps in all prior phases are `Complete` or `Skipped`. Within an eligible phase, a `Pending` step whose `depends_on` are all `Complete|Skipped` may become `Active`.

## Requirements

### P0 — Must Have

#### Data Model

**R1: DAG-backed phased plan types in `types/` crate (pure, no IO, no async)**

```rust
/// Unique identifier for a step within a plan.
/// Named PlanStepId to avoid collision with context::StepId (stream recovery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanStepId(u32);

pub enum StepStatus {
    Pending,
    Active,
    Complete(String),   // outcome summary
    Failed(String),     // failure reason
    Skipped(String),    // skip reason
}

pub struct PlanStep {
    id: PlanStepId,
    description: String,
    status: StepStatus,
    depends_on: Vec<PlanStepId>,  // DAG edges (steps in earlier phases)
}

pub struct Phase {
    name: String,
    steps: Vec<PlanStep>,
}

pub struct Plan {
    phases: Vec<Phase>,
}

/// Plan lifecycle state (IFA §9.1: State as Location).
///
/// Each variant is a named domain state with distinct valid operations.
/// No Option<Plan>, no empty sentinels, no tag fields.
pub enum PlanState {
    /// Session is not plan-guided.
    Inactive,
    /// LLM created a plan, awaiting user approval.
    Proposed(Plan),
    /// User approved. Harness enforces constraints.
    Active(Plan),
}

/// Proof that all steps in a plan are terminal (IFA §10.1).
/// Produced only by Plan::try_complete(). Cannot be forged.
pub struct CompletedPlan { /* private fields */ }
```

Acceptance criteria:
- `Plan` always contains at least one phase with at least one step. Construction via `Plan::new()` returns `Result<Plan, PlanValidationError>` — empty plans are unrepresentable (IFA §2.1).
- `PlanState` is a discriminated union (IFA §9.1). The `App` field is `plan_state: PlanState`. Pattern matching determines available operations — no `is_empty()` checks.
- `PlanStepId` is plan-scoped, monotonically assigned at creation. No two steps share an ID within a plan.
- `depends_on` edges point backward only (to steps in earlier phases). Edges within the same phase are forbidden — phase boundaries are the sequencing mechanism.
- Forward transitions only: `Pending → Active → Complete|Failed|Skipped`. Enforced at runtime via `PlanStep::transition()`.
- `Plan` is `Serialize + Deserialize` for persistence.
- `Plan::try_complete() -> Option<CompletedPlan>` returns `Some` only when all steps are `Complete|Skipped`. `CompletedPlan` is a proof type with private fields — cannot be forged outside the Authority Boundary.
- `Plan::render()` produces a UTF-8 status block grouped by phase. Size scales linearly with step count.

**R2: PlanState field on `App`**

The `App` struct (engine `lib.rs`) gets a `plan_state: PlanState` field.

Acceptance criteria:
- Field is `plan_state: PlanState`, initialized to `PlanState::Inactive`.
- Operations that require a plan pattern-match on the variant. No `if plan.is_empty()` idiom.
- `PlanState::Inactive` is the only variant with no associated data.

#### Enforcement

**R3: Harness enforcement of plan contract**

When `PlanState::Active(plan)` is the current state, the engine enforces structural constraints on plan transitions.

Acceptance criteria:
- The LLM cannot call `advance` on a step in phase N+1 while phase N has incomplete steps.
- The LLM cannot activate a step whose `depends_on` steps are not `Complete|Skipped`.
- Only one step may be `Active` at a time within a phase. The LLM must advance the current step before activating another.
- If the LLM completes all steps in the current phase and the next phase isn't ready (blocked by a `Failed` step), execution pauses for user decision.
- Precondition violations return `ToolResult { is_error: true }` with a message explaining which constraint was violated and what the LLM should do instead (e.g., "Step 5 depends on step 3 which is Failed. Propose a plan edit or skip step 3.").
- `create` returns an error if `PlanState::Active(_)` — the LLM must complete or the user must `/plan clear` the current plan first. `create` during `Proposed` replaces the proposal.

#### Tool Interface

**R4: `Plan` tool with subcommands**

The Plan tool is **not** a `ToolExecutor`. The engine intercepts `Plan` tool calls in `tool_loop.rs` before they reach the executor — a pre-resolved tool. The engine parses the subcommand, runs validation, and resolves the `ToolResult` directly.

This keeps the `ToolExecutor` trait clean and puts all plan logic in the engine where enforcement lives.

| Subcommand | Input | Effect |
|------------|-------|--------|
| `create` | `phases: Vec<PhaseInput>` | Stores plan, transitions to `Proposed(Plan)`, renders in UI, pauses for user approval |
| `advance` | `step_id: PlanStepId, outcome: String` | Marks step `Complete(outcome)`, creates checkpoint, activates next eligible step if any |
| `skip` | `step_id: PlanStepId, reason: String` | Marks step `Skipped(reason)`, activates next eligible step if any |
| `fail` | `step_id: PlanStepId, reason: String` | Marks step `Failed(reason)`, pauses for user decision |
| `edit` | `EditOp, justification: String` | Proposes a plan edit, pauses for user approval |
| `status` | (none) | Returns rendered plan state |

Where `PhaseInput` and `EditOp` are:

```rust
pub struct PhaseInput {
    name: String,
    steps: Vec<StepInput>,
}

pub struct StepInput {
    description: String,
    depends_on: Vec<PlanStepId>,  // empty for steps with no cross-phase deps
}

pub enum EditOp {
    AddStep { phase_index: usize, step: StepInput },
    RemoveStep(PlanStepId),              // only Pending steps
    ReorderStep { step_id: PlanStepId, new_phase: usize },
    UpdateDescription { step_id: PlanStepId, description: String },
    AddPhase { index: usize, phase: PhaseInput },
    RemovePhase(usize),              // only if all steps Pending
}
```

Acceptance criteria:
- `create` validates the DAG (no forward edges, no cycles, all `depends_on` reference valid `PlanStepId`s in earlier phases). Rejects invalid structures with a specific error.
- `create` transitions to `PlanState::Proposed(plan)` and pauses for user approval. On rejection, transitions back to `Inactive` and the LLM receives a `ToolResult` indicating rejection.
- On approval, transitions to `PlanState::Active(plan)` and the first phase's eligible steps become `Active`.
- `advance`/`skip`/`fail` require `PlanState::Active` and the target step to be `Active`. Calling on a `Pending` or terminal step returns an error.
- `edit` requires `PlanState::Active`, a non-empty `justification`, and pauses for user approval. Rejected edits return an error result.
- `edit` validates that the resulting plan is still a valid DAG (no cycles, no dangling `depends_on`).
- Phase-modifying edits (`AddStep`, `AddPhase`, `RemoveStep`, `RemovePhase`, `ReorderStep`) cannot target completed phases. `RemoveStep`/`RemovePhase` only operate on `Pending` steps/phases.
- `status` works in any `PlanState` — returns "No active plan." for `Inactive`, rendered plan for `Proposed` or `Active`.
- All subcommands return `ToolResult` with appropriate success/error state.
- Tool risk level: `Low` (no filesystem side effects).
- The tool schema is registered in `ToolRegistry` for LLM visibility but execution is intercepted by the engine.

**R5: Checkpoint on step advance**

`advance` and `skip` trigger `CheckpointStore::create` with a new `CheckpointKind::PlanStep` variant.

Acceptance criteria:
- A new `CheckpointKind::PlanStep(PlanStepId)` variant stores which step just completed.
- The checkpoint includes a snapshot of the `PlanState` at that point (for plan-aware rewind).
- `/rewind step N` parses the step ID and finds the corresponding `PlanStep` checkpoint.
- Existing `/undo` behavior is unaffected (still rewinds to last `Turn` checkpoint).

#### Context Injection

**R6: Plan state injected into API payload after distillation**

Before distillation, the plan state is already in context — `create`, `advance`, `skip`, `fail`, and `edit` tool calls and their results are part of the message history. No injection needed.

After distillation compacts the history, the plan's tool call trail is gone. At that point, the rendered plan block is prepended to the most recent user message in the API request, restoring the LLM's awareness of where it is.

Acceptance criteria:
- Injection is conditional: only when `FullHistory::is_compacted()` returns true and `PlanState::Active(_)`.
- The plan block is prepended to the most recent user message in the **API payload only** — not persisted to history. The injection is transient and re-rendered on every request post-compaction. This is architecturally different from AGENTS.md (which is consumed once and becomes part of history).
- It is never part of the system prompt (system prompt is static and cacheable — no dynamic content).
- Injection happens in `start_streaming` (engine `streaming.rs`), after `ContextManager::prepare()` builds the message list but before sending to the provider.
- Completed steps show a one-line outcome. Active and pending steps show full descriptions.
- `PlanState::Inactive` injects nothing.
- The plan block does not consume a cache slot (it changes every request post-compaction).
- Render format groups by phase:

```
[Active Plan — Phase 2: Implementation (3 of 4 phases)]

Phase 1: Discovery ✓
  ✓ 1. Audit existing config paths — Found 3 hardcoded ~/.forge refs
  ✓ 2. Map provider dispatch flow — Documented in scratch notes

Phase 2: Implementation →
  ✓ 3. Replace hardcoded paths with dirs::home_dir() — 3 files updated
  → 4. Add config_path() display helper
    5. Update error messages to show resolved path

Phase 3: Validation
    6. Add integration tests for path resolution
    7. Run verify

Phase 4: Ship
    8. Update docs/
    9. Commit and push
```

#### Persistence

**R7: Plan state persists with conversation history**

The plan is persisted in a dedicated `plan.json` file in the data directory, saved and loaded alongside `history.json`. Users may quit and return — the plan must survive.

Acceptance criteria:
- `plan.json` is saved atomically (same `atomic_write_with_options` pattern as history).
- `save_plan()` is called from `autosave_history()` — the two are saved together. Load happens in `load_history_if_exists()`.
- Crash during plan execution restores the `PlanState` at the correct step on recovery.
- `PlanState::Proposed` is persisted. A crash before approval resumes with the plan awaiting approval.
- `PlanState::Inactive` produces no file (or an empty/absent `plan.json`). On load, a missing `plan.json` means `PlanState::Inactive`.
- `/clear` resets the conversation but does NOT touch `plan.json` or `PlanState`. The plan survives context clearing.

#### Slash Commands

**R8: `/plan` slash command**

Displays the full plan in the UI, with a `clear` subcommand to explicitly discard a plan.

Acceptance criteria:
- Added to `CommandKind` enum, `COMMAND_SPECS`, and `COMMAND_ALIASES`.
- `/plan` with no argument: renders full plan with phase grouping, step statuses, and outcomes. Shows "No active plan." for `PlanState::Inactive`.
- `/plan clear`: transitions `PlanState` to `Inactive`, deletes `plan.json`, notifies user. Works in both `Proposed` and `Active` states.

**R9: User approval flow for create and edit**

Plan creation and edits pause execution and present the plan to the user for approval.

Acceptance criteria:
- `create` renders the proposed plan and prompts the user. The LLM cannot proceed until the user responds.
- `edit` renders the current plan with the proposed changes highlighted, the LLM's justification, and prompts the user.
- On rejection, the tool returns an error result to the LLM explaining the rejection.
- Approval integrates with `OperationState` via a new `PlanApproval` variant that holds the pending plan/edit and the originating `ToolResult` slot. This is distinct from `ToolLoopPhase::AwaitingApproval` (which is for tool execution approval, not data structure approval).

### P1 — Nice to Have

**R10: Status bar integration**

The TUI status bar shows `Plan: Phase N — Step description` when a plan is active.

Acceptance criteria:
- Plan status is cached alongside `ContextUsageStatus` and invalidated on plan state changes.
- Renders only when `PlanState::Active`. No visual cost when `Inactive`.

**R11: Step transition notifications**

Brief inline notification on step and phase transitions.

Acceptance criteria:
- Step completion: `✓ Step 3 complete → Step 4: Add config_path() helper`
- Phase completion: `✓ Phase 2: Implementation complete → Phase 3: Validation`
- Notifications appear as system messages in the display, not modals.
- Do not interrupt streaming.

**R12: `/rewind step N` semantic rewind**

Extend `/rewind` to accept `step N` syntax.

Acceptance criteria:
- Finds the `PlanStep(PlanStepId)` checkpoint for step N and restores files + history.
- Also restores the `PlanState` to the state it was in when that checkpoint was created (so steps completed after N revert to Pending).

### P2 — Future Considerations

**R13: Plan-aware distillation**

When compaction runs with an active plan, the distillation prompt includes the plan structure. Instead of "summarize this conversation," the prompt becomes "summarize the outcomes of completed steps and the current state of the active step."

The distillation output is organized around the plan skeleton rather than chronological conversation flow. The existing `distillation.md` template (GOAL, STATE, DECISIONS, BLOCKERS, NEXT) gains a PLAN section:

```
PLAN (Phase 2/4: Implementation)
Phase 1 ✓: Discovery — Audited 3 hardcoded paths, mapped provider dispatch
Step 3 ✓: Replace paths — Updated config/lib.rs, engine/lib.rs, cli/main.rs
Step 4 →: Add config_path() — In progress, adding to config crate public API
Steps 5-9: Pending (error messages, tests, verify, docs, commit)
```

This is the highest-payoff integration but can ship after the enforced core is working.

**R14: Plan template library**

Common plan templates (e.g., "implement feature", "fix bug", "refactor module") that the LLM can instantiate and customize rather than building from scratch each time.

## Technical Design

### Where Types Live

| Type | Crate | Rationale |
|------|-------|-----------|
| `Plan`, `Phase`, `PlanStep`, `PlanStepId`, `StepStatus`, `EditOp`, `PlanState`, `CompletedPlan` | `types` | Pure domain types, no IO |
| Plan tool schema registration | `tools` | Schema only — no `ToolExecutor` impl |
| `plan_state: PlanState` field, enforcement logic, tool interception | `engine` (`App`, `tool_loop.rs`) | Orchestration state + harness enforcement |
| Plan rendering | `tui` | Display logic |
| Plan persistence | `engine` (`persistence.rs`) | `plan.json` save/load alongside `history.json` |

### DAG Validation

On `create` and `edit`, validate:
1. All `depends_on` references resolve to existing `PlanStepId`s.
2. All `depends_on` edges point to steps in strictly earlier phases (no same-phase or forward edges).
3. No cycles (guaranteed by the earlier-phase constraint, but validate defensively).
4. At least one phase exists.
5. Each phase has at least one step.
6. Phase names are non-empty.

Validation is a pure function: `Plan::validate() -> Result<(), PlanValidationError>`. Errors enumerate exactly which constraint was violated. Validation runs at construction — `Plan::new()` calls `validate()` internally. Invalid plans are unrepresentable (IFA §2.1).

### Context Injection Point

Before distillation, the plan state lives in the message history as tool calls and results — no injection needed. After distillation compacts the history, the plan block is prepended to the most recent user message during message assembly in `start_streaming` (engine `streaming.rs`), gated on `FullHistory::is_compacted()`.

This is architecturally distinct from AGENTS.md injection (`input_modes.rs` via `take_agents_md()`). AGENTS.md is consumed once, becomes part of persisted history, and is never re-injected. Plan injection is transient — it modifies the API payload only, is re-rendered each request, and is never persisted as a history entry.

### Cache Budget Impact

The system prompt remains static and fully cacheable — no dynamic content is ever appended to it. Post-compaction, the plan block is part of the most recent user message, which is typically not cached anyway. The `plan_cache_allocation()` function in `streaming.rs` needs no modification.

### Enforcement Mechanics

The harness enforcement lives in `engine`, not in the tool layer. The Plan tool is intercepted in `tool_loop.rs` before reaching any `ToolExecutor`. The engine parses the subcommand and dispatches to `App` methods:

```
LLM calls Plan tool (advance step 5)
  → tool_loop.rs intercepts Plan tool call
    → App::plan_advance(step_id, outcome)
      → App validates: Is PlanState::Active? Is step 5 Active? Dependencies met?
        → Yes: transition step, create checkpoint, return ToolResult { is_error: false }
        → No: return ToolResult { is_error: true, content: "..." }
```

For `create`/`edit` (which require user approval):
```
LLM calls Plan tool (create phases)
  → tool_loop.rs intercepts Plan tool call
    → App::plan_create(phases)
      → Validates DAG, transitions to PlanState::Proposed(plan)
      → Returns PlanApprovalPending signal to tool loop
        → Engine transitions OperationState to PlanApproval variant
        → User approves → Active(plan), resume with success ToolResult
        → User rejects → Inactive, resume with error ToolResult
```

### Approval Flow Integration

Plan approval uses a new `OperationState::PlanApproval` variant, distinct from `ToolLoopPhase::AwaitingApproval`. The existing approval flow is for tool execution permission (approve/deny tool calls in a batch). Plan approval is for data structure approval (approve/reject a proposed plan or edit).

The `PlanApproval` variant holds:
- The pending plan or edit operation
- The originating tool call ID (to construct the `ToolResult` on resolution)
- The `ToolLoopState` that was interrupted (to resume after approval)

### Checkpoint Integration

Extend `CheckpointKind`:

```rust
pub(crate) enum CheckpointKind {
    Turn,
    ToolEdit,
    PlanStep(PlanStepId),  // step that just completed
}
```

`CheckpointStore` already handles creation and lookup. The engine calls `create_plan_step_checkpoint()` after a successful `advance` or `skip`. The checkpoint includes a serialized snapshot of `PlanState` for plan-aware rewind (P1).

### `/clear` Behavior

`/clear` resets conversation history and context but does NOT touch `PlanState`. The plan is a scope-of-work contract that exists alongside the conversation — users may want to free context space without discarding the plan.

To explicitly discard a plan, use `/plan clear`.

### Failure Mode: LLM Forgets to Advance

If the LLM completes a step's work but doesn't call `advance`, the plan stays on the current step. This is a stale pointer, not corruption. The user sees the step stuck in the UI/status bar and can nudge. The LLM can call `status` after compaction to reorient.

### Failure Mode: LLM Tries to Deviate

If the LLM tries to advance a step out of order, activate a step in a future phase, or do anything that violates the DAG, the harness returns an error. The LLM's options are:
1. Call `edit` to propose restructuring the plan (requires user approval).
2. Call `fail` on the blocking step and wait for user decision.
3. Call `skip` on a blocking step (if it's active).

The LLM cannot silently route around the plan. This is the core difference from an advisory system.

## Implementation Order

| Phase | Scope | Deliverable |
|-------|-------|-------------|
| 1 | Data model | `Plan`, `Phase`, `PlanStep`, `PlanStepId`, `StepStatus`, `PlanState`, `CompletedPlan`, `EditOp` in `types/src/` with DAG validation, `render()`, serialization, and unit tests |
| 2 | Tool interception + enforcement | Plan tool schema in `tools/src/`. Engine intercepts Plan calls in `tool_loop.rs`. Enforcement logic in `App` methods. |
| 3 | Approval flow | `OperationState::PlanApproval` variant. Plan create/edit pause for user approval. |
| 4 | App integration | `plan_state: PlanState` field on `App`, checkpoint on advance, `plan.json` persistence |
| 5 | Context injection | Plan rendered and injected into API payload in `start_streaming` |
| 6 | UI | `/plan` command (with `clear` subcommand), status bar indicator, step/phase transition notifications |
| 7 | Distillation | Plan-aware compaction prompt and structured output |

Phases 1-4 produce an enforced plan tool. Phases 5-7 make it a genuine engine-level primitive that interacts with context management and the UI.

## Success Metrics

**Leading indicators** (measurable within 1 week of shipping):
- Plan tool adoption: >30% of multi-step tasks (>3 tool calls) use a plan within 2 weeks
- Step advance rate: >80% of plan steps are explicitly advanced (vs. stale/forgotten)
- Plan edit rate: <20% of plans require mid-execution edits (indicates plans are reasonably accurate upfront)

**Lagging indicators** (measurable within 1 month):
- Post-compaction task completion: Tasks with an active plan complete successfully >90% of the time after distillation, vs. baseline without plans
- Enforcement effectiveness: <5% of plan sessions have the LLM attempting invalid transitions (indicates the LLM learns the contract quickly)
- Rewind usage: `/rewind step N` used in >10% of plan sessions
- User re-prompting: Reduction in "you forgot what you were doing" corrections after compaction

## Resolved Questions

| # | Question | Resolution |
|---|----------|------------|
| 1 | How does the plan interact with `/retry`? | No special interaction. The LLM handles retries via `edit`/`skip`/`fail`. |
| 2 | Should `fail` auto-pause or mark-and-continue? | **Mark and continue.** `fail` marks the step as `Failed`, which blocks phase advancement. The LLM's only path forward is `edit` (propose a restructured plan), which requires user approval. The edit approval flow is the pause mechanism — no special auto-pause needed. |
| 3 | Single vs. parallel active steps? | **Single.** One active step at a time for P0. Simplifies enforcement, checkpointing, and status display. Parallel deferred to future iteration. |
| 4 | Edit visualization? | **Diff view.** Strikethrough removed steps, highlight added steps. |
| 5 | Max phase/step count? | **No cap.** The user approval flow is the throttle. If the LLM proposes a bloated plan, the user rejects it. No arbitrary limit in the type system. |
| 6 | `create` during `Proposed`? | **Replace.** The old proposal was inadequate; the LLM is iterating. Swap silently. |

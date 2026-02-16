# IFA Refactor: Engine Operation Lifecycle, Tooling, and Focus

- Date: 2026-02-16
- Scope: `engine` operation orchestration (`engine/src/*.rs`)
- Standard: `INVARIANT_FIRST_ARCHITECTURE.md`
- Compliance mode: strict absolute. No compatibility shims, no deprecation paths, no backward-compatibility assumptions.

## 0) Verified Baseline (No Blind Spots)

- `engine/src/state.rs:596-605` defines `OperationState` with 8 variants.
- `engine/src/ui/view_state.rs:43-67` defines `FocusState` and `ViewMode`.
- `engine/src/lib.rs:801-806` stores mutable `tools_disabled_state` as `Option<ToolsDisabledState>`.
- `engine/src/lib.rs:1617-1644` define `tool_journal_disabled_reason()` and `busy_reason()`.
- `engine/src/lib.rs:1679-1690` define `idle_state()` and `replace_with_idle()`.
- `engine/src/init.rs:317` initializes `OperationState::Idle`.
- Mutation audit in `engine/src` (excluding tests):
  - `35` direct assignments `self.state = ...`
  - `7` `std::mem::replace(&mut self.state, ...)` sites
  - `5` call sites of `replace_with_idle()`
- Tooling latch writes in `engine/src`: `8`
- Direct focus writes outside boundary: `3` total
  - state writes in `streaming.rs` and one in-place review cursor path in `lib.rs`
- `transition_to_journaled()` is already fail-fast in `engine/src/state.rs:186-188`.

## 1) Conformance Contract

- Lifecycle correctness is represented structurally, not by comments or checks.
- All phase transitions occur through one authority boundary.
- Tooling enablement has exactly one owner and one representation.
- Focus mode is derived from operation transitions and cursor state.
- Core lifecycle payloads contain no optional absence sentinels.
- Illegal transition paths are hard failures, not silent fallbacks.

## 2) IFA-17 Operational Definitions (Mandatory)

### 2.1 Invariant Registry

1. `I-STATE`: Operation phase is represented by a canonical phase machine.
2. `I-TOOLING`: Tooling enablement has one proof and one owner.
3. `I-TRANSITION`: Legal phase transitions are explicit and totalized in one match.
4. `I-FOCUS`: Focus mode is derived from authoritative transition effects.
5. `I-FAILURE`: Recoverable failures are boundary-owned and transition-aware.

### 2.2 Authority Boundary Map

- `EngineOperationBoundary`: sole authority for operation phase transitions and focus effects.
- `EnginePersistenceBoundary`: sole authority for persistence recovery conversion.
- `EnginePersistenceRecoveryBoundary`: sole authority for recovery result conversion.
- `EngineCommandBoundary`: sole authority for command intent translation.
- Core modules (`tool_loop`, `streaming`, `plan`, `distillation`, `persistence`) consume only proofs/events.

### 2.3 Parametricity Rules

- Generic boundary helpers MAY inspect payloads only via explicit trait constraints.
- Unconstrained generics must not branch on payload shape.
- Any generic branch on payload data without contract is a hard violation of `IFA-4.2`.

### 2.4 Move-Semantics Rules

- A transition consumes the prior phase proof.
- Consumed proofs cannot be used for old-phase behavior.
- Only boundary helpers may execute `mem::replace` on operation state.

### 2.5 DRY Proof Map

- `OperationMachine` is the single proof for operation phase.
- `ToolingState` is the single proof for tool enablement.
- `TransitionReceipt` is the single proof that an operation transition happened.
- `FocusEffect` is the only effect channel for focus-mode changes.

## 3) Structural Violations and Exact IFA Mapping

| Gap | IFA sections | Why this is a violation | Required fix |
|-----|---------------|--------------------------|--------------|
| G1 | IFA-6.1, IFA-7.2 | Tooling has two owners: `tools_disabled_state` and `OperationState::ToolsDisabled` | Remove latch shadow; keep tooling only in machine proof |
| G2 | IFA-7.6, IFA-9.3 | Focus transitions are independent mutable writes | Derive focus from boundary transitions |
| G3 | IFA-7.1, IFA-7.2, IFA-17 | `self.state` mutation is fragmented across modules | Single transition authority boundary |
| G4 | IFA-10.1, IFA-11.1 | `busy_reason()` enforces phase at runtime instead of via proof | Replace with phase tokens/events and boundary-validated transitions |
| G5 | IFA-11.2, IFA-14.3 | Core payloads use `Option` as lifecycle sentinel | Replace with structural variants |
| G6 | IFA-12.1 | Impossible transitions are not universally fail-fast | Hard-fail invalid transition in boundary |
| G7 | IFA-11.3, IFA-17 | Transition intent and cross-cutting effects are scattered | One authoritative transition ledger |

## 4) Canonical Target Architecture

### 4.1 New boundary module: `engine/src/operation/`

Create:

- `engine/src/operation/mod.rs`
- `engine/src/operation/machine.rs`
- `engine/src/operation/event.rs`
- `engine/src/operation/ledger.rs`
- `engine/src/operation/focus.rs` (optional split for clarity)

#### `machine.rs`

- `enum OperationPhase`:
  - `Idle`
  - `Streaming(StreamingState)`
  - `PlanApproval(PlanApprovalState)`
  - `ToolLoop(ToolLoopState)`
  - `ToolRecovery(ToolRecoveryState)`
  - `Distilling(DistillingState)`
  - `RecoveryBlocked(RecoveryBlockedState)`
- `enum ToolingState`:
  - `Enabled`
  - `Disabled(ToolsDisabledState)`
- `struct OperationMachine { phase: OperationPhase, tooling: ToolingState }`

#### `event.rs`

- `enum OpEvent` with explicit domain events only:
  - `StartStreaming`, `StreamFinished`, `StreamFailed`
  - `PlanApprovalRequested`, `PlanApprovalApproved`, `PlanApprovalRejected`
  - `ToolLoopStarted`, `ToolLoopAdvanced`, `ToolLoopCommitted`
  - `ToolRecoveryStarted`, `ToolRecoveryResolved`
  - `DistillationStarted`, `DistillationFinished`
  - `ToolsDisabled`, `ToolsEnabled`
  - `RecoveryBlocked`
  - `CancelActiveOperation`

#### `ledger.rs`

- `TransitionReceipt` with:
  - `from: OpPhaseTag`
  - `event: OpEventTag`
  - `to: OpPhaseTag`
  - `focus_effect: FocusEffect`
  - `cause: &'static str`
  - timestamp
- Bounded in-memory ring buffer for diagnostics and UI replay.

#### `focus.rs`

- `enum FocusEffect`
  - `EnterExecuting { started_at: Instant }`
  - `EnterReviewing`
  - `EnterIdle`
  - `Noop`
- Boundary is the only emitter and applicator of focus effects.

### 4.2 Boundary API

- `EngineOperationBoundary::apply(event: OpEvent) -> Result<TransitionReceipt, OperationTransitionError>`
- Internal process:
  - compute `from` from current machine snapshot
  - validate legal `(from, event)` edge
  - update `OperationMachine`
  - update tooling state if event requires it
  - emit exactly one `TransitionReceipt`
  - apply exactly one focus effect
- Invalid edges are fail-fast in debug and explicit `OperationTransitionError` in UI-facing paths.

### 4.3 Mutation lock-down

Outside `engine/src/operation/`:

- No direct `self.state = ...`
- No `mem::replace(&mut self.state, ...)`
- No `self.tools_disabled_state = ...`
- No direct `focus_state = ...`

## 5) Tooling Ownership Elimination

- Delete `App::tools_disabled_state` from `engine/src/lib.rs`.
- Replace every tooling-latch write with `OpEvent::ToolsDisabled` and `OpEvent::ToolsEnabled`.
- `tool_journal_disabled_reason()` reads from `OperationMachine` proof exclusively.

## 6) Focus-State Redesign

### 6.1 Derive focus mode

- `FocusState` is updated only through `FocusEffect`.
- Executing mode is a derived consequence of active operation phases.
- Reviewing mode is entered only on explicit turn completion and review-context condition.
- Idle mode is entered only on non-active terminal idle phases.

### 6.2 Remove derivable optionality

- Replace `Option<Instant>` in `FocusState::Executing` with concrete type.
- Keep review cursor (`index`, `auto_advance`) as dedicated UI state.
- Keep `focus_review_next()` and `focus_review_prev()` as only mutable cursor operations.

## 7) Transition Graph (Authoritative)

Exactly these edges are legal:

- `Idle/ToolsDisabled` -> `Streaming` on `StartStreaming`
- `Idle/ToolsDisabled` -> `Distilling` on `DistillationStarted`
- `Streaming` -> `ToolLoop` on `StreamFinished` with tool calls
- `Streaming` -> `Idle/ToolsDisabled` on `StreamFinished` without tool calls
- `Streaming` -> `ToolRecovery` on stream journal failure path
- `Streaming` -> `RecoveryBlocked` on hard recovery block
- `PlanApproval` -> `ToolLoop` on `PlanApprovalApproved`
- `PlanApproval` -> `Idle/ToolsDisabled` on `PlanApprovalRejected`
- `PlanApproval` -> `RecoveryBlocked` on hard recovery block
- `ToolLoop` -> `ToolLoop` on `ToolLoopAdvanced`
- `ToolLoop` -> `Streaming` on `ToolLoopCommitted` with `auto_resume = true`
- `ToolLoop` -> `Idle/ToolsDisabled` on `ToolLoopCommitted` with `auto_resume = false`
- `ToolLoop` -> `PlanApproval` on `PlanApprovalRequested`
- `ToolLoop` -> `ToolRecovery` on recoverable journal issue requiring recovery
- `ToolLoop` -> `RecoveryBlocked` on unrecoverable tool recovery branch
- `ToolRecovery` -> `Streaming` or `Idle/ToolsDisabled` on `ToolRecoveryResolved`
- `Distilling` -> `Idle/ToolsDisabled` on `DistillationFinished`
- `RecoveryBlocked` -> `Idle/ToolsDisabled` only through explicit clear path
- `Any` -> same phase with tooling state changed by `ToolsDisabled` / `ToolsEnabled`

## 8) Migration Phases (Strict, Ordered, Non-negotiable)

### Phase 0: Boundary-first extraction

- Add operation boundary and ledger.
- Replace all 35 `self.state = ...`, 7 `mem::replace`, and 5 `replace_with_idle()` call paths with event application.
- Keep existing payload enums temporarily while migration compiles.

### Phase 1: Remove tool-disable shadow latch

- Delete `tools_disabled_state`.
- Move all 8 writes into explicit `ToolsDisabled` / `ToolsEnabled` transitions.
- Remove all reads outside boundary proofs.

### Phase 2: Focus effects only

- Remove direct focus writes from operational modules.
- Apply `FocusEffect` exactly inside boundary.
- Ensure no direct focus mutation outside boundary.

### Phase 3: Optionality elimination in core lifecycle payloads

- Replace `ToolLoopInput.tool_batch_id: Option<ToolBatchId>` with structural enum variant.
- Replace `thinking_message: Option<Message>` in tool loop payloads with explicit `ThinkingMessage` enum.
- Audit and remove additional core optionals that encode lifecycle absence.

### Phase 4: Runtime-guard eradication

- Remove `busy_reason()` as phase gate from command and stream entrypoints.
- Replace with transition precondition failures in boundary and phase tokens where necessary.

### Phase 5: Capability typing

- Add tokens (`IdleOp`, `StreamingOp`, `ToolLoopOp`, `ToolRecoveryOp`) for phase-locked operations.
- Consumers cannot invoke phase-restricted logic without token.

### Phase 6: Transition ledger enforcement

- Expose ledger for diagnostics, timings, and recovery breadcrumbs.
- Compute all duration metrics from transition receipts.
- Remove ad-hoc spread of duration logging and custom timing calculations.

## 9) Hard Acceptance Conditions

- `rg -n "self\\.state\\s*=" engine/src` yields only operation boundary module paths.
- `rg -n "tools_disabled_state" engine/src` yields only boundary-owned declaration and proof helpers.
- `rg -n "focus_state\\s*=|focus_state_mut\\(" engine/src` yields no operational logic outside the boundary.
- No operation state mutation via `mem::replace` outside boundary.
- `busy_reason()` is not used as command policy.
- `OpEvent` matrix coverage is unit-tested and fully covered.
- `transition_to_journaled` remains fail-fast and tested.
- `just verify` passes after each phase and finalization.

## 10) Test Requirements (minimum set, no skipping)

- Unit tests for every legal/illegal transition edge.
- Property tests for recovery and tooling-disablement consistency.
- Integration tests for tool journal failure and recovery paths.
- Determinism tests for focus mode transitions across streaming/tool-loop completion.

## 11) Final Guarantee

Post-refactor, operation lifecycle, tooling state, and focus mode are enforced by one boundary, one ledger, and non-optional proof state. There are no compatibility branches, no optional core lifecycle sentinels, and no scattered phase authority. Invalid behavior cannot be expressed through the safe API.  

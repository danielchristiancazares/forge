# OperationState and FocusState Gap Analysis

<!--
meta:
  doc_id: gap-analysis-operation-v1
  scope: engine state machine conformance against INVARIANT_FIRST_ARCHITECTURE
  source: INVARIANT_FIRST_ARCHITECTURE.md
  target: engine crate
  date: 2026-02-15
-->

> Fast path for deep-reasoning models: use `docs/analysis.prompt.md` as the initial prompt payload, then fetch specific sections from this file by `GAP-*` key.

## LLM Parsing Contract

- Canonical section keys are provided in HTML comments (example: `<!-- GAP-MAP-4 -->`).
- Gap records use a fixed field order:
  - `IFA refs`
  - `Problem`
  - `Evidence`
  - `Impact`
- File anchors always use `path:line` form.
- Section identifiers in this document are numeric IFA section values (example: `6.1`).

## LLM Friendly TOC

| ID | Section |
|----|---------|
| GAP-0 | Summary |
| GAP-INV | Live Inventory |
| GAP-WP | Write Pressure Evidence |
| GAP-MAP | IFA Gap Map |
| GAP-MAP-1 | G1: Shared state split across ownership domains |
| GAP-MAP-2 | G2: Parallel state machine not structurally synchronized |
| GAP-MAP-3 | G3: Transition ownership is fragmented |
| GAP-MAP-4 | G4: Protective guards in core flow |
| GAP-MAP-5 | G5: Weak traceability for cross-cutting logic |
| GAP-MAP-6 | G6: Optional field in FocusState::Executing |
| GAP-MAP-7 | G7: ActiveStream::transition_to_journaled no-op path |
| GAP-MAP-8 | G8: Optional tool_batch_id in ToolLoopInput |
| GAP-MAP-9 | G9: thinking_message triplication |
| GAP-MAP-10 | G10: idle_state() conflates mechanism and policy |
| GAP-MAP-11 | G11: busy_reason is protective guard |
| GAP-MAP-12 | G12: No Section 17 compliance path |
| GAP-XREF | Cross-Reference: gap to IFA to phase |
| GAP-PROP | Overhaul Design Proposal |
| GAP-PROP-A | Phase A: Single transition API |
| GAP-PROP-B | Phase B: Collapse tools-disabled ownership |
| GAP-PROP-C | Phase C: Focus synchronization |
| GAP-PROP-D | Phase D: Explicit transition graph |
| GAP-EXEC | Execution Sequence |
| GAP-RISK | Risk Notes |
| GAP-DRIFT | Residual Doc Drift |
| GAP-ANCHORS | File Anchors |

## Summary
<!-- GAP-0 -->

- Purpose: identify state machine gaps in `engine` against IFA and provide a controlled migration plan.
- Scope: `OperationState` in `engine/src/state.rs`, `FocusState` in `engine/src/ui/view_state.rs`, and mutation/synchronization across `engine/src/`.
- Core source: `INVARIANT_FIRST_ARCHITECTURE.md` (IFA sections referenced as numeric IDs, for example `6.1`).
- Main finding: 12 identified gaps.
- Key indicators:
  - 35 direct state mutation sites in `engine/src` (excluding test-only seeds).
  - two independent machines (`OperationState`, `FocusState`) that are not structurally synchronized.
  - one shadow field (`tools_disabled_state`) that duplicates machine ownership and weakens invariants.

## Live Inventory
<!-- GAP-INV -->

- `OperationState` has 8 variants, defined in `engine/src/state.rs`.
- `FocusState` has 3 variants, defined in `engine/src/ui/view_state.rs`.
- Transition shims exist in `engine/src/lib.rs`:
  - `idle_state()`
  - `replace_with_idle()`

## Write Pressure Evidence
<!-- GAP-WP -->

### Operation-state writes

- Total assignment sites in `engine/src`: **35** (excluding test-only assignments in `engine/src/tests.rs`).
- Assignment types:
  - direct variant writes: 22
  - idle normalization writes: 5
  - restore or fallback writes: 8

### Writes by file

| File | Sites |
|------|-------|
| `engine/src/commands.rs` | 4 |
| `engine/src/distillation.rs` | 4 |
| `engine/src/persistence.rs` | 4 |
| `engine/src/plan.rs` | 3 |
| `engine/src/streaming.rs` | 5 |
| `engine/src/tool_loop.rs` | 15 |

### Writes by variant

| Variant | Count |
|---------|-------|
| `OperationState::ToolLoop` | 11 |
| `OperationState::RecoveryBlocked` | 3 |
| `OperationState::Streaming` | 2 |
| `OperationState::Distilling` | 2 |
| `OperationState::PlanApproval` | 1 |
| `OperationState::ToolsDisabled` | 1 |
| `OperationState::ToolRecovery` | 1 |
| `OperationState::Idle` | 1 |

### Focus-state writes

- `engine/src/streaming.rs:246` writes `FocusState::Executing`.
- `engine/src/tool_loop.rs:1862` writes `FocusState::Reviewing`.
- No direct path in code writes `FocusState::Idle` at operation-end edges.

### Shadow disabled-state writes

- Total: 8 assignments to `tools_disabled_state` in:
  - `engine/src/commands.rs`: 493, 729
  - `engine/src/persistence.rs`: 418, 519, 629
  - `engine/src/tool_loop.rs`: 192, 321, 380

## IFA Gap Map
<!-- GAP-MAP -->

### G1 - Shared state split across ownership domains
<!-- GAP-MAP-1 -->
- IFA refs: 6.1, 7.6
- Problem: `tools_disabled_state` duplicates state that is already represented by `OperationState::ToolsDisabled`.
- Evidence:
  - `engine/src/lib.rs:1619` reads helper field.
  - `engine/src/lib.rs:1679-1685` reconstructs via `idle_state()`.
  - Multiple writes across `commands.rs`, `persistence.rs`, and `tool_loop.rs`.
- Impact: drift can appear if one source is updated without the other.

### G2 - Parallel state machine not structurally synchronized
<!-- GAP-MAP-2 -->
- IFA refs: 6.1, 7.6
- Problem: `FocusState` is updated independently from `OperationState`.
- Evidence:
  - `engine/src/streaming.rs:246` writes `Executing` in one path.
  - `engine/src/tool_loop.rs:1862` writes `Reviewing` in one path.
  - Multiple files transition operation state without guaranteed focus side updates.
- Impact: stale focus values can persist after completion, cancel, or error paths.

### G3 - Transition ownership is fragmented
<!-- GAP-MAP-3 -->
- IFA refs: 7.3, 6.1
- Problem: many call sites implement the same logical transition.
- Evidence:
  - dense transition logic in `tool_loop.rs`.
  - `replace_with_idle()` plus `std::mem::replace()` used in `streaming.rs`, `tool_loop.rs`, `distillation.rs`, `plan.rs`, `commands.rs`.
- Impact: side effects must be duplicated manually for each site, increasing misses.

### G4 - Protective guards in core flow
<!-- GAP-MAP-4 -->
- IFA refs: 7.4, 9.4, 13.6
- Problem: runtime checks (busy checks and fallback branches) enforce policy after transitions instead of at transition boundaries.
- Evidence:
  - gating and recovery checks split across call sites.
  - status derived from both `tools_disabled_state` and `OperationState`.
- Impact: defensive guard logic can drift from the true state representation.

### G5 - Traceability for cross-cutting logic is weak
<!-- GAP-MAP-5 -->
- IFA refs: 7.2, 10.1
- Problem: no canonical transition ledger for cross-cutting effects.
- Evidence: many transition sites independently infer that a transition occurred.
- Impact: transition-based side effects are hard to test and reason about consistently.

### G6 - FocusState::Executing carries an optional field
<!-- GAP-MAP-6 -->
- IFA refs: 11.2, 13
- Problem: `FocusState::Executing { step_started_at: Option<Instant> }` makes invalid in-memory states possible.
- Evidence: definition in `engine/src/ui/view_state.rs:57-58`.
- Impact: callers must handle unnecessary unknown-state branch.

### G7 - ActiveStream::transition_to_journaled no-op path (remediated)
<!-- GAP-MAP-7 -->
- IFA refs: 3.3
- Problem status: remediated.
- Evidence: `engine/src/state.rs:167-189` now uses `unreachable!` when already journaled.
- Impact: explicit failure now replaces silent no-op; still runtime check, not compile-time typestate.

### G8 - ToolLoopInput.tool_batch_id is optional
<!-- GAP-MAP-8 -->
- IFA ref: 11.2
- Problem: `Option<ToolBatchId>` in a core payload.
- Evidence: `engine/src/state.rs:63`.
- Impact: representable invalid state in input domain unless absence is structurally encoded.

### G9 - thinking_message triplication
<!-- GAP-MAP-9 -->
- IFA refs: 11.2, 7.5
- Problem: `Option<Message>` appears in three core payloads (`ToolLoopInput`, `ToolCommitPayload`, `ToolBatch`).
- Impact: repeated optionality pattern instead of one shared abstraction.

### G10 - idle_state() conflates mechanism and policy
<!-- GAP-MAP-10 -->
- IFA refs: 8.1, 8.2
- Problem: `idle_state()` chooses between `Idle` and `ToolsDisabled` internally and returns a single variant type.
- Evidence: `engine/src/lib.rs:1679-1685`.
- Impact: caller loses structural knowledge of which concrete state was entered.

### G11 - busy_reason as protective guard
<!-- GAP-MAP-11 -->
- IFA refs: 13.6, 7.4
- Problem: command eligibility is runtime-gated in `busy_reason`.
- Evidence: `engine/src/lib.rs:1634`.
- Impact: prohibited operation checks depend on remembering to call this guard.

### G12 - No Section 17 compliance deliverables
<!-- GAP-MAP-12 -->
- IFA ref: 17
- Problem: current plan does not yet define: invariant registry, authority map, parametricity rules, move-semantics rules, proof map.
- Impact: cannot claim full IFA conformance without these artifacts.

## Cross-Reference: Gap to IFA Section to Phase
<!-- GAP-XREF -->

| Gap | IFA Sections | Primary Remediation | Conformance Signal |
|-----|---------------|---------------------|--------------------|
| G1 | 6.1, 7.6 | Phase B | Conforming if single-owner model is adopted |
| G2 | 6.1, 7.6 | Phase C | Conforming (option 1) or limited if option 2 chosen |
| G3 | 7.3, 6.1 | Phase A | Runtime transition matrix only, unless typestate is implemented |
| G4 | 7.4, 9.4, 13.6 | Phase A + D | Runtime validation unless typestate is enforced |
| G5 | 7.2, 10.1 | Phase A | Centralized transition audit, not compile-time proof |
| G6 | 11.2, 13 p.8 | Standalone | Conforming with variant split or no optional |
| G7 | 3.3 | Remediated | Runtime fail-fast only |
| G8 | 11.2 | Standalone | Option or variant encoding required |
| G9 | 11.2, 7.5 | Standalone | Single shared abstraction required |
| G10 | 8.1, 8.2 | Phase B | Typed input or discriminated output required |
| G11 | 13.6, 7.4 | Phase D | Type-level command eligibility target |
| G12 | 17 | All phases | Required process deliverable |

## Overhaul Design Proposal
<!-- GAP-PROP -->

### Phase A - Single transition API
<!-- GAP-PROP-A -->
- Goal: centralize state transitions and make transition intent explicit.
- Core shape:
  - add private enum `OperationTransitionKind` (`Start`, `Finish`, `Cancel`, `Recover`, etc.).
  - add `App::transition_state(kind, payload)`.
  - transition entry point validates allowed edges and applies focus updates.
- Migration scope:
  - replace direct `self.state = ...` writes.
  - replace `self.state = self.idle_state()`.
  - replace restore/fallback assignments with transition entry.
  - replace defensive state-match fallback branches where possible.
- Conformance: centralizes control but may still be runtime-only unless variant-level typestate is adopted.
- Construction control: only transition module should construct operation variants; others should not directly instantiate states.

### Phase B - Collapse tools-disabled ownership
<!-- GAP-PROP-B -->
- Goal: make `OperationState::ToolsDisabled` the only tools-disabled storage location.
- Actions:
  - remove `tools_disabled_state` from `App`.
  - remove `idle_state()` helper.
  - use transition typing so idle-to-disabled decision is explicit and structural.
  - route all checks, persistence reads, and command eligibility through `self.state` only.
- Risk: high due to recovery semantics.

### Phase C - Focus synchronization strategy
<!-- GAP-PROP-C -->
- Goal: remove independent mutable ownership mismatch between operation and focus state.
- Preferred option 1: derive `FocusState` from `OperationState + view mode` each render/update cycle.
  - extract `Reviewing` nav state to separate struct when needed.
  - no mutable synchronization writes for focus.
- Option 2: keep focus field but route all focus writes through transition API only.
- Priority: option 1 unless proven impractical.

### Phase D - Explicit transition graph and validation
<!-- GAP-PROP-D -->
- Goal: make transition legality and side-effects explicit and testable.
- Actions:
  - add transition matrix mapping all valid source->target edges.
  - add transition tests:
    - one focus transition per public operation transition.
    - no invalid focus+operation pair after any transition.
    - recovery and error paths preserve invariants.
- Target conformance: compile-time if feasible, runtime fallback only with explicit limitation argument.

## Execution Sequence
<!-- GAP-EXEC -->

| Step | Action | Gaps |
|------|--------|-------|
| 1 | Freeze this analysis as canonical evidence base. | N/A |
| 2 | Add transition enum and `App::transition_state` authority boundary. | G3, G5 |
| 3 | Route all mutation sites through transition API and enforce ownership rules. | G3, G4 |
| 4 | Merge tools-disabled ownership into `OperationState`. | G1, G10 |
| 5 | Align focus updates with transition boundaries. | G2 |
| 6 | Resolve `FocusState::Executing` optionality. | G6 |
| 7 | Add regression test for `ActiveStream::transition_to_journaled`. | G7 |
| 8 | Rework `tool_batch_id` handling structure. | G8 |
| 9 | Consolidate thinking message handling into one abstraction. | G9 |
| 10 | Move command gating from runtime guard to type-level API shape. | G11 |
| 11 | Add Section 17 compliance artifacts. | G12 |

## Risk Notes
<!-- GAP-RISK -->

- High: ownership merge for tools-disabled can affect recovery and error behavior.
- Medium: focus transitions may change behavior when user mode switches during operations.
- Low: migration churn from many `self.state` assignment sites (~35).

## Residual Doc Drift
<!-- GAP-DRIFT -->

- `engine/README.md` and relevant `docs/` sections may describe an older model.
- When transition API lands, update those documents in lockstep with implementation.

## File Anchors
<!-- GAP-ANCHORS -->

### Definitions

| File | Entity |
|------|--------|
| `engine/src/state.rs:596` | `OperationState` |
| `engine/src/ui/view_state.rs:51` | `FocusState` |
| `engine/src/init.rs:317` | `initial OperationState` |
| `engine/src/init.rs:341` | `initial tools_disabled_state` |

### Core ownership and entrypoints

| File | Entity |
|------|--------|
| `engine/src/lib.rs:763` | `App.state` |
| `engine/src/lib.rs:1634` | `busy_reason` |
| `engine/src/lib.rs:1679` | `idle_state` |
| `engine/src/lib.rs:1687` | `replace_with_idle` |
| `engine/src/lib.rs:1884` | `tick` |
| `engine/src/streaming.rs:478` | `process_stream_events` |
| `cli/src/main.rs:315` | `app.tick()` |
| `cli/src/main.rs:316` | `app.process_stream_events()` |

### Focus mutation points

| File | Value |
|------|--------|
| `engine/src/streaming.rs:246` | `Executing` |
| `engine/src/tool_loop.rs:1862` | `Reviewing` |
| `engine/src/lib.rs:877` | `focus_review_next` |
| `engine/src/lib.rs:888` | `focus_review_prev` |

### Operation-state write hotspots

| File | Lines |
|------|-------|
| `engine/src/commands.rs` | 379, 395, 499, 731 |
| `engine/src/distillation.rs` | 116, 147, 198, 216 |
| `engine/src/plan.rs` | 451, 527, 555 |
| `engine/src/persistence.rs` | 424, 434, 481, 612 |
| `engine/src/streaming.rs` | 362, 591, 707, 714, 787 |
| `engine/src/tool_loop.rs` | 562, 573, 627, 956, 965, 1062, 1106, 1300, 1305, 1408, 1509, 1575, 1582, 1759, 1767 |

### Shadow-state writes

| File | Lines |
|------|-------|
| `engine/src/commands.rs` | 493, 729 |
| `engine/src/persistence.rs` | 418, 519, 629 |
| `engine/src/tool_loop.rs` | 192, 321, 380 |

### Test-only direct state seeding

| File | Lines |
|------|-------|
| `engine/src/tests.rs` | 147, 1032, 1070, 1120, 1155, 1687 |

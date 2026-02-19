# PLAN_IFA_PHASE_2_OPERATION_AUTHORITY

## Purpose

Make `core::operation` the only authority that can change operation phase.

## Drivers

- Transition labels exist (`OperationTag`, `OperationEdge`), but authority is still fragmented.
- `mem::replace` plus `op_restore` appears in multiple modules:
  - `engine/src/app/tool_loop.rs:952-969`
  - `engine/src/app/distillation.rs:139-149`
  - `engine/src/app/plan.rs:442-454`

## Scope

- Move transition legality graph and edge logic out of `App`.
- Expose one transition API (`apply(event)` style) from `core::operation`.
- Remove direct phase mutation from app submodules.

## Tasks

1. Add `core::operation` module with:
   - machine state
   - legal transition graph
   - transition receipt/result type
2. Move legality checks from `App` into this module.
3. Convert callsites from ad hoc mutation to machine events.
4. Keep edge/event names stable via `OperationEdge`.

## Candidate files

- `engine/src/core/operation/mod.rs` (new)
- `engine/src/core/operation/machine.rs` (new)
- `engine/src/core/operation/graph.rs` (new)
- `engine/src/core/mod.rs`
- `engine/src/app/mod.rs`
- `engine/src/app/tool_loop.rs`
- `engine/src/app/distillation.rs`
- `engine/src/app/plan.rs`
- `engine/src/app/streaming.rs`

## Exit criteria

- No transition legality logic remains in `engine/src/app/mod.rs`.
- Operation phase mutation occurs only through `core::operation`.
- Existing edge logging semantics remain intact.

## Validation

- `just fix`
- `just verify`

--
 PLAN_IFA_PHASE_2_OPERATION_AUTHORITY

 What this is

 This is a mechanical implementation runbook for Phase 2.
 It is written so another Claude session can execute it in order without looking anything up.

 Goal

 Extract the operation state transition authority out of App and into a dedicated OperationMachine in core::operation. After this refactor:

 - OperationMachine owns a private state: Option<OperationState> field — unauthorized writes are a compile error (IFA §2.1), replacing the current grep-based
 guardrail. The Option sentinel makes zombie machines inert (IFA §9.3/§15.7).
 - The transition legality graph lives in core::operation::graph — pure functions testable in isolation.
 - App methods (op_transition, op_restore, etc.) become thin wrappers that delegate to the machine and dispatch UI side-effects.

 Hard rules

 - Do not change behavior.
 - Do not change public API signatures in this phase.
 - Run just fix then just verify after each major step.

 IFA conformance

 ┌───────────┬──────────────────────────────────────────────────────────────────────────────────────────────────┐
 │ Section   │                                            Conformance                                          │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §1.3/§2.1 │ `TakeToken` linear proof type: non-Copy, non-Clone. `take()` returns `(OperationState,          │
 │           │ TakeToken)`. `restore()` and `transition_from()` consume token. Rust move semantics enforce      │
 │           │ exactly-once consumption at compile time — `from` tag is unforgeable.                            │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §2.1      │ Private `Option<OperationState>` field + no `state_mut()`. Typed variant accessors only.         │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §6.1      │ `OperationMachine` is the sole write authority.                                                  │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §7        │ Transition graph encoded once in `graph.rs`.                                                     │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §8        │ Machine = mechanism (returns `TransitionReceipt`). App = policy (dispatches UI effects).          │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §9.3/     │ Internal `Option<OperationState>` sentinel: `None` = taken. All read/write methods panic on      │
 │ §15.7     │ `None`, making the zombie inert. The `Option` is internal to the authority boundary (private     │
 │           │ field) — §11.2 (no optionality in core interfaces) does not apply.                               │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §12.1     │ `assert!` (not `debug_assert!`) for illegal transitions — fail-stop in all builds.               │
 ├───────────┼──────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ §12.2     │ Acknowledged non-conformance: `restore()` is an unvalidated write necessary for Rust             │
 │           │ take+restore. §12.2 applies — this component is not fully IFA-conformant for this invariant.     │
 └───────────┴──────────────────────────────────────────────────────────────────────────────────────────────────┘

 Current state (what we're starting from)

 App struct (engine/src/app/mod.rs:836-840)

 pub struct App {
     ui: AppUi,
     core: AppCore,
     runtime: AppRuntime,
 }

 AppCore state field (engine/src/app/mod.rs:741)

 struct AppCore {
     // ...
     state: OperationState,       // ← this moves into OperationMachine
     // ...
 }

 Types in engine/src/state.rs

 // line 644
 pub(crate) enum OperationState {
     Idle,
     Streaming(ActiveStream),
     ToolLoop(Box<ToolLoopState>),
     PlanApproval(Box<PlanApprovalState>),
     ToolRecovery(ToolRecoveryState),
     RecoveryBlocked(RecoveryBlockedState),
     Distilling(DistillationState),
 }

 // line 659
 pub(crate) enum OperationTag {
     Idle, Streaming, ToolLoop, PlanApproval, ToolRecovery, RecoveryBlocked, Distilling,
 }

 // line 675
 pub(crate) enum OperationEdge {
     StartStreaming,
     EnterToolLoopAwaitingApproval,
     EnterToolLoopExecuting,
     ResolvePlanApproval,
     FinishToolBatch,
     StartDistillation,
     FinishTurn,
 }

 Existing App methods being moved/replaced (engine/src/app/mod.rs)

 To be deleted (moved into machine/graph):

 // line 1710-1712
 fn idle_state(&self) -> OperationState {
     OperationState::Idle
 }

 // line 1715-1718
 fn replace_with_idle(&mut self) -> OperationState {
     let idle = self.idle_state();
     std::mem::replace(&mut self.core.state, idle)
 }

 // line 1759-1775 — moves to graph.rs
 fn op_transition_edge(from: OperationTag, to: OperationTag) -> Option<OperationEdge> {
     use OperationEdge::{ ... };
     use OperationTag::{ ... };
     match (from, to) {
         (Idle, Streaming) => Some(StartStreaming),
         (Idle, Distilling) => Some(StartDistillation),
         (Streaming, PlanApproval) => Some(EnterToolLoopAwaitingApproval),
         (Streaming, ToolLoop) => Some(EnterToolLoopExecuting),
         (PlanApproval, ToolLoop) => Some(ResolvePlanApproval),
         (ToolLoop | PlanApproval | Idle, Idle) => Some(FinishToolBatch),
         _ => None,
     }
 }

 // line 1777-1801 — moves to graph.rs
 fn op_is_legal_transition(from: OperationTag, edge: OperationEdge, to: OperationTag) -> bool {
     use OperationEdge::{ ... };
     use OperationTag::{ ... };
     match edge {
         StartStreaming => from == Idle && to == Streaming,
         StartDistillation => from == Idle && to == Distilling,
         EnterToolLoopAwaitingApproval => from == Streaming && matches!(to, PlanApproval | ToolLoop),
         EnterToolLoopExecuting => matches!(to, ToolLoop) && matches!(from, Streaming),
         ResolvePlanApproval => from == PlanApproval && matches!(to, ToolLoop),
         FinishToolBatch => to == Idle && matches!(from, ToolLoop | PlanApproval | Idle),
         FinishTurn => from == to && from == Idle,
     }
 }

 To be rewritten as thin wrappers:

 // line 1810-1851 — op_transition: validation+logging moves to machine
 // line 1858-1898 — op_transition_from: same
 // line 1905-1907 — op_restore: delegates to machine.restore()
 // line 1725-1757 — op_edge: validation moves to machine

 Stays unchanged:

 // line 1913-1928 — op_apply_edge_effects (touches self.ui, can't move)

 Step 0 — Create core::operation module scaffold

 Create directory engine/src/core/operation/.

 engine/src/core/operation/mod.rs (new file)

 //! Operation state machine authority (IFA Phase 2).
 //!
 //! This module is the sole write authority for `OperationState`.
 //! The `OperationMachine` wrapper enforces this structurally:
 //! the `state` field is private, so unauthorized writes are compile errors.

 mod graph;
 mod machine;

 pub(crate) use graph::{is_legal, transition_edge};
 pub(crate) use machine::{OperationMachine, TakeToken, TransitionReceipt};

 engine/src/core/operation/graph.rs (new file)

 //! Pure transition legality graph.
 //!
 //! No state, no side effects. These functions encode the legal transition
 //! topology for `OperationState` and are the single point of encoding (IFA §7).

 use crate::state::{OperationEdge, OperationTag};

 /// Map a `(from, to)` tag pair to the named edge that connects them, if any.
 pub(crate) fn transition_edge(from: OperationTag, to: OperationTag) -> Option<OperationEdge> {
     use OperationEdge::{
         EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch,
         ResolvePlanApproval, StartDistillation, StartStreaming,
     };
     use OperationTag::{Distilling, Idle, PlanApproval, Streaming, ToolLoop};

     match (from, to) {
         (Idle, Streaming) => Some(StartStreaming),
         (Idle, Distilling) => Some(StartDistillation),
         (Streaming, PlanApproval) => Some(EnterToolLoopAwaitingApproval),
         (Streaming, ToolLoop) => Some(EnterToolLoopExecuting),
         (PlanApproval, ToolLoop) => Some(ResolvePlanApproval),
         (ToolLoop | PlanApproval | Idle, Idle) => Some(FinishToolBatch),
         _ => None,
     }
 }

 /// Check whether a `(from, edge, to)` triple is legal.
 pub(crate) fn is_legal(from: OperationTag, edge: OperationEdge, to: OperationTag) -> bool {
     use OperationEdge::{
         EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch, FinishTurn,
         ResolvePlanApproval, StartDistillation, StartStreaming,
     };
     use OperationTag::{Distilling, Idle, PlanApproval, Streaming, ToolLoop};

     match edge {
         StartStreaming => from == Idle && to == Streaming,
         StartDistillation => from == Idle && to == Distilling,
         EnterToolLoopAwaitingApproval => {
             from == Streaming && matches!(to, PlanApproval | ToolLoop)
         }
         EnterToolLoopExecuting => matches!(to, ToolLoop) && matches!(from, Streaming),
         ResolvePlanApproval => from == PlanApproval && matches!(to, ToolLoop),
         FinishToolBatch => to == Idle && matches!(from, ToolLoop | PlanApproval | Idle),
         FinishTurn => from == to && from == Idle,
     }
 }

 engine/src/core/operation/machine.rs (new file)

 //! `OperationMachine` — sole write authority for `OperationState`.
 //!
 //! The `state` field is private. Only methods on this struct can mutate it.
 //! This makes unauthorized state writes a compile error (IFA §2.1).
 //!
 //! The field is `Option<OperationState>`: `None` = taken (fail-stop sentinel).
 //! All read/write methods panic on `None`, making a zombie machine inert (IFA §9.3/§15.7).

 use crate::state::{
     ActiveStream, OperationEdge, OperationState, OperationTag, ToolLoopState,
 };

 use super::graph;

 /// Linear proof that a `take()` occurred. Non-Copy, non-Clone.
 /// Must be consumed by `restore()` or `transition_from()`.
 ///
 /// Rust move semantics enforce exactly-once consumption at compile time.
 /// The `from` tag is captured at take-time and cannot be forged (IFA §1.3/§2.1).
 pub(crate) struct TakeToken {
     from: OperationTag,
 }

 impl TakeToken {
     pub(crate) fn from_tag(&self) -> OperationTag {
         self.from
     }
 }

 /// Record of a named transition edge that was fired.
 ///
 /// Returned by `transition` / `transition_from` when a legal named edge
 /// connects the `from` and `to` states. Callers use this to dispatch
 /// cross-cutting effects (UI sync, metrics, etc.).
 ///
 /// Not `Clone` or `Copy` — consumed by `op_apply_edge_effects` (IFA §1.3).
 /// Only produced on legal transitions (illegal ones panic before construction).
 #[derive(Debug)]
 pub(crate) struct TransitionReceipt {
     pub(crate) from: OperationTag,
     pub(crate) edge: OperationEdge,
     pub(crate) to: OperationTag,
 }

 /// Wraps `OperationState` with controlled mutation.
 ///
 /// # Write API (4 production methods + 1 test-only, all `&mut self`)
 ///
 /// | Method | Validates | Logs | Use case |
 /// |--------|-----------|------|----------|
 /// | `transition` | Yes (assert!) | Yes | Normal state changes |
 /// | `transition_from` | Yes (assert!) | Yes | State changes after `take()` — consumes `TakeToken` |
 /// | `restore` | No | No | Put-back after `take()` — consumes `TakeToken` |
 /// | `take` | No | No | Extract state + produce `TakeToken` |
 /// | `set_state` | No | No | Test-only setup (`#[cfg(test)]`) |
 ///
 /// # Read API
 ///
 /// | Method | Returns |
 /// |--------|---------|
 /// | `state` | `&OperationState` — pattern matching, no mutation |
 /// | `tag` | `OperationTag` — cheap copy for logging |
 /// | `streaming_mut` | `Option<&mut ActiveStream>` — interior mutation only |
 /// | `tool_loop_mut` | `Option<&mut ToolLoopState>` — interior mutation only |
 ///
 /// # Why no `state_mut()`
 ///
 /// Returning `&mut OperationState` would allow `*machine.state_mut() = Idle`,
 /// bypassing all transition validation. Typed variant accessors prevent
 /// variant-level assignment while permitting interior data mutation (IFA §2.1).
 #[derive(Debug)]
 pub(crate) struct OperationMachine {
     state: Option<OperationState>,
 }

 impl OperationMachine {
     pub(crate) fn new(state: OperationState) -> Self {
         Self { state: Some(state) }
     }

     /// Current state tag (cheap copy for logging/comparison).
     ///
     /// # Panics
     /// Panics if `state` is `None` (zombie — taken but never restored).
     pub(crate) fn tag(&self) -> OperationTag {
         self.state.as_ref().expect("OperationMachine: zombie (taken but never restored)").tag()
     }

     /// Shared read access for pattern matching.
     ///
     /// # Panics
     /// Panics if `state` is `None` (zombie).
     pub(crate) fn state(&self) -> &OperationState {
         self.state.as_ref().expect("OperationMachine: zombie (taken but never restored)")
     }

     /// Mutable access to `ActiveStream` data without variant-level mutation.
     ///
     /// # Panics
     /// Panics if `state` is `None` (zombie).
     pub(crate) fn streaming_mut(&mut self) -> Option<&mut ActiveStream> {
         match self.state.as_mut().expect("OperationMachine: zombie (taken but never restored)") {
             OperationState::Streaming(active) => Some(active),
             _ => None,
         }
     }

     /// Mutable access to `ToolLoopState` data without variant-level mutation.
     ///
     /// # Panics
     /// Panics if `state` is `None` (zombie).
     pub(crate) fn tool_loop_mut(&mut self) -> Option<&mut ToolLoopState> {
         match self.state.as_mut().expect("OperationMachine: zombie (taken but never restored)") {
             OperationState::ToolLoop(state) => Some(state.as_mut()),
             _ => None,
         }
     }

     /// Extract state and produce a `TakeToken` proving the take occurred.
     ///
     /// Sets internal state to `None` (zombie sentinel). All subsequent
     /// read/write calls will panic until `restore` or `transition_from`
     /// is called with the returned token (IFA §9.3/§15.7).
     ///
     /// The caller MUST consume the token via `restore` or `transition_from`.
     /// Rust move semantics enforce this at compile time.
     pub(crate) fn take(&mut self) -> (OperationState, TakeToken) {
         let state = self.state.take()
             .expect("OperationMachine: double take (already in zombie state)");
         let token = TakeToken { from: state.tag() };
         (state, token)
     }

     /// Raw write-back for "take + restore" patterns.
     ///
     /// Consumes `TakeToken` to ensure exactly-once put-back.
     /// Does NOT validate or log. This exists because the temporary zombie
     /// during take+restore is a Rust borrow-checker artifact, not a real
     /// transition. Logging it would drown out real lifecycle edges.
     pub(crate) fn restore(&mut self, _token: TakeToken, next: OperationState) {
         self.state = Some(next);
     }

     /// Validated state transition with logging.
     ///
     /// Returns `Some(TransitionReceipt)` if a named edge was fired,
     /// `None` if the transition has no named edge (still applied).
     ///
     /// # Panics
     /// - Panics if `state` is `None` (zombie).
     /// - Panics (`assert!`) if the transition has a named edge that is illegal (IFA §12.1).
     #[track_caller]
     pub(crate) fn transition(&mut self, next: OperationState) -> Option<TransitionReceipt> {
         let from = self.tag();
         let to = next.tag();
         let edge = graph::transition_edge(from, to);

         if let Some(edge) = edge {
             let loc = std::panic::Location::caller();
             let legal = graph::is_legal(from, edge, to);
             if !legal {
                 tracing::warn!(
                     from = ?from,
                     to = ?to,
                     edge = edge.as_str(),
                     file = loc.file(),
                     line = loc.line(),
                     column = loc.column(),
                     "Illegal OperationState transition",
                 );
                 assert!(
                     legal,
                     "Illegal OperationState transition: {from:?} --{edge:?}--> {to:?} at {}:{}:{}",
                     loc.file(),
                     loc.line(),
                     loc.column()
                 );
             }
         }

         if from != to {
             let loc = std::panic::Location::caller();
             tracing::debug!(
                 from = ?from,
                 to = ?to,
                 file = loc.file(),
                 line = loc.line(),
                 column = loc.column(),
                 "OperationState transition",
             );
         }

         self.state = Some(next);
         edge.map(|edge| TransitionReceipt { from, edge, to })
     }

     /// Like [`Self::transition`], but consumes a `TakeToken` to prove the `from` tag.
     ///
     /// Used after `take()` when the machine is in zombie state (`None`) but the
     /// real semantic origin was the state that was taken. The token's `from` tag
     /// is unforgeable — it was captured at take-time (IFA §1.3/§2.1).
     ///
     /// # Panics
     /// Panics (`assert!`) if the transition has a named edge that is illegal (IFA §12.1).
     #[track_caller]
     pub(crate) fn transition_from(
         &mut self,
         token: TakeToken,
         next: OperationState,
     ) -> Option<TransitionReceipt> {
         let from = token.from;
         let to = next.tag();
         let edge = graph::transition_edge(from, to);

         if let Some(edge) = edge {
             let loc = std::panic::Location::caller();
             let legal = graph::is_legal(from, edge, to);
             if !legal {
                 tracing::warn!(
                     from = ?from,
                     to = ?to,
                     edge = edge.as_str(),
                     file = loc.file(),
                     line = loc.line(),
                     column = loc.column(),
                     "Illegal OperationState transition",
                 );
                 assert!(
                     legal,
                     "Illegal OperationState transition: {from:?} --{edge:?}--> {to:?} at {}:{}:{}",
                     loc.file(),
                     loc.line(),
                     loc.column()
                 );
             }
         }

         if from != to {
             let loc = std::panic::Location::caller();
             tracing::debug!(
                 from = ?from,
                 to = ?to,
                 file = loc.file(),
                 line = loc.line(),
                 column = loc.column(),
                 "OperationState transition",
             );
         }

         self.state = Some(next);
         edge.map(|edge| TransitionReceipt { from, edge, to })
     }

     /// Check if an edge is legal at the current state (for same-state edges).
     ///
     /// # Panics
     /// Panics if `state` is `None` (zombie).
     pub(crate) fn validate_edge(&self, edge: OperationEdge) -> bool {
         let tag = self.tag();
         graph::is_legal(tag, edge, tag)
     }

     /// Test-only state override. Bypasses transition validation.
     ///
     /// Exists so test setup doesn't need a `TakeToken` for initial state configuration.
     #[cfg(test)]
     pub(crate) fn set_state(&mut self, state: OperationState) {
         self.state = Some(state);
     }
 }

 Update engine/src/core/mod.rs

 Add after line 8:

 pub(crate) mod operation;

 Step 1 — Update AppCore in engine/src/app/mod.rs

 Replace the state field in AppCore (line 741)

 Replace:
     state: OperationState,

 With:
     operation: crate::core::operation::OperationMachine,

 Step 2 — Update constructor in engine/src/app/init.rs

 Update import (line 11)

 Replace:
 use crate::state::{DataDir, DataDirSource, OperationState};

 With:
 use crate::state::{DataDir, DataDirSource};

 Replace constructor field (line 127)

 Replace:
             state: OperationState::Idle,

 With:
             operation: crate::core::operation::OperationMachine::new(
                 crate::state::OperationState::Idle,
             ),

 Step 3 — Rewrite App transition methods in engine/src/app/mod.rs

 Delete these methods entirely:

 - idle_state (lines 1710-1712)
 - replace_with_idle (lines 1715-1718)
 - op_transition_edge (lines 1759-1775)
 - op_is_legal_transition (lines 1777-1801)

 Replace op_transition (lines 1803-1851) with:

     /// Authoritative `OperationState` transition point.
     ///
     /// Delegates validation and logging to `OperationMachine`, then dispatches
     /// cross-cutting UI effects for any named edge that fired.
     #[track_caller]
     fn op_transition(&mut self, next: OperationState) {
         if let Some(receipt) = self.core.operation.transition(next) {
             self.op_apply_edge_effects(receipt.from, receipt.edge, receipt.to);
         }
     }

 Replace op_transition_from (lines 1853-1898) with:

     /// Like [`Self::op_transition`], but uses `TakeToken` to prove the `from` tag.
     ///
     /// Used after `take()` — the token proves which state was taken and
     /// prevents forging the `from` tag (IFA §1.3/§2.1).
     #[track_caller]
     fn op_transition_from(&mut self, token: TakeToken, next: OperationState) {
         if let Some(receipt) = self.core.operation.transition_from(token, next) {
             self.op_apply_edge_effects(receipt.from, receipt.edge, receipt.to);
         }
     }

 Replace op_restore (lines 1900-1907) with:

     /// Internal state write used for "take + restore" patterns.
     ///
     /// Consumes `TakeToken` to ensure exactly-once put-back.
     /// Delegates to `OperationMachine::restore()` — no validation, no logging.
     fn op_restore(&mut self, token: TakeToken, next: OperationState) {
         self.core.operation.restore(token, next);
     }

 Replace op_edge (lines 1720-1757) with:

     /// Emit an operation edge without changing `OperationState`.
     ///
     /// Used for lifecycle edges that should remain centrally observable even when
     /// Rust move/borrow rules force us into "take + restore" implementation patterns.
     #[track_caller]
     fn op_edge(&mut self, edge: OperationEdge) {
         let tag = self.core.operation.tag();
         let legal = self.core.operation.validate_edge(edge);
         if !legal {
             let loc = std::panic::Location::caller();
             tracing::warn!(
                 state = ?tag,
                 edge = edge.as_str(),
                 file = loc.file(),
                 line = loc.line(),
                 column = loc.column(),
                 "Illegal Operation edge",
             );
             assert!(
                 legal,
                 "Illegal Operation edge: {tag:?} --{edge:?}--> {tag:?} at {}:{}:{}",
                 loc.file(),
                 loc.line(),
                 loc.column()
             );
         }

         let loc = std::panic::Location::caller();
         tracing::debug!(
             state = ?tag,
             edge = edge.as_str(),
             file = loc.file(),
             line = loc.line(),
             column = loc.column(),
             "Operation edge",
         );

         self.op_apply_edge_effects(tag, edge, tag);
     }

 Step 4 — Migrate read/write sites in engine/src/app/mod.rs

 All replacements are inside impl App only (starts at line 856).
 Do NOT touch non-App impls (e.g., impl StreamingMessage, settings editors).

 Read patterns (replace self.core.state → self.core.operation)

 ┌──────┬────────────────────────────────────────────────────────────────┬────────────────────────────────────────────────────────────────────────────┐
 │ Line │                              Old                               │                                    New                                     │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1343 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1353 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1457 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1467 │ if !matches!(self.core.state, OperationState::PlanApproval(_)) │ if !matches!(self.core.operation.state(), OperationState::PlanApproval(_)) │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1497 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1504 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1519 │ self.core.state, (inside matches!)                             │ self.core.operation.state(),                                               │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1658 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1667 │ match &self.core.state {                                       │ match self.core.operation.state() {                                        │
 ├──────┼────────────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────────────────────────┤
 │ 1726 │ self.core.state.tag()                                          │ self.core.operation.tag()                                                  │
 └──────┴────────────────────────────────────────────────────────────────┴────────────────────────────────────────────────────────────────────────────┘

 Mutable interior access (1 site)

 Line: 1360-1365
 Old: fn tool_loop_state_mut(&mut self) -> Option<&mut state::ToolLoopState> { match &mut self.core.state { OperationState::ToolLoop(state) =>
   Some(state.as_mut()), _ => None } }
 New: fn tool_loop_state_mut(&mut self) -> Option<&mut state::ToolLoopState> { self.core.operation.tool_loop_mut() }

 Step 5 — Migrate sites in engine/src/app/streaming.rs

 Read patterns

 ┌──────┬─────────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────────┐
 │ Line │                             Old                             │                                   New                                   │
 ├──────┼─────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────┤
 │ 335  │ self.core.state, (inside format debug)                      │ self.core.operation.state(),                                            │
 ├──────┼─────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────┤
 │ 498  │ if !matches!(self.core.state, OperationState::Streaming(_)) │ if !matches!(self.core.operation.state(), OperationState::Streaming(_)) │
 └──────┴─────────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────────┘

 Mutable interior access (3 sites)

 Lines 521, 545, 571 all have the same pattern:
 let active = match &mut self.core.state {
     OperationState::Streaming(active) => active,
     _ => return,  // or break
 };

 Replace each with:
 let Some(active) = self.core.operation.streaming_mut() else {
     return;  // or break — match the original
 };

 Take patterns (1 site)

 Line: 606-610
 Old: let idle = self.idle_state(); let mut active = match std::mem::replace(&mut self.core.state, idle) { OperationState::Streaming(active) => active,
   other => { self.op_restore(other); return; } };
 New:
 let (state, token) = self.core.operation.take();
 let mut active = match state {
     OperationState::Streaming(active) => active,
     other => { self.op_restore(token, other); return; }
 };

 replace_with_idle (2 sites)

 Lines 729 and 803 — self.replace_with_idle() → self.core.operation.take()
 Token must be consumed on each code path:
 - Line 729: let (state, token) = self.core.operation.take(); ... self.op_transition_from(token, next)
 - Line 803: let (state, token) = self.core.operation.take(); ... self.op_transition_from(token, next)

 idle_state (0 direct calls remain after above changes)

 Step 6 — Migrate sites in engine/src/app/tool_loop.rs

 Take patterns (3 sites — lines 973-980, 1614-1619, 1812-1817)

 Each has:
 let idle = self.idle_state();
 let state = match std::mem::replace(&mut self.core.state, idle) {
     OperationState::ToolLoop(state) => *state,
     other => {
         self.op_restore(other);
         return;
     }
 };

 Replace with:
 let (taken, token) = self.core.operation.take();
 let tl_state = match taken {
     OperationState::ToolLoop(s) => *s,
     other => {
         self.op_restore(token, other);
         return;
     }
 };

 For paths that call functions touching the machine state after take
 (e.g. `commit_tool_batch`, `cancel_tool_batch`, `handle_distillation_failure`,
 `start_streaming`), consume the token first:
     self.core.operation.restore(token, OperationState::Idle);
     self.commit_tool_batch(...);

 For `op_transition_from` calls:
     // OLD: self.op_transition_from(OperationTag::X, next)
     // NEW: self.op_transition_from(token, next)

 idle_state in transitions (2 sites)

 ┌──────┬────────────────────────────────────────┬───────────────────────────────────────────┐
 │ Line │                  Old                   │                    New                    │
 ├──────┼────────────────────────────────────────┼───────────────────────────────────────────┤
 │ 1437 │ self.op_transition(self.idle_state()); │ self.op_transition(OperationState::Idle); │
 ├──────┼────────────────────────────────────────┼───────────────────────────────────────────┤
 │ 1545 │ self.op_transition(self.idle_state()); │ self.op_transition(OperationState::Idle); │
 └──────┴────────────────────────────────────────┴───────────────────────────────────────────┘

 Step 7 — Migrate sites in engine/src/app/distillation.rs

 Read pattern (1 site)

 ┌──────┬──────────────────────────┬─────────────────────────────────────┐
 │ Line │           Old            │                 New                 │
 ├──────┼──────────────────────────┼─────────────────────────────────────┤
 │ 130  │ match &self.core.state { │ match self.core.operation.state() { │
 └──────┴──────────────────────────┴─────────────────────────────────────┘

 Take pattern (1 site — lines 139-149)

 Replace:
 let idle = self.idle_state();
 let (task, queued_request) = match std::mem::replace(&mut self.core.state, idle) {
 With:
 let (taken, token) = self.core.operation.take();
 let (task, queued_request) = match taken {
 (non-matching arms use self.op_restore(token, other); return;)
 (paths that call start_streaming or handle_distillation_failure: restore token first)

 idle_state in transitions (1 site)

 ┌──────┬────────────────────────────────────────┬───────────────────────────────────────────┐
 │ Line │                  Old                   │                    New                    │
 ├──────┼────────────────────────────────────────┼───────────────────────────────────────────┤
 │ 216  │ self.op_transition(self.idle_state()); │ self.op_transition(OperationState::Idle); │
 └──────┴────────────────────────────────────────┴───────────────────────────────────────────┘

 Step 8 — Migrate sites in engine/src/app/plan.rs

 Take pattern (1 site — lines 447-454)

 Replace:
 let idle = self.idle_state();
 let state = match std::mem::replace(&mut self.core.state, idle) {
 With:
 let (taken, token) = self.core.operation.take();
 let state = match taken {
 (non-matching arms use self.op_restore(token, other); return;)

 For op_transition_from calls (plan.rs lines 532, 563):
     // OLD: self.op_transition_from(OperationTag::PlanApproval, next)
     // NEW: self.op_transition_from(token, next)

 Step 9 — Migrate sites in engine/src/app/commands.rs

 Read pattern (1 site)

 Line: 607
 Old: let streaming = matches!(self.core.state, OperationState::Streaming(_));
 New: let streaming = matches!(self.core.operation.state(), OperationState::Streaming(_));

 replace_with_idle (2 sites — now token-based)

 Line 343 (`cancel_active_operation`):
 Old: match self.replace_with_idle() {
 New:
 let (state, token) = self.core.operation.take();
 match state {

 Every match arm must consume the token:
 - Arms that do cleanup and leave machine Idle:
   self.core.operation.restore(token, OperationState::Idle); then call cleanup functions
 - RecoveryBlocked arm: self.op_restore(token, OperationState::RecoveryBlocked(state))
 - Idle arm: self.core.operation.restore(token, OperationState::Idle);

 Line 430 (`/clear` command):
 Old: let state = self.replace_with_idle(); ... self.op_transition(self.idle_state());
 New:
 let (state, token) = self.core.operation.take();
 ... self.op_transition_from(token, OperationState::Idle);

 idle_state in transitions (1 site)

 ┌──────┬────────────────────────────────────────┬───────────────────────────────────────────┐
 │ Line │                  Old                   │                    New                    │
 ├──────┼────────────────────────────────────────┼───────────────────────────────────────────┤
 │ 521  │ self.op_transition(self.idle_state()); │ self.op_transition(OperationState::Idle); │
 └──────┴────────────────────────────────────────┴───────────────────────────────────────────┘

 Step 10 — Migrate sites in engine/src/app/persistence.rs

 Read patterns (2 sites)

 ┌──────┬─────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────┐
 │ Line │                         Old                         │                               New                               │
 ├──────┼─────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────┤
 │ 287  │ if !matches!(self.core.state, OperationState::Idle) │ if !matches!(self.core.operation.state(), OperationState::Idle) │
 ├──────┼─────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────┤
 │ 424  │ if matches!(self.core.state, OperationState::Idle)  │ if matches!(self.core.operation.state(), OperationState::Idle)  │
 └──────┴─────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────┘

 idle_state in transitions (1 site)

 ┌──────┬────────────────────────────────────────┬───────────────────────────────────────────┐
 │ Line │                  Old                   │                    New                    │
 ├──────┼────────────────────────────────────────┼───────────────────────────────────────────┤
 │ 425  │ self.op_transition(self.idle_state()); │ self.op_transition(OperationState::Idle); │
 └──────┴────────────────────────────────────────┴───────────────────────────────────────────┘

 Step 11 — Migrate sites in engine/src/app/input_modes.rs

 Read patterns (2 sites)

 ┌──────┬─────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────┐
 │ Line │                           Old                           │                                 New                                 │
 ├──────┼─────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────┤
 │ 111  │ if !matches!(self.app.core.state, OperationState::Idle) │ if !matches!(self.app.core.operation.state(), OperationState::Idle) │
 ├──────┼─────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────┤
 │ 126  │ !matches!(self.app.core.state, OperationState::Idle)    │ !matches!(self.app.core.operation.state(), OperationState::Idle)    │
 └──────┴─────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────┘

 Step 12 — Migrate sites in engine/src/app/tests.rs

 Direct assignments (6 sites — used for test setup)

 Lines 130, 1015, 1054, 1105, 1141, 1677: app.core.state = OperationState::X(...) → app.core.operation.set_state(OperationState::X(...))

 Read matches with matches! (11 sites)

 Lines 240, 1241, 1503, 1690, 1719, 1778, 1789, 1803, 1837, 1878, 1893:
 matches!(app.core.state, X) → matches!(app.core.operation.state(), X)

 Read match destructuring (3 sites)

 Lines 243, 1164, 1489, 1586:
 match app.core.state or match &app.core.state → match app.core.operation.state()
 (For match app.core.state { ... } that consumed by value, prefer ref-matching via
 match app.core.operation.state() { ... }. If ownership is truly needed, use
 let (state, _token) = app.core.operation.take(); match state { ... } — but note
 the machine is left in zombie state until set_state is called again.)

 Mutable interior access (1 site)

 Line 2146: if let OperationState::Streaming(ref mut active) = app.core.state → if let Some(active) = app.core.operation.streaming_mut()

 Tag access (1 site)

 Line 2168: app.core.state.tag() → app.core.operation.tag()

 Assert with state (1 site)

 Line 2166: matches!(app.core.state, OperationState::Idle) → matches!(app.core.operation.state(), OperationState::Idle)

 Guardrail tests

 self_state_assignment_only_in_authorized_locations (line 2224):

 The old needle ["self", ".core.state", " ="] scanned app/*.rs.
 After this refactor, self.core.state doesn't exist — it's self.core.operation (an OperationMachine with private state). The compiler now enforces what the
  grep used to.

 Update the test to:
 1. Scan core/operation/machine.rs for self.state = — expected 4 sites:
    - 3 production: `transition` (`self.state = Some(next)`), `transition_from` (`self.state = Some(next)`),
      `restore` (`self.state = Some(next)`)
    - 1 test-only: `set_state` (`self.state = Some(state)`) (`#[cfg(test)]`)
    - Note: `take()` uses `self.state.take()` (method call, not direct assignment)
 2. Scan app/*.rs for the old needle — expected 0 everywhere (the field path no longer compiles).

 #[test]
 fn state_mutation_only_in_operation_machine() {
     let machine_src = include_str!("../../core/operation/machine.rs");
     let needle = ["self", ".state", " ="].concat();
     let count = machine_src
         .lines()
         .filter(|line| {
             let trimmed = line.trim();
             !trimmed.starts_with("//") && !trimmed.starts_with("///") && !trimmed.starts_with('*')
                 && trimmed.contains(&needle)
         })
         .count();
     assert_eq!(
         count, 4,
         "machine.rs: expected 4 state-assignment sites \
          (transition, transition_from, restore, set_state[test]), found {count}"
     );
 }

 replace_with_idle_usage_baseline (line 2274):

 replace_with_idle no longer exists. Replace with operation_take_usage_baseline:

 #[test]
 fn operation_take_usage_baseline() {
     let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
     let needle = ["operation", ".take("].concat();

     for entry in std::fs::read_dir(&app_dir).expect("read app dir") {
         let entry = entry.expect("dir entry");
         let path = entry.path();
         if path.extension().is_none_or(|ext| ext != "rs") {
             continue;
         }
         let filename = path.file_name().unwrap().to_str().unwrap();
         let source = std::fs::read_to_string(&path).expect("read source file");
         let count = source.matches(&*needle).count();

         match filename {
             "tool_loop.rs" => {
                 assert_eq!(count, 3, "tool_loop.rs: expected 3 operation.take(), found {count}");
             }
             "streaming.rs" => {
                 assert_eq!(count, 3, "streaming.rs: expected 3 operation.take(), found {count}");
             }
             "distillation.rs" => {
                 assert_eq!(count, 1, "distillation.rs: expected 1 operation.take(), found {count}");
             }
             "plan.rs" => {
                 assert_eq!(count, 1, "plan.rs: expected 1 operation.take(), found {count}");
             }
             "commands.rs" => {
                 assert_eq!(count, 2, "commands.rs: expected 2 operation.take(), found {count}");
             }
             _ => {
                 assert_eq!(count, 0, "{filename}: expected 0 operation.take(), found {count}");
             }
         }
     }
 }

 Transition edge tests (lines 2004-2093):

 Update App::op_transition_edge(...) → crate::core::operation::transition_edge(...)
 Update App::op_is_legal_transition(...) → crate::core::operation::is_legal(...)

 Step 13 — Compile and fix fallout

 just fix
 just verify

 Common fallout:
 - Missing imports for OperationState::Idle in files that used to get it through idle_state().
 - Tests that consumed app.core.state by value in match — need match app.core.operation.take() instead.
 - Unused `TakeToken` warnings — every code path must consume the token exactly once.
 - `TakeToken` import needed in files that call `take()`/`restore()`/`transition_from()`:
   use crate::core::operation::TakeToken;

 Step 14 — Structural checks

 # No direct state field access should remain in app/
 rg "self\.core\.state" engine/src/app/
 # Expected: zero matches

 # Machine field should be private
 rg "pub.*state:" engine/src/core/operation/machine.rs
 # Expected: zero matches

 # State writes only in machine.rs (Option-based)
 rg "self\.state\s*=" engine/src/core/operation/machine.rs
 # Expected: 4 sites (transition, transition_from, restore, set_state[test])

 # No state_mut method exists
 rg "fn state_mut" engine/src/core/operation/
 # Expected: zero matches

 # idle_state and replace_with_idle are gone
 rg "idle_state\|replace_with_idle" engine/src/app/
 # Expected: zero matches

 # TakeToken consumed correctly (no unused token warnings)
 # Compiler enforces this via move semantics — unused TakeToken is a compile error
 cargo build 2>&1 | rg "unused.*TakeToken"
 # Expected: zero matches

 # TakeToken is not Clone or Copy
 rg "derive.*Clone.*Copy|derive.*Copy.*Clone" engine/src/core/operation/machine.rs
 # Expected: only on types that should be cloneable (not TakeToken, not TransitionReceipt)

 Step 15 — Commit

 git add engine/src/core/ engine/src/app/
 git commit -m "refactor(engine): extract OperationMachine into core::operation

 Move transition legality graph and state mutation authority into a
 dedicated OperationMachine. Private state field makes unauthorized
 writes a compile error (IFA §2.1), replacing grep-based guardrail
 with structural enforcement."

 Out of scope for Phase 2

 - Converting callers from constructing OperationState variants to event-based API
 - Moving OperationState/OperationTag/OperationEdge types out of state.rs
 - Capability tokens for state access (IFA §10)
 - Correct `from` tag semantics in `commit_tool_batch` (currently fires `Idle→Idle`
   instead of `ToolLoop→Idle` — the TakeToken proves the real origin, but callsites
   that restore before calling commit_tool_batch lose the semantic edge)

 Those are Phase 3+.

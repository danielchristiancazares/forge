# IFA Full Remediation Plan

## Objective

Bring the codebase to strict conformance with `docs/IFA_CONFORMANCE_RULES.md` with no partial measures, no temporary compatibility debt in Core, and no stale Section 17 artifacts.

## Non-Negotiable Constraints

1. Every module is classified as either Core or Boundary.
2. Core interfaces and Core domain structs contain no `Option<T>`.
3. Core lifecycle is encoded with typestate or data-carrying enum variants, not bool flags.
4. Proof and controlled types are unforgeable through safe APIs.
5. Boundary performs conversion once; Core assumes always.
6. Section 17 artifacts are accurate and validated against real symbols.
7. CI rejects regressions on all rules above.

## Program Structure

## Phase 0: Guardrails Before Refactor

### Goals

1. Prevent additional drift while refactor is in progress.
2. Make conformance check executable and enforceable in CI.

### Work

1. Add `ifa/classification_map.toml` with full module classification (Core or Boundary).
2. Extend `scripts/ifa_conformance_check.py` to:
3. Validate symbol existence for paths in all `ifa/*.toml` files.
4. Validate classification map completeness.
5. Enforce Core bans:
6. `Option<` in interfaces/domain structs.
7. Lifecycle bool fields.
8. Domain `Unknown`, `Default`, `Empty`, `None` variants.
9. Forgeable proof surfaces (public fields/constructors bypassing validation).
10. Wire checks into `just ifa-check` and therefore `just verify`.
11. Document enforcement and expected update process in `ifa/README.md`.

### Deliverables

1. `ifa/classification_map.toml` (new).
2. Updated `scripts/ifa_conformance_check.py`.
3. Updated `ifa/README.md`.

### Exit Criteria

1. CI fails on stale artifact paths and banned Core patterns.
2. CI passes with current code only after explicit documented exceptions are removed or resolved.

## Phase 1: Core/Boundary Decomposition

### Goals

1. Eliminate mixed-concern modules.
2. Make architectural boundaries physically visible.

### Work

1. Move boundary behavior out of Core paths:
2. `core/src/environment.rs` (clock/fs/env) to a Boundary module.
3. `core/src/errors.rs` raw diagnostic parsing and JSON string probing to Boundary.
4. `core/src/util.rs` fallback parsing APIs returning maybe values to Boundary.
5. Split `types` internals into:
6. Core proof/domain modules.
7. Boundary transport/wire modules (serde fallback and wire compatibility).
8. Remove non-conformant re-exports from Core surfaces.
9. Update call sites in `engine`, `tui`, and `cli` to import from correct layer.

### Deliverables

1. Module relocation commits with imports updated.
2. Updated architecture docs describing module placement rules.

### Exit Criteria

1. Core no longer performs boundary concerns.
2. All IO/env/time/diagnostic parsing code is Boundary-only.

## Phase 2: Plan Domain Rewrite (No Escape Hatches)

### Goals

1. Make plan invariants unforgeable.
2. Replace mutable-in-place lifecycle mutation with consuming transitions.

### Work

1. Remove forgeable public construction and mutation:
2. Privatize `PlanStepId`, `PlanStep`, `Phase` internals.
3. Remove `phases_mut` and `plan_mut`.
4. Replace `PlanStep::transition(&mut self, ...)` with consuming typestate transitions.
5. Introduce typestate model:
6. `PendingStep`, `ActiveStep`, terminal variants.
7. Enforce legal transitions by type, not runtime checks.
8. Replace rollback-prone edit behavior:
9. `Plan + EditCommand -> Result<Plan, EditError>` (pure transform).
10. Remove mutate-then-revert path.
11. Ensure step outcomes/reasons use proof types (`NonEmptyString` or equivalent).

### Deliverables

1. New plan state machine types.
2. Removed mutable escape hatch APIs.
3. Updated engine plan orchestration adapters.

### Exit Criteria

1. Invalid plan graph cannot be constructed through safe APIs.
2. Illegal lifecycle transition is unrepresentable at call site.

## Phase 3: Remove `Option` from Core Interfaces

### Goals

1. Satisfy deterministic bans for maybe-valid Core interfaces.

### Work

1. Replace `Option` returns with explicit domain outcomes:
2. `CacheBudget::take_one` to explicit enum (`Remaining` vs `Exhausted`).
3. `Plan::try_complete` to explicit completion type.
4. `PlanState::plan()` style maybe APIs to variant-specific access enums.
5. Move transport-only optional fields out of Core domain objects into Boundary DTOs.
6. Audit and remove `Option` from Core structs and public Core methods.

### Deliverables

1. Core API signatures without `Option`.
2. Boundary adapters mapping optional wire inputs to typed outcomes.

### Exit Criteria

1. Zero `Option<` in Core interfaces and Core domain structs.
2. Remaining `Option` usage exists only in Boundary modules.

## Phase 4: Remove Lifecycle Bools and Tag-Field State

### Goals

1. Eliminate primitive lifecycle encoding.

### Work

1. Replace `ToolResult { is_error: bool }` with enum variants:
2. `ToolResult::Success { ... }`
3. `ToolResult::Error { ... }`
4. Replace `ViewState` tag-plus-shared-field model:
5. From `view_mode` + `focus_state`
6. To `enum ViewState { Classic(ClassicState), Focus(FocusStateData) }`
7. Refactor modal/input state bools that alter valid operations into explicit variants.
8. Keep non-lifecycle feature toggles in Boundary and document rationale.

### Deliverables

1. New domain enums replacing bool lifecycle flags.
2. Updated UI/engine integration paths.

### Exit Criteria

1. No lifecycle bools in Core.
2. No tag fields that reinterpret shared fields by state.

## Phase 5: Authority Boundary Hardening

### Goals

1. Ensure proof types are not forgeable.
2. Ensure constructors are visibility-minimized and validated.

### Work

1. Audit all proof/control types:
2. `NonEmptyString`, `PersistableContent`, `ModelName`, plan proofs, replay proofs.
3. Remove public bypass constructors.
4. Replace unsafe or unchecked construction patterns with authority-only constructors.
5. For serde-sensitive proof types, use validated deserialization (`try_from` or custom impls).
6. Remove `Default` derives that bypass invariant establishment on proof objects.

### Deliverables

1. Hardened constructors and visibility updates.
2. Updated authority map entries tied to real constructors.

### Exit Criteria

1. Invalid proofs cannot be created via safe API outside authority boundary.

## Phase 6: Boundary Conversion Discipline in Engine

### Goals

1. Make invalid input rejection a boundary concern only.
2. Make Core operations validity-infallible.

### Work

1. Refactor plan tool pipeline (`engine/src/app/plan.rs`):
2. Parse JSON arguments into Boundary DTOs.
3. Convert DTOs into proof-carrying Core commands once.
4. Invoke Core without shape/range/state guard checks in Core.
5. Remove duplicate validation currently scattered in handlers.
6. Ensure errors crossing outward are boundary-defined representations.

### Deliverables

1. Clear DTO-to-proof conversion layer.
2. Slimmed Core invocation paths.

### Exit Criteria

1. Core no longer fails for invalid raw inputs.
2. Boundary handles all invalid input with typed outcomes.

## Phase 7: Section 17 Artifact Rewrite and Validation

### Goals

1. Make artifacts true to code, not aspirational.

### Work

1. Rewrite all five required artifacts:
2. `ifa/invariant_registry.toml`
3. `ifa/authority_boundary_map.toml`
4. `ifa/parametricity_rules.toml`
5. `ifa/move_semantics_rules.toml`
6. `ifa/dry_proof_map.toml`
7. Remove stale paths and update canonical proof paths to real symbols.
8. Add new invariants for rewritten plan typestate and tool result sum types.
9. Enforce cross-artifact consistency and symbol existence in checker.

### Deliverables

1. Accurate Section 17 artifact set.
2. Checker that fails on path drift and mismatch.

### Exit Criteria

1. Artifacts are complete, current, and machine-validated.

## Phase 8: Test, Coverage, and Documentation Closure

### Goals

1. Ship conformance without regression.

### Work

1. Update unit and integration tests impacted by API and type changes.
2. Add compile-fail tests for forgeability and constructor visibility boundaries.
3. Add property tests for typestate transitions and non-forking guarantees.
4. Run `cargo cov`; ensure no coverage decrease.
5. Update docs for public API and architecture:
6. `docs/` conformance notes.
7. crate READMEs for changed APIs.
8. Keep workflow discipline:
9. `just fix` after edits.
10. `just verify` after changes.

### Deliverables

1. Passing verification and coverage checks.
2. Updated public docs and architecture references.

### Exit Criteria

1. `just verify` passes.
2. Coverage is non-decreasing.
3. No compatibility shims that re-introduce banned Core patterns.

## Cross-Phase Workstreams

### Workstream A: API Migration

1. Provide migration adapters only at Boundary.
2. Remove adapters once call sites migrate.
3. Do not preserve non-conformant Core APIs.

### Workstream B: Tooling and CI

1. Expand lint and grep-based bans with context-aware checks.
2. Enforce artifact correctness and module classification continuously.

### Workstream C: Documentation

1. Maintain an explicit conformance status table by rule.
2. Record any temporary non-conformance with owner and removal date.
3. Do not mark full conformance until all temporary items are gone.

## Milestones and Gates

1. Gate 1: Guardrails complete (Phase 0).
2. Gate 2: Structural split complete (Phase 1).
3. Gate 3: Plan typestate rewrite merged (Phase 2).
4. Gate 4: Core maybe/lifecycle bans cleared (Phases 3-4).
5. Gate 5: Authority hardening and boundary conversion complete (Phases 5-6).
6. Gate 6: Artifacts truthful and enforced (Phase 7).
7. Gate 7: Tests, coverage, docs complete (Phase 8).

## Definition of Done

1. No open violations against `docs/IFA_CONFORMANCE_RULES.md`.
2. Core-only ban checks pass with zero suppressions.
3. All Section 17 artifacts are accurate and symbol-validated.
4. Public documentation matches implemented architecture.
5. CI enforces these conditions for all future changes.


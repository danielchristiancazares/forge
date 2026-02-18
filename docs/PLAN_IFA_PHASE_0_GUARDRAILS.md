# PLAN_IFA_PHASE_0_GUARDRAILS

## Purpose

Create safety rails and baseline checks before structural changes.

## Drivers

- Enforce a single transition authority boundary.
- Prevent regression toward ad hoc `mem::replace` state handling.
- Make illegal transitions and missing proofs detectable in tests.

## Scope

- Add transition legality tests for operation edges.
- Add capability-proof tests for tool execution and commit paths.
- Add static guardrails to catch forbidden patterns once later phases land.

## Tasks

1. Add operation transition matrix tests keyed by `OperationTag` and `OperationEdge`.
2. Add tests that prove:
   - tools cannot execute without tooling-enabled authority.
   - tool commit cannot happen without persistence proof capability.
3. Add a guardrail test/check that rejects:
   - direct reintroduction of `replace_with_idle`.
   - direct `self.state = ...` mutation outside operation boundary.

## Candidate files

- `engine/src/app/tests.rs`
- `engine/src/core/operation/tests.rs` (new)

## Exit criteria

- Transition legality is explicit and tested.
- Capability requirements are test enforced.
- Guardrails fail fast when centralization is bypassed.

## Validation

- `just fix`
- `just verify`

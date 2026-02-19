# IFA Operational Artifacts

This directory contains the Section 13 operational artifacts required by `docs/IFA_CONFORMANCE_RULES.md` (IFA-R46):

1. `invariant_registry.toml`
2. `authority_boundary_map.toml`
3. `parametricity_rules.toml`
4. `move_semantics_rules.toml`
5. `dry_proof_map.toml`

It also includes the phase-0 guardrail artifact:

6. `classification_map.toml` (module-level Core/Boundary ownership map)

The CI gate is `just ifa-check` (also included in `just verify`). The checker validates:

1. Required artifact presence and schema.
2. Cross-artifact ID consistency.
3. Symbol/module path existence for recorded authority/proof references.
4. Classification-map coverage for all `src/**/*.rs` workspace modules.
5. Core deterministic bans (`Option<`, lifecycle `bool`, banned enum variants).
6. Controlled-type struct unforgeability (no public fields).
7. Constructor visibility-rung enforcement (see below).

### Constructor visibility rungs

Each `authority_boundary_map.toml` entry declares `max_constructor_visibility_rung`, the most permissive Rust visibility the listed constructors are allowed to have. The checker resolves the actual visibility of every constructor path (inherent methods, enum variant declarations, and trait impl methods) and fails if the actual visibility exceeds the declared rung.

Rung ordering (least to most permissive):

    private < pub(super) < pub(crate) < pub

Use `private` for constructors reachable only within the defining module. Use `pub(crate)` for same-crate callers. Use `pub` when the controlled type lives in a library crate (e.g. `types`, `core`) and callers are in downstream crates (e.g. `engine`, `config`).

When adding or changing constructor paths, update `max_constructor_visibility_rung` to the tightest rung that still covers all declared `allowed_caller_module_paths`.

Any change to proof types, authority boundaries, transition ownership, or parametric behavior must update these files in the same change.

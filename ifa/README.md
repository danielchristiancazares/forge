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

Any change to proof types, authority boundaries, transition ownership, or parametric behavior must update these files in the same change.

# Enterprise Provider Routing Plan (Bedrock + Vertex)

## Context

Forge currently supports direct provider integrations (Claude, OpenAI, Gemini). Enterprise environments often require cloud-identity routing through AWS Bedrock or GCP Vertex AI. This plan adds deterministic provider routing that remains fully constrained by `DESIGN.md`: boundary uncertainty stays at boundaries, core execution stays inhabited and policy-free.

## Contract With `DESIGN.md`

This plan is valid only if these rules hold:

1. Invalid states are unrepresentable in core execution paths.
2. `Result`/`Option` exist only in boundary parsing and boundary adaptation layers.
3. Typestate transitions consume precursor values and return proof-carrying next-state values.
4. Routing policy chooses targets; provider mechanism only executes targets.
5. No fallback decisions are hidden inside provider modules.

Clarification on proof semantics:

Compilation cannot prove external cloud credential validity. Compilation can prove that provider execution entry points require capability-bearing values that can only be constructed by successful boundary validation.

## Non-Goals (v1)

1. Weighted routing, latency-aware routing, or cost-based routing.
2. Automatic cross-cloud failover after transport has already begun.
3. Full workload identity federation support on day one.
4. Runtime mutation of routing policy after session initialization.

## Routing Boundary Model

### Boundary Inputs

1. Raw config values from `config::ForgeConfig`.
2. Runtime environment evidence (AWS env/profile/metadata; GCP service-account file/metadata).
3. Provider metadata strings (region, project, endpoint/model path) as raw text.

### Boundary Outputs

The boundary collapses uncertainty and emits either strict capabilities or explicit route failures:

```rust
pub(crate) enum InferenceTarget {
    Bedrock(BedrockCapability),
    Vertex(VertexCapability),
}

pub(crate) struct RouteResolutionFailure {
    attempts: Vec<RouteAttemptFailure>,
}
```

`InferenceTarget` must not include `Unconfigured` or `Unknown` variants. Unconfigured states fail at the boundary before execution.

### Deterministic Selection Algorithm

Given `ProviderRoutePolicy`, selection is deterministic and testable:

1. Expand policy to ordered candidates (`[Bedrock, Vertex]` or `[Vertex, Bedrock]`).
2. For each candidate, call the provider-specific boundary constructor.
3. Return first successful capability as `InferenceTarget`.
4. If all candidates fail, return `RouteResolutionFailure` with per-attempt stable failure codes.

No provider module is allowed to re-run this algorithm.

## Provider Typestate Contracts

### AWS Bedrock Boundary

Strict types:

```rust
pub(crate) enum AwsCredentialMaterial {
    AccessKeyPair {
        access_key_id: AccessKeyId,
        secret_access_key: SecretAccessKey,
    },
    SessionKey {
        access_key_id: AccessKeyId,
        secret_access_key: SecretAccessKey,
        session_token: SessionToken,
    },
}

pub(crate) struct BedrockCapability {
    signer: SigV4Signer,
    endpoint: BedrockEndpoint,
    region: AwsRegion,
    _proof: (),
}
```

Boundary steps:

1. Resolve credential source from explicit ordered sources.
2. Parse and validate into `AwsCredentialMaterial` without optional fields.
3. Build signer and bind validated target metadata.
4. Construct `BedrockCapability` with private constructor.

Guarantee:

Execution never branches on missing AWS key fields, missing region, or missing endpoint.

### GCP Vertex Boundary

Strict types:

```rust
pub(crate) enum VertexIdentityMaterial {
    ServiceAccount(ServiceAccountMaterial),
    MetadataIdentity(MetadataIdentityMaterial),
}

pub(crate) struct VertexCapability {
    token_provider: VertexTokenProvider,
    project: GcpProjectId,
    location: GcpLocation,
    model_path: VertexModelPath,
    _proof: (),
}
```

Boundary steps:

1. Resolve identity source from explicit ordered sources.
2. Parse and validate into `VertexIdentityMaterial`.
3. Build token provider and bind validated target metadata.
4. Construct `VertexCapability` with private constructor.

Guarantee:

Execution never branches on which identity source succeeded.

## Failure Model (Stable Codes)

Routing failures should be concrete and auditable, similar to typed violation models in other plans.

```rust
pub(crate) enum RouteFailureCode {
    AwsCredentialSourceUnavailable,
    AwsCredentialMalformed,
    AwsRegionInvalid,
    BedrockEndpointInvalid,
    BedrockSignerUnavailable,
    GcpIdentitySourceUnavailable,
    GcpIdentityMalformed,
    VertexProjectInvalid,
    VertexLocationInvalid,
    VertexModelPathInvalid,
    VertexTokenUnavailable,
    NoRoutableProvider,
}
```

Rules:

1. Every failed candidate route yields one stable code.
2. Policy fallback behavior is explicit from candidate ordering, never inferred from code paths.
3. Logs and telemetry record code and provider identity, not raw secrets.

## Config Contract (v1)

Proposed schema:

```toml
[enterprise_routing]
policy = "prefer_bedrock_then_vertex"  # prefer_bedrock_then_vertex | prefer_vertex_then_bedrock | force_bedrock | force_vertex

[enterprise_routing.bedrock]
enabled = true
region = "us-east-1"
endpoint = "https://bedrock-runtime.us-east-1.amazonaws.com"
model_id = "anthropic.claude-3-7-sonnet-20250219-v1:0"

[enterprise_routing.vertex]
enabled = true
project = "my-project"
location = "us-central1"
publisher = "anthropic"
model = "claude-sonnet-4"
```

Validation at config boundary:

1. `force_*` policies require that target provider section is enabled.
2. Region/location and model identifiers must parse into strict newtypes.
3. Unknown policy strings fail fast during config load.

## Mechanism vs Policy Boundaries

Module ownership:

1. `core/src/environment.rs`: boundary resolution and capability construction.
2. `engine/src/app/streaming.rs`: route policy evaluation and target selection.
3. `providers/src/bedrock.rs` and `providers/src/vertex.rs`: request mechanism only, no fallback or provider switching.

Enforcement:

1. Capability constructors are private to boundary modules.
2. Provider execution functions accept capability-bearing targets only.
3. Policy enum and selection function are the single route decision authority.

## Planned File Changes

| File | Planned Change |
|------|----------------|
| `types/src/model.rs` | Add Bedrock/Vertex model/provider surface and strict route-facing types |
| `config/src/lib.rs` | Add enterprise routing schema and parse-time validation for policy + target metadata |
| `core/src/environment.rs` | Add boundary credential/identity discovery and strict capability construction hooks |
| `engine/src/app/streaming.rs` | Add deterministic route selection before provider dispatch |
| `providers/src/lib.rs` | Extend dispatch interfaces to accept capability-backed `InferenceTarget` values |
| `providers/src/bedrock.rs` (new) | Bedrock transport mechanism that requires `BedrockCapability` |
| `providers/src/vertex.rs` (new) | Vertex transport mechanism that requires `VertexCapability` |
| `docs/` | Add enterprise setup and failure-code troubleshooting docs |
| `ifa/*.toml` | Update invariants, authority boundaries, and proof maps for new capability flow |

## Implementation Phases

## Phase 1: Domain Types and Capability Sealing

Deliverables:

1. Add strict credential/identity material types without `Option` fields.
2. Add `BedrockCapability` and `VertexCapability` with private constructors and proof fields.
3. Add `InferenceTarget` variants and require them in provider execution signatures.

Exit criteria:

1. Bedrock/Vertex execution paths are uncallable without capabilities.
2. Compile-time checks fail when attempting to construct capabilities outside boundary module.

Risk: LOW to MEDIUM (type migration breadth).

## Phase 2: Boundary Builders and Failure Codes

Deliverables:

1. Implement AWS boundary parsing and signer construction.
2. Implement GCP boundary parsing and token-provider construction.
3. Implement stable `RouteFailureCode` mapping.

Exit criteria:

1. Boundary constructors fully collapse uncertainty before core routing receives values.
2. Failure paths surface stable codes with no secret leakage.

Risk: MEDIUM (credential-source variability).

## Phase 3: Policy Integration

Deliverables:

1. Implement deterministic policy ordering and candidate evaluation.
2. Emit first-success target or `RouteResolutionFailure` with attempt details.
3. Enforce no fallback behavior in provider mechanism modules.

Exit criteria:

1. Route decisions are unit-testable in isolation.
2. Provider modules cannot select or switch provider targets.

Risk: MEDIUM (behavioral changes in startup routing path).

## Phase 4: Runtime Adoption, Docs, and IFA

Deliverables:

1. Integrate route selection into streaming request flow.
2. Add user-facing documentation for enterprise config and failure diagnostics.
3. Update IFA artifacts when authority boundaries or invariants change.

Exit criteria:

1. End-to-end inference uses capability-backed targets only.
2. Docs and IFA artifacts match final behavior.

Risk: LOW to MEDIUM (docs and artifact completeness).

## Testing Strategy

Unit tests:

1. Typestate transitions consume precursor values and prevent reuse.
2. Capability constructors are not accessible outside boundary modules.
3. Policy ordering returns deterministic first-success targets.
4. Each boundary failure maps to one stable `RouteFailureCode`.

Integration tests:

1. Bedrock route succeeds with valid AWS material and valid target metadata.
2. Vertex route succeeds with valid GCP identity and valid target metadata.
3. `force_bedrock` and `force_vertex` fail explicitly when capability construction fails.
4. Preference policies fall through only via explicit boundary failure results.

Security/robustness tests:

1. Malformed service account JSON is rejected at boundary.
2. Partial or malformed AWS material is rejected at boundary.
3. Diagnostics redact credentials, tokens, and private keys.

Validation commands:

1. `just fix`
2. `just verify`

## Operational Guidance

1. Default policy remains current direct-provider behavior unless enterprise routing is explicitly enabled.
2. Enterprise failures must surface actionable reason codes and remediation text.
3. Telemetry should capture provider candidate order and failure codes, not secret-bearing payloads.

## Success Criteria

1. `InferenceTarget` contains only capability-bearing, executable variants.
2. Core execution has no provider-specific optionality checks for Bedrock/Vertex.
3. Routing policy is deterministic, isolated, and independently testable.
4. Provider mechanism code contains no hidden fallback logic.
5. IFA and docs are updated when capability boundaries or authority maps change.

## Open Questions

1. Should workload identity federation be in v1 or a follow-on phase?
2. Should direct-provider and enterprise-provider routing share one policy enum or remain separately staged initially?

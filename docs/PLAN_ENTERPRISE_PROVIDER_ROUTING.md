# Enterprise Provider Routing Plan (Bedrock + Vertex)

## Audit Outcome Against `DESIGN.md`

This revision corrects the following inaccuracies from the previous draft:

1. **Boundary ownership was misplaced**: `core/src/environment.rs` is an OS/environment facts boundary and should not become cloud transport credential orchestration.
2. **Provider/domain surface was overloaded**: adding Bedrock/Vertex to `types/src/model.rs` would conflate model provider identity with transport backend identity.
3. **Current queue-time API-key gate was not addressed**: `engine/src/app/input_modes.rs` currently blocks sends before streaming if direct API keys are absent.
4. **Config optionality leaked into core shape**: `enabled` booleans in the runtime contract conflict with `DESIGN.md` direction for inhabited core types.

## Contract With `DESIGN.md`

This plan is valid only if all of the following remain true:

1. Invalid states are unrepresentable in core execution paths.
2. `Option`/`Result` stay in boundary parse/adaptation layers; core execution consumes strict types.
3. Typestate transitions consume precursor values and return proof-carrying next-state values.
4. Routing policy chooses targets; provider mechanism only executes chosen targets.
5. No fallback or route switching is hidden inside provider modules.

Compilation cannot prove external cloud credential validity. It **can** prove that execution entry points require capability-bearing values only constructible by successful boundary validation.

## Non-Goals (v1)

1. Weighted/latency/cost-based dynamic routing.
2. Mid-stream cross-cloud failover after transport has started.
3. Full workload identity federation.
4. Runtime mutation of routing policy after session initialization.

## Corrected Boundary Ownership

| Responsibility | Module Location |
|---|---|
| Raw config parsing and validation | `config/src/lib.rs` |
| Strict resolved routing config types | `types/src/settings.rs` |
| Route policy ordering + deterministic selection | `engine/src/app/provider_routing.rs` (new) |
| Queue-time preflight (direct key vs enterprise capability readiness) | `engine/src/app/input_modes.rs` |
| Transport mechanism implementations | `providers/src/bedrock.rs` and `providers/src/vertex.rs` (new) |
| Dispatch wiring | `providers/src/lib.rs` and `engine/src/app/streaming.rs` |

`core/src/environment.rs` remains focused on OS/runtime environment facts and AGENTS discovery.

## Routing Type Contracts (Concrete)

### Core Route Targets

```rust
pub(crate) enum InferenceTarget {
    Bedrock(BedrockCapability),
    Vertex(VertexCapability),
}

pub(crate) enum DispatchTarget {
    Direct(ApiConfig),
    Enterprise(InferenceTarget),
}
```

`InferenceTarget` and `DispatchTarget` must not include `Unknown`, `Disabled`, or `Unconfigured` variants.

### Deterministic Policy

```rust
pub(crate) enum ProviderRoutePolicy {
    PreferBedrockThenVertex,
    PreferVertexThenBedrock,
    ForceBedrock,
    ForceVertex,
}
```

Candidate order is derived only from this enum; provider modules must not recompute order.

### Non-Empty Failure Evidence

```rust
pub(crate) struct RouteResolutionFailure {
    first: RouteAttemptFailure,
    rest: Vec<RouteAttemptFailure>,
}
```

`RouteResolutionFailure` is structurally non-empty (no empty `attempts: Vec<_>` state).

## Provider Capability Contracts

### AWS Bedrock Boundary

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
    model_id: BedrockModelId,
    _proof: (),
}
```

Boundary steps:

1. Resolve source from explicit ordered credential sources.
2. Parse into strict `AwsCredentialMaterial` (no partial fields).
3. Build signer and validated endpoint/region/model identifiers.
4. Construct `BedrockCapability` via private constructor.

### GCP Vertex Boundary

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
2. Parse into strict `VertexIdentityMaterial`.
3. Build token provider and validated project/location/model path.
4. Construct `VertexCapability` via private constructor.

Execution paths branch only on target variant, never on missing fields.

## Deterministic Selection Algorithm

Given `ProviderRoutePolicy`:

1. Expand to ordered candidates.
2. Run candidate-specific boundary constructor for each candidate in order.
3. Return first success as `InferenceTarget`.
4. On total failure, return `RouteResolutionFailure` with stable per-attempt codes.

No provider mechanism module is allowed to run this algorithm.

## Failure Model (Stable Codes)

```rust
pub(crate) enum RouteFailureCode {
    AwsCredentialSourceUnavailable,
    AwsCredentialMalformed,
    AwsRegionInvalid,
    BedrockEndpointInvalid,
    BedrockModelInvalid,
    BedrockSignerUnavailable,
    GcpIdentitySourceUnavailable,
    GcpIdentityMalformed,
    VertexProjectInvalid,
    VertexLocationInvalid,
    VertexModelPathInvalid,
    VertexTokenUnavailable,
    ModelUnsupportedForTarget,
    NoRoutableProvider,
}
```

Rules:

1. Each failed candidate contributes exactly one stable `RouteFailureCode`.
2. Candidate ordering defines fallback behavior explicitly.
3. Logs/telemetry include candidate + failure code only (never raw secrets).

## Config Contract (v1)

Raw TOML boundary (example):

```toml
[enterprise_routing]
policy = "prefer_bedrock_then_vertex" # prefer_bedrock_then_vertex | prefer_vertex_then_bedrock | force_bedrock | force_vertex

[enterprise_routing.bedrock]
region = "us-east-1"
endpoint = "https://bedrock-runtime.us-east-1.amazonaws.com"
model_id = "<bedrock-model-id>"

[enterprise_routing.vertex]
project = "my-project"
location = "us-central1"
publisher = "anthropic"
model = "claude-sonnet-4"
```

Resolved runtime contract:

1. No `enabled` booleans in resolved route types.
2. Referenced targets in policy must be present and valid at parse boundary.
3. Unknown policy strings fail fast during config load.
4. Target metadata parses into strict newtypes before entering core routing.

## Direct vs Enterprise Preflight

Current behavior blocks queued messages when direct API keys are missing. v1 enterprise routing must make this explicit:

1. `DispatchTarget::Direct` requires direct provider API key.
2. `DispatchTarget::Enterprise` requires enterprise capability construction.
3. The queue path in `engine/src/app/input_modes.rs` must call route preflight and stop using direct API-key presence as the only send gate.
4. No silent fallback from enterprise failure to direct path unless policy explicitly says direct path is selected.

## Mechanism vs Policy Enforcement

1. Policy authority lives in one engine routing module (`provider_routing.rs`).
2. Provider modules (`bedrock.rs`, `vertex.rs`) accept capability-bearing input only.
3. Capability constructors are private to boundary modules.
4. Providers never inspect policy ordering or perform cross-provider retries.

## Planned File Changes (Corrected)

| File | Planned Change |
|---|---|
| `types/src/settings.rs` | Add strict enterprise routing config types and policy enums (resolved, inhabited runtime contract) |
| `config/src/lib.rs` | Add raw enterprise routing schema + parse boundary conversion into strict types |
| `engine/src/app/provider_routing.rs` (new) | Add deterministic candidate ordering, selection, and failure aggregation |
| `engine/src/app/input_modes.rs` | Replace direct API-key-only queue gate with route preflight gate |
| `engine/src/app/streaming.rs` | Dispatch via `DispatchTarget` (direct or enterprise) before provider send |
| `providers/src/lib.rs` | Add dispatch entrypoints for enterprise targets while preserving direct path |
| `providers/src/bedrock.rs` (new) | Bedrock transport mechanism requiring `BedrockCapability` |
| `providers/src/vertex.rs` (new) | Vertex transport mechanism requiring `VertexCapability` |
| `docs/` | Add enterprise setup + failure-code troubleshooting |
| `ifa/*.toml` | Update invariants, authority maps, and proof maps for new routing boundaries |

## Implementation Phases

### Phase 1: Routing Types and Sealed Capabilities

Deliverables:

1. Add strict routing policy/types in `types/src/settings.rs`.
2. Add capability structs with private constructors in provider boundary modules.
3. Add `DispatchTarget` and `InferenceTarget` as executable-only variants.

Exit criteria:

1. No execution entrypoint accepts raw cloud credential/identity text.
2. Capability creation is impossible outside boundary modules.

### Phase 2: Boundary Builders + Failure Codes

Deliverables:

1. Implement Bedrock boundary builder.
2. Implement Vertex boundary builder.
3. Implement stable code mapping into `RouteAttemptFailure`.

Exit criteria:

1. Boundary builders collapse uncertainty fully before selection returns.
2. Failure payloads are stable and redact secret-bearing details.

### Phase 3: Engine Policy Integration

Deliverables:

1. Add deterministic selection in `engine/src/app/provider_routing.rs`.
2. Integrate route preflight into queue path.
3. Integrate `DispatchTarget` into streaming send path.

Exit criteria:

1. Route decisions are unit-testable in isolation.
2. Queue gating supports enterprise-ready sessions without direct API keys.

### Phase 4: Docs + IFA + Rollout

Deliverables:

1. Add user-facing enterprise setup docs and remediation guidance for stable failure codes.
2. Update IFA artifacts for routing authority boundaries.
3. Add release notes describing direct-vs-enterprise dispatch behavior.

Exit criteria:

1. End-to-end enterprise dispatch works without direct API key dependency.
2. Docs/IFA artifacts match implementation.

## Testing Strategy

Unit tests:

1. Policy ordering deterministically expands candidate order.
2. `RouteResolutionFailure` cannot be empty.
3. Capability constructors are not visible outside their modules.
4. Each boundary failure maps to one stable `RouteFailureCode`.

Integration tests:

1. Bedrock dispatch succeeds with valid boundary material.
2. Vertex dispatch succeeds with valid boundary material.
3. `force_bedrock` / `force_vertex` fail explicitly with stable codes when capability build fails.
4. Preference policies fall through only by explicit boundary failure outputs.
5. Queue preflight allows enterprise dispatch with no direct API key when enterprise capability succeeds.

Security/robustness tests:

1. Malformed AWS/GCP material is rejected at boundary.
2. Diagnostics/telemetry redact credentials, tokens, and private keys.
3. Provider modules do not emit fallback decisions.

Validation commands:

1. `just fix`
2. `just verify`

## Operational Guidance

1. Default behavior remains current direct-provider routing unless enterprise routing is configured.
2. Enterprise failures must include actionable failure codes and remediation text.
3. Telemetry captures candidate order + failure codes only (no secrets).

## Success Criteria

1. Execution receives only inhabited dispatch targets (`Direct` or capability-backed enterprise target).
2. Policy decisions are centralized and deterministic.
3. Provider mechanisms are policy-free.
4. Queue/start-stream paths no longer assume direct API keys are always required.
5. IFA + docs stay in lockstep with authority boundary changes.

## Open Questions

1. Should v1 restrict enterprise routing to a model allowlist per target, or support all model families immediately?
2. Should direct and enterprise dispatch share one top-level enum from day one, or stage enterprise dispatch behind a temporary adapter?

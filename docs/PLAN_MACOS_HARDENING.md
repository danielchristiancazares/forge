# macOS Hardening Plan

## Context

Forge ships onto user-managed macOS systems. Hardening must be enforced by structure, not convention. This plan aligns macOS hardening with `DESIGN.md`: invalid states are unrepresentable, boundary parsing collapses uncertainty immediately, and policy decisions stay with callers.

Five phases of increasing depth. Phases 1-3 are implementable now. Phases 3.5 and 4 are design-only.

---

## Contract With DESIGN.md

This plan follows these non-negotiable rules:

1. No mechanism-owned fallback policy. Mechanisms report facts; callers choose behavior.
2. No "remember to harden" conventions. Spawn APIs must make unhardened process launch impossible in core paths.
3. No loose core tuples for security state. Use named sum/product types that carry proofs.
4. No erased security state. Sandboxed vs unsandboxed execution must remain a type-level distinction until the final spawn decision.
5. Boundary ingestion must normalize inherited process state (environment, file descriptors, and limits) before child launch.
6. Launch identity must be proof-carrying. Resolved executable paths are insufficient without stable identity checks.
7. Typestate guarantees must be compile-time tested, not only behaviorally tested.
8. Capability grants must be proof tokens with private constructors; no free-form capability enums at spawn call sites.
9. Mechanism-specific unavailability reasons must remain distinct until the final spawn decision type.
10. Environment toggles and config are caller policy inputs, never mechanism-owned bypass decisions.

---

## Phase 1: Secret Memory Safety

**Problem:** `SecretString` does not zeroize heap bytes on drop.

**Change:** Add `zeroize` and enforce cleanup in `Drop`.

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `zeroize = "1.8"` to `[workspace.dependencies]` |
| `types/Cargo.toml` | Add `zeroize.workspace = true` |
| `types/src/lib.rs` | `use zeroize::Zeroize;` and zeroize backing bytes in `Drop for SecretString` |

**IFA/Design alignment:** Cleanup is owned by `SecretString` boundary type; callers cannot forget it.

**Test:** Unit test private zeroization helper invoked by `Drop` and assert bytes are cleared before deallocation. Run `just verify`.

**Risk:** LOW.

---

## Phase 2: Mandatory Process Hardening for Spawns (macOS)

**Problem:** Security-critical launch normalization can be bypassed by direct `.spawn()` usage.

**Remediation:** Introduce spawn typestate so call sites cannot launch child processes without hardening proof, and cannot erase sandbox state before policy resolution.

### Design

`core/src/process_hardening.rs`:

```rust
pub struct UnhardenedCommand { /* wraps std::process::Command */ }
pub struct BaseHardenedCommand { /* private proof field */ }
pub struct SandboxedCommand { /* private proof field */ }
pub struct UnsandboxedCommand { /* private proof field */ }
pub struct TrustedExecutable { /* private proof field */ }
pub struct VerifiedExecutableFd { /* private proof field */ }
pub struct SanitizedEnv { /* private proof field */ }
pub struct ClosedFdSet { /* private proof field */ }
pub struct StdioPolicy { /* private proof field */ }
pub struct BoundarySnapshot { /* private proof field */ }
pub struct LaunchIdentity { /* private proof field */ }

pub struct ProcessBoundaryProfile {
    executable_policy: ExecutablePolicy,
    env_policy: EnvPolicy,
    fd_policy: FdPolicy,
    limits: ResourceLimits,
    identity_policy: ProcessIdentityPolicy,
    signal_policy: SignalPolicy,
    cwd_policy: CwdPolicy,
    umask_policy: UmaskPolicy,
}

impl UnhardenedCommand {
    pub fn harden_base(self, profile: ProcessBoundaryProfile) -> Result<BaseHardenedCommand, HardeningError>;
}

pub fn spawn_sandboxed(cmd: SandboxedCommand) -> io::Result<Child>;
pub fn spawn_unsandboxed(cmd: UnsandboxedCommand, reason: UnsandboxedReason) -> io::Result<Child>;
```

Rules:
1. Keep raw `Command` spawn helper private to module.
2. Export only hardened construction/spawn paths.
3. Update all tool spawn sites to use `UnhardenedCommand -> BaseHardenedCommand -> {SandboxedCommand|UnsandboxedCommand} -> spawn_*`.
4. Do not expose conversion from `SandboxedCommand` to `UnsandboxedCommand` or vice versa.
5. Keep hardening implementation shared for `tools` and `lsp`; do not maintain mirrored independent spawn hardening logic.

`lsp/src/server.rs`:
1. Use the shared hardening helper/API used by `tools`; do not fork behavior locally.
2. Do not call `.spawn()` directly from server setup.

`pre_exec`/launch hardening sequence (macOS):
1. create a new session/process group (`setsid`) for deterministic child lifecycle control
2. reset signal handlers and signal mask to policy baseline
3. apply UID/GID/supplementary-group policy when explicitly configured
4. apply `ResourceLimits` (`RLIMIT_CORE=0` baseline, explicit caps for `NOFILE` and `NPROC`, optional `CPU`/`AS`)
5. apply `StdioPolicy` and enforce CLOEXEC-by-default on non-stdio descriptors
6. close inherited file descriptors using explicit policy (`closefrom` when available, bounded fallback loop otherwise)
7. apply sanitized child environment from explicit allowlist/override policy
8. apply deterministic `PATH` policy for hardened launches

Boundary normalization (before `pre_exec`):
1. Resolve executable path to absolute canonical location and materialize `TrustedExecutable` proof:
   - validate path ancestry trust policy (reject writable-by-untrusted parent directories)
   - open executable handle and materialize `VerifiedExecutableFd`
   - capture stable identity facts (`dev`, `ino`, mode/owner checks) for launch-time revalidation
2. Reject ambiguous launch inputs (`PATH` surprises, relative executable with missing trusted cwd policy).
3. Materialize `SanitizedEnv`, `ClosedFdSet`, `StdioPolicy`, and `BoundarySnapshot` proof objects at the boundary.
4. Normalize cwd policy (explicit trusted cwd or deny spawn).
5. Normalize umask policy and inherited process identity facts before type transition.

Environment policy invariants:
1. `SanitizedEnv` is allowlist-first and typed; inherited environment is denied by default.
2. Runtime and loader injection variables (`DYLD_*`, `LD_*`, and runtime preload hooks) are explicitly denied unless policy grants them.
3. `PATH` is explicit and deterministic for hardened launches.
4. Elevating from minimal to toolchain environment requires an explicit capability proof token at the boundary.

**IFA/Design alignment:** hardening proof objects make unhardened spawn unrepresentable and preserve sandbox state as a first-class type until final policy match.

**Implementation**

| File | Change |
|------|--------|
| `core/src/process_hardening.rs` (new) | Shared typestate wrappers, boundary profile types, and private raw spawn path used by `tools` and `lsp` |
| `tools/src/process.rs` | Thin adapter over shared hardening API (tool-specific policy wiring only) |
| `tools/src/builtins.rs` | Migrate to hardened spawn API |
| `tools/src/git.rs` | Migrate to hardened spawn API |
| `tools/src/search.rs` | Migrate to hardened spawn API |
| `tools/src/windows_run.rs` | Replace direct run-command spawn assembly with hardened spawn API integration on macOS paths |
| `lsp/src/server.rs` | Migrate to shared hardened spawn API for LSP child |
| `tools/tests/compile_fail/*.rs` | Add typestate negative tests (`trybuild`) for forbidden transitions and raw spawn bypass |

**Test:**
1. Unit: API does not expose direct raw spawn from hardened modules.
2. Integration (macOS): spawned child is session-isolated (new process group/session behavior).
3. Integration: child gets sanitized environment only (unexpected inherited vars absent, especially `DYLD_*`).
4. Integration: child does not inherit unexpected file descriptors.
5. Integration: executable identity (`dev`/`ino`) is revalidated at launch and fails closed on mismatch.
6. Integration: signal mask/handler, cwd, and umask reflect hardened baseline.
7. Compile-fail: direct raw spawn and illegal typestate transitions fail to compile.
8. Run `just verify`.

**Risk:** LOW to MEDIUM (boundary normalization may surface latent assumptions in tool launch paths).

---

## Phase 3: Seatbelt as Policy + Proof + Mechanism

**Problem:** Tool subprocesses need kernel-enforced confinement on macOS, but `sandbox-exec` availability and policy fallback must remain caller-owned.

### Remediated design (policy/mechanism split)

Boundary facts:

```rust
pub enum SeatbeltAvailability {
    Available(SeatbeltPolicy),
    Unavailable(SeatbeltUnavailableReason),
}

pub enum SeatbeltUnavailableReason {
    DisabledByCallerPolicy,
    SandboxExecMissing,
    ProfileRenderFailed,
    ProbeFailed(i32),
}
```

Caller policy (chosen outside mechanism):

```rust
pub enum SandboxPolicy {
    RequireSandbox,
    AllowUnsandboxed,
}
```

Spawn decision result:

```rust
pub enum SpawnDecision {
    Sandboxed(SandboxedCommand),
    Unsandboxed(UnsandboxedCommand, SeatbeltUnavailableReason),
    Denied(SeatbeltUnavailableReason), // only when policy requires sandbox
}
```

Mechanism rule:
1. Seatbelt module only applies policy proof (`SeatbeltPolicy`) in launch setup.
2. Seatbelt module never decides to continue unsandboxed.
3. Callers must exhaustively match `(SandboxPolicy, SeatbeltAvailability)` to produce `SpawnDecision`.
4. Sensitive call paths may require sandbox by type/capability token and must never receive `AllowUnsandboxed`.

### `SeatbeltPolicy` proof object shape

Replace ad-hoc profile string assembly with typed rules that cannot desynchronize policy intent and generated profile:

```rust
pub struct SeatbeltPolicy {
    rules: Vec<SeatbeltPathRule>,
    network: NetworkPolicy,
}

pub struct SeatbeltPathRule {
    path: CanonicalPath,
    access: AccessProfile,
}

pub struct CanonicalPath(PathBuf); // constructor validates: absolute, canonical, exists

pub enum AccessProfile {
    ReadOnly,
    ReadWrite,
    ReadExec,
}

pub enum NetworkPolicy {
    DenyAll,
    AllowExplicit(Vec<NetworkCapability>),
}
```

Construction boundary validates each `CanonicalPath` and renders profile text from typed records only. Raw profile fragments are not accepted at call sites.

### Capability-scoped path policy (least privilege by tool class)

Do not use one broad default profile for every tool. Build policy from explicit capability tokens:

```rust
pub mod sandbox_caps {
    pub struct WorkspaceRead(());
    pub struct WorkspaceWrite(());
    pub struct TempRw(());
    pub struct SystemExecRead(());
    pub struct GitConfigRead(());
    pub struct ToolchainExec(());
    pub struct NetworkClient(());
}

pub struct CapabilitySet { /* private fields, no public Vec constructor */ }
```

`CapabilitySet::for_tool_class(tool_class: ToolClass) -> CapabilitySet` builds the minimal default proof set for each tool class. Optional capability widening requires explicit boundary-issued proof tokens.  
`seatbelt_policy_for(capabilities: CapabilitySet) -> SeatbeltPolicy` maps proofs to rules.

Baseline capability-to-path mapping:

| Path | Access | Rationale |
|------|--------|-----------|
| Workspace roots (resolved) | rw | Only for write-capable tools |
| Workspace roots (resolved) | ro | Read-only tools |
| `/tmp`, `/private/tmp` | rw | Temporary files |
| `/System`, `/usr`, `/bin`, `/sbin`, `/Library` | ro+exec | System binaries/libraries |
| `/dev/null`, `/dev/urandom` | rw | Minimal device nodes |
| `~/.gitconfig`, `~/.config/git` | ro | Git config |
| `~/Library/Developer/CommandLineTools`, `/Applications/Xcode.app/Contents/Developer` | ro+exec | Toolchain reads when required |
| `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/Library/Keychains` | deny | Secret-bearing locations |

Policy notes:
1. Keep default-deny network posture; enabling network requires explicit `NetworkClient` capability proof.
2. Keep capability tokens separate for read and write workspace behavior; do not collapse them.
3. Add explicit narrow allow-rules only when a concrete tool requirement is proven.
4. Keep compatibility escape hatches explicit and traceable.
5. Do not expose public constructors for capability tokens or `CapabilitySet`.
6. Keep profile literal escaping centralized (single renderer boundary).

### Implementation

| File | Change |
|------|--------|
| `tools/src/windows_run.rs` | Replace ad-hoc macOS profile generation with typed `SeatbeltPolicy` builder and policy/mechanism split |
| `tools/src/builtins.rs` | Resolve `SeatbeltAvailability`, choose `SandboxPolicy`, match to `SpawnDecision` |
| `tools/src/git.rs` | Same |
| `tools/src/search.rs` | Same |
| `tools/src/lib.rs` | Expose `SeatbeltAvailability`, `SeatbeltUnavailableReason`, and capability-scoped policy boundary types as needed |
| `config/src/lib.rs` | Keep `run.macos.enabled` and `fallback_mode` as caller policy inputs only |

`FORGE_RUN_ALLOW_UNSANDBOXED` and config fallback values are parsed at the caller boundary into policy input/proof tokens; mechanism modules never read environment toggles directly.

**IFA/Design alignment:**
1. Boundary collapses uncertainty into strict enums immediately.
2. Callers own fallback policy.
3. Mechanism only enforces declared policy.
4. Sum types preserve security-relevant distinctions through to spawn.

**Test:**
1. Unit: `seatbelt_policy_for(CapabilitySet)` builds expected typed rules.
2. Integration (macOS with `sandbox-exec` available): blocked read/write outside allowed roots returns permission error.
3. Integration: allowed workspace reads/writes still succeed.
4. Integration: default network-deny profile blocks outbound networking.
5. Integration: `RequireSandbox + Unavailable` yields denied spawn (no child launch).
6. Integration: `AllowUnsandboxed + Unavailable` launches with `tracing::warn!` including reason.
7. Compile-fail: code outside boundary modules cannot construct widening capability tokens.
8. Compile-fail: sensitive call paths requiring sandbox capability cannot select `AllowUnsandboxed`.
9. Run `just verify`.

**Risk:** MEDIUM. `sandbox-exec` is legacy and behavior can vary by macOS release.

---

## Phase 3.5: Seatbelt Backend Resilience Beyond `sandbox-exec`

**Problem:** `sandbox-exec` is a legacy interface and may become unavailable or insufficient for future macOS versions.

**Remediation:** Add backend abstraction and typed availability for Seatbelt application while preserving caller-owned fallback policy.

Design additions:
1. Add `SeatbeltBackendAvailability` boundary fact enum (`SandboxExec`, `FrameworkBackend`, `Unavailable(Reason)`).
2. Keep `SandboxPolicy` caller enum (`RequireSandbox | AllowUnsandboxed`) and require explicit matching.
3. Add `SeatbeltBackendDecision` sum type parallel to `SpawnDecision`.
4. Define backend-independent policy rendering contracts so the same typed `SeatbeltPolicy` drives each backend.
5. Keep backend-specific failure reasons distinct until final spawn denial/logging enum.

Mechanism rule:
1. Backend modules apply selected profile only.
2. Backend modules never decide policy fallback.
3. Callers exhaustively match policy and availability before spawn transition.

**Test:**
1. Integration: both backends (where available) enforce equivalent deny/allow semantics for core path and network rules.
2. Integration: `RequireSandbox + BackendUnavailable` denies spawn.
3. Integration: `AllowUnsandboxed + BackendUnavailable` preserves explicit warning path.
4. Compile-fail: sandbox-required paths cannot produce unsandboxed spawn decisions.
5. Run `just verify`.

**Risk:** MEDIUM to HIGH (platform churn and backend parity validation).

---

## Phase 4: Code-Signing Identity Policy (Future, Design Only)

Not for immediate implementation. Path canonicalization and ownership checks reduce risk, but they do not fully encode macOS software identity semantics.

If pursued:
1. Add typed executable identity facts that include code-signing metadata for launch decisions.
2. Keep signature policy caller-owned (`RequireSigned`, `AllowUnsigned`, etc.); mechanism only validates and reports facts.
3. Gate sensitive auto-approved command paths on explicit signature policy proofs.
4. Keep unsupported/unavailable signature-verification outcomes in distinct reason enums until final decision type.
5. Preserve explicit policy logging for unsigned fallback decisions.

---

## Sequencing

```
Phase 1 (zeroize) -> Phase 2 (spawn typestate + boundary normalization) -> Phase 3 (seatbelt policy split + least privilege) -> Phase 3.5 (seatbelt backend resilience) -> Phase 4 (code-sign identity policy)
   [independent]                    [blocks bypass + inherited-state leaks + launch identity checks] [adds macOS kernel-enforced confinement + proof-scoped capabilities] [deprecation-proof backend strategy] [supply-chain identity hardening]
```

One phase per PR.

## Verification

After each phase:
1. `just fix`
2. `just verify`
3. Manual smoke test: run Forge and invoke a tool command
4. Phase 2: validate descriptor/environment normalization behavior and session/process-group isolation
5. Phase 3: validate both `RequireSandbox` and `AllowUnsandboxed` policy branches
6. Phase 3: validate least-privilege defaults (secret-bearing home paths denied by default)
7. Phase 2/3/3.5: run compile-fail tests for typestate and capability invariants
8. Phase 3.5: validate backend-availability fallback behavior on hosts without usable sandbox backend

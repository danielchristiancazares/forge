# Linux Hardening Plan

## Context

Forge ships onto user-managed Linux systems. Hardening must be enforced by structure, not convention. This plan aligns Linux hardening with `DESIGN.md`: invalid states are unrepresentable, boundary parsing collapses uncertainty immediately, and policy decisions stay with callers.

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
10. Environment toggles are caller policy inputs, never mechanism-owned bypass decisions.
11. Mechanism fact enums must not encode caller policy choices.
12. Kernel/feature compatibility is collapsed at boundary ingestion into strict proof-bearing profiles; core hardening logic remains branch-free over probed facts.

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

## Phase 2: Mandatory Process Hardening for Spawns

**Problem:** Security-critical `pre_exec` setup can be bypassed by direct `.spawn()` usage.

**Remediation:** Introduce spawn typestate so call sites cannot launch child processes without hardening proof, and cannot erase sandbox state before policy resolution.

### Design

`tools/src/process.rs`:

```rust
pub struct UnhardenedCommand { /* wraps std::process::Command */ }
pub struct BaseHardenedCommand { /* private proof field */ }
pub struct SandboxedCommand { /* private proof field */ }
pub struct UnsandboxedCommand { /* private proof field */ }
pub struct TrustedExecutable { /* private proof field */ }
pub struct VerifiedExecutableFd { /* private proof field */ }
pub enum SanitizedEnv {
    Minimal(MinimalEnv),
    Toolchain(ToolchainEnv),
}
pub struct ClosedFdSet { /* private proof field */ }
pub struct StdioPolicy { /* private proof field */ }
pub struct BoundarySnapshot { /* private proof field */ }
pub struct CapabilityDropProof { /* private proof field */ }
pub struct KernelFeatureProfile { /* private proof field */ }
pub struct ResourceLimitsProfile { /* every limit expressed, no Option fields */ }
pub enum ExecLaunchStrategy {
    FdExec(VerifiedExecutableFd),
    PathExecFallback(PathExecFallbackToken),
}
pub enum UnsandboxedReason {
    LandlockUnavailable(LandlockUnavailableReason),
    IsolationUnavailable(IsolationUnavailableReason),
}

pub struct ProcessBoundaryProfile {
    executable_policy: ExecutablePolicy,
    env_policy: EnvPolicy,
    fd_policy: FdPolicy,
    limits: ResourceLimitsProfile,
    identity_policy: ProcessIdentityPolicy,
    signal_policy: SignalPolicy,
    cwd_policy: CwdPolicy,
    umask_policy: UmaskPolicy,
    capability_policy: CapabilityPolicy,
    kernel_features: KernelFeatureProfile,
    exec_strategy: ExecLaunchStrategy,
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

`pre_exec` hardening sequence:
1. `setsid()`
2. `prctl(PR_SET_PDEATHSIG, SIGKILL)`
3. close parent-death race window (re-check parent identity immediately after `PR_SET_PDEATHSIG`; fail closed on mismatch/reparent)
4. `prctl(PR_SET_NO_NEW_PRIVS, 1)`
5. apply dumpability action selected in `KernelFeatureProfile` (no runtime compatibility probing in `pre_exec`)
6. reset signal handlers and signal mask to policy baseline
7. apply UID/GID/supplementary-group/capability policy (drop ambient and inheritable capabilities unless explicitly needed)
8. apply `ResourceLimitsProfile` (explicit values for every tracked limit; no optional limit branches in core hardening code)
9. apply `StdioPolicy` and enforce CLOEXEC-by-default on non-stdio descriptors
10. close inherited file descriptors using pre-selected `ClosedFdSet` strategy from boundary profile (no in-core fallback probing)
11. apply sanitized child environment from explicit allowlist/override policy

Boundary normalization (before `pre_exec`):
1. Resolve executable path to absolute canonical location and materialize `TrustedExecutable` proof:
   - validate path ancestry trust policy (reject writable-by-untrusted parent directories)
   - open executable handle and materialize `VerifiedExecutableFd`
   - select `ExecLaunchStrategy` at boundary (`FdExec` preferred; `PathExecFallback` only with explicit policy token)
   - capture stable identity facts (`dev`, `ino`, mode/owner checks) for launch-time revalidation when fallback path launch is policy-approved
2. Reject ambiguous launch inputs (`PATH` surprises, relative executable with missing trusted cwd policy).
3. Probe kernel capability surface once and collapse it into `KernelFeatureProfile` before type transition.
4. Materialize `SanitizedEnv`, `ClosedFdSet`, `StdioPolicy`, and `BoundarySnapshot` proof objects at the boundary.
5. Normalize cwd policy (explicit trusted cwd or deny spawn).
6. Normalize umask policy and inherited process identity facts before type transition.

Environment policy invariants:
1. `SanitizedEnv` is allowlist-first and typed (`MinimalEnv` vs `ToolchainEnv`); inherited environment is denied by default.
2. Runtime and loader injection variables (`LD_*`, `DYLD_*`, and runtime preload hooks) are explicitly denied unless policy grants them.
3. `PATH` is explicit and deterministic for hardened launches.
4. Elevating from `MinimalEnv` to `ToolchainEnv` requires an explicit capability proof token at the boundary.

**IFA/Design alignment:** hardening proof objects make unhardened spawn unrepresentable and preserve sandbox state as a first-class type until final policy match.

**Implementation**

| File | Change |
|------|--------|
| `core/src/process_hardening.rs` (new) | Shared typestate wrappers, boundary profile types, and private raw spawn path used by `tools` and `lsp` |
| `tools/src/process.rs` | Thin adapter over shared hardening API (tool-specific policy wiring only) |
| `tools/src/builtins.rs` | Migrate to hardened spawn API |
| `tools/src/git.rs` | Migrate to hardened spawn API |
| `tools/src/search.rs` | Migrate to hardened spawn API |
| `tools/src/powershell_ast.rs` | Migrate to hardened spawn API |
| `lsp/src/server.rs` | Migrate to shared hardened spawn API for LSP child |
| `lsp/Cargo.toml` | Add `libc` unix dep if needed for `prctl`/`setsid` |
| `tools/tests/compile_fail/*.rs` | Add typestate negative tests (`trybuild`) for forbidden transitions and raw spawn bypass |

**Test:**
1. Unit: API does not expose direct raw spawn from hardened modules.
2. Integration (Linux): spawned child shows `NoNewPrivs: 1` in `/proc/<pid>/status`.
3. Integration: child process dies on parent death (`PDEATHSIG` behavior), including race-window regression coverage.
4. Integration: child gets sanitized environment only (unexpected inherited vars absent).
5. Integration: child does not inherit unexpected file descriptors.
6. Integration: fd-backed exec path is used when available and survives executable path rename races.
7. Integration: when fd-backed exec is unavailable, spawn is denied unless `PathExecFallbackToken` policy is present.
8. Integration: executable identity (`dev`/`ino`) is revalidated at launch and fails closed on mismatch when fallback path launch is used.
9. Integration: signal mask/handler, cwd, and umask reflect hardened baseline.
10. Compile-fail: direct raw spawn and illegal typestate transitions fail to compile.
11. Compile-fail: path-based launch cannot be selected without explicit fallback proof token.
12. Run `just verify`.

**Risk:** LOW to MEDIUM (boundary normalization may surface latent assumptions in tool launch paths).

---

## Phase 3: Landlock as Policy + Proof + Mechanism

**Problem:** Tool subprocesses need kernel-enforced filesystem confinement, but support varies by kernel.

### Remediated design (policy/mechanism split)

Boundary facts:

```rust
pub enum LandlockAvailability {
    Available(LandlockPolicy),
    Unavailable(LandlockUnavailableReason),
}

pub enum LandlockUnavailableReason {
    KernelTooOld,   // ENOSYS
    NotSupported,   // EOPNOTSUPP
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
    Unsandboxed(UnsandboxedCommand, LandlockUnavailableReason),
    Denied(LandlockUnavailableReason), // only when policy requires sandbox
}
```

Mechanism rule:
1. Landlock module only applies policy proof (`LandlockPolicy`) in `pre_exec`.
2. Landlock module never decides to continue unsandboxed.
3. Callers must exhaustively match `(SandboxPolicy, LandlockAvailability)` to produce `SpawnDecision`.
4. Sensitive call paths may require sandbox by type/capability token and must never receive `AllowUnsandboxed`.

### `LandlockPolicy` proof object shape

Replace loose tuple rules with typed rules that cannot desynchronize path and FD state:

```rust
pub struct LandlockPolicy {
    rules: NonEmptyLandlockRules,
}

pub struct NonEmptyLandlockRules {
    // Keyed by canonical path; constructor enforces non-empty and de-duplicates by join.
    by_path: BTreeMap<CanonicalDir, LandlockPathRule>,
}

pub struct LandlockPathRule {
    access: AccessProfile,
    fd: OwnedFd,
}

pub struct CanonicalDir(PathBuf); // constructor validates: absolute, canonical, exists, directory

pub enum AccessProfile {
    ReadOnly,
    ReadWrite,
    ReadExec,
}
```

Construction boundary validates each `CanonicalDir`, opens all FDs up front, rejects empty policy construction, and canonicalizes duplicate path grants with monotonic access joins.

### Capability-scoped path policy (least privilege by tool class)

Do not use one broad default rule set for every tool. Build policy from explicit capability tokens:

```rust
pub mod sandbox_caps {
    pub struct WorkspaceRead(());
    pub struct WorkspaceWrite(());
    pub struct TempRw(());
    pub struct SystemExecRead(());
    pub struct GitConfigRead(());
    pub struct RustToolchainExec(());
    pub struct TtyAccess(());
}

pub struct CapabilitySet { /* private fields, no public Vec constructor */ }
```

`CapabilitySet::for_tool_class(tool_class: ToolClass) -> CapabilitySet` builds the minimal default proof set for each tool class. Optional capability widening requires explicit boundary-issued proof tokens.  
`landlock_policy_for(capabilities: CapabilitySet) -> LandlockPolicy` maps proofs to rules.

Baseline capability-to-path mapping:

| Path | Access | Rationale |
|------|--------|-----------|
| Workspace roots (resolved) | rw | Only for write-capable tools |
| Workspace roots (resolved) | ro | Read-only tools |
| `/tmp`, `/var/tmp` | rw | Temporary files |
| `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin` | ro+exec | System binaries/libraries |
| `/etc` | ro | Runtime config reads |
| `/dev/null`, `/dev/urandom` | rw | Minimal device nodes |
| Controlling tty path (resolved per process, optional) | rw | Interactive subprocess support when required |
| `~/.gitconfig`, `~/.config/git` | ro | Git config |
| `~/.cargo/bin`, `~/.rustup` | ro+exec | Rust toolchain (only for rust tool invocations) |

Policy notes:
1. Do not grant blanket `/dev` or `/proc` access by default.
2. Keep capability tokens separate for read and write workspace behavior; do not collapse them.
3. Add explicit narrow allow-rules only when a concrete tool requirement is proven.
4. Keep compatibility escape hatches explicit and traceable (for example `/nix/store`).
5. Do not expose public constructors for capability tokens or `CapabilitySet`.

### Implementation

| File | Change |
|------|--------|
| `tools/src/process.rs` | Add landlock module, typed proof objects, and hardening integration in one `pre_exec` |
| `tools/src/builtins.rs` | Resolve `LandlockAvailability`, choose `SandboxPolicy`, match to `SpawnDecision` |
| `tools/src/git.rs` | Same |
| `tools/src/search.rs` | Same |
| `tools/src/powershell_ast.rs` | Same |

`FORGE_LANDLOCK=0` is parsed at the caller boundary into a policy input that yields `SandboxPolicy::AllowUnsandboxed`; mechanism modules never read environment toggles directly.

**IFA/Design alignment:**
1. Boundary collapses uncertainty into strict enums immediately.
2. Callers own fallback policy.
3. Mechanism only enforces declared policy.
4. Sum types preserve security-relevant distinctions through to spawn.

**Test:**
1. Unit: `landlock_policy_for(CapabilitySet)` builds canonical, non-empty typed rules keyed by canonical path with no conflicting duplicates.
2. Integration (Linux 5.13+): blocked read outside allowed roots returns permission error.
3. Integration: allowed project reads/writes still succeed.
4. Integration: `RequireSandbox + Unavailable` yields denied spawn (no child launch).
5. Integration: `AllowUnsandboxed + Unavailable` launches with `tracing::warn!` including reason.
6. Integration: baseline capability-scoped policy denies access to non-allowed `/dev` and `/proc` paths.
7. Unit: capability token sets produce minimal rules per tool class.
8. Compile-fail: code outside boundary modules cannot construct widening capability tokens.
9. Compile-fail: sensitive call paths requiring sandbox capability cannot select `AllowUnsandboxed`.
10. Run `just verify`.

**Risk:** MEDIUM. Toolchains on uncommon layouts (`/nix/store`, linuxbrew) may require additional explicit rules.

---

## Phase 3.5: Process Isolation Beyond Landlock

**Problem:** Filesystem confinement does not isolate process tree visibility, network surface, or resource exhaustion risk.

**Remediation:** Add namespace and cgroup policy as typed boundary decisions, preserving caller-owned fallback policy.

Design additions:
1. Add `IsolationAvailability` boundary fact enum (`Available(IsolationProfile)` or `Unavailable(IsolationUnavailableReason)`).
2. Add `IsolationPolicy` caller enum (`RequireIsolation | AllowUnisolated`).
3. Add `IsolationDecision` sum type parallel to `SpawnDecision`.
4. Add typed profiles for:
   - user, mount, and pid namespace setup (where permitted)
   - default-deny network namespace isolation (network access requires explicit capability token)
   - cgroup limits (`pids.max`, memory ceilings) with explicit defaults
5. Keep `IsolationUnavailableReason` distinct from landlock reasons; merge only in top-level spawn-denial enum.

Mechanism rule:
1. Isolation module applies selected profile only.
2. Isolation module never decides policy fallback.
3. Callers exhaustively match policy and availability before spawn transition.

**Test:**
1. Integration: isolated child cannot inspect host process tree beyond namespace policy.
2. Integration: network-deny profiles block unneeded network access.
3. Integration: cgroup pid and memory ceilings enforce bounded failure modes.
4. Compile-fail: isolation-required paths cannot produce unisolated spawn decisions.
5. Compile-fail: network-enabled profiles require explicit network capability proof token.
6. Run `just verify`.

**Risk:** MEDIUM to HIGH (kernel and container environment variance).

---

## Phase 4: seccomp-bpf

Syscall-level confinement layered beneath Landlock (Phase 3). Full specification in [`PLAN_SECCOMP_HARDENING.md`](PLAN_SECCOMP_HARDENING.md).

Summary: per-tool-class BPF filters compiled via `seccompiler` (pure Rust, Firecracker pedigree). Three sub-phases — audit infrastructure (`SECCOMP_RET_LOG`), enforcement rollout, argument filtering. Installs as step 13 in the `pre_exec` sequence (after Landlock at step 12).

---

## Sequencing

```
Phase 1 (zeroize) -> Phase 2 (spawn typestate + boundary normalization) -> Phase 3 (landlock policy split + least privilege) -> Phase 3.5 (namespaces/cgroups) -> Phase 4 (seccomp)
   [independent]                    [blocks bypass + inherited-state leaks + launch identity checks] [adds filesystem confinement + proof-scoped capabilities] [process/network/resource isolation]   [syscall minimization]
```

One phase per PR.

## Verification

After each phase:
1. `just fix`
2. `just verify`
3. Manual smoke test: run Forge and invoke a tool command
4. Phase 2: confirm `NoNewPrivs: 1` in `/proc/<pid>/status`
5. Phase 2: validate `PDEATHSIG`, fd-backed exec behavior, and descriptor/environment normalization behavior
6. Phase 3: validate both `RequireSandbox` and `AllowUnsandboxed` policy branches
7. Phase 3: validate least-privilege defaults (`/dev` and `/proc` not broadly available)
8. Phase 2/3/3.5: run compile-fail tests for typestate and capability invariants
9. Phase 3.5: validate namespace and cgroup isolation behavior where supported

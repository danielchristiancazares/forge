# Linux Hardening Plan

## Context

Forge ships onto user-managed Linux systems. Hardening must be enforced by structure, not convention. This plan aligns Linux hardening with `DESIGN.md`: invalid states are unrepresentable, boundary parsing collapses uncertainty immediately, and policy decisions stay with callers.

Four phases of increasing depth. Phases 1-3 are implementable now. Phase 4 is design-only.

---

## Contract With DESIGN.md

This plan follows these non-negotiable rules:

1. No mechanism-owned fallback policy. Mechanisms report facts; callers choose behavior.
2. No "remember to harden" conventions. Spawn APIs must make unhardened process launch impossible in core paths.
3. No loose core tuples for security state. Use named sum/product types that carry proofs.

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

**Remediation:** Introduce spawn typestate so call sites cannot launch child processes without hardening proof.

### Design

`tools/src/process.rs`:

```rust
pub struct UnhardenedCommand { /* wraps std::process::Command */ }
pub struct HardenedCommand { /* private proof field */ }

pub enum ChildHardening {
    Minimal,                 // setsid + PDEATHSIG + NO_NEW_PRIVS
    WithFilesystemSandbox,   // same + landlock in Phase 3
}

impl UnhardenedCommand {
    pub fn harden(self, profile: ChildHardening) -> Result<HardenedCommand, HardeningError>;
}

pub fn spawn_hardened(cmd: HardenedCommand) -> io::Result<Child>;
```

Rules:
1. Keep raw `Command` spawn helper private to module.
2. Export only hardened construction/spawn path.
3. Update all tool spawn sites to use `UnhardenedCommand -> HardenedCommand -> spawn_hardened`.

`lsp/src/server.rs`:
1. Mirror same pattern locally (or shared crate helper if one is introduced later).
2. Do not call `.spawn()` directly from server setup.

`pre_exec` hardening sequence:
1. `setsid()`
2. `prctl(PR_SET_PDEATHSIG, SIGKILL)`
3. `prctl(PR_SET_NO_NEW_PRIVS, 1)`

**IFA/Design alignment:** hardening proof object (`HardenedCommand`) makes unhardened spawn unrepresentable in updated paths.

**Implementation**

| File | Change |
|------|--------|
| `tools/src/process.rs` | Add typestate wrappers and private raw spawn path |
| `tools/src/builtins.rs` | Migrate to hardened spawn API |
| `tools/src/git.rs` | Migrate to hardened spawn API |
| `tools/src/search.rs` | Migrate to hardened spawn API |
| `tools/src/powershell_ast.rs` | Migrate to hardened spawn API |
| `lsp/src/server.rs` | Migrate to hardened spawn API for LSP child |
| `lsp/Cargo.toml` | Add `libc` unix dep if needed for `prctl`/`setsid` |

**Test:**
1. Unit: API does not expose direct raw spawn from hardened modules.
2. Integration (Linux): spawned child shows `NoNewPrivs: 1` in `/proc/<pid>/status`.
3. Run `just verify`.

**Risk:** LOW.

---

## Phase 3: Landlock as Policy + Proof + Mechanism

**Problem:** Tool subprocesses need kernel-enforced filesystem confinement, but support varies by kernel.

### Remediated design (policy/mechanism split)

Boundary facts:

```rust
pub enum LandlockAvailability {
    Available(LandlockPolicy),
    Unavailable(UnsandboxedReason),
}

pub enum UnsandboxedReason {
    DisabledByEnv,
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
    Sandboxed(HardenedCommand),
    Unsandboxed(HardenedCommand, UnsandboxedReason),
    Denied(UnsandboxedReason), // only when policy requires sandbox
}
```

Mechanism rule:
1. Landlock module only applies policy proof (`LandlockPolicy`) in `pre_exec`.
2. Landlock module never decides to continue unsandboxed.
3. Callers must exhaustively match `(SandboxPolicy, LandlockAvailability)` to produce `SpawnDecision`.

### `LandlockPolicy` proof object shape

Replace loose tuple rules with typed rules:

```rust
pub struct LandlockPolicy {
    rules: Vec<LandlockRule>,
    fds: Vec<OwnedFd>,
}

pub struct LandlockRule {
    path: CanonicalDir,
    access: AccessProfile,
}

pub struct CanonicalDir(PathBuf); // constructor validates: absolute, canonical, exists, directory

pub enum AccessProfile {
    ReadOnly,
    ReadWrite,
    ReadExec,
}
```

Construction boundary validates each `CanonicalDir` and opens all FDs up front.

### Default tool path policy

| Path | Access | Rationale |
|------|--------|-----------|
| Project root | rw | Tools edit workspace |
| `/tmp`, `/var/tmp` | rw | Temporary files |
| `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin` | ro+exec | System binaries/libraries |
| `/etc` | ro | Runtime config reads |
| `/proc` | ro | Process metadata reads |
| `/dev` | rw | `/dev/null`, `/dev/urandom`, tty paths |
| `~/.gitconfig`, `~/.config/git` | ro | Git config |
| `~/.cargo/bin`, `~/.rustup` | ro+exec | Rust toolchain |

### Implementation

| File | Change |
|------|--------|
| `tools/src/process.rs` | Add landlock module, typed proof objects, and hardening integration in one `pre_exec` |
| `tools/src/builtins.rs` | Resolve `LandlockAvailability`, choose `SandboxPolicy`, match to `SpawnDecision` |
| `tools/src/git.rs` | Same |
| `tools/src/search.rs` | Same |
| `tools/src/powershell_ast.rs` | Same |

`FORGE_LANDLOCK=0` is parsed at boundary and maps to `UnsandboxedReason::DisabledByEnv`.

**IFA/Design alignment:**
1. Boundary collapses uncertainty into strict enums immediately.
2. Callers own fallback policy.
3. Mechanism only enforces declared policy.

**Test:**
1. Unit: `LandlockPolicy::for_tool` builds expected typed rules.
2. Integration (Linux 5.13+): blocked read outside allowed roots returns permission error.
3. Integration: allowed project reads/writes still succeed.
4. Integration: `RequireSandbox + Unavailable` yields denied spawn (no child launch).
5. Integration: `AllowUnsandboxed + Unavailable` launches with `tracing::warn!` including reason.
6. Run `just verify`.

**Risk:** MEDIUM. Toolchains on uncommon layouts (`/nix/store`, linuxbrew) may require additional explicit rules.

---

## Phase 4: seccomp-bpf (Future, Design Only)

Not for immediate implementation. seccomp-bpf can kill processes with minimal diagnostics when filters are wrong.

If pursued:
1. Start in `SECCOMP_RET_LOG` audit mode.
2. Use strict allowlists per tool class.
3. Maintain per-architecture filters.
4. Gate behind explicit opt-in (`FORGE_SECCOMP=1`).

---

## Sequencing

```
Phase 1 (zeroize) -> Phase 2 (hardened spawn typestate) -> Phase 3 (landlock policy split)
   [independent]          [blocks bypass]                       [adds filesystem confinement]
```

One phase per PR.

## Verification

After each phase:
1. `just verify`
2. `just fix`
3. Manual smoke test: run Forge and invoke a tool command
4. Phase 2: confirm `NoNewPrivs: 1` in `/proc/<pid>/status`
5. Phase 3: validate both `RequireSandbox` and `AllowUnsandboxed` policy branches

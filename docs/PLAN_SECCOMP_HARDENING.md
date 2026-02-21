# Seccomp-BPF Hardening Plan

## Context

This plan expands Phase 4 of `PLAN_LINUX_HARDENING.md` into a standalone implementation specification. It covers syscall-level confinement for Forge tool subprocesses using seccomp-bpf, layered beneath Landlock (Phase 3) and the spawn typestate (Phase 2).

seccomp-bpf is the lowest-level knob in the Linux sandbox stack. It intercepts every syscall before it reaches the kernel and either allows, logs, or kills the process. A wrong filter doesn't return an error — it sends `SIGKILL` with zero diagnostics. This demands a conservative, auditable, proof-carrying approach.

Three sub-phases. Sub-phase A (audit infrastructure) and Sub-phase B (enforcement profiles) are implementable after Phase 3. Sub-phase C (argument filtering) is an incremental tightening pass.

---

## Contract With DESIGN.md

Inherits all rules from `PLAN_LINUX_HARDENING.md` § Contract. Additionally:

1. seccomp mechanism only installs a caller-selected filter proof. It never decides whether to fall back to unfiltered execution.
2. Filter construction is a boundary operation. Core hardening code receives a compiled `BpfProgram` proof — no runtime syscall enumeration or architecture detection in `pre_exec`.
3. Per-architecture filter compilation is resolved at the boundary via `cfg(target_arch)`. Architecture is a compile-time fact — no runtime `ArchUnsupported` variant exists. Core code is branch-free over architecture facts.
4. Audit mode vs enforcement mode is a caller policy choice, not a mechanism default.
5. Filter profiles are opaque proof objects with private constructors. The proof is a strict newtype over `BpfProgram` — mode, class, and architecture metadata are consumed at the boundary during compilation, not carried as dead-weight fields.
6. `PR_SET_NO_NEW_PRIVS` is enforced via a compile-time capability token (`NoNewPrivsToken`), not a runtime probe. The `pre_exec` sequence is a typestate chain where Step 4 produces the token and Step 13 statically demands it.
7. Seccomp and Landlock decisions are composed into a single `ConfinementDecision` **sum type** at the boundary. The core receives one fully-resolved confinement object, not fragmented per-mechanism decisions.
8. Policy denial (`RequireSeccomp` + unavailable) is a boundary error (`Err(SpawnError::SeccompDenied)`), not a data variant. The boundary halts before `fork()` — no `Denied` variant crosses into the child.
9. Unavailability reasons are logged and consumed at the boundary. The child's `pre_exec` core receives `SeccompConfinement::Filtered(proof)` or `SeccompConfinement::Unfiltered` — no reason payload crosses the `fork` boundary into the async-signal-unsafe child context.
10. Tool-to-filter-class mapping is boundary-owned and exhaustive. Tool executors consume a selected class token and cannot choose arbitrary classes ad hoc.
11. `NoNewPrivsToken` is consumed by value (affine ownership) in `apply_seccomp`, enforcing exactly-once use. The token cannot be reused after seccomp installation.
12. Seccomp-BPF cannot dereference pointer arguments. `execve(path, ...)` path filtering is impossible at the seccomp layer. Landlock (Phase 3) is the complementary mechanism for binary execution restriction. This is why `ConfinementDecision` must resolve both mechanisms together.
13. Default deny for unlisted syscalls uses `SECCOMP_RET_ERRNO(ENOSYS)`, not `SECCOMP_RET_KILL_PROCESS`. Universal deny list syscalls use `SECCOMP_RET_KILL_PROCESS`. This tiered strategy prevents spurious kills from libc probing newer syscalls while maintaining hard denial of known-dangerous syscalls.

---

## Crate Selection

**`seccompiler`** (from `rust-vmm`, extracted from AWS Firecracker).

| Property | Value |
|----------|-------|
| C dependency | None (pure Rust) |
| Architectures | x86_64, aarch64, riscv64 |
| Filter compilation | Rust types → BPF bytecode at construction time |
| Argument filtering | Yes (`SeccompCondition` with typed comparisons) |
| Thread sync | `apply_filter_all_threads()` via `SECCOMP_FILTER_FLAG_TSYNC` |
| Production pedigree | AWS Firecracker |

Alternatives considered and rejected:

- **`libseccomp-rs`**: Wraps the C `libseccomp` library. Runtime C dependency complicates cross-compilation and CI. Broader architecture support (32-bit, MIPS, PPC) is irrelevant for Forge's targets.
- **`syscallz`**: Wraps `libseccomp` with a manually-maintained syscall enum. Incomplete coverage, not production-hardened.

---

## Syscall Taxonomy

Filter profiles use systemd's syscall group taxonomy as the organizational framework. This is the most widely-adopted categorization (mirrored by Firejail, referenced by Docker/Moby documentation).

### Group Definitions (systemd nomenclature)

| Group | Syscalls (representative, not exhaustive) |
|-------|-------------------------------------------|
| `@default` | `arch_prctl`, `brk`, `clock_getres`, `clock_gettime`, `exit`, `exit_group`, `futex`, `get_robust_list`, `getpid`, `getppid`, `gettid`, `getuid`, `geteuid`, `getgid`, `getegid`, `getrlimit`, `gettimeofday`, `membarrier`, `nanosleep`, `prlimit64`, `rseq`, `rt_sigreturn`, `sched_yield`, `set_robust_list`, `set_tid_address`, `sigreturn` |
| `@basic-io` | `close`, `dup`, `dup2`, `dup3`, `lseek`, `pread64`, `preadv`, `pwrite64`, `pwritev`, `read`, `readv`, `write`, `writev` |
| `@file-system` | `access`, `chdir`, `chmod`, `chown`, `creat`, `faccessat`, `fallocate`, `fchmod`, `fchown`, `fcntl`, `fstat`, `fstatfs`, `ftruncate`, `getcwd`, `getdents64`, `getxattr`, `inotify_add_watch`, `inotify_init`, `link`, `lstat`, `mkdir`, `open`, `openat`, `openat2`, `readlink`, `rename`, `renameat2`, `rmdir`, `stat`, `statfs`, `statx`, `symlink`, `truncate`, `unlink`, `unlinkat`, `utimensat` |
| `@signal` | `rt_sigaction`, `rt_sigpending`, `rt_sigprocmask`, `rt_sigsuspend`, `rt_sigtimedwait`, `sigaltstack`, `signalfd`, `signalfd4` |
| `@process` | `clone`, `clone3`, `execve`, `execveat`, `fork`, `vfork`, `getrusage`, `kill`, `prctl`, `tgkill`, `tkill`, `wait4`, `waitid` |
| `@io-event` | `epoll_create`, `epoll_create1`, `epoll_ctl`, `epoll_wait`, `epoll_pwait`, `eventfd`, `eventfd2`, `poll`, `ppoll`, `select`, `pselect6` |
| `@network-io` | `accept`, `accept4`, `bind`, `connect`, `getpeername`, `getsockname`, `getsockopt`, `listen`, `recv`, `recvfrom`, `recvmsg`, `recvmmsg`, `send`, `sendto`, `sendmsg`, `sendmmsg`, `setsockopt`, `shutdown`, `socket`, `socketpair` |
| `@sync` | `fdatasync`, `fsync`, `msync`, `sync`, `syncfs`, `sync_file_range` |
| `@timer` | `timer_create`, `timer_delete`, `timer_settime`, `timer_gettime`, `timerfd_create`, `timerfd_settime`, `timerfd_gettime` |
| `@ipc` | `msgctl`, `msgget`, `msgrcv`, `msgsnd`, `semctl`, `semget`, `semop`, `shmat`, `shmctl`, `shmdt`, `shmget` |
| `@memory` | `mmap`, `mprotect`, `munmap`, `mremap`, `madvise`, `mlock`, `mlock2`, `munlock`, `mlockall`, `munlockall`, `memfd_create`, `mincore` |

### Universal Deny List

Blocked unconditionally across all tool classes. Derived from cross-project consensus (Docker, Chrome, systemd, Firejail, Firecracker):

| Category | Syscalls | Rationale |
|----------|----------|-----------|
| Kernel modules | `init_module`, `finit_module`, `delete_module`, `create_module` | Kernel code injection |
| Mount/pivot | `mount`, `umount`, `umount2`, `pivot_root` | Filesystem namespace manipulation |
| Reboot/kexec | `reboot`, `kexec_load`, `kexec_file_load` | System destruction |
| Swap | `swapon`, `swapoff` | System resource manipulation |
| Raw I/O | `iopl`, `ioperm` | Direct hardware access |
| Tracing | `ptrace`, `process_vm_readv`, `process_vm_writev` | Cross-process memory access |
| BPF/perf | `bpf`, `perf_event_open` | Kernel instrumentation |
| Keyring | `add_key`, `request_key`, `keyctl` | Kernel keyring manipulation |
| Handle escape | `open_by_handle_at` | Container escape vector (CVE-2015-1322) |
| Userfault | `userfaultfd` | Kernel attack surface expansion |
| Accounting | `acct`, `quotactl` | System accounting manipulation |
| Sysctl | `_sysctl`, `sysfs` | Kernel parameter modification |
| Obsolete | `uselib`, `nfsservctl`, `query_module`, `get_kernel_syms` | Legacy, no legitimate use |
| CPU emulation | `modify_ldt`, `subpage_prot` | CPU state manipulation |
| Namespace | `unshare`, `setns` | Namespace escape |

---

## Type Design

### Capability Tokens

`PR_SET_NO_NEW_PRIVS` is a hard prerequisite for unprivileged seccomp. DESIGN.md rejects sequence-by-convention: *"The compiler cannot enforce the sequence... Return type serves as evidence."* Step 4 of the `pre_exec` chain must produce a zero-sized capability token. Step 13 (`apply_seccomp`) statically demands it.

Seccomp filters syscalls, but they do not filter resources the process *already holds*. If file descriptors leak or the environment is compromised (`LD_PRELOAD`) before the filter is applied, the sandbox's effectiveness is diminished. Steps 10 and 11 produce additional capability tokens to prove the environment is pristine before seccomp locks the door.

```rust
/// Produced by a successful `prctl(PR_SET_NO_NEW_PRIVS, 1)` call.
/// Private constructor — only the `pre_exec` step that sets NO_NEW_PRIVS can mint this.
/// Consumed by value (affine) in `apply_seccomp` — cannot be reused.
pub struct NoNewPrivsToken(());

/// Produced by the FD close step (Step 10). Proves inherited FDs are closed.
pub struct ClosedFdsToken(());

/// Produced by the env sanitization step (Step 11). Proves LD_PRELOAD etc. are clean.
pub struct EnvSanitizedToken(());
```

`NoNewPrivsToken` is consumed by value in `apply_seccomp`, enforcing exactly-once use per DESIGN.md §Affine Types. `ClosedFdsToken` and `EnvSanitizedToken` are taken by reference (they may be needed by Landlock in Step 12 as well).

This eliminates the runtime `NoNewPrivsNotSet` error path. If any token cannot be produced, the typestate chain halts before seccomp is reached — no runtime branch, no fallback.

### Boundary Facts

```rust
pub enum SeccompAvailability {
    Available(SeccompFilterProof),
    Unavailable(SeccompUnavailableReason),
}

pub enum SeccompUnavailableReason {
    KernelTooOld,         // EINVAL from seccomp(2)
    ProbeFailed(i32),     // unexpected errno
    // ArchUnsupported is eliminated: architecture is a compile-time fact
    // resolved via cfg(target_arch). The compiler prevents producing a
    // SeccompFilterProof on an unsupported architecture.
    // NoNewPrivsNotSet is eliminated: enforced by NoNewPrivsToken capability.
}
```

### Caller Policy

```rust
pub enum SeccompPolicy {
    RequireSeccomp,
    AllowUnfiltered,
}

pub enum SeccompMode {
    Enforce,    // SECCOMP_RET_KILL_PROCESS for universal deny list,
                // SECCOMP_RET_ERRNO(ENOSYS) for unlisted syscalls
    Audit,      // SECCOMP_RET_LOG on violation (development/profiling)
}

/// Logged and consumed at the boundary (parent process).
/// Does NOT cross the fork boundary into the child.
pub enum SeccompUnfilteredReason {
    DisabledByPolicy,
    Unavailable(SeccompUnavailableReason),
}
```

### Filter Proof Objects

```rust
/// Strict newtype over compiled BPF bytecode. Private constructor.
/// Mode, class, and architecture are consumed at the boundary during
/// compilation — they are baked into the bytecode and not carried as
/// metadata fields. Storing them alongside the bytecode would create
/// representational invalid states (e.g., struct claiming mode: Audit
/// while bytecode is compiled for Kill). DESIGN.md: "Product types
/// multiply the state space. Do not use multiplication when you mean
/// addition."
pub struct SeccompFilterProof(BpfProgram);

pub enum FilterClass {
    ReadOnly,
    ReadWrite,
    Git,
    Shell,
}
```

`SeccompFilterProof` has a private constructor. Only `build_seccomp_filter()` at the boundary can produce one. Logging of class, mode, and architecture happens at the boundary during compilation via `tracing::info!`, not by interrogating proof metadata after the fact.

### Spawn Integration (Unified Confinement)

`SeccompDecision` and Phase 3's Landlock decision must not be passed independently to the core. DESIGN.md: *"If two pieces of code must 'agree' on the state of a resource, the architecture is flawed."* Passing two uncoordinated decisions to `pre_exec` fragments confinement policy and permits partial application (seccomp without Landlock, or vice versa).

#### Policy Resolution (Boundary, Pre-Fork)

The boundary exhaustively matches `(SeccompPolicy, SeccompAvailability)` **before fork**:

```rust
// At the boundary (parent process), before fork:
match (policy, availability) {
    (RequireSeccomp, Unavailable(reason)) => {
        // Hard stop. No fork. No child. No ConfinementDecision.
        return Err(SpawnError::SeccompDenied(reason));
    }
    (AllowUnfiltered, Unavailable(reason)) => {
        tracing::warn!("seccomp unavailable: {reason:?}, proceeding unfiltered");
        SeccompConfinement::Unfiltered  // reason consumed here
    }
    (AllowUnfiltered, Available(_)) if disabled_by_policy => {
        tracing::info!("seccomp disabled by policy");
        SeccompConfinement::Unfiltered  // reason consumed here
    }
    (_, Available(proof)) => SeccompConfinement::Filtered(proof),
}
```

`SeccompConfinement` is the type that crosses the fork boundary — it carries no reason payload:

```rust
/// What the child's pre_exec receives. No reason data — reasons are
/// logged at the boundary before fork, not carried into the
/// async-signal-unsafe child context.
pub enum SeccompConfinement {
    Filtered(SeccompFilterProof),
    Unfiltered,
}
```

#### Unified Confinement Decision

The boundary composes both mechanism outcomes into one closed decision algebra:

```rust
/// Unified confinement decision. This is a closed sum type: each variant
/// is a legal end-state, and illegal cross-product combinations are
/// unrepresentable. Policy denial is an Err at the boundary — it never
/// appears as a variant here.
pub enum ConfinementDecision {
    FullyConfined {
        sandboxed: SandboxedCommand,
        seccomp: SeccompFilterProof,
    },
    SeccompOnly {
        unsandboxed: UnsandboxedCommand,
        seccomp: SeccompFilterProof,
    },
    LandlockOnly {
        sandboxed: SandboxedCommand,
    },
    Unconfined {
        unsandboxed: UnsandboxedCommand,
    },
}
```

The boundary function `compose_confinement_decision()` returns `Result<ConfinementDecision, SpawnError>`. The `Err` path handles policy denial. The `Ok` path is a fully-resolved confinement state with no reason payloads — reasons were logged at the boundary. Only the boundary module can construct this enum. The core's `pre_exec` receives `ConfinementDecision`, never individual mechanism decisions.

Note: `ConfinementDecision` variants that lack a mechanism (e.g., `LandlockOnly` has no seccomp field, `Unconfined` has neither) carry no reason data. Reasons were logged by the boundary before this type was constructed. The child process is post-fork and async-signal-unsafe — it cannot safely allocate or format reason strings.

### Composition With Existing Phases

The `pre_exec` hardening sequence from Phase 2 gains a new step. seccomp filter installation happens **last** in the sequence because `seccomp(2)` with `SECCOMP_SET_MODE_FILTER` is itself a syscall that must be permitted by any previously-installed filter.

Updated `pre_exec` sequence (additions marked):

```
 1.  setsid()
 2.  prctl(PR_SET_PDEATHSIG, SIGKILL)
 3.  parent-death race window check
 4.  prctl(PR_SET_NO_NEW_PRIVS, 1) → NoNewPrivsToken   ← capability token (NEW)
 5.  apply dumpability policy
 6.  reset signal handlers and mask
 7.  apply UID/GID/capability policy
 8.  apply resource limits
 9.  apply stdio policy + CLOEXEC
10.  close inherited file descriptors → ClosedFdsToken  ← capability token
11.  apply sanitized environment → EnvSanitizedToken    ← capability token
12.  apply Landlock policy (Phase 3)                    ← filesystem confinement
13.  apply seccomp filter (NoNewPrivsToken) (Phase 4)   ← syscall confinement (NEW)
```

Step 4 produces a `NoNewPrivsToken` capability token — a zero-sized proof that `PR_SET_NO_NEW_PRIVS` succeeded. Steps 10 and 11 produce `ClosedFdsToken` and `EnvSanitizedToken` respectively. Step 13 statically demands all three tokens, consuming `NoNewPrivsToken` by value (affine ownership) to enforce exactly-once use:

```rust
fn apply_seccomp(
    proof: SeccompFilterProof,
    nnp_token: NoNewPrivsToken,      // Step 4, consumed (affine)
    _fds_token: &ClosedFdsToken,     // Step 10, proves FDs closed
    _env_token: &EnvSanitizedToken,  // Step 11, proves env sanitized
) -> Result<(), SeccompInstallError>;
```

This is a compile-time guarantee, not a runtime check. If any preceding step fails, its token does not exist, and `apply_seccomp` cannot be called. The typestate chain halts. `NoNewPrivsToken` is consumed by value — it cannot be reused after seccomp installation, enforcing exactly-once semantics per DESIGN.md §Affine Types.

---

## Filter Profiles

### FilterClass::ReadOnly

Tools: `grep`/`ripgrep`, `find`, file read operations, search tools.

Narrowest profile. No write syscalls beyond stdout/stderr, no network, no process creation beyond the initial exec.

| Group | Included | Notes |
|-------|----------|-------|
| `@default` | Yes | Process lifecycle basics |
| `@basic-io` | Yes | read/write/close/dup/lseek |
| `@file-system` | Partial | Read-only subset: `open`/`openat` (arg filtering in Sub-phase C), `stat`, `fstat`, `lstat`, `statx`, `access`, `faccessat`, `readlink`, `getdents64`, `getcwd`, `chdir`, `fchdir` |
| `@signal` | Yes | Signal handling |
| `@io-event` | Yes | epoll/poll/select for async I/O |
| `@memory` | Yes | mmap/mprotect/munmap/brk |
| `@sync` | No | Read-only tools don't fsync |
| `@process` | Partial | `execve`, `execveat`, `wait4`, `waitid`, `exit`, `exit_group`, `clone` (thread-only flags) |
| `@network-io` | No | No network access |
| `@ipc` | No | No IPC needed |
| `@timer` | Yes | Timer support for timeouts |
| Extras | `getrandom`, `pipe`, `pipe2`, `ioctl` (filtered), `uname`, `sysinfo`, `seccomp`, `prctl` (filtered), `futex`, `set_tid_address`, `rseq` | Runtime/libc requirements |

### FilterClass::ReadWrite

Tools: file write, patch/edit, mkdir, file creation.

ReadOnly profile plus write-path filesystem syscalls.

| Additional over ReadOnly | Syscalls |
|--------------------------|----------|
| Write filesystem ops | `creat`, `mkdir`, `mkdirat`, `rename`, `renameat2`, `unlink`, `unlinkat`, `rmdir`, `link`, `linkat`, `symlink`, `symlinkat`, `truncate`, `ftruncate`, `fallocate`, `chmod`, `fchmod`, `fchmodat`, `chown`, `fchown`, `fchownat`, `utimensat` |
| Sync | `fsync`, `fdatasync`, `sync_file_range` |
| Temp files | `memfd_create` |

### FilterClass::Git

Tools: git operations (status, diff, commit, push, pull, clone).

ReadWrite profile plus network and credential helper support.

| Additional over ReadWrite | Syscalls |
|---------------------------|----------|
| Network | Full `@network-io` group |
| Process creation | `fork`, `vfork`, `clone` (with broader flag allowance for subprocesses) |
| IPC | `pipe`, `pipe2` (already in base), `socketpair` |
| Credential helpers | No additional — credential helpers run as child processes within the same filter |

### FilterClass::Shell

Tools: user-invoked shell commands via the `run` tool.

Broadest profile. Equivalent to Docker's default baseline minus the universal deny list. This is the escape hatch for arbitrary user commands.

| Additional over Git | Syscalls |
|---------------------|----------|
| Full `@ipc` | System V IPC for legacy programs |
| Full `@process` | Unrestricted (except `unshare`, `setns` which remain denied) |
| `ioctl` | Unfiltered (terminal control for interactive commands) |
| `mknod` | For programs that create FIFOs |
| `personality` | Restricted to safe values (0, `UNAME26`, `PER_LINUX`) |

---

## Architecture Handling

Syscall numbers differ between architectures. A filter compiled for x86_64 is silently wrong on aarch64.

### Strategy

1. Use `cfg(target_arch)` at compile time to select `seccompiler::TargetArch`. Architecture is a compile-time fact — `std::env::consts::ARCH` is not used. Unsupported architectures are excluded at compilation: `build_seccomp_filter()` only exists behind `#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]`.
2. Use `libc::SYS_*` constants for syscall numbers — these are architecture-correct at compile time.
3. Validate `seccomp_data.arch` (AUDIT_ARCH) at BPF program entry — `seccompiler` does this automatically.
4. Conditional-compile architecture-specific syscalls:

```rust
#[cfg(target_arch = "x86_64")]
fn arch_specific_allows(rules: &mut BTreeMap<i64, Vec<SeccompRule>>) {
    // x86_64-only legacy syscalls that glibc still uses
    allow(rules, libc::SYS_arch_prctl);
    allow(rules, libc::SYS_open);   // openat preferred but glibc may use open
    allow(rules, libc::SYS_stat);   // fstatat preferred but glibc may use stat
    allow(rules, libc::SYS_lstat);
    allow(rules, libc::SYS_poll);
    allow(rules, libc::SYS_fork);   // aarch64 only has clone
    allow(rules, libc::SYS_vfork);
}

#[cfg(target_arch = "aarch64")]
fn arch_specific_allows(rules: &mut BTreeMap<i64, Vec<SeccompRule>>) {
    // aarch64 uses the new-style syscall table; most legacy calls absent
    // Nothing additional needed — base groups use the modern variants
}
```

### Cross-Architecture Testing

CI must test on both x86_64 and aarch64. Filter compilation is deterministic, so the compiled BPF can be snapshot-tested per architecture.

---

## Sub-phase A: Audit Infrastructure

**Goal:** Install seccomp filters in `SECCOMP_RET_LOG` mode. No process kills. Blocked syscalls are logged to the kernel audit subsystem for profile validation.

### Implementation

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `seccompiler = "0.5"` to `[workspace.dependencies]` |
| `tools/Cargo.toml` | Add `seccompiler = { workspace = true }` under `[target.'cfg(target_os = "linux")'.dependencies]` |
| `tools/src/seccomp.rs` (new) | Filter construction boundary: `build_seccomp_filter()`, `SeccompFilterProof` (newtype over `BpfProgram`), `SeccompAvailability`, `NoNewPrivsToken`, profile builders. Gated behind `#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]`. |
| `tools/src/process.rs` | Probe seccomp availability at boundary (parent), compose `SeccompConfinement` + `LandlockDecision` into `ConfinementDecision` via `compose_confinement_decision()`, install filter in `pre_exec` with capability tokens |
| `tools/src/lib.rs` | Wire `SeccompPolicy` and `SeccompMode` from config/env |

### Config / Environment Interface

```toml
# ~/.forge/config.toml
[security]
seccomp = "audit"    # "off" | "audit" | "enforce"
```

Environment override: `FORGE_SECCOMP=off|audit|enforce` (parsed at caller boundary into `SeccompPolicy` + `SeccompMode`).

Default: `off` during Sub-phase A rollout. Switches to `audit` once profiles are validated.

### Audit Log Consumption

When `seccomp = "audit"`:
1. Filters use `SeccompAction::Log` (maps to `SECCOMP_RET_LOG`).
2. Blocked syscalls appear in `dmesg` / `journalctl` as `audit: type=1326 (SECCOMP)` entries.
3. Forge logs a `tracing::info!` at spawn time indicating audit mode is active.
4. Users validate coverage by running their typical workflow and checking for audit entries.

### Probe Sequence

At boundary (parent process, before fork):

1. Attempt `seccomp(SECCOMP_GET_ACTION_AVAIL, SECCOMP_RET_LOG)` to confirm audit support.
2. Compile filter for current architecture (resolved at compile-time via `cfg(target_arch)`).
3. Materialize `SeccompFilterProof` or `SeccompUnavailableReason`.

`PR_SET_NO_NEW_PRIVS` is **not** probed here. It executes in the child process during `pre_exec` (Step 4), producing a `NoNewPrivsToken`. Probing it in the parent is a logical impossibility — `NO_NEW_PRIVS` is a per-thread attribute set after `fork()`/`clone()`. The compile-time capability token eliminates the need for any runtime check.

### Test

1. Unit: `build_seccomp_filter(FilterClass::ReadOnly, SeccompMode::Audit)` produces a valid `BpfProgram` (architecture resolved at compile time).
2. Unit: all four `FilterClass` variants compile without error on both x86_64 and aarch64.
3. Unit: universal deny list syscalls are present in every filter profile.
4. Integration (Linux): spawned child with audit filter does not crash during normal tool execution.
5. Integration: `SeccompPolicy::RequireSeccomp` + `SeccompUnavailableReason` yields `Err(SpawnError::SeccompDenied)` before fork — no child process created.
6. Integration: `SeccompPolicy::AllowUnfiltered` + unavailable yields unfiltered spawn with `tracing::warn!`, reason consumed at boundary.
7. Compile-fail: `SeccompFilterProof` cannot be constructed outside `seccomp.rs` (newtype with private inner).
8. Compile-fail: `SeccompConfinement` cannot be produced without matching policy and availability at the boundary.
9. Compile-fail: `apply_seccomp()` cannot be called without `NoNewPrivsToken` (by value), `&ClosedFdsToken`, and `&EnvSanitizedToken`.
10. Compile-fail: `NoNewPrivsToken`, `ClosedFdsToken`, `EnvSanitizedToken` cannot be constructed outside the `pre_exec` module.
11. Compile-fail: `build_seccomp_filter()` does not exist on unsupported architectures (gated by `cfg`).
12. Compile-fail: invalid combined confinement states are unrepresentable; only boundary composition can construct `ConfinementDecision`. No `Denied` variant exists — policy denial is `Err(SpawnError)`.
13. Compile-fail: `NoNewPrivsToken` cannot be used after being consumed by `apply_seccomp` (affine/move semantics).
14. Snapshot: compiled BPF bytecode for each `(FilterClass, TargetArch)` pair is snapshot-tested (insta) to detect unintended filter changes.
15. Run `just verify`.

**Risk:** LOW. Audit mode cannot break tool execution.

---

## Sub-phase B: Enforcement Profiles

**Goal:** Switch from `SECCOMP_RET_LOG` to tiered enforcement for validated profiles.

### Tiered Default-Deny Strategy

Enforcement mode uses two deny tiers, not a flat `KILL_PROCESS`:

| Tier | Action | Applied To | Rationale |
|------|--------|-----------|-----------|
| Hard deny | `SECCOMP_RET_KILL_PROCESS` | Universal deny list (§Universal Deny List) | Known-dangerous syscalls with no legitimate use. Kill immediately. |
| Soft deny | `SECCOMP_RET_ERRNO(ENOSYS)` | All other unlisted syscalls | Safely informs libc the "kernel does not support" the syscall. glibc/musl probe newer syscalls (`clone3`, `statx`, `close_range`, `rseq`) and gracefully fall back to older, allowed variants when they receive `ENOSYS`. A flat `KILL_PROCESS` here causes spurious deaths from normal libc version drift. |

This is what Firecracker and crosvm both do. The hard/soft split prevents libc probing from killing tool processes while maintaining unconditional denial of genuinely dangerous syscalls.

### Prerequisites

- Sub-phase A deployed and audit logs reviewed across representative workloads.
- No unexpected syscall denials observed in audit mode for each tool class.
- CI passes on both x86_64 and aarch64 with audit filters active.

### Implementation

| File | Change |
|------|--------|
| `tools/src/seccomp.rs` | `SeccompMode::Enforce`: universal deny list uses `SeccompAction::KillProcess`, default action for unlisted syscalls uses `SeccompAction::Errno(libc::ENOSYS as u32)` |
| `tools/src/process.rs` | No structural changes — mode is already threaded through |
| Config | Default changes: `seccomp = "audit"` → `seccomp = "enforce"` |

### Error Handling

When a child process is killed by seccomp (`SIGSYS`):
1. Parent observes `WIFSIGNALED` with signal 31 (`SIGSYS`).
2. Forge reports: `"Tool process killed by seccomp filter (blocked syscall from universal deny list). Run with FORGE_SECCOMP=audit to identify the blocked syscall, or FORGE_SECCOMP=off to disable."`.
3. No retry — a seccomp kill from the universal deny list indicates a genuinely dangerous syscall.

When a child process receives `ENOSYS` for an unlisted syscall:
1. libc typically falls back to an older allowed variant silently.
2. If the tool cannot function without the denied syscall, it fails with its own error (not `SIGSYS`).
3. Users diagnose via `FORGE_SECCOMP=audit` to see which syscall was denied, then either add it to the profile allowlist or investigate why the tool needs it.

### Rollout Strategy

1. `FilterClass::ReadOnly` enforced first (narrowest profile, lowest risk).
2. `FilterClass::ReadWrite` enforced after ReadOnly is stable.
3. `FilterClass::Git` enforced after ReadWrite is stable.
4. `FilterClass::Shell` enforced last (broadest profile, most likely to hit edge cases).

### Test

1. Integration (Linux): child process attempting a universally-denied syscall (`ptrace`, `mount`) is killed with `SIGSYS` (`SECCOMP_RET_KILL_PROCESS`).
2. Integration: child process attempting an unlisted syscall receives `ENOSYS` (`SECCOMP_RET_ERRNO`), not `SIGSYS`.
3. Integration: `FilterClass::ReadOnly` child cannot call `socket()` (receives `ENOSYS`).
4. Integration: `FilterClass::ReadWrite` child cannot call `socket()` (receives `ENOSYS`).
5. Integration: `FilterClass::Git` child can call `socket()`.
6. Integration: `FilterClass::Shell` child can call most syscalls except universal deny list.
7. Integration: all existing tool tests pass with enforcement active.
8. Integration: `FORGE_SECCOMP=off` disables enforcement completely.
9. Integration: universal deny list kill produces actionable error message mentioning audit mode.
10. Integration: libc syscall probing (`clone3`, `statx`, `close_range`) does not kill the process — `ENOSYS` triggers graceful fallback.
11. Run `just verify`.

**Risk:** MEDIUM. Filter gaps will manifest as `SIGKILL` with poor diagnostics. Audit mode coverage in Sub-phase A mitigates this.

---

## Sub-phase C: Argument Filtering

**Goal:** Tighten allowed syscalls with argument-level restrictions for defense in depth.

### Targeted Syscalls

| Syscall | Argument Filter | Rationale |
|---------|-----------------|-----------|
| `clone` / `clone3` | Deny flags: `CLONE_NEWUSER`, `CLONE_NEWNS`, `CLONE_NEWNET`, `CLONE_NEWPID`, `CLONE_NEWUTS`, `CLONE_NEWIPC` | Prevent namespace creation/escape |
| `socket` | Allow: `AF_UNIX`, `AF_INET`, `AF_INET6`, `AF_NETLINK` only. Deny: `AF_VSOCK` (VM-host escape), `AF_PACKET` (raw packets), `AF_BLUETOOTH` | Restrict socket domain |
| `mmap` | Deny `PROT_WRITE \| PROT_EXEC` simultaneously | W^X enforcement |
| `mprotect` | Deny `PROT_WRITE \| PROT_EXEC` simultaneously | W^X enforcement |
| `prctl` | Allow: `PR_SET_NAME`, `PR_GET_NAME`, `PR_SET_PDEATHSIG`, `PR_SET_NO_NEW_PRIVS`, `PR_SET_DUMPABLE`, `PR_GET_DUMPABLE`, `PR_SET_SECCOMP`, `PR_GET_SECCOMP`, `PR_SET_TIMERSLACK`, `PR_CAPBSET_READ`. Deny others. | Restrict prctl surface |
| `personality` | Allow: `0`, `UNAME26` (0x0020000), `PER_LINUX` (0), `0xFFFFFFFF` (query). Deny others. | Prevent execution domain manipulation |
| `ioctl` | Per-class: ReadOnly/ReadWrite deny `TIOCSTI` (terminal injection). Shell allows all. | Prevent TTY stuffing attacks |

### Implementation

| File | Change |
|------|--------|
| `tools/src/seccomp.rs` | Add `SeccompCondition` rules to targeted syscalls in each profile builder |

### seccompiler API for Argument Filtering

```rust
use seccompiler::{SeccompCondition, SeccompCmpArgLen, SeccompCmpOp, SeccompRule};

// Example: deny clone with CLONE_NEWUSER (bit 0x10000000)
// Allow clone only when arg0 (flags) does NOT have CLONE_NEWUSER set
let clone_rule = SeccompRule::new(vec![
    SeccompCondition::new(
        0,                              // arg0 (flags)
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::MaskedEq(libc::CLONE_NEWUSER as u64),
        0,                              // masked value must equal 0 (bit not set)
    )?,
]);
```

### Test

1. Unit: `clone` filter denies `CLONE_NEWUSER` flag.
2. Unit: `socket` filter denies `AF_VSOCK`, `AF_PACKET`.
3. Unit: `mmap`/`mprotect` filter denies `PROT_WRITE | PROT_EXEC`.
4. Integration (Linux): child process attempting `clone(CLONE_NEWUSER)` is killed or denied.
5. Integration: child process can still create threads via `clone` with thread-safe flags.
6. Integration: W^X enforcement does not break JIT-free tool execution (no tool in Forge's set requires W+X pages).
7. Snapshot: updated BPF bytecode snapshots for all `(FilterClass, TargetArch)` pairs.
8. Run `just verify`.

### Limitation: Pointer Arguments (`execve`)

Seccomp-BPF operates on `seccomp_data`, which contains the syscall number and up to 6 integer arguments. It **cannot dereference pointers**. This means:

- `execve(const char *pathname, ...)` — the `pathname` argument is a pointer. Seccomp can see the pointer value (a memory address) but cannot inspect the string it points to.
- Path-based filtering of `execve` is impossible at the seccomp layer.
- Binary execution restriction is exclusively Landlock's responsibility (Phase 3, `LANDLOCK_ACCESS_FS_EXECUTE`).

This architectural limitation is why the contract (§12) requires `ConfinementDecision` to resolve both mechanisms together. Seccomp without Landlock cannot prevent execution of arbitrary binaries. Landlock without seccomp cannot prevent dangerous syscalls. Neither mechanism alone provides complete confinement.

**Risk:** MEDIUM. Argument filtering is the most brittle layer. libc internals may use unexpected flag combinations. The audit-first methodology from Sub-phase A applies here — deploy with `SeccompMode::Audit` first, validate, then enforce.

---

## Tool-to-FilterClass Mapping

| Tool | FilterClass | Rationale |
|------|-------------|-----------|
| `Read` / `Cat` | ReadOnly | File content inspection |
| `Grep` / `Search` | ReadOnly | Pattern matching over files |
| `Find` / `Glob` | ReadOnly | Directory traversal |
| `LSP diagnostics` | ReadOnly | Language server reads |
| `Write` / `Patch` | ReadWrite | File creation/modification |
| `Mkdir` | ReadWrite | Directory creation |
| `Git status/diff/log` | ReadOnly | Local-only git operations |
| `Git commit/add` | ReadWrite | Local write git operations |
| `Git push/pull/fetch/clone` | Git | Network-capable git operations |
| `Run` (shell) | Shell | Arbitrary user commands |
| `PowerShell AST` | ReadOnly | Script analysis |

### Mapping Enforcement

The `FilterClass` for each tool is determined by a boundary-owned mapper. Tool executors consume that selected class when constructing `SeccompFilterProof`:

```rust
// Boundary-owned exhaustive mapping:
// one place decides class, call sites consume the result.
let class = seccomp_class_for_tool(ToolKind::Read);
let filter = build_seccomp_filter(class, mode)?;

let class = seccomp_class_for_git_operation(op);
let filter = build_seccomp_filter(class, mode)?;

let class = seccomp_class_for_tool(ToolKind::RunShell);
let filter = build_seccomp_filter(class, mode)?;
```

Class selection lives in one boundary module with an exhaustive match over tool/operation kinds. Tool executors cannot directly construct arbitrary `FilterClass` selections.

---

## Compatibility and Escape Hatches

### Kernel Requirements

- `seccomp(2)` with `SECCOMP_SET_MODE_FILTER`: Linux 3.17+
- `SECCOMP_RET_LOG`: Linux 4.14+
- `SECCOMP_RET_KILL_PROCESS` (vs thread): Linux 4.14+
- `PR_SET_NO_NEW_PRIVS`: Linux 3.5+

Minimum effective kernel: **4.14** (for audit mode). Systems below 4.14 get `SeccompUnavailableReason::KernelTooOld`. Unsupported architectures are excluded at compile time via `cfg(target_arch)` — no runtime `ArchUnsupported` variant exists.

### libc Variance

glibc and musl use different underlying syscalls for the same libc functions. Known divergences:

| Operation | glibc | musl |
|-----------|-------|------|
| `open()` | may use `SYS_open` (x86_64) | always uses `SYS_openat` |
| `stat()` | may use `SYS_stat` | uses `SYS_fstatat` / `SYS_statx` |
| `fork()` | `SYS_clone` | `SYS_clone` |
| `poll()` | `SYS_poll` (x86_64) | `SYS_ppoll` |

**Mitigation:** Allow both legacy and modern variants on x86_64. aarch64 only has the modern variants (new-style syscall table).

### Nix / Guix / Non-Standard Layouts

seccomp operates on syscall numbers, not filesystem paths. No layout-specific adjustments needed (unlike Landlock Phase 3).

### Override

`FORGE_SECCOMP=off` disables seccomp entirely. Parsed at the caller boundary into `SeccompPolicy::AllowUnfiltered`, which maps to `SeccompUnfilteredReason::DisabledByPolicy` during decision composition. Mechanism modules never read this variable.

---

## Development Workflow

No Dockerfiles required. The methodology is:

1. Build filters with `SeccompMode::Audit` (maps to `SECCOMP_RET_LOG`).
2. Run `just verify` and exercise tool commands manually.
3. Check `dmesg | grep SECCOMP` or `journalctl -k | grep SECCOMP` for audit entries.
4. Any logged syscall is either:
   - **Missing from allowlist** → add it to the appropriate profile.
   - **Genuinely dangerous** → keep it denied; investigate why the tool uses it.
5. Once no audit entries appear for a profile, switch to `SeccompMode::Enforce`.
6. Repeat on both x86_64 and aarch64.

### Diagnostic Tooling

- **`seccomp-tools dump`** (Ruby gem): disassemble compiled BPF to verify filter logic.
- **`strace -f`**: trace child syscalls to cross-validate against filter profiles.
- **`/proc/<pid>/status`**: confirm `Seccomp: 2` (filter mode) after installation.

---

## File Summary

| File | Sub-phase | Change |
|------|-----------|--------|
| `Cargo.toml` (workspace) | A | Add `seccompiler = "0.5"` |
| `tools/Cargo.toml` | A | Add `seccompiler` Linux-only dep |
| `tools/src/seccomp.rs` (new) | A | Filter boundary: `SeccompFilterProof` (newtype), `NoNewPrivsToken`, profile builders, availability probe. `cfg`-gated per architecture. |
| `tools/src/process.rs` | A | Compose boundary seccomp + landlock outcomes via `compose_confinement_decision()` → `Result<ConfinementDecision, SpawnError>`. Policy denial returns `Err` before fork. Install filter in `pre_exec` with `NoNewPrivsToken` (consumed), `&ClosedFdsToken`, `&EnvSanitizedToken`. |
| `tools/src/lib.rs` | A | Wire `SeccompPolicy` / `SeccompMode` from config |
| `tools/src/builtins.rs` | A | Select `FilterClass` per tool |
| `tools/src/git.rs` | A | Select `FilterClass` per git operation |
| `tools/src/search.rs` | A | Select `FilterClass::ReadOnly` |
| `config/src/lib.rs` | A | Add `seccomp` field to security config |
| `tools/src/seccomp.rs` | B | `SeccompMode::Enforce` path (no structural change) |
| `tools/src/seccomp.rs` | C | Add `SeccompCondition` argument filters |
| `tools/tests/compile_fail/*.rs` | A | Proof object construction, `NoNewPrivsToken`/`ClosedFdsToken`/`EnvSanitizedToken` construction, `apply_seccomp` without tokens, `NoNewPrivsToken` reuse after consumption, `cfg`-gated architecture exclusion |
| `tools/tests/seccomp_integration.rs` (new) | A–C | Linux-only integration tests including `ConfinementDecision` composition |

---

## Sequencing

```
Sub-phase A (audit infrastructure)
    → validate profiles via audit logs
    → Sub-phase B (enforcement, per-class rollout)
        → Sub-phase C (argument filtering, incremental tightening)
```

Sub-phase A is one PR. Sub-phases B and C may be combined into a single PR if audit validation is clean.

## Verification

After each sub-phase:
1. `just fix`
2. `just verify`
3. Manual smoke test: run Forge, invoke tool commands, check `dmesg` for SECCOMP entries
4. Confirm `/proc/<pid>/status` shows `Seccomp: 2` for filtered children
5. Snapshot-test compiled BPF for each `(FilterClass, TargetArch)` pair
6. Run on both x86_64 and aarch64
7. Test `FORGE_SECCOMP=off` disables all filters
8. Test `FORGE_SECCOMP=audit` logs without killing
9. Test `FORGE_SECCOMP=enforce` kills on universal deny list syscalls (`SECCOMP_RET_KILL_PROCESS`) and returns `ENOSYS` on unlisted syscalls (`SECCOMP_RET_ERRNO`)

## References

| Source | URL |
|--------|-----|
| Docker/Moby default profile | `github.com/moby/profiles/blob/main/seccomp/default.json` |
| Chromium baseline policy | `chromium/sandbox/linux/seccomp-bpf-helpers/baseline_policy.cc` |
| systemd syscall groups | `systemd/src/shared/seccomp-util.c` |
| Firecracker seccomp docs | `github.com/firecracker-microvm/firecracker/blob/main/docs/seccomp.md` |
| crosvm seccomp docs | `crosvm.dev/book/appendix/seccomp.html` |
| Minijail policy format | `google.github.io/minijail/minijail0.5.html` |
| seccompiler crate | `github.com/rust-vmm/seccompiler` |
| Juszkiewicz syscall table | `gpages.juszkiewicz.com.pl/syscalls-table/syscalls.html` |
| syscalls.mebeim.net | `syscalls.mebeim.net` |
| PLAN_LINUX_HARDENING.md | `docs/PLAN_LINUX_HARDENING.md` (parent plan, Phases 1–3.5) |

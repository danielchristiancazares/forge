# Linux Hardening Plan

## Context

Forge is a product shipping to user machines — we don't own the infrastructure. This plan adds application-level self-hardening for Linux, layered on top of an already-strong baseline (SecretString, env denylist, filesystem sandbox, crash hardening, SSRF protection, secret redaction). Each phase follows IFA: invariants encoded structurally, proof objects, boundary-owned enforcement.

Four phases of increasing depth. Phases 1-3 are implementable now; Phase 4 is design-only.

---

## Phase 1: Secret Memory Safety

**Problem:** `SecretString` doesn't zero backing memory on drop. Keys persist in freed heap.

**Change:** Add `zeroize` crate (already transitive via rustls — zero new binary code) and impl `Drop`.

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `zeroize = "1.8"` to `[workspace.dependencies]` |
| `types/Cargo.toml` | Add `zeroize.workspace = true` |
| `types/src/lib.rs` | `use zeroize::Zeroize;` + `impl Drop for SecretString { fn drop(&mut self) { self.0.zeroize(); } }` |

**IFA:** Authority Boundary (`SecretString` module) guarantees cleanup. Callers can't forget — `Drop` is automatic. `ApiKey` inherits guarantee by composition (IFA-7.5).

**Test:** Unit test: capture ptr/len, drop, verify zeroed via unsafe read. + `just verify`.

**Risk:** LOW. 4-line change, well-audited crate, no behavioral change.

---

## Phase 2: Process Privilege Restriction

**Problem:** Child processes can escalate via setuid binaries. LSP servers lack `setsid`/`PR_SET_PDEATHSIG` entirely.

### 2a: Add `PR_SET_NO_NEW_PRIVS` to tool subprocesses

| File | Change |
|------|--------|
| `tools/src/process.rs` | Add `prctl(PR_SET_NO_NEW_PRIVS, 1)` to `set_new_session()` pre_exec, after existing `PR_SET_PDEATHSIG` |

All 4 spawn sites (builtins, search, git, powershell_ast) inherit via `set_new_session()`.

### 2b: Add session isolation + privilege restriction to LSP servers

| File | Change |
|------|--------|
| `lsp/Cargo.toml` | Add `[target.'cfg(unix)'.dependencies] libc.workspace = true` |
| `lsp/src/server.rs` | Add `pre_exec` hook before `.spawn()`: `setsid()` + `PR_SET_PDEATHSIG` + `PR_SET_NO_NEW_PRIVS` |

**IFA:** `pre_exec` is the Authority Boundary — kernel enforces NO_NEW_PRIVS irreversibly. Invalid state (privilege escalation) is unrepresentable at the substrate level (IFA-10).

**Note on duplication (IFA-7):** Two 10-line pre_exec bodies in two crates (tools, lsp) with different lifecycles. Acceptable — audit via `grep PR_SET_NO_NEW_PRIVS` is clear. Extract to shared function if a third site appears.

**Test:** Linux integration test: spawn child via `set_new_session`, read `/proc/self/status`, assert `NoNewPrivs:\t1`. + `just verify`.

**Risk:** LOW. Available since Linux 3.5 (2012). Only breaks if tools need setuid (extremely unlikely for code editing).

---

## Phase 3: Landlock Filesystem Confinement

**Problem:** Application-level sandbox validates paths Forge accesses, but spawned tool subprocesses have unrestricted kernel-level filesystem access.

### Design

**`LandlockPolicy` proof object** in `tools/src/process.rs`:

```
LandlockPolicy {
    rules: Vec<(PathBuf, AccessFlags)>,  // validated at construction
    fds: Vec<OwnedFd>,                   // opened before fork, inherited by child
}
```

Construction is the boundary: canonicalize paths, open directory FDs, validate existence. Application happens in `pre_exec` via raw syscalls (`landlock_create_ruleset`, `landlock_add_rule`, `landlock_restrict_self`).

**Default path policy for tool subprocesses:**

| Path | Access | Rationale |
|------|--------|-----------|
| Project root | rw | Tools operate on project files |
| `/tmp`, `/var/tmp` | rw | Many tools need temp files |
| `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin` | ro+exec | System libraries and binaries |
| `/etc` | ro | Config files (git config, resolv.conf, etc.) |
| `/proc` | ro | Process info (many tools read /proc/self) |
| `/dev` | rw | /dev/null, /dev/urandom, /dev/tty |
| `~/.gitconfig`, `~/.config/git` | ro | Git configuration |
| `~/.cargo/bin`, `~/.rustup` | ro+exec | Rust toolchain (if exists) |

**Graceful degradation:** `landlock_create_ruleset` probe at construction. If ENOSYS (kernel < 5.13) or EOPNOTSUPP (disabled), return `None`. Tool spawns unsandboxed with `tracing::warn!`.

**Opt-out:** `FORGE_LANDLOCK=0` env var disables Landlock (mirrors `FORGE_ALLOW_COREDUMPS` pattern).

### Implementation

| File | Change |
|------|--------|
| `tools/src/process.rs` | Add `mod landlock` with syscall wrappers, `LandlockPolicy` type, refactor `set_new_session` → `harden_child` that accepts `Option<&LandlockPolicy>` |
| `tools/src/builtins.rs` | Construct `LandlockPolicy::for_tool(project_root)` before spawn, pass to `harden_child` |
| `tools/src/git.rs` | Same pattern |
| `tools/src/search.rs` | Same pattern |
| `tools/src/powershell_ast.rs` | Same pattern |

**Key constraint:** Only one `pre_exec` closure per Command. Phase 2's `PR_SET_NO_NEW_PRIVS` and Phase 3's Landlock must share a single closure. Refactor `set_new_session` into `harden_child(cmd, landlock_policy)` that does all pre_exec work in one closure:
1. `setsid()`
2. `PR_SET_PDEATHSIG`
3. `PR_SET_NO_NEW_PRIVS` (also required by Landlock)
4. Landlock rules (if policy provided)

**IFA:** `LandlockPolicy` is a capability token (IFA-10) — its existence proves access was declared. Kernel enforcement makes unauthorized access unrepresentable (IFA-2.1). Policy vs mechanism cleanly separated (IFA-8): construction = policy, syscall wrappers = mechanism.

**Test:**
- Unit: `LandlockPolicy::for_tool` produces expected rules for given project root
- Integration (Linux 5.13+): spawn child with Landlock, attempt to read outside allowed paths, verify EACCES
- Integration: verify legitimate tool operations (git, file write in project) still work under Landlock
- Graceful degradation: mock/old kernel → `None` → tools work unsandboxed
- `just verify`

**Risk:** MEDIUM. Path allowlist may miss toolchain-specific paths (Nix: `/nix/store`, Homebrew linuxbrew: `/home/linuxbrew`). Mitigated by opt-out env var and warning logs.

---

## Phase 4: seccomp-bpf (Future — Design Only)

**Not for implementation now.** seccomp-bpf restricts syscall surface but is extremely high-risk: wrong filter = SIGKILL with no diagnostic. Different tool binaries need different syscalls, and the set varies by architecture.

**If pursued later:**
1. Start with `SECCOMP_RET_LOG` (audit mode) for weeks
2. Allowlist approach (permit known-needed syscalls)
3. Architecture-specific BPF programs
4. Opt-in only (`FORGE_SECCOMP=1`)

---

## Sequencing

```
Phase 1 (zeroize) → Phase 2 (NO_NEW_PRIVS) → Phase 3 (Landlock)
   [standalone]      [modifies pre_exec]       [extends pre_exec]
```

Each phase = one PR, validated with `just verify`.

## Verification

After each phase:
1. `just verify` (fmt + clippy + test)
2. `just fix` (CRLF normalization)
3. Manual smoke test: launch Forge, run a tool command, verify it works
4. Phase 2: on Linux, verify child processes show `NoNewPrivs: 1` in `/proc/PID/status`
5. Phase 3: on Linux 5.13+, verify Landlock active; on older kernels, verify graceful degradation warning

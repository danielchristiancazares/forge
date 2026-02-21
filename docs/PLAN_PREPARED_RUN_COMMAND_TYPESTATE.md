# Remediate PreparedRunCommand DESIGN.md Violations

`PreparedRunCommand` uses `warning: Option<String>` and `requires_host_sandbox: bool` -- both Death List violations. The struct permits the dead state `(Some(warning), true)` which is never constructed. Refactor into a three-variant enum that makes invalid states unrepresentable.

## Proposed Changes

### Sandbox Policy Types

#### [MODIFY] windows_run.rs

**Replace `PreparedRunCommand` struct with enum:**

```rust
pub enum PreparedRunCommand {
    HostIsolated { program: PathBuf, args: Vec<OsString> },
    Direct { program: PathBuf, args: Vec<OsString> },
    UnsandboxedFallback { program: PathBuf, args: Vec<OsString>, warning: NonEmptyString },
}
```

- `HostIsolated` -- Windows host sandbox (job object, `CREATE_NO_WINDOW`). Happy path when `host_probe` succeeds.
- `Direct` -- no host isolation. `passthrough()`, macOS `sandbox-exec`, sandbox-disabled, Linux/BSD passthrough.
- `UnsandboxedFallback` -- no host isolation, with attached warning. `AllowWithWarning` fallback arm only.

**Introduce `UnsandboxedOptIn` capability token:**

```rust
pub(crate) struct UnsandboxedOptIn(());

impl UnsandboxedOptIn {
    pub(crate) fn from_env() -> Option<Self> {
        // Check FORGE_RUN_ALLOW_UNSANDBOXED, return Some(Self(())) if enabled
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self(())
    }
}
```

This replaces the `allow_unsandboxed: bool` parameter. Possession of the token proves the env-var opt-in was validated.

**Replace constructors:**

- Delete `PreparedRunCommand::new()`. Each site builds the correct variant directly.
- `passthrough(shell, command)` constructs `Direct { program, args }`.

**Delete ALL accessor methods.** Consumers pattern-match on variants directly. No `program()`, `args()`, `warning()`, or `requires_host_sandbox()` accessors.

**Update construction sites in `prepare_windows_run_command_with_host_probe`:**

| Current code | New code |
|---|---|
| L325: `PreparedRunCommand::passthrough(...)` | No change (`passthrough` returns `Direct`) |
| L331: passthrough passed to fallback handler | Pass `(shell.binary.clone(), args)` |
| L375: `PreparedRunCommand::new(...)` passed to fallback handler | Pass `(shell.binary.clone(), fallback_args)` |
| L387-392: `PreparedRunCommand::new(prog, args, None, requires_host_sandbox)` | `HostIsolated { .. }` when true, `Direct { .. }` when false |

**Refactor `handle_unsandboxed_fallback` and `handle_unsandboxed_fallback_with_opt_in`:**

```rust
fn handle_unsandboxed_fallback(
    program: PathBuf,
    args: Vec<OsString>,
    mode: RunSandboxFallbackMode,
    reason: NonEmptyString,
) -> Result<PreparedRunCommand, ToolError>
```

```rust
fn handle_unsandboxed_fallback_with_opt_in(
    program: PathBuf,
    args: Vec<OsString>,
    mode: RunSandboxFallbackMode,
    reason: NonEmptyString,
    opt_in: Option<UnsandboxedOptIn>,
) -> Result<PreparedRunCommand, ToolError>
```

- `Deny` / `Prompt` arms: return `Err` (unchanged logic, `reason` interpolated via `.as_str()`).
- `AllowWithWarning` arm: requires `opt_in.is_some()`. Constructs `UnsandboxedFallback { program, args, warning }` where `warning` is built via `NonEmptyString::prefixed(...)` or equivalent using the `reason`.

**Update macOS construction sites (`prepare_macos_run_command`):**

| Current code | New code |
|---|---|
| L548: `passthrough(...)` | No change |
| L553-554: passthrough passed to fallback handler | Pass `(shell.binary.clone(), args)` |
| L575: `PreparedRunCommand::new(...)` | `Direct { program: sandbox_exec.clone(), args }` |

**Update Linux/BSD construction site:**

L289: `passthrough(...)` -- no change needed.

---

### Run Tool Consumer

#### [MODIFY] builtins.rs

**Replace accessor-based consumption with variant matching:**

L1174-1309: Destructure `prepared` via `match` to extract `program`, `args`, and optionally `warning`. Use the matched variant to determine:

- `HostIsolated` -> set `CREATE_NO_WINDOW`, attach to sandbox job.
- `Direct` -> attach to kill-on-close job.
- `UnsandboxedFallback` -> attach to kill-on-close job, prepend `warning.as_str()` to output.

---

### Test Updates

#### [MODIFY] windows_run.rs (tests module)

**Update variant assertions (replaces deleted accessors):**

| Test | Current assertion | New assertion |
|---|---|---|
| L893 | `prepared.warning().is_some()` | `matches!(prepared, PreparedRunCommand::UnsandboxedFallback { .. })` |
| L966 | `prepared.warning().is_some()` | `matches!(prepared, PreparedRunCommand::UnsandboxedFallback { .. })` |
| L969 | `!prepared.requires_host_sandbox()` | `matches!(prepared, PreparedRunCommand::UnsandboxedFallback { .. })` |
| L982 | `prepared.requires_host_sandbox()` | `matches!(prepared, PreparedRunCommand::HostIsolated { .. })` |
| L1021 | `!prepared.requires_host_sandbox()` | `matches!(prepared, PreparedRunCommand::Direct { .. })` |

**Update `allow_with_warning_denied_without_env_var` test:**

L899-904: Pass `(program, args, ..., None)` (no `UnsandboxedOptIn` token) instead of `(PreparedRunCommand, ..., false)`.

**Update `passthrough_uses_shell_binary_as_program` test:**

L988-990: Replace `prepared.program()` / `prepared.args()` accessor calls with destructuring: `let PreparedRunCommand::Direct { program, args } = prepared else { panic!(...) };`.

## Verification Plan

### Automated Tests

- `cargo check` from workspace root -- verifies exhaustive matching across the entire project.
- `cargo test --manifest-path tools/Cargo.toml` -- runs all sandbox policy tests.

### Manual Verification

None required. Structural refactor -- the compiler proves correctness through exhaustive matching.

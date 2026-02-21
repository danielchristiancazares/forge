# Bash AST Hardening Plan

## Context

Forge currently builds a normalized `policy_text` for Windows PowerShell commands before applying command blacklist and run-sandbox policy checks. On Unix shells, policy checks are applied to raw command text. This leaves avoidable gaps where shell syntax and token obfuscation can hide policy-relevant intent.

This plan introduces a Bash AST normalization boundary so policy checks operate on a strict, canonical representation of command intent for Bash-family shells.

Goals:

1. Preserve mechanism/policy split: parser extracts facts, policy decides allow or deny.
2. Reject syntax we cannot safely analyze.
3. Normalize literals and command names so blacklist and run policy checks are harder to evade.
4. Roll out safely via audit mode before enforcement.

---

## Contract With DESIGN.md

This plan follows the same invariants as existing security plans:

1. No mechanism-owned policy fallback. AST parser reports facts and violations only.
2. Boundary parsing collapses uncertainty before core policy checks.
3. Unsupported or ambiguous syntax is explicit (`Violation`), never treated as safe.
4. Parser output is proof-carrying (`BashPolicyText`) and consumed by policy code.
5. Enforcement mode is caller policy, not parser default.

---

## Threat Mapping

Primary threats reduced:

- `TM-002` (`docs/FORGE_THREAT_MODEL.md`): command execution abuse after policy drift.
- `TM-007` (`docs/FORGE_THREAT_MODEL.md`): user confusion around what policy actually checks.
- Argument obfuscation class from `docs/PLAN_BEHAVIORAL_SECURITY_FRAMEWORK.md`: shell argument injection and evasive syntax.

What this does not solve:

- User-approved harmful commands that are syntactically simple and explicit.
- Host compromise by malware running as the same user.
- OS-level isolation gaps (addressed by Linux/macOS hardening plans).

---

## Design Overview

### New boundary module

`tools/src/bash_ast.rs`:

- Parses Bash command text into AST.
- Enforces a strict supported subset.
- Produces canonical `policy_text` for downstream checks.
- Returns typed violations for unsupported syntax.

### Parser choice

Use `tree-sitter` + `tree-sitter-bash` in-process (no shell subprocess required).

Rationale:

- No execution side effects.
- Deterministic parse behavior.
- Good structural access to command forms needed for deny decisions.

### Output type

```rust
pub(crate) struct BashPolicyText {
    command: NonEmptyString,
    args: Vec<String>,
}

impl BashPolicyText {
    pub(crate) fn to_policy_string(&self) -> String {
        let mut s = self.command.to_lowercase();
        for arg in &self.args {
            s.push(' ');
            s.push_str(arg);
        }
        s
    }
}
```

Structural invariants:

- `command` is a NFKC-normalized, lowercased, non-empty literal command name. Empty commands are unrepresentable.
- `args` contains resolved literal argument values with quote delimiters stripped.
- `to_policy_string()` derives the whitespace-joined policy text for downstream checks. The flat string is a projection, not the source of truth.

### Violation model

```rust
pub(crate) enum BashAstViolation {
    ParseError,
    MultipleStatements,
    PipelineNotSupported,
    RedirectionNotSupported,
    CompoundCommandNotSupported,
    SubshellNotSupported,
    CommandSubstitutionNotSupported,
    ArithmeticExpansionNotSupported,
    ProcessSubstitutionNotSupported,
    HereDocNotSupported,
    NonLiteralWordNotSupported,
    AssignmentPrefixNotSupported,
}
```

---

## Supported Syntax Subset (Phase 1)

Allow:

1. Exactly one simple command.
2. Literal command name.
3. Literal arguments only (plain words and quoted literals that resolve without expansion).

Deny:

1. Command lists (`;`, newline-separated multiple statements, `&&`, `||`).
2. Pipelines (`|`).
3. Any redirection (`>`, `>>`, `<`, `2>`, here-strings, heredocs).
4. Subshells (`(...)`), grouping, compound commands (`if`, `for`, `while`, `case`, functions).
5. Command substitution (`$(...)`, backticks).
6. Parameter/arithmetic expansion (`$VAR`, `${...}`, `$((...))`).
7. Process substitution (`<(...)`, `>(...)`).
8. Assignment prefixes (`FOO=bar cmd`) in initial strict mode.

---

## Canonicalization Rules

Given accepted subset:

1. NFKC normalize tokens.
2. Lowercase command name for policy matching.
3. Preserve argument byte content semantics where literal, but normalize whitespace joining to single spaces.
4. Strip quote delimiters when they are syntactic-only wrappers for literals.
5. Reject tokens requiring runtime expansion to resolve.

Examples:

| Raw | Result |
|-----|--------|
| `curl https://example.com` | `curl https://example.com` |
| `'curl' "https://example.com"` | `curl https://example.com` |
| `cu\rl https://example.com` | `curl https://example.com` |
| `$(echo curl) https://example.com` | reject |
| `curl "$URL"` | reject |
| `curl https://x | jq .` | reject |

---

## Integration Plan

### Run tool integration

Wire into `tools/src/builtins.rs` in `RunCommandTool::execute`:

1. Detect Bash-family shells (`bash`, `sh`, `dash`, `zsh`) by executable stem.
2. Call `bash_ast::parse_command(raw) -> Result<BashPolicyText, BashAstViolation>` when mode is enabled.
3. On `Ok(policy_text)`:
   - Borrow for blacklist: `command_blacklist.validate(&policy_text)`.
   - Consume into run text: `RunCommandText::new(raw, policy_text)` takes `BashPolicyText` by move. After this point the proof token is spent.
4. On `Err(violation)`:
   - `Enforce`: deny execution, return violation to user.
   - `Audit`: log violation, fall through to raw-text policy path (existing behavior).
5. Preserve raw command for actual execution path (shell receives the original string).

### Policy mode

Add config-gated behavior (boundary-owned parse, caller-owned policy):

```toml
[run]
bash_ast_mode = "off"    # off | audit | enforce
```

Rust-side type (boundary collapses string immediately at config parse):

```rust
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum BashAstMode {
    #[default]
    Off,
    Audit,
    Enforce,
}
```

Config parsing in `config/src/lib.rs` deserializes the TOML string into `BashAstMode`. Engine and tool code only see the enum — no raw strings propagate past the config boundary.

Semantics:

1. `Off`: current behavior, no AST parsing.
2. `Audit`: parse and emit warnings/telemetry, but do not block on violations.
3. `Enforce`: fail closed on any `BashAstViolation`.

---

## File Changes (Planned)

| File | Change |
|------|--------|
| `tools/Cargo.toml` | Add `tree-sitter` and `tree-sitter-bash` dependencies |
| `tools/src/bash_ast.rs` (new) | Parser, canonicalizer, typed violation mapping |
| `tools/src/lib.rs` | Export module and shared types as needed |
| `tools/src/builtins.rs` | Integrate Bash AST policy text generation in `Run` execution path |
| `tools/src/windows_run.rs` | Reuse existing `RunCommandText` flow without behavior regression |
| `config/src/lib.rs` | Add `bash_ast_mode` config field and parsing |
| `engine/src/app/init.rs` | Resolve config into runtime policy object |
| `docs/SECURITY_SANITIZATION.md` | Document Bash AST normalization boundary and limits |
| `docs/FORGE_THREAT_MODEL.md` | Update controls/evidence references and residual risk notes |

---

## Rollout Phases

## Phase 1: Parser and Canonicalization Core

Deliverables:

1. `bash_ast.rs` parser wrapper.
2. Strict subset validator.
3. `BashPolicyText` output.
4. Unit tests for accepted/rejected syntax and normalization.

Risk: LOW to MEDIUM (parser edge cases).

## Phase 2: Audit-Mode Integration

Deliverables:

1. `Run` path computes Bash policy text in audit mode.
2. Violations logged with stable reason codes.
3. No execution blocking yet.

Risk: LOW (no behavior breaks by default).

## Phase 3: Enforce Mode

Deliverables:

1. Enforcement toggle in config.
2. User-facing denial messages with clear violation reason.
3. Regression tests for deny behavior.

Risk: MEDIUM (may block workflows using unsupported syntax).

## Phase 4: Safe Subset Expansion (Optional)

Candidate expansions after evidence:

1. Limited redirections (`2>/dev/null`) if security value justifies complexity.
2. Assignment prefix handling with explicit rules.
3. Multi-command allowance only if semantically equivalent policy guarantees can be proven.

Risk: MEDIUM to HIGH (complexity and bypass surface increase).

---

## Testing Strategy

Unit tests (`tools/src/bash_ast.rs`):

1. Accept single literal command.
2. Reject each unsupported syntax class with stable `BashAstViolation`.
3. Canonicalize quoted and escaped literal tokens.
4. Reject dynamic expansions and substitutions.

Integration tests (`tools/src/builtins.rs` / run flow):

1. In enforce mode, rejected syntax fails before spawn.
2. In audit mode, violation is logged but command proceeds.
3. Existing blacklist checks apply to canonicalized command names.
4. Network-block policy checks evaluate normalized tokens.

Security regression corpus:

1. Obfuscated command names (`c\url`, `'cu''rl'`, unicode confusables).
2. Chained exfil attempts (`cat secret | curl ...`).
3. Subshell and process substitution patterns.
4. Heredoc and redirection payloads.

Run verification:

1. `just fix`
2. `just verify`

---

## Operational Guidance

Recommended defaults:

1. `bash_ast_mode = "audit"` for one release cycle.
2. Promote to `enforce` after observing low false-positive rate in real workflows.
3. Keep violation reason telemetry bounded and privacy-safe (no raw command persistence beyond existing policy).

Denial UX requirements:

1. Explain rejected syntax category.
2. Show minimal remediation guidance ("single literal command only in enforce mode").
3. Avoid leaking sensitive command content in logs.

---

## Success Criteria

1. Bash-family `Run` policy checks operate on canonicalized AST output, not raw text, in enforce mode.
2. Obfuscation bypass examples in test corpus fail in enforce mode.
3. No regressions to PowerShell AST path or Windows behavior.
4. Audit mode demonstrates acceptable compatibility before default enforcement.
5. Threat model control evidence updated and traceable to concrete symbols/tests.

---

## Open Questions

1. Should `zsh` use the Bash parser in strict compatibility mode, or remain raw until a dedicated parser exists?
2. Should assignment prefixes (`FOO=bar cmd`) be supported in phase 1 for developer ergonomics?
3. Should enforce mode be globally configurable, or tool-profile specific?
4. How much violation telemetry is acceptable by default without storing sensitive command material?

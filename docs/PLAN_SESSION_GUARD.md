# Session Guard Plan (Idle Timeout + Startup Auth)

## Context

Forge is a local-first TUI that can execute privileged tools. For this threat model, two practical controls provide strong real-world value with manageable complexity:

1. End sessions automatically when the app is idle too long.
2. Require an explicit unlock step at startup before privileged interaction.

This plan defines a staged implementation for session guarding with:

- Idle auto-exit or lock behavior.
- Startup authentication (passphrase-first).
- Optional TOTP support as an additive control.

The plan is intentionally pragmatic: reduce easy walk-up misuse and accidental exposure while acknowledging that same-user malware compromise is out of scope for true prevention.

---

## Security Goals

1. Reduce unattended terminal risk (walk-away sessions).
2. Prevent immediate access to persisted session state and tool execution after app launch without local user intent.
3. Add friction before high-risk actions when configured.
4. Preserve developer ergonomics with explicit, configurable policy.

---

## Non-Goals

1. Defeating same-user malware compromise.
2. Creating cloud account identity or remote MFA infrastructure.
3. Replacing OS-level disk encryption, screen lock, or endpoint controls.

---

## Contract With DESIGN.md

1. Policy and mechanism remain separated: guard policy is configuration; enforcement is in explicit session/auth state transitions.
2. No implicit fallback from locked to unlocked state.
3. Authentication result is represented as typed state, not ad-hoc booleans.
4. Boundary parsing collapses config uncertainty at init time.
5. Unsafe recovery/bypass actions are explicit and auditable.

---

## Threat Mapping

Most relevant from `docs/FORGE_THREAT_MODEL.md`:

- `TM-002`: social engineering + policy weakening (adds friction around sensitive actions).
- `TM-003`: local history/session exposure (limits unattended exposure window).
- `TM-007`: approval confusion / rushed action patterns (optional step-up auth can reduce accidental high-risk approval).

Residual risk:

- Same-user compromise remains high-impact regardless of local OTP/passphrase.

---

## User-Facing Policy Surface

Proposed config additions:

```toml
[security]
idle_timeout_secs = 900               # 0 disables idle guard
idle_action = "lock"                  # lock | exit
lock_on_startup = true
auth_mode = "passphrase"              # none | passphrase | totp
auth_ttl_secs = 1800                  # re-auth window for optional step-up
require_step_up_for = ["Run"]         # optional future extension
```

Passphrase/TOTP material:

- Prefer OS keychain-backed secret storage where possible.
- Provide explicit fallback path if keychain is unavailable.
- Never log plaintext secrets.

---

## State Model

Introduce explicit session guard state:

```rust
enum SessionGuardState {
    Unlocked { authenticated_at: Instant },
    Locked { reason: LockReason },
}

enum LockReason {
    Startup,
    IdleTimeout,
    ManualLock,
    ReauthRequired,
}
```

Rules:

1. Tool execution is denied while locked.
2. Message send is denied while locked (configurable later if read-only mode is desired).
3. Unlock transition requires successful auth per active `auth_mode`.

---

## Architecture and Integration Points

| Area | Planned change |
|------|----------------|
| `config/src/lib.rs` | Add `[security]` schema + parse/validation defaults |
| `engine/src/app/init.rs` | Resolve security config into runtime session guard policy |
| `engine/src/app/mod.rs` | Add guard state, lock/unlock transitions, idle timer hooks |
| `engine/src/app/tool_loop.rs` | Hard gate tool execution when locked; optional step-up checks |
| `engine/src/app/input_modes.rs` | Block send/queue while locked; route unlock input mode |
| `tui/src/lib.rs` + `tui/src/input.rs` | Locked overlay/modal and unlock input handling |
| `engine/src/app/persistence.rs` | Ensure lock state itself is not persisted as unlocked proof |
| `docs/FORGE_THREAT_MODEL.md` | Update controls and residual risk notes |

---

## Phased Rollout

## Phase 1: Idle Timeout Guard (No Auth Yet)

Deliverables:

1. Idle tracking on input/activity boundaries.
2. Configurable idle action: `exit` or `lock`.
3. Manual `/lock` command.
4. Clear UI indicator before timeout (optional warning threshold).

Acceptance:

1. With `idle_timeout_secs > 0`, app consistently locks/exits after inactivity.
2. Activity resets timer.
3. No tool execution occurs after lock until unlocked (if lock mode used).

Risk: LOW.

## Phase 2: Startup Passphrase Auth

Deliverables:

1. `lock_on_startup` support.
2. `auth_mode = passphrase`.
3. Passphrase setup/update flow (single-user local flow).
4. Secure verification path (constant-time compare, redacted logs, no plaintext persistence).

Acceptance:

1. Startup enters locked state when enabled.
2. Wrong passphrase does not unlock; rate-limited retries.
3. Correct passphrase unlocks and records auth timestamp.

Risk: LOW to MEDIUM (UX and recovery handling).

## Phase 3: Optional TOTP Mode

Deliverables:

1. `auth_mode = totp`.
2. Enrollment flow (secret generation, QR/otpauth URI display, confirmation code).
3. Verification with clock-skew tolerance and replay window checks.
4. Recovery mechanism (backup codes or explicit local admin reset flow).

Acceptance:

1. Valid TOTP unlocks within skew window.
2. Reused code is rejected within replay horizon.
3. Recovery path is explicit and auditable.

Risk: MEDIUM.

## Phase 4: Step-Up Auth for High-Risk Actions (Optional)

Deliverables:

1. `auth_ttl_secs` and `require_step_up_for`.
2. Re-auth prompt before configured high-risk actions (for example `Run`).
3. Audit metadata for overrides and step-up events.

Acceptance:

1. High-risk action denied when re-auth required and not satisfied.
2. Re-auth success unlocks action for TTL window.

Risk: MEDIUM (workflow friction).

---

## Secret Storage Strategy

Preferred:

1. OS keychain/credential manager for passphrase-derived verifier and TOTP seed.

Fallback:

1. Local encrypted blob protected by user-provided passphrase at startup.
2. Explicit warnings when falling back to non-keychain storage.

Invariant:

1. No plaintext secrets in config, logs, history, or journals.

---

## Failure and Recovery Design

1. Configurable max attempts + backoff for unlock failures.
2. Explicit local recovery path:
   - CLI flag or environment gate for emergency reset.
   - Must display clear security warning and require local confirmation.
3. Never silently disable auth due to parse/storage failures; fail closed with actionable error.

---

## Testing Strategy

Unit tests:

1. Security config parsing defaults and invalid value handling.
2. Lock state transition correctness.
3. Idle timer boundary behavior.
4. Passphrase/TOTP verifier behavior and failure modes.

Integration tests:

1. Startup lock blocks send/tools until unlock.
2. Idle timeout enforces lock/exit as configured.
3. Tool loop rejects execution while locked.
4. Step-up auth gating for configured tools.

Security tests:

1. No secret leakage in debug output/log paths.
2. Replay prevention for TOTP codes.
3. Rate-limit behavior under repeated failed attempts.

Workflow:

1. `just fix`
2. `just verify` (for code-bearing phases)

---

## Operational Rollout Recommendation

1. Ship Phase 1 + Phase 2 first (`passphrase` default option, `totp` optional).
2. Keep TOTP opt-in initially.
3. Observe false-lock and usability pain before enabling step-up controls broadly.

Suggested defaults for first secure profile:

```toml
[security]
idle_timeout_secs = 900
idle_action = "lock"
lock_on_startup = true
auth_mode = "passphrase"
auth_ttl_secs = 1800
```

---

## Success Criteria

1. Idle unattended sessions are automatically locked/exited.
2. Startup access requires explicit user authentication when configured.
3. Tool execution cannot proceed from locked state.
4. Secrets for auth are never persisted or displayed in plaintext.
5. Threat model reflects reduced unattended-access risk and explicit residual compromise limits.

---

## Open Questions

1. Should `exit` be the default idle action for high-security profiles, with `lock` as standard profile default?
2. Is TOTP enrollment in-TUI sufficient, or should setup use an explicit CLI flow?
3. Should backup codes be mandatory for TOTP mode?
4. Should step-up auth apply only to `Run` first, or include Git mutators and write tools?

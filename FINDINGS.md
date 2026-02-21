# Security Findings Remediation Plan

Date: 2026-02-21
Repository: `forge`

## Executive Summary

This plan remediates four confirmed security gaps:

1. Plaintext persistence of conversation history.
2. Missing Windows ACL hardening for app data directories.
3. Linux/BSD `Run` execution without OS-level sandboxing.
4. Missing Windows ACL hardening for WebFetch cache.

Priority order: **F1 -> F3 -> F2/F4**.

---

## Finding Register

| ID | Severity | Title | Primary Code References |
|---|---|---|---|
| F1 | High | History contains sensitive content at rest | `engine/src/app/persistence.rs:41`, `context/src/manager.rs:419`, `context/src/history.rs:45`, `types/src/proofs.rs:269` |
| F2 | Medium-High | Windows data dir ACLs not explicitly hardened | `engine/src/app/init.rs:502`, `utils/src/windows_acl.rs:151` |
| F3 | High | Linux/BSD `Run` path has no OS sandbox | `tools/src/windows_run.rs:241`, `tools/src/windows_run.rs:259`, `tools/src/windows_run.rs:347` |
| F4 | Medium | WebFetch cache ACL hardening missing on Windows | `tools/src/webfetch/cache.rs:130`, `tools/src/webfetch/cache.rs:245`, `tools/src/webfetch/cache.rs:248` |

---

## F1 - Concrete Remediation (History Protection)

### Goal

Prevent sensitive history data from being readable in plaintext on disk, even if files are copied.

### Design Decision

Use **defense in depth**:

1. **Redact known secrets before persistence** (limits blast radius if encryption fails or key is compromised).
2. **Encrypt history at rest** with a per-user key.

This directly addresses the concern that "the key is on the same machine": yes, but OS-protected key material (DPAPI/Keychain/Secret Service) is still materially better than plaintext files.

### Implementation Tasks

1. Add secure history codec:
   - New module: `context/src/history_crypto.rs` (or equivalent in `utils`).
   - Format: versioned envelope (`magic`, `version`, `nonce`, `ciphertext`, `tag`).
   - Cipher: AEAD (e.g., XChaCha20-Poly1305 or AES-256-GCM).
2. Add key management abstraction:
   - New trait + backend module (e.g., `utils/src/secure_key_store.rs`).
   - Windows: DPAPI (user scope).
   - macOS: Keychain.
   - Linux: Secret Service; fallback policy must be explicit (deny persistence or user passphrase).
3. Wire into history save/load:
   - Update `context/src/manager.rs:419` and `context/src/manager.rs:435` to encrypt/decrypt history payloads.
4. Add redaction before serialization:
   - Add `Message::redacted_for_persistence()` in `types/src/message.rs`.
   - Apply in `context/src/history.rs:45` before writing.
   - Reuse `forge_utils::sanitize_display_text`/redaction utilities so behavior is consistent with journaling.
5. Migration path:
   - On startup, detect plaintext `history.json`.
   - Load -> redact -> encrypt -> atomically replace plaintext.
   - Keep one-time `.bak` rollback for failed migration only.
6. Failure behavior:
   - If decryption key unavailable, fail closed with a user-facing error (do not silently write plaintext fallback).

### Tests

1. `context` unit tests:
   - Encrypt/decrypt round-trip.
   - Wrong key fails.
   - Tamper detection fails.
2. Migration tests:
   - Plaintext file migrates once and no longer persists plaintext.
3. Secret persistence tests:
   - Confirm known secret patterns are not present in stored blob plaintext surface.

### Acceptance Criteria

- `history.json` no longer contains readable message text.
- Known key patterns (`sk-`, bearer/JWT, etc.) do not appear in persisted artifacts.
- Existing users can migrate without losing history.

---

## F3 - Concrete Remediation (Linux/BSD Run Sandbox Gap)

### Goal

Ensure `Run` cannot execute unsandboxed commands on Linux/BSD when enabled.

### Immediate Fix (Phase 1)

1. Fail closed on Linux/BSD in `tools/src/windows_run.rs`:
   - Replace current token-only baseline path (`tools/src/windows_run.rs:259`) with:
     - deny execution when no supported Linux/BSD sandbox backend is available.
2. Preserve explicit approvals and keep `Run` denylisted by default:
   - Current denylist default in `engine/src/app/init.rs:723` remains.
3. Add explicit runtime warning when user config allowlists `Run` on Linux/BSD without sandbox backend.

### Full Fix (Phase 2)

1. Add Linux sandbox backend:
   - New module: `tools/src/linux_run.rs`.
   - Backend priority: `bubblewrap` (or equivalent), with no-network default and restricted filesystem mounts.
2. Extend run policy model:
   - Add Linux/BSD policy in `config/src/lib.rs` and `tools/src/windows_run.rs` replacement type.
   - Include fallback mode semantics parallel to Windows/macOS (`deny` default).
3. Integrate with `RunCommandTool` path in `tools/src/builtins.rs:1104`.

### Tests

1. Unit tests for Linux/BSD:
   - sandbox unavailable -> deny.
   - fallback mode behavior.
2. Integration tests:
   - command runs only with sandbox backend.
   - network/file access constraints enforced in sandbox mode.

### Acceptance Criteria

- Linux/BSD `Run` cannot execute unsandboxed by default.
- If sandbox backend missing, execution is denied with actionable error text.

---

## F2 - Concrete Remediation (Windows Data Dir ACLs)

### Goal

Ensure app data directories and sensitive files are owner-only on Windows.

### Implementation Tasks

1. Harden directory setup:
   - Update `engine/src/app/init.rs:502` (`ensure_secure_dir`) to apply `forge_utils::set_owner_only_dir_acl` on Windows (best effort with warning on failure).
2. Harden sensitive file writes consistently:
   - Update `utils/src/atomic_write.rs` for `PersistMode::SensitiveOwnerOnly` on Windows to call `set_owner_only_file_acl` after persist.
   - This centralizes protection for history/session/plan writes that already use sensitive mode.
3. Post-write verification hooks:
   - For critical files (`history.json`, `session.json`, `plan.json`), add optional post-write ACL verification/logging in `engine/src/app/persistence.rs`.

### Tests

1. Windows-only tests for ACL helper invocation on temp dirs/files.
2. Regression tests for atomic write behavior (no data loss, ACL applied best effort).

### Acceptance Criteria

- New sensitive files created by Forge on Windows are owner-only by ACL.
- Data directory creation path attempts owner-only ACL hardening and logs failures.

---

## F4 - Concrete Remediation (WebFetch Cache ACLs on Windows)

### Goal

Prevent cached WebFetch payloads from being readable by other local users on Windows.

### Implementation Tasks

1. Cache directory ACL hardening:
   - In `tools/src/webfetch/cache.rs:130`, apply `set_owner_only_dir_acl` on Windows after directory creation.
2. Cache file ACL hardening:
   - In `tools/src/webfetch/cache.rs:248`, change cache writes to `PersistMode::SensitiveOwnerOnly`.
   - Ensure parent subdirectory creation path also applies owner-only ACL on Windows.
3. Optional hygiene:
   - Add config switch to disable disk cache for high-sensitivity environments.

### Tests

1. Windows cache tests:
   - New cache dir/file gets hardened ACL (best effort behavior verified).
2. Existing cache behavior regression:
   - Put/get/evict remains unchanged functionally.

### Acceptance Criteria

- Cache entries are not written with default permissive ACLs on Windows.
- Existing WebFetch cache functionality remains intact.

---

## Delivery Plan

### Milestone 1 (Security Hotfix)

- Ship F3 Phase 1 fail-closed Linux/BSD behavior.
- Ship F2/F4 ACL hardening changes.
- Add release note calling out stricter `Run` behavior on Linux/BSD.

### Milestone 2 (At-Rest Data Protection)

- Ship F1 encryption + redaction + migration.
- Add recovery UX for key-unavailable state.
- Add documentation for key backend behavior by OS.

### Milestone 3 (Hardening + Observability)

- Add Linux sandbox backend for `Run`.
- Add telemetry/warnings for security fallback events (without secret content).

---

## Validation Checklist (Release Gate)

- `just fix`
- `just verify`
- New/updated tests pass on Windows and Linux CI lanes.
- Manual checks:
  - history file not plaintext,
  - Linux `Run` denied without sandbox,
  - Windows ACL hardening warnings are actionable and non-fatal.

---

## Rollback Plan

1. Feature-flag encrypted history read/write path (`history.encryption_enabled`).
2. Preserve one backup migration artifact per user profile.
3. If encryption regression occurs, disable encryption flag but keep redaction path active.


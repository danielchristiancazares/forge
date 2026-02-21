# Forge Utils (`forge-utils`)

`forge-utils` contains shared infrastructure utilities reused across workspace crates.

## Responsibilities

- Crash-safe atomic file persistence.
- Security-oriented string redaction and display sanitization helpers.
- Unified diff formatting and diff-stat utilities.
- Platform ACL helpers for owner-only file and directory protections.

## Module Map

- `atomic_write.rs`: temp-file + rename atomic persistence helpers.
- `security.rs`: secret redaction and stream/display sanitization utilities.
- `diff.rs`: unified diff generation and compact diff statistics.
- `windows_acl.rs`: owner-only ACL setup helpers.

## Re-exported API

`forge-utils` re-exports the primary helpers from each module through `lib.rs`, so consumers can depend on a small, stable utility surface without importing module internals directly.

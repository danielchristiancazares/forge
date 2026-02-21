# Forge Core (`forge-core`)

`forge-core` contains shared domain-focused modules extracted from engine orchestration.

## Responsibilities

- Define UI display-log data structures (`DisplayItem`, `DisplayLog`).
- Provide environment-context assembly helpers used to build system prompts.
- Provide trusted system notification queue/types.
- Provide core thinking payload representation.
- Provide shared boundary utilities for model parsing, API-key wrapping, and stream error sanitization.

## Module Map

- `display.rs`: display log and display item primitives.
- `env_context.rs` / `environment.rs`: environment snapshot + prompt assembly helpers.
- `notifications.rs`: queue-backed notification types.
- `thinking.rs`: explicit thinking payload sum type.
- `errors.rs`: stream error extraction helpers.
- `security.rs`: display sanitization re-exports.
- `util.rs`: model parsing and API-key wrapping helpers.

## Design Intent

This crate is a dependency-safe core layer for logic reused across crates, while keeping heavy runtime orchestration in `forge-engine`.

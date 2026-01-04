# Repository Guidelines

## Project Structure & Module Organization
- `Cargo.toml` / `Cargo.lock`: crate metadata and locked dependencies.
- `src/`: application code
  - `main.rs`: terminal setup + async main loop
  - `app.rs`: core state (`App`) and command handling
  - `input.rs`: keyboard/mode handling (`Normal`/`Insert`/`Command`)
  - `ui.rs`: Ratatui rendering and layout
  - `message.rs`: conversation/message types
  - `theme.rs`: Claude-inspired colors/styles
- `target/`: Cargo build output (generated; don’t edit by hand).

## Build, Test, and Development Commands
- `cargo run`: run the TUI locally.
- `cargo check`: fast compile/type-check during development.
- `cargo build --release`: optimized build in `target/release/`.
- `cargo test`: run tests (add them as the project grows).
- `cargo fmt`: format with rustfmt.
- `cargo clippy -- -D warnings`: lint and fail on warnings.

Configuration:
- Set `ANTHROPIC_API_KEY` to enable API-backed behavior (the app reads it on startup).
- PowerShell example: `$env:ANTHROPIC_API_KEY="..."; $env:RUST_LOG="info"; cargo run`.
- Set `FORGE_CONTEXT_INFINITY=0` to disable ContextInfinity (no summarization; basic history truncation).

## Coding Style & Naming Conventions
- Rust edition: 2024; keep compatibility with Rust `1.92+` (see `Cargo.toml`).
- Follow standard Rust conventions: `snake_case` (modules/functions), `CamelCase` (types), `SCREAMING_SNAKE_CASE` (consts).
- Keep rendering concerns in `ui.rs`; keep state transitions and input behavior in `app.rs` / `input.rs`.

## Testing Guidelines
- No dedicated test suite yet.
- Prefer unit tests in `src/*` with `#[cfg(test)]`; use `tests/` for integration tests.
- Use `#[tokio::test]` for async code and keep tests deterministic.

## Commit & Pull Request Guidelines
- Git metadata isn’t included in this workspace; use a Conventional Commit style: `type(scope): summary` (e.g., `feat(ui): add model command`).
- PRs should include: intent (“what/why”), how to run/verify, and screenshots/GIFs for UI changes.
- Never commit secrets (API keys, tokens); use environment variables instead.

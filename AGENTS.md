# Repository Guidelines

## Project Structure & Module Organization
- `Cargo.toml` / `Cargo.lock`: workspace + crate metadata and locked dependencies.
- `src/`: binary entrypoint and assets
  - `main.rs`: terminal setup + async main loop
  - `assets.rs`: bundled prompt/assets
- `crates/forge-engine/`: core application state + command handling
  - `lib.rs`: `App`, input state machine, commands, model selection
  - `config.rs`: config parsing
- `crates/forge-tui/`: TUI rendering + input handling
  - `lib.rs`: full-screen rendering + overlays (command palette, model picker)
  - `ui_inline.rs`: inline mode rendering
  - `input.rs`: crossterm key handling
  - `theme.rs`: Claude-inspired colors/styles
  - `markdown.rs`: markdown rendering
  - `effects.rs`: lightweight modal effects
- `crates/forge-context/`: ContextInfinity (history, summarization, journals)
- `crates/forge-providers/`: provider HTTP/SSE implementations
- `crates/forge-types/`: shared domain types
- `target/`: Cargo build output (generated; don’t edit by hand).

## Build, Test, and Development Commands
- `cargo run`: run the TUI locally.
- `cargo check`: fast compile/type-check during development.
- `cargo build --release`: optimized build in `target/release/`.
- `cargo test`: run tests (add them as the project grows).
- `cargo fmt`: format with rustfmt.
- `cargo clippy -- -D warnings`: lint and fail on warnings.

Configuration:
- Config file: `~/.forge/config.toml` (preferred; supports `${ENV_VAR}` expansion for API keys).
- Env fallback: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` are used if config keys are empty or missing.
- Deprecated: `[cache]` and `[thinking]` are legacy; prefer `[anthropic]` for Claude cache/thinking defaults.
- Sample config:
```
[app]
provider = "claude"
model = "claude-sonnet-4-5-20250929"
# ui mode: "full" or "inline"
tui = "inline"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"

[context]
infinity = true

[anthropic]
cache_enabled = true
thinking_enabled = false
thinking_budget_tokens = 10000

[openai]
reasoning_effort = "high"
verbosity = "high"
truncation = "auto"
```
- PowerShell example: `$env:ANTHROPIC_API_KEY="..."; $env:RUST_LOG="info"; cargo run`.
- Set `FORGE_CONTEXT_INFINITY=0` to disable ContextInfinity (no summarization; basic history truncation).
- OpenAI provider rejects non-GPT-5.x models (GPT-5.x is the minimum).

## Development Considerations (Non-Exhaustive, Work In Progress)
- When implementing a TUI feature, you must maintain parity with both inline and alternate screen views

## Coding Style & Naming Conventions
- Rust edition: 2024; keep compatibility with Rust `1.92+` (see `Cargo.toml`).
- Follow standard Rust conventions: `snake_case` (modules/functions), `CamelCase` (types), `SCREAMING_SNAKE_CASE` (consts).
- Keep rendering concerns in `crates/forge-tui/src/lib.rs` / `crates/forge-tui/src/ui_inline.rs`.
- Keep state transitions and input behavior in `crates/forge-engine/src/lib.rs` (state) and `crates/forge-tui/src/input.rs` (key handling).

## Testing Guidelines
- No dedicated test suite yet.
- Prefer unit tests alongside crate modules in `crates/*/src` with `#[cfg(test)]`; use `tests/` for integration tests.
- Use `#[tokio::test]` for async code and keep tests deterministic.

## Commit & Pull Request Guidelines
- Git metadata isn’t included in this workspace; use a Conventional Commit style: `type(scope): summary` (e.g., `feat(ui): add model command`).
- PRs should include: intent (“what/why”), how to run/verify, and screenshots/GIFs for UI changes.
- Never commit secrets (API keys, tokens); use environment variables instead.

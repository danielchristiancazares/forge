# Repository Guidelines

## Project Structure & Module Organization
Forge is a Rust Cargo workspace split into focused crates. Key paths:
- `cli/` entry point and terminal session
- `engine/` state machines, commands, tool executor
- `tui/` rendering and input handling
- `context/` Context Infinity, token budgeting, SQLite journals
- `providers/` LLM clients (Claude/OpenAI/Gemini)
- `types/` shared domain types (no IO)
- `webfetch/` URL fetching and parsing
- `tests/` integration and snapshot tests (`tests/snapshots/`)
- `docs/` architecture notes; `scripts/` helper tools; `cli/assets/` embedded prompts

## Build, Test, and Development Commands
- `just --list` show available recipes; `just check|build|release|test` map to cargo.
- `cargo run --release` run the TUI locally.
- `just fmt` / `cargo fmt` format; `just lint` / `cargo clippy --workspace --all-targets -- -D warnings` lint.
- `just cov` or `cargo cov` for coverage (requires cargo-llvm-cov).

## Coding Style & Naming Conventions
- Rust 2024 edition; follow rustfmt defaults and clippy settings in `clippy.toml`.
- Prefer type-driven invariants (see `DESIGN.md` and `INVARIANT_FIRST_ARCHITECTURE.md`); make invalid states unrepresentable.
- Naming: crates use `forge-*`, modules are snake_case, types are PascalCase, tests mirror module names.

## Testing Guidelines
- Run the full suite: `cargo test`.
- Integration aggregator: `cargo test --test all`.
- UI snapshots: `cargo test --test ui_snapshots` (uses `insta` snapshots in `tests/snapshots/`).
- Keep coverage stable or improving; use `cargo cov` when touching core logic.

## Commit & Pull Request Guidelines
- Commit format follows Conventional Commits: `type(scope): summary` (scope optional). Examples: `feat(engine): add rewind`, `fix: resolve clippy lints`.
- Keep commits cohesive; run `just pre-commit` before pushing.
- PRs should include a short summary, test results, and screenshots for TUI-visible changes; link related issues when applicable.

## Configuration & Secrets
- Local config lives in `~/.forge/config.toml`; API keys come from env vars like `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`.
- Never commit real tokens or local config files.

## Agent Notes
- Extended agent and architecture guidance lives in `CLAUDE.md` and `GEMINI.md`.

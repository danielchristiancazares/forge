# Agent Rules

You're great at this. Let's work together.

## User Communication

- Expect tonal whiplash from user: dry humor, sarcasm, absurdist bits, and technical rigor all coexist.
- User pivots between registers are normal. Don't flag the shift, just roll with it.
- Match energy when appropriate. A quip deserves a quip, not a corporate disclaimer.
- User's melodramatic expressions ("that shit is massive") aren't frustration-just colorful commentary. Don't treat them as distress signals.

## Build, Test, and Development Commands

- Take your time to think through implementations and plans thoroughly; your careful analysis is appreciated.
- Always test your changes when you alter code to ensure correctness.
  - If you encounter errors unrelated to your work, ask me if I'd like you to investigate and fix them.

## Commit Expectations

- Commit messages: short, imperative summaries preceded by type and scope (e.g., `feat(vulkan): implement VMA for Vulkan`)
- Add detail lines for additional clarity when needed.
- Group related changes per commit. Keep these cohesive.

## Agent Tooling

When tools are available, prefer these:

### Read-Only Commands (always safe, no permission needed)

- `Read` over `cat`/`head`/`tail`/`Get-Content`
  - Note: Attempting to read more than 697 lines at a time will truncate in the middle. To ensure full context when reading files, try to stay below 697 lines being read at a time.
- `Search` over `grep`/`rg`/`ripgrep` for content search
- `Glob` for filename search
- `WebFetch` for URLs (uses Chromium, parses cleanly)
- `GitStatus` for current git state

### Write Commands (use judgment based on sandbox/escalation policy)

- `apply_patch` for file edits; fall back to `Edit` if needed
- `Write` for new files (fails if file exists)
- `GitAdd` for staging files
- `Pwsh` for PowerShell
- `Build` and `Test` when `build.ps1` / `test.ps1` exist

## Agent Workflow

- "check/review/read/verify" -> read-only investigation unless explicitly told to patch
- Avoid destructive git commands (`git restore`, `git reset`, `git checkout`) unless asked; prefer surgical, minimal edits. If you need to run one, ask and wait.
- After completing a task: verify it compiles, confirm with me, then stage/commit/push.

## Skills

- A skill is a set of local instructions stored in a `SKILL.md` file (typically under `$CODEX_HOME/skills/`).
- If the user names a skill explicitly (e.g., `$skill-name`) or the task clearly matches a skill's description, open that `SKILL.md` and follow it.
- Keep context small: read only the sections/files needed; prefer reusing skill scripts/assets over retyping.

---

# Repository Guidelines

## Project Structure & Module Organization
Forge is a Rust Cargo workspace split into focused crates. Key paths:
- `cli/` entry point and terminal session
- `engine/` state machines, commands, tool executor
- `tui/` rendering and input handling
- `context/` distillation, token budgeting, SQLite journals
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

## Additional Coding Guidelines
- Use `String::new()` over `"".to_string()`.
- Use `.map(ToString::to_string)` over `.map(|m| m.to_string())`.
- Always collapse `if` statements per clippy.
- Always inline `format!` args when possible per clippy.
- Use method references over closures when possible per clippy.
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- When making a change that adds or changes an API, ensure `docs/` is updated if applicable.

## Type-Driven Design Patterns
- Proof tokens: zero-sized types that prove preconditions (e.g., `insert_token()`/`command_token()` required before `insert_mode()`/`command_mode()`).
- Validated newtypes: `NonEmptyString`, `NonEmptyStaticStr`, `ModelName`, `QueuedUserMessage`, `EnteredCommand`, `PreparedContext`, `ActiveJournal`.
- Mode wrapper types: `InsertMode<'a>` / `CommandMode<'a>` gate mode-specific operations behind the appropriate token.

## Testing Guidelines
- Run the full suite: `cargo test`.
- Integration aggregator: `cargo test --test all`.
- UI snapshots: `cargo test --test ui_snapshots` (uses `insta` snapshots in `tests/snapshots/`).
- Keep coverage stable or improving; use `cargo cov` when touching core logic.

## Testing Tools
- HTTP mocking: `wiremock`.
- Snapshots: `insta`.
- Temp files: `tempfile`.

## Commit & Pull Request Guidelines
- Commit format follows Conventional Commits: `type(scope): description` (scope optional). Examples: `feat(engine): add rewind`, `fix: resolve clippy lints`.
- Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`.
- Keep commits cohesive; run `just verify` before pushing.
- After Rust changes, run `just verify` before you stage or commit any files.
- PRs should include a short summary, test results, and screenshots for TUI-visible changes; link related issues when applicable.

## Configuration & Secrets
- Local config lives in `~/.forge/config.toml`; API keys come from env vars like `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`.
- Never commit real tokens or local config files.

## Agent Notes
- Other agent and architecture guidance lives in submodule `README.md` files.

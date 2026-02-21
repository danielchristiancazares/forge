# AGENTS.md

Forge is a vim-modal terminal user interface for interacting with Claude (Anthropic), GPT (OpenAI), and Gemini (Google), with adaptive context management and agentic tool execution.

This is the canonical repo-wide instruction source.

- `CLAUDE.md` and `GEMINI.md` import `@AGENTS.md`.
- Keep repo-level agent guidance centralized here.

## AGENTS.md Discovery and Injection

Forge runtime behavior (implemented in `core/src/environment.rs` and `engine/src/app/input_modes.rs`):

1. Discover `~/.forge/AGENTS.md` first (global instructions).
2. Discover ancestor `AGENTS.md` files from filesystem root down to current working directory.
3. Concatenate all discovered files, global first and most-specific last.
4. Cap total injected AGENTS content to 64 KB.
5. Prepend AGENTS content to the first outgoing user message only, then consume it.

## Environment

Forge is developed on multiple OSes. This repo's agent/test workflow should work from:

- Windows (PowerShell 7)
- macOS/Linux (zsh/bash)

When writing instructions or scripts, avoid assuming a single shell. If you need shell-specific syntax, call it out explicitly as **PowerShell** or **POSIX shell**.

## Rules

- Run `just verify` after code changes (pipeline includes `ifa`, `fix`, `fmt`, `lint`, `test`, and `cargo deny` checks).
- Run `just fix` after editing files (normalizes CRLF -> LF in `*.rs` and `*.md`).
- Avoid ad-hoc `cargo check` or `cargo test` in normal workflow; use `just` recipes unless debugging a narrow target.
- Never add trivial comments. Do not restate the obvious.
- Never decrease test coverage. Check with `just cov` when coverage is relevant.
- Update `docs/` when changing any public API or user-visible behavior.
- Update `ifa/` operational artifacts in the same change when invariants, authority boundaries, proofs, or parametricity rules change.
- Use `dirs::home_dir()` for config paths, not hardcoded `~/.forge/`. Display actual path via `config::config_path()`.
- Use `tracing::warn!` (or appropriate `tracing::*`) for diagnostics, never `eprintln!` (corrupts TUI output).

## Tooling Baseline

Built-in tool families available in Forge include:

- File operations: `Read`, `Write`, `Edit`, `Glob`
- Search: `Search`
- Shell: `Run`
- Web: `WebFetch`
- Context: `Recall`, `Memory`
- Git: `GitStatus`, `GitDiff`, `GitAdd`, `GitCommit`, `GitLog`, `GitBranch`, `GitCheckout`, `GitStash`, `GitShow`, `GitBlame`, `GitRestore`

## Rust Style

- `use` imports over qualified paths inlined
- `String::new()` not `"".to_string()`
- `.map(ToString::to_string)` not `.map(|m| m.to_string())`
- Method references over closures (`clippy::redundant_closure_for_method_calls`)
- Collapse every collapsible `if` with no exceptions; treat `clippy::collapsible_if` as a hard error. Never suppress it (`#[allow(clippy::collapsible_if)]` is forbidden).
- Inline `format!` args (`clippy::uninlined_format_args`)
- Test assertions: compare whole objects, not field-by-field
- Respect workspace lint invariants: `clippy::wildcard_imports = deny`, `clippy::absolute_paths = deny`

## IFA Artifacts

`ifa/` is operational, not optional. Keep these artifacts current when architecture rules change:

1. `ifa/invariant_registry.toml`
2. `ifa/authority_boundary_map.toml`
3. `ifa/parametricity_rules.toml`
4. `ifa/move_semantics_rules.toml`
5. `ifa/dry_proof_map.toml`
6. `ifa/classification_map.toml`

Use `just ifa-check` (included in `just verify`) to enforce artifact presence, schema, cross-file consistency, symbol path validity, and coverage rules.

## Shell Pitfalls (Windows)

These are **PowerShell-specific** pitfalls (and/or apply when running commands in a tool runner that does not preserve a working directory):

- Avoid `2>&1` redirection in suggested commands (stderr is already captured).
- Avoid `cd dir && command` patterns in automation; prefer `--manifest-path` (Cargo) or an explicit working directory option when available.
- Avoid `Push-Location`/`Set-Location` sequences; run commands directly.

## Shell Pitfalls (macOS/Linux)

- Prefer POSIX-compatible examples (zsh/bash). If a command needs GNU-only flags, mention it.
- In automation, prefer `--manifest-path` (Cargo) or an explicit working directory option rather than relying on `cd` state.

## Commands

```bash
just verify      # ifa + fix + fmt + lint + test + cargo-deny summary
just ifa-check   # enforce IFA operational artifacts
just fix         # clippy --fix + CRLF -> LF normalization
just check       # fast type-check
just cov         # coverage report
just deny        # advisories + bans + licenses + sources
just context     # regenerate CONTEXT.md
just digest      # regenerate DIGEST.md (nightly)
just zip         # regenerate source bundle + sha256

# targeted fallback commands (narrow debugging only)
cargo test test_name
cargo test -- --nocapture
cargo test --test integration_test
```

## Commit Workflow

- Never bypass GPG signing.

```bash
just verify && just fix
git add -A
git commit -S -m "type(scope): description"
git push
```

Conventional commits: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## Configuration

Config: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
model = "claude-opus-4-6"    # claude-* -> Claude, gpt-* -> OpenAI, gemini-* -> Gemini

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"

[context]
memory = true                # Librarian fact extraction/retrieval

[anthropic]
cache_enabled = true
thinking_enabled = false

[google]
thinking_enabled = true      # thinkingLevel="high" for Gemini 3 Pro
```

Env fallbacks and runtime knobs:

- API keys: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`
- Context default when `[context]` is absent: `FORGE_CONTEXT_INFINITY`
- Crash hardening opt-out: `FORGE_ALLOW_COREDUMPS`
- Streaming/tool journal tuning:
  - `FORGE_STREAM_IDLE_TIMEOUT_SECS`
  - `FORGE_STREAM_JOURNAL_FLUSH_THRESHOLD`
  - `FORGE_STREAM_JOURNAL_FLUSH_INTERVAL_MS`
  - `FORGE_STREAM_JOURNAL_FLUSH_BYTES`
  - `FORGE_TOOL_JOURNAL_FLUSH_BYTES`
  - `FORGE_TOOL_JOURNAL_FLUSH_INTERVAL_MS`

## Crates (11)

| Crate | Purpose |
|-------|---------|
| `cli` | Binary entry point, terminal session, event loop |
| `config` | Config schemas, parsing, resolution helpers, and persistence |
| `context` | Context window management, SQLite persistence, journaling |
| `core` | Core environment and cross-cutting domain boundaries |
| `engine` | App state machine, commands, tool execution orchestration |
| `lsp` | LSP client for language server diagnostics |
| `providers` | LLM API clients: Claude, OpenAI, Gemini |
| `tools` | Tool executor framework, built-in tools, URL fetch/extraction |
| `tui` | TUI rendering (ratatui), input handling, themes |
| `types` | Core domain types (no IO, no async) |
| `utils` | Shared utilities: atomic IO, security redaction, text diffing |

## Key Files

| Crate | File | Purpose |
|-------|------|---------|
| `cli` | `cli/src/main.rs` | Entry point, terminal session, event loop |
| `cli` | `cli/src/crash_hardening.rs` | Crash hardening + `FORGE_ALLOW_COREDUMPS` behavior |
| `core` | `core/src/environment.rs` | Environment gathering + AGENTS discovery/concatenation |
| `core` | `core/src/env_context.rs` | Environment context rendering + AGENTS consumption |
| `config` | `config/src/lib.rs` | Config parsing and persistence (`ForgeConfig`) |
| `utils` | `utils/src/atomic_write.rs` | Crash-safe file persistence (temp + rename) |
| `utils` | `utils/src/security.rs` | Secret redaction and sanitization for display |
| `utils` | `utils/src/diff.rs` | Unified diff formatting and stats |
| `engine` | `engine/src/lib.rs` | App API surface and re-exports |
| `engine` | `engine/src/state.rs` | `ToolBatch`, `ApprovalState`, operation states |
| `engine` | `engine/src/app/mod.rs` | App state machine orchestration |
| `engine` | `engine/src/app/commands.rs` | Slash command parsing and dispatch |
| `engine` | `engine/src/app/tool_loop.rs` | Tool executor orchestration, approval flow |
| `engine` | `engine/src/app/streaming.rs` | Stream event handling and provider request flow |
| `engine` | `engine/src/app/persistence.rs` | Crash recovery and session restore |
| `engine` | `engine/src/app/input_modes.rs` | Insert/command mode wrappers and send pipeline |
| `engine` | `engine/src/ui/mod.rs` | Engine-side UI state/types bridge |
| `tui` | `tui/src/lib.rs` | Full-screen rendering |
| `tui` | `tui/src/input.rs` | Keyboard input handling |
| `tui` | `tui/src/theme.rs` | Colors and styles |
| `tui` | `tui/src/markdown.rs` | Markdown to ratatui conversion |
| `tui` | `tui/src/effects.rs` | Modal animations (PopScale, SlideUp, Shake) |
| `tui` | `tui/src/tool_display.rs` | Tool result rendering |
| `context` | `context/src/manager.rs` | Context orchestration, distillation triggers |
| `context` | `context/src/history.rs` | Persistent storage (`MessageId`, `DistillateId`) |
| `context` | `context/src/stream_journal.rs` | SQLite WAL for crash recovery |
| `context` | `context/src/tool_journal.rs` | Tool execution journaling |
| `context` | `context/src/working_context.rs` | Token budget allocation |
| `context` | `context/src/distillation.rs` | Distillate generation |
| `context` | `context/src/model_limits.rs` | Per-model token limits |
| `context` | `context/src/token_counter.rs` | Token counting |
| `context` | `context/src/fact_store.rs` | Fact extraction and storage |
| `context` | `context/src/librarian.rs` | Context retrieval orchestration |
| `providers` | `providers/src/lib.rs` | Provider dispatch, SSE parsing |
| `providers` | `providers/src/claude.rs` | Anthropic integration |
| `providers` | `providers/src/openai.rs` | OpenAI Responses integration |
| `providers` | `providers/src/gemini.rs` | Gemini integration |
| `types` | `types/src/lib.rs` | Message types, `NonEmptyString`, `ModelName` |
| `types` | `types/src/model.rs` | Provider/model defaults and parsing |
| `types` | `types/src/ui/modal.rs` | `ModalEffectKind`, modal effect types |
| `lsp` | `lsp/src/lib.rs` | LSP client re-exports |
| `lsp` | `lsp/src/manager.rs` | Server lifecycle, event polling, diagnostics |
| `lsp` | `lsp/src/server.rs` | Child process, JSON-RPC routing |
| `lsp` | `lsp/src/codec.rs` | LSP Content-Length framing |
| `lsp` | `lsp/src/protocol.rs` | LSP message serde types |
| `lsp` | `lsp/src/diagnostics.rs` | Per-file diagnostics, `DiagnosticsSnapshot` |
| `lsp` | `lsp/src/types.rs` | `LspConfig`, `ServerConfig`, `ForgeDiagnostic` |
| `tools` | `tools/src/builtins.rs` | Built-in tool registry and definitions |
| `tools` | `tools/src/sandbox.rs` | Filesystem sandboxing rules |
| `tools` | `tools/src/search.rs` | Search tool implementation |
| `tools` | `tools/src/git.rs` | Git tool implementations |
| `tools` | `tools/src/webfetch/mod.rs` | URL fetch pipeline and tool executor |

## Extension Points

| Task | Where |
|------|-------|
| Add command | `engine/src/app/commands.rs` — command spec + dispatch |
| Add input mode behavior | `engine/src/app/input_modes.rs` + `engine/src/ui/mod.rs` + `tui/src/input.rs` + `tui/src/lib.rs` |
| Add provider | `types/src/model.rs` provider/model enums + module in `providers/src/` |
| Change colors | `tui/src/theme.rs` |
| Add UI overlay | `tui/src/lib.rs` — `draw_*` function |
| Add modal animation | `types/src/ui/modal.rs` + `tui/src/effects.rs` |

## Providers

| Provider | Default Model | Context | Output |
|----------|---------------|---------|--------|
| Claude | `claude-opus-4-6` | 1M | 128K |
| OpenAI | `gpt-5.2` | 400K | 128K |
| Gemini | `gemini-3.1-pro-preview` | 1M | 65K |

## Pitfalls

- **Claude cache_control limit**: Max 4 blocks. `CacheBudget` type enforces this structurally. `plan_cache_allocation()` in `engine/src/app/streaming.rs` distributes slots across system prompt, tools, and messages based on token thresholds (>=4096).
- **Scrollbar rendering**: Only render when `max_scroll > 0`. Use `max_scroll` as content_length, not `total_lines`.
- **Cache expensive computations**: `context_usage_status()` should be cached, not recomputed per frame.
- **Journal atomicity**: commit+prune must be one transaction. Only commit journal if history save succeeds. Always discard or commit steps in error paths (prevents session brick).
- **Platform paths**: Use `dirs::home_dir()`, not hardcoded `~/.forge/`.

## Known Bugs

- **Test suite failure**: `attach_process_to_sandbox_succeeds_for_running_child` is a known failing test when invoking `cargo test`/`just verify` from within the Forge app flow; running the same commands outside of Forge succeeds.

## Testing

Integration tests live in `tests/`. Uses wiremock for HTTP mocking, insta for snapshots, tempfile for isolation.

## Reference Docs

| Document | Description |
|----------|-------------|
| `INVARIANT_FIRST_ARCHITECTURE.md` | IFA design principles |
| `docs/IFA_CONFORMANCE_RULES.md` | IFA conformance and artifact rules |
| `SECURITY.md` | Vulnerability reporting and security policy |
| `docs/ANTHROPIC_MESSAGES_API.md` | Claude API reference |
| `docs/OPENAI_RESPONSES_GPT52.md` | OpenAI Responses API integration |
| `docs/OPENAI_REASONING_ROUNDTRIP.md` | OpenAI reasoning item replay notes |
| `docs/PARALLEL_TOOL_EXECUTION.md` | Parallel tool execution design |
| `docs/CI_RUNBOOK.md` | CI troubleshooting notes |
| `docs/COLOR_SCHEME.md` | TUI palette reference |
| `docs/LP1.md` | Line-oriented patch DSL |
| `docs/RUST_2024_REFERENCE.md` | Rust 2024 edition features used |
| `docs/SECURITY_SANITIZATION.md` | Security sanitization architecture |

Each crate has its own `README.md` with detailed architecture. Read those on demand.

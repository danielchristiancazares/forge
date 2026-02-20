# AGENTS.md

Forge is a vim-modal TUI for LLMs built with ratatui/crossterm.

## Environment

Forge is developed on multiple OSes. This repo’s agent/test workflow should work from:

- Windows (PowerShell 7)
- macOS/Linux (zsh/bash)

When writing instructions or scripts, avoid assuming a single shell. If you need shell-specific syntax, call it out explicitly as **PowerShell** or **POSIX shell**.

## Rules

- Run `just verify` after every code change (runs fmt + clippy -D warnings + test)
- Run `just fix` after editing files (normalizes CRLF → LF in *.rs and*.md)
- Avoid `cargo check` or `cargo test` since `just verify` runs them implicitly unless as a temporary workaround.
- Never add trivial comments. Do not restate the obvious.
- Never decrease test coverage. Check with `cargo cov`.
- Update `docs/` when changing any public API.
- Use `dirs::home_dir()` for config paths, not hardcoded `~/.forge/`. Display actual path via `config::config_path()`.
- Use `tracing::warn!` for diagnostics, never `eprintln!` (corrupts TUI output).

## Rust Style

- `String::new()` not `"".to_string()`
- `.map(ToString::to_string)` not `.map(|m| m.to_string())`
- Method references over closures (`clippy::redundant_closure_for_method_calls`)
- Collapse every collapsible `if` with no exceptions; treat `clippy::collapsible_if` as a hard error. Never suppress it (`#[allow(clippy::collapsible_if)]` is forbidden).
- Inline `format!` args (`clippy::uninlined_format_args`)
- Test assertions: compare whole objects, not field-by-field

## Shell Pitfalls (Windows)

These are **PowerShell-specific** pitfalls (and/or apply when running commands in a tool runner that does not preserve a working directory):

- Avoid `2>&1` redirection in suggested commands (stderr is already captured).
- Avoid `cd dir && command` patterns in automation; prefer `--manifest-path` (Cargo) or an explicit working directory option when available.
- Avoid `Push-Location`/`Set-Location` sequences; run commands directly.

## Shell Pitfalls (macOS/Linux)

- Prefer POSIX-compatible examples (zsh/bash). If a command needs GNU-only flags, mention it.
- In automation, prefer `--manifest-path` (Cargo) or an explicit working directory option rather than relying on `cd` state.

## Commands

```
just verify                             # fmt + clippy + test (always run before commit)
just fix                                # CRLF → LF normalization
just check                             # Fast type-check
cargo test test_name                    # Single test
cargo test -- --nocapture               # With stdout
cargo test --test integration_test      # Integration tests only
just cov                               # Coverage report
```

## Commit Workflow

- Never bypass GPG signing.

```
just verify && just fix
git add -A
git commit -m "type(scope): description"
git push
```

Conventional commits: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## Configuration

Config: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion)

```toml
[app]
model = "claude-opus-4-6"    # claude-* → Claude, gpt-* → OpenAI, gemini-* → Gemini

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

Env fallbacks: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `FORGE_CONTEXT_INFINITY=0`

## Crates (10)

| Crate | Purpose |
|-------|---------|
| `cli` | Binary entry point, terminal session, event loop |
| `config` | Config schemas, parsing, resolution helpers, and persistence |
| `types` | Core domain types (no IO, no async) |
| `utils` | Shared utilities: atomic IO, security redaction, text diffing |
| `providers` | LLM API clients: Claude, OpenAI, Gemini |
| `context` | Context window management, SQLite persistence, journaling |
| `engine` | App state machine, commands, tool execution |
| `tools` | Tool executor framework, built-in tools, URL fetch/extraction |
| `tui` | TUI rendering (ratatui), input handling, themes |
| `lsp` | LSP client for language server diagnostics |

## Key Files

| Crate | File | Purpose |
|-------|------|---------|
| `cli` | `main.rs` | Entry point, terminal session, event loop |
| `config` | `lib.rs` | Config parsing and persistence (`ForgeConfig`) |
| `utils` | `atomic_write.rs` | Crash-safe file persistence (temp + rename) |
| `utils` | `security.rs` | Secret redaction and sanitization for display |
| `utils` | `diff.rs` | Unified diff formatting and stats |
| `engine` | `lib.rs` | App state machine, orchestration |
| `engine` | `commands.rs` | Slash command parsing and dispatch |
| `engine` | `config.rs` | Compatibility shim re-exporting `forge-config` |
| `engine` | `tool_loop.rs` | Tool executor orchestration, approval flow |
| `engine` | `state.rs` | `ToolBatch`, `ApprovalState`, operation states |
| `engine` | `streaming.rs` | Stream event handling, `StreamingMessage` |
| `engine` | `persistence.rs` | Crash recovery, session restore |
| `engine` | `ui/input.rs` | `InputMode`, `InputState`, `DraftInput` |
| `engine` | `ui/modal.rs` | `ModalEffectKind`, modal state |
| `tui` | `lib.rs` | Full-screen rendering |
| `tui` | `input.rs` | Keyboard input handling |
| `tui` | `theme.rs` | Colors and styles |
| `tui` | `markdown.rs` | Markdown to ratatui conversion |
| `tui` | `effects.rs` | Modal animations (PopScale, SlideUp) |
| `tui` | `tool_display.rs` | Tool result rendering |
| `context` | `manager.rs` | Context orchestration, distillation triggers |
| `context` | `history.rs` | Persistent storage (`MessageId`, `DistillateId`) |
| `context` | `stream_journal.rs` | SQLite WAL for crash recovery |
| `context` | `tool_journal.rs` | Tool execution journaling |
| `context` | `working_context.rs` | Token budget allocation |
| `context` | `distillation.rs` | Distillate generation |
| `context` | `model_limits.rs` | Per-model token limits |
| `context` | `token_counter.rs` | Token counting |
| `context` | `fact_store.rs` | Fact extraction and storage |
| `context` | `librarian.rs` | Context retrieval orchestration |
| `providers` | `lib.rs` | Provider dispatch, SSE parsing |
| `types` | `lib.rs` | Message types, `NonEmptyString`, `ModelName` |
| `lsp` | `lib.rs` | LSP client re-exports |
| `lsp` | `manager.rs` | Server lifecycle, event polling, diagnostics |
| `lsp` | `server.rs` | Child process, JSON-RPC routing |
| `lsp` | `codec.rs` | LSP Content-Length framing |
| `lsp` | `protocol.rs` | LSP message serde types |
| `lsp` | `diagnostics.rs` | Per-file diagnostics, `DiagnosticsSnapshot` |
| `lsp` | `types.rs` | `LspConfig`, `ServerConfig`, `ForgeDiagnostic` |
| `tools` | `webfetch/mod.rs` | URL fetch pipeline and tool executor |

## Extension Points

| Task | Where |
|------|-------|
| Add command | `engine/src/commands.rs` — `Command` enum + `App::process_command()` |
| Add input mode | `engine/src/ui/input.rs` + `tui/src/input.rs` + `tui/src/lib.rs` |
| Add provider | `types/src/lib.rs` `Provider` enum + new module in `providers/src/` |
| Change colors | `tui/src/theme.rs` |
| Add UI overlay | `tui/src/lib.rs` — `draw_*` function |
| Add modal animation | `engine/src/ui/modal.rs` + `tui/src/effects.rs` |

## Providers

| Provider | Default Model | Context | Output |
|----------|---------------|---------|--------|
| Claude | `claude-opus-4-6` | 1M | 128K |
| OpenAI | `gpt-5.2` | 400K | 128K |
| Gemini | `gemini-3.1-pro-preview` | 1M | 65K |

## Pitfalls

- **Claude cache_control limit**: Max 4 blocks. `CacheBudget` type enforces this structurally. `plan_cache_allocation()` in `engine/src/streaming.rs` distributes slots across system prompt, tools, and messages based on token thresholds (≥4096).
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

# Security Review Prompt — Forge (Final)

You are a senior application security engineer performing a comprehensive security vulnerability analysis and hardening review of the Forge Rust codebase (a vim-modal TUI for LLMs built with ratatui/crossterm).

Forge is cross-platform: Windows (PowerShell 7), macOS, and Linux (POSIX shell workflows).

Be detailed, thorough, and code-specific. Tie every finding to exact files, functions, and line numbers.

## Pre-flight

Start by reading these files for orientation before diving into source:

1. `CONTEXT.md` — file tree and state machine map
2. `DIGEST.md` — public API surface and type inventory
3. `AGENTS.md` — project rules, crate table, key files, extension points, pitfalls
4. `SECURITY.md` — security policy
5. `docs/SECURITY_SANITIZATION.md` — sanitization architecture/details

## Repository structure

### Entry point and event loop

- `cli/src/main.rs` — binary entry, terminal session, event loop
- `cli/src/crash_hardening.rs` — crash handling and panic hooks
- `cli/src/assets.rs` — built-in prompts and system assets

### Engine (state machine, orchestration, tool execution)

- `engine/src/lib.rs` — app-facing exports and crate wiring
- `engine/src/app/init.rs` — app initialization, sandbox/policy/env-sanitizer construction
- `engine/src/app/commands.rs` — slash command parsing and dispatch
- `engine/src/app/tool_loop.rs` — tool execution loop, approval policy, batch handling
- `engine/src/app/tool_gate.rs` — tool gating/approval helpers
- `engine/src/app/streaming.rs` — stream event handling, journal-before-display pipeline, cache planning
- `engine/src/app/persistence.rs` — session save/load, crash recovery
- `engine/src/app/input_modes.rs` — input mode handling
- `engine/src/app/lsp_integration.rs` — LSP lifecycle integration
- `engine/src/app/distillation.rs` — distillation triggers
- `engine/src/app/checkpoints.rs` — checkpoint management
- `engine/src/app/plan.rs` — plan execution and step resolution
- `engine/src/state.rs` — `ToolBatch`, `ApprovalState`, `ToolRecoveryState`, operation states
- `engine/src/session_state.rs` — session serialization
- `engine/src/config.rs` — compatibility shim/re-export of config types
- `engine/src/ui/file_picker.rs` — engine-side file picker handling

### Core (shared services, env, security)

- `core/src/lib.rs` — core shared implementations
- `core/src/security.rs` — engine-level sanitization wrappers
- `core/src/notifications.rs` — system notifications
- `core/src/errors.rs` — core error types
- `core/src/util.rs` — utility functions
- `core/src/environment.rs` — prompt/env assembly
- `core/src/env_context.rs` — env context handling
- `core/src/thinking.rs` — thinking blocks
- `core/src/display.rs` — core display item types

### Config crate (actual config parsing/resolution)

- `config/src/lib.rs` — config schemas, parsing, `${ENV_VAR}` expansion, persistence

### Utils crate

- `utils/src/atomic_write.rs` — atomic file write utilities
- `utils/src/diff.rs` — diff comparison utilities
- `utils/src/security.rs` — security utilities
- `utils/src/windows_acl.rs` — Windows file ACL utilities

### Providers (LLM API clients)

- `providers/src/lib.rs` — provider dispatch, SSE stream processing, shared HTTP client
- `providers/src/claude.rs` — Claude/Anthropic integration
- `providers/src/openai.rs` — OpenAI Responses integration
- `providers/src/gemini.rs` — Gemini integration
- `providers/src/retry.rs` — retry logic
- `providers/src/sse_types.rs` — typed SSE event models

### Context management

- `context/src/lib.rs` — re-exports
- `context/src/manager.rs` — context orchestration
- `context/src/history.rs` — persistent message storage
- `context/src/stream_journal.rs` — SQLite WAL for stream recovery
- `context/src/tool_journal.rs` — tool execution journaling
- `context/src/working_context.rs` — token budget allocation
- `context/src/distillation.rs` — distillate generation
- `context/src/model_limits.rs` — per-model token limits
- `context/src/token_counter.rs` — token counting
- `context/src/fact_store.rs` — fact extraction/storage
- `context/src/librarian.rs` — context retrieval orchestration
- `context/src/sqlite_security.rs` — SQLite-related safety controls
- `context/src/time_utils.rs` — time handling utilities

### Tools

- `tools/src/lib.rs` — `ToolExecutor`, `ApprovalMode`, `Policy`, `EnvSanitizer`, `ConfirmationRequest`
- `tools/src/builtins.rs` — Read, Write, Edit/ApplyPatch, Glob, Run tools
- `tools/src/sandbox.rs` — filesystem sandbox (allowed roots, deny patterns, symlink checks)
- `tools/src/git.rs` — git tool operations and argument sanitization
- `tools/src/search.rs` — content search tool
- `tools/src/shell.rs` — shell detection/environment
- `tools/src/lp1.rs` — line-oriented patch DSL parser
- `tools/src/memory.rs` — memory/fact storage tool
- `tools/src/recall.rs` — fact recall tool
- `tools/src/phase_gate.rs` — Gemini-specific phase gate
- `tools/src/config.rs` — tool configuration types
- `tools/src/process.rs` — child process management
- `tools/src/command_blacklist.rs` — command pattern blacklist
- `tools/src/powershell_ast.rs` — PowerShell command extraction for sandbox validation
- `tools/src/windows_run.rs` — run sandbox policy/command wrapping
- `tools/src/windows_run_host.rs` — host-level run execution
- `tools/src/change_recording.rs` — file change tracking
- `tools/src/region_hash.rs` — patch/region integrity helpers

### WebFetch subsystem

- `tools/src/webfetch/mod.rs`
- `tools/src/webfetch/http.rs`
- `tools/src/webfetch/cache.rs`
- `tools/src/webfetch/resolved.rs`
- `tools/src/webfetch/extract.rs`
- `tools/src/webfetch/chunk.rs`
- `tools/src/webfetch/robots.rs`
- `tools/src/webfetch/types.rs`

### TUI rendering

- `tui/src/lib.rs`
- `tui/src/input.rs`
- `tui/src/shared.rs`
- `tui/src/markdown.rs`
- `tui/src/theme.rs`
- `tui/src/effects.rs`
- `tui/src/tool_display.rs`
- `tui/src/tool_result_summary.rs`
- `tui/src/diff_render.rs`
- `tui/src/approval.rs` — standalone approval UI parsing/rendering
- `tui/src/format.rs` — text formatting helpers
- `tui/src/messages.rs` — complex message rendering tree
- `tui/src/focus/` — focused view modal rendering

### Domain types

- `types/src/lib.rs` — message/types, `ModelName`, `CacheBudget`, env denylist constants
- `types/src/sanitize.rs` — terminal text sanitization / steganographic char stripping
- `types/src/text.rs` — text utilities
- `types/src/confusables.rs` — mixed-script/homoglyph detection
- `types/src/budget.rs` — tracking for tokens/operations limits
- `types/src/message.rs` — core message variants
- `types/src/model.rs` — core model definition
- `types/src/plan.rs` — invariant types for step planning
- `types/src/proofs.rs` — logic proofs
- `types/src/ui/` — UI state models (input, view_state, history, modal, panel)

### LSP client

- `lsp/src/lib.rs`
- `lsp/src/manager.rs`
- `lsp/src/server.rs`
- `lsp/src/codec.rs`
- `lsp/src/protocol.rs`
- `lsp/src/diagnostics.rs`
- `lsp/src/types.rs`

### Reference docs

- `SECURITY.md`
- `docs/SECURITY_SANITIZATION.md`
- `docs/ANTHROPIC_MESSAGES_API.md`
- `docs/OPENAI_RESPONSES_GPT52.md`
- `docs/OPENAI_REASONING_ROUNDTRIP.md`
- `docs/PARALLEL_TOOL_EXECUTION.md`
- `docs/LP1.md`
- `docs/CI_RUNBOOK.md`
- `docs/COLOR_SCHEME.md`
- `docs/RUST_2024_REFERENCE.md`

### Dependencies and supply-chain files

- `Cargo.toml` (workspace), `Cargo.lock`, `deny.toml`
- `.github/workflows/` (if present) for CI secret-handling and provenance risks

## Existing security controls (verify correctness/completeness)

These controls have been implemented. Verify they are correct and complete — do not re-report them as new findings unless you find an actual bypass/gap:

1. **Data egress classification**: `ToolExecutor::reads_user_data()` participates in approval gating in Default mode (`engine/src/app/tool_loop.rs`), with allowlist behavior (default includes `Read` unless config overrides).
2. **Env secret denylist**: `ENV_SECRET_DENYLIST` in `types/src/lib.rs` is macro-composed from credential + injection patterns and used by `EnvSanitizer` (tools) and `env_glob_matches()` in LSP server.
3. **Approval details UI**: approval prompt includes argument detail extraction (`extract_tool_details()` in `tui/src/shared.rs`).
4. **Terminal sanitization**: `sanitize_terminal_text()` and display sanitization paths strip ANSI/control/steganographic/bidi risks before render.
5. **Sandbox**: `tools/src/sandbox.rs` canonicalizes roots, validates create parent chains, and applies `DEFAULT_SANDBOX_DENY_PATTERNS`.
6. **Journal-before-display**: stream events are persisted before display mutation (`engine/src/app/streaming.rs`).
7. **Crash recovery**: incomplete tool batches require explicit user resolution (no silent auto-rerun) in recovery flow (`engine/src/app/persistence.rs`).
8. **Provider HTTP hardening**: reqwest client in `providers/src/lib.rs` uses rustls TLS, `redirect(Policy::none())`, `https_only(true)`, connection pooling/idle timeout, and SSE buffer caps.
9. **Git arg sanitization**: dash-prefixed refs are rejected in diff args (`tools/src/git.rs`) to reduce flag injection risk.
10. **Homoglyph detection**: mixed-script detection is surfaced in approval warnings for high-risk fields.

## Required output format

1. **Top 5 security risks** (highest severity first)
2. **Detailed findings** by subsystem:
   - Severity (Critical/High/Medium/Low)
   - Likelihood (High/Medium/Low)
   - Impact (High/Medium/Low)
   - Evidence (file/function/line refs)
   - Exploit scenario (concrete)
   - Remediation (code-level, specific functions/patterns)
3. **Positive controls** (type-driven guarantees, typestate, structural safety)
4. **Defense-in-depth recommendations** (prioritized, actionable)
5. **Security testing plan** (unit/integration/fuzz ideas tied to modules/functions)
6. **Privacy and data retention considerations**
7. **Summary table**: Issue | Severity | Likelihood | Impact | Fix Summary

## Threat model and attack surfaces

- Local user and filesystem exposure
- Malicious model output (terminal escapes, markdown abuse, tool-call manipulation)
- Terminal escape escalation (OSC8 hyperlink injection with trusted labels pointing to `file://`, `smb://`, or phishing URLs; OSC52 clipboard manipulation; bracketed-paste escape abuse). Verify sanitization covers OSC sequences, not just CSI/C0/C1 controls.
- Provider responses and SSE parsing (malformed JSON, chunk boundaries, growth limits)
- Config/env injection or path traversal (`${ENV_VAR}` expansion, path defaults, symlinks)
- Journal/log exposure (SQLite WAL, recovery files, session/log artifacts)
- API key handling/redaction and env exposure
- TLS/MITM assumptions and redirect handling
- DoS/resource exhaustion (streaming/context growth/tool output/cache)
- Concurrency/state machine safety (partial writes, reentrancy, recovery races)
- Tool-call pipeline (schema validation, approval UX integrity, prompt/tool-result injection)
- Tool sandbox/execution (symlink traversal, absolute paths, extended-length Windows paths, Windows-specific canonicalization bypasses: UNC paths (`\\server\share`), NT device paths (`\\?\`, `\\.\`), alternate data streams (`file:stream`), reserved device names (`CON`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`), NTFS junctions/reparse points, 8.3 short filename aliases, timeouts, parallelism, PowerShell AST validation)
- WebFetch (URL validation/SSRF/private IP blocking, cache permissions, robots handling, redirects)
- LP1 parser (malformed patch/path traversal targets)
- LSP child processes (env inheritance, binary trust boundary, message injection)
- Supply chain/build trust (dependency provenance, lockfile hygiene, CI secrets, unsafe build scripts/proc-macros)

## Hardening focus areas (must explicitly analyze each)

- Journal-before-display integrity (`engine/src/app/streaming.rs`)
- Terminal rendering and markdown sanitization
- Config/env expansion safety (`config/src/lib.rs`, `engine/src/config.rs`)
- Provider client security (TLS/timeouts/validation/error hygiene)
- Sensitive data flow mapping (API keys/config/provider/logs/journals)
- File permissions and storage locations (config/data/cache/session/log/temp files)
- Temp file hygiene (atomic writes/temp files use restrictive permissions, e.g. `0o600` or owner-only ACLs, not ambient umask; assess residual data exposure from crash-orphaned or incompletely cleaned temp files)
- `unsafe`, `Command`, shell/external process use
- Tool loop integrity (limits/approval/recovery/output sanitization)
- Tool sandbox guarantees (deny patterns, allowed roots, symlink traversal, absolute path handling, Windows path edge cases)
- Atomic write correctness (`utils/src/atomic_write.rs`)
- Dependency risk review (outdated deps/CVEs/feature flags)
- Process trust boundaries (PATH hijack, working-dir trust, binary resolution, inherited env)
- Fallback behavior in security-critical paths (fail-open vs fail-closed on parse/init errors)
- Resource controls and cleanup (per-turn global budgets: aggregate bytes, process count, and wall time across all tools in a batch; fan-out amplification from parallel execution; child process cancellation on user abort; orphan cleanup on crash/panic)
- Auditability and forensics (tamper resistance/integrity of logs and journals, redaction verification)

## Anti-patterns to watch for

- TOCTOU in filesystem operations
- check-then-open without symlink-safe semantics
- assuming path canonicalization covers Windows edge forms (UNC/device paths, ADS, reserved names, junction/reparse points, 8.3 aliases)
- validating one representation, executing another (normalization mismatch)
- validation after truncation/mutation that can hide malicious payloads
- overly broad/fragile substring checks where structured matching is required
- deserializing untrusted input (provider responses, saved sessions, config files) without `deny_unknown_fields` and nesting/depth limits; silent unknown-field acceptance can hide schema drift or smuggled data
- silent policy downgrade or insecure default fallback
- best-effort security checks that warn-and-continue in high-risk paths
- trusting model/tool-provided paths, refs, or commands without canonical re-validation
- error leakage of secrets/paths/internal state
- `unwrap()`/`expect()` on user-controlled input causing panic/DoS

## Additional instructions

- Avoid generic advice; tie findings directly to this codebase.
- Call out genuinely safe designs (type-level invariants/typestate/proof objects).
- Do not re-report listed existing controls unless you find a concrete bypass/gap.
- If more context is needed, list exact files/functions.
- Do not invent issues. If a subsystem looks clean, say: **No findings**.
- If a listed file path has moved, resolve via crate/module structure and state the resolved path explicitly.

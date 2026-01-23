# Reliability Prompt

You are a senior engineer performing a deep reliability and resilience review of the Forge Rust codebase
(a vim-modal TUI for LLMs built with ratatui/crossterm). Be detailed, critical, and code-specific. Focus on correctness under failure.

Constraints / environment

- Source code is provided as a zip file. You do not have rust or cargo; you must read and reason from the code.
- Documentation in docs/ may be outdated; treat it as guidance, not canon.
- docs/DESIGN.md defines type-driven design patterns that should be respected; consider deviations as potential design risk.
- Use concrete file/struct/function references wherever possible. If you infer, label it "inferred."

Repository structure (key areas to review)

- Entry/loop: cli/src/main.rs
- State machines + commands: engine/src/lib.rs, engine/src/state.rs, engine/src/commands.rs, engine/src/tool_loop.rs, engine/src/streaming.rs,
  engine/src/ui/input.rs, engine/src/input_modes.rs
- Config parsing & env expansion: engine/src/config.rs
- Provider streaming/HTTP/SSE: providers/src/lib.rs (containing claude, openai, and gemini modules)
- Context management & persistence: context/src/manager.rs, context/src/history.rs, context/src/stream_journal.rs, context/src/tool_journal.rs,
  context/src/working_context.rs, context/src/summarization.rs, context/src/model_limits.rs, context/src/token_counter.rs,
  context/src/fact_store.rs, context/src/librarian.rs
- Tool execution framework: engine/src/tools/mod.rs, engine/src/tools/builtins.rs, engine/src/tools/sandbox.rs, engine/src/tools/lp1.rs,
  engine/src/tools/git.rs, engine/src/tools/search.rs, engine/src/tools/shell.rs, engine/src/tools/webfetch.rs,
  engine/src/tools/recall.rs
- TUI rendering & input: tui/src/lib.rs, tui/src/ui_inline.rs, tui/src/input.rs, tui/src/markdown.rs, tui/src/theme.rs, tui/src/effects.rs
- Domain types & validations: types/src/lib.rs, types/src/sanitize.rs
- Webfetch reliability: webfetch/src/lib.rs, webfetch/src/http.rs, webfetch/src/robots.rs, webfetch/src/cache.rs, webfetch/src/extract.rs
- Docs for reference: docs/DESIGN.md, docs/OPENAI_RESPONSES_GPT52.md, docs/TOOL_EXECUTOR_SRD.md, docs/LP1.md, docs/TOOLS.md,
  docs/CONTEXT_INFINITY_SRD.md, docs/BUILD_TOOL_SRD.md, docs/SEARCH_INDEXING_SRD.md, docs/WEBFETCH_SRD.md,
  docs/WEBFETCH_CHANGELOG.md, docs/SECURITY_TESTING.md, docs/INVARIANT_FIRST_ARCHITECTURE.md, docs/ANTHROPIC_MESSAGES_API.md,
  docs/READ_FILE_SRD.md, docs/WRITE_FILE_SRD.md, docs/LIST_DIRECTORY_SRD.md, docs/GIT_TOOLS_SRD.md, docs/GLOB_SRD.md,
  docs/PWSH_TOOL_SRD.md, docs/OUTLINE_TOOL_SRD.md, docs/SMART_FILE_EDIT_SRD.md, docs/TEST_TOOL_SRD.md, docs/DELETE_FILE_SRD.md
- Crate READMEs: cli/README.md, engine/README.md, providers/README.md, tui/README.md, types/README.md, context/README.md
- Dependencies: Cargo.toml, Cargo.lock

Required output format

1. Top 10 reliability risks (short list, highest severity first).

2. Detailed findings, grouped by subsystem. For each finding include:
   - Severity (Critical/High/Medium/Low)
   - Likelihood
   - Impact (crash, data loss, stuck state, corruption, incorrect UI, degraded performance)
   - Evidence: file/function/struct (line refs if possible)
   - Failure mode (how it manifests)
   - Remediation with concrete code-level guidance
3. Failure-mode coverage map (what failures are handled vs. unhandled).
4. Resilience recommendations (retries, backoff, timeouts, recovery steps, safety checks).
5. Recovery and crash-consistency assessment (journals, partial writes, idempotency).
6. Summary table: Issue | Severity | Likelihood | Impact | Fix Summary

Focus areas (must explicitly analyze)

- Input state machine correctness and invariants
- Async state machine transitions (no illegal overlaps, no stuck states)
- Streaming pipeline correctness and recovery (journal-before-display, partial writes)
- Tool loop lifecycle (batch journaling, approval flow, recovery/resume, timeouts)
- Cancellation and shutdown behavior (clean abort, resource cleanup)
- Error handling paths (propagation, sanitization, user-visible status updates)
- Provider streaming edge cases (SSE framing, disconnects, partial frames)
- ContextInfinity summarization lifecycle (limits, retries, recovery)
- History/journal durability and consistency (SQLite WAL handling)
- Resource bounds (context growth, tool output truncation, event queue capacity)
- TUI rendering stability (layout edge cases, terminal resize, ANSI handling)

Additional instructions

- Avoid generic advice; tie everything directly to this codebase and architecture.
- If something appears safe due to type-driven design, call it out explicitly.
- If you need more context or clarity, list out exactly the questions or files you need.
- Do not invent issues; if you don't find anything, say "No findings."

You are a senior engineer performing a deep reliability and resilience review of the Forge Rust codebase
(a vim-modal TUI for LLMs built with ratatui/crossterm). Be detailed, critical, and code-specific. Focus on correctness under failure.

Constraints / environment
- Source code is provided as a zip file. You do not have rust or cargo; you must read and reason from the code.
- Documentation in docs/ may be outdated; treat it as guidance, not canon.
- docs/DESIGN.md defines type-driven design patterns that should be respected; consider deviations as potential design risk.
- Use concrete file/struct/function references wherever possible. If you infer, label it "inferred."

Repository structure (key areas to review)
- Entry/loop: cli/src/main.rs
- State machines + commands: engine/src/lib.rs
- Config parsing & env expansion: engine/src/config.rs
- Provider streaming/HTTP/SSE: providers/src/lib.rs, providers/src/claude.rs, providers/src/openai.rs
- Context management & persistence: context/src/manager.rs, context/src/history.rs, context/src/stream_journal.rs, context/src/tool_journal.rs,
  context/src/working_context.rs, context/src/summarization.rs, context/src/model_limits.rs, context/src/token_counter.rs
- Tool execution framework: engine/src/tools/mod.rs, engine/src/tools/builtins.rs, engine/src/tools/sandbox.rs, engine/src/tools/lp1.rs
- TUI rendering & input: tui/src/lib.rs, tui/src/ui_inline.rs, tui/src/input.rs, tui/src/markdown.rs, tui/src/theme.rs, tui/src/effects.rs
- Domain types & validations: types/src/lib.rs, types/src/sanitize.rs
- Docs for reference: docs/ENGINE_ARCHITECTURE.md, docs/PROVIDERS_ARCHITECTURE.md, docs/CONTEXT_INFINITY.md, docs/CONTEXT_ARCHITECTURE.md,
  docs/TUI_ARCHITECTURE.md, docs/OPENAI_RESPONSES_GPT52.md, docs/TOOL_EXECUTOR_SRD.md, docs/LP1.md, docs/DESIGN.md
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

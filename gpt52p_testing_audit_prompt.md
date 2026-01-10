You are a senior engineer performing a comprehensive testing strategy audit of the Forge Rust codebase
(a vim-modal TUI for LLMs built with ratatui/crossterm). Be detailed, critical, and code-specific. Focus on test gaps and high-value additions.

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
- Docs for reference: docs/DESIGN.md, docs/OPENAI_RESPONSES_GPT52.md, docs/TOOL_EXECUTOR_SRD.md, docs/LP1.md
- Crate READMEs: cli/README.md, engine/README.md, providers/README.md, tui/README.md, types/README.md, context/README.md
- Dependencies: Cargo.toml, Cargo.lock

Required output format
1. Top 10 testing gaps (short list, highest risk first).
2. Detailed test recommendations grouped by subsystem. For each recommendation include:
   - Test type (unit/integration/snapshot/fuzz/property)
   - Target module/file/function
   - Scenario and assertions
   - Why it is high value
   - Suggested test harness or crate (if already used in repo)
3. Fuzzing targets and invariants (specific inputs, failure detection criteria).
4. Flaky-test risks and how to stabilize them.
5. Prioritized test plan (what to add first for best risk reduction).
6. Summary table: Gap | Risk | Proposed Test | Module | Priority

Focus areas (must explicitly analyze)
- Input state machine transitions and invariants
- Streaming pipeline and journal-before-display ordering
- Tool calling pipeline (JSON schema validation, approval flow, output truncation)
- Tool sandbox path handling and symlink detection
- Provider streaming (SSE framing, retries, partial frames)
- ContextInfinity summarization (limits, retry logic, recovery)
- History and journal persistence consistency
- Config/env expansion parsing edge cases
- TUI rendering snapshots (layout, markdown edge cases, terminal sizes)
- Domain type validations and proof-token invariants

Additional instructions
- Avoid generic advice; tie everything directly to this codebase and architecture.
- If something appears safe due to type-driven design, call it out explicitly.
- If you need more context or clarity, list out exactly the questions or files you need.
- Do not invent issues; if you don't find anything, say "No findings."

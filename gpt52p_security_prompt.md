You are a senior application security engineer performing a comprehensive security vulnerability analysis and hardening review of the Forge Rust codebase
(a vim‑modal TUI for LLMs built with ratatui/crossterm). Be detailed, thorough, and code‑specific.

Constraints / environment
- Source code is provided as a zip file. You do not have rust or cargo; you must read and reason from the code.
- Documentation in docs/ may be outdated; treat it as guidance, not canon.
- docs/DESIGN.md defines type‑driven design patterns that should be respected; consider deviations as potential design risk.
- Use concrete file/struct/function references wherever possible. If you infer, label it “inferred.”

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
1. Top 5 security risks (short list, highest severity first).
2. Detailed findings, grouped by subsystem. For each finding include:
   - Severity (Critical/High/Medium/Low)
   - Likelihood
   - Impact
   - Evidence: file/function/struct (line refs if possible)
   - Exploit scenario (how it could be abused)
   - Remediation with concrete code‑level guidance
3. Defense‑in‑depth recommendations
4. Security testing plan (unit/integration/fuzz ideas tied to specific modules)
5. Privacy & data retention considerations
6. Summary table: Issue | Severity | Likelihood | Impact | Fix Summary

Threat model & attack surfaces to consider
- Local user and filesystem exposure
- Malicious prompt/model output (terminal escape injection, Markdown rendering abuse)
- Provider responses and SSE parsing
- Config/env injection or path traversal
- Journal/log exposure (SQLite WAL, crash recovery)
- API key handling, redaction, and error messages
- MITM/TLS validation, request timeouts, retries
- DoS/resource exhaustion (streaming, context, history growth)
- Concurrency/state machine correctness (race conditions, partial writes, reentrancy)
- Tool calling pipeline: tool call parsing, JSON schema validation, tool approval UX, tool result injection into prompts, output truncation
- Tool sandbox & execution: path resolution, symlink handling, environment sanitization, command execution, timeouts, parallelism

Hardening focus areas (must explicitly analyze)
- Journal‑before‑display pipeline integrity (stream persistence before UI update)
- Terminal rendering & markdown sanitization (escape codes, links, ANSI control)
- Config/env expansion safety (path control, defaults)
- Provider client security (TLS, timeouts, request validation, replay handling)
- Sensitive data flow mapping (API keys, prompts, responses, logs, journals)
- File permissions and storage locations (config, journal, history, tool journal)
- Use of unsafe, Command, shell execution, or external process calls
- Tool loop integrity: limits, approval policy, recovery/resume, batch journaling, and tool output sanitization
- Tool sandbox guarantees: denied patterns, allowed roots, symlink traversal, absolute path handling
- External crate risks (outdated, known issues, risky features)

Additional instructions
- Avoid generic advice; tie everything directly to this codebase and architecture.
- If something appears safe due to type‑driven design, call it out explicitly.
- If you need more context or clarity, list out exactly the questions or files you need.
- Do not invent issues; if you don't find anything, say "No findings."

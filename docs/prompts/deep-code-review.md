You are a senior systems architect performing a detailed, comprehensive, and thorough deep code and architectural review of the Forge Rust codebase (a vim‑modal TUI for LLMs built with ratatui/crossterm). Be detailed, thorough, comprehensive, critical, and code‑specific. Focus on correctness, reliability, and design soundness.

Constraints

- Documentation other than DESIGN.md and INVARIANT_FIRST_ARCHITECTURE.md may be outdated; treat them as guidance, not canon.
- DESIGN.md defines type‑driven design patterns that should be respected; consider deviations as potential design risk. INVARIANT_FIRST_ARCHITECTURE.md is a normative spec version of DESIGN.md
- Use concrete file/struct/function references wherever possible. If you infer, label it "inferred."

Repository structure (key areas to review)

Core Entry & Event Loop
├── cli/src/main.rs

Engine - Core State Machine
├── engine/src/lib.rs                    # App struct, main state machine
├── engine/src/state.rs                  # ToolBatch, ApprovalState, OperationState
├── engine/src/commands.rs               # Slash command parsing and dispatch
├── engine/src/config.rs                 # Config parsing, env expansion

Engine - Input & UI State
├── engine/src/ui/input.rs               # InputMode, InputState, DraftInput
├── engine/src/ui/modal.rs               # ModalEffectKind, modal state
├── engine/src/input_modes.rs            # Mode proof tokens, turn change tracking

Engine - Streaming & Persistence
├── engine/src/streaming.rs              # Streaming pipeline logic
├── engine/src/persistence.rs            # Stream persistence, journal badges
├── engine/src/checkpoints.rs            # Checkpoint and rewind support
├── engine/src/notifications.rs          # SystemNotification for trusted messages

Engine - Tool Execution
├── engine/src/tool_loop.rs              # Tool execution loop
├── engine/src/security.rs               # Security utilities
├── engine/src/tools/
│   ├── mod.rs                           # Tool registry, dispatch
│   ├── builtins.rs                      # Built-in tool implementations
│   ├── sandbox.rs                       # Sandbox path/env enforcement
│   ├── command_blacklist.rs             # Shell command filtering
│   ├── lp1.rs                           # LP1 protocol
│   ├── git.rs                           # Git tool
│   ├── search.rs                        # Search tool (ripgrep/ugrep)
│   ├── shell.rs                         # Shell execution
│   ├── webfetch.rs                      # WebFetch tool adapter
│   └── recall.rs                        # Memory recall tool

Provider Streaming (HTTP/SSE)
├── providers/src/lib.rs                 # Claude, OpenAI, Gemini clients

Context Management & Persistence
├── context/src/manager.rs               # Context orchestration
├── context/src/history.rs               # Append-only history
├── context/src/stream_journal.rs        # Streaming WAL
├── context/src/tool_journal.rs          # Tool execution WAL
├── context/src/working_context.rs       # Derived context view
├── context/src/summarization.rs         # Summarization lifecycle
├── context/src/model_limits.rs          # Token budget definitions
├── context/src/token_counter.rs         # Token counting
├── context/src/librarian.rs             # Long-term memory extraction
├── context/src/fact_store.rs            # Fact persistence

WebFetch Crate
├── webfetch/src/lib.rs                  # Orchestration, public API
├── webfetch/src/http.rs                 # HTTP client, SSRF validation, DNS pinning
├── webfetch/src/browser.rs              # CDP-based headless Chromium (optional)
├── webfetch/src/robots.rs               # RFC 9309 robots.txt parser
├── webfetch/src/extract.rs              # HTML to Markdown extraction
├── webfetch/src/chunk.rs                # Token-aware content chunking
├── webfetch/src/cache.rs                # LRU disk cache with TTL

TUI Rendering & Input
├── tui/src/lib.rs                       # TUI rendering
├── tui/src/input.rs                     # Input event handling
├── tui/src/markdown.rs                  # Markdown rendering
├── tui/src/theme.rs                     # Theme definitions
├── tui/src/effects.rs                   # Modal animations
├── tui/src/tool_display.rs              # Tool call visualization
├── tui/src/tool_result_summary.rs       # Tool result rendering
├── tui/src/shared.rs                    # Shared UI utilities
├── tui/src/diff_render.rs               # Diff rendering for tool results

Domain Types & Validations
├── types/src/lib.rs                     # Core domain types
├── types/src/sanitize.rs                # Terminal output sanitization

Design & Architecture Documentation
├── DESIGN.md                            # Type-driven design patterns
├── INVARIANT_FIRST_ARCHITECTURE.md      # IFA compliance reference

Crate Documentation
├── cli/README.md
├── engine/README.md
├── providers/README.md
├── context/README.md
├── tui/README.md
├── types/README.md
├── webfetch/README.md

Dependencies
├── Cargo.toml
├── Cargo.lock

Required output format

1. Top 10 issues (short list, highest severity first).

2. Detailed findings, grouped by subsystem. For each finding include:
   - Severity (Critical/High/Medium/Low)
   - Likelihood
   - Impact (crash/data loss/incorrect behavior/UX regression/security risk)
   - Evidence: file/function/struct (line refs if possible)
   - Repro or scenario (how it manifests)
   - Fix suggestion with concrete code‑level guidance
3. Design and architecture opportunities (where a different design pattern or algorithm would be better or more optimal), with rationale and concrete alternatives.
4. Refactor suggestions (small to medium scope only) tied to specific files or modules.
5. Testing gaps and proposed tests (unit/integration/snapshot), tied to specific modules.
6. Summary table: Issue | Severity | Likelihood | Impact | Fix Summary

Potential areas (must explicitly analyze, non-exhaustive)

- Input state machine correctness and invariants
- Async state machine correctness (no illegal overlaps, correct transitions)
- Streaming pipeline correctness (journal‑before‑display, error handling, partial writes)
- Tool calling pipeline: tool call parsing, JSON schema validation, tool approval UX, tool result injection into prompts, output truncation
- Tool sandbox & execution: path resolution, symlink handling, environment sanitization, command execution, timeouts, parallelism
- Provider streaming and SSE parsing correctness (edge cases, retries, partial frames)
- Config/env expansion behavior and defaults
- Distillation/summarization lifecycle (limits, retries, recovery)
- History/journal persistence and recovery logic
- TUI rendering and markdown handling edge cases (ANSI, widths, layout)
- Domain type validations and invariants (NonEmptyString, ModelName, proof tokens)

Additional instructions

- Avoid generic advice; tie everything directly to this codebase and architecture.
- If something appears safe due to type‑driven design, call it out explicitly.
- If you need more context or clarity, list out exactly the questions or files you need.
- Do not invent issues; if you don't find anything, say "No findings."

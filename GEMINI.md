# Forge Developer Guide

## Project Overview

**Forge** is a vim-modal Terminal User Interface (TUI) for interacting with Large Language Models (LLMs). It distinguishes itself with:

-   **Modal Editing:** Vim-inspired modes (Normal, Insert, Command, Model Select) for efficient navigation and composition.
-   **Context Infinity™:** An adaptive context management system that automatically summarizes older conversation history to stay within token limits without losing continuity.
-   **Agentic Tool Use:** A secure, interactive framework allowing LLMs to execute local tools (filesystem, git, shell) with user approval and crash recovery.
-   **Multi-Provider Support:** First-class support for Claude (Anthropic), GPT (OpenAI), and Gemini (Google).
-   **Invariant-First Architecture:** A strict design philosophy where invalid states are unrepresentable in the core.

## Architecture Philosophy

Forge follows the **Invariant-First Architecture (IFA)**. The core rule is: **Invalid states MUST NOT be representable in the core.**

*   **Type-Driven Design:** We use Rust's type system to enforce invariants at compile time.
    *   **Proof Tokens:** Operations specific to a mode require a token (e.g., `InsertToken`) that can only be obtained when the app is confirmed to be in that mode.
    *   **Smart Types:** `NonEmptyString`, `ModelName`, `ApiKey`, and `OutputLimits` enforce validity at construction.
*   **State Machines:** Logic is driven by explicit state enums (`InputState`, `OperationState`) to prevent invalid transitions.
*   **Journaling:** To prevent data loss, all stream events and tool executions are written to a SQLite Write-Ahead Log (WAL) *before* they are applied to the application state or displayed.

## Workspace Structure

The project is a Rust workspace divided into focused crates to enforce separation of concerns:

| Crate | Path | Responsibility |
| :--- | :--- | :--- |
| **`cli`** | `cli/` | Binary entry point, terminal session lifecycle, main event loop, and signal handling. |
| **`engine`** | `engine/` | Core business logic, state machines (`InputState`, `OperationState`), tool execution, and command dispatch. |
| **`tui`** | `tui/` | UI rendering (`ratatui`), input event handling, and theming. Handles "Full" and "Inline" modes. |
| **`context`** | `context/` | Context Infinity: token counting, context budgeting, summarization, and SQLite persistence. |
| **`providers`** | `providers/` | HTTP/SSE clients for LLM APIs (Claude, OpenAI, Gemini). |
| **`types`** | `types/` | Shared domain types (`Message`, `ModelName`, `ToolCall`) with no IO/async dependencies. |
| **`webfetch`** | `webfetch/` | Chromium-based web fetching, SSRF protection, and content extraction. |

---

## Crate Deep Dives

### 1. CLI (`cli/`)

The entry point. It manages the terminal session and the main event loop.

*   **Modes:** Supports `Full` (alternate screen) and `Inline` (viewport at cursor) modes.
*   **Lifecycle:** Uses RAII `TerminalSession` to ensure terminal state (raw mode, cursor) is restored even on panic.
*   **Assets:** Embeds system prompts (`assets/prompt.md`) at compile time.
*   **Event Loop:**
    1.  `app.tick()`: Advance animations/background tasks.
    2.  `yield_now()`: **Critical** to let async tasks progress (crossterm poll is blocking).
    3.  `app.process_stream_events()`: Apply buffered stream chunks.
    4.  `terminal.draw()`: Render the UI.
    5.  `handle_events()`: Process input.

### 2. Engine (`engine/`)

The "brain" of the application. TUI-agnostic.

*   **Input State Machine:** `Normal`, `Insert` (with `DraftInput`), `Command`, `ModelSelect`.
*   **Operation State Machine:** `Idle`, `Streaming`, `ToolLoop`, `Summarizing`, `ToolRecovery`. Mutually exclusive.
*   **Tool Executor:**
    *   **Journaling:** Tool calls are persisted to `ToolJournal` before execution.
    *   **Approval:** `AwaitingApproval` state allows user review before side effects.
    *   **Recovery:** Detects incomplete batches on startup.
*   **Command System:** Slash commands (`/quit`, `/model`, `/rewind`) parsed via `Command` enum.

### 3. Context (`context/`)

Implements **Context Infinity™**.

*   **Append-Only History:** Messages are never deleted. They are stored in `FullHistory`.
*   **Working Context:** A derived view built on-demand. It mixes original messages and `Summary` objects to fit the model's budget.
*   **Summarization:** Triggered when the budget is exceeded. Background tasks summarize older message blocks using cheaper models (e.g., `claude-haiku`).
*   **Librarian:** A background system that extracts structured facts from conversations and retrieves them for future context ("Long-term memory").
*   **Stream Journal:** SQLite WAL ensures that streaming responses are durable per-chunk.

### 4. TUI (`tui/`)

Pure presentation layer.

*   **Rendering:** Two implementations: `lib.rs` (Full) and `ui_inline.rs` (Inline).
*   **Theming:** "Kanagawa Wave" inspired palette. Supports high contrast and ASCII-only modes.
*   **Markdown:** Custom markdown renderer with caching.
*   **Effects:** Modal animations (`PopScale`, `SlideUp`).

### 5. Providers (`providers/`)

Unified LLM client.

*   **Dispatch:** `send_message()` routes to `claude`, `openai`, or `gemini` modules.
*   **Streaming:** Normalizes SSE streams into unified `StreamEvent`s (`TextDelta`, `ThinkingDelta`, `ToolCallStart`).
*   **Configuration:** `ApiConfig` ensures keys match providers.

### 6. WebFetch (`webfetch/`)

Safe web retrieval tool.

*   **Security:** SSRF protection, blocks private IPs, validates schemes.
*   **Robots.txt:** Compliant parser.
*   **Rendering:** HTTP client by default; optional headless Chromium for JS-heavy sites.
*   **Output:** Returns LLM-friendly Markdown chunks.

### 7. Types (`types/`)

Foundation types.

*   **`NonEmptyString`**: Validated string type.
*   **`Message`**: Proper sum type (`User`, `Assistant`, `System`, `ToolUse`, `ToolResult`).
*   **`Sanitize`**: Cleans terminal output to prevent escape sequence attacks.

---

## Development Workflow

### Prerequisites
*   **Rust:** Version 1.92.0 or later (2024 edition).

### Common Commands

*   **Build:** `cargo build`
*   **Run:** `cargo run --release`
*   **Test (All):** `cargo test`
*   **Test (Specific):** `cargo test -p forge-engine`
*   **Lint:** `cargo clippy --workspace --all-targets -- -D warnings`
*   **Format:** `cargo fmt`

### Git Conventions

*   **Commit Messages:** [Conventional Commits](https://www.conventionalcommits.org/).
    *   `feat(context): add fact storage`
    *   `fix(tui): correct scroll offset`

## Configuration

Config file: `~/.forge/config.toml` (supports `${ENV_VAR}` expansion).

```toml
[app]
provider = "claude"
model = "claude-opus-4-5-20251101"
tui = "full" # or "inline"

[context]
infinity = true

[tools.sandbox]
allowed_roots = ["."]
```
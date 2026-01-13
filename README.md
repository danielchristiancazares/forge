# Forge

A powerful TUI for interacting with GPT and Claude, featuring an agentic tool execution framework.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-13 | Header & TOC |
| 14-35 | Features: Core Capabilities, Context Infinity, Tool Executor |
| 36-45 | Quick Start |
| 46-70 | Configuration |
| 71-86 | Documentation Index |
| 87-94 | Development |
| 95-108 | Coverage (LCOV) |

## Features

### Core Capabilities

- **Vim-style Modal Interface**: Normal, Insert, Command, and ModelSelect modes
- **Multi-Provider Support**: Seamless switching between Claude (Anthropic) and GPT (OpenAI)
- **Full/Inline Display Modes**: Full-screen (alternate buffer) or inline terminal mode
- **Rich Markdown Rendering**: Tables, code blocks, and syntax highlighting

### Context Infinity™

- **Adaptive Context Management**: Summarizes older messages as needed to stay within model token limits
- **Crash Recovery**: Streaming responses are journaled and recoverable
- **History Persistence**: Conversations are saved and restored across sessions

### Tool Executor Framework

- **Agentic Tool Calling**: LLM can read files, apply patches, and run shell commands
- **Sandboxed Execution**: Path-based tools are restricted to allowed directories
- **Interactive Approval**: User can review and approve or deny tool calls before tool execution
- **Crash Recovery**: Tool batches are journaled for durability

## Quick Start

```bash
# Build
cargo build --release

# Run
cargo run --release
```

## Configuration

Create `~/.forge/config.toml`:

```toml
[app]
provider = "claude"          # or "openai"
model = "claude-sonnet-4-5-20250929"
tui = "full"                 # or "inline"

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"

[context]
infinity = true              # Enable adaptive context management

[tools]
mode = "enabled"             # disabled | parse_only | enabled

[tools.sandbox]
allowed_roots = ["."]
allow_absolute = false
```

## Documentation

| Document | Description |
|----------|-------------|
| [`engine/README.md`](engine/README.md) | Core state machine and orchestration |
| [`tui/README.md`](tui/README.md) | Terminal UI rendering and input handling |
| [`context/README.md`](context/README.md) | Context Infinity™ implementation |
| [`cli/README.md`](cli/README.md) | CLI entry point and terminal session |
| [`providers/README.md`](providers/README.md) | LLM API clients (Claude, OpenAI) |
| [`types/README.md`](types/README.md) | Core domain types |
| [`docs/CONTEXT_INFINITY_SRD.md`](docs/CONTEXT_INFINITY_SRD.md) | Context Infinity™ specification |
| [`docs/TOOL_EXECUTOR_SRD.md`](docs/TOOL_EXECUTOR_SRD.md) | Tool Executor Framework requirements |
| [`docs/TOOLS.md`](docs/TOOLS.md) | User guide for tool configuration |
| [`docs/LP1.md`](docs/LP1.md) | Line Patch v1 format specification |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Type-driven design philosophy |

## Development

Development commands (debug mode):

- Build: `cargo build`
- Run: `cargo run`
- Test: `cargo test`

## Coverage (LCOV)

This project uses `cargo-llvm-cov` to generate an LCOV report.

One-time setup:

- `cargo install cargo-llvm-cov`
- `rustup component add llvm-tools-preview`

Generate `lcov.info`:

- `cargo cov`
- or `./scripts/coverage.ps1`

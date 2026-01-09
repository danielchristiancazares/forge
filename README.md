# Forge

A powerful TUI for interacting with GPT and Claude, featuring an agentic tool execution framework.

## Features

### Core Capabilities

- **Vim-style Modal Interface**: Normal, Insert, Command, and ModelSelect modes
- **Multi-Provider Support**: Seamless switching between Claude (Anthropic) and GPT (OpenAI)
- **Full/Inline Display Modes**: Full-screen alternate screen or inline terminal mode
- **Rich Markdown Rendering**: Tables, code blocks, syntax highlighting

### Context Infinity™

- **Adaptive Context Management**: Automatically summarizes older messages to stay within model limits
- **Crash Recovery**: Streaming responses are journaled and recoverable
- **History Persistence**: Conversations are saved and restored across sessions

### Tool Executor Framework

- **Agentic Tool Calling**: LLM can read files, apply patches, and run shell commands
- **Sandboxed Execution**: Path-based tools are restricted to allowed directories
- **Interactive Approval**: User can review and approve/deny tool calls before execution
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
| [`docs/CONTEXT_ARCHITECTURE.md`](docs/CONTEXT_ARCHITECTURE.md) | Context Infinity™ specification |
| [`docs/TOOL_EXECUTOR_SRD.md`](docs/TOOL_EXECUTOR_SRD.md) | Tool Executor Framework requirements |
| [`docs/TOOLS.md`](docs/TOOLS.md) | User guide for tool configuration |
| [`docs/LP1.md`](docs/LP1.md) | Line Patch v1 format specification |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Type-driven design philosophy |

## Development

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

## License

See the repository root for license information.



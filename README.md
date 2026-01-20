# Forge

A vim-modal terminal user interface for interacting with Claude, GPT, and Gemini, featuring adaptive context management and an agentic tool execution framework.

Forge brings the efficiency of vim-style modal editing to AI conversation, letting you navigate, compose, and manage conversations without leaving your terminal. With Context Infinity, Forge automatically summarizes older messages to stay within model limits while preserving full conversation history. The Tool Executor Framework enables the LLM to read files, apply patches, and run shell commands - all with interactive approval and crash recovery.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
| --- | --- |
| 1-12 | Header and Introduction |
| 13-60 | Features: Core Capabilities, Context Infinity, Tool Executor |
| 61-100 | Requirements and Installation |
| 101-160 | Quick Start: First Run, Basic Usage |
| 161-250 | Configuration: Full Reference |
| 251-310 | Keyboard Shortcuts: All Modes |
| 311-370 | Commands Reference |
| 371-430 | Workspace Structure |
| 431-470 | Development |
| 471-510 | Troubleshooting |
| 511-540 | Documentation Index |
| 541-560 | Contributing and License |

## Features

### Core Capabilities

- **Vim-style Modal Interface**: Navigate with Normal mode, edit with Insert mode, run commands with Command mode, and switch models with ModelSelect mode
- **Multi-Provider Support**: Seamless switching between Claude (Anthropic), GPT (OpenAI), and Gemini (Google) with provider-specific optimizations
- **Full/Inline Display Modes**: Full-screen alternate buffer or inline terminal mode that preserves your scrollback
- **Rich Markdown Rendering**: Tables with box-drawing borders, syntax-highlighted code blocks, lists, and formatting
- **Streaming Responses**: Real-time token streaming with animated indicators

### Context Infinity

Forge's adaptive context management system keeps conversations flowing without hitting model limits:

- **Automatic Summarization**: When context fills up, older messages are compressed into summaries that preserve key information
- **Never Lose History**: Original messages are preserved and can be restored when switching to models with larger context windows
- **Crash Recovery**: Streaming responses are journaled to SQLite before display, so crashes never lose your work
- **Token Usage Display**: Real-time visibility into context usage with color-coded warnings

### Tool Executor Framework

Enable the LLM to interact with your local filesystem and execute tasks:

- **Built-in Tools**: `read_file`, `write_file`, `apply_patch`, `Glob`, `Search`, `run_command`, `WebFetch`, and various Git tools.
- **Sandboxed Execution**: Path-based tools are restricted to allowed directories with symlink escape prevention
- **Interactive Approval**: Review and approve or deny tool calls before execution
- **Stale File Protection**: Files must be read before patching, with SHA validation to catch external changes
- **Crash Recovery**: Tool batches are journaled for durability

## Requirements

- **Rust**: 1.92.0 or later (Rust 2024 edition)
- **Operating System**: Windows, macOS, or Linux
- **Terminal**: Any terminal supporting ANSI escape codes and Unicode
- **API Keys**: At least one of:
  - `ANTHROPIC_API_KEY` for Claude models
  - `OPENAI_API_KEY` for GPT models
  - `GEMINI_API_KEY` for Gemini models

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/forge.git
cd forge

# Build release binary
cargo build --release

# The binary will be at target/release/forge (or forge.exe on Windows)
# Optionally, copy it to a directory in your PATH
```

### Verifying the Build

```bash
# Run tests to verify everything works
cargo test

# Run linting (optional)
cargo clippy -- -D warnings
```

## Quick Start

### 1. Set Up API Key

Set your API key as an environment variable:

```bash
# For Claude (Anthropic)
export ANTHROPIC_API_KEY="your-key-here"

# For GPT (OpenAI)
export OPENAI_API_KEY="your-key-here"

# For Gemini (Google)
export GEMINI_API_KEY="your-key-here"
```

Or create a configuration file at `~/.forge/config.toml`:

```toml
[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"
```

### 2. Run Forge

```bash
# Run with default settings
cargo run --release

# Or if you installed the binary
forge
```

### 3. Basic Usage

1. **Start typing**: Press `i` to enter Insert mode, type your message.
2. **Send message**: Press `Enter` to send.
3. **Navigate**: Press `Esc` to return to Normal mode, use `j`/`k` to scroll.
4. **Commands**: Press `/` or `:` to enter Command mode.
5. **Quit**: Type `/q` and press `Enter`, or press `q` in Normal mode.

## Configuration

Create `~/.forge/config.toml` for persistent configuration. All settings are optional with sensible defaults.

### Complete Configuration Reference

```toml
[app]
provider = "claude"                    # "claude", "openai", or "gemini"
model = "claude-sonnet-4-5-20250929"   # Model name for the provider
tui = "full"                           # "full" (alternate screen) or "inline"
max_output_tokens = 16000              # Limit model output length

# Accessibility options
ascii_only = false                     # Use ASCII-only glyphs (no Unicode icons)
high_contrast = false                  # High-contrast color palette
reduced_motion = false                 # Disable modal animations

[api_keys]
anthropic = "${ANTHROPIC_API_KEY}"     # Supports environment variable expansion
openai = "${OPENAI_API_KEY}"
google = "${GEMINI_API_KEY}"

[context]
infinity = true                        # Enable adaptive context management

[anthropic]
cache_enabled = true                   # Enable prompt caching (reduces costs)
thinking_enabled = false               # Enable extended thinking
thinking_budget_tokens = 10000         # Token budget for thinking

[openai]
reasoning_effort = "high"              # "low", "medium", or "high" (GPT-5+)
verbosity = "high"                     # "low", "medium", or "high" (GPT-5+)
truncation = "auto"                    # "auto", "none" or "preserve"

[google]
thinking_enabled = true                # Enable thinking (for compatible Gemini models)
cache_enabled = true                   # Enable explicit context caching
cache_ttl_seconds = 3600               # TTL for cached content

[tools]
max_tool_calls_per_batch = 8
max_tool_iterations_per_user_turn = 4

[tools.approval]
mode = "enabled"                       # "disabled", "parse_only", or "enabled"
allowlist = ["ReadFile", "Glob"]       # Skip approval for these tools
denylist = ["RunCommand"]              # Always deny these tools

[tools.sandbox]
allowed_roots = ["."]                  # Allowed base directories
denied_patterns = ["**/.git/**"]       # Excluded glob patterns
allow_absolute = false                 # Block absolute paths

[tools.timeouts]
default_seconds = 30
file_operations_seconds = 30
shell_commands_seconds = 300           # 5 minutes for shell commands

[tools.output]
max_bytes = 102400                     # 100 KB max output per tool

[tools.webfetch]
user_agent = "Forge/1.0"               # Custom User-Agent
timeout_seconds = 30                   # Fetch timeout
max_download_bytes = 10485760          # 10MB limit

[tools.search]
backend = "ugrep"                      # ugrep | ripgrep
max_results = 100                      # Search result limit

```

### Environment Variables

| Variable | Description |
| --- | --- |
| `ANTHROPIC_API_KEY` | Claude API key (fallback if not in config) |
| `OPENAI_API_KEY` | GPT API key (fallback if not in config) |
| `GEMINI_API_KEY` | Gemini API key (fallback if not in config) |
| `FORGE_TUI` | Override TUI mode: `full` or `inline` |
| `FORGE_CONTEXT_INFINITY` | Override context infinity: `1` or `0` |

## Keyboard Shortcuts

### Normal Mode (Navigation)

| Key | Action |
| --- | --- |
| `q` | Quit application |
| `i` | Enter Insert mode |
| `a` | Enter Insert mode at end of line |
| `o` | Enter Insert mode with cleared line |
| `:` or `/` | Enter Command mode |
| `Tab` | Open model selector |
| `j` or `Down` | Scroll down |
| `k` or `Up` | Scroll up |
| `g` | Scroll to top |
| `G` or `End` | Scroll to bottom |

### Insert Mode (Editing)

| Key | Action |
| ----- | -------- |
| `Esc` | Return to Normal mode |
| `Enter` | Send message |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start/end of line |
| `Ctrl+U` | Clear entire line |
| `Ctrl+W` | Delete word backwards |

### Command Mode

| Key | Action |
| ----- | -------- |
| `Esc` | Cancel and return to Normal mode |
| `Enter` | Execute command |
| `Backspace` | Delete last character |

### Model Select Mode

| Key | Action |
| ----- | -------- |
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `1`, `2` | Direct selection by index |

### Tool Approval Mode

| Key | Action |
| ----- | -------- |
| `a` | Approve all tools |
| `d` | Deny all tools |
| `Space` | Toggle individual tool |
| `j` / `k` | Navigate tools |
| `Enter` | Confirm selection |
| `Esc` | Deny all and cancel |

## Commands Reference

Enter Command mode by pressing `:` or `/` in Normal mode.

| Command | Aliases | Description |
| :--- | :--- | :--- |
| `/quit` | `/q` | Exit the application |
| `/clear` | - | Clear conversation and reset context |
| `/cancel` | - | Abort active streaming or tool execution |
| `/model [name]` | - | Set model or open model selector (no argument) |
| `/provider [name]` | `/p` | Switch provider (`claude`, `openai`, or `gemini`) |
| `/context` | `/ctx` | Show context usage statistics |
| `/journal` | `/jrnl` | Show stream journal statistics |
| `/summarize` | `/sum` | Manually trigger summarization |
| `/screen` | - | Toggle between full-screen and inline mode |
| `/tools` | - | List available tools |
| `/tool <id> <result>` | - | Manually submit tool result |
| `/help` | - | Show available commands |

## Workspace Structure

Forge is organized as a Cargo workspace with focused crates:

```text
forge/
├── cli/            # Binary entry point, terminal session management
├── engine/         # Core state machine, commands, streaming orchestration
├── tui/            # Terminal UI rendering (ratatui), input handling
├── context/        # Context Infinity: token counting, summarization, persistence
├── providers/      # LLM API clients (Claude, OpenAI, Gemini)
├── types/          # Core domain types (Message, Provider, ModelName)
├── webfetch/       # Web page fetching and parsing
├── tests/          # Integration tests
└── docs/           # Architecture and design documentation
```

### Crate Responsibilities

| Crate | Purpose |
| --- | --- |
| `cli` | Application entry point, terminal lifecycle, event loop |
| `engine` | Input modes, async operations, tool execution, configuration |
| `tui` | Full-screen and inline rendering, markdown, theming |
| `context` | Token budgeting, summarization, crash recovery journals |
| `providers` | HTTP clients, SSE parsing, provider-specific formatting (Claude, OpenAI, Gemini) |
| `types` | Shared types ensuring compile-time correctness |
| `webfetch` | Chromium-based web fetching for `web_fetch` tool |

## Development

### Build Commands

```bash
cargo check              # Fast type-check (use during development)
cargo build              # Debug build
cargo build --release    # Optimized release build
cargo test               # Run all tests
cargo clippy -- -D warnings  # Lint (run before committing)
```

### Test Coverage

```bash
# One-time setup
cargo install cargo-llvm-cov
rustup component add llvm-tools-preview

# Generate coverage report
cargo cov
# Or: ./scripts/coverage.ps1
```

### Running Specific Tests

```bash
cargo test test_name                    # Single test by name
cargo test -- --nocapture               # With stdout output
cargo test --test integration_test      # Integration tests only
cargo test -p forge-context             # Tests for specific crate
```

## Troubleshooting

### Authentication Errors

**Problem**: "Auth error: set ANTHROPIC_API_KEY" or similar

**Solution**: Ensure your API key is set correctly:

```bash
# Check if the variable is set
echo $ANTHROPIC_API_KEY

# Set it if missing
export ANTHROPIC_API_KEY="sk-ant-..."
```

Or add to `~/.forge/config.toml`:

```toml
[api_keys]
anthropic = "sk-ant-your-actual-key"
```

### Context Budget Exceeded

**Problem**: "Recent messages exceed budget"

**Solution**: Your recent messages are too large for the model's context window. Options:

1. Start a new conversation with `/clear`
2. Switch to a model with a larger context window
3. Write shorter messages

### Tool Access Denied

**Problem**: File operations fail with "outside sandbox"

**Solution**: Ensure the file is within an allowed root:

```toml
[tools.sandbox]
allowed_roots = [".", "/path/to/other/directory"]
```

### Patch Fails with "Stale File"

**Problem**: `apply_patch` fails saying the file is stale

**Solution**: The LLM must read a file before patching it. Ask the assistant to read the file first, then retry the edit.

### Terminal Display Issues

**Problem**: Characters display incorrectly or animations are distracting

**Solution**: Enable accessibility options:

```toml
[app]
ascii_only = true       # Use ASCII-only characters
reduced_motion = true   # Disable animations
high_contrast = true    # Increase contrast
```

### Crash Recovery

If Forge crashes during streaming or tool execution, it will automatically recover on next launch:

- **Stream recovery**: Partial responses are restored with a recovery badge
- **Tool recovery**: You'll be prompted to resume or discard incomplete tool batches

## Documentation

Detailed documentation is available in each crate:

| Document | Description |
| -------- | ----------- |
| [`cli/README.md`](cli/README.md) | CLI entry point and terminal session lifecycle |
| [`engine/README.md`](engine/README.md) | Core state machine and orchestration |
| [`tui/README.md`](tui/README.md) | Terminal UI rendering and input handling |
| [`context/README.md`](context/README.md) | Context Infinity implementation |
| [`providers/README.md`](providers/README.md) | LLM API clients (Claude, OpenAI, Gemini) |
| [`types/README.md`](types/README.md) | Core domain types |

### Design Documents

| Document | Description |
| ---------- | ------------- |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Type-driven design philosophy |
| [`docs/CONTEXT_INFINITY_SRD.md`](docs/CONTEXT_INFINITY_SRD.md) | Context Infinity specification |
| [`docs/TOOL_EXECUTOR_SRD.md`](docs/TOOL_EXECUTOR_SRD.md) | Tool Executor requirements |
| [`docs/TOOLS.md`](docs/TOOLS.md) | User guide for tool configuration |
| [`docs/LP1.md`](docs/LP1.md) | Line Patch v1 format specification |

## Contributing

Contributions are welcome! Please ensure:

1. **Code compiles**: `cargo check` passes
2. **Tests pass**: `cargo test` succeeds
3. **Linting passes**: `cargo clippy -- -D warnings` reports no errors
4. **Commit style**: Use conventional commits with type and scope
   - Example: `feat(context): add token budget display`
   - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## License

This project is currently unlicensed. Please contact the maintainer for licensing information.

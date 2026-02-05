# Forge

A vim-modal terminal user interface for interacting with Claude, GPT, and Gemini, featuring adaptive context management and an agentic tool execution framework.

Forge brings the efficiency of vim-style modal editing to AI conversation, letting you navigate, compose, and manage conversations without leaving your terminal. With Context Infinity™, Forge automatically distills older messages to stay within model limits while preserving full conversation history. The Tool Executor Framework enables the LLM to read files, apply patches, and run shell commands - all with interactive approval and crash recovery.

## LLM-TOC
<!-- toc:start -->
| Lines | Section |
| --- | --- |
| 7-26 | LLM-TOC |
| 27-61 | Features: Core Capabilities, Context Infinity, Tool Executor |
| 62-71 | Requirements |
| 72-97 | Installation |
| 98-141 | Quick Start: First Run, Basic Usage |
| 142-269 | Configuration: Full Reference |
| 270-362 | Keyboard Shortcuts: All Modes |
| 363-381 | Commands Reference |
| 382-412 | Workspace Structure |
| 413-447 | Development |
| 448-517 | Troubleshooting |
| 518-557 | Documentation Index |
| 558-568 | Contributing and License |
| 569-572 | License |
<!-- toc:end -->

## Features

### Core Capabilities

- **Vim-style Modal Interface**: Navigate with Normal mode, edit with Insert mode, run commands with Command mode, and switch models with ModelSelect mode
- **Multi-Provider Support**: Seamless switching between Claude (Anthropic), GPT (OpenAI), and Gemini (Google) with provider-specific optimizations
- **Full/Inline Display Modes**: Full-screen alternate buffer or inline terminal mode that preserves your scrollback
- **Rich Markdown Rendering**: Tables with box-drawing borders, syntax-highlighted code blocks, lists, and formatting
- **Streaming Responses**: Real-time token streaming with animated indicators

### Context Infinity™

Forge's adaptive context management system keeps conversations flowing without hitting model limits:

- **Automatic Distillation**: When context fills up, older messages are compressed into distillates that preserve key information
- **Never Lose History**: Original messages are preserved and can be restored when switching to models with larger context windows
- **Crash Recovery**: Streaming responses are journaled to SQLite before display, so crashes never lose your work
- **Token Usage Display**: Real-time visibility into context usage with color-coded warnings

### Tool Executor Framework

Enable the LLM to interact with your local filesystem and execute tasks:

- **Built-in Tools**:
  - File operations: `Read`, `Write`, `Edit` (LP1 patches), `Glob`
  - Search: `Search` (aliases: `search`, `rg`, `ripgrep`, `ugrep`, `ug`)
  - Shell: `Run`
  - Web: `WebFetch`
  - Context: `Recall` (retrieve facts from conversation)
  - Git: `GitStatus`, `GitDiff`, `GitAdd`, `GitCommit`, `GitLog`, `GitBranch`, `GitCheckout`, `GitStash`, `GitShow`, `GitBlame`, `GitRestore`
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
model = "claude-opus-4-6"   # Model name (provider inferred from prefix)
tui = "full"                           # "full" (alternate screen) or "inline"
show_thinking = false                  # Render provider thinking/reasoning in UI

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

[cache]
enabled = true                         # Legacy prompt caching (Claude only)

[thinking]
enabled = false                        # Legacy extended thinking toggle
budget_tokens = 10000                  # Legacy thinking budget (min 1024)

[anthropic]
cache_enabled = true                   # Enable prompt caching (reduces costs)
thinking_enabled = false               # Enable extended thinking
thinking_budget_tokens = 10000         # Token budget for thinking

[openai]
reasoning_effort = "high"              # "low", "medium", "high", or "xhigh" (GPT-5+)
reasoning_summary = "auto"                # "none", "auto", "concise", "detailed" (GPT-5+, shown when show_thinking=true)
verbosity = "high"                     # "low", "medium", or "high" (GPT-5+)
truncation = "auto"                    # "auto" or "disabled"
# gpt-5.2-pro defaults to xhigh when reasoning_effort is unset.

[google]
thinking_enabled = true                # Enable thinking (for compatible Gemini models)
cache_enabled = true                   # Enable explicit context caching
cache_ttl_seconds = 3600               # TTL for cached content

[tools]
max_tool_calls_per_batch = 8
max_tool_iterations_per_user_turn = 4

[tools.approval]
mode = "default"                       # "permissive", "default", or "strict"
allowlist = ["Read", "GitStatus", "GitDiff", "GitLog", "GitShow", "GitBlame"]  # Skip approval for these tools
denylist = ["Run"]                     # Always deny these tools

[tools.environment]
denylist = ["*_KEY", "*_TOKEN", "*_SECRET", "*_PASSWORD", "AWS_*", "ANTHROPIC_*", "OPENAI_*"]  # Case-insensitive patterns

[tools.sandbox]
allowed_roots = ["."]                  # Allowed base directories
denied_patterns = ["**/.git/**"]       # Excluded glob patterns (in addition to defaults)
allow_absolute = false                 # Block absolute paths
include_default_denies = true          # Include built-in deny patterns

[tools.timeouts]
default_seconds = 30
file_operations_seconds = 30
shell_commands_seconds = 300           # 5 minutes for shell commands

[tools.output]
max_bytes = 65536                      # 64 KB max output per tool

[tools.webfetch]
user_agent = "forge-webfetch/1.0"      # Custom User-Agent
timeout_seconds = 20                   # Fetch timeout
max_download_bytes = 10485760          # 10MB limit
max_redirects = 5                      # Max HTTP redirects
default_max_chunk_tokens = 600         # Token budget per chunk
cache_dir = "/tmp/webfetch"            # Cache directory
cache_ttl_days = 7                     # Cache TTL in days

[tools.search]
binary = "ugrep"                       # Search binary: "ugrep" or "rg" (ripgrep)
fallback_binary = "rg"                 # Fallback if primary not found
default_max_results = 200              # Search result limit
default_timeout_ms = 20000             # Search timeout
max_matches_per_file = 50              # Max matches per file
max_files = 10000                      # Max files to search
max_file_size_bytes = 2000000          # Skip files larger than 2MB

[tools.shell]
binary = "pwsh"                        # Override shell binary
args = ["-NoProfile", "-Command"]      # Override shell args

[tools.read_file]
max_file_read_bytes = 204800           # Max bytes to read per file
max_scan_bytes = 2097152               # Max bytes to scan for line counting

[tools.apply_patch]
max_patch_bytes = 524288               # Max patch size in bytes

[[tools.definitions]]
name = "get_weather"
description = "Get current weather for a location"
[tools.definitions.parameters]
type = "object"
[tools.definitions.parameters.properties.location]
type = "string"
description = "City name, e.g. 'Seattle, WA'"

```

### Environment Variables

| Variable | Description |
| --- | --- |
| `ANTHROPIC_API_KEY` | Claude API key (fallback if not in config) |
| `OPENAI_API_KEY` | GPT API key (fallback if not in config) |
| `GEMINI_API_KEY` | Gemini API key (fallback if not in config) |
| `FORGE_TUI` | Override TUI mode: `full` or `inline` |
| `FORGE_CONTEXT_INFINITY` | Override context infinity: `1` or `0` |
| `FORGE_STREAM_IDLE_TIMEOUT_SECS` | Override streaming idle timeout in seconds (default: 60) |
| `FORGE_STREAM_JOURNAL_FLUSH_THRESHOLD` | Override stream journal flush threshold in deltas (default: 25) |
| `FORGE_STREAM_JOURNAL_FLUSH_INTERVAL_MS` | Override stream journal flush interval in ms (default: 200) |

## Keyboard Shortcuts

### Normal Mode (Navigation)

| Key | Action |
| --- | --- |
| `q` | Quit application |
| `i` | Enter Insert mode |
| `a` | Enter Insert mode at end of line |
| `o` | Toggle thinking visibility |
| `:` or `/` | Enter Command mode |
| `m` | Open model selector |
| `f` | Toggle files panel |
| `s` | Toggle screen mode (fullscreen/inline) |
| `j`, `Down`, or scroll wheel | Scroll down |
| `k`, `Up`, or scroll wheel | Scroll up |
| `PageDown` or `Ctrl+D` | Scroll page down |
| `PageUp` or `Ctrl+U` | Scroll page up |
| `g` | Scroll to top |
| `G`, `End`, or `Right` | Scroll to bottom |
| `Left` | Scroll up by chunk (20%) |
| `Tab` / `Shift+Tab` | Files panel: next/previous file (when visible) |
| `Enter` / `Esc` | Files panel: collapse expanded diff |

### Insert Mode (Editing)

| Key | Action |
| --- | --- |
| `Esc` | Return to Normal mode |
| `Enter` | Send message |
| `Ctrl+Enter`, `Shift+Enter`, `Ctrl+J` | Insert newline (multiline input) |
| `Up` / `Down` | Navigate prompt history |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start/end of line |
| `Ctrl+U` | Clear entire line |
| `Ctrl+W` | Delete word backwards |
| `@` | Open file selector |

### Command Mode

| Key | Action |
| --- | --- |
| `Esc` | Cancel and return to Normal mode |
| `Enter` | Execute command |
| `Up` / `Down` | Navigate command history |
| `Tab` | Tab completion |
| `Backspace` | Delete last character |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start/end of line |
| `Ctrl+A` / `Ctrl+E` | Jump to start/end of line |
| `Ctrl+U` | Clear line |
| `Ctrl+W` | Delete word backwards |

### Model Select Mode

| Key | Action |
| --- | --- |
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `1`-`9` | Direct selection by index |

### File Select Mode

| Key | Action |
| --- | --- |
| `Esc` | Cancel and return to Insert mode |
| `Enter` | Insert selected file path |
| `Up` / `Down` | Move selection |
| `Backspace` | Delete filter character (or cancel if empty) |
| Typing | Filter file list |

### Tool Approval Mode

| Key | Action |
| --- | --- |
| `a` | Approve all tools |
| `d` or `Esc` | Deny all tools |
| `Space` | Toggle individual tool |
| `Tab` | Toggle tool details |
| `j` / `k` or `Up` / `Down` | Navigate tools |
| `Enter` | Confirm selection |

### Tool Recovery Mode

| Key | Action |
| --- | --- |
| `r` or `R` | Resume interrupted tool batch |
| `d`, `D`, or `Esc` | Discard interrupted tool batch |

## Commands Reference

Enter Command mode by pressing `:` or `/` in Normal mode.

| Command | Aliases | Description |
| :--- | :--- | :--- |
| `/quit` | `/q` | Exit the application |
| `/clear` | - | Clear conversation and reset context |
| `/cancel` | - | Abort active streaming, tool execution, or distillation |
| `/model [name]` | - | Set model or open model selector (no argument) |
| `/context` | `/ctx` | Show context usage statistics |
| `/journal` | `/jrnl` | Show stream journal statistics |
| `/distill` | - | Manually trigger distillation |
| `/screen` | - | Toggle between full-screen and inline mode |
| `/rewind [id\|last] [scope]` | `/rw` | Rewind to an automatic checkpoint (scope: `code`, `conversation`, or `both`) |
| `/undo` | - | Undo the last user turn (rewind to last turn checkpoint) |
| `/retry` | - | Undo the last user turn and restore its prompt into the input box |
| `/help` | - | Show available commands |

## Workspace Structure

Forge is organized as a Cargo workspace with focused crates:

```text
forge/
├── cli/            # Binary entry point, terminal session management
├── context/        # Context Infinity: token counting, distillation, persistence
├── engine/         # Core state machine, commands, streaming orchestration
├── providers/      # LLM API clients (Claude, OpenAI, Gemini)
├── tui/            # Terminal UI rendering (ratatui), input handling
├── types/          # Core domain types (Message, Provider, ModelName)
├── webfetch/       # Web page fetching and parsing
│
├── tests/          # Integration tests (not a workspace crate)
├── docs/           # Architecture and design documentation
└── scripts/        # Development and maintenance scripts
```

### Crate Responsibilities

| Crate | Purpose |
| --- | --- |
| `cli` | Application entry point, terminal lifecycle, event loop |
| `engine` | Input modes, async operations, tool execution, configuration |
| `tui` | Full-screen and inline rendering, markdown, theming |
| `context` | Token budgeting, distillation, crash recovery journals |
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
cargo clippy --workspace --all-targets -- -D warnings  # Lint (run before committing)
cargo cov                # Coverage report (requires cargo-llvm-cov)
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
cargo test --test all                   # Integration aggregator
cargo test --test ui_snapshots          # Snapshot tests
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
| [`webfetch/README.md`](webfetch/README.md) | Web page fetching and HTML-to-markdown |
| [`scripts/README.md`](scripts/README.md) | Development and maintenance scripts |
| [`tests/README.md`](tests/README.md) | Integration test structure and guidelines |

### Design Documents

| Document | Description |
| -------- | ----------- |
| [`DESIGN.md`](DESIGN.md) | Type-driven design philosophy |
| [`INVARIANT_FIRST_ARCHITECTURE.md`](INVARIANT_FIRST_ARCHITECTURE.md) | Making invalid states unrepresentable |
| [`docs/LP1.md`](docs/LP1.md) | Line Patch v1 format specification |
| [`docs/ANTHROPIC_MESSAGES_API.md`](docs/ANTHROPIC_MESSAGES_API.md) | Claude API reference |
| [`docs/OPENAI_RESPONSES_GPT52.md`](docs/OPENAI_RESPONSES_GPT52.md) | OpenAI Responses API integration |
| [`docs/RUST_2024_REFERENCE.md`](docs/RUST_2024_REFERENCE.md) | Rust 2024 edition features |

### Additional Specs

| Document | Description |
| -------- | ----------- |
| [`docs/SEARCH_SRS.md`](docs/SEARCH_SRS.md) | Search tool specification |
| [`docs/SEARCH_INDEXING_SRS.md`](docs/SEARCH_INDEXING_SRS.md) | Search indexing and change tracking |
| [`docs/CONVERSATION_SEARCH_SRS.md`](docs/CONVERSATION_SEARCH_SRS.md) | Conversation search requirements |
| [`docs/FILEOPS_SRS.md`](docs/FILEOPS_SRS.md) | File operations specification |
| [`docs/OUTLINE_TOOL_SRS.md`](docs/OUTLINE_TOOL_SRS.md) | Outline tool specification |
| [`docs/PWSH_TOOL_SRS.md`](docs/PWSH_TOOL_SRS.md) | PowerShell tool specification |
| [`docs/SUBAGENT_SRS.md`](docs/SUBAGENT_SRS.md) | Subagent delegation specification |
| [`docs/COLOR_SCHEME.md`](docs/COLOR_SCHEME.md) | Color scheme documentation |

## Contributing

Contributions are welcome! Please ensure:

1. **Code compiles**: `cargo check` passes
2. **Tests pass**: `cargo test` succeeds
3. **Linting passes**: `cargo clippy --workspace --all-targets -- -D warnings` reports no errors
4. **Commit style**: Use conventional commits with type and scope
   - Example: `feat(context): add token budget display`
   - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## License

This project is currently unlicensed. Please contact the maintainer for licensing information.


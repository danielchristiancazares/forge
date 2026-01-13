# Forge Tools Guide

This guide covers configuring and using the Tool Executor Framework in Forge. For the technical specification, see [TOOL_EXECUTOR_SRD.md](TOOL_EXECUTOR_SRD.md).

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-30 | Header & TOC |
| 31-44 | Overview |
| 45-84 | Quick Start |
| 85-99 | Tool Modes |
| 100-196 | Built-in Tools |
| 197-282 | Configuration Reference |
| 283-306 | Security Model |
| 307-336 | Approval Workflow |
| 337-371 | Crash Recovery |
| 372-417 | Troubleshooting |
| 418-433 | Commands Reference & Links |

## Table of Contents

1. [Overview](#overview)
2. [Quick Start](#quick-start)
3. [Tool Modes](#tool-modes)
4. [Built-in Tools](#built-in-tools)
5. [Configuration Reference](#configuration-reference)
6. [Security Model](#security-model)
7. [Approval Workflow](#approval-workflow)
8. [Crash Recovery](#crash-recovery)
9. [Troubleshooting](#troubleshooting)

---

## Overview

The Tool Executor Framework enables the LLM to interact with your local filesystem and execute shell commands. When enabled, the assistant can:

- **Read files** to understand your codebase
- **Apply patches** to edit files using the LP1 format
- **Write new files** when creating code or assets
- **Run shell commands** to build, test, and execute tasks

All tools are:

- **Sandboxed** to allowed directories (by default, the current working directory)
- **Approval-gated** for side-effecting operations
- **Journaled** for crash recovery

---

## Quick Start

### 1. Enable Tools

Add to your `~/.forge/config.toml`:

```toml
[tools]
mode = "enabled"

[tools.sandbox]
allowed_roots = ["."]  # Restrict to current directory
```

### 2. Test Read Access

Ask the assistant to read a file:

```
Read the contents of Cargo.toml
```

The assistant will call `read_file` and display the file contents.

### 3. Test Patching (with Approval)

Ask the assistant to make an edit:

```
Add a comment at the top of src/main.rs
```

An **approval prompt** will appear showing the proposed patch. Press:

- `Enter` to approve all
- `d` to deny all
- `Space` to toggle individual tools

---

## Tool Modes

| Mode | Behavior |
|------|----------|
| `disabled` | Tools are not advertised; tool calls are rejected |
| `parse_only` | Tool calls are displayed but not executed; user supplies results manually via `:tool` command |
| `enabled` | Tool calls are validated, approved if required, and executed automatically |

```toml
[tools]
mode = "parse_only"  # For manual control
```

---

## Built-in Tools

### `read_file`

Reads file contents within the sandbox.

| Property | Value |
|----------|-------|
| Side-effecting | No |
| Requires approval | No |
| Risk level | Low |

**Parameters:**

- `path` (string, required) — Path to file
- `start_line` (u32, optional, 1-indexed) — First line to read
- `end_line` (u32, optional, 1-indexed) — Last line to read

**Limits:**

- Max file read: 200 KB (configurable)
- Max line scan: 2 MB (for line-range reads)

**Binary files:** Automatically detected and returned as base64-encoded data.

---

### `apply_patch`

Applies LP1 patches to files within the sandbox. See [LP1.md](LP1.md) for the patch format.

| Property | Value |
|----------|-------|
| Side-effecting | Yes |
| Requires approval | Yes (unless allowlisted) |
| Risk level | Medium |

**Parameters:**

- `patch` (string, required) — LP1-formatted patch

**Safety features:**

- **Stale file protection**: Files must have been read in the current conversation before patching
- **SHA validation**: File content must not have changed since last read
- **Atomic writes**: Uses temp file + rename to prevent partial writes
- **Auto-backup**: Original files are backed up before modification

---

### `write_file`

Creates a new file within the sandbox.

| Property | Value |
|----------|-------|
| Side-effecting | Yes |
| Requires approval | Yes (unless allowlisted) |
| Risk level | Medium |

**Parameters:**

- `path` (string, required) — Path to new file
- `content` (string, required) — File contents

**Notes:**

- Fails if the file already exists (create-new only).
- Use `apply_patch` to modify existing files.

---

### `run_command`

Executes shell commands using the platform shell.

| Property | Value |
|----------|-------|
| Side-effecting | Yes |
| Requires approval | **Always** |
| Risk level | High |

**Parameters:**

- `command` (string, required) — Shell command to execute

**Default policy:** Denylisted. You must explicitly enable it:

```toml
[tools.approval]
denylist = []  # Remove run_command from denylist
```

**Environment sanitization:** Sensitive environment variables (API keys, tokens, secrets) are automatically stripped before command execution.

---

## Configuration Reference

### `[tools]`

```toml
[tools]
mode = "enabled"                    # disabled | parse_only | enabled
max_tool_calls_per_batch = 8        # Limit concurrent tool calls
max_tool_iterations_per_user_turn = 4  # Limit tool loop iterations
```

### `[tools.sandbox]`

```toml
[tools.sandbox]
allowed_roots = ["."]               # Directories tools can access
denied_patterns = ["**/.ssh/**"]    # Glob patterns to always deny
allow_absolute = false              # Reject absolute paths
include_default_denies = true       # Include default deny patterns
```

**Default deny patterns:**

- `**/.ssh/**`
- `**/.gnupg/**`
- `**/id_rsa*`
- `**/*.pem`
- `**/*.key`

### `[tools.approval]`

```toml
[tools.approval]
enabled = true                      # Enable approval workflow
mode = "prompt"                     # auto | prompt | deny
allowlist = ["read_file"]           # Tools that skip prompting
denylist = ["run_command"]          # Tools that are always denied
prompt_side_effects = true          # Prompt for all side-effecting tools
```

**Approval modes:**

- `auto`: Execute tools unless denylisted
- `prompt`: Prompt for confirmation per policy
- `deny`: Deny all tools unless explicitly allowlisted

### `[tools.timeouts]`

```toml
[tools.timeouts]
default_seconds = 30                # Fallback timeout
file_operations_seconds = 30        # For read_file, apply_patch
shell_commands_seconds = 300        # For run_command (5 minutes)
```

### `[tools.output]`

```toml
[tools.output]
max_bytes = 102400                  # 100 KB max output per tool
```

### `[tools.environment]`

```toml
[tools.environment]
denylist = ["*_KEY", "*_TOKEN", "*_SECRET", "*_PASSWORD", "AWS_*", "ANTHROPIC_*", "OPENAI_*"]
```

### `[tools.read_file]`

```toml
[tools.read_file]
max_file_read_bytes = 204800        # 200 KB
max_scan_bytes = 2097152            # 2 MB
```

### `[tools.apply_patch]`

```toml
[tools.apply_patch]
max_patch_bytes = 524288            # 512 KB
```

---

## Security Model

### Filesystem Sandbox

All file paths are validated before access:

1. **Lexical traversal blocked**: `..` components are rejected
2. **Symlink escape prevention**: Every path component is checked for symlinks
3. **Root containment**: Final canonical path must be within `allowed_roots`
4. **Pattern matching**: Denied patterns are applied to canonical paths

### Environment Sanitization

The `run_command` tool strips sensitive environment variables:

- Anything matching `*_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`
- Cloud credentials (`AWS_*`, `ANTHROPIC_*`, `OPENAI_*`)

### Output Sanitization

All tool outputs are sanitized to prevent terminal control injection before rendering.

---

## Approval Workflow

When a tool batch requires approval, an interactive overlay appears:

```
╭─────────────────────────────────────╮
│  Tool Approval Required              │
│                                      │
│  ⏸ apply_patch (call_001)           │
│      Apply patch to 2 file(s)        │
│                                      │
│  ⏸ run_command (call_002)           │
│      Run command: cargo build        │
│                                      │
│  [a]pprove all  [d]eny all          │
│  [Space] toggle  [Enter] confirm     │
╰─────────────────────────────────────╯
```

**Keyboard shortcuts:**

- `a` — Approve all tools in the batch
- `d` — Deny all tools in the batch
- `j`/`k` or `↓`/`↑` — Navigate between tools
- `Space` — Toggle approval for highlighted tool
- `Enter` — Execute the approved selection
- `Esc` — Deny all and cancel

---

## Crash Recovery

Tool execution is journaled to SQLite for crash recovery.

### What's Saved

- Tool calls (ID, name, arguments)
- Tool results (as they complete)
- Batch state (which tools were approved)

### Recovery Prompt

If Forge crashes during tool execution, you'll see a recovery prompt on next launch:

```
╭─────────────────────────────────────╮
│  Recovered Tool Batch                │
│                                      │
│  Previous session crashed during     │
│  tool execution.                     │
│                                      │
│  2 of 3 tools completed:            │
│  ✓ read_file                         │
│  ✓ apply_patch                       │
│  • run_command (not started)         │
│                                      │
│  [r]esume with partial results       │
│  [d]iscard and mark failed           │
╰─────────────────────────────────────╯
```

**Important:** Recovery does NOT re-execute tools. Side effects may have already occurred.

---

## Troubleshooting

### Tool is denied but I want to use it

Check your denylist and allowlist:

```toml
[tools.approval]
denylist = []                        # Remove from denylist
allowlist = ["run_command"]          # Or add to allowlist
```

### File read fails with "outside sandbox"

Ensure the file is within an `allowed_roots` path:

```toml
[tools.sandbox]
allowed_roots = [".", "${HOME}/projects"]
```

### Patch fails with "stale file"

The LLM must read a file before patching it. Ask the assistant to read the file first, then retry the edit.

### Tool output is truncated

Increase the output limit:

```toml
[tools.output]
max_bytes = 524288                   # 512 KB
```

### Commands fail with missing environment variables

Sensitive variables are stripped. If you need a specific variable:

```toml
[tools.environment]
# Remove the pattern that matches your variable
denylist = ["*_SECRET", "*_PASSWORD"]  # Removed *_KEY
```

---

## Commands Reference

| Command | Description |
|---------|-------------|
| `:tools` | List configured tools and their schemas |
| `:tool <id> <result>` | Manually submit a tool result (parse_only mode) |
| `:tool error <id> <message>` | Submit an error result |

---

## See Also

- [TOOL_EXECUTOR_SRD.md](TOOL_EXECUTOR_SRD.md) — Technical requirements specification
- [LP1.md](LP1.md) — Line Patch v1 format grammar
- [engine/README.md](../engine/README.md) — Tool loop state machine

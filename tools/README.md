# Forge Tools (`forge-tools`)

`forge-tools` provides the tool execution framework and built-in tools used by Forge.

## Responsibilities

- Define the executor interface (`ToolExecutor`) and execution context (`ToolCtx`).
- Own tool registration and dispatch (`ToolRegistry`).
- Enforce sandbox and policy constraints before execution.
- Emit structured tool events for streaming tool output.
- Provide built-in tools (filesystem, shell, search, git, web fetch, memory/recall, and platform-specific runners).

## Key Types

- `ToolRegistry`: name-to-executor registry with schema lookup and dispatch.
- `ToolExecutor`: trait implemented by each tool backend.
- `ToolSettings`: runtime policy/sandbox/limit configuration.
- `ToolError`: typed error surface for validation, timeout, sandbox denial, and execution failures.
- `ApprovalMode` / `Policy` / `ConfirmationRequest`: approval gating model.

## Notable Modules

- `builtins.rs`: built-in tool implementations and registration wiring.
- `sandbox.rs`: path/command sandbox checks and policy enforcement.
- `shell.rs`, `search.rs`, `git.rs`: core operational tools.
- `webfetch/`: URL fetch pipeline (robots, HTTP, extraction, caching, chunking).
- `windows_run*.rs`: Windows run-host and sandbox integration.
- `change_recording.rs`: per-turn file-change recording support.

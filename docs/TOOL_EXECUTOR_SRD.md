# Tool Executor Framework

## Software Requirements Document

**Version:** 1.12
**Date:** 2026-01-12
**Status:** Final
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-46 | Header, Change Log (ref) |
| 47-110 | Section 1 - Introduction: purpose, scope, definitions, references |
| 111-195 | Section 2 - Overall Description: product perspective, functions, constraints |
| 196-274 | Section 3.1 - Tool Executor Trait: trait definition, schema validation, typed parsing |
| 275-375 | Section 3.2-3.3 - Registry and Context: ToolRegistry, ToolCtx, SharedToolCtx |
| 376-489 | Section 3.4 - Approval Workflow: outcomes, policy, precedence, planning-time validation |
| 490-546 | Section 3.5 - Sandbox: Sandbox struct, root policies, symlink/junction defense |
| 547-751 | Section 3.6 - Execution: sequential execution, timeout, cancellation, child cleanup |
| 752-909 | Section 3.7-3.9 - Output, Resume, TUI: persistence, crash recovery, UI display |
| 910-1132 | Sections 4-8 - Errors, NFRs, Config, Verification, Appendices |

---

## 0. Change Log

### 0.1 Changes since v1.9

* Added **FR-TOOL-PATCH-02**: `apply_patch` MUST validate file SHA matches last `read_file` before applying; added `ToolError::StaleFile`.
* Changed **FR-EXE-05** from SHOULD to MUST for Unix process group termination; specified `setsid()`/`killpg()` pattern.
* Strengthened **FR-EXE-04b** panic safety: framework MUST use `catch_unwind`; executors MUST be `UnwindSafe`.

### 0.2 Changes since v1.8

* Added explicit **tools loop modes** (`disabled | parse_only | enabled`) and clarified behavior for each mode.
* Clarified **tool definition source of truth**: registry-backed tools for auto-execution; config-defined tools supported for `parse_only`.
* Added missing **durability requirements**: tool call events and pending batches MUST be journaled/persisted for crash recovery (previously unspecified).
* Fixed ambiguity in **timeouts**: per-tool executor timeout is preferred; `ToolCtx.default_timeout` is a fallback only.
* Tightened **output truncation** so final output (including marker) MUST NOT exceed effective limits.
* Added missing **DoS/abuse limits**: max tool calls per batch, max arguments size, max patch size, max file scan bytes, etc.
* Clarified **apply_patch atomicity**, symlink safety, and file write semantics.
* Added explicit **sanitization** requirements for tool outputs and streaming chunks to prevent terminal injection.
* Clarified **multi-tool ordering** requirements for persistence and provider adapters.

### 0.3 Changes since v1.10

* Added `list_directory` to the initial built-in tool set and sandbox applicability notes.

### 0.4 Changes since v1.11

* Added `write_file` to the initial built-in tool set and sandbox applicability notes.

---

## 1. Introduction

### 1.1 Purpose

This document specifies requirements for the Tool Executor Framework, a subsystem of Forge that enables automatic execution of tool calls requested by LLM providers. It is the authoritative reference for implementation, testing, and validation.

### 1.2 Scope

The Tool Executor Framework:

* Receives tool call requests from LLM streaming responses (as `ToolCall`)
* Validates, approves, and executes tools in a sandboxed environment (where applicable)
* Returns `ToolResult` objects to the conversation context and (in enabled mode) resumes the LLM conversation

**In Scope**

* Tool registration, discovery, and schema generation
* Tool loop modes: disabled / parse-only / enabled
* Approval workflow (auto-approve, prompt, deny)
* Filesystem sandboxing for path-based tools (`read_file`, `apply_patch`, `write_file`, `list_directory`)
* Sequential execution with timeout and cancellation
* Streaming output for long-running tools (`run_command`)
* Durability / crash recovery integration for tool batches (calls + results)
* Initial tool set: `read_file`, `apply_patch`, `write_file`, and `list_directory`; `run_command` is specified but MUST be denylisted by default

**Out of Scope**

* Network-based tools (web search, HTTP requests)
* Plugin/dynamic loading of external tools (runtime loading)
* Implementing arbitrary third-party tool ecosystems (e.g., MCP) in this version
* Provider-specific API correctness beyond the normalized tool call/result model (however, adapter ordering invariants are in-scope as dependencies)

### 1.3 Definitions

| Term            | Definition                                                                                           |
| --------------- | ---------------------------------------------------------------------------------------------------- |
| **Tool**        | A callable function that the LLM can invoke to perform actions                                       |
| **Tool Call**   | A request from the LLM to execute a tool with arguments (`ToolCall`)                                 |
| **Tool Result** | Output of tool execution (`ToolResult`)                                                              |
| **Executor**    | Component implementing a tool’s behavior (`ToolExecutor`)                                            |
| **Registry**    | Mapping from tool name → executor, and generator of tool schemas                                     |
| **Sandbox**     | Security boundary restricting filesystem access for path-based tools                                 |
| **Approval**    | User consent gate before executing higher-risk tools                                                 |
| **Policy**      | Config-driven approval rules (allow/deny/prompt)                                                     |
| **Tool Batch**  | A set of tool calls produced by a single assistant response                                          |
| **Tool Loop**   | The iterative cycle: assistant emits tool calls → tools run → results returned → assistant continues |

### 1.4 References

| Document                         | Description                                          |
| -------------------------------- | ---------------------------------------------------- |
| `engine/README.md`    | Engine state machine specification                   |
| `docs/LP1.md`                    | Line Patch v1 edit format specification              |
| `providers/README.md` | LLM provider integration                             |
| `context/README.md`              | Context subsystem and tool-mode configuration intent |
| RFC 2119                         | Key words for requirement levels                     |
| RFC 8174                         | Clarification of RFC 2119 (uppercase only)           |

### 1.5 Requirement Level Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in BCP 14 (RFC 2119 / RFC 8174) when, and only when, they appear in all capitals.

---

## 2. Overall Description

### 2.1 Product Perspective

The Tool Executor Framework integrates into the Forge engine state machine around the existing “tool waiting” phase:

```
Idle → Streaming → ToolLoop (planning/approval/execution) → Streaming (resume)
```

The “ToolLoop” phase is entered when the streaming assistant response contains ≥1 tool call.

### 2.2 Product Functions

| Function   | Description                                                      |
| ---------- | ---------------------------------------------------------------- |
| **FR-REG** | Tool registration, schema generation, definitions export         |
| **FR-VAL** | Validation (tool existence, JSON schema, sandbox/path preflight) |
| **FR-APR** | Approval planning and user consent                               |
| **FR-EXE** | Execution (sequential, timeout, cancellation, child cleanup)     |
| **FR-OUT** | Output streaming + truncation + sanitization                     |
| **FR-RES** | Persist tool calls/results and resume conversation               |

### 2.3 User Characteristics

| User          | Interaction                                                        |
| ------------- | ------------------------------------------------------------------ |
| **LLM**       | Emits tool calls via provider function-calling semantics           |
| **End User**  | Approves/denies risky calls; observes streaming output; can cancel |
| **Developer** | Adds new tool executors and tests                                  |

### 2.4 Constraints

| Constraint | Rationale                                                                  |
| ---------- | -------------------------------------------------------------------------- |
| **C-01**   | Object-safe trait required for dynamic registry                            |
| **C-02**   | Cross-platform (Windows/macOS/Linux)                                       |
| **C-03**   | Async-compatible with Tokio runtime                                        |
| **C-04**   | No external network dependencies for core tools                            |
| **C-05**   | Tool outputs are untrusted and MUST be sanitized before terminal rendering |

### 2.5 Assumptions and Dependencies

| ID       | Assumption/Dependency                                                                            |
| -------- | ------------------------------------------------------------------------------------------------ |
| **A-01** | `ToolCall` / `ToolResult` / `ToolDefinition` types exist and are stable (`forge-types`)          |
| **A-02** | Providers normalize tool call streams into `StreamEvent::ToolCallStart/Delta` (per provider)     |
| **D-01** | JSON schema generation via `schemars` (or equivalent)                                            |
| **D-02** | JSON schema validation requires a validator crate (e.g., `jsonschema`)                           |
| **D-03** | Crash recovery uses SQLite via existing persistence primitives; tool journaling may extend these |
| **A-03** | LP1 parsing/applying library is available (or will be implemented) to back `apply_patch`         |

> Note: If a provider does not yet emit `StreamEvent::ToolCallStart/Delta`, the Tool Executor Framework still functions for other providers but tool calls from that provider will not be auto-executed until normalization exists.

### 2.6 Tool Loop Modes

The system SHALL support the following modes:

```rust
pub enum ToolsMode {
    Disabled,   // Tools are not advertised; tool calls are rejected
    ParseOnly,  // Tool calls are captured; user supplies results manually
    Enabled,    // Tool calls are validated/approved/executed automatically
}
```

**Mode requirements**

* **FR-MODE-01:** When `ToolsMode::Disabled`, the engine MUST NOT advertise any tools to providers and MUST treat any received tool calls as invalid input (see FR-VAL-06 for handling).
* **FR-MODE-02:** When `ToolsMode::ParseOnly`, the engine MUST advertise tools (if configured) and MUST surface pending calls to the user, but MUST NOT execute tools automatically.
* **FR-MODE-03:** When `ToolsMode::Enabled`, the engine MUST validate, gate, and execute tool calls per this SRD and MUST resume the LLM conversation after results are committed (FR-RES-01).

### 2.7 Safety Limits and Defaults

To reduce abuse and runaway loops, the system SHALL enforce:

* **FR-LIM-01:** `max_tool_calls_per_batch` (default: **8**)
* **FR-LIM-02:** `max_tool_iterations_per_user_turn` (default: **4**)
* **FR-LIM-03:** `max_tool_args_bytes` (default: **256 KB**) measured on JSON-serialized arguments
* **FR-LIM-04:** `max_apply_patch_bytes` (default: **512 KB**)
* **FR-LIM-05:** `max_read_file_scan_bytes` for line-range reads (default: **2 MB**) to cap scanning cost
* **FR-LIM-06:** Exceeding a limit MUST pre-resolve the offending call(s) as `ToolResult::error` with a clear message and MUST continue the loop for remaining calls (FR-EXE-02).

---

## 3. Functional Requirements

### 3.1 Tool Executor Trait

#### 3.1.1 Trait Definition (FR-REG-01)

**Requirement:** The system SHALL provide an object-safe trait for tool execution.

```rust
pub type ToolFut<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + 'a>>;

pub trait ToolExecutor: Send + Sync {
    /// Unique tool name (must match advertised schema)
    fn name(&self) -> &'static str;

    /// Human-readable description for tool schema
    fn description(&self) -> &'static str;

    /// JSON Schema for tool parameters
    fn schema(&self) -> serde_json::Value;

    /// Whether this tool has side effects
    fn is_side_effecting(&self) -> bool;

    /// Whether this tool always requires user approval (default false)
    fn requires_approval(&self) -> bool { false }

    /// Risk level for approval UX
    fn risk_level(&self) -> RiskLevel {
        if self.is_side_effecting() { RiskLevel::Medium } else { RiskLevel::Low }
    }

    /// Human-readable summary for approval prompts (MUST redact secrets)
    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError>;

    /// Tool-specific timeout (preferred). If None, use ToolCtx.default_timeout.
    fn timeout(&self) -> Option<std::time::Duration> { None }

    /// Execute tool, returning content string on success (framework wraps into ToolResult)
    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}
```

**FR-REG-01a (No side effects during planning):** `approval_summary()` MUST NOT perform I/O or side effects. It MUST be safe to call during planning.

#### 3.1.2 Schema Validation (FR-VAL-01)

**Requirement:** The framework MUST validate tool arguments against `executor.schema()` before calling `execute()`. Schema-invalid arguments SHALL be pre-resolved as `ToolResult::error` (ToolError::BadArgs) and MUST NOT invoke the executor.

**FR-VAL-01a (Schema dialect):** The framework SHALL validate against a single configured JSON Schema dialect (default: Draft 2020-12). If the underlying validator supports only a different draft, the implementation MUST document the supported draft and enforce a restricted schema subset consistently across all tools.

#### 3.1.3 Typed Argument Parsing (FR-REG-02)

**Requirement:** Each executor MUST deserialize arguments from `serde_json::Value` into a strongly-typed struct before use.

```rust
fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
    Box::pin(async move {
        let typed: MyArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::BadArgs { message: e.to_string() })?;
        // ... use typed ...
        Ok("...".to_string())
    })
}
```

**Invariant:** Executors MUST treat deserialization as the single parse boundary; raw `Value` MUST NOT be used as a secondary source of truth after parsing.

#### 3.1.4 Approval Summary Requirements (FR-APR-01)

**Requirement:** Executors that may require confirmation MUST provide a summary via `approval_summary()`.

* Summary MUST be derived from the same typed parsing logic used by `execute()`.
* Summary MUST redact secrets (tokens, credentials, API keys, passwords).
* Summary SHOULD be ≤ 200 characters; if longer, truncate with an ellipsis.

---

### 3.2 Tool Registry

#### 3.2.1 Registry Structure (FR-REG-03)

**Requirement:** The system SHALL maintain a registry mapping tool names to executors.

```rust
pub struct ToolRegistry {
    executors: std::collections::HashMap<String, Box<dyn ToolExecutor>>,
}

impl ToolRegistry {
    /// Register a tool executor; reject duplicates
    pub fn register(&mut self, executor: Box<dyn ToolExecutor>) -> Result<(), ToolError> { /*...*/ }

    /// Lookup executor by name
    pub fn lookup(&self, name: &str) -> Result<&dyn ToolExecutor, ToolError> { /*...*/ }

    /// Generate tool definitions for LLM (sorted by name)
    pub fn definitions(&self) -> Vec<forge_types::ToolDefinition> { /*...*/ }
}
```

**FR-REG-03a (Advertised tool names):** In `ToolsMode::Enabled`, the engine MUST advertise only tools present in the registry (unless explicitly configured otherwise).

---

### 3.3 Tool Context

#### 3.3.1 Per-call Context (FR-REG-04)

**Requirement:** The system SHALL provide per-call execution context to tools.

```rust
pub struct ToolCtx {
    pub sandbox: Sandbox,

    /// Abort handle for THIS CALL only
    pub abort: futures_util::future::AbortHandle,

    /// Channel for streaming output to UI (bounded)
    pub output_tx: tokio::sync::mpsc::Sender<ToolEvent>,

    /// Fallback timeout when executor.timeout() is None
    pub default_timeout: std::time::Duration,

    /// Max output size in bytes for this call (before truncation marker)
    pub max_output_bytes: usize,

    /// Estimated remaining capacity available for tool results (bytes)
    pub available_capacity_bytes: usize,

    /// Current tool call ID
    pub tool_call_id: String,

    /// Whether framework truncation is allowed (binary read_file disables)
    pub allow_truncation: bool,

    /// Resolved working directory (typically first sandbox root)
    pub working_dir: std::path::PathBuf,
}
```

**FR-REG-04a:** `ToolCtx` MUST be constructed fresh per tool call.

**FR-REG-04b:** `available_capacity_bytes` MUST be computed conservatively. Default algorithm:

```
available_tool_tokens =
  max(0, model_context_limit
          - estimated_current_context_tokens
          - response_reserve_tokens
          - safety_margin_tokens)

available_capacity_bytes = available_tool_tokens * 4
```

If estimates are unavailable, the engine MUST fall back to a conservative fixed cap (default: 64 KB).

#### 3.3.2 Shared Batch Context (FR-REG-05)

**Requirement:** The system SHALL define a shared context for batch-level configuration.

```rust
pub struct SharedToolCtx {
    pub sandbox: Sandbox,
    pub output_tx: tokio::sync::mpsc::Sender<ToolEvent>,

    /// Global fallback timeout
    pub default_timeout: std::time::Duration,

    pub max_output_bytes: usize,
    pub initial_capacity_bytes: usize,

    pub env_sanitizer: EnvSanitizer,
    pub journal: ToolJournal,
}
```

---

### 3.4 Approval Workflow

#### 3.4.1 Outcomes (FR-APR-02)

**Requirement:** The planner SHALL classify each call into one of:

* Execute immediately
* Requires user confirmation
* Pre-resolved error (denied or invalid)

```rust
pub enum PlannedDisposition {
    ExecuteNow,
    RequiresConfirmation(ConfirmationRequest),
    PreResolved(forge_types::ToolResult),
}
```

#### 3.4.2 Supporting Types (FR-APR-03)

```rust
pub struct ConfirmationRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
    pub risk_level: RiskLevel,
}

pub enum RiskLevel {
    Low,    // read-only
    Medium, // file edits
    High,   // shell commands or destructive operations
}

pub enum ApprovalDecision {
    ApproveAll,
    ApproveSelected(Vec<String>), // tool_call_ids
    DenyAll,
}
```

#### 3.4.3 Policy (FR-APR-04)

**Requirement:** The system SHALL derive approval policy from configuration.

```rust
pub struct Policy {
    pub enabled: bool,
    pub mode: ApprovalMode,
    pub allowlist: std::collections::HashSet<String>,
    pub denylist: std::collections::HashSet<String>,
    pub prompt_side_effects: bool,
}

pub enum ApprovalMode {
    Auto,   // execute unless denied
    Prompt, // prompt per policy/traits
    Deny,   // deny unless allowlisted
}
```

**Policy rules**

* **FR-APR-04a:** If `enabled = false`, all tool calls MUST be pre-resolved as `ToolResult::error` with message `"Tool execution disabled by policy"`.
* **FR-APR-04b:** `denylist` MUST always deny, regardless of mode.
* **FR-APR-04c:** In `Deny` mode, only tools in `allowlist` may execute (still subject to sandbox validation and tool-level `requires_approval()`).
* **FR-APR-04d:** In `Prompt` mode:

  * Tools in `allowlist` MAY execute without policy prompting (but not bypassing tool-level `requires_approval()`).
  * If `prompt_side_effects = true`, all side-effecting tools MUST require confirmation unless allowlisted and not tool-required.
* **FR-APR-04e:** Tool-level `requires_approval()` MUST always trigger confirmation unless the tool is denied outright.

#### 3.4.4 Precedence (FR-APR-05)

**Requirement:** Evaluation order MUST be:

1. Global disabled (`policy.enabled=false`) → PreResolved error
2. Denylist → PreResolved error (with denial reason)
3. Sandbox/path violation (when applicable) → PreResolved error
4. Mode-based prompting / allowlist logic → prompt or allow
5. Tool-level `requires_approval()` → prompt
6. Otherwise → execute

#### 3.4.5 Planning-time Validation (FR-VAL-02)

**Requirement:** In `ToolsMode::Enabled`, the planner MUST, before prompting:

* Validate tool existence in registry
* Validate argument schema
* Enforce `max_tool_calls_per_batch` and `max_tool_args_bytes`
* Perform sandbox preflight for path-based tools where possible

Failures MUST become `PreResolved(ToolResult::error)` and MUST NOT prompt.

**FR-VAL-02a (Duplicate IDs):** The planner MUST reject duplicate `tool_call_id` within a batch; duplicates MUST be pre-resolved as errors and MUST NOT execute.

#### 3.4.6 Confirmation Request Generation (FR-APR-06)

**Requirement:** For `RequiresConfirmation`, the planner MUST:

* Call `executor.approval_summary(args)` after schema validation
* Truncate `summary` to ≤ 200 characters with ellipsis
* Set `risk_level` from `executor.risk_level()`

If `approval_summary()` errors, the call MUST be pre-resolved as error and MUST NOT prompt.

**Built-in tool guidance**

* `read_file`: `Read <path> [lines X-Y]` → Low
* `apply_patch`: `Apply patch to <n> file(s): <file1>, <file2>...` → Medium
* `run_command`: `Run command: <command>` → High (with explicit warning banner in UI)
* `list_directory`: `List <path> [depth N]` → Low

---

### 3.5 Sandbox

#### 3.5.1 Structure (FR-VAL-03)

```rust
pub struct Sandbox {
    allowed_roots: Vec<std::path::PathBuf>, // canonicalized at construction
    denied_patterns: Vec<Glob>,
    allow_absolute: bool,
    include_default_denies: bool, // NEW (default true)
}
```

#### 3.5.2 Path Validation (FR-VAL-04)

**Requirement:** The sandbox MUST validate paths before any filesystem access for sandboxed tools.

**Algorithm (normative)**

1. Reject absolute paths unless `allow_absolute=true`
2. Reject any `..` component (lexical traversal)
3. Resolve the path relative to a configured base (working directory)
4. Find deepest existing ancestor, canonicalize that ancestor (resolving symlinks)
5. Reconstruct the target under canonical ancestor
6. Ensure the canonical target is contained by an `allowed_root`
7. Apply denied patterns on a normalized path string (forward slashes)

**FR-VAL-04a:** The returned path MUST be canonical/absolute and MUST be used for subsequent file operations.

#### 3.5.3 Default Denies (FR-VAL-05)

**Requirement:** When `include_default_denies=true`, the sandbox MUST deny at least:

* `**/.ssh/**`
* `**/.gnupg/**`
* `**/id_rsa*`
* `**/*.pem`
* `**/*.key`

Additionally, on Unix-like platforms, it SHOULD deny system credential files when they fall within allowed roots (defense-in-depth), but MUST NOT assume `/etc/*` is reachable when `allow_absolute=false`.

#### 3.5.4 Symlink Safety (FR-VAL-06)

**Requirement:** File read/write operations MUST use symlink-safe primitives to prevent TOCTOU escapes after validation.

* Unix: prefer `openat()` patterns with no-follow for final component
* Windows: open with reparse-point protections or re-check final canonical target after open
* Fallback: re-validate immediately before I/O and abort on mismatch

#### 3.5.5 Tool Applicability (FR-VAL-07)

Sandbox validation SHALL apply to file-based tools (`read_file`, `apply_patch`, `list_directory`) and any future tools operating on paths.

`run_command` is NOT subject to filesystem sandboxing and MUST be treated as unsandboxed.

---

### 3.6 Execution Semantics

#### 3.6.1 Sequential Execution (FR-EXE-01)

**Requirement:** The system SHALL execute tools sequentially in original call order.

**Invariant:** Every tool call produces exactly one `ToolResult` (success or error).

#### 3.6.2 Continue-on-failure (FR-EXE-02)

**Requirement:** The system SHALL continue executing remaining tools after a failure unless the user cancels (FR-EXE-05).

#### 3.6.3 Timeout Enforcement (FR-EXE-03)

**Requirement:** The system SHALL enforce a timeout per tool call.

* Use `executor.timeout()` if provided
* Else use `ctx.default_timeout`

Timeout MUST yield `ToolError::Timeout` and an error ToolResult.

#### 3.6.4 Cancellation (FR-EXE-04)

**Requirement:** The system SHALL support cancellation via an abort handle for the active call.

* Tools MUST check `ctx.abort` periodically for long-running work.
* On user cancellation:

  1. Abort the current tool call
  2. Mark current and remaining calls as `ToolResult::error("Cancelled by user")`
  3. Persist results and return to Idle (no auto-resume)

#### 3.6.5 Panic Safety (FR-EXE-04b)

**Requirement:** The framework MUST wrap tool execution in `std::panic::catch_unwind` and convert caught panics to `ToolResult::error("Tool panicked: <message>")`.

* **FR-EXE-04b-1:** Tool executors MUST be `UnwindSafe` (or the framework MUST use `AssertUnwindSafe` with documented limitations).
* **FR-EXE-04b-2:** For async executors, the framework MUST use `FutureExt::catch_unwind` from `futures` or equivalent.
* **FR-EXE-04b-3:** Panic payloads MUST be sanitized before inclusion in error messages (no arbitrary user-controlled content).

#### 3.6.6 Child Process Termination (FR-EXE-05)

**Requirement:** On timeout or abort, child processes MUST be terminated.

* Unix: MUST terminate the entire process group via `killpg(pgid, SIGKILL)` after the child is spawned with `setsid()` or equivalent. Failure to kill the group MUST be logged but MUST NOT block completion.
* Windows: MUST call `TerminateJobObject` if a job object was used, otherwise MUST call `TerminateProcess` on the child handle.

#### 3.6.7 Execution Environment (FR-EXE-06)

**Requirement:** The system SHALL define execution environment for command tools:

| Property          | Specification                                  |
| ----------------- | ---------------------------------------------- |
| Working directory | `ctx.working_dir` (first allowed sandbox root) |
| stdin             | MUST be null/closed (non-interactive)          |
| env               | Inherit then sanitize via `EnvSanitizer`       |
| shell             | Windows: `cmd.exe /C`; Unix: `sh -c`           |
| encoding          | UTF-8 for text outputs; binary uses base64     |

---

#### 3.6.7a Error Passthrough (FR-ERR-JSON-TOOL-01)

**Requirement:** If `ToolError::ExecutionFailed.message` is a valid JSON object string,
the framework MUST pass it verbatim to `ToolResult::error` without prefixing or
wrapping. If parsing fails, the framework MUST prefix with `"{tool} failed: "`.

**FR-ERR-JSON-TOOL-02 (Sanitization order):** When JSON passthrough applies, the
framework MUST NOT insert truncation markers or prefixes. Sanitization MAY still
be applied, but MUST NOT alter valid JSON bytes (no added/removed characters).

---

### 3.6.8 Built-in Tool: `read_file` (FR-TOOL-READ-01)

**Requirement:** `read_file` SHALL read file contents within the sandbox and return UTF-8 text or base64-encoded binary.

**Parameters**

* `path` (string, required)
* `start_line` (u32, optional, 1-indexed, inclusive)
* `end_line` (u32, optional, 1-indexed, inclusive)

**Validation**

* `path` MUST pass sandbox validation before any read.
* Line range rules:

  * `start_line` / `end_line` MUST be ≥ 1 when provided
  * If both provided and `start_line > end_line` → `BadArgs`
  * If `end_line` beyond EOF, return through EOF

**Binary detection**

* Sniff first 8KB (or EOF if smaller)
* If NUL bytes present OR UTF-8 decode fails → binary

**Limits**

* `max_file_read_bytes` (config) default: 200 KB
* `read_limit = min(max_file_read_bytes, ctx.available_capacity_bytes)`
* `output_limit = min(ctx.max_output_bytes, ctx.available_capacity_bytes)`

**Text behavior**

* If no line range and file size > `read_limit`: return an error message instructing use of line ranges.
* If line range:

  * Stream lines and return requested range
  * **FR-TOOL-READ-01a:** Scanning MUST be bounded by `max_read_file_scan_bytes` (default: 2 MB). Exceeding scan limit MUST error with a message to narrow the range.

**Binary behavior**

* Line ranges are invalid for binary → `BadArgs`
* Return header `[binary:base64]` plus `[truncated]` when applicable, then `\n`, then base64 bytes.
* Binary encoding must ensure output ≤ `output_limit`.
* **FR-TOOL-READ-01b:** Framework truncation MUST be disabled (`ctx.allow_truncation=false`) for binary outputs; the tool MUST self-limit.

**Side effects / approval**

* Read-only, `is_side_effecting=false`
* MUST NOT require approval by default

---

### 3.6.9 Built-in Tool: `apply_patch` (FR-TOOL-PATCH-01)

**Requirement:** `apply_patch` SHALL accept an LP1 patch string and apply it within the sandbox.

**Arguments**

* `patch` (string, required; LP1 per `docs/LP1.md`)

**Path validation**

* All referenced file paths MUST be sandbox-validated before applying any changes.
* **FR-TOOL-PATCH-01a (All-or-nothing):** The tool MUST apply patches atomically at the tool-call level:

  * If any file operation fails, the tool MUST NOT leave partial edits applied.
  * Implementation SHOULD stage edits in memory and write using temp-file + atomic rename per file.

**Symlink safety**

* Writes MUST be symlink-safe per FR-VAL-06.

**File creation/deletion policy**

* The tool MUST follow LP1 backend policy for non-existent files (`T`/`B` behavior), but this policy MUST be explicit in implementation documentation and tests.
* Deletion operations (if introduced) MUST be treated as High risk and require approval.

**Stale file protection (FR-TOOL-PATCH-02)**

* **FR-TOOL-PATCH-02a:** `apply_patch` MUST reject patches targeting files that were not previously read in the current conversation. Rejection MUST yield `ToolError::StaleFile { file, reason: "File was not read before patching" }`.
* **FR-TOOL-PATCH-02b:** `apply_patch` MUST reject patches targeting files whose content has changed since the last `read_file` call (SHA-256 mismatch). Rejection MUST yield `ToolError::StaleFile { file, reason: "File content changed since last read" }`.
* **FR-TOOL-PATCH-02c:** The framework MUST track file SHAs from `read_file` results in a per-conversation cache. The cache entry MUST include: file path (canonical), SHA-256 of content at read time, and read timestamp.
* **FR-TOOL-PATCH-02d:** SHA validation MUST occur after sandbox validation but before any file modifications.

**Success output**

* Human-readable summary, one file per line:

  * `modified: <path>`
  * `created: <path>` (if applicable)
  * If no changes: `"No changes applied."`

**Failure mapping**

* LP1 failures MUST map to `ToolError::PatchFailed { file, message }`

**Side effects / approval**

* Mutating, `is_side_effecting=true`
* SHOULD require approval unless allowlisted

---

### 3.6.10 Built-in Tool: `run_command` (FR-TOOL-CMD-01)

**Requirement:** `run_command` SHALL execute a shell command using the platform shell.

**Arguments**

* `command` (string, required, non-empty)

**Streaming**

* Emit `ToolEvent::StdoutChunk` / `ToolEvent::StderrChunk` as output is produced.

**Output semantics**

* Success: return stdout; if stderr non-empty, append `\n\n[stderr]\n<stderr>`
* Non-zero exit: return `ToolError::ExecutionFailed { tool: "run_command", message: "exit code <n>" }`
* Final output is subject to truncation (FR-OUT-03)

**Sandboxing**

* Not sandboxed; treated as unsandboxed

**Policy defaults**

* MUST remain denylisted by default
* MUST require approval (tool-level)

---

### 3.6.11 Built-in Tool: `list_directory` (FR-TOOL-LIST-01)

**Requirement:** `list_directory` SHALL list directory contents within the sandbox and return a JSON object string. Full behavior, schema, and limits are specified in `docs/LIST_DIRECTORY_SRD.md`.

### 3.7 Output Handling

#### 3.7.1 Tool Events (FR-OUT-01)

```rust
pub enum ToolEvent {
    Started { tool_call_id: String, tool_name: String },
    StdoutChunk { tool_call_id: String, chunk: String },
    StderrChunk { tool_call_id: String, chunk: String },
    Completed { tool_call_id: String },
}
```

**Ordering guarantees**

* `Started` MUST precede any chunk events for that call.
* `Completed` MUST follow the final chunk for that call.

**Backpressure**

* Output channel MUST be bounded.
* Implementations MUST NOT silently drop chunks.
* When full, implementations MUST either await capacity or coalesce.

#### 3.7.2 Sanitization (FR-OUT-02)

**Requirement:** All tool output strings (streaming chunks and final `ToolResult.content`) MUST be sanitized to prevent terminal control injection before rendering in the TUI.

Sanitization MUST preserve printable text and remove/neutralize control sequences.

#### 3.7.3 Persistence Policy (FR-OUT-03)

* Streaming chunks are UI-only and MUST NOT be persisted to conversation history.
* Final `ToolResult` objects MUST be persisted to history (FR-RES-01).

#### 3.7.4 Output Truncation (FR-OUT-04)

**Requirement:** The system SHALL truncate outputs exceeding the effective limit:

```
effective_max = min(ctx.max_output_bytes, ctx.available_capacity_bytes)
```

**FR-OUT-04a:** The truncation marker MUST be included within `effective_max`. The final string length MUST NOT exceed `effective_max`.

```rust
fn truncate_output(mut output: String, effective_max: usize) -> String {
    if output.len() <= effective_max { return output; }
    let marker = "\n\n... [output truncated]";
    if effective_max <= marker.len() {
        return marker[..effective_max].to_string();
    }
    let max_body = effective_max - marker.len();

    let mut end = max_body;
    while end > 0 && !output.is_char_boundary(end) { end -= 1; }
    output.truncate(end);
    output.push_str(marker);
    output
}
```

**Scope**

* Applies to all tool results including pre-resolved errors
* EXCEPT binary `read_file` outputs where `allow_truncation=false` and the tool self-limits

---

### 3.8 Resume, Persistence, and Crash Recovery

#### 3.8.1 Persistence Ordering (FR-RES-01)

**Requirement:** When a tool batch begins (assistant emitted tool calls), the engine MUST persist:

1. The assistant message text (if any)
2. The tool use requests (`Message::ToolUse`) for each call, in call order

Then, as tool results are produced, the engine MUST persist each `Message::ToolResult`, ensuring one per tool call ID.

**FR-RES-01a (Ordering key):** The engine MUST preserve the original call order as emitted by the model when persisting tool uses and when supplying results back to the model.

#### 3.8.2 Auto-resume (FR-RES-02)

**Requirement:** In `ToolsMode::Enabled`, after all tool results for a batch are collected and persisted, the engine SHALL resume streaming the LLM continuation automatically using the same model/provider that produced the tool calls.

#### 3.8.3 Max Iterations (FR-RES-03)

**Requirement:** The tool loop MUST enforce `max_tool_iterations_per_user_turn`. When exceeded:

* New tool calls MUST be pre-resolved as errors: `"Max tool iterations reached"`
* The engine MUST resume the model continuation with these errors (unless user cancels)

#### 3.8.4 Tool Journaling (FR-RES-04)

**Requirement:** Tool execution MUST integrate with crash recovery.

At minimum, the system MUST durably record:

* The tool calls in a batch (IDs, names, arguments, ordering)
* The per-call result as it becomes available (success/error)

**FR-RES-04a:** Tool calls emitted during streaming MUST be journaled (not only text deltas). This is REQUIRED to recover from crashes after tool calls were received but before tool batch persistence.

**FR-RES-04b:** Tool results MUST be journaled immediately after completion (before starting the next tool).

**FR-RES-04c:** Recovery MUST NOT auto-retry tool execution; side effects may have occurred.

#### 3.8.5 Recovery UX (FR-RES-05)

If a crash occurs mid-batch and recovery finds partial tool results, the system MUST prompt the user to either:

1. Resume conversation with partial results as-is, OR
2. Discard all tool results from this batch (tool calls remain visible; results marked failed)

---

### 3.9 TUI Integration

#### 3.9.1 Approval Prompt (FR-TUI-01)

When the current batch includes confirmation-required calls, the TUI SHALL display:

* Tool name
* Approval summary
* Risk level (visual indicator)
* Actions:

  * Approve All
  * Deny All
  * Select individual tools

#### 3.9.2 Execution Progress (FR-TUI-02)

The TUI SHALL display per-call state:

* Pending
* Executing (with spinner)
* Completed (✓ or ✗)
* Denied/Pre-resolved (⊘ with reason)

#### 3.9.3 Streaming Output Display (FR-TUI-03)

For tools emitting chunks:

* Display output in a scrollable region
* Auto-scroll unless user scrolled up
* Limit rendered lines to last 50 for performance (UI-only)

#### 3.9.4 Parse-only Mode UI (FR-TUI-04)

In `ToolsMode::ParseOnly`, the UI MUST:

* List pending tool calls (name + ID)
* Provide guidance on manual submission (e.g., `/tool <id> <result>`)

---

## 4. Error Handling

### 4.1 Error Types (NFR-ERR-01)

```rust
pub enum ToolError {
    BadArgs { message: String },
    Timeout { tool: String, elapsed: std::time::Duration },
    SandboxViolation(DenialReason),
    ExecutionFailed { tool: String, message: String },
    Cancelled,
    UnknownTool { name: String },
    DuplicateTool { name: String },
    DuplicateToolCallId { id: String },
    PatchFailed { file: std::path::PathBuf, message: String },
    StaleFile { file: std::path::PathBuf, reason: String },
}
```

### 4.2 Denial Reasons (NFR-ERR-02)

```rust
pub enum DenialReason {
    Denylisted { tool: String },
    Disabled,
    PathOutsideSandbox { attempted: std::path::PathBuf, resolved: std::path::PathBuf },
    DeniedPatternMatched { attempted: std::path::PathBuf, pattern: String },
    LimitsExceeded { message: String },
}
```

---

## 5. Non-Functional Requirements

### 5.1 Performance (NFR-PERF)

| Requirement | Specification                                             |
| ----------- | --------------------------------------------------------- |
| NFR-PERF-01 | Tool lookup O(1) via HashMap                              |
| NFR-PERF-02 | Sandbox validation < 1ms for existing paths (typical)     |
| NFR-PERF-03 | Tool schemas generated at startup and cached              |
| NFR-PERF-04 | Streaming chunk coalescing avoids unbounded memory growth |

### 5.2 Security (NFR-SEC)

| Requirement | Specification                                                      |
| ----------- | ------------------------------------------------------------------ |
| NFR-SEC-01  | Path traversal prevention via sandbox                              |
| NFR-SEC-02  | Symlink escape prevention via symlink-safe primitives              |
| NFR-SEC-03  | Sensitive file protection via denied patterns                      |
| NFR-SEC-04  | Approval required for mutating operations per policy               |
| NFR-SEC-05  | Tool output sanitized before terminal rendering                    |
| NFR-SEC-06  | `run_command` denylisted by default; explicit user opt-in required |

### 5.3 Reliability (NFR-REL)

| Requirement | Specification                                                     |
| ----------- | ----------------------------------------------------------------- |
| NFR-REL-01  | Exactly one ToolResult per ToolCall                               |
| NFR-REL-02  | Timeout prevents indefinite blocking                              |
| NFR-REL-03  | Cancellation supported                                            |
| NFR-REL-04  | Crash recovery preserves tool batch progress without re-executing |

### 5.4 Maintainability (NFR-MAIN)

| Requirement | Specification                                             |
| ----------- | --------------------------------------------------------- |
| NFR-MAIN-01 | Schema derived from Rust types via schemars (recommended) |
| NFR-MAIN-02 | Single source of truth for args parsing                   |
| NFR-MAIN-03 | Tool registry definitions deterministic (sorted by name)  |

---

## 6. Configuration

### 6.1 Tools Mode (CFG-TOOLS-01)

```toml
[tools]
mode = "parse_only"      # disabled | parse_only | enabled
allow_parallel = false
max_tool_calls_per_batch = 8
max_tool_iterations_per_user_turn = 4
max_tool_args_bytes = 262144
```

### 6.2 Tool Definitions (CFG-TOOLS-02)

Config-defined tool definitions are supported for `parse_only` (manual tools):

```toml
[[tools.definitions]]
name = "read_file"
description = "Read file contents"
parameters = { "type"="object", "properties"={ "path"={ "type"="string" } }, "required"=["path"] }
```

**CFG-TOOLS-02a:** In `enabled` mode, the engine SHOULD advertise registry-backed tool definitions, not config-defined ones, unless explicitly configured.

### 6.3 Sandbox (CFG-SBX-01)

```toml
[tools.sandbox]
allowed_roots = ["."]
denied_patterns = ["**/.ssh/**", "**/*.pem"]
allow_absolute = false
include_default_denies = true
```

### 6.4 Timeouts (CFG-TMO-01)

```toml
[tools.timeouts]
default_seconds = 30
file_operations_seconds = 30
shell_commands_seconds = 300
```

**CFG-TMO-01a:** Built-in executors SHOULD set `timeout()` from these values.

### 6.5 Output (CFG-OUT-01)

```toml
[tools.output]
max_bytes = 102400
```

### 6.6 Environment Sanitization (CFG-ENV-01)

```toml
[tools.environment]
denylist = ["*_KEY", "*_TOKEN", "*_SECRET", "*_PASSWORD", "AWS_*", "ANTHROPIC_*", "OPENAI_*"]
```

* Matching MUST be case-insensitive on Windows.
* The configured list is additive to defaults.

### 6.7 Approval Policy (CFG-APR-01)

```toml
[tools.approval]
enabled = true
mode = "prompt"           # auto | prompt | deny
allowlist = ["read_file"]
denylist = ["run_command"]
prompt_side_effects = true
```

### 6.8 read_file Limits (CFG-READ-01)

```toml
[tools.read_file]
max_file_read_bytes = 204800
max_scan_bytes = 2097152
```

### 6.9 apply_patch Limits (CFG-PATCH-01)

```toml
[tools.apply_patch]
max_patch_bytes = 524288
```

---

## 7. Verification Requirements

### 7.1 Unit Tests

| Test ID    | Category    | Description                                              |
| ---------- | ----------- | -------------------------------------------------------- |
| T-MODE-01  | Modes       | Disabled mode rejects tool calls                         |
| T-MODE-02  | Modes       | Parse-only mode surfaces pending calls without execution |
| T-LIM-01   | Limits      | Exceed max_tool_calls_per_batch pre-resolves extras      |
| T-APR-01   | Approval    | Mixed batch prompts before any execution                 |
| T-APR-02   | Approval    | ApproveSelected executes only selected                   |
| T-VAL-01   | Validation  | Unknown tool in enabled mode pre-resolved (no prompt)    |
| T-VAL-02   | Validation  | Schema-invalid args pre-resolved (no execution)          |
| T-SBX-01   | Sandbox     | Reject `..` traversal                                    |
| T-SBX-02   | Sandbox     | Reject absolute paths when disallowed                    |
| T-SBX-03   | Sandbox     | Denied patterns matched on canonical path                |
| T-READ-01  | read_file   | Line range behavior + BadArgs for invalid ranges         |
| T-READ-02  | read_file   | Scan limit enforced for large files                      |
| T-PATCH-01 | apply_patch | Atomicity: failure yields no partial writes              |
| T-OUT-01   | Output      | Truncation marker included within effective limit        |
| T-OUT-02   | Output      | Sanitization removes terminal controls                   |
| T-TMO-01   | Timeout     | Timeout yields ToolError::Timeout and continues          |
| T-CAN-01   | Cancel      | Cancel mid-execution marks remaining as cancelled        |

### 7.2 Integration Tests

| Test ID   | Description                                                                      |
| --------- | -------------------------------------------------------------------------------- |
| IT-E2E-01 | Full loop: model requests tool → execute → persist → auto-resume                 |
| IT-JRN-01 | Crash after tool calls received but before persistence → recovered batch present |
| IT-JRN-02 | Crash mid-execution → recovery prompts user; no auto-retry                       |
| IT-CMD-01 | run_command streaming chunks shown; final result persisted (if enabled)          |

---

## 8. Appendix

### 8.1 Multi-tool Ordering Invariant (Provider-facing)

**Requirement:** When a single assistant response contains multiple tool calls, the system MUST preserve the call order and ensure provider adapters can reconstruct valid message ordering rules (e.g., tool results follow tool uses without interleaving unrelated assistant turns). This may be implemented by grouping consecutive `ToolUse`/`ToolResult` messages at adapter serialization time.

### 8.2 State Machine Sketch

```
Idle
  └─ start_streaming → Streaming
         └─ tool_calls_detected →
              ToolLoop:
                - Plan/Validate
                - (Optional) NeedsApproval
                - Execute sequentially
                - Persist results
                - Auto-resume → Streaming
```

**Denied-only batch:** If all calls are pre-resolved (policy/sandbox/invalid), results are persisted and auto-resume occurs (enabled mode) without executing any tool.

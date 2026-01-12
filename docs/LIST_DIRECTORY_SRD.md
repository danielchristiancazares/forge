# list_directory Tool

## Software Requirements Document

**Version:** 1.1
**Date:** 2026-01-12
**Status:** Draft
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log

### 0.1 Initial version

* Initial SRD for the built-in `list_directory` tool.

### 0.2 Clarifications and alignment

* Clarified traversal order, truncation behavior, defaults, and output canonicalization.
* Added error_code semantics and read_dir failure handling.
* Aligned references and configuration defaults.

---

## 1. Introduction

### 1.1 Purpose

This document specifies requirements for the `list_directory` tool, a built-in tool for the Forge Tool Executor Framework. It defines expected behavior, inputs, outputs, limits, and error handling.

### 1.2 Scope

**In Scope**

* Tool definition and JSON schema
* Sandbox and path validation requirements
* Directory traversal semantics
* Output format, limits, and sanitization requirements
* Configuration keys and defaults
* Test and validation requirements

**Out of Scope**

* UI rendering changes
* Provider-specific tool-call adapter behavior
* Network or remote filesystem listing

### 1.3 Definitions

| Term            | Definition                                                |
| --------------- | --------------------------------------------------------- |
| Tool            | A callable function the LLM can invoke                    |
| Tool Call       | A request to execute a tool with JSON arguments           |
| Tool Result     | String output returned from the tool                      |
| Entry           | A file system item inside a directory                     |
| Depth           | Number of path components from the listed root directory  |
| Sandbox         | Filesystem boundary applied to tool paths                 |

### 1.4 References

| Document                      | Description                                  |
| ----------------------------- | -------------------------------------------- |
| `docs/TOOL_EXECUTOR_SRD.md`   | Tool execution framework requirements        |
| `engine/README.md`            | Engine state machine constraints             |
| `docs/DESIGN.md`             | Type-driven design patterns and invariants   |
| RFC 2119 / RFC 8174           | Requirement level keywords                   |

### 1.5 Requirement Level Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in RFC 2119 and RFC 8174.

---

## 2. Overall Description

### 2.1 Product Perspective

`list_directory` is a built-in tool that integrates with the Tool Executor Framework. It lists directory entries within the configured sandbox and returns a structured, deterministic output suitable for LLM consumption.

### 2.2 Product Functions

| Function   | Description                                      |
| ---------- | ------------------------------------------------ |
| FR-LD-DEF  | Tool definition and schema                       |
| FR-LD-VAL  | Argument validation and sandbox/path validation  |
| FR-LD-LST  | Directory traversal and filtering                |
| FR-LD-OUT  | Output format and size control                   |
| FR-LD-ERR  | Error handling and partial failure behavior      |

### 2.3 User Characteristics

| User        | Interaction                                                         |
| ----------- | ------------------------------------------------------------------- |
| LLM         | Requests listings to understand repo structure                       |
| End User    | Approves or denies tool calls if policy requires                     |
| Developer   | Implements tool and tests against this SRD                           |

### 2.4 Constraints

| Constraint | Rationale                                                |
| ---------- | -------------------------------------------------------- |
| C-01       | MUST be cross-platform (Windows/macOS/Linux)             |
| C-02       | MUST use sandbox path validation before filesystem access|
| C-03       | Output MUST be deterministic for same inputs             |
| C-04       | Output MUST be sanitized before terminal rendering       |
| C-05       | No network access                                        |

### 2.5 Assumptions and Dependencies

| ID   | Assumption/Dependency                                                 |
| ---- | --------------------------------------------------------------------- |
| A-01 | Tool executor framework and sandbox exist and are stable              |
| A-02 | JSON schema validation is available                                   |
| A-03 | Output truncation and sanitization are enforced by framework defaults |
| A-04 | Directory listings are performed using OS-provided filesystem APIs    |

---

## 3. Functional Requirements

### 3.1 Tool Definition (FR-LD-DEF)

* **FR-LD-DEF-01:** The tool name MUST be `list_directory`.
* **FR-LD-DEF-02:** The tool description MUST be "List directory entries".
* **FR-LD-DEF-03:** The tool MUST be read-only and report `is_side_effecting=false`.
* **FR-LD-DEF-04:** The tool MUST NOT require approval by default (`requires_approval=false`) and MUST report risk level Low.

### 3.2 Arguments and Schema (FR-LD-VAL)

**Schema (normative)**

```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string" },
    "recursive": { "type": "boolean", "default": false },
    "max_depth": { "type": "integer", "minimum": 1 },
    "max_entries": { "type": "integer", "minimum": 1 },
    "include_hidden": { "type": "boolean", "default": false },
    "include_files": { "type": "boolean", "default": true },
    "include_dirs": { "type": "boolean", "default": true },
    "include_symlinks": { "type": "boolean", "default": true },
    "include_other": { "type": "boolean", "default": false }
  },
  "required": ["path"]
}
```

**Validation requirements**

* **FR-LD-VAL-01:** `path` MUST be non-empty after trimming whitespace.
* **FR-LD-VAL-02:** If `recursive` is false, `max_depth` MUST be absent or equal to 1; otherwise return `ToolError::BadArgs`.
* **FR-LD-VAL-03:** If `recursive` is true and `max_depth` is absent, the tool MUST use the configured default depth (CFG-LD-01).
* **FR-LD-VAL-04:** `max_entries` MUST be <= configured limit (CFG-LD-02); otherwise return `ToolError::BadArgs`.
* **FR-LD-VAL-05:** At least one of `include_files`, `include_dirs`, or `include_symlinks` MUST be true; otherwise return `ToolError::BadArgs`.
* **FR-LD-VAL-06:** `include_other` only applies to non-regular, non-directory, non-symlink entries and defaults to false.
* **FR-LD-VAL-06a:** Omitted arguments MUST resolve using defaults in `[tools.list_directory]` (CFG-LD-01). If no config is present, use built-in defaults (CFG-LD-05). Precedence: explicit args > config defaults > built-in defaults.
* **FR-LD-VAL-06b:** If `recursive` is true and the requested `max_depth` exceeds the configured cap (CFG-LD-03), return `ToolError::BadArgs`.

### 3.3 Sandbox and Path Handling (FR-LD-VAL)

* **FR-LD-VAL-07:** The tool MUST validate `path` using the sandbox before any filesystem access.
* **FR-LD-VAL-08:** The resolved path MUST exist and be a directory; otherwise return `ToolError::ExecutionFailed` with a clear message.
* **FR-LD-VAL-09:** The tool MUST NOT follow symlinks when traversing directories (even when recursive).
* **FR-LD-VAL-10:** The `path` field in output MUST be a normalized form of the requested path: trim whitespace; convert all separators to `/`; remove redundant `./` segments; collapse repeated `/` to a single `/`; remove trailing `/` unless the path is exactly `/` or a drive root (`X:/`). Case MUST be preserved.

### 3.4 Traversal Semantics (FR-LD-LST)

* **FR-LD-LST-01:** If `recursive` is false, only immediate children (depth=1) MUST be returned.
* **FR-LD-LST-02:** If `recursive` is true, entries MUST be returned for all descendants with depth <= effective max depth, subject to `max_entries` and output-size limits.
* **FR-LD-LST-03:** The tool MUST ignore `.` and `..` entries if present.
* **FR-LD-LST-04:** For entries where metadata retrieval fails, the tool MUST still include an entry with `type="unknown"`, `error_code`, and `error`, and MUST NOT abort the entire listing.
* **FR-LD-LST-05:** Traversal order MUST be deterministic: depth-first pre-order. Within each directory, entries MUST be enumerated in ascending lexical order by entry name using the lossy UTF-8 string for that name (see NFR-LD-REL-02). Directories are descended in that same order.
* **FR-LD-LST-06:** Filtering is applied before counting toward `max_entries`. When `include_hidden=false`, hidden entries (name starts with `.`) MUST be omitted and hidden directories MUST NOT be traversed.
* **FR-LD-LST-06a:** Hidden determination is based solely on a leading `.` in the entry name; platform-specific hidden attributes MUST be ignored.
* **FR-LD-LST-07:** When `max_entries` is reached during traversal, the tool MUST stop traversal, set `truncated=true`, and set `truncated_reason="max_entries"` (unless later overridden by output-size truncation per FR-LD-OUT-07).
* **FR-LD-LST-08:** If a directory cannot be read during traversal (e.g., permission denied, removed), the tool MUST include an entry with `type="unknown"` and `error_code="read_dir_failed"`, MUST NOT recurse into it, and MUST continue.

### 3.5 Output Format (FR-LD-OUT)

**Output MUST be a single JSON object string** with the following fields:

| Field                | Type                | Description                                          |
| -------------------- | ------------------- | ---------------------------------------------------- |
| `path`               | string              | Normalized requested path (forward slashes; see FR-LD-VAL-10) |
| `entries`            | array               | List of entry objects                                |
| `returned`           | integer             | Number of entries in `entries`                       |
| `max_entries`        | integer             | Effective max entries                                |
| `truncated`          | boolean             | True if results were truncated                       |
| `truncated_reason`   | string or null      | `max_entries` or `max_output_bytes`                  |

**Entry object fields**

| Field              | Type           | Description                                               |
| ------------------ | -------------- | --------------------------------------------------------- |
| `name`             | string         | File name (last path component)                           |
| `path`             | string         | Relative path from the requested root (forward slashes)   |
| `depth`            | integer        | Depth from the requested root (root children are 1)       |
| `type`             | string         | `file`, `dir`, `symlink`, `other`, or `unknown`           |
| `size_bytes`       | integer or null| File size for regular files, null otherwise               |
| `modified_epoch_ms`| integer or null| Last-modified time in epoch milliseconds, null if unknown |
| `is_hidden`        | boolean        | True if name starts with `.`                              |
| `error_code`       | string or null | Per-entry error code (only for type `unknown`)            |
| `error`            | string or null | Per-entry error message (only for type `unknown`)         |

**Entry field rules**

* `type` MUST be determined using no-follow metadata (lstat or equivalent). Symlinks MUST be reported as `type="symlink"` even if they target directories. Symlink targets MUST NOT be traversed (FR-LD-VAL-09).
* `size_bytes` MUST be `null` for non-regular files (dirs, symlinks, other, unknown).
* `modified_epoch_ms` MUST be derived from the entry's own metadata (no-follow). If unavailable or out of range, use `null`.
* `error_code` MUST be one of: `metadata_unavailable`, `permission_denied`, `read_dir_failed`, `io_error`, `unknown`. `error` SHOULD be a short, non-localized message.

**Ordering**

* **FR-LD-OUT-01:** After traversal (and any `max_entries` truncation), entries MUST be sorted by `path` using ascending lexical order on the UTF-8 string produced after lossy conversion. Sorting is for output ordering only.
* **FR-LD-OUT-02:** The output MUST be deterministic for the same filesystem state and inputs, including error_code mapping and lossy conversion.
* **FR-LD-OUT-02a:** JSON serialization MUST be canonical: object keys in the exact order shown in §3.5, no insignificant whitespace, and standard JSON escaping.
* **FR-LD-OUT-02b:** `returned` MUST equal `entries.length`. If `truncated=false`, `truncated_reason` MUST be `null`; otherwise it MUST be `max_entries` or `max_output_bytes`.

### 3.6 Output Size Control (FR-LD-OUT)

* **FR-LD-OUT-03:** The tool MUST ensure the final JSON output length is <= `effective_max = min(ctx.max_output_bytes, ctx.available_capacity_bytes)`.
* **FR-LD-OUT-04:** The tool MUST set `ctx.allow_truncation=false` to prevent framework truncation that would corrupt JSON.
* **FR-LD-OUT-05:** If output would exceed `effective_max`, the tool MUST reduce entries and set `truncated=true` with `truncated_reason="max_output_bytes"`.
* **FR-LD-OUT-06:** Output size accounting MUST use UTF-8 byte length of the final JSON string.
* **FR-LD-OUT-07:** Output-size truncation MUST remove entries from the end of the sorted list until the JSON output fits within `effective_max`. When this occurs, `truncated_reason` MUST be `"max_output_bytes"` (overriding `"max_entries"` if previously set).
* **FR-LD-OUT-08:** If the minimal valid JSON (with zero entries) exceeds `effective_max`, the tool MUST return `ToolError::ExecutionFailed` with a clear message (e.g., "output budget too small") instead of returning invalid JSON.

### 3.7 Error Handling (FR-LD-ERR)

* **FR-LD-ERR-01:** Invalid arguments MUST return `ToolError::BadArgs` with a clear, actionable message.
* **FR-LD-ERR-02:** Sandbox violations MUST return `ToolError::SandboxViolation`.
* **FR-LD-ERR-03:** If the target path does not exist or is not a directory, return `ToolError::ExecutionFailed`.
* **FR-LD-ERR-04:** Per-entry metadata errors MUST NOT fail the overall tool call (see FR-LD-LST-04).

### 3.8 Policy and Approval (FR-LD-DEF)

* **FR-LD-DEF-05:** The tool MUST respect global tool policy (allowlist/denylist/prompt) as defined in `docs/TOOL_EXECUTOR_SRD.md`.

---

## 4. Non-Functional Requirements

### 4.1 Performance (NFR-LD-PERF)

* **NFR-LD-PERF-01:** Directory traversal MUST be bounded by `max_entries` and `max_depth`.
* **NFR-LD-PERF-02:** Sorting MUST be O(n log n) or better for n returned entries.

### 4.2 Security (NFR-LD-SEC)

* **NFR-LD-SEC-01:** Paths MUST be sandbox-validated before access.
* **NFR-LD-SEC-02:** Output MUST be sanitized before terminal rendering (framework requirement).
* **NFR-LD-SEC-03:** The tool MUST NOT follow symlinks during recursion.

### 4.3 Reliability (NFR-LD-REL)

* **NFR-LD-REL-01:** Output MUST be valid JSON even under size constraints.
* **NFR-LD-REL-02:** Non-UTF8 filenames MUST be converted to valid UTF-8 via a lossy conversion using the Unicode replacement character. The resulting string MUST be used for ordering and output.

### 4.4 Portability (NFR-LD-PORT)

* **NFR-LD-PORT-01:** Path separators in output MUST be normalized to `/` for cross-platform consistency.

---

## 5. Configuration

**CFG-LD-01:** The system SHALL support tool-specific configuration under `[tools.list_directory]`.

Example:

```toml
[tools.list_directory]
max_entries = 200
max_depth = 4
include_hidden_default = false
include_symlinks_default = true
include_files_default = true
include_dirs_default = true
include_other_default = false
```

**CFG-LD-02:** `max_entries` is a hard cap; requested `max_entries` above this value MUST return `ToolError::BadArgs`.

**CFG-LD-03:** `max_depth` is the default and the hard cap for recursive listings. Requested depths above this value MUST return `ToolError::BadArgs`.

**CFG-LD-04:** `*_default` values are used when corresponding arguments are omitted.

**CFG-LD-05:** If `[tools.list_directory]` is absent, the built-in defaults are: `max_entries=200`, `max_depth=4`, `include_hidden_default=false`, `include_symlinks_default=true`, `include_files_default=true`, `include_dirs_default=true`, `include_other_default=false`.

---

## 6. Test and Validation Requirements

| ID         | Scenario                                                     | Expected Result                                     |
| ---------- | ------------------------------------------------------------ | --------------------------------------------------- |
| T-LD-01    | List directory with default args                             | Entries returned, sorted, depth=1                   |
| T-LD-02    | Path is file                                                  | ExecutionFailed with "path is not a directory"      |
| T-LD-03    | Path outside sandbox                                         | SandboxViolation                                    |
| T-LD-04    | Recursive with max_depth                                     | Entries limited to depth <= max_depth               |
| T-LD-05    | max_entries exceeded by directory size                       | Truncated=true, returned=max_entries, reason=max_entries |
| T-LD-06    | Output size would exceed effective_max                       | Fewer entries returned, truncated_reason=max_output_bytes |
| T-LD-07    | Hidden files present, include_hidden=false                   | Hidden entries omitted                              |
| T-LD-08    | Symlink entries present, recursion enabled                   | Symlinks listed (if enabled) but not traversed      |
| T-LD-09    | Non-UTF8 filename                                            | Output remains valid UTF-8 with lossy conversion    |
| T-LD-10    | Per-entry metadata error                                     | Entry included with type=unknown, error_code, and error |
| T-LD-11    | Hidden directory with include_hidden=false + recursive       | Hidden dir omitted and not traversed                |
| T-LD-12    | Directory read failure during recursion                      | Entry with type=unknown and error_code=read_dir_failed |
| T-LD-13    | Output budget too small for minimal JSON                      | ExecutionFailed with clear message                  |
| T-LD-14    | max_entries hit, then output size truncation                 | reason=max_output_bytes (override)                  |

---

## 7. Open Questions

* Should the tool expose optional inode or permission data, or keep output minimal?
* Should output include a stable unique identifier per entry (not required for MVP)?

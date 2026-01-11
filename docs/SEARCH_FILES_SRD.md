# search_files Tool

## Software Requirements Document

**Version:** 1.0
**Date:** 2026-01-08
**Status:** Draft
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log

### 0.1 Initial version

- Initial SRD for the built-in `search_files` tool.

---

## 1. Introduction

### 1.1 Purpose

This document specifies requirements for the `search_files` tool, a built-in tool for the Forge Tool Executor Framework. It defines behavior, inputs, outputs, limits, and error handling for searching file contents in a sandboxed directory tree.

### 1.2 Scope

**In Scope**

- Tool definition and JSON schema
- Argument validation and sandbox/path validation
- Search semantics for exact, regex, and fuzzy modes
- Output format and size control
- Configuration keys and defaults
- Test and validation requirements

**Out of Scope**

- UI rendering changes
- Provider-specific tool-call adapter behavior
- Network or remote filesystem search

### 1.3 Definitions

| Term            | Definition                                                |
| --------------- | --------------------------------------------------------- |
| Tool            | A callable function the LLM can invoke                    |
| Tool Call       | A request to execute a tool with JSON arguments           |
| Tool Result     | String output returned from the tool                      |
| Entry           | A file match result with location and snippet             |
| Sandbox         | Filesystem boundary applied to tool paths                 |
| Fuzzy match     | Approximate match within a line using a defined threshold |

### 1.4 References

| Document                    | Description                                |
| --------------------------- | ------------------------------------------ |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework requirements      |
| `docs/DESIGN.md`           | Type-driven design patterns and invariants |
| `docs/SEARCH_FILES_INDEXING_SRD.md` | Indexing, change tracking, and fuzzy fallback |
| RFC 2119 / RFC 8174         | Requirement level keywords                 |

### 1.5 Requirement Level Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in RFC 2119 and RFC 8174.

---

## 2. Overall Description

### 2.1 Product Perspective

`search_files` integrates with the Tool Executor Framework and searches text files within the configured sandbox. It returns deterministic, structured results suitable for agentic workflows.

### 2.2 Product Functions

| Function   | Description                                      |
| ---------- | ------------------------------------------------ |
| FR-SF-DEF  | Tool definition and schema                       |
| FR-SF-VAL  | Argument validation and sandbox/path validation  |
| FR-SF-SRC  | Search semantics and traversal                   |
| FR-SF-OUT  | Output format and size control                   |
| FR-SF-ERR  | Error handling and partial failure behavior      |

### 2.3 Constraints

| Constraint | Rationale                                                |
| ---------- | -------------------------------------------------------- |
| C-01       | MUST be cross-platform (Windows/macOS/Linux)             |
| C-02       | MUST use sandbox path validation before file access      |
| C-03       | Output MUST be deterministic for same inputs             |
| C-04       | Output MUST be sanitized before terminal rendering       |
| C-05       | No network access                                        |

---

## 3. Functional Requirements

### 3.1 Tool Definition (FR-SF-DEF)

- **FR-SF-DEF-01:** The tool name MUST be `search_files`.
- **FR-SF-DEF-02:** The tool description MUST be "Search file contents".
- **FR-SF-DEF-03:** The tool MUST be read-only and report `is_side_effecting=false`.
- **FR-SF-DEF-04:** The tool MUST NOT require approval by default (`requires_approval=false`) and MUST report risk level Low.

### 3.2 Arguments and Schema (FR-SF-VAL)

**Schema (normative)**

```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string" },
    "query": { "type": "string" },
    "mode": { "type": "string", "enum": ["exact", "regex", "fuzzy"], "default": "exact" },
    "case": { "type": "string", "enum": ["sensitive", "insensitive", "smart"], "default": "smart" },
    "recursive": { "type": "boolean", "default": true },
    "max_depth": { "type": "integer", "minimum": 1 },
    "max_results": { "type": "integer", "minimum": 1 },
    "max_matches_per_file": { "type": "integer", "minimum": 1 },
    "max_file_size_bytes": { "type": "integer", "minimum": 1 },
    "max_files": { "type": "integer", "minimum": 1 },
    "context_lines": { "type": "integer", "minimum": 0, "default": 0 },
    "include_hidden": { "type": "boolean", "default": false },
    "follow_symlinks": { "type": "boolean", "default": false },
    "respect_gitignore": { "type": "boolean", "default": true },
    "include_globs": { "type": "array", "items": { "type": "string" } },
    "exclude_globs": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["path", "query"]
}
```

**Validation requirements**

- **FR-SF-VAL-01:** `path` MUST be non-empty after trimming whitespace.
- **FR-SF-VAL-02:** `query` MUST be non-empty after trimming whitespace.
- **FR-SF-VAL-03:** If `recursive` is false, `max_depth` MUST be absent or equal to 1; otherwise return `ToolError::BadArgs`.
- **FR-SF-VAL-04:** `max_results`, `max_matches_per_file`, `max_file_size_bytes`, and `max_files` MUST be <= configured hard caps; otherwise return `ToolError::BadArgs`.
- **FR-SF-VAL-05:** If `mode` is `fuzzy` and the configured backend does not support fuzzy matching, return `ToolError::BadArgs`.

### 3.3 Sandbox and Path Handling (FR-SF-VAL)

- **FR-SF-VAL-06:** The tool MUST validate `path` using the sandbox before any filesystem access.
- **FR-SF-VAL-07:** The resolved path MUST exist and be a directory; otherwise return `ToolError::ExecutionFailed` with a clear message.
- **FR-SF-VAL-08:** The tool MUST NOT follow symlinks when traversing directories, even if `follow_symlinks=true`. If `follow_symlinks=true`, symlinks MAY be searched when they are direct files, but symlinked directories MUST NOT be traversed.

### 3.4 Search Semantics (FR-SF-SRC)

- **FR-SF-SRC-01:** Exact mode MUST treat `query` as a literal substring.
- **FR-SF-SRC-02:** Regex mode MUST interpret `query` using the configured regex engine. Invalid regex MUST return `ToolError::BadArgs`.
- **FR-SF-SRC-03:** Fuzzy mode MUST perform approximate matching within a single line, returning matches that meet the configured fuzzy threshold (CFG-SF-04).
- **FR-SF-SRC-04:** Binary files MUST be skipped by default. A file is binary if it contains NUL bytes or invalid UTF-8 sequences in the scanned region.
- **FR-SF-SRC-05:** The tool MUST respect `include_globs` and `exclude_globs` during traversal.
- **FR-SF-SRC-06:** If `respect_gitignore=true`, ignore rules from `.gitignore` and `.git/info/exclude` MUST be applied.
- **FR-SF-SRC-07:** Results MUST include only matches within `max_file_size_bytes` limits; larger files are skipped with a recorded error entry.
- **FR-SF-SRC-08:** Traversal MUST stop when `max_results` is reached; the output MUST indicate truncation.

### 3.5 Output Format (FR-SF-OUT)

**Output MUST be a single JSON object string** with the following fields:

| Field              | Type           | Description                                               |
| ------------------ | -------------- | --------------------------------------------------------- |
| `path`             | string         | Normalized requested path (forward slashes)               |
| `query`            | string         | Effective query                                            |
| `mode`             | string         | `exact`, `regex`, or `fuzzy`                              |
| `case`             | string         | `sensitive`, `insensitive`, or `smart`                    |
| `matches`          | array          | List of match objects                                     |
| `returned`         | integer        | Number of matches in `matches`                            |
| `max_results`      | integer        | Effective max results                                     |
| `truncated`        | boolean        | True if results were truncated                            |
| `truncated_reason` | string or null | `max_results` or `max_output_bytes` or `max_files`        |
| `stats`            | object         | Aggregate stats (see below)                               |
| `errors`           | array          | Per-file errors (see below)                               |

**Match object fields**

| Field              | Type            | Description                                              |
| ------------------ | --------------- | -------------------------------------------------------- |
| `path`             | string          | Relative path from the requested root                    |
| `line`             | integer         | 1-based line number                                      |
| `column`           | integer         | 1-based column number                                    |
| `match_text`       | string          | Matched substring (exact or regex)                       |
| `line_text`        | string          | Full line text                                           |
| `before`           | array           | Up to `context_lines` lines before                       |
| `after`            | array           | Up to `context_lines` lines after                        |
| `score`            | number or null  | Fuzzy score; null for non-fuzzy modes                    |

**Stats object fields**

| Field              | Type            | Description                                              |
| ------------------ | --------------- | -------------------------------------------------------- |
| `files_scanned`    | integer         | Number of files scanned                                  |
| `files_matched`    | integer         | Number of files with >=1 match                           |
| `matches_total`    | integer         | Total matches found before truncation                    |
| `elapsed_ms`       | integer         | Wall-clock elapsed time                                  |

**Errors array entries**

| Field              | Type            | Description                                              |
| ------------------ | --------------- | -------------------------------------------------------- |
| `path`             | string          | Relative path for the file                               |
| `error`            | string          | Error message                                            |

**Ordering**

- **FR-SF-OUT-01:** Matches MUST be ordered by `path`, then `line`, then `column` (ascending).
- **FR-SF-OUT-02:** Output MUST be deterministic for the same filesystem state and inputs.

### 3.6 Output Size Control (FR-SF-OUT)

- **FR-SF-OUT-03:** The tool MUST ensure the final JSON output length is <= `effective_max = min(ctx.max_output_bytes, ctx.available_capacity_bytes)`.
- **FR-SF-OUT-04:** The tool MUST set `ctx.allow_truncation=false` to prevent framework truncation that would corrupt JSON.
- **FR-SF-OUT-05:** If output would exceed `effective_max`, the tool MUST reduce `matches` and set `truncated=true` with `truncated_reason="max_output_bytes"`.

### 3.7 Error Handling (FR-SF-ERR)

- **FR-SF-ERR-01:** Invalid arguments MUST return `ToolError::BadArgs` with a clear, actionable message.
- **FR-SF-ERR-02:** Sandbox violations MUST return `ToolError::SandboxViolation`.
- **FR-SF-ERR-03:** If the target path does not exist or is not a directory, return `ToolError::ExecutionFailed`.
- **FR-SF-ERR-04:** Per-file I/O and metadata errors MUST NOT fail the overall tool call; they MUST be recorded in `errors`.

---

## 4. Non-Functional Requirements

### 4.1 Performance (NFR-SF-PERF)

- **NFR-SF-PERF-01:** Traversal MUST be bounded by `max_results`, `max_matches_per_file`, `max_files`, and `max_depth`.
- **NFR-SF-PERF-02:** Sorting MUST be O(n log n) or better for n returned matches.

### 4.2 Security (NFR-SF-SEC)

- **NFR-SF-SEC-01:** Paths MUST be sandbox-validated before access.
- **NFR-SF-SEC-02:** Output MUST be sanitized before terminal rendering (framework requirement).
- **NFR-SF-SEC-03:** The tool MUST NOT traverse symlinked directories.

### 4.3 Reliability (NFR-SF-REL)

- **NFR-SF-REL-01:** Output MUST be valid JSON even under size constraints.
- **NFR-SF-REL-02:** Non-UTF8 filenames MUST be converted to valid UTF-8 via a lossy conversion.

### 4.4 Portability (NFR-SF-PORT)

- **NFR-SF-PORT-01:** Path separators in output MUST be normalized to `/`.

---

## 5. Configuration

**CFG-SF-01:** The system SHALL support tool-specific configuration under `[tools.search_files]`.

Example:

```toml
[tools.search_files]
backend = "native"            # native | rg | ugrep
max_results = 200
max_matches_per_file = 20
max_file_size_bytes = 2000000
max_files = 5000
max_depth = 12
respect_gitignore_default = true
include_hidden_default = false
follow_symlinks_default = false
mode_default = "exact"         # exact | regex | fuzzy
case_default = "smart"         # sensitive | insensitive | smart
fuzzy_threshold = 0.8
```

**CFG-SF-02:** `max_results`, `max_matches_per_file`, `max_file_size_bytes`, `max_files`, and `max_depth` are hard caps. Requests above caps MUST return `ToolError::BadArgs`.

**CFG-SF-03:** If `backend` is `rg` or `ugrep`, the tool MUST verify the binary exists and meets minimum version requirements at startup. Otherwise return `ToolError::ExecutionFailed` at call time.

**CFG-SF-04:** `fuzzy_threshold` defines the minimum acceptable fuzzy score (0.0 to 1.0). Requests for fuzzy search MUST use this threshold.

---

## 6. Test and Validation Requirements

| ID         | Scenario                                                     | Expected Result                                     |
| ---------- | ------------------------------------------------------------ | --------------------------------------------------- |
| T-SF-01    | Search exact with default args                               | Matches returned, ordered                            |
| T-SF-02    | Invalid regex pattern                                        | BadArgs                                              |
| T-SF-03    | Path outside sandbox                                         | SandboxViolation                                     |
| T-SF-04    | Directory path does not exist                                | ExecutionFailed                                      |
| T-SF-05    | Binary file encountered                                      | Skipped; error recorded if configured                |
| T-SF-06    | max_results exceeded                                         | Truncated=true, reason=max_results                   |
| T-SF-07    | Output size exceeds effective_max                            | Truncated=true, reason=max_output_bytes              |
| T-SF-08    | Non-UTF8 filename                                            | Output remains valid UTF-8                           |
| T-SF-09    | respect_gitignore true                                       | Ignored paths not scanned                             |
| T-SF-10    | Fuzzy mode unsupported by backend                            | BadArgs                                              |

---

## 7. Open Questions

- Should `search_files` use a single backend with optional fuzzy support, or support multiple backends (native, rg, ugrep)?
- If external binaries are used, how are they shipped (bundled per platform, optional dependency, or user-installed)?
- Do we need both `rg` and `ugrep`, or should we standardize on one backend and implement the other mode natively?
- Should Forge build and maintain a search index at startup, or rely on on-demand scanning?
- If indexing is added, should it be event-driven (file watcher) with periodic rescan fallback?
- How should indexing interact with the `read_file` stale-file cache to avoid granting patch permissions implicitly?
- Do we need paging or resume tokens for large result sets to improve agentic workflows?
- Should we include per-file match counts even when no matches are returned to support search narrowing?
- Should we expose a path-only search mode for fast file name discovery?
- Should we allow search scopes by language or file type (e.g., `rust`, `toml`) beyond glob filters?

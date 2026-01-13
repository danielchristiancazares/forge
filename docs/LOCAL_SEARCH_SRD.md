# Local Search Tool

## Software Requirements Document

**Version:** 1.2
**Date:** 2026-01-11
**Status:** Implementation-Ready
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-22 | Header & TOC |
| 23-32 | 0. Change Log |
| 33-84 | 1. Introduction |
| 85-107 | 2. Overall Description |
| 108-315 | 3. Functional Requirements |
| 316-337 | 4. Non-Functional Requirements |
| 338-365 | 5. Configuration |
| 366-400 | 6. Verification Requirements |
| 401-435 | Appendix A - Path Normalization |

---

## 0. Change Log

| Version | Date       | Notes |
| ------- | ---------- | ----- |
| 1.0     | 2026-01-08 | Initial draft |
| 1.1     | 2026-01-11 | Implementation-ready: explicit schema, validation rules, deterministic ordering + truncation detection, exit code mapping, encoding rules, and expanded test matrix |
| 1.2     | 2026-01-11 | Schema extensions: per-file caps, max files, size cap, include/exclude globs, non-recursive mode, match columns/substring, per-file errors, and files_scanned |

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a local filesystem search tool that runs a fast external backend (ugrep or ripgrep), parses structured results, and returns deterministic output for LLM consumption.

### 1.2 Scope

**In Scope**

- Tool identity, request/response schema, and argument validation
- Search semantics (regex, literal, word-boundary, fuzzy)
- Deterministic ordering and truncation behavior
- Backend execution requirements and exit-code handling
- Output parsing, encoding, and response fields
- Configuration keys and defaults
- Verification requirements

**Out of Scope**

- Remote or network search
- Semantic search (handled by CodeQuery)
- Indexing, change tracking, and fuzzy fallback orchestration (see `docs/SEARCH_INDEXING_SRD.md`)
- UI rendering behavior

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| **Tool** | The Forge tool named `Search` specified by this document. |
| **Event** | One element of `matches[]` in the response. `Event = MatchEvent | ContextEvent` where `type` is `"match"` or `"context"`. |
| **Match** | A line that satisfies the search pattern. |
| **Context** | A non-matching line emitted because it is within `context` lines of a match. |
| **Truncation boundary** | The exact event index at which `max_results` causes the tool to stop emitting events. |
| **Order root** | The directory used as the base for computing relative paths for deterministic ordering. |
| **Search root** | The resolved directory or file path that is searched for a given request. |
| **Eligible file set** | The set of files that would be searched for a given request without indexing. |

### 1.4 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework, sandbox rules |
| `docs/SEARCH_INDEXING_SRD.md` | Indexing, change tracking, and fuzzy fallback extensions |
| RFC 2119 / RFC 8174 | Requirement keywords |

### 1.5 Requirement Keywords

RFC 2119 / RFC 8174 keywords apply when capitalized (MUST, SHOULD, etc.).

---

## 2. Overall Description

### 2.1 Product Perspective

Local Search is a non-networked tool executed through Forge's tool subsystem. It shells out to an external search binary (ugrep preferred, ripgrep fallback) and parses results into structured events.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-LS-REQ | Accept search parameters and return matches |
| FR-LS-EXE | Execute external search tool safely |
| FR-LS-PARSE | Parse output into structured match data |
| FR-LS-LIM | Enforce result limits and timeouts |

### 2.3 Constraints

- Requires ugrep or ripgrep installed on PATH or configured explicitly.
- Must honor filesystem sandbox roots and deny patterns.
- Output MUST be deterministic for identical inputs and filesystem state.

---

## 3. Functional Requirements

### 3.1 Tool Identity and Request Schema

**FR-LS-01:** Tool name MUST be `Search` with aliases `search`, `rg`, `ripgrep`, `ugrep`, `ug`.

**FR-LS-02:** Request schema MUST include:

- `pattern` (string, required)
- `path` (string, optional; default is the working directory)
- `case` ("smart" | "sensitive" | "insensitive", optional)
- `fixed_strings` (boolean, optional)
- `word_regexp` (boolean, optional)
- `include_glob` (array of strings, optional; include-only filter)
- `exclude_glob` (array of strings, optional; exclude filter)
- `recursive` (boolean, optional, default true)
- `hidden` (boolean, optional)
- `follow` (boolean, optional)
- `no_ignore` (boolean, optional)
- `context` (integer, optional)
- `max_results` (integer, optional, default 200)
- `max_matches_per_file` (integer, optional)
- `max_files` (integer, optional)
- `max_file_size_bytes` (integer, optional)
- `timeout_ms` (integer, optional, default 20000)
- `fuzzy` (integer 1-4, optional)

**FR-LS-02 (Compatibility):** `glob` MAY be accepted as a deprecated alias for `include_glob`.

**FR-LS-02a (Normative schema):** The JSON schema for request validation is:

```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "pattern": { "type": "string", "minLength": 1 },
    "path": { "type": "string" },
    "case": { "type": "string", "enum": ["smart", "sensitive", "insensitive"], "default": "smart" },
    "fixed_strings": { "type": "boolean", "default": false },
    "word_regexp": { "type": "boolean", "default": false },
    "include_glob": { "type": "array", "items": { "type": "string" } },
    "exclude_glob": { "type": "array", "items": { "type": "string" } },
    "glob": { "type": "array", "items": { "type": "string" } },
    "recursive": { "type": "boolean", "default": true },
    "hidden": { "type": "boolean", "default": false },
    "follow": { "type": "boolean", "default": false },
    "no_ignore": { "type": "boolean", "default": false },
    "context": { "type": "integer", "minimum": 0, "default": 0 },
    "max_results": { "type": "integer", "minimum": 1, "default": 200 },
    "max_matches_per_file": { "type": "integer", "minimum": 1 },
    "max_files": { "type": "integer", "minimum": 1 },
    "max_file_size_bytes": { "type": "integer", "minimum": 1 },
    "timeout_ms": { "type": "integer", "minimum": 1, "default": 20000 },
    "fuzzy": { "type": "integer", "minimum": 1, "maximum": 4 }
  },
  "required": ["pattern"]
}
```

**FR-LS-02b:** Unknown request fields MUST be rejected as `ToolError::BadArgs`.

### 3.2 Argument Validation and Path Handling

- **FR-LS-VAL-01:** `pattern` MUST be non-empty after trimming whitespace.
- **FR-LS-VAL-02:** `context` MUST be >= 0; `max_results` MUST be >= 1; `timeout_ms` MUST be >= 1; `fuzzy` (if present) MUST be in [1, 4].
- **FR-LS-VAL-03:** `path` MUST be validated with the sandbox rules in `docs/TOOL_EXECUTOR_SRD.md` (FR-VAL-04). Relative paths resolve against the sandbox working directory.
- **FR-LS-VAL-04:** If `path` resolves to a file, only that file is searched. If it resolves to a directory, the search is recursive (subject to traversal options).
- **FR-LS-VAL-05:** If `path` does not exist or is not readable, return `ToolError::ExecutionFailed` with a clear message.
- **FR-LS-VAL-06:** `include_glob` and `exclude_glob` entries MUST be non-empty strings; invalid glob syntax MUST return `ToolError::BadArgs`.
- **FR-LS-VAL-07:** If `fuzzy` is provided but the selected backend does not support fuzzy matching, return `ToolError::BadArgs`.
- **FR-LS-VAL-08:** `max_matches_per_file`, `max_files`, and `max_file_size_bytes` MUST be <= configured hard caps; otherwise return `ToolError::BadArgs`.
- **FR-LS-VAL-09:** If `recursive=false`, the search MUST scan only direct children of the target directory (depth 1). If `path` resolves to a file, `recursive` is ignored.
- **FR-LS-VAL-10:** `exclude_glob` patterns are applied after `include_glob`. A file must match at least one include pattern (if any specified) AND match zero exclude patterns to be eligible.
- **FR-LS-VAL-11 (Compatibility):** If `include_glob` is absent and `glob` is present, treat `glob` as `include_glob`. If both are present, `include_glob` is authoritative and `glob` is ignored.

### 3.3 Search Semantics

- **FR-LS-SRC-01:** If `fixed_strings=true`, the pattern MUST be treated as a literal substring. If `fixed_strings=false`, the pattern MUST be treated as a regular expression.
- **FR-LS-SRC-02:** Invalid regular expressions MUST return `ToolError::BadArgs`.
- **FR-LS-SRC-03:** If `word_regexp=true`, the pattern MUST be constrained to word boundaries using backend word-boundary semantics (e.g., `-w`).
- **FR-LS-SRC-04 (Case handling):**
  - `case="sensitive"`: case-sensitive match.
  - `case="insensitive"`: case-insensitive match using ASCII-only casefolding.
  - `case="smart"`: ASCII smartcase (if the pattern contains any ASCII uppercase A-Z, use sensitive; otherwise insensitive).
  - The implementation MUST configure the backend to match these semantics. If the backend cannot be configured to comply, the tool MUST fail with `ToolError::ExecutionFailed`.
- **FR-LS-SRC-05:** `include_glob` patterns restrict the eligible file set to paths matching at least one include glob. If `include_glob` is empty or omitted, no include filter is applied. `exclude_glob` patterns are applied after include filtering to remove matching paths.
- **FR-LS-SRC-06:** `hidden`, `follow`, and `no_ignore` MUST map to their standard backend meanings:
  - `hidden`: include dotfiles.
  - `follow`: follow symlinked directories.
  - `no_ignore`: disable ignore files and global excludes.
- **FR-LS-SRC-07:** `context` emits up to N lines before and after each match as `context` events. Context events count toward `max_results` (see FR-LS-06b).
- **FR-LS-SRC-08:** Files larger than `max_file_size_bytes` (or config default if not specified) MUST be skipped without error.
- **FR-LS-SRC-09:** Once `max_files` files have been scanned, traversal MUST stop. Files are counted regardless of whether they contain matches.
- **FR-LS-SRC-10:** Once `max_matches_per_file` matches are found in a single file, the tool MUST stop searching that file and proceed to the next. Context events do not count toward this limit.
- **FR-LS-SRC-11 (Fuzzy):** If `fuzzy` is provided, the backend MUST run in fuzzy mode with the specified level. Level 1 is the strictest and level 4 is the loosest; higher levels MUST be supersets of lower levels. Exact scoring is backend-defined but MUST be deterministic.

### 3.4 Execution and Backend Requirements

**FR-LS-03:** The tool MUST execute the search process without a shell and enforce timeout and result limits.

- **FR-LS-EXE-01 (Backend selection):** The tool MUST prefer `ugrep` and fall back to `rg` when `ugrep` is unavailable. It MUST verify backend versions at startup:
  - ugrep >= 3.0
  - ripgrep >= 13.0
  If no valid backend is available, return `ToolError::ExecutionFailed`.
- **FR-LS-EXE-02 (Machine-readable output):** The tool MUST invoke the backend in a machine-readable output mode (JSON or equivalent) sufficient to reconstruct `match` and `context` events with path and line number. If machine-readable output is unavailable, return `ToolError::ExecutionFailed`.
- **FR-LS-EXE-03 (Deterministic ordering):** The tool MUST request ordered output from the backend (e.g., sort by path) OR sort parsed events before truncation using the ordering rules in Section 3.5.
- **FR-LS-EXE-04 (Timeout enforcement):** On timeout, the backend process MUST be terminated and `timed_out=true` set (FR-LS-07).
- **FR-LS-EXE-05 (Max-results enforcement):** The tool MUST stop emitting events after `max_results` (FR-LS-06) and MAY terminate the backend once the truncation boundary is reached.

### 3.5 Deterministic Ordering and Truncation

**FR-LS-ORD-01:** The tool MUST define and implement deterministic ordering of events (`matches[]`) for a given request and filesystem state.

**FR-LS-ORD-02 (Ordering model):** Events MUST be ordered by:

1. `path_sort_key` computed via `NormalizeRelPathForOrder(path, order_root)` (Appendix A) using bytewise lexicographic comparison.
2. `line_number` ascending (numeric).
3. For the same file and line:
   - `context` events sort before `match` events.
   - If both type and line are equal, preserve parse order.

**FR-LS-ORD-03 (Order root selection):** The `order_root` MUST be determined deterministically:

1. If indexing is enabled and a configured Index Root contains the request `path`, use that Index Root (see `docs/SEARCH_INDEXING_SRD.md` Section 3.4).
2. Else if the request `path` is absolute: use its canonicalized directory (file parent if `path` is a file).
3. Else: use the sandbox working directory.

**FR-LS-ORD-04:** If backend ordering cannot be guaranteed, the tool MUST sort all parsed events before truncation. If sorting cannot complete within timeout, the tool MUST set `timed_out=true` rather than emit nondeterministic order.

**FR-LS-06:** The tool MUST stop after `max_results` match/context events and mark `truncated=true` if additional events exist.

**FR-LS-06a (Truncation detection):** To determine whether additional events exist, the tool MUST attempt to observe one additional event beyond `max_results` (buffered, not emitted). If it cannot safely observe +1 (timeout/budget), it MUST set `truncated=true`.

**FR-LS-06b:** Match and context events both count toward `max_results`.

**FR-LS-07:** On timeout, the process MUST be terminated and `timed_out=true`.

**FR-LS-07a:** If a timeout occurs before truncation detection completes, `timed_out=true` and `truncated` MAY be true or false depending on whether +1 observation was possible.

### 3.6 Parsing and Response

**FR-LS-04:** The tool MUST parse line-oriented output into structured match records:

```json
{
  "type": "match",
  "data": {
    "path": { "text": "<path>" },
    "line_number": <u64>,
    "column": <u64>,
    "lines": { "text": "<line text>" },
    "match_text": "<matched substring>"
  }
}
```

**FR-LS-04a:** `line_number` MUST be 1-based. `lines.text` MUST NOT include trailing newline characters. `path.text` MUST use forward slashes.
**FR-LS-04b:** `column` MUST be 1-based, indicating the byte offset of the match start within the line.
**FR-LS-04c:** `match_text` MUST contain the matched substring. For regex mode, this is the full match (group 0). For exact/fuzzy modes, this is the matched literal. Context events MUST omit `column` and `match_text`.

**FR-LS-05:** Response payload MUST include:

- `pattern`
- `path`
- `count`
- `matches` (array of structured records)
- `truncated` (boolean)
- `timed_out` (boolean)
- `files_scanned` (integer)
- `errors` (array of error objects; may be empty)
- `exit_code` (optional integer)
- `stderr` (optional string)
- `content` text view for human readability

**FR-LS-05a:** `count` MUST equal `matches.length` and MUST count both match and context events.

**FR-LS-05b:** `pattern` MUST echo the request `pattern`. `path` MUST be the normalized effective search root (forward slashes, resolved per FR-LS-VAL-03/04).

**FR-LS-05c:** `content` MUST be a UTF-8 string and SHOULD represent a human-readable view of results in the same order as `matches`. If `truncated` or `timed_out`, `content` SHOULD include a short trailing note.

**FR-LS-05d (Extensions):** Additional top-level fields MAY be included only if documented by an extension SRD (e.g., `stats` in `docs/SEARCH_INDEXING_SRD.md`). Consumers MUST ignore unknown fields.

**FR-LS-05e:** `files_scanned` MUST be the number of eligible files examined (post include/exclude filtering), including files skipped due to size or per-file match limits.

**FR-LS-ENC-01:** Backend stdout and stderr MUST be decoded as UTF-8 with deterministic lossy conversion (invalid byte sequences replaced with U+FFFD). JSON parse failures after decoding MUST be treated as backend errors.

### 3.7 Exit Codes and Error Handling

**Exit code mapping (ugrep and ripgrep):**

| Exit Code | Meaning | Classification |
| --------- | ------- | -------------- |
| 0 | Matches found | ok_with_matches |
| 1 | No matches | ok_no_matches |
| 2 | Error (syntax, IO) | backend_error |
| >2 | Error | backend_error |

- **FR-LS-ERR-01:** Invalid arguments MUST return `ToolError::BadArgs` with a clear message.
- **FR-LS-ERR-02:** Backend spawn failures or unsupported backend versions MUST return `ToolError::ExecutionFailed`.
- **FR-LS-ERR-03:** Backend exit codes MUST be classified per the table above. Exit code 1 MUST NOT be treated as an error.
- **FR-LS-ERR-04:** If a backend error occurs after some events were parsed, the tool MAY return partial results with `exit_code` and `stderr` set. If parsing cannot be trusted, return `ToolError::ExecutionFailed`.
- **FR-LS-ERR-05:** Per-file I/O errors (permission denied, file deleted mid-scan, encoding failures) MUST NOT fail the overall tool call.
- **FR-LS-ERR-06:** Per-file errors MUST be recorded in an `errors` array in the response: `{ "path": "<relative path>", "error": "<message>" }`.
- **FR-LS-ERR-07:** The `errors` array MUST be included in the response even if empty.

---

## 4. Non-Functional Requirements

### 4.1 Correctness and Determinism

- **NFR-LS-COR-01:** For identical inputs and filesystem state, `matches[]` ordering and truncation boundary MUST be identical.
- **NFR-LS-COR-02:** The tool MUST never emit events in nondeterministic order.

### 4.2 Security

- **NFR-LS-SEC-01:** All filesystem access MUST obey sandbox containment and deny patterns (Tool Executor FR-VAL-04/06).
- **NFR-LS-SEC-02:** The tool MUST be read-only and MUST NOT modify files.
- **NFR-LS-SEC-03:** Search results MUST NOT satisfy stale-file "read" gating for edit tools (see `docs/SEARCH_INDEXING_SRD.md` SEC-LSI-CACHE-01/02).

### 4.3 Performance and Portability

- **NFR-LS-PERF-01:** Search SHOULD stream results incrementally.
- **NFR-LS-PERF-02:** Parsing SHOULD be linear in output size.
- **NFR-LS-PORT-01:** Must work on Windows/macOS/Linux.
- **NFR-LS-PORT-02:** Path normalization MUST be specified and implemented consistently (Appendix A).

---

## 5. Configuration

Configuration is under `[tools.search]`.

```toml
[tools.search]
enabled = false
binary = "ugrep"
fallback_binary = "rg"
default_timeout_ms = 20000
default_max_results = 200
max_matches_per_file = 50
max_files = 10000
max_file_size_bytes = 2000000
```

**CFG-LS-01:** `binary` and `fallback_binary` MUST be resolved at startup. If a configured binary is missing or below minimum version, it MUST be ignored (and fallback attempted).

**CFG-LS-02:** `default_timeout_ms` and `default_max_results` MUST be positive integers.

**CFG-LS-03:** Per-request `timeout_ms` and `max_results` override defaults when provided, subject to validation rules in Section 3.2.

**CFG-LS-04:** `max_matches_per_file`, `max_files`, and `max_file_size_bytes` are hard caps. Requests exceeding them MUST return `ToolError::BadArgs`.

**CFG-LS-05:** If `max_file_size_bytes` is not specified in the request, the config default applies.

---

## 6. Verification Requirements

### 6.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-LS-PARSE-01 | Parse match and context lines into structured events |
| T-LS-ORD-01 | Deterministic ordering for same inputs |
| T-LS-TRUNC-01 | Truncation detection when more than max_results exist |
| T-LS-TRUNC-02 | `truncated=false` when exactly max_results exist |
| T-LS-CASE-01 | Smartcase ASCII detection |
| T-LS-ENC-01 | Invalid UTF-8 decoded with U+FFFD deterministically |
| T-LS-MPF-01 | `max_matches_per_file` stops after N matches in single file |
| T-LS-MF-01 | `max_files` stops traversal after N files scanned |
| T-LS-SIZE-01 | Files exceeding `max_file_size_bytes` are skipped |
| T-LS-EXCL-01 | `exclude_glob` filters out matching files |
| T-LS-INCL-01 | `include_glob` + `exclude_glob` interaction |
| T-LS-REC-01 | `recursive=false` scans only direct children |
| T-LS-COL-01 | `column` is accurate for match position |
| T-LS-MTXT-01 | `match_text` extracts correct substring |
| T-LS-ERR-01 | Permission-denied file recorded in `errors`, search continues |

### 6.2 Integration Tests

| Test ID | Description |
| --- | --- |
| IT-LS-E2E-01 | Search returns expected matches |
| IT-LS-FZ-01 | Fuzzy mode returns approximate matches (ugrep) |
| IT-LS-EXIT-01 | Exit code 1 classified as ok_no_matches |
| IT-LS-EXIT-02 | Exit code 2 classified as backend_error |
| IT-LS-CTX-01 | Context events counted toward max_results |
| IT-LS-PATH-01 | Order normalization stable across platforms |

---

# Appendix A: Path Normalization (Normative)

This appendix defines normalization functions used for deterministic ordering. The algorithms align with `docs/TOOL_EXECUTOR_SRD.md` FR-VAL-04 (path validation) and FR-VAL-06 (symlink safety).

### NormalizePathForKey(path, base_dir)

Used for: sandbox validation and canonical absolute path resolution.

**Algorithm:**

1. If `path` is relative, resolve against the sandbox working directory.
2. Reject any `..` component that would escape the sandbox root.
3. Find the deepest existing ancestor and canonicalize that ancestor (symlink-safe).
4. Reconstruct the target under the canonical ancestor.
5. Normalize separators to `/` (forward slash).
6. On Windows, normalize drive letter casing consistently (uppercase).
7. Remove redundant `.` segments and trailing `/` except for root paths.

### NormalizeRelPathForOrder(path, order_root)

Used for: `path_sort_key` in deterministic ordering.

**Algorithm:**

1. Apply `NormalizePathForKey(path, order_root)` to obtain a canonical absolute path.
2. Compute the relative path from `order_root` to the canonical path.
3. On Unix, convert non-UTF-8 bytes using deterministic lossy conversion (equivalent to Rust `to_string_lossy()`).
4. Apply Unicode NFC normalization.
5. Encode as UTF-8; the resulting bytes are the `path_sort_key`.

### Path Comparison

Compare `path_sort_key` values using bytewise lexicographic ordering (no locale). Shorter paths sort before longer paths when one is a prefix of the other.

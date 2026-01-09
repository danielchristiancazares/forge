# Smart File Edit Tool
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
* Initial requirements for a newline-preserving edit tool based on `../tools` smart_file_edit.

---

## 1. Introduction

### 1.1 Purpose
Specify requirements for a smart, newline-aware edit tool that performs targeted snippet or line-range edits while preserving original file line endings.

### 1.2 Scope
The Smart File Edit tool will:
* Read files as raw bytes and preserve line endings
* Normalize content to a canonical LF view for matching
* Replace a snippet or line range safely
* Guard against stale edits using file hash validation

Out of scope:
* Full-file formatting
* Multi-file batch edits (handled by the tool loop)

### 1.3 Definitions
| Term | Definition |
| --- | --- |
| Canonical view | LF-only representation used for matching |
| Dominant newline | Most frequent newline style in the file |
| Match hint | Optional line-range hint for disambiguation |

### 1.4 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `docs/LP1.md` | Patch format (related, not required) |
| `../tools/src/smart_file_edit/mod.rs` | Reference implementation |
| RFC 2119 / RFC 8174 | Requirement keywords |

### 1.5 Requirement Keywords
The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective
Smart File Edit is a file-editing tool that complements `apply_patch` by offering snippet and line-range replacement while preserving original newline bytes.

### 2.2 Product Functions
| Function | Description |
| --- | --- |
| FR-SFE-REQ | Accept edit instructions and return status |
| FR-SFE-LF | Normalize to LF for matching |
| FR-SFE-NL | Preserve dominant newline style |
| FR-SFE-STALE | Prevent edits on stale files |

### 2.3 User Characteristics
* LLMs submit snippet or line-range edits.
* Users review and approve edits via tool approval UI.

### 2.4 Constraints
* Must operate inside the Forge filesystem sandbox.
* Must respect tool approval policy for mutating operations.

---

## 3. Functional Requirements

### 3.1 Tool Interface
**FR-SFE-01:** Tool name MUST be `Edit` with alias `edit`.

**FR-SFE-02:** Request schema MUST allow two modes:

Snippet mode:
* `path` (string, required)
* `old_snippet` (string, required)
* `new_snippet` (string, required)
* `match_hint` (object, optional: `start_line`, `end_line`)
* `file_hash` (string, optional)
* `region_id` (string, optional)

Line-range mode:
* `path` (string, required)
* `start_line` (integer, required)
* `end_line` (integer, required)
* `new_content` (string, required)
* `file_hash` (string, optional)
* `region_id` (string, optional)

**FR-SFE-03:** The tool MUST reject requests that mix snippet and line-range modes incorrectly.

**FR-SFE-04:** Response payload MUST include:
* `action` ("apply_snippet_edit" or "apply_line_edit")
* `status` ("ok" | "no_match" | "stale_file" | "error")
* `message` (human-readable)
* `current_file_hash`
* `newline_kind` (e.g., "LF", "CRLF", "CR")
* Optional `region_id` echo

### 3.2 Canonical LF Processing
**FR-SFE-05:** The tool MUST parse file bytes and build a canonical LF-only view for matching.

**FR-SFE-06:** Matching MUST be performed on the canonical view and mapped back to byte offsets in the original file.

### 3.3 Newline Preservation
**FR-SFE-07:** The tool MUST detect newline kinds (LF, CRLF, CR) and select a dominant style.

**FR-SFE-08:** Replacement content MUST be written using the dominant newline style.

**FR-SFE-09:** In case of equal counts, dominance MUST be CRLF > LF > CR.

### 3.4 Staleness and Match Handling
**FR-SFE-10:** If `file_hash` is provided and does not match the current file hash, the tool MUST return `stale_file` and MUST NOT modify the file.

**FR-SFE-11:** If multiple matches exist and `match_hint` is provided, the tool MUST prefer matches within the hinted range and MUST NOT fall back if no match exists in that range.

**FR-SFE-12:** If no match is found, the tool SHOULD return a limited set of candidate suggestions (line previews) when feasible.

### 3.5 Atomicity
**FR-SFE-13:** Edits MUST be applied atomically (write temp file then rename) to avoid partial writes.

---

## 4. Non-Functional Requirements

### 4.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-SFE-SEC-01 | Operate within tool sandbox and deny unsafe paths |
| NFR-SFE-SEC-02 | Respect approval policy for mutating operations |

### 4.2 Reliability
| Requirement | Specification |
| --- | --- |
| NFR-SFE-REL-01 | Stale file detection MUST prevent unintended edits |
| NFR-SFE-REL-02 | Failures MUST not corrupt file content |

---

## 5. Configuration

```toml
[tools.edit]
enabled = false
max_snippet_bytes = 262144
max_line_edit_bytes = 262144
require_file_hash = false
```

---

## 6. Verification Requirements

### 6.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-SFE-NL-01 | CRLF file preserves CRLF after edit |
| T-SFE-NL-02 | Mixed newlines choose dominant style |
| T-SFE-STALE-01 | Stale hash rejects edit |
| T-SFE-HINT-01 | Match hint restricts replacements |
| T-SFE-ATOM-01 | Atomic write prevents partial edits |

### 6.2 Integration Tests
| Test ID | Description |
| --- | --- |
| IT-SFE-E2E-01 | Snippet edit succeeds with canonical LF input |
| IT-SFE-LINE-01 | Line-range edit replaces exact lines |


# Local Search Tool
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
* Initial requirements for a local search tool based on `../tools` Search (ugrep/ripgrep) implementation.

---

## 1. Introduction

### 1.1 Purpose
Define requirements for a local search tool that performs regex and literal searches over the filesystem with structured results for LLM consumption.

### 1.2 Scope
The Local Search tool will:
* Execute a fast search process (ugrep or ripgrep)
* Support regex, literal, word-boundary, and fuzzy modes
* Return both human-readable output and structured match records

Out of scope:
* Remote code search
* Semantic search (handled by CodeQuery)

### 1.3 Definitions
| Term | Definition |
| --- | --- |
| Match | A line that satisfies the search pattern |
| Context | Surrounding lines emitted by search tool |
| Fuzzy | Approximate matching with edit distance |

### 1.4 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/search.rs` | Tool schema |
| `../tools/src/tools/handlers/ripgrep.rs` | Handler behavior |
| RFC 2119 / RFC 8174 | Requirement keywords |

### 1.5 Requirement Keywords
The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective
Local Search is a non-networked tool executed through Forge's Tool Executor. It shells out to an external search binary (ugrep preferred) and parses output into structured results.

### 2.2 Product Functions
| Function | Description |
| --- | --- |
| FR-LS-REQ | Accept search parameters and return matches |
| FR-LS-EXE | Execute external search tool safely |
| FR-LS-PARSE | Parse output into structured match data |
| FR-LS-LIM | Enforce result limits and timeouts |

### 2.3 User Characteristics
* LLMs use the tool to locate code references.
* Developers configure default limits and tool paths.

### 2.4 Constraints
* Requires ugrep or ripgrep to be installed on PATH or configured explicitly.
* Must honor filesystem sandbox roots.

---

## 3. Functional Requirements

### 3.1 Tool Interface
**FR-LS-01:** Tool name MUST be `Search` with aliases `search`, `rg`, `ripgrep`, `ugrep`, `ug`.

**FR-LS-02:** Request schema MUST include:
* `pattern` (string, required)
* `path` (string, optional; default current directory)
* `case` ("smart" | "sensitive" | "insensitive", optional)
* `fixed_strings` (boolean, optional)
* `word_regexp` (boolean, optional)
* `glob` (array of strings, optional)
* `hidden` (boolean, optional)
* `follow` (boolean, optional)
* `no_ignore` (boolean, optional)
* `context` (integer, optional)
* `max_results` (integer, optional, default 200)
* `timeout_ms` (integer, optional, default 20000)
* `fuzzy` (integer 1-4, optional)

**FR-LS-03:** The tool MUST execute the search process without a shell and enforce timeout and result limits.

**FR-LS-04:** The tool MUST parse line-oriented output into structured match records:
```
{
  "type": "match" | "context",
  "data": {
    "path": { "text": "<path>" },
    "line_number": <u64>,
    "lines": { "text": "<line text>" }
  }
}
```

**FR-LS-05:** Response payload MUST include:
* `pattern`
* `path`
* `count`
* `matches` (array of structured records)
* `truncated` (boolean)
* `timed_out` (boolean)
* `exit_code` (optional integer)
* `stderr` (optional string)
* `content` text view for human readability

### 3.2 Limits and Truncation
**FR-LS-06:** The tool MUST stop after `max_results` match/context events and mark `truncated=true`.

**FR-LS-07:** On timeout, the process MUST be terminated and `timed_out=true`.

---

## 4. Non-Functional Requirements

### 4.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-LS-SEC-01 | Must enforce sandboxed root path access |
| NFR-LS-SEC-02 | Must not execute via shell |

### 4.2 Performance
| Requirement | Specification |
| --- | --- |
| NFR-LS-PERF-01 | Search should stream results incrementally |
| NFR-LS-PERF-02 | Parsing should be linear in output size |

---

## 5. Configuration

```toml
[tools.search]
enabled = false
binary = "ugrep"
fallback_binary = "rg"
default_timeout_ms = 20000
default_max_results = 200
```

---

## 6. Verification Requirements

### 6.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-LS-PARSE-01 | Parse match lines and context lines |
| T-LS-LIM-01 | Result truncation sets flag |
| T-LS-TMO-01 | Timeout kills process |

### 6.2 Integration Tests
| Test ID | Description |
| --- | --- |
| IT-LS-E2E-01 | Search returns expected matches |
| IT-LS-FZ-01 | Fuzzy mode returns approximate matches |


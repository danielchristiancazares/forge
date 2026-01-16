# Read File Tool

## Software Requirements Document

**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Final  
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-21 | Header & TOC |
| 22-29 | Change Log |
| 30-64 | Introduction: Purpose, Scope, Definitions |
| 65-80 | Overall Description: Perspective, Functions |
| 81-113 | Functional Requirements: Interface, Output, Limits |
| 114-123 | Non-Functional Requirements |
| 124-134 | Configuration |
| 135-145 | Verification Requirements |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for a read-only file tool based on `../tools` Read handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that reads file contents with optional line-range controls and safe handling for large/binary files.

### 1.2 Scope

The Read File tool will:

* Read text or binary files within the sandbox
* Support line range and head/tail convenience reads
* Return line-numbered output by default

Out of scope:

* File edits or patches (handled by Edit/ApplyPatch)

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| Line range | Inclusive range of 1-indexed line numbers |
| Head/Tail | Convenience read of first/last N lines |

### 1.4 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/read.rs` | Tool schema |
| `../tools/src/tools/handlers/read_file.rs` | Handler behavior |

---

## 2. Overall Description

### 2.1 Product Perspective

Read File is a low-risk, read-only tool executed within Forge’s sandbox.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-RD-REQ | Accept file path + optional range params |
| FR-RD-LIM | Enforce size and scan limits |
| FR-RD-BIN | Detect binary files and return base64 |

---

## 3. Functional Requirements

### 3.1 Tool Interface

**FR-RD-01:** Tool name MUST be `Read` with aliases `read`, `read_file`, `read-file`, `ReadFile`.

**FR-RD-02:** Request schema MUST include:

* `path` (string, required)
* `start_line` (integer, optional, 1-indexed)
* `end_line` (integer, optional, 1-indexed)
* `limit` (integer, optional)
* `head` (integer, optional)
* `tail` (integer, optional)
* `show_line_numbers` (boolean, optional, default true)

**FR-RD-03:** `head`/`tail` MUST be mutually exclusive with `start_line`/`end_line`/`limit`.

### 3.2 Output

**FR-RD-04:** Output MUST include a text view with optional line numbers.

**FR-RD-05:** Binary files MUST return a base64 payload and indicate binary status.

### 3.3 Limits

**FR-RD-06:** The tool MUST enforce:

* `max_file_read_bytes`
* `max_scan_bytes` for line-based scanning

---

## 4. Non-Functional Requirements

### 4.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-RD-SEC-01 | Must enforce sandboxed path access |

---

## 5. Configuration

```toml
[tools.read_file]
max_file_read_bytes = 204800
max_scan_bytes = 2097152
show_line_numbers = true
```

---

## 6. Verification Requirements

### 6.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-RD-01 | Read full file |
| T-RD-02 | Line range read |
| T-RD-03 | Head/tail mutually exclusive |
| T-RD-04 | Binary file returns base64 |

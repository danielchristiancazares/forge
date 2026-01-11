# Write File Tool

## Software Requirements Document

**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-15 | Header & Change Log |
| 16-41 | Introduction: Purpose, Scope, Definitions |
| 42-55 | Overall Description: Perspective, Functions |
| 56-75 | Functional Requirements: Interface, Output |
| 76-106 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for a write tool based on `../tools` Write handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that creates new files (or overwrites existing ones) within the sandbox.

### 1.2 Scope

The Write tool will:

* Create or overwrite files
* Support text or base64 binary content
* Provide safe path validation

Out of scope:

* Partial edits (handled by Edit/ApplyPatch)

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| Overwrite | Replace existing file contents |

### 1.4 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/write.rs` | Tool schema |

---

## 2. Overall Description

### 2.1 Product Perspective

Write is a mutating file tool subject to approval policy and sandbox rules.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-WR-REQ | Accept path + content |
| FR-WR-ENC | Support text and base64 payloads |
| FR-WR-OVR | Optional overwrite behavior |

---

## 3. Functional Requirements

### 3.1 Tool Interface

**FR-WR-01:** Tool name MUST be `Write` with alias `write`.

**FR-WR-02:** Request schema MUST include:

* `path` (string, required)
* `content` (string, required)
* `base64` (boolean, optional, default false)
* `overwrite` (boolean, optional, default false)

**FR-WR-03:** If `overwrite=false` and the file exists, the tool MUST return an error.

**FR-WR-04:** If `base64=true`, `content` MUST be decoded from base64 bytes before writing.

### 3.2 Output

**FR-WR-05:** Response payload MUST include status and bytes written.

---

## 4. Non-Functional Requirements

### 4.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-WR-SEC-01 | Must enforce sandboxed path access |
| NFR-WR-SEC-02 | Mutating operation requires approval per policy |

---

## 5. Configuration

```toml
[tools.write]
max_write_bytes = 1048576
allow_overwrite = false
```

---

## 6. Verification Requirements

### 6.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-WR-01 | Create new file |
| T-WR-02 | Overwrite blocked when not allowed |
| T-WR-03 | Base64 write produces correct bytes |

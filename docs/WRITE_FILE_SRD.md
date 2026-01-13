# Write File Tool

## Software Requirements Document

**Version:** 1.1  
**Date:** 2026-01-12  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-33 | Header & TOC |
| 34-66 | Introduction |
| 67-81 | Overall Description |
| 82-99 | Functional Requirements |
| 100-110 | Non-Functional Requirements |
| 111-116 | Configuration |
| 117-125 | Verification Requirements |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for a write tool based on `../tools` Write handler.

### 0.2 Alignment update

* Updated tool name and behavior to match Forge `write_file` implementation (create-new only, no overwrite/base64).

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that creates new files within the sandbox.

### 1.2 Scope

The write_file tool will:

* Create new files only (no overwrite)
* Write text content
* Provide safe path validation

Out of scope:

* Partial edits (handled by Edit/ApplyPatch)

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| Create-new | Create file only if it does not already exist |

### 1.4 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/write.rs` | Reference handler |

---

## 2. Overall Description

### 2.1 Product Perspective

write_file is a mutating file tool subject to approval policy and sandbox rules.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-WR-REQ | Accept path + content |
| FR-WR-NEW | Create-new only (no overwrite) |

---

## 3. Functional Requirements

### 3.1 Tool Interface

**FR-WR-01:** Tool name MUST be `write_file`.

**FR-WR-02:** Request schema MUST include:

* `path` (string, required)
* `content` (string, required)
**FR-WR-03:** If the file already exists, the tool MUST return an error.

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

No write_file-specific configuration is defined in this version.

---

## 6. Verification Requirements

### 6.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-WR-01 | Create new file |
| T-WR-02 | Existing file returns error |

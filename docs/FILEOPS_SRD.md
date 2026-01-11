# File Operations Tools (Move/Copy/ListDir)

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
| 16-38 | Introduction: Move, Copy, ListDir scope |
| 39-80 | Functional Requirements: Interface, Move, Copy, ListDir logic |
| 81-110 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Move, Copy, and ListDir tools based on `../tools` fileops.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for file operation tools that move, copy, and list directory contents within the sandbox.

### 1.2 Scope

Tools included:

* Move (rename)
* Copy
* ListDir

Out of scope:

* Recursive delete
* Advanced file metadata operations

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/fileops.rs` | Tool schemas and handlers |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-FOPS-01:** Tools MUST be named `Move`, `Copy`, and `ListDir` with aliases:

* Move: `move`, `rename`, `mv`
* Copy: `copy`, `cp`
* ListDir: `listdir`, `ls`, `dir`

### 2.2 Move

**FR-FOPS-MV-01:** Request schema MUST include:

* `source` (string, required)
* `destination` (string, required)
* `overwrite` (boolean, optional, default false)

**FR-FOPS-MV-02:** If destination is a directory, the tool MUST move into it using the source filename.

**FR-FOPS-MV-03:** If `overwrite=false` and destination exists, the tool MUST return an error.

**FR-FOPS-MV-04:** If rename fails across filesystems, the tool MUST fallback to copy + delete for files.

### 2.3 Copy

**FR-FOPS-CP-01:** Request schema MUST include:

* `source` (string, required)
* `destination` (string, required)
* `overwrite` (boolean, optional, default false)

**FR-FOPS-CP-02:** If destination is a directory, the tool MUST copy into it using the source filename.

**FR-FOPS-CP-03:** If `overwrite=false` and destination exists, the tool MUST return an error.

### 2.4 ListDir

**FR-FOPS-LS-01:** Request schema MUST include:

* `path` (string, required)
* `all` (boolean, optional, default false)
* `long` (boolean, optional, default false)

**FR-FOPS-LS-02:** If `long=true`, entries MUST include size, modified time, and type.

**FR-FOPS-LS-03:** Entries MUST be sorted lexicographically.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-FOPS-SEC-01 | Must enforce sandboxed path access |
| NFR-FOPS-SEC-02 | Move/Copy operations require approval per policy |

---

## 4. Configuration

```toml
[tools.fileops]
allow_overwrite = false
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-FOPS-MV-01 | Move file to directory |
| T-FOPS-MV-02 | Overwrite blocked |
| T-FOPS-CP-01 | Copy file to directory |
| T-FOPS-LS-01 | ListDir returns sorted entries |

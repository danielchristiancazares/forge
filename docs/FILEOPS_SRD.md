# File Operations Tools (Move/Copy/List Directory)

## Software Requirements Document

**Version:** 1.1  
**Date:** 2026-01-12  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-20 | Header & TOC |
| 21-32 | 0. Change Log |
| 33-60 | 1. Introduction |
| 61-102 | 2. Functional Requirements |
| 103-113 | 3. Non-Functional Requirements |
| 114-124 | 4. Configuration |
| 125-135 | 5. Verification Requirements |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Move, Copy, and list_directory tools based on `../tools` fileops.

### 0.2 Alignment update

* list_directory is specified in `docs/LIST_DIRECTORY_SRD.md`; updated naming and config references.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for file operation tools that move, copy, and list directory contents within the sandbox.

### 1.2 Scope

Tools included:

* Move (rename)
* Copy
* list_directory (alias: listdir/ls/dir)

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

**FR-FOPS-01:** Tools MUST be named `move`, `copy`, and `list_directory` with aliases:

* Move: `move`, `rename`, `mv`
* Copy: `copy`, `cp`
* list_directory: `list_directory`, `listdir`, `ls`, `dir`

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

### 2.4 list_directory

**FR-FOPS-LS-01:** The list_directory tool behavior and schema are specified in `docs/LIST_DIRECTORY_SRD.md`. This document does not add additional requirements.

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

Note: list_directory configuration is under `[tools.list_directory]` (see `docs/LIST_DIRECTORY_SRD.md`).

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-FOPS-MV-01 | Move file to directory |
| T-FOPS-MV-02 | Overwrite blocked |
| T-FOPS-CP-01 | Copy file to directory |
| T-FOPS-LS-01 | list_directory returns sorted entries |

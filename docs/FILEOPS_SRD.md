# File Operations Tools (Move/Copy)

## Software Requirements Document

**Version:** 1.2
**Date:** 2026-01-16
**Status:** Draft
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-18 | Header & TOC |
| 19-28 | 0. Change Log |
| 29-50 | 1. Introduction |
| 51-82 | 2. Functional Requirements |
| 83-93 | 3. Non-Functional Requirements |
| 94-102 | 4. Configuration |
| 103-111 | 5. Verification Requirements |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Move and Copy tools.

### 0.2 Scope refinement

* Removed list_directory (see `docs/LIST_DIRECTORY_SRD.md`).

---

## 1. Introduction

### 1.1 Purpose

Define requirements for file operation tools that move and copy files within the sandbox.

### 1.2 Scope

Tools included:

* Move (rename)
* Copy

Out of scope:

* Recursive delete
* Advanced file metadata operations
* Directory listing (see `docs/LIST_DIRECTORY_SRD.md`)

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-FOPS-01:** Tools MUST be named `move` and `copy` with aliases:

* Move: `move`, `rename`, `mv`
* Copy: `copy`, `cp`

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

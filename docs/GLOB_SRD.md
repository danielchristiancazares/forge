# Glob Tool

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
| 16-34 | Introduction: Purpose, Scope (Glob tool) |
| 35-56 | Functional Requirements: Interface, Behavior |
| 57-83 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for a glob tool based on `../tools` Glob handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that lists files matching glob patterns with ignore awareness.

### 1.2 Scope

The Glob tool will:

* Expand glob patterns to file paths
* Respect ignore files unless configured otherwise
* Return sorted matches

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/glob.rs` | Tool schema |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-GLB-01:** Tool name MUST be `Glob` with alias `glob`.

**FR-GLB-02:** Request schema MUST include:

* `pattern` (string, required)
* `path` (string, optional, default ".")
* `hidden` (boolean, optional)
* `no_ignore` (boolean, optional)
* `follow` (boolean, optional)
* `max_results` (integer, optional, default 2000)

### 2.2 Behavior

**FR-GLB-03:** The tool MUST enforce sandbox rules on the root path and results.

**FR-GLB-04:** The tool MUST return results sorted lexicographically.

**FR-GLB-05:** The tool MUST stop at `max_results` and indicate truncation.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-GLB-SEC-01 | Must enforce sandboxed path access |

---

## 4. Configuration

```toml
[tools.glob]
default_max_results = 2000
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-GLB-01 | Glob returns matching files |
| T-GLB-02 | Max results truncates |

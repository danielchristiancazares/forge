# Outline Tool

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
| 16-39 | Introduction: Purpose, Scope (C++ parsing) |
| 40-56 | Functional Requirements: Interface, Behavior |
| 57-82 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Outline tool based on `../tools` C++ outline handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that extracts C++ class and method signatures using tree-sitter.

### 1.2 Scope

The Outline tool will:

* Parse C++ source files
* Extract class/struct/function signatures
* Return a structured outline

Out of scope:

* Other languages (initial version)
* Type inference beyond syntax tree data

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/outline.rs` | Tool schema |
| `../tools/src/tools/handlers/*` | Handler behavior |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-OUT-01:** Tool name MUST be `Outline` with alias `outline`.

**FR-OUT-02:** Request schema MUST include:

* `path` (string, required)

### 2.2 Behavior

**FR-OUT-03:** The tool MUST parse the file using tree-sitter-cpp and extract:

* class/struct names
* method/function signatures

**FR-OUT-04:** The tool MUST return a structured outline and a human-readable summary.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-OUT-SEC-01 | Must enforce sandboxed path access |

---

## 4. Configuration

```toml
[tools.outline]
enabled = false
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-OUT-01 | Extracts class and method signatures |

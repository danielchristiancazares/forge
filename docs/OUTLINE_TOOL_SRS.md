# Outline Tool

## Software Requirements Document

**Version:** 1.1
**Date:** 2026-01-16
**Status:** Implementation Ready
**Baseline code reference:** `../tools/src/tools/outline.rs`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-20 | Header & TOC |
| 21-27 | 0. Change Log |
| 28-56 | 1. Introduction |
| 57-77 | 2. Functional Requirements |
| 78-87 | 3. Non-Functional Requirements |
| 88-96 | 4. Configuration |
| 97-105 | 5. Verification Requirements |

---

## 0. Change Log

### 0.2 Implementation Ready

* Added `include_private` parameter from reference implementation.
* Updated baseline reference and status.

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
* `include_private` (boolean, optional, default false)

### 2.2 Behavior

**FR-OUT-03:** The tool MUST parse the file using tree-sitter-cpp and extract:

* namespaces
* class/struct names with base classes
* enum specifiers (including enum class)
* method/function signatures
* type definitions and aliases
* template declarations
* preprocessor includes and conditionals

**FR-OUT-04:** When `include_private` is false (default), the tool MUST omit private members from class outlines.

**FR-OUT-05:** The tool MUST preserve doc comments (`///` and `/**`) preceding declarations.

**FR-OUT-06:** The tool MUST return the outline as human-readable indented text.

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

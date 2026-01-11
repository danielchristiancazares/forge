# Test Tool

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
| 16-36 | Introduction: Purpose, Scope (Test tool) |
| 37-56 | Functional Requirements: Interface, Behavior |
| 57-87 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Test tool based on `../tools` test handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that runs a project test script in a controlled manner.

### 1.2 Scope

The Test tool will:

* Execute a platform-appropriate test command
* Stream output and enforce timeouts

Out of scope:

* Arbitrary shell command execution (handled by RunCommand/Pwsh)

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/test.rs` | Tool schema |
| `../tools/src/tools/handlers/script_runner.rs` | Handler behavior |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-TST-01:** Tool name MUST be `Test` with alias `test`.

**FR-TST-02:** Request schema MUST include:

* `command` (string, optional)
* `workdir` (string, optional)
* `timeout_ms` (integer, optional)

### 2.2 Behavior

**FR-TST-03:** If `command` is omitted, the tool MUST run a default test script (platform-specific).

**FR-TST-04:** Output MUST be streamed and truncated to configured limits.

**FR-TST-05:** Tool MUST enforce timeout and return exit code.

**FR-TST-06:** Mutating execution MUST be approval-gated.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-TST-SEC-01 | Must sanitize environment variables |

---

## 4. Configuration

```toml
[tools.test]
default_timeout_ms = 300000
default_command_windows = "powershell -File test.ps1"
default_command_unix = "./scripts/test.sh"
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-TST-01 | Default command selected per platform |
| T-TST-02 | Timeout terminates process |

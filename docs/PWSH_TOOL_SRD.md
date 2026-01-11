# PowerShell Tool

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
| 16-36 | Introduction: Purpose, Scope (PowerShell) |
| 37-56 | Functional Requirements: Interface, Behavior |
| 57-86 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for PowerShell execution tool based on `../tools` pwsh handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a PowerShell command execution tool on Windows.

### 1.2 Scope

The PowerShell tool will:

* Execute a provided PowerShell command
* Stream output and enforce timeouts
* Sanitize environment variables

Out of scope:

* Unix shell execution (handled by run_command or platform-specific tools)

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/pwsh.rs` | Tool schema |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-PWSH-01:** Tool name MUST be `Pwsh` with alias `pwsh`.

**FR-PWSH-02:** Request schema MUST include:

* `command` (string, required)
* `workdir` (string, optional)
* `timeout_ms` (integer, optional)

### 2.2 Behavior

**FR-PWSH-03:** The tool MUST execute PowerShell without invoking an interactive shell profile.

**FR-PWSH-04:** Output MUST be streamed and truncated to configured limits.

**FR-PWSH-05:** Timeout MUST terminate the process and return an error.

**FR-PWSH-06:** Execution MUST be approval-gated.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-PWSH-SEC-01 | Must sanitize environment variables |
| NFR-PWSH-SEC-02 | Must not inherit sensitive variables |

---

## 4. Configuration

```toml
[tools.pwsh]
default_timeout_ms = 300000
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-PWSH-01 | Command executes and returns output |
| T-PWSH-02 | Timeout terminates command |

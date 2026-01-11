# Build Tool

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
| 16-36 | Introduction: Purpose, Scope (Build tool) |
| 37-57 | Functional Requirements: Interface, Behavior, Timeouts |
| 58-88 | NFRs, Configuration, Verification |

---

## 0. Change Log

### 0.1 Initial draft

* Requirements for Build tool based on `../tools` build handler.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for a tool that runs a project build script in a controlled manner.

### 1.2 Scope

The Build tool will:

* Execute a platform-appropriate build script
* Stream output and enforce timeouts

Out of scope:

* Arbitrary shell command execution (handled by RunCommand/Pwsh)

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/build.rs` | Tool schema |
| `../tools/src/tools/handlers/script_runner.rs` | Handler behavior |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-BLD-01:** Tool name MUST be `Build` with alias `build`.

**FR-BLD-02:** Request schema MUST include:

* `command` (string, optional)
* `workdir` (string, optional)
* `timeout_ms` (integer, optional)

### 2.2 Behavior

**FR-BLD-03:** If `command` is omitted, the tool MUST run a default build script (platform-specific).

**FR-BLD-04:** Output MUST be streamed and truncated to configured limits.

**FR-BLD-05:** Tool MUST enforce timeout and return exit code.

**FR-BLD-06:** Mutating execution MUST be approval-gated.

---

## 3. Non-Functional Requirements

### 3.1 Security

| Requirement | Specification |
| --- | --- |
| NFR-BLD-SEC-01 | Must sanitize environment variables |
| NFR-BLD-SEC-02 | Must not execute via shell when possible |

---

## 4. Configuration

```toml
[tools.build]
default_timeout_ms = 300000
default_command_windows = "powershell -File build.ps1"
default_command_unix = "./scripts/build.sh"
```

---

## 5. Verification Requirements

### 5.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-BLD-01 | Default command selected per platform |
| T-BLD-02 | Timeout terminates process |

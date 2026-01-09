# Delete File Tool
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
* Requirements for delete tool based on `../tools` Delete handler.

---

## 1. Introduction

### 1.1 Purpose
Define requirements for a tool that deletes files within the sandbox.

### 1.2 Scope
The Delete tool will:
* Remove files (not directories) with validation
* Provide clear errors for missing files

Out of scope:
* Recursive directory deletion

### 1.3 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/delete.rs` | Tool schema |

---

## 2. Functional Requirements

### 2.1 Tool Interface
**FR-DEL-01:** Tool name MUST be `Delete` with alias `delete`.

**FR-DEL-02:** Request schema MUST include:
* `path` (string, required)

### 2.2 Behavior
**FR-DEL-03:** The tool MUST reject directory paths unless explicitly configured.

**FR-DEL-04:** The tool MUST return a clear error if the file does not exist.

**FR-DEL-05:** Successful deletion MUST return status and deleted path.

---

## 3. Non-Functional Requirements

### 3.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-DEL-SEC-01 | Must enforce sandboxed path access |
| NFR-DEL-SEC-02 | Mutating operation requires approval per policy |

---

## 4. Configuration

```toml
[tools.delete]
allow_delete_dirs = false
```

---

## 5. Verification Requirements

### 5.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-DEL-01 | Delete existing file |
| T-DEL-02 | Missing file returns error |
| T-DEL-03 | Directory deletion blocked |


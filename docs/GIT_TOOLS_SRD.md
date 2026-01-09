# Git Tools
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
* Requirements for GitStatus/GitDiff/GitRestore/GitAdd/GitCommit based on `../tools` git handlers.

---

## 1. Introduction

### 1.1 Purpose
Define requirements for Git-related tools that inspect and manipulate repo state.

### 1.2 Scope
Tools included:
* GitStatus
* GitDiff
* GitRestore
* GitAdd
* GitCommit

Out of scope:
* Git push/pull/fetch
* Branch management

### 1.3 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/git.rs` | Tool schema |
| `../tools/src/git/*` | Handler behavior |

---

## 2. Functional Requirements

### 2.1 Tool Interface
**FR-GIT-01:** Tools MUST be named `GitStatus`, `GitDiff`, `GitRestore`, `GitAdd`, `GitCommit`.

**FR-GIT-02:** Each tool MUST accept a `repo_path` (string, optional, default ".").

### 2.2 GitStatus
**FR-GIT-STATUS-01:** Return `git status --porcelain=v1` output and a human-readable summary.

### 2.3 GitDiff
**FR-GIT-DIFF-01:** Support:
* `staged` (boolean, optional)
* `pathspec` (array of strings, optional)
* `context` (integer, optional)

### 2.4 GitRestore
**FR-GIT-REST-01:** Support restoring:
* `paths` (array of strings, required)
* `staged` (boolean, optional)
* `worktree` (boolean, optional)

### 2.5 GitAdd
**FR-GIT-ADD-01:** Support `paths` (array, required) and `all` (boolean, optional).

### 2.6 GitCommit
**FR-GIT-COMMIT-01:** Support:
* `message` (string, required)
* `amend` (boolean, optional, default false)

### 2.7 Behavior
**FR-GIT-02a:** Tools MUST execute git via process invocation, not shell.

**FR-GIT-02b:** Errors MUST include stderr output when available.

**FR-GIT-02c:** Mutating tools (restore/add/commit) MUST be approval-gated.

---

## 3. Non-Functional Requirements

### 3.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-GIT-SEC-01 | Must enforce sandboxed repo path |
| NFR-GIT-SEC-02 | Must not execute via shell |

---

## 4. Configuration

```toml
[tools.git]
timeout_ms = 20000
```

---

## 5. Verification Requirements

### 5.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-GIT-01 | Status returns porcelain output |
| T-GIT-02 | Diff respects staged flag |
| T-GIT-03 | Restore reverts file |
| T-GIT-04 | Add stages files |
| T-GIT-05 | Commit creates commit |


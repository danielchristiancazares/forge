# Git Tools
## Software Requirements Document
**Version:** 1.1  
**Date:** 2026-01-09  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.2 Added missing tools
* Added GitLog, GitBranch, GitCheckout, GitStash, GitShow, GitBlame.
* Updated scope to reflect full tool set.

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
* GitLog
* GitBranch
* GitCheckout
* GitStash
* GitShow
* GitBlame

Out of scope:
* Git push/pull/fetch (network operations)

### 1.3 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/tools/git.rs` | Tool schema |
| `../tools/src/git/*` | Handler behavior |

---

## 2. Functional Requirements

### 2.1 Tool Interface
**FR-GIT-01:** Tools MUST be named: `GitStatus`, `GitDiff`, `GitRestore`, `GitAdd`, `GitCommit`, `GitLog`, `GitBranch`, `GitCheckout`, `GitStash`, `GitShow`, `GitBlame`.

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
**FR-GIT-COMMIT-01:** Support conventional commit format:
* `type` (string, required) — commit type (feat, fix, docs, etc.)
* `scope` (string, optional) — scope/area of change
* `message` (string, required) — commit description

**FR-GIT-COMMIT-02:** Commit message MUST be formatted as `type(scope): message` or `type: message`.

### 2.7 GitLog
**FR-GIT-LOG-01:** Support:
* `max_count` (integer, optional) — limit number of commits
* `oneline` (boolean, optional) — single-line format
* `format` (string, optional) — custom pretty format
* `author` (string, optional) — filter by author
* `since` (string, optional) — commits after date
* `until` (string, optional) — commits before date
* `grep` (string, optional) — filter by message pattern
* `path` (string, optional) — show commits affecting path
* `max_bytes` (integer, optional) — output size limit

### 2.8 GitBranch
**FR-GIT-BRANCH-01:** Support operations:
* `list_all` (boolean, optional) — list local and remote branches (`-a`)
* `list_remote` (boolean, optional) — list remote branches only (`-r`)
* `create` (string, optional) — create new branch with this name
* `delete` (string, optional) — delete branch (`-d`, must be merged)
* `force_delete` (string, optional) — force delete branch (`-D`)
* `rename` (string, optional) — rename branch (requires `new_name`)
* `new_name` (string, optional) — new name when renaming

**FR-GIT-BRANCH-02:** When listing, MUST include verbose output (`-v`).

### 2.9 GitCheckout
**FR-GIT-CHECKOUT-01:** Support:
* `branch` (string, optional) — switch to existing branch
* `create_branch` (string, optional) — create and switch to new branch (`-b`)
* `commit` (string, optional) — checkout specific commit (detached HEAD)
* `paths` (array of strings, optional) — restore files from HEAD

**FR-GIT-CHECKOUT-02:** At least one of branch, create_branch, commit, or paths MUST be provided.

### 2.10 GitStash
**FR-GIT-STASH-01:** Support `action` parameter with values:
* `push` (default) — save changes to stash
* `pop` — apply and remove top stash
* `apply` — apply stash without removing
* `drop` — remove stash entry
* `list` — list all stashes
* `show` — show stash contents with patch
* `clear` — remove all stashes

**FR-GIT-STASH-02:** Support additional parameters:
* `message` (string, optional) — stash message (for push)
* `index` (integer, optional) — stash index for pop/apply/drop/show
* `include_untracked` (boolean, optional) — include untracked files (`-u`)

### 2.11 GitShow
**FR-GIT-SHOW-01:** Support:
* `commit` (string, optional, default HEAD) — commit to show
* `stat` (boolean, optional) — show diffstat only
* `name_only` (boolean, optional) — show changed file names only
* `format` (string, optional) — custom pretty format
* `max_bytes` (integer, optional) — output size limit

### 2.12 GitBlame
**FR-GIT-BLAME-01:** Support:
* `path` (string, required) — file to blame
* `start_line` (integer, optional) — start of line range
* `end_line` (integer, optional) — end of line range
* `commit` (string, optional) — blame at specific commit
* `max_bytes` (integer, optional) — output size limit

### 2.14 Common Behavior
**FR-GIT-COMMON-01:** All tools MUST accept `working_dir` (string, optional) and `timeout_ms` (integer, optional).

**FR-GIT-COMMON-02:** Tools MUST execute git via process invocation, not shell.

**FR-GIT-COMMON-03:** Errors MUST include stderr output when available.

**FR-GIT-COMMON-04:** Mutating tools (restore/add/commit/checkout/stash/branch) MUST be approval-gated.

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
| T-GIT-STATUS-01 | Status returns porcelain output |
| T-GIT-DIFF-01 | Diff respects cached flag |
| T-GIT-DIFF-02 | Diff with from_ref/to_ref writes patches to output_dir |
| T-GIT-RESTORE-01 | Restore reverts file |
| T-GIT-ADD-01 | Add stages files |
| T-GIT-COMMIT-01 | Commit creates conventional commit message |
| T-GIT-LOG-01 | Log respects max_count |
| T-GIT-LOG-02 | Log respects author filter |
| T-GIT-BRANCH-01 | Branch lists with verbose output |
| T-GIT-BRANCH-02 | Branch create works |
| T-GIT-CHECKOUT-01 | Checkout switches branch |
| T-GIT-CHECKOUT-02 | Checkout with -b creates branch |
| T-GIT-STASH-01 | Stash push saves changes |
| T-GIT-STASH-02 | Stash pop restores changes |
| T-GIT-SHOW-01 | Show displays commit |
| T-GIT-BLAME-01 | Blame shows line authorship |
| T-GIT-BLAME-02 | Blame respects line range |


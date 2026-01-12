# Git Tools

## Software Requirements Document

**Version:** 2.2
**Date:** 2026-01-12
**Status:** Implementation Ready
**Baseline code reference:** `forge-source.zip`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-81 | Header, changelog (v2.2, v2.1, etc.) |
| 82-165 | Section 1: Introduction, scope, references, definitions, MCP relationship |
| 166-270 | Section 2.1-2.3: Module structure, ToolExecutor trait, shared types |
| 270-875 | Section 2.4-2.9: git_status, git_diff, git_restore, git_add, git_commit |
| 876-1565 | Section 2.10-2.15: git_log, git_branch, git_checkout, git_stash, git_show, git_blame |
| 1565-1823 | Section 2.16-2.19: Common behavior, output format, errors, validation |
| 1824-1907 | Section 3: NFRs - security, sandboxing, performance, reliability |
| 1908-2059 | Section 4: Configuration with TOML and Rust types, integration wiring |
| 2060-2191 | Section 5: Verification matrix (87 tests: functional, error, security, edge cases) |
| 2192-2774 | Section 6: Implementation guide, module skeleton, tool patterns, testing utilities |
| 2775-2820 | Appendices A-C: Tool reference, error messages, revision history |

---

## 0. Change Log

### 0.7 Naming standardization (v2.2)
* Standardized tool names to snake_case (`git_status`, `git_diff`, etc.).

### 0.6 Gap closure and alignment (v2.1)
* Added Section 1.6: Relationship to MCP Tools with coexistence model.
* Fixed Section 6.3: Removed references to non-existent `ToolError::ApprovalRequired`.
* Fixed Section 6.4: Added `process_group(0)` for Unix process termination.
* Fixed Section 6.4: Added proper environment sanitization via `EnvSanitizer`.
* Added Section 4.6: Configuration integration wiring with `ToolSettings`.
* Added git_commit pre-execution checks (staging area, user config validation).
* Fixed git_checkout.RestorePaths to require approval (consistency with git_restore).
* Added security tests T-GIT-SEC-05 through T-GIT-SEC-10.
* Added edge case tests T-GIT-EDGE-17 through T-GIT-EDGE-22.
* Updated test count from 75 to 87.

### 0.5 Implementation-ready revision (v2.0)
* Added Section 2.1: ToolExecutor trait implementations with Rust signatures.
* Added Section 2.2: Complete Rust type definitions for all argument structs.
* Added complete JSON schemas for each tool (Section 2.3-2.13).
* Added Section 4: Configuration integration with existing `config.rs` patterns.
* Added Section 6: Implementation guide with pseudocode and code examples.
* Expanded Section 5: Test matrix with 60+ test cases covering all edge cases.
* Added approval summary format specifications for each tool.
* Added risk level assignments per tool operation.
* Clarified sandbox integration points with `engine/src/tools/sandbox.rs`.
* Added symlink and junction safety requirements aligned with TOOL_EXECUTOR_SRD.
* Added process group termination requirements for timeout handling.
* Specified environment sanitization integration.

### 0.4 Comprehensive spec hardening
* Added Section 2.14: Output Format specification (JSON structure).
* Added Section 2.15: Error Handling specification.
* Added Section 2.16: Parameter Validation rules (mutual exclusivity, required combos).
* Added FR-GIT-COMMON-05: Argument sanitization requirement.
* Expanded NFR-GIT-SEC with argument injection, path traversal, symlink protections.
* Standardized `max_bytes` default (200000) across all tools.
* Clarified approval gating granularity (action-level for git_stash).
* Expanded test matrix with error cases, security tests, edge cases.

### 0.3 Align with implementation
* git_status: Added `porcelain`, `branch`, `untracked` parameters.
* git_diff: Fixed param names (`cached` not `staged`, `paths` not `pathspec`, `unified` not `context`). Added `name_only`, `stat`, `from_ref`, `to_ref`, `output_dir`, `max_bytes`.
* git_add: Added `update` parameter.

### 0.2 Added missing tools
* Added git_log, git_branch, git_checkout, git_stash, git_show, git_blame.
* Updated scope to reflect full tool set.

### 0.1 Initial draft
* Requirements for git_status/git_diff/git_restore/git_add/git_commit based on `../tools` git handlers.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for Git-related tools that inspect and manipulate repository state within a sandboxed working directory. This document provides implementation-ready specifications including Rust type definitions, JSON schemas, and integration patterns with the existing Tool Executor Framework.

### 1.2 Scope

**Tools included (11 total):**

| Tool | Category | Side Effects | Default Risk |
|------|----------|--------------|--------------|
| git_status | Read-only | No | Low |
| git_diff | Read-only | No | Low |
| git_log | Read-only | No | Low |
| git_show | Read-only | No | Low |
| git_blame | Read-only | No | Low |
| git_restore | Mutating | Yes (destructive) | High |
| git_add | Mutating | Yes | Medium |
| git_commit | Mutating | Yes | Medium |
| git_branch | Mixed | Yes (when mutating) | Medium |
| git_checkout | Mutating | Yes | Medium |
| git_stash | Mixed | Yes (action-dependent) | Medium/High |

**Out of scope:**
* Network operations: push, pull, fetch, clone, remote
* History rewriting: rebase, reset --hard, cherry-pick, amend
* Submodules, worktrees, sparse checkout
* Interactive operations (rebase -i, add -i, add -p)
* Merge conflict resolution

### 1.3 References

| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework (authoritative) |
| `engine/src/tools/mod.rs` | ToolExecutor trait, ToolCtx, ToolError |
| `engine/src/tools/builtins.rs` | Reference implementations (read_file, apply_patch) |
| `engine/src/tools/sandbox.rs` | Sandbox path validation |
| `engine/src/config.rs` | Configuration patterns |
| RFC 2119 / RFC 8174 | Requirement level keywords |

### 1.4 Definitions

| Term | Definition |
| --- | --- |
| **Ref** | Git reference (branch name, tag, commit SHA, HEAD, etc.) |
| **Porcelain** | Machine-parseable git output format |
| **Working tree** | Files in the repository directory (excluding `.git/`) |
| **Index/Staging area** | Git's staging area for the next commit |
| **Detached HEAD** | State where HEAD points to a commit, not a branch |
| **Stash** | Temporary storage for uncommitted changes |

### 1.5 Requirement Level Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in BCP 14 (RFC 2119 / RFC 8174) when, and only when, they appear in all capitals.

### 1.6 Relationship to MCP Tools

This SRD specifies **native Forge git tools** implemented as `ToolExecutor` implementations within the Forge codebase. These are distinct from MCP (Model Context Protocol) tools.

**Coexistence model:**

| Aspect | Native Forge Tools | MCP Tools |
|--------|-------------------|-----------|
| Naming | `git_status`, `git_diff`, etc. | `mcp__tools__git_status`, etc. |
| Implementation | Rust in `engine/src/tools/git/` | External MCP server |
| Sandbox | Integrated with Forge sandbox | MCP server's own policies |
| Approval | Forge approval workflow | MCP permission model |
| Configuration | `config.toml` `[tools.git]` | MCP server config |

**FR-GIT-MCP-01:** Native git tools SHALL use unprefixed names (`git_status`) to distinguish from MCP tools (`mcp__tools__git_status`).

**FR-GIT-MCP-02:** When both native and MCP git tools are available, the tool registry SHALL prefer native tools. MCP tools with colliding names (after prefix stripping) SHALL NOT be registered.

**FR-GIT-MCP-03:** Configuration MAY include `prefer_mcp = true` to reverse precedence, allowing MCP tools to override native implementations.

**Migration notes:**
* Systems currently using MCP git tools will continue to work unchanged.
* Native tools provide tighter sandbox integration and approval workflow.
* Future versions MAY deprecate MCP git tools in favor of native implementations.

---

## 2. Functional Requirements

### 2.1 Module Structure

**FR-GIT-MOD-01:** Git tools SHALL be implemented in a dedicated module:

```
engine/src/tools/
├── mod.rs           # Add: pub mod git;
├── git/
│   ├── mod.rs       # Git tool registration, shared utilities
│   ├── types.rs     # Argument structs, GitToolConfig
│   ├── status.rs    # git_status executor
│   ├── diff.rs      # git_diff executor
│   ├── restore.rs   # git_restore executor
│   ├── add.rs       # git_add executor
│   ├── commit.rs    # git_commit executor
│   ├── log.rs       # git_log executor
│   ├── branch.rs    # git_branch executor
│   ├── checkout.rs  # git_checkout executor
│   ├── stash.rs     # git_stash executor
│   ├── show.rs      # git_show executor
│   └── blame.rs     # git_blame executor
```

**FR-GIT-MOD-02:** Registration function signature:

```rust
pub fn register_git_tools(
    registry: &mut ToolRegistry,
    config: GitToolConfig,
) -> Result<(), ToolError>;
```

### 2.2 ToolExecutor Trait Implementation

**FR-GIT-TRAIT-01:** Each git tool MUST implement the `ToolExecutor` trait per `engine/src/tools/mod.rs`:

```rust
pub trait ToolExecutor: Send + Sync + std::panic::UnwindSafe {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn is_side_effecting(&self) -> bool;
    fn requires_approval(&self) -> bool { false }
    fn risk_level(&self) -> RiskLevel;
    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError>;
    fn timeout(&self) -> Option<Duration> { None }
    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a>;
}
```

**FR-GIT-TRAIT-02:** Tool implementations overview:

| Tool | `is_side_effecting()` | `requires_approval()` | `risk_level()` |
|------|----------------------|----------------------|----------------|
| git_status | `false` | `false` | `Low` |
| git_diff | `false` | `false` | `Low` |
| git_log | `false` | `false` | `Low` |
| git_show | `false` | `false` | `Low` |
| git_blame | `false` | `false` | `Low` |
| git_restore | `true` | `true` | `High` |
| git_add | `true` | `true` | `Medium` |
| git_commit | `true` | `true` | `Medium` |
| git_branch | dynamic* | dynamic* | `Medium` |
| git_checkout | dynamic* | dynamic* | `Medium` |
| git_stash | dynamic* | dynamic* | dynamic* |

*Dynamic based on operation (see individual tool specs).

### 2.3 Shared Rust Types

**FR-GIT-TYPES-01:** Common argument types:

```rust
use serde::Deserialize;
use std::path::PathBuf;

/// Common timeout parameter (all tools).
pub const DEFAULT_GIT_TIMEOUT_MS: u64 = 30_000;

/// Common max_bytes parameter (tools with large output).
pub const DEFAULT_GIT_MAX_BYTES: usize = 200_000;
pub const MAX_GIT_MAX_BYTES: usize = 5_000_000;

/// Git tool configuration from config.toml.
#[derive(Debug, Clone)]
pub struct GitToolConfig {
    pub enabled: bool,
    pub timeout_ms: u64,
    pub max_bytes: usize,
}

impl Default for GitToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: DEFAULT_GIT_TIMEOUT_MS,
            max_bytes: DEFAULT_GIT_MAX_BYTES,
        }
    }
}
```

### 2.4 Tool Interface

**FR-GIT-01:** Tools MUST be named exactly: `git_status`, `git_diff`, `git_restore`, `git_add`, `git_commit`, `git_log`, `git_branch`, `git_checkout`, `git_stash`, `git_show`, `git_blame`.

### 2.5 git_status

**FR-GIT-STATUS-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitStatusArgs {
    /// Use porcelain output (`--porcelain=1`). Default: true.
    #[serde(default = "default_true")]
    pub porcelain: bool,
    
    /// Include branch info (`-b`). Default: true.
    #[serde(default = "default_true")]
    pub branch: bool,
    
    /// Include untracked files. When false, uses `-uno`. Default: true.
    #[serde(default = "default_true")]
    pub untracked: bool,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}

fn default_true() -> bool { true }
fn default_timeout() -> u64 { DEFAULT_GIT_TIMEOUT_MS }
```

**FR-GIT-STATUS-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "porcelain": {
      "type": "boolean",
      "default": true,
      "description": "Use porcelain output (--porcelain=1)"
    },
    "branch": {
      "type": "boolean",
      "default": true,
      "description": "Include branch info (-b)"
    },
    "untracked": {
      "type": "boolean",
      "default": true,
      "description": "Include untracked files (when false, uses -uno)"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-STATUS-03:** Command construction:

```rust
fn build_command(args: &GitStatusArgs) -> Vec<&str> {
    let mut cmd = vec!["git", "status"];
    if args.porcelain {
        cmd.push("--porcelain=1");
        if args.branch {
            cmd.push("-b");
        }
    }
    if !args.untracked {
        cmd.push("-uno");
    }
    cmd
}
```

**FR-GIT-STATUS-04:** When `porcelain=false`, output MUST be the human-readable `git status` format.

**FR-GIT-STATUS-05:** Approval summary format: `"Show git status"` (read-only, no approval needed).

### 2.6 git_diff

**FR-GIT-DIFF-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitDiffArgs {
    /// Diff staged changes (`--cached`). Default: false.
    #[serde(default)]
    pub cached: bool,
    
    /// Show only changed file names (`--name-only`). Default: false.
    #[serde(default)]
    pub name_only: bool,
    
    /// Show diffstat only (`--stat`). Default: false.
    #[serde(default)]
    pub stat: bool,
    
    /// Number of context lines (`-U<N>`).
    pub unified: Option<u32>,
    
    /// Paths to diff (passed after `--`).
    #[serde(default)]
    pub paths: Vec<String>,
    
    /// Starting ref for ref-to-ref comparison.
    pub from_ref: Option<String>,
    
    /// Ending ref for ref-to-ref comparison.
    pub to_ref: Option<String>,
    
    /// Directory to write per-file patches (requires from_ref AND to_ref).
    pub output_dir: Option<String>,
    
    /// Maximum output bytes before truncation. Default: 200000, max: 5000000.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}

fn default_max_bytes() -> usize { DEFAULT_GIT_MAX_BYTES }
```

**FR-GIT-DIFF-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "cached": {
      "type": "boolean",
      "default": false,
      "description": "Diff staged changes (--cached)"
    },
    "name_only": {
      "type": "boolean",
      "default": false,
      "description": "Show only changed file names (--name-only)"
    },
    "stat": {
      "type": "boolean",
      "default": false,
      "description": "Show diffstat only (--stat)"
    },
    "unified": {
      "type": "integer",
      "minimum": 0,
      "description": "Number of context lines (-U<N>)"
    },
    "paths": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Paths to diff (passed after --)"
    },
    "from_ref": {
      "type": "string",
      "description": "Starting ref for ref-to-ref comparison"
    },
    "to_ref": {
      "type": "string",
      "description": "Ending ref for ref-to-ref comparison"
    },
    "output_dir": {
      "type": "string",
      "description": "Directory to write per-file patches (requires from_ref AND to_ref)"
    },
    "max_bytes": {
      "type": "integer",
      "minimum": 1,
      "maximum": 5000000,
      "default": 200000,
      "description": "Maximum output bytes before truncation"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-DIFF-03:** Parameter validation (pre-execution):

```rust
fn validate_args(args: &GitDiffArgs) -> Result<(), ToolError> {
    // Mutual exclusivity: cached vs from_ref/to_ref
    if args.cached && (args.from_ref.is_some() || args.to_ref.is_some()) {
        return Err(ToolError::BadArgs {
            message: "cached cannot be used with from_ref/to_ref".to_string(),
        });
    }
    
    // output_dir requires both refs
    if args.output_dir.is_some() && (args.from_ref.is_none() || args.to_ref.is_none()) {
        return Err(ToolError::BadArgs {
            message: "output_dir requires both from_ref and to_ref".to_string(),
        });
    }
    
    // max_bytes within bounds
    if args.max_bytes > MAX_GIT_MAX_BYTES {
        return Err(ToolError::BadArgs {
            message: format!("max_bytes cannot exceed {}", MAX_GIT_MAX_BYTES),
        });
    }
    
    Ok(())
}
```

**FR-GIT-DIFF-04:** `name_only` and `stat` precedence: if both true, `name_only` takes precedence.

**FR-GIT-DIFF-05:** When `output_dir` is set:
* Directory MUST be created if it does not exist (within sandbox via `ctx.sandbox.resolve_path()`).
* Per-file patches MUST be named `<sanitized-path>.patch` where path separators become `__`.
* Tool MUST return JSON with `"patches": ["file1.patch", "file2.patch"]` in output.

**FR-GIT-DIFF-06:** Approval summary format: `"Show diff [cached] [from_ref..to_ref]"` (read-only, no approval needed).

### 2.7 git_restore

**FR-GIT-REST-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitRestoreArgs {
    /// Files to restore. REQUIRED, minimum 1 element.
    pub paths: Vec<String>,
    
    /// Restore the index/staging area (`--staged`). Default: false.
    #[serde(default)]
    pub staged: bool,
    
    /// Restore the working tree (`--worktree`). Default: true.
    #[serde(default = "default_true")]
    pub worktree: bool,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-REST-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "paths": {
      "type": "array",
      "items": { "type": "string" },
      "minItems": 1,
      "description": "Files to restore (passed after --)"
    },
    "staged": {
      "type": "boolean",
      "default": false,
      "description": "Restore the index/staging area (--staged)"
    },
    "worktree": {
      "type": "boolean",
      "default": true,
      "description": "Restore the working tree (--worktree)"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "required": ["paths"],
  "additionalProperties": false
}
```

**FR-GIT-REST-03:** Parameter validation:

```rust
fn validate_args(args: &GitRestoreArgs) -> Result<(), ToolError> {
    if args.paths.is_empty() {
        return Err(ToolError::BadArgs {
            message: "paths must contain at least one element".to_string(),
        });
    }
    
    if !args.staged && !args.worktree {
        return Err(ToolError::BadArgs {
            message: "At least one of staged or worktree must be true".to_string(),
        });
    }
    
    Ok(())
}
```

**FR-GIT-REST-04:** If a path does not exist in the restore source (HEAD or index), git will error; this error MUST be propagated.

**FR-GIT-REST-05:** Approval summary format:
* `"Restore <n> file(s) to worktree"` (worktree only)
* `"Unstage <n> file(s)"` (staged only)
* `"Restore and unstage <n> file(s)"` (both)

**FR-GIT-REST-06:** Risk level: `High` (destructive, discards uncommitted changes).

### 2.8 git_add

**FR-GIT-ADD-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitAddArgs {
    /// Files to stage.
    #[serde(default)]
    pub paths: Vec<String>,
    
    /// Stage all changes (`-A`). Default: false.
    #[serde(default)]
    pub all: bool,
    
    /// Stage modified/deleted only (`-u`). Default: false.
    #[serde(default)]
    pub update: bool,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-ADD-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "paths": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Files to stage"
    },
    "all": {
      "type": "boolean",
      "default": false,
      "description": "Stage all changes (-A)"
    },
    "update": {
      "type": "boolean",
      "default": false,
      "description": "Stage modified/deleted only (-u)"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-ADD-03:** Parameter validation:

```rust
fn validate_args(args: &GitAddArgs) -> Result<(), ToolError> {
    // At least one action required
    if args.paths.is_empty() && !args.all && !args.update {
        return Err(ToolError::BadArgs {
            message: "At least one of paths, all, or update must be specified".to_string(),
        });
    }
    
    // Mutual exclusivity
    if args.all && args.update {
        return Err(ToolError::BadArgs {
            message: "all and update are mutually exclusive".to_string(),
        });
    }
    
    Ok(())
}
```

**FR-GIT-ADD-04:** If `paths` is provided with `all=true`, `all` takes precedence and `paths` is ignored.

**FR-GIT-ADD-05:** Approval summary format:
* `"Stage all changes"` (all=true)
* `"Stage modified/deleted files"` (update=true)
* `"Stage <n> file(s): <file1>, <file2>..."` (paths)

**FR-GIT-ADD-06:** Risk level: `Medium`.

### 2.9 git_commit

**FR-GIT-COMMIT-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitCommitArgs {
    /// Commit type (feat, fix, docs, style, refactor, test, chore, etc.). REQUIRED.
    #[serde(rename = "type")]
    pub commit_type: String,
    
    /// Scope/area of change.
    pub scope: Option<String>,
    
    /// Commit description. REQUIRED.
    pub message: String,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-COMMIT-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "type": {
      "type": "string",
      "pattern": "^[a-z]+$",
      "description": "Commit type: feat, fix, docs, style, refactor, test, chore, etc."
    },
    "scope": {
      "type": "string",
      "pattern": "^[a-z0-9_-]+$",
      "description": "Scope/area of change"
    },
    "message": {
      "type": "string",
      "minLength": 1,
      "description": "Commit description"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "required": ["type", "message"],
  "additionalProperties": false
}
```

**FR-GIT-COMMIT-03:** Parameter validation:

```rust
fn validate_args(args: &GitCommitArgs) -> Result<(), ToolError> {
    // Type pattern
    let type_re = regex::Regex::new(r"^[a-z]+$").unwrap();
    if !type_re.is_match(&args.commit_type) {
        return Err(ToolError::BadArgs {
            message: "type must be lowercase letters only (e.g., feat, fix, docs)".to_string(),
        });
    }
    
    // Scope pattern (if provided)
    if let Some(scope) = &args.scope {
        let scope_re = regex::Regex::new(r"^[a-z0-9_-]+$").unwrap();
        if !scope_re.is_match(scope) {
            return Err(ToolError::BadArgs {
                message: "scope must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            });
        }
    }
    
    // Message not empty
    if args.message.trim().is_empty() {
        return Err(ToolError::BadArgs {
            message: "message must not be empty".to_string(),
        });
    }
    
    Ok(())
}
```

**FR-GIT-COMMIT-04:** Commit message MUST be formatted as `type(scope): message` or `type: message` when scope is omitted.

**FR-GIT-COMMIT-05:** Pre-execution checks:
* If staging area is empty, return error with message `"nothing to commit"`.
* If git user.name or user.email is not configured, return error with message `"Git user.name or user.email not configured. Run: git config --global user.name 'Your Name' && git config --global user.email 'you@example.com'"`.

```rust
/// Pre-execution validation for git_commit.
async fn validate_commit_preconditions(
    working_dir: &Path,
    timeout: Duration,
    ctx: &ToolCtx,
) -> Result<(), ToolError> {
    // Check if staging area has changes
    // git diff --cached --quiet returns exit 0 if no staged changes
    let status_result = execute_git_raw(
        &["diff", "--cached", "--quiet"],
        working_dir,
        timeout,
        ctx,
    ).await;

    match status_result {
        Ok(output) if output.exit_code == 0 => {
            // Exit 0 means no staged changes
            return Err(ToolError::ExecutionFailed {
                tool: "git_commit".to_string(),
                message: "nothing to commit".to_string(),
            });
        }
        Ok(_) => { /* Exit non-zero means there ARE staged changes - continue */ }
        Err(e) => return Err(e),
    }

    // Check user.name is configured
    let name_result = execute_git_raw(
        &["config", "user.name"],
        working_dir,
        timeout,
        ctx,
    ).await;

    let name_ok = name_result
        .map(|o| o.exit_code == 0 && !o.stdout.trim().is_empty())
        .unwrap_or(false);

    // Check user.email is configured
    let email_result = execute_git_raw(
        &["config", "user.email"],
        working_dir,
        timeout,
        ctx,
    ).await;

    let email_ok = email_result
        .map(|o| o.exit_code == 0 && !o.stdout.trim().is_empty())
        .unwrap_or(false);

    if !name_ok || !email_ok {
        return Err(ToolError::ExecutionFailed {
            tool: "git_commit".to_string(),
            message: "Git user.name or user.email not configured. Run: \
                      git config --global user.name 'Your Name' && \
                      git config --global user.email 'you@example.com'".to_string(),
        });
    }

    Ok(())
}
```

**FR-GIT-COMMIT-06:** Approval summary format: `"Commit: <type>(<scope>): <message truncated to 50 chars>"`

**FR-GIT-COMMIT-07:** Risk level: `Medium`.

### 2.10 git_log

**FR-GIT-LOG-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitLogArgs {
    /// Limit number of commits.
    pub max_count: Option<u32>,
    
    /// Single-line format (`--oneline`). Default: false.
    #[serde(default)]
    pub oneline: bool,
    
    /// Custom pretty format (`--format=<string>`).
    pub format: Option<String>,
    
    /// Filter by author (`--author=<pattern>`).
    pub author: Option<String>,
    
    /// Commits after date (`--since=<date>`).
    pub since: Option<String>,
    
    /// Commits before date (`--until=<date>`).
    pub until: Option<String>,
    
    /// Filter by message pattern (`--grep=<pattern>`).
    pub grep: Option<String>,
    
    /// Show commits affecting path.
    pub path: Option<String>,
    
    /// Maximum output bytes. Default: 200000, max: 5000000.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-LOG-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "max_count": {
      "type": "integer",
      "minimum": 1,
      "description": "Limit number of commits"
    },
    "oneline": {
      "type": "boolean",
      "default": false,
      "description": "Single-line format (--oneline)"
    },
    "format": {
      "type": "string",
      "description": "Custom pretty format (--format=<string>)"
    },
    "author": {
      "type": "string",
      "description": "Filter by author (--author=<pattern>)"
    },
    "since": {
      "type": "string",
      "description": "Commits after date (--since=<date>)"
    },
    "until": {
      "type": "string",
      "description": "Commits before date (--until=<date>)"
    },
    "grep": {
      "type": "string",
      "description": "Filter by message pattern (--grep=<pattern>)"
    },
    "path": {
      "type": "string",
      "description": "Show commits affecting path"
    },
    "max_bytes": {
      "type": "integer",
      "minimum": 1,
      "maximum": 5000000,
      "default": 200000,
      "description": "Maximum output bytes"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-LOG-03:** `oneline` and `format` precedence: if both provided, `format` takes precedence.

**FR-GIT-LOG-04:** Date formats for `since`/`until` accept any format git accepts (ISO 8601, relative like "2 weeks ago", etc.).

**FR-GIT-LOG-05:** Approval summary format: `"Show git log"` (read-only, no approval needed).

**FR-GIT-LOG-06:** Output truncation: if output exceeds `max_bytes`, include `"truncated": true` in response.

### 2.11 git_branch

**FR-GIT-BRANCH-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitBranchArgs {
    /// List local and remote branches (`-a`). Default: false.
    #[serde(default)]
    pub list_all: bool,
    
    /// List remote branches only (`-r`). Default: false.
    #[serde(default)]
    pub list_remote: bool,
    
    /// Create new branch with this name.
    pub create: Option<String>,
    
    /// Delete branch (`-d`, must be merged).
    pub delete: Option<String>,
    
    /// Force delete branch (`-D`).
    pub force_delete: Option<String>,
    
    /// Rename this branch (requires `new_name`).
    pub rename: Option<String>,
    
    /// New name when renaming a branch.
    pub new_name: Option<String>,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}

/// Determine which operation to perform based on precedence.
impl GitBranchArgs {
    pub fn operation(&self) -> git_branchOp {
        if self.rename.is_some() { return git_branchOp::Rename; }
        if self.force_delete.is_some() { return git_branchOp::ForceDelete; }
        if self.delete.is_some() { return git_branchOp::Delete; }
        if self.create.is_some() { return git_branchOp::Create; }
        git_branchOp::List
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum git_branchOp {
    List,
    Create,
    Delete,
    ForceDelete,
    Rename,
}
```

**FR-GIT-BRANCH-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "list_all": {
      "type": "boolean",
      "default": false,
      "description": "List both local and remote branches (-a)"
    },
    "list_remote": {
      "type": "boolean",
      "default": false,
      "description": "List only remote branches (-r)"
    },
    "create": {
      "type": "string",
      "description": "Create a new branch with this name"
    },
    "delete": {
      "type": "string",
      "description": "Delete this branch (-d, must be merged)"
    },
    "force_delete": {
      "type": "string",
      "description": "Force delete this branch (-D)"
    },
    "rename": {
      "type": "string",
      "description": "Rename this branch (requires new_name)"
    },
    "new_name": {
      "type": "string",
      "description": "New name when renaming a branch"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-BRANCH-03:** Parameter validation:

```rust
fn validate_args(args: &GitBranchArgs) -> Result<(), ToolError> {
    // rename requires new_name
    if args.rename.is_some() && args.new_name.is_none() {
        return Err(ToolError::BadArgs {
            message: "rename requires new_name".to_string(),
        });
    }
    Ok(())
}
```

**FR-GIT-BRANCH-04:** Operation precedence (first match wins):
1. `rename` (requires `new_name`, error if missing)
2. `force_delete`
3. `delete`
4. `create`
5. List (default) — includes `-v` for verbose output

**FR-GIT-BRANCH-05:** Dynamic side effects and approval:
* `List`: `is_side_effecting=false`, no approval
* `Create/Delete/ForceDelete/Rename`: `is_side_effecting=true`, approval required

**FR-GIT-BRANCH-06:** Approval summary formats:
* `"List branches"`
* `"Create branch '<name>'"`
* `"Delete branch '<name>'"`
* `"Force delete branch '<name>'"`
* `"Rename branch '<old>' to '<new>'"`

**FR-GIT-BRANCH-07:** Git errors propagated:
* Deleting current branch → git error
* Creating existing branch → git error

### 2.12 git_checkout

**FR-GIT-CHECKOUT-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitCheckoutArgs {
    /// Switch to existing branch.
    pub branch: Option<String>,
    
    /// Create and switch to new branch (`-b`).
    pub create_branch: Option<String>,
    
    /// Checkout specific commit (detached HEAD).
    pub commit: Option<String>,
    
    /// Restore files from HEAD or specified commit.
    #[serde(default)]
    pub paths: Vec<String>,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}

impl GitCheckoutArgs {
    pub fn operation(&self) -> git_checkoutOp {
        if self.create_branch.is_some() { return git_checkoutOp::CreateBranch; }
        if self.branch.is_some() { return git_checkoutOp::SwitchBranch; }
        if self.commit.is_some() { return git_checkoutOp::Commit; }
        if !self.paths.is_empty() { return git_checkoutOp::RestorePaths; }
        git_checkoutOp::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum git_checkoutOp {
    None,
    CreateBranch,
    SwitchBranch,
    Commit,
    RestorePaths,
}
```

**FR-GIT-CHECKOUT-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "branch": {
      "type": "string",
      "description": "Branch to switch to"
    },
    "create_branch": {
      "type": "string",
      "description": "Create and switch to a new branch (-b)"
    },
    "commit": {
      "type": "string",
      "description": "Checkout a specific commit (detached HEAD)"
    },
    "paths": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Restore these paths from HEAD or specified commit"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-CHECKOUT-03:** Parameter validation:

```rust
fn validate_args(args: &GitCheckoutArgs) -> Result<(), ToolError> {
    if args.operation() == git_checkoutOp::None {
        return Err(ToolError::BadArgs {
            message: "At least one of branch, create_branch, commit, or paths must be specified".to_string(),
        });
    }
    Ok(())
}
```

**FR-GIT-CHECKOUT-04:** Operation precedence (first match wins):
1. `create_branch` — create and switch (`git checkout -b <name>`)
2. `branch` — switch to existing (`git checkout <branch>`)
3. `commit` — checkout commit (detached HEAD) (`git checkout <commit>`)
4. `paths` — restore files only (does not switch branch) (`git checkout -- <paths>`)

**FR-GIT-CHECKOUT-05:** When checking out `commit`, output MUST include warning about detached HEAD state (git provides this).

**FR-GIT-CHECKOUT-06:** Dynamic side effects and approval:
* `RestorePaths`: `is_side_effecting=true`, approval required (destructive, discards uncommitted changes — consistent with git_restore)
* `CreateBranch/SwitchBranch/Commit`: `is_side_effecting=true`, approval required

**FR-GIT-CHECKOUT-07:** Approval summary formats:
* `"Create and switch to branch '<name>'"`
* `"Switch to branch '<name>'"`
* `"Checkout commit '<sha>'"`
* `"Restore <n> file(s) from HEAD"`

**FR-GIT-CHECKOUT-08:** If checkout would overwrite uncommitted changes, git will error; this error MUST be propagated (no implicit `--force`).

### 2.13 git_stash

**FR-GIT-STASH-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitStashArgs {
    /// Stash action to perform. Default: "push".
    #[serde(default = "default_action")]
    pub action: StashAction,
    
    /// Stash message (for push only).
    pub message: Option<String>,
    
    /// Stash index for pop/apply/drop/show. Default: 0.
    pub index: Option<u32>,
    
    /// Include untracked files (`-u`, for push only). Default: false.
    #[serde(default)]
    pub include_untracked: bool,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}

fn default_action() -> StashAction { StashAction::Push }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StashAction {
    Push,
    Pop,
    Apply,
    Drop,
    List,
    Show,
    Clear,
}
```

**FR-GIT-STASH-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "action": {
      "type": "string",
      "enum": ["push", "pop", "apply", "drop", "list", "show", "clear"],
      "default": "push",
      "description": "Stash action to perform"
    },
    "message": {
      "type": "string",
      "description": "Message for the stash (with push)"
    },
    "index": {
      "type": "integer",
      "minimum": 0,
      "description": "Stash index for pop/apply/drop/show"
    },
    "include_untracked": {
      "type": "boolean",
      "default": false,
      "description": "Include untracked files (with push)"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-STASH-03:** Action-specific behavior:

| Action | Side Effects | Approval | Risk Level |
|--------|--------------|----------|------------|
| `push` | Yes | Yes | Medium |
| `pop` | Yes | Yes | Medium |
| `apply` | Yes | Yes | Medium |
| `drop` | Yes | Yes | Medium |
| `list` | No | No | Low |
| `show` | No | No | Low |
| `clear` | Yes (destructive) | Yes (explicit warning) | High |

**FR-GIT-STASH-04:** Approval summary formats:
* `"Stash changes"` or `"Stash changes: <message>"`
* `"Pop stash@{<n>}"`
* `"Apply stash@{<n>}"`
* `"Drop stash@{<n>}"`
* `"List stashes"` (no approval)
* `"Show stash@{<n>}"` (no approval)
* `"Clear ALL stashes (WARNING: destructive)"`

**FR-GIT-STASH-05:** Error conditions:
* `pop`/`apply`/`drop`/`show` with `index` on empty stash list → error
* `push` with nothing to stash → return message `"No local changes to save"`

### 2.14 git_show

**FR-GIT-SHOW-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitShowArgs {
    /// Commit to show. Default: "HEAD".
    pub commit: Option<String>,
    
    /// Show diffstat only (`--stat`). Default: false.
    #[serde(default)]
    pub stat: bool,
    
    /// Show changed file names only (`--name-only`). Default: false.
    #[serde(default)]
    pub name_only: bool,
    
    /// Custom pretty format for commit info.
    pub format: Option<String>,
    
    /// Maximum output bytes. Default: 200000, max: 5000000.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-SHOW-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "commit": {
      "type": "string",
      "default": "HEAD",
      "description": "Commit to show"
    },
    "stat": {
      "type": "boolean",
      "default": false,
      "description": "Show diffstat only (--stat)"
    },
    "name_only": {
      "type": "boolean",
      "default": false,
      "description": "Show only names of changed files (--name-only)"
    },
    "format": {
      "type": "string",
      "description": "Pretty-print format for commit info"
    },
    "max_bytes": {
      "type": "integer",
      "minimum": 1,
      "maximum": 5000000,
      "default": 200000,
      "description": "Maximum output bytes"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "additionalProperties": false
}
```

**FR-GIT-SHOW-03:** `stat` and `name_only` are mutually exclusive; if both true, `name_only` takes precedence.

**FR-GIT-SHOW-04:** If `commit` does not exist, return error from git.

**FR-GIT-SHOW-05:** Approval summary format: `"Show commit <sha>"` (read-only, no approval needed).

### 2.15 git_blame

**FR-GIT-BLAME-01:** Rust argument struct:

```rust
#[derive(Debug, Deserialize)]
pub struct GitBlameArgs {
    /// File path to blame. REQUIRED.
    pub path: String,
    
    /// Start line number for range (1-indexed).
    pub start_line: Option<u32>,
    
    /// End line number for range (1-indexed).
    pub end_line: Option<u32>,
    
    /// Blame at specific commit instead of HEAD.
    pub commit: Option<String>,
    
    /// Maximum output bytes. Default: 200000, max: 5000000.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    
    /// Timeout in milliseconds. Default: 30000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    
    /// Working directory for git command.
    pub working_dir: Option<String>,
}
```

**FR-GIT-BLAME-02:** JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "File path to blame"
    },
    "start_line": {
      "type": "integer",
      "minimum": 1,
      "description": "Start line number for range"
    },
    "end_line": {
      "type": "integer",
      "minimum": 1,
      "description": "End line number for range"
    },
    "commit": {
      "type": "string",
      "description": "Blame at specific commit instead of HEAD"
    },
    "max_bytes": {
      "type": "integer",
      "minimum": 1,
      "maximum": 5000000,
      "default": 200000,
      "description": "Maximum output bytes"
    },
    "timeout_ms": {
      "type": "integer",
      "minimum": 100,
      "default": 30000,
      "description": "Timeout in milliseconds"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory for git command"
    }
  },
  "required": ["path"],
  "additionalProperties": false
}
```

**FR-GIT-BLAME-03:** Parameter validation:

```rust
fn validate_args(args: &GitBlameArgs) -> Result<(), ToolError> {
    if args.path.trim().is_empty() {
        return Err(ToolError::BadArgs {
            message: "path must not be empty".to_string(),
        });
    }
    
    if let (Some(start), Some(end)) = (args.start_line, args.end_line) {
        if start > end {
            return Err(ToolError::BadArgs {
                message: "start_line must be <= end_line".to_string(),
            });
        }
    }
    
    Ok(())
}
```

**FR-GIT-BLAME-04:** Line range behavior:
* If only `start_line` is provided → blame from that line to EOF (`-L <start>,`)
* If only `end_line` is provided → blame from line 1 to `end_line` (`-L 1,<end>`)
* If both provided → blame range (`-L <start>,<end>`)

**FR-GIT-BLAME-05:** Error conditions:
* File does not exist at specified commit → return error from git
* Binary files → git will fail; propagate error

**FR-GIT-BLAME-06:** Approval summary format: `"Blame <path> [lines <start>-<end>]"` (read-only, no approval needed).

### 2.16 Common Behavior

**FR-GIT-COMMON-01:** All tools MUST accept common parameters:

```rust
/// Common parameters embedded in all git tool argument structs.
pub struct CommonGitParams {
    /// Directory to run git in. Default: ctx.working_dir (sandbox root).
    pub working_dir: Option<String>,
    
    /// Maximum execution time in milliseconds. Default: from config (30000).
    pub timeout_ms: u64,
}
```

**FR-GIT-COMMON-02:** Git process execution:

```rust
async fn execute_git(
    args: &[&str],
    working_dir: &Path,
    timeout: Duration,
    ctx: &mut ToolCtx,
) -> Result<GitOutput, ToolError> {
    // Build command WITHOUT shell
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
       .current_dir(working_dir)
       .stdin(std::process::Stdio::null())
       .stdout(std::process::Stdio::piped())
       .stderr(std::process::Stdio::piped());
    
    // Sanitize environment
    let env: Vec<(String, String)> = std::env::vars().collect();
    let sanitized = ctx.env_sanitizer.sanitize_env(&env);
    cmd.env_clear();
    cmd.envs(sanitized);
    
    // Execute with timeout...
}
```

**FR-GIT-COMMON-03:** Tools MUST NOT execute via shell (no `sh -c`, no `cmd /c`). Direct process invocation only.

**FR-GIT-COMMON-04:** Tool output structure (internal, before framework processing):

```rust
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

**FR-GIT-COMMON-05:** Approval requirements summary:

| Tool | Requires Approval | Condition |
|------|------------------|-----------|
| git_status | No | — |
| git_diff | No | — |
| git_log | No | — |
| git_show | No | — |
| git_blame | No | — |
| git_restore | **Yes** | Always (destructive) |
| git_add | **Yes** | Always |
| git_commit | **Yes** | Always |
| git_checkout | **Yes** | When switching/creating branches |
| git_checkout | No | Path-only restore |
| git_stash (list, show) | No | Read-only actions |
| git_stash (push, pop, apply, drop) | **Yes** | Mutating actions |
| git_stash (clear) | **Yes** | With explicit warning |
| git_branch (list) | No | Read-only |
| git_branch (create, delete, rename) | **Yes** | Mutating actions |

**FR-GIT-COMMON-06:** Argument sanitization (CRITICAL for security):

```rust
/// Sanitize a git argument that could be a ref, branch name, or path.
/// Arguments starting with `-` could be interpreted as flags.
fn sanitize_git_arg(arg: &str) -> Vec<&str> {
    if arg.starts_with('-') {
        // Use -- separator before the argument
        vec!["--", arg]
    } else {
        vec![arg]
    }
}

/// For paths array, always use -- separator before paths.
fn build_paths_args(paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return vec![];
    }
    let mut args = vec!["--".to_string()];
    args.extend(paths.iter().cloned());
    args
}
```

**FR-GIT-COMMON-07:** Working directory validation:

```rust
fn validate_working_dir(
    working_dir: &Option<String>,
    ctx: &ToolCtx,
) -> Result<PathBuf, ToolError> {
    let dir = match working_dir {
        Some(wd) => ctx.sandbox.resolve_path(wd, &ctx.working_dir)?,
        None => ctx.working_dir.clone(),
    };
    
    // Verify it's a git repository
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        return Err(ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: format!("Not a git repository: {}", dir.display()),
        });
    }
    
    Ok(dir)
}
```

### 2.17 Output Format

**FR-GIT-OUTPUT-01:** Tool executor returns plain string; framework wraps into `ToolResult`. The string content MUST be the git stdout for successful operations.

**FR-GIT-OUTPUT-02:** For tools with `max_bytes`, output truncation:

```rust
fn truncate_git_output(output: String, max_bytes: usize) -> (String, bool) {
    if output.len() <= max_bytes {
        return (output, false);
    }
    
    let marker = "\n\n... [output truncated]";
    let max_body = max_bytes.saturating_sub(marker.len());
    
    // Find valid UTF-8 boundary
    let mut end = max_body;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    
    let mut truncated = output;
    truncated.truncate(end);
    truncated.push_str(marker);
    (truncated, true)
}
```

**FR-GIT-OUTPUT-03:** Tool-specific output formats:

| Tool | Output Format |
|------|---------------|
| git_status | Porcelain or human-readable per `porcelain` flag |
| git_diff | Diff output, or `"patches": [...]` when `output_dir` set |
| git_log | Log entries, truncated if exceeds `max_bytes` |
| git_show | Commit details + diff, truncated if exceeds `max_bytes` |
| git_blame | Blame output, truncated if exceeds `max_bytes` |
| git_restore | `"Restored <n> file(s)"` |
| git_add | `"Staged <n> file(s)"` |
| git_commit | `"[<branch> <sha>] <message>"` (git output) |
| git_branch | Branch list or operation result |
| git_checkout | Switch result or `"Restored <n> file(s)"` |
| git_stash | Stash operation result |

**FR-GIT-OUTPUT-04:** Stderr handling:
* Non-empty stderr MUST be appended to output: `"\n\n[stderr]\n<stderr>"`
* Exception: informational stderr (e.g., "Switched to branch...") MAY be appended to stdout

### 2.18 Error Handling

**FR-GIT-ERROR-01:** Error mapping to `ToolError`:

```rust
fn map_git_error(
    exit_code: i32,
    stderr: &str,
    tool_name: &str,
) -> ToolError {
    // Validation errors are handled before execution
    // This handles git execution errors
    
    ToolError::ExecutionFailed {
        tool: tool_name.to_string(),
        message: if stderr.is_empty() {
            format!("exit code {}", exit_code)
        } else {
            stderr.trim().to_string()
        },
    }
}
```

**FR-GIT-ERROR-02:** Error categories and handling:

| Category | Detection | Handling |
|----------|-----------|----------|
| Validation | Pre-execution | Return `ToolError::BadArgs` immediately |
| Not Found | Exit 128 + "not found" in stderr | Propagate git error |
| Conflict | Exit 1 + "conflict" in stderr | Propagate git error |
| Sandbox Violation | Sandbox validation fails | Return `ToolError::SandboxViolation` |
| Timeout | Duration exceeded | Kill process, return `ToolError::Timeout` |
| Cancelled | Abort signal received | Return `ToolError::Cancelled` |

**FR-GIT-ERROR-03:** Timeout handling with process termination:

```rust
async fn execute_with_timeout(
    child: &mut tokio::process::Child,
    timeout: Duration,
) -> Result<Output, ToolError> {
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.map_err(|e| ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: e.to_string(),
        }),
        Err(_) => {
            // Timeout - kill the process
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    unsafe { libc::killpg(pid as i32, libc::SIGKILL); }
                }
            }
            #[cfg(windows)]
            {
                let _ = child.start_kill();
            }
            
            Err(ToolError::Timeout {
                tool: "git".to_string(),
                elapsed: timeout,
            })
        }
    }
}
```

### 2.19 Parameter Validation Summary

| Tool | Mutual Exclusivity | Required Combinations | Validation Code |
|------|-------------------|----------------------|-----------------|
| git_status | — | — | None |
| git_diff | `name_only` vs `stat`; `cached` vs `from_ref/to_ref` | `output_dir` requires `from_ref` AND `to_ref` | FR-GIT-DIFF-03 |
| git_restore | — | `staged` OR `worktree` must be true; `paths` required | FR-GIT-REST-03 |
| git_add | `all` vs `update` | At least one of `paths`/`all`/`update` | FR-GIT-ADD-03 |
| git_commit | — | `type` and `message` required | FR-GIT-COMMIT-03 |
| git_log | `oneline` vs `format` (format wins) | — | None |
| git_branch | — | `rename` requires `new_name` | FR-GIT-BRANCH-03 |
| git_checkout | — | At least one of `branch`/`create_branch`/`commit`/`paths` | FR-GIT-CHECKOUT-03 |
| git_stash | — | — | None |
| git_show | `stat` vs `name_only` (name_only wins) | — | None |
| git_blame | — | `path` required; `start_line` ≤ `end_line` | FR-GIT-BLAME-03 |

---

## 3. Non-Functional Requirements

### 3.1 Security

**NFR-GIT-SEC-01:** Working directory sandbox validation:

```rust
fn validate_sandbox(working_dir: &str, ctx: &ToolCtx) -> Result<PathBuf, ToolError> {
    // Use existing sandbox infrastructure from engine/src/tools/sandbox.rs
    ctx.sandbox.resolve_path(working_dir, &ctx.working_dir)
}
```

**NFR-GIT-SEC-02:** Tools MUST NOT execute via shell:
* Direct `tokio::process::Command::new("git")`, NOT `Command::new("sh")` or `Command::new("cmd")`
* This prevents shell injection attacks in arguments

**NFR-GIT-SEC-03:** Argument injection prevention:
* Arguments starting with `-` MUST be preceded by `--` separator
* Example: branch name `--delete` → `git checkout -- --delete`
* Applies to: branch names, paths, refs, commit messages

**NFR-GIT-SEC-04:** Path traversal prevention:
* All paths MUST be validated via `sandbox.resolve_path()`
* Reject any path containing `..` components (lexical check)
* Reject absolute paths unless `allow_absolute=true` in sandbox config

**NFR-GIT-SEC-05:** Symlink safety:
* Path resolution MUST canonicalize symlinks
* Final canonical path MUST be within sandbox allowed_roots
* Implementation: use `std::fs::canonicalize()` then verify containment

**NFR-GIT-SEC-06:** `output_dir` validation for git_diff:
* Directory MUST be validated within sandbox before creation
* Created directories MUST have restrictive permissions (0750 on Unix)

**NFR-GIT-SEC-07:** Environment sanitization:
* Use `ctx.env_sanitizer.sanitize_env()` from framework
* Removes: `*_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, `AWS_*`, `ANTHROPIC_*`, `OPENAI_*`

**NFR-GIT-SEC-08:** Output sanitization:
* All git output MUST be sanitized before returning via `sanitize_terminal_text()`
* Prevents terminal control sequence injection

| ID | Requirement | Implementation |
|----|-------------|----------------|
| NFR-GIT-SEC-01 | `working_dir` validated against sandbox | `ctx.sandbox.resolve_path()` |
| NFR-GIT-SEC-02 | No shell execution | Direct `Command::new("git")` |
| NFR-GIT-SEC-03 | Argument injection prevention | `--` separator for `-` prefixed args |
| NFR-GIT-SEC-04 | Path traversal prevention | Reject `..`, validate via sandbox |
| NFR-GIT-SEC-05 | Symlink containment | Canonicalize + containment check |
| NFR-GIT-SEC-06 | `output_dir` within sandbox | `sandbox.resolve_path()` |
| NFR-GIT-SEC-07 | Env sanitization | `ctx.env_sanitizer.sanitize_env()` |
| NFR-GIT-SEC-08 | Output sanitization | `sanitize_terminal_text()` |

### 3.2 Performance

| ID | Requirement | Specification |
|----|-------------|---------------|
| NFR-GIT-PERF-01 | Configurable timeout | Default 30000ms, per-tool override |
| NFR-GIT-PERF-02 | Output truncation | Truncate, don't fail, at `max_bytes` |
| NFR-GIT-PERF-03 | Truncation marker | `"\n\n... [output truncated]"` |
| NFR-GIT-PERF-04 | Process cleanup | Kill process group on timeout |
| NFR-GIT-PERF-05 | Async execution | Non-blocking with tokio |

### 3.3 Reliability

| ID | Requirement | Specification |
|----|-------------|---------------|
| NFR-GIT-REL-01 | Exactly one result per call | Every tool call produces exactly one result |
| NFR-GIT-REL-02 | Timeout enforcement | Process killed after timeout_ms |
| NFR-GIT-REL-03 | Cancellation support | Abort handle in ToolCtx |
| NFR-GIT-REL-04 | Error propagation | Git errors passed through to user |

### 3.4 Maintainability

| ID | Requirement | Specification |
|----|-------------|---------------|
| NFR-GIT-MAIN-01 | Typed argument parsing | Serde deserialization to typed structs |
| NFR-GIT-MAIN-02 | Single validation source | Validation in one place per tool |
| NFR-GIT-MAIN-03 | Shared utilities | Common git execution in `git/mod.rs` |

---

## 4. Configuration

### 4.1 TOML Configuration

Add to `~/.forge/config.toml`:

```toml
[tools.git]
enabled = true
timeout_ms = 30000
max_bytes = 200000
```

### 4.2 Rust Configuration Types

Add to `engine/src/config.rs`:

```rust
/// Git tools configuration.
#[derive(Debug, Default, Deserialize)]
pub struct GitToolsConfig {
    /// Enable/disable all git tools. Default: true.
    pub enabled: Option<bool>,
    
    /// Default timeout in milliseconds. Default: 30000.
    pub timeout_ms: Option<u64>,
    
    /// Default max output bytes. Default: 200000.
    pub max_bytes: Option<usize>,
}
```

Add field to `ToolsConfig`:

```rust
pub struct ToolsConfig {
    // ... existing fields ...
    
    /// Git tools configuration.
    pub git: Option<GitToolsConfig>,
}
```

### 4.3 Runtime Configuration

In `engine/src/tools/git/mod.rs`:

```rust
/// Runtime configuration for git tools.
#[derive(Debug, Clone)]
pub struct GitToolConfig {
    pub enabled: bool,
    pub timeout_ms: u64,
    pub max_bytes: usize,
}

impl Default for GitToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 30_000,
            max_bytes: 200_000,
        }
    }
}

impl GitToolConfig {
    pub fn from_config(config: Option<&GitToolsConfig>) -> Self {
        let defaults = Self::default();
        match config {
            Some(cfg) => Self {
                enabled: cfg.enabled.unwrap_or(defaults.enabled),
                timeout_ms: cfg.timeout_ms.unwrap_or(defaults.timeout_ms),
                max_bytes: cfg.max_bytes.unwrap_or(defaults.max_bytes),
            },
            None => defaults,
        }
    }
    
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}
```

### 4.4 Configuration Parameters

| Key | Type | Default | Min | Max | Description |
|-----|------|---------|-----|-----|-------------|
| `enabled` | bool | `true` | — | — | Enable/disable all git tools |
| `timeout_ms` | u64 | `30000` | `100` | `600000` | Default timeout for git operations |
| `max_bytes` | usize | `200000` | `1` | `5000000` | Default max output bytes |

### 4.5 Environment Variables

No dedicated environment variables. Git tools inherit:
* Sandbox configuration from `[tools.sandbox]`
* Environment sanitization from `[tools.environment]`
* Approval policy from `[tools.approval]`

### 4.6 Integration Wiring

**FR-GIT-CONFIG-01:** Git tools registration in the engine initialization:

```rust
// In engine/src/init.rs or tool registration module

use crate::tools::git::{self, GitToolConfig};

pub fn register_builtin_tools(
    registry: &mut ToolRegistry,
    config: &ForgeConfig,
) -> Result<(), ToolError> {
    // ... existing tools ...

    // Register git tools if enabled
    let git_config = GitToolConfig::from_config(config.tools.git.as_ref());
    git::register_git_tools(registry, git_config)?;

    Ok(())
}
```

**FR-GIT-CONFIG-02:** Add `git` field to `ToolsConfig`:

```rust
// In engine/src/config.rs

#[derive(Debug, Default, Deserialize)]
pub struct ToolsConfig {
    // ... existing fields ...

    /// Git tools configuration.
    pub git: Option<GitToolsConfig>,
}
```

**FR-GIT-CONFIG-03:** MCP preference configuration:

```rust
#[derive(Debug, Default, Deserialize)]
pub struct GitToolsConfig {
    pub enabled: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub max_bytes: Option<usize>,
    /// Prefer MCP git tools over native. Default: false.
    pub prefer_mcp: Option<bool>,
}
```

---

## 5. Verification Requirements

### 5.1 Unit Tests — Functional
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-STATUS-01 | Status returns porcelain output | FR-GIT-STATUS-01 |
| T-GIT-STATUS-02 | Status with `porcelain=false` returns human format | FR-GIT-STATUS-02 |
| T-GIT-DIFF-01 | Diff respects `cached` flag | FR-GIT-DIFF-01 |
| T-GIT-DIFF-02 | Diff with `from_ref`/`to_ref` writes patches to `output_dir` | FR-GIT-DIFF-03 |
| T-GIT-DIFF-03 | Diff rejects `cached` with `from_ref` | FR-GIT-DIFF-02 |
| T-GIT-DIFF-04 | Diff `name_only` takes precedence over `stat` | FR-GIT-DIFF-02 |
| T-GIT-RESTORE-01 | Restore reverts worktree file | FR-GIT-REST-01 |
| T-GIT-RESTORE-02 | Restore staged removes from index | FR-GIT-REST-01 |
| T-GIT-RESTORE-03 | Restore rejects both false | FR-GIT-REST-02 |
| T-GIT-ADD-01 | Add stages specific paths | FR-GIT-ADD-01 |
| T-GIT-ADD-02 | Add with `all=true` stages everything | FR-GIT-ADD-01 |
| T-GIT-ADD-03 | Add rejects `all` and `update` both true | FR-GIT-ADD-02 |
| T-GIT-COMMIT-01 | Commit creates conventional message | FR-GIT-COMMIT-02 |
| T-GIT-COMMIT-02 | Commit with scope formats correctly | FR-GIT-COMMIT-02 |
| T-GIT-COMMIT-03 | Commit rejects empty staging area | FR-GIT-COMMIT-04 |
| T-GIT-LOG-01 | Log respects `max_count` | FR-GIT-LOG-01 |
| T-GIT-LOG-02 | Log respects `author` filter | FR-GIT-LOG-01 |
| T-GIT-LOG-03 | Log `format` overrides `oneline` | FR-GIT-LOG-02 |
| T-GIT-BRANCH-01 | Branch lists with verbose output | FR-GIT-BRANCH-02 |
| T-GIT-BRANCH-02 | Branch create works | FR-GIT-BRANCH-01 |
| T-GIT-BRANCH-03 | Branch delete works | FR-GIT-BRANCH-01 |
| T-GIT-BRANCH-04 | Branch rename requires `new_name` | FR-GIT-BRANCH-03 |
| T-GIT-CHECKOUT-01 | Checkout switches branch | FR-GIT-CHECKOUT-01 |
| T-GIT-CHECKOUT-02 | Checkout with `create_branch` creates branch | FR-GIT-CHECKOUT-01 |
| T-GIT-CHECKOUT-03 | Checkout commit shows detached HEAD warning | FR-GIT-CHECKOUT-04 |
| T-GIT-CHECKOUT-04 | Checkout requires at least one param | FR-GIT-CHECKOUT-02 |
| T-GIT-STASH-01 | Stash push saves changes | FR-GIT-STASH-01 |
| T-GIT-STASH-02 | Stash pop restores changes | FR-GIT-STASH-01 |
| T-GIT-STASH-03 | Stash list returns stash entries | FR-GIT-STASH-01 |
| T-GIT-STASH-04 | Stash clear removes all stashes | FR-GIT-STASH-01 |
| T-GIT-SHOW-01 | Show displays commit | FR-GIT-SHOW-01 |
| T-GIT-SHOW-02 | Show `name_only` takes precedence over `stat` | FR-GIT-SHOW-02 |
| T-GIT-BLAME-01 | Blame shows line authorship | FR-GIT-BLAME-01 |
| T-GIT-BLAME-02 | Blame respects line range | FR-GIT-BLAME-01 |
| T-GIT-BLAME-03 | Blame rejects `start_line > end_line` | FR-GIT-BLAME-02 |

### 5.2 Unit Tests — Error Handling
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-ERR-01 | Invalid `working_dir` returns error | FR-GIT-COMMON-01 |
| T-GIT-ERR-02 | Non-repo `working_dir` returns git error | FR-GIT-COMMON-01 |
| T-GIT-ERR-03 | Timeout kills process, returns partial | FR-GIT-ERROR-03 |
| T-GIT-ERR-04 | Unknown branch returns `not_found` | FR-GIT-ERROR-01 |
| T-GIT-ERR-05 | Checkout with uncommitted changes returns `conflict` | FR-GIT-ERROR-01 |

### 5.3 Unit Tests — Security
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-SEC-01 | Branch name starting with `-` is sanitized | NFR-GIT-SEC-03 |
| T-GIT-SEC-02 | Path traversal `../` is rejected | NFR-GIT-SEC-04 |
| T-GIT-SEC-03 | `working_dir` outside sandbox is rejected | NFR-GIT-SEC-01 |
| T-GIT-SEC-04 | `output_dir` outside sandbox is rejected | NFR-GIT-SEC-06 |
| T-GIT-SEC-05 | Symlink pointing outside sandbox is rejected | NFR-GIT-SEC-05 |
| T-GIT-SEC-06 | Junction pointing outside sandbox is rejected (Windows) | NFR-GIT-SEC-05 |
| T-GIT-SEC-07 | Environment variables with secrets are filtered | NFR-GIT-SEC-07 |
| T-GIT-SEC-08 | Terminal escape sequences in output are sanitized | NFR-GIT-SEC-08 |
| T-GIT-SEC-09 | Commit message with shell metacharacters is safe | NFR-GIT-SEC-02 |
| T-GIT-SEC-10 | Ref name `--exec=malicious` is safely handled | NFR-GIT-SEC-03 |

### 5.4 Unit Tests — Common Behavior
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-COMMON-01 | Stderr captured in output | FR-GIT-COMMON-03 |
| T-GIT-COMMON-02 | Mutating tool requires approval | FR-GIT-COMMON-04 |
| T-GIT-COMMON-03 | Output truncation includes marker | NFR-GIT-PERF-03 |
| T-GIT-COMMON-04 | JSON output format correct | FR-GIT-OUTPUT-01 |

### 5.5 Unit Tests — Edge Cases
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-EDGE-01 | Status on empty repository (no commits) | FR-GIT-STATUS-01 |
| T-GIT-EDGE-02 | Diff with empty result (no changes) | FR-GIT-DIFF-01 |
| T-GIT-EDGE-03 | Diff output exactly at `max_bytes` boundary | FR-GIT-OUTPUT-02 |
| T-GIT-EDGE-04 | Diff output one byte over truncation | FR-GIT-OUTPUT-02 |
| T-GIT-EDGE-05 | Log on repository with single commit | FR-GIT-LOG-01 |
| T-GIT-EDGE-06 | Log with `max_count=0` returns validation error | FR-GIT-LOG-01 |
| T-GIT-EDGE-07 | Branch name with Unicode characters | FR-GIT-BRANCH-01 |
| T-GIT-EDGE-08 | Branch name with slashes (feature/foo) | FR-GIT-BRANCH-01 |
| T-GIT-EDGE-09 | Checkout branch that matches a file name | FR-GIT-CHECKOUT-01 |
| T-GIT-EDGE-10 | Stash on clean working tree | FR-GIT-STASH-05 |
| T-GIT-EDGE-11 | Blame on file with Windows line endings | FR-GIT-BLAME-01 |
| T-GIT-EDGE-12 | Blame on newly added file (uncommitted) | FR-GIT-BLAME-05 |
| T-GIT-EDGE-13 | Restore with glob pattern in path | FR-GIT-REST-01 |
| T-GIT-EDGE-14 | Add with paths containing spaces | FR-GIT-ADD-01 |
| T-GIT-EDGE-15 | Commit with multi-line message | FR-GIT-COMMIT-01 |
| T-GIT-EDGE-16 | Show merge commit (multiple parents) | FR-GIT-SHOW-01 |
| T-GIT-EDGE-17 | Status on detached HEAD | FR-GIT-STATUS-01 |
| T-GIT-EDGE-18 | Operations on shallow clone repository | FR-GIT-COMMON-01 |
| T-GIT-EDGE-19 | Blame on file tracked by Git LFS (stub) | FR-GIT-BLAME-05 |
| T-GIT-EDGE-20 | Path containing submodule directory | FR-GIT-COMMON-07 |
| T-GIT-EDGE-21 | Commit rejects unconfigured user.name/email | FR-GIT-COMMIT-05 |
| T-GIT-EDGE-22 | git_checkout RestorePaths requires approval | FR-GIT-CHECKOUT-06 |

### 5.6 Unit Tests — Concurrency and Timeout
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-TIMEOUT-01 | Command exceeding timeout is killed | NFR-GIT-PERF-04 |
| T-GIT-TIMEOUT-02 | Partial output returned on timeout | NFR-GIT-PERF-02 |
| T-GIT-TIMEOUT-03 | Cancellation aborts running git command | NFR-GIT-REL-03 |
| T-GIT-TIMEOUT-04 | Process group terminated on timeout (Unix) | FR-GIT-ERROR-03 |

### 5.7 Integration Tests
| Test ID | Description | Covers |
| --- | --- | --- |
| T-GIT-INT-01 | Full commit workflow: add → commit | FR-GIT-ADD, FR-GIT-COMMIT |
| T-GIT-INT-02 | Branch workflow: create → checkout → delete | FR-GIT-BRANCH, FR-GIT-CHECKOUT |
| T-GIT-INT-03 | Stash workflow: push → list → pop | FR-GIT-STASH |
| T-GIT-INT-04 | Restore undoes add | FR-GIT-ADD, FR-GIT-RESTORE |
| T-GIT-INT-05 | Diff between two tagged commits | FR-GIT-DIFF |
| T-GIT-INT-06 | Log filtered by author and date range | FR-GIT-LOG |
| T-GIT-INT-07 | Blame at historical commit | FR-GIT-BLAME |
| T-GIT-INT-08 | Show with custom format | FR-GIT-SHOW |

### 5.8 Test Count Summary
| Category | Count |
| --- | --- |
| Functional | 34 |
| Error Handling | 5 |
| Security | 10 |
| Common Behavior | 4 |
| Edge Cases | 22 |
| Concurrency/Timeout | 4 |
| Integration | 8 |
| **Total** | **87** |

---

## 6. Implementation Guide

### 6.1 Module Skeleton

Create `engine/src/tools/git/mod.rs`:

```rust
//! Git tools module.
//!
//! Implements git operations as ToolExecutor implementations.

mod types;
mod status;
mod diff;
mod restore;
mod add;
mod commit;
mod log;
mod branch;
mod checkout;
mod stash;
mod show;
mod blame;

pub use types::*;

use crate::tools::{ToolError, ToolRegistry};

/// Register all git tools with the registry.
pub fn register_git_tools(
    registry: &mut ToolRegistry,
    config: GitToolConfig,
) -> Result<(), ToolError> {
    if !config.enabled {
        tracing::info!("Git tools disabled by configuration");
        return Ok(());
    }

    registry.register(Box::new(status::git_status::new(config.clone())));
    registry.register(Box::new(diff::git_diff::new(config.clone())));
    registry.register(Box::new(restore::git_restore::new(config.clone())));
    registry.register(Box::new(add::git_add::new(config.clone())));
    registry.register(Box::new(commit::git_commit::new(config.clone())));
    registry.register(Box::new(log::git_log::new(config.clone())));
    registry.register(Box::new(branch::git_branch::new(config.clone())));
    registry.register(Box::new(checkout::git_checkout::new(config.clone())));
    registry.register(Box::new(stash::git_stash::new(config.clone())));
    registry.register(Box::new(show::git_show::new(config.clone())));
    registry.register(Box::new(blame::git_blame::new(config.clone())));

    Ok(())
}

/// Execute a git command and capture output.
pub(crate) async fn execute_git(
    args: &[&str],
    working_dir: &std::path::Path,
    timeout: std::time::Duration,
    ctx: &crate::tools::ToolCtx,
) -> Result<GitOutput, ToolError> {
    use tokio::process::Command;

    // Build command without shell
    let mut cmd = Command::new("git");
    cmd.args(args)
       .current_dir(working_dir)
       .stdin(std::process::Stdio::null())
       .stdout(std::process::Stdio::piped())
       .stderr(std::process::Stdio::piped());

    // Unix: Create new process group for clean termination
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: process_group(0) creates a new process group with the child as leader.
        // This allows killpg() to terminate the entire process tree on timeout.
        cmd.process_group(0);
    }

    // Sanitize environment using the EnvSanitizer from context
    let current_env: Vec<(String, String)> = std::env::vars().collect();
    let sanitized = ctx.env_sanitizer.sanitize_env(&current_env);
    cmd.env_clear();
    for (key, value) in sanitized {
        cmd.env(key, value);
    }

    // Spawn process
    let mut child = cmd.spawn().map_err(|e| ToolError::ExecutionFailed {
        tool: "git".to_string(),
        message: format!("Failed to spawn git: {}", e),
    })?;

    // Execute with timeout
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);

            if exit_code != 0 {
                return Err(ToolError::ExecutionFailed {
                    tool: "git".to_string(),
                    message: if stderr.is_empty() {
                        format!("exit code {}", exit_code)
                    } else {
                        stderr.trim().to_string()
                    },
                });
            }

            Ok(GitOutput { stdout, stderr, exit_code })
        }
        Ok(Err(e)) => Err(ToolError::ExecutionFailed {
            tool: "git".to_string(),
            message: e.to_string(),
        }),
        Err(_) => {
            // Timeout - kill process tree
            kill_process_tree(&mut child).await;
            Err(ToolError::Timeout {
                tool: "git".to_string(),
                elapsed: timeout,
            })
        }
    }
}

#[cfg(unix)]
async fn kill_process_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // Kill the entire process group (child + any spawned subprocesses)
        // SAFETY: pid is valid since we just retrieved it from the child.
        unsafe { libc::killpg(pid as i32, libc::SIGKILL); }
    }
    // Wait for process to actually terminate to avoid zombies
    let _ = child.wait().await;
}

#[cfg(windows)]
async fn kill_process_tree(child: &mut tokio::process::Child) {
    // On Windows, start_kill() sends termination signal
    let _ = child.start_kill();
    // Wait for process to actually terminate
    let _ = child.wait().await;
}

/// Output from git command execution.
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

### 6.2 Tool Implementation Pattern

Each tool follows this pattern. Example for `git_status`:

```rust
// engine/src/tools/git/status.rs

use super::{execute_git, GitToolConfig, GitOutput};
use crate::tools::{ToolExecutor, ToolCtx, ToolError, ToolFut, RiskLevel};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub struct GitStatus {
    config: GitToolConfig,
}

impl GitStatus {
    pub fn new(config: GitToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default = "default_true")]
    porcelain: bool,
    #[serde(default = "default_true")]
    branch: bool,
    #[serde(default = "default_true")]
    untracked: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
    working_dir: Option<String>,
}

fn default_true() -> bool { true }

impl ToolExecutor for git_status {
    fn name(&self) -> &'static str { "git_status" }

    fn description(&self) -> &'static str {
        "Show working tree status: staged, modified, and untracked files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "porcelain": {
                    "type": "boolean",
                    "default": true,
                    "description": "Use porcelain output (--porcelain=1)"
                },
                "branch": {
                    "type": "boolean",
                    "default": true,
                    "description": "Include branch info (-b)"
                },
                "untracked": {
                    "type": "boolean",
                    "default": true,
                    "description": "Include untracked files"
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 100,
                    "default": 30000,
                    "description": "Timeout in milliseconds"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for git command"
                }
            },
            "additionalProperties": false
        })
    }

    fn is_side_effecting(&self) -> bool { false }

    fn requires_approval(&self) -> bool { false }

    fn risk_level(&self) -> RiskLevel { RiskLevel::Low }

    fn approval_summary(&self, _args: &Value) -> Result<String, ToolError> {
        Ok("Show git status".to_string())
    }

    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(self.config.timeout_ms))
    }

    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            // Parse arguments
            let args: Args = serde_json::from_value(args)
                .map_err(|e| ToolError::BadArgs { message: e.to_string() })?;

            // Resolve working directory
            let working_dir = match &args.working_dir {
                Some(wd) => ctx.sandbox.resolve_path(wd, &ctx.working_dir)?,
                None => ctx.working_dir.clone(),
            };

            // Verify git repository
            if !working_dir.join(".git").exists() {
                return Err(ToolError::ExecutionFailed {
                    tool: "git".to_string(),
                    message: format!("Not a git repository: {}", working_dir.display()),
                });
            }

            // Build command
            let mut cmd_args = vec!["status"];
            if args.porcelain {
                cmd_args.push("--porcelain=1");
                if args.branch {
                    cmd_args.push("-b");
                }
            }
            if !args.untracked {
                cmd_args.push("-uno");
            }

            // Execute
            let timeout = Duration::from_millis(
                args.timeout_ms.unwrap_or(self.config.timeout_ms)
            );
            let output = execute_git(&cmd_args, &working_dir, timeout, ctx).await?;

            // Return stdout (stderr appended if non-empty)
            let mut result = output.stdout;
            if !output.stderr.is_empty() {
                result.push_str("\n\n[stderr]\n");
                result.push_str(&output.stderr);
            }

            Ok(result)
        })
    }
}
```

### 6.3 Dynamic Approval Pattern

For tools with operation-dependent approval (git_branch, git_checkout, git_stash), the trait methods inspect the arguments to determine approval requirements. The framework handles approval **before** calling `execute()`, so executors do not check approval status at runtime.

```rust
// Partial example for git_branch

pub struct GitBranch {
    config: GitToolConfig,
}

impl GitBranch {
    pub fn new(config: GitToolConfig) -> Self {
        Self { config }
    }

    /// Parse args to determine operation (used by multiple trait methods).
    fn parse_operation(&self, args: &Value) -> Result<git_branchOp, ToolError> {
        let args: GitBranchArgs = serde_json::from_value(args.clone())
            .map_err(|e| ToolError::BadArgs { message: e.to_string() })?;
        Ok(args.operation())
    }
}

impl ToolExecutor for git_branch {
    fn name(&self) -> &'static str { "git_branch" }

    fn description(&self) -> &'static str {
        "List, create, rename, or delete branches."
    }

    fn schema(&self) -> Value { /* ... */ }

    fn is_side_effecting(&self) -> bool {
        // Conservative: return true since some operations mutate.
        // Framework uses requires_approval() for actual gating.
        true
    }

    fn requires_approval(&self) -> bool {
        // Conservative default - framework calls approval_summary()
        // to get operation-specific behavior. Read-only ops still
        // return a summary but framework may skip prompting based
        // on policy configuration for low-risk tools.
        true
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    fn approval_summary(&self, args: &Value) -> Result<String, ToolError> {
        let args: GitBranchArgs = serde_json::from_value(args.clone())
            .map_err(|e| ToolError::BadArgs { message: e.to_string() })?;

        match args.operation() {
            git_branchOp::List => Ok("List branches".to_string()),
            git_branchOp::Create => {
                Ok(format!("Create branch '{}'", args.create.as_ref().unwrap()))
            }
            git_branchOp::Delete => {
                Ok(format!("Delete branch '{}'", args.delete.as_ref().unwrap()))
            }
            git_branchOp::ForceDelete => {
                Ok(format!("Force delete branch '{}'", args.force_delete.as_ref().unwrap()))
            }
            git_branchOp::Rename => {
                Ok(format!(
                    "Rename branch '{}' to '{}'",
                    args.rename.as_ref().unwrap(),
                    args.new_name.as_ref().unwrap()
                ))
            }
        }
    }

    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            let args: GitBranchArgs = serde_json::from_value(args)
                .map_err(|e| ToolError::BadArgs { message: e.to_string() })?;

            // Validate arguments
            validate_args(&args)?;

            // NOTE: Approval is handled by the framework BEFORE execute() is called.
            // If we reach here, the operation was either:
            // - Read-only (List) and auto-approved by policy
            // - Mutating and explicitly approved by user
            // No runtime approval check needed in execute().

            // Resolve working directory
            let working_dir = resolve_working_dir(&args.working_dir, ctx)?;

            // Execute based on operation
            match args.operation() {
                git_branchOp::List => {
                    let mut cmd_args = vec!["branch", "-v"];
                    if args.list_all { cmd_args.push("-a"); }
                    if args.list_remote { cmd_args.push("-r"); }
                    let output = execute_git(&cmd_args, &working_dir, self.config.timeout(), ctx).await?;
                    Ok(output.stdout)
                }
                git_branchOp::Create => {
                    let name = args.create.as_ref().unwrap();
                    let output = execute_git(&["branch", name], &working_dir, self.config.timeout(), ctx).await?;
                    Ok(format!("Created branch '{}'\n{}", name, output.stdout))
                }
                // ... other operations ...
            }
        })
    }
}
```

### 6.4 Path Sanitization Utility

```rust
// engine/src/tools/git/types.rs

/// Sanitize path arguments for git commands.
/// Paths are always passed after `--` separator to prevent flag injection.
pub fn build_path_args(paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return vec![];
    }

    let mut args = vec!["--".to_string()];
    args.extend(paths.iter().cloned());
    args
}

/// Sanitize a single argument that could be a ref or branch name.
/// Arguments starting with `-` could be interpreted as flags.
pub fn sanitize_ref_arg(arg: &str) -> Vec<&str> {
    if arg.starts_with('-') || arg.contains("..") {
        // Use explicit -- separator
        vec!["--", arg]
    } else {
        vec![arg]
    }
}

/// Truncate output at max_bytes while preserving UTF-8 validity.
pub fn truncate_output(output: String, max_bytes: usize) -> (String, bool) {
    if output.len() <= max_bytes {
        return (output, false);
    }

    const MARKER: &str = "\n\n... [output truncated]";
    let max_body = max_bytes.saturating_sub(MARKER.len());

    // Find valid UTF-8 boundary
    let mut end = max_body;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }

    let mut truncated = output;
    truncated.truncate(end);
    truncated.push_str(MARKER);
    (truncated, true)
}
```

### 6.5 Testing Utilities

```rust
// engine/src/tools/git/tests.rs

use tempfile::TempDir;
use std::process::Command;

/// Create a temporary git repository for testing.
pub fn create_test_repo() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");

    // Initialize repo
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("git init");

    // Configure user (required for commits)
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .expect("git config email");

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir.path())
        .output()
        .expect("git config name");

    dir
}

/// Create a file in the test repo.
pub fn create_file(dir: &TempDir, name: &str, content: &str) {
    std::fs::write(dir.path().join(name), content).expect("write file");
}

/// Stage and commit a file.
pub fn commit_file(dir: &TempDir, name: &str, content: &str, message: &str) {
    create_file(dir, name, content);

    Command::new("git")
        .args(["add", name])
        .current_dir(dir.path())
        .output()
        .expect("git add");

    Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir.path())
        .output()
        .expect("git commit");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_status_clean() {
        let repo = create_test_repo();
        commit_file(&repo, "test.txt", "content", "Initial commit");

        // Create mock ToolCtx...
        // Execute git_status...
        // Assert output is empty (clean working tree)
    }

    #[tokio::test]
    async fn test_git_status_modified() {
        let repo = create_test_repo();
        commit_file(&repo, "test.txt", "content", "Initial commit");
        create_file(&repo, "test.txt", "modified content");

        // Execute git_status...
        // Assert output shows " M test.txt"
    }
}
```

### 6.6 Implementation Checklist

Use this checklist when implementing each tool:

- [ ] Create `engine/src/tools/git/<tool>.rs`
- [ ] Define argument struct with `#[derive(Debug, Deserialize)]`
- [ ] Add serde defaults for optional fields
- [ ] Implement `ToolExecutor` trait:
  - [ ] `name()` returns exact tool name
  - [ ] `description()` returns concise description
  - [ ] `schema()` returns JSON Schema matching FR spec
  - [ ] `is_side_effecting()` returns correct value
  - [ ] `requires_approval()` returns correct value (or dynamic)
  - [ ] `risk_level()` returns correct RiskLevel
  - [ ] `approval_summary()` returns formatted summary
  - [ ] `timeout()` returns configured timeout
  - [ ] `execute()` implements full logic
- [ ] Add validation in `execute()`:
  - [ ] Parse args with serde
  - [ ] Validate mutual exclusivity
  - [ ] Validate required combinations
  - [ ] Resolve and validate working_dir
  - [ ] Verify git repository
- [ ] Build command arguments:
  - [ ] Use `--` separator before paths
  - [ ] Sanitize ref arguments starting with `-`
- [ ] Handle output:
  - [ ] Apply truncation if `max_bytes` exceeded
  - [ ] Append stderr if non-empty
- [ ] Write unit tests:
  - [ ] Happy path
  - [ ] Validation errors
  - [ ] Edge cases
- [ ] Register in `mod.rs`

---

## Appendix A: Complete Tool Reference

| Tool | Name | Side Effects | Approval | Risk | Parameters |
|------|------|--------------|----------|------|------------|
| git_status | `git_status` | No | No | Low | porcelain, branch, untracked, timeout_ms, working_dir |
| git_diff | `git_diff` | No | No | Low | cached, name_only, stat, unified, paths, from_ref, to_ref, output_dir, max_bytes, timeout_ms, working_dir |
| git_restore | `git_restore` | Yes | Yes | High | paths*, staged, worktree, timeout_ms, working_dir |
| git_add | `git_add` | Yes | Yes | Medium | paths, all, update, timeout_ms, working_dir |
| git_commit | `git_commit` | Yes | Yes | Medium | type*, message*, scope, timeout_ms, working_dir |
| git_log | `git_log` | No | No | Low | max_count, oneline, format, author, since, until, grep, path, max_bytes, timeout_ms, working_dir |
| git_branch | `git_branch` | Dynamic | Dynamic | Medium | list_all, list_remote, create, delete, force_delete, rename, new_name, timeout_ms, working_dir |
| git_checkout | `git_checkout` | Yes | Dynamic | Medium | branch, create_branch, commit, paths, timeout_ms, working_dir |
| git_stash | `git_stash` | Dynamic | Dynamic | Dynamic | action, message, index, include_untracked, timeout_ms, working_dir |
| git_show | `git_show` | No | No | Low | commit, stat, name_only, format, max_bytes, timeout_ms, working_dir |
| git_blame | `git_blame` | No | No | Low | path*, start_line, end_line, commit, max_bytes, timeout_ms, working_dir |

*Required parameter

---

## Appendix B: Error Messages

| Error Condition | Error Type | Message Pattern |
|-----------------|------------|-----------------|
| Invalid JSON args | `BadArgs` | `"Invalid arguments: <serde error>"` |
| Missing required param | `BadArgs` | `"<param> is required"` |
| Mutual exclusivity | `BadArgs` | `"<param1> and <param2> are mutually exclusive"` |
| Required combination | `BadArgs` | `"<param1> requires <param2>"` |
| Not a git repo | `ExecutionFailed` | `"Not a git repository: <path>"` |
| Sandbox violation | `SandboxViolation` | `"Path outside sandbox: <path>"` |
| Git error | `ExecutionFailed` | `"<git stderr>"` |
| Timeout | `Timeout` | `"git command timed out after <ms>ms"` |
| Approval needed | `ApprovalRequired` | `"<approval_summary>"` |

---

## Appendix C: Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 2.1 | 2026-01-11 | Claude | Added MCP relationship section, fixed approval patterns, added security/edge case tests |
| 2.0 | 2026-01-10 | Claude | Implementation-ready revision with Rust types, JSON schemas, pseudocode |
| 1.3 | 2025-12-15 | — | Align with implementation, add missing tools |
| 1.0 | 2025-11-01 | — | Initial draft |



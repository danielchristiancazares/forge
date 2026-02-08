# Undo Tool

## Software Requirements Document

**Version:** 0.1
**Date:** 2026-02-08
**Status:** Draft
**Baseline code reference:**

- `engine/src/tools/builtins.rs` (planned `Undo` + `Edit` integration)
- `engine/src/tools/mod.rs` (ToolCtx wiring)
- `engine/src/tool_loop.rs` (error formatting + approval gating)

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-25 | Header & TOC |
| 26-34 | 0. Change Log |
| 35-74 | 1. Introduction |
| 75-138 | 2. Functional Requirements |
| 139-166 | 3. Non-Functional Requirements |
| 167-176 | 4. Configuration |
| 177-189 | 5. Verification Requirements |

---

## 0. Change Log

### 0.1 Initial draft

- Defines a safe, session-local `Undo` tool for reverting the last successfully-applied `Edit`.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for an `Undo` tool that allows an LLM to revert the most recent successfully-applied `Edit` (LP1 patch) in the current Forge session.

This tool is intended as a safety valve for cases where the model accidentally applies an incorrect patch (for example, erasing a file's contents).

### 1.2 Scope

In scope:

- Undoing the last successfully-applied `Edit` tool call, as one atomic unit (potentially multi-file).
- Safety checks that prevent overwriting changes made after the `Edit` (including user changes or other tools).
- Session-local state only (lifetime of the running Forge process).

Out of scope:

- Undoing `Write`, `Run`, or git tools.
- Multi-step undo stacks, redo, or persistence across restarts.
- Conversation/history rewind (note: Forge already has a `/undo` command for conversation checkpoints; this tool is distinct).

### 1.3 Definitions

- Session: lifetime of the running Forge process.
- Applied edit: an `Edit` tool call that successfully writes at least one changed file to disk.
- Undo entry: the session-local record used to revert the last applied edit.
- Post-edit state: the exact bytes written by the applied edit.

### 1.4 References

| Document | Description |
| --- | --- |
| `engine/src/tools/builtins.rs` | `Edit` implementation (`ApplyPatchTool`) and built-in tool registration |
| `engine/src/tools/sandbox.rs` | Sandbox path resolution and denied-pattern enforcement |
| `engine/src/tool_loop.rs` | Tool dispatch, approval workflow, and error string formatting |
| `INVARIANT_FIRST_ARCHITECTURE.md` | Boundary/proof style used throughout Forge |

---

## 2. Functional Requirements

### 2.1 Tool Interface

**FR-UNDO-01:** Tool name MUST be `Undo`.

**FR-UNDO-02:** Request schema MUST be an empty object and MUST set `additionalProperties=false`.

**FR-UNDO-03:** The tool MUST be treated as side-effecting and MUST be approval-gated under the standard tool approval policy (i.e., it prompts like `Edit`/`Write` in Default mode). The tool MUST NOT be allowlisted by default.

### 2.2 Session-Local Undo State

**FR-UNDO-04:** The system MUST store at most one undo entry (the last applied edit) per session.

**FR-UNDO-05:** The undo entry MUST be created or replaced only after an `Edit` call successfully applies changes to disk AND changes at least one file.

**FR-UNDO-06:** The undo entry MUST record, for each changed file:

- Canonical, sandbox-resolved path
- Whether the file existed before the edit
- The exact pre-edit bytes (baseline)
- Pre-edit permissions (when available and meaningful on the platform)
- A cryptographic hash of the post-edit bytes written by Forge (e.g., SHA-256)

**FR-UNDO-07:** If an `Edit` tool call applies no changes (e.g., "No changes applied."), it MUST NOT create or replace the undo entry.

### 2.3 Undo Execution

**FR-UNDO-08:** If no undo entry exists, `Undo` MUST fail with the exact user-facing message (verbatim, no prefixes/suffixes/newlines):

`No edits have been applied to any file with this session.`

**FR-UNDO-09:** `Undo` MUST revert only the most recent applied edit recorded in the undo entry.

**FR-UNDO-10:** `Undo` MUST validate all target files before making any filesystem changes. Validation MUST include:

- Every target is within the sandbox allowed roots and does not match denied patterns.
- No target path refers to a directory.
- Each target exists on disk (since post-edit files must exist).
- The current file bytes hash MUST equal the recorded post-edit hash.

If any validation fails, `Undo` MUST make no filesystem changes.

**FR-UNDO-11:** `Undo` MUST be atomic across all files in the undo entry: either all target files are reverted successfully, or the workspace is left unchanged.

**FR-UNDO-12:** For files that existed before the edit, `Undo` MUST restore the recorded pre-edit bytes and restore recorded permissions when applicable.

**FR-UNDO-13:** For files created by the edit, `Undo` MUST remove the file.

**FR-UNDO-14:** On success, `Undo` MUST clear the undo entry so it cannot be applied twice.

**FR-UNDO-15:** On success, `Undo` MUST invalidate stale-file protection for affected files (e.g., by clearing relevant entries in the tool file hash cache).

### 2.4 Tool Output

**FR-UNDO-16:** On success, the tool output MUST include:

- Number of files reverted
- A list of reverted paths (displayed relative to the working directory when possible)

**FR-UNDO-17:** On failure (other than FR-UNDO-08), the tool error MUST include a specific reason suitable for LLM recovery (e.g., "hash mismatch", "file missing", "permission denied") and SHOULD identify the relevant file using a display-safe path.

---

## 3. Non-Functional Requirements

### 3.1 Safety

| Requirement | Specification |
| --- | --- |
| NFR-UNDO-SAFE-01 | Must never overwrite changes made after the applied edit (enforced via post-edit hash check). |
| NFR-UNDO-SAFE-02 | Must be atomic across multiple files. |
| NFR-UNDO-SAFE-03 | Must not operate on directories (refuse to delete/overwrite). |
| NFR-UNDO-SAFE-04 | Must not accept user/model-provided paths or checkpoint IDs that expand undo scope. |

### 3.2 Security

| Requirement | Specification |
| --- | --- |
| NFR-UNDO-SEC-01 | Must enforce sandboxed path access. |
| NFR-UNDO-SEC-02 | Requires approval per policy (not allowlisted by default). |

### 3.3 Performance & Resource Use

| Requirement | Specification |
| --- | --- |
| NFR-UNDO-PERF-01 | Must store only the last applied edit (no unbounded growth). |
| NFR-UNDO-PERF-02 | Undo preflight hashing is O(total bytes of affected files). |
| NFR-UNDO-PERF-03 | Implementation SHOULD document memory impact (baseline bytes are retained until undo or replaced by a later edit). |

---

## 4. Configuration

No user-facing configuration is required for v0.1.

Future optional configuration (not required by this SRS):

- `tools.undo.max_snapshot_bytes` to cap retained baseline size.

---

## 5. Verification Requirements

### 5.1 Unit / Integration Tests

| Test ID | Description |
| --- | --- |
| T-UNDO-01 | `Undo` restores original bytes after an `Edit` that empties a file. |
| T-UNDO-02 | `Undo` removes a file created by an `Edit` (new file created via non-match LP1 ops). |
| T-UNDO-03 | `Undo` returns the exact empty-state error string when no applied edits exist. |
| T-UNDO-04 | `Undo` refuses and makes no changes if a target file was modified after the edit (hash mismatch). |
| T-UNDO-05 | `Undo` is atomic across multiple files (simulated failure mid-apply rolls back). |
| T-UNDO-06 | After `Undo`, a subsequent `Edit` still requires an explicit `Read` (stale-file protection remains effective). |

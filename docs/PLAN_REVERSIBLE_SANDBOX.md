# Reversible Sandbox: Undo as Security

**Status**: Draft v1
**Date**: 2026-02-17
**Motivation**: The current approval model gates every file write with a user prompt. This causes permission fatigue — users rubber-stamp approvals to maintain flow, defeating the security purpose. Reversibility is a stronger guarantee than permission: if every change can be atomically undone, the user doesn't need to approve each one individually.

---

## Principle

```
Permission model:  "May I write to foo.rs?"  → user approves/denies each time
Reversibility model: "I wrote to foo.rs."    → user reviews all changes, commits or discards
```

Irreversible operations (network, `git push`, process side effects) still
require approval. Reversible operations (file writes within the sandbox) do
not — because they can be undone.

**The threat model splits on reversibility, not on operation type.**

---

## Design

### Two-layer rollback

**Layer 1: Git checkpoint (tracked files)**

The workspace is a git repo. Git's index is the overlay.

```
Task start:
  git stash create → stash_ref   (snapshot working tree + index, no UI)
  — or —
  git commit --allow-empty -m "forge:checkpoint" on detached HEAD

Agent runs freely:
  Write/Edit/ApplyPatch execute without approval prompts
  File sandbox deny patterns still enforced (.ssh/, .env, *.pem, etc.)
  Changes are real on disk — the agent sees its own writes immediately

Task end:
  TUI shows diff summary against checkpoint
  User chooses: [C]ommit  [D]iscard  [R]eview file-by-file

Discard:
  git checkout -- .  &&  git clean -fd   (restore to checkpoint)
  — or —
  git stash pop stash_ref
```

**Layer 2: Change journal (untracked/out-of-git files)**

Files outside git (build artifacts, dotfiles, new untracked files) are not
covered by git rollback. The change journal records original contents before
each write, enabling userspace rollback.

Forge already has `tools/src/change_recording.rs` for tracking file changes.
Extend it to:

1. Before writing to a file not tracked by git, save the original content
   (or record "did not exist" for new files)
2. On discard, restore originals and delete newly created files
3. On commit, discard the journal (changes are accepted)

```rust
struct ChangeJournal {
    entries: Vec<ChangeEntry>,
}

enum ChangeEntry {
    /// File existed before — original content saved for rollback.
    Modified { path: PathBuf, original: Vec<u8> },
    /// File did not exist — delete on rollback.
    Created { path: PathBuf },
    /// File was deleted — original content saved for rollback.
    Deleted { path: PathBuf, original: Vec<u8> },
}
```

### Operation classification

| Operation | Reversible? | Mechanism | Approval |
|-----------|-------------|-----------|----------|
| Write/Edit/ApplyPatch (git-tracked) | Yes | Git checkpoint | None (sandbox deny patterns still enforced) |
| Write/Edit (untracked, in sandbox) | Yes | Change journal | None |
| Write (outside sandbox roots) | N/A | Blocked | Blocked by sandbox |
| Write (to deny-pattern path) | N/A | Blocked | Blocked by sandbox |
| Read/Search/Glob | N/A (read-only) | N/A | Existing approval model (Default mode) |
| Run (trusted, read-only) | N/A | N/A | Auto-approve (per PLAN_RUN_REFACTOR) |
| Run (side-effecting) | **No** | N/A | User approval required |
| Run (unknown program) | **No** | N/A | User approval required |
| git push / remote ops | **No** | N/A | User approval required |

### Checkpoint lifecycle

```
┌─────────────────────────────────────────────────────┐
│                    User sends prompt                 │
├─────────────────────────────────────────────────────┤
│  1. Create checkpoint (git stash create + journal)  │
│  2. Agent executes (writes are free, runs are gated)│
│  3. Agent completes or user interrupts              │
├─────────────────────────────────────────────────────┤
│  4. TUI shows change summary:                       │
│     "Modified 12 files, created 3, deleted 1"       │
│     [C]ommit  [D]iscard  [R]eview                   │
├─────────────────────────────────────────────────────┤
│  Commit: discard checkpoint, changes are permanent  │
│  Discard: restore checkpoint, all changes undone    │
│  Review: show per-file diff, then commit/discard    │
└─────────────────────────────────────────────────────┘
```

### Incremental checkpoints (multi-turn)

A single task may span multiple turns. The checkpoint covers the entire task,
not individual turns. The user can also create manual checkpoints:

- `/checkpoint` — create a named savepoint within the current task
- `/rollback` — roll back to the most recent checkpoint
- `/rollback <name>` — roll back to a named checkpoint

Checkpoints stack: rolling back to checkpoint N discards all changes after N.

### What about concurrent file access?

The agent's writes are real on disk (no CoW layer). If the user edits a file
in their editor while the agent is working, both see the same filesystem.
This is the current behavior — no change. The checkpoint only guarantees
rollback to the state at checkpoint creation, not isolation from concurrent
edits.

---

## UX

### Change summary (post-task)

```
┌─ Task Complete ──────────────────────────────────────┐
│                                                      │
│  Modified:  src/lib.rs, src/config.rs, Cargo.toml   │
│  Created:   src/new_module.rs, tests/new_test.rs    │
│  Deleted:   src/old_module.rs                        │
│                                                      │
│  +142 -38 across 6 files                            │
│                                                      │
│  [C]ommit  [D]iscard  [R]eview                      │
└──────────────────────────────────────────────────────┘
```

### Review mode

Pressing `[R]eview` enters a file-by-file diff view (reuse existing diff
rendering from `tui/src/diff_render.rs`). Each file can be individually
accepted or rejected. Partially accepting creates a selective commit.

### Discard confirmation

Discarding is destructive (throws away agent work). Require a confirmation:
```
Discard all changes from this task? (y/n)
```

### Status line integration

While a task is active with a checkpoint, the status line shows:
```
[checkpoint: 6 files changed]
```

This reminds the user that changes are staged, not committed.

---

## Interaction with Existing Systems

### Sandbox deny patterns

Deny patterns (`.ssh/`, `.env`, `*.pem`, etc.) are still enforced. The
reversible sandbox removes the approval prompt for *allowed* writes, not
for *denied* writes. Denied writes are still hard-blocked.

### `ObservedRegion` edit proof

The `ObservedRegion` mechanism (hash-based stale edit prevention) is
orthogonal. It prevents the agent from editing file regions it hasn't read.
The reversible sandbox prevents the user from losing changes they don't want.
Both coexist.

### Crash recovery

If Forge crashes mid-task, the checkpoint exists in git (stash ref or
detached HEAD commit) and in the change journal (SQLite or file-based).
On restart, Forge detects the uncommitted checkpoint and prompts:

```
Forge was interrupted during a task. 6 files were modified.
[C]ommit changes  [D]iscard and restore  [R]eview
```

This reuses the existing crash recovery flow (`engine/src/app/persistence.rs`)
with checkpoint awareness.

### Tool journal integration

The tool journal already records every tool call with full arguments and
results. The change journal adds a "checkpoint_id" column linking file
operations to their checkpoint, enabling rollback at the journal level.

---

## What This Does NOT Cover

- **Run command side effects**: Process execution, network calls, and system
  state changes are not reversible. These still require approval per
  PLAN_RUN_REFACTOR.md.

- **Database mutations**: If a tool writes to SQLite or other databases within
  the sandbox, the change journal captures the file-level change but not the
  semantic database change. Rollback restores the file, which may corrupt an
  in-progress database. Mitigation: exclude database files from the
  reversible sandbox (add `*.db`, `*.sqlite` to deny patterns or a new
  "no-auto-write" pattern list).

- **Large binary files**: The change journal stores original file contents.
  For large binaries (build artifacts, images), this could consume significant
  memory/disk. Mitigation: size cap on journaled files (e.g., 10MB). Files
  above the cap require approval as today.

- **Filesystem metadata**: Permissions, timestamps, xattrs are not captured
  by the change journal. Git rollback restores content but not metadata.
  Acceptable for the common case (source code editing).

---

## Platform Notes

### Git dependency

This design requires the workspace to be a git repository. For non-git
workspaces, only the change journal layer is available. The UX degrades
to "journal-only rollback" which is less robust (no atomic rollback of
tracked files, relies on per-file restore).

### Windows

`git stash` and `git checkout` work on Windows. The change journal is
pure Rust (no OS-level FS support needed). No OverlayFS, no elevated
permissions, no platform-specific filesystem APIs.

### macOS/Linux

Same as Windows. No OverlayFS, no APFS snapshots, no root. Pure git +
userspace journal.

---

## Migration

### Phase 1: Change journal infrastructure

- Extend `change_recording.rs` to save original file contents before writes
- Add `ChangeJournal` type with `Modified/Created/Deleted` entries
- Add `rollback()` method that restores all originals
- Unit tests for journal + rollback correctness

### Phase 2: Git checkpoint integration

- Implement `create_checkpoint()` using `git stash create`
- Implement `discard_checkpoint()` using `git checkout -- . && git clean -fd`
- Implement `commit_checkpoint()` (discard the stash ref)
- Handle edge cases: dirty index, merge conflicts, submodules

### Phase 3: Wire into approval flow

- Remove approval prompts for Write/Edit/ApplyPatch when checkpoint is active
- Sandbox deny patterns still enforced (hard block, not approval)
- Add post-task change summary UI
- Add `[C]ommit / [D]iscard / [R]eview` interaction

### Phase 4: Incremental checkpoints

- `/checkpoint` and `/rollback` commands
- Named checkpoint support
- Checkpoint stack with rollback-to-N semantics

---

## Required Tests

| Category | Test | Purpose |
|----------|------|---------|
| **Git checkpoint** | Create checkpoint, modify 3 files, discard → all restored | Basic rollback |
| **Git checkpoint** | Create checkpoint, add new file, discard → new file deleted | Untracked file cleanup |
| **Git checkpoint** | Create checkpoint, delete file, discard → file restored | Deletion rollback |
| **Change journal** | Write to non-git file, rollback → original restored | Journal-only rollback |
| **Change journal** | Create new non-git file, rollback → file deleted | Journal creation tracking |
| **Deny patterns** | Write to `.env` with checkpoint active → still blocked | Deny patterns not bypassed |
| **Crash recovery** | Kill process mid-task, restart → checkpoint detected | Recovery prompt shown |
| **Large files** | Write 50MB file → falls back to approval (size cap) | Memory/disk protection |
| **Concurrent edit** | User edits file during task, discard → user's edit also rolled back | Expected behavior documented |
| **Partial accept** | Review mode: accept 3/5 files → selective commit | Partial rollback works |
| **Nested checkpoints** | Checkpoint A, modify, checkpoint B, modify, rollback B → A state | Stack semantics |
| **Non-git workspace** | No .git directory → journal-only mode, degraded UX | Graceful fallback |

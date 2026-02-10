# Parallel Tool Execution via RwLock

## Problem

Tool execution is serial. `spawn_next_from_queue` pops one `ToolCall` at a time
from `ToolQueue.queue: VecDeque<ToolCall>`, spawns it via `SpawnedTool::spawn`,
waits for completion, then pops the next. When an LLM returns a batch of 5 reads,
they run sequentially despite having zero mutual interference.

## Design

Add an `Arc<RwLock<()>>` to the tool loop. Nothing behind the lock — it's a pure
concurrency gate.

- **Read-only tools** (`is_side_effecting() == false`): acquire **read lock** → run concurrently.
- **Side-effecting tools** (`is_side_effecting() == true`): acquire **write lock** → run exclusively.

Multiple readers proceed simultaneously. A writer waits for all readers to finish,
then holds exclusive access. This is standard `tokio::sync::RwLock` semantics.

### Classification

Existing `ToolExecutor::is_side_effecting(&self, args: &Value) -> bool` already
provides the correct classification. No new trait method needed.

| Tool | Side-Effecting | Lock | Rationale |
|------|:-:|:-:|-----------|
| Read | no | read | File content, no mutation |
| Glob | no | read | Directory listing |
| Search | no | read | Content search |
| Recall | no | read | Memory retrieval |
| WebFetch | no | read | HTTP GET, no local state |
| Git Status | no | read | Read-only query |
| Git Diff | no | read | Read-only query |
| Git Log | no | read | Read-only query |
| Git Show | no | read | Read-only query |
| Git Blame | no | read | Read-only query |
| Git Branch | no | read | List only (no create/delete) |
| Edit | **yes** | write | Modifies files |
| Write | **yes** | write | Creates/overwrites files |
| ApplyPatch | **yes** | write | Modifies files |
| Run | **yes** | write | Arbitrary shell execution |
| Memory | **yes** | write | Persists facts |
| Git Add | **yes** | write | Stages files |
| Git Commit | **yes** | write | Creates commits |
| Git Checkout | **yes** | write | Switches branches |
| Git Stash | **yes** | write | Stash mutations |
| Git Restore | **yes** | write | Discards changes |

**Run is always exclusive.** Shell commands can do anything — marking them
parallel-safe is incorrect regardless of what Codex does.

## State Machine Changes

Current flow (serial):

```
Processing(queue) → pop one → Executing(active) → complete → Processing(queue) → ...
```

New flow (parallel):

```
Processing(queue) → partition batch → spawn all as tokio tasks with lock → collect results → commit batch
```

### Execution Steps

1. **Partition**: Split `ToolQueue` into readers (read-lock) and writers (write-lock)
   based on `is_side_effecting`.
2. **Spawn all**: Each tool gets its own `tokio::spawn`. Inside the task, acquire
   the appropriate lock guard before calling `execute`.
3. **Collect**: `join_all` or `FuturesUnordered` to gather `CompletedTool` results.
4. **Commit**: Feed results back to the batch in original call order.

### ToolLoopPhase

`Executing(ActiveExecution)` currently holds a single `SpawnedTool`. Two options:

**Option A — Vec of spawned tools**: `Executing` holds `Vec<SpawnedTool>`. Poll all,
drain events from each, complete when all finished. Minimal state machine change.

**Option B — Flatten to Processing**: Remove `Executing` variant entirely. Spawn all
tools from `Processing`, collect via `JoinSet`, return to `Processing` when done.
Simpler but loses per-tool event streaming granularity.

Recommend **Option A** — preserves existing event streaming (progress, output lines)
and abort semantics per-tool.

## Implementation Sketch

```rust
// New field on App (or ToolLoopState)
parallel_lock: Arc<RwLock<()>>,

// In spawn_batch (replaces spawn_next_from_queue for parallel batches)
fn spawn_batch(&mut self, batch_id: ToolBatchId, queue: ToolQueue) -> Result<ToolLoopPhase> {
    let mut spawned = Vec::new();

    for call in queue.queue {
        let lock = Arc::clone(&self.parallel_lock);
        let is_writer = self.tool_registry
            .lookup(&call.name)?
            .is_side_effecting(&call.arguments);

        let spawned_tool = SpawnedTool::spawn(call, |event_tx, abort_handle| async move {
            let _guard = if is_writer {
                Either::Right(lock.write().await)
            } else {
                Either::Left(lock.read().await)
            };
            // ... existing execute logic ...
        });
        spawned.push(spawned_tool);
    }

    Ok(ToolLoopPhase::Executing(ActiveExecution::batch(spawned)))
}
```

## Journal Integration

`tool_journal.mark_call_started` must be called for each tool before spawn.
`mark_call_completed` when each individual tool finishes. Batch commit happens
after all tools complete — same as today, just with N completions instead of 1.

## Edge Cases

- **Approval flow unchanged**: Side-effecting tools still go through approval
  before entering `Processing`. The lock is orthogonal to approval.
- **Abort**: Each `SpawnedTool` retains its own `AbortHandle`. Aborting one tool
  doesn't affect others in the batch.
- **Output capacity**: `remaining_capacity_bytes` is shared across the batch.
  Use `AtomicUsize` or collect results and check capacity sequentially after join.
- **Mixed batches**: A batch with 4 reads and 1 write spawns all 5. The 4 reads
  run concurrently, the write waits for them, then runs alone. Correct by
  construction — no special partitioning logic needed.

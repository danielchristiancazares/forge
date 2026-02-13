# Stream Responsiveness and Journal Correctness Refactor

## Summary

Forge can appear frozen when stream backlog is high because stream event draining and journal writes happen on the same path as UI input progression. This refactor makes responsiveness and crash consistency both first-class invariants.

The design is IFA-first:

1. Persistence proofs are produced only at the boundary.
2. Core state transitions consume those proofs.
3. Failure is fail-closed, explicit, and non-silent.

## Goals

1. Input remains responsive during heavy streaming backlogs.
2. No stream/tool state transition is applied unless persistence succeeded.
3. Stream and tool journals remain crash-recoverable and order-safe.
4. Persistence failures move the app into explicit blocked safety state.

## Non-goals

1. Minimal diff.
2. Backward compatibility for internal engine interfaces.
3. Best-effort operation when persistence is unhealthy.

## Invariants (IFA mapping)

1. Persist-before-apply:
   Core must not call `StreamingMessage::apply_event` until journal persistence for that event is acknowledged.
2. Ordered application:
   Event sequence must be monotonic and contiguous when applied.
3. Single writer authority:
   Only one component owns stream/tool journal writes.
4. Fail-closed persistence:
   If persistence proof cannot be produced, new streaming/tool progress is blocked.
5. Commit protocol ordering:
   `seal -> history persisted -> commit/prune` remains enforced.

## Proposed Architecture

### 1) Journal Writer Actor

Add `engine/src/journal_writer.rs` with:

1. Bounded command channel.
2. Single task that owns journal write side effects.
3. Ack channel carrying persistence result and proof token.
4. Health state tracking (`Healthy`, `Degraded`, `Blocked`).

Core types:

1. `JournalWriteCmd`:
   stream append text/error/done, tool call start, tool args append, seal, discard, commit/prune.
2. `JournalAck`:
   `{ seq, result }`.
3. `PersistedProof`:
   Unforgeable token minted only by actor on successful persistence.
4. `JournalHealth`:
   Snapshot for UI and command guards.

### 2) Two-phase stream processing

Refactor `App::process_stream_events` into:

1. Ingest phase:
   read provider events, sanitize/coalesce, enqueue `JournalWriteCmd`.
2. Ack phase:
   apply only acked events whose `PersistedProof` exists.

No direct in-loop SQLite write calls in stream path.

### 3) Deterministic budgets

Keep count-based budgets (not wall-clock core gating):

1. `MAX_INCOMING_EVENTS_PER_TICK`.
2. `MAX_PERSISTED_APPLIES_PER_TICK`.
3. Prioritize terminal/control events over large text delta floods.

### 4) Persisted envelopes

Introduce explicit envelope states:

1. `ReceivedEvent { seq, event }`.
2. `PersistedEvent { seq, event, proof }`.

Application consumes `PersistedEvent` only.

### 5) Fail-closed blocked state

Add operation safety state:

1. `OperationState::PersistenceBlocked(PersistenceBlockedState)`.

When entered:

1. Active stream/tool work is aborted.
2. New stream/tool starts are denied.
3. User sees clear recovery/reset guidance.

## Engine Changes

## Files

1. `engine/src/journal_writer.rs` (new).
2. `engine/src/lib.rs`:
   add module, actor handles, health snapshot APIs, busy-state messaging updates.
3. `engine/src/init.rs`:
   construct writer actor and channels during `App::new`.
4. `engine/src/streaming.rs`:
   remove inline journal writes from event loop; switch to enqueue + ack apply.
5. `engine/src/state.rs`:
   add blocked state type and optional stream envelope state carrier as needed.
6. `engine/src/commands.rs`:
   preserve `/cancel` and `/clear` semantics with new blocked state.
7. `engine/src/persistence.rs`:
   integrate cleanup/commit flow with writer actor ownership.
8. `engine/README.md`:
   update streaming and journal architecture sections.

## Behavioral policy decisions

1. Persistence model:
   writer actor with bounded queues.
2. Failure policy:
   fail closed.
3. Backpressure:
   bounded channels; if full, stop ingest for current tick and yield.
4. Ordering:
   sequence IDs monotonic per stream session.

## API and type changes (internal)

1. New:
   `JournalWriterHandle`, `JournalWriteCmd`, `JournalAck`, `PersistedProof`, `JournalHealth`.
2. New:
   `PersistenceBlockedState`.
3. Modified:
   stream processing path no longer mutates journals directly.

## Failure modes and handling

1. Actor channel closed:
   transition to `PersistenceBlocked`.
2. Journal write error (retryable):
   actor retries per explicit boundary policy.
3. Journal write error (non-retryable/exhausted):
   emit failed ack; core transitions to `PersistenceBlocked`.
4. Out-of-order ack:
   reject and block (invariant contradiction).
5. Full persist queue:
   stop ingest this tick, keep app responsive, retry next tick.

## Testing Plan

### Unit

1. Event application requires persisted proof.
2. Out-of-order ack rejection.
3. Control-event priority over delta floods.
4. Fail-closed transition on unrecoverable persistence failure.

### Integration

1. High backlog responsiveness:
   input actions (`q`, `f`, command mode) remain usable.
2. Tool call followed by stream error:
   no freeze, no unsafe tool continuation.
3. Crash recovery idempotency with persisted sequence chain.
4. Blocked state prevents new stream/tool starts until reset path.

### Stress/property

1. Random interleavings preserve sequence monotonicity.
2. Bounded queue memory remains bounded under load.

## Rollout

1. Land as coordinated breaking internal refactor.
2. Keep plan as source-of-truth document for follow-up implementation tasks.
3. Update docs and architecture references in same series.

## Acceptance Criteria

1. User input remains responsive under synthetic stream flood.
2. No event reaches core apply path without persistence proof.
3. Persistence failures are explicit, safe, and non-silent.
4. Recovery semantics remain idempotent and ordered.


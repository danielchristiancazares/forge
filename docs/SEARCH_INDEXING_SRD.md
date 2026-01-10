# Local Search Indexing, Change Tracking, and Fuzzy Fallback

## Software Requirements Document

**Version:** 1.6
**Date:** 2026-01-10
**Status:** Implementation-Ready
**Baseline code reference:** `forge-source.zip`
**Normative baseline spec:** `docs/LOCAL_SEARCH_SRD.md` (v1.0, 2026-01-08)

---

## 0. Change Log

| Version | Date       | Notes                                                                                                                                                                                                                                                                                                                                                                                    |
| ------- | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1.0     | 2026-01-08 | Initial concept drafted (indexing + fuzzy fallback)                                                                                                                                                                                                                                                                                                                                      |
| 1.1     | 2026-01-08 | Expanded ARB-grade lifecycle, safety, observability, and tests                                                                                                                                                                                                                                                                                                                           |
| 1.2     | 2026-01-09 | **Remediated 16 identified gaps**: tool/schema alignment, deterministic truncation/order, backend execution scaling, exit-code classification, fuzzy-level alignment, stale-file safety, ignore-file invalidation, stats determinism, COMPLETE criteria, multi-root/subpath, path normalization, persistence permissions, context semantics, output decoding, and budget decision policy |
| 1.3     | 2026-01-09 | **Added normative state machine**: transition table with precedence rules, uncertain_reason enum, reference SQLite schema, short-pattern exclusion rule, Unicode normalization requirements, rename/move triggers, dirty queue ordering, Bloom filter parameter storage |
| 1.4     | 2026-01-09 | **Background indexer lifecycle**: initial build runs asynchronously on launch (not blocking searches), separated initial build budget from per-request incremental maintenance budget, workspace root configuration, graceful shutdown semantics, indexer thread priority/throttling |
| 1.5     | 2026-01-10 | **Implementation-readiness remediation (40+ findings)**: Split path normalization (Appendix C) for key vs order with NFC/UTF-8 bytewise comparison; fixed partial-index exclusion contradiction (§4.2 vs §11.1); clarified Index Root selection and subpath reuse (§5.2); added truncation detection mechanism (§3.2); added backend ordering control (FR-LSI-ORD-04); added fuzzy fallback total timeout budget (§9.3); fixed uncertain_reason assignments for DISABLED causes; added SQLite concurrency requirements (WAL, read-only connections); added Appendix F (tokenizer/Bloom algorithm spec); added Appendix G (backend file-list ingestion); added P4 precedence rule for multi-trigger uncertain_reason; expanded verification tests (§15) |
| 1.6     | 2026-01-10 | **Specification hardening (28 findings remediated)**: Fixed Event definition formatting (§1.3); resolved truncation semantics inconsistency (FR-LSI-TRUNC-01 vs FR-LSI-CTX-02); added determinism mandate (FR-LSI-ORD-05); added order_root selection algorithm (§3.4); aligned build budget transition BUILDING→UNCERTAIN (§4.2.2); added MAINT_BUDGET_EXCEEDED and WATCHER_OVERFLOW_DURING_BUILD transitions; added Index Key scheduling policy (§5.2.3) with query-time filters (glob[], case, fixed_strings, word_regexp removed from key); added cache_root selection algorithm (§5.3.1); clarified storage fallback policy (§5.3.2, §12.2); aligned Appendix C with Tool Executor sandbox; fixed Appendix E schema (removed glob_hash); fully specified Appendix F Bloom algorithm (xxHash64, bit layout, n determination, rounding); specified newline-in-path rejection policy (Appendix G); resolved §18.2 eligible file set (Forge standardizes eligibility); added tokenizer/Bloom verification tests (§15.6); added path edge case tests (§15.7); expanded stats field definitions with fallback_reason enum (§10.2) |

---

## 1. Introduction

### 1.1 Purpose

This SRD specifies requirements for **indexing**, **incremental change tracking**, and **optional fuzzy fallback** enhancements to Forge’s **Local Search tool** as defined in `docs/LOCAL_SEARCH_SRD.md`.

These enhancements are strictly **performance and resiliency features** and MUST NOT change the tool’s request schema or required response schema. They MUST preserve correctness and determinism under scrutiny.

### 1.2 Scope

#### In Scope

* Index modes, lifecycle, storage backends, resource limits, and eviction
* Index safety model (“coverage” and state machine) for safe candidate exclusion
* Incremental change tracking (watchers, dirty sets, reconcile scans)
* Deterministic ordering and truncation equivalence with/without indexing
* Backend execution scaling (large candidate lists, chunking, deterministic merge)
* Backend exit classification and error handling
* Optional fuzzy fallback (using existing `fuzzy: 1–4` request semantics internally)
* Output extensions policy (optional `stats` block) and determinism classification
* Security constraints: sandbox, deny patterns, symlink/junction defense
* Explicit rule: Search/indexing MUST NOT satisfy “file has been read” gating for edits

#### Out of Scope

* Remote/network search
* Semantic search (handled by CodeQuery)
* Changing `Search` tool request JSON schema
* Changing required response fields from `docs/LOCAL_SEARCH_SRD.md`
* UI rendering behavior

### 1.3 Definitions

| Term                    | Definition                                                                                                                      |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| **Tool**                | The Forge tool named **`Search`** specified by `docs/LOCAL_SEARCH_SRD.md`.                                                      |
| **Eligible file set**   | The set of files that would be searched for a given request if executed without indexing (baseline semantics).                  |
| **Event**               | One element of `matches[]` in the response. `Event = MatchEvent \| ContextEvent` where type is `"match"` or `"context"`. Both event types count equally toward `max_results` per FR-LSI-TRUNC-01 and FR-LSI-CTX-02. |
| **Truncation boundary** | The exact event index at which `max_results` causes the tool to stop emitting events and set `truncated=true`.                  |
| **Index**               | A persisted or in-memory structure used to accelerate candidate selection and/or file enumeration.                              |
| **Candidate set**       | Files selected for backend scanning for a given request.                                                                        |
| **Coverage**            | A guarantee that the index's file catalog fully covers the eligible file set for the current request/config.                    |
| **Index Safety State**  | A state machine indicating whether the index may be used to **exclude** files (prevent false negatives).                        |
| **Dirty set**           | Files and/or subtrees flagged for reindexing due to change signals or uncertainty.                                              |
| **Reconcile scan**      | A bounded scan that restores coverage after watcher overflow/failure or other uncertainty.                                      |
| **Fuzzy fallback**      | Optional behavior: if a literal search yields zero matches (without truncation/timeout), retry with increasing `fuzzy` levels.  |
| **Order root**          | The directory used as the base for computing relative paths for deterministic ordering (see §3.4).                              |

### 1.4 References

| Document                    | Description                                                           |
| --------------------------- | --------------------------------------------------------------------- |
| `docs/LOCAL_SEARCH_SRD.md`  | **Authoritative** Search tool schema and baseline semantics           |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool executor requirements: sandboxing, timeouts, output sanitization |
| `docs/TOOLS.md`             | Tool configuration and stale-file protection overview                 |
| RFC 2119 / RFC 8174         | Requirement keywords                                                  |

### 1.5 Requirement Keywords

RFC 2119 / RFC 8174 keywords apply when capitalized (MUST, SHOULD, etc.).

---

## 2. Tool Identity, Contract, and Compatibility

### 2.1 Tool Identity and Aliasing

**FR-LSI-BASE-01:** This SRD applies to the tool named **`Search`** as specified in `docs/LOCAL_SEARCH_SRD.md` (**FR-LS-01..07** in that document).

**FR-LSI-BASE-02:** The tool MUST retain the baseline aliases defined in `docs/LOCAL_SEARCH_SRD.md` (`search`, `rg`, `ripgrep`, `ugrep`, `ug`).

**FR-LSI-BASE-03 (Optional alias):** The system MAY additionally advertise an alias `search_files` **only if** it is a strict alias of `Search` with identical request/response schema and identical semantics.

### 2.2 Request Schema Compatibility

**FR-LSI-SCHEMA-REQ-01:** The request schema MUST remain exactly as specified in `docs/LOCAL_SEARCH_SRD.md` (**FR-LS-02**). No new request fields may be introduced by this SRD.

### 2.3 Response Schema Compatibility and Extension Policy

Baseline response fields (per `docs/LOCAL_SEARCH_SRD.md` **FR-LS-05**) include:
`pattern`, `path`, `count`, `matches[]`, `truncated`, `timed_out`, optional `exit_code`, optional `stderr`, and `content`.

**FR-LSI-SCHEMA-RESP-01:** The response payload MUST include all required baseline fields and preserve their types.

**FR-LSI-SCHEMA-RESP-02 (Additive diagnostics):** The response MAY include an additional top-level object `stats` **only if** enabled by configuration (CFG-LSI-STAT-01) and only if consumers tolerate forward-compatible additive fields.

**FR-LSI-SCHEMA-RESP-03:** If `stats` is present, it MUST include `stats_version` (integer) and MUST be documented as forward-compatible and strictly optional.

**FR-LSI-SCHEMA-RESP-04:** No other additional top-level keys are permitted.

---

## 3. Determinism, Ordering, and Truncation Semantics

This section is normative because indexing and chunking can otherwise create subtle semantic drift.

### 3.1 Deterministic Event Ordering Model

**FR-LSI-ORD-01:** The tool MUST define and implement a deterministic ordering of emitted events (`matches[]`) for a given request and filesystem state.

**FR-LSI-ORD-02:** The deterministic ordering MUST be stable across platforms and across runs (same inputs + same filesystem state ⇒ same `matches[]` order).

**FR-LSI-ORD-03 (Event ordering rule):** Events MUST be ordered by:

1. `path_sort_key` computed via `NormalizeRelPathForOrder(path, index_root)` (see Appendix C) — bytewise lexicographic comparison
2. `line_number` ascending (numeric)
3. Within the same file and line number, preserve backend-emitted ordering between `"context"` and `"match"` events **as parsed**.

**FR-LSI-ORD-04 (Backend ordering control):** If the backend emits results out-of-order (common with parallel search), the tool MUST either:
* Run the backend in an ordered output mode (e.g., single-threaded, backend sort flags), OR
* Perform tool-side stable sorting of all events before truncation, using the ordering model above.

**FR-LSI-ORD-05 (Determinism is mandatory):** If neither backend ordering nor tool-side sorting can complete within timeout budget, the tool MUST still preserve deterministic ordering among emitted events by:
1. Forcing the backend into a deterministic output mode (even if slower), OR
2. Sorting whatever events were parsed before timeout.

The tool MAY time out (`timed_out=true`) rather than emit events in nondeterministic order. Under no circumstances SHOULD events be emitted in an order that differs from FR-LSI-ORD-03 across runs with identical inputs and filesystem state.

> Rationale: `docs/LOCAL_SEARCH_SRD.md` requires structured match records but does not define ordering; this SRD tightens that to ensure determinism and truncation equivalence.

### 3.2 Deterministic Truncation Equivalence

Baseline: **FR-LS-06:** stop after `max_results` match/context events and mark `truncated=true`.

**FR-LSI-TRUNC-01:** The tool MUST stop emitting events after exactly `max_results` events (match + context) and MUST set `truncated=true` if and only if additional events would have been emitted under the same ordering model.

**FR-LSI-TRUNC-01a (Detection mechanism):** To determine whether additional events exist:
1. The tool MUST attempt to observe **one additional event** beyond `max_results` (buffered, not emitted).
2. If an additional event exists: set `truncated=true`.
3. If no additional event exists (search exhausted): set `truncated=false`.
4. If the tool cannot safely observe +1 (timeout/budget constraint): set `truncated=true` (conservative) and record in `stats` if enabled.

**FR-LSI-TRUNC-02:** When indexing, candidate filtering, or chunked backend execution is used, the tool MUST preserve the exact same truncation boundary as it would under a non-indexed execution using the same deterministic ordering model.

### 3.3 Stable-Filter Constraint for Candidate Exclusion

**FR-LSI-STABLE-01 (Stable filter):** If the index is used to exclude files from scanning, it MUST behave as a **stable filter** over the baseline per-file processing order implied by **FR-LSI-ORD-03**. It MUST NOT reorder files in a way that changes which events fall before the truncation boundary.

**FR-LSI-STABLE-02:** If stable-filter equivalence cannot be guaranteed for a given request (due to backend constraints, query type, or budget limits), the tool MUST bypass index-based exclusion for that call.

### 3.4 Order Root Selection (Normative)

The **order root** is the directory used as the base for computing relative paths for deterministic ordering. It is used by `NormalizeRelPathForOrder(path, order_root)` and MUST be determined consistently regardless of whether indexing is used.

**FR-LSI-ORD-ROOT-01 (Order root selection algorithm):**

1. If a configured Index Root (§5.2.1) contains the request `path`, use the **selected Index Root** as the order root.
2. If no Index Root contains the request `path` (indexing bypassed):
   * If the request `path` is absolute: use its canonicalized directory as order root (file's parent if path is a file, path itself if directory).
   * If the request `path` is relative or `.`: use the sandbox root (or current working directory if no sandbox) as order root.
3. The order root MUST be canonicalized via `NormalizePathForKey` (Appendix C) before use.

**FR-LSI-ORD-ROOT-02:** The order root selection MUST be deterministic: the same request `path` and configuration MUST always produce the same order root.

**FR-LSI-ORD-ROOT-03:** When indexing is bypassed, the order root selection ensures deterministic ordering is still possible. The ordering key computation (FR-LSI-ORD-03) uses `order_root` even when no index exists.

> **Rationale:** This ensures that two correct implementations produce identical deterministic orders and truncation boundaries for the same request, satisfying NFR-LSI-COR-01.

---

## 4. Indexing Overview

### 4.1 Goals and Non-Goals

**Goals**

* Reduce repeated work for repeated searches over the same tree.
* Avoid rescanning unchanged files.
* Provide safe candidate exclusion where provably correct (no false negatives).
* Degrade gracefully: correctness first, then performance.

**Non-goals**

* Do not change tool schema or core tool semantics.
* Do not store raw file contents by default.

### 4.2 Index Safety State Machine

Index-based exclusion is allowed only under a defined "coverage" guarantee.

**FR-LSI-SAFE-01:** The index subsystem MUST maintain an **Index Safety State** per Index Key (see 5.2):

* `ABSENT` — No index exists for this key
* `BUILDING` — Index construction in progress
* `COMPLETE` — Index valid and coverage confirmed
* `UNCERTAIN` — Index may exist but coverage not guaranteed
* `CORRUPT` — Index data integrity failure detected
* `DISABLED` — Indexing explicitly disabled for this key

**FR-LSI-SAFE-02:** Index-based **candidate exclusion** MAY occur only when:

* State is `COMPLETE`, AND
* Index Key matches the effective request/config, AND
* The candidate strategy is proven superset-safe for the request (Section 7), AND
* Coverage is confirmed (Section 6).

**FR-LSI-SAFE-03:** In any other state, the tool MUST NOT exclude files using the index.

**FR-LSI-SAFE-04:** When a persisted index is opened, it MUST enter `UNCERTAIN` state with `uncertain_reason='OPEN_REQUIRES_VALIDATION'` until validated against current filesystem state, even if it was `COMPLETE` when last closed.

#### 4.2.1 State Transition Precedence Rules (Normative)

When multiple triggers fire simultaneously (during one tool call or maintenance tick), the implementation MUST resolve transitions deterministically:

**P1. State severity wins (most conservative).**
If multiple triggers propose different next states, choose the highest severity:

`DISABLED` > `CORRUPT` > `UNCERTAIN` > `BUILDING` > `ABSENT` > `COMPLETE`

**P2. If severities tie, trigger priority wins (fixed order).**
Within the same severity, apply the first matching trigger:

1. Hard disable (`index_mode=off`, policy prohibits persistence, sandbox/cache root invalid)
2. Corruption / integrity failures
3. Schema/version/key/tokenizer mismatch
4. Coverage invalidation (watcher overflow/failure, ignore-control change, deny/sandbox change, enumeration hiding errors)
5. Resource enforcement (eviction, memory cap, lock timeout)
6. Build lifecycle (start build, build complete)
7. Incremental maintenance (dirty queue processing)
8. Normal access bookkeeping (`last_accessed_at`)

**P3. Exclusion gate is strict.**
Regardless of state precedence, **candidate exclusion** is permitted only if the post-resolution state is `COMPLETE` and the per-request candidate strategy is superset-safe.

**P4. `uncertain_reason` selection under multi-trigger.**
When the next state is `UNCERTAIN` or `DISABLED` and multiple triggers propose different `uncertain_reason` values:
1. Use the `uncertain_reason` from the highest-priority trigger per P2.
2. If already in `UNCERTAIN` and a higher-priority trigger occurs, overwrite the reason; else retain the existing reason.
3. When transitioning to `COMPLETE`, `uncertain_reason` MUST be cleared to `NULL`.

#### 4.2.2 State Transition Table (Normative)

| Trigger | Current State(s) | Next State | Required Actions |
| ------- | ---------------- | ---------- | ---------------- |
| `index_mode=off` becomes effective | Any | `DISABLED` | Stop watchers; clear/ignore dirty queue; close DB handles; do not build/reconcile; never use index for exclusion or maintenance |
| Persistence not permitted (cannot validate cache root at all, e.g., no valid cache directory determinable) | Any (except `DISABLED`) | `DISABLED` | Same as above; set `uncertain_reason='PERSISTENCE_NOT_PERMITTED'`; record reason for diagnostics; see §12.2 for permission-only failures which use storage fallback instead |
| `DISABLED` cooldown active (`disabled_until_ms` set AND `now < disabled_until_ms`) | `DISABLED` | `DISABLED` | No maintenance; no exclusion |
| `DISABLED` cooldown expires (`disabled_until_ms` set AND `now ≥ disabled_until_ms`) AND `index_mode!=off` | `DISABLED` | `ABSENT` | Clear `disabled_until_ms`; treat as cold start; open index if exists; if open+integrity ok then immediately follow "open ok" trigger which sets `UNCERTAIN`; otherwise remain `ABSENT` |
| Index file missing at open | Any (except `DISABLED`) | `ABSENT` | No exclusion; background indexer will initiate build if enabled |
| Auto-mode threshold check: file count and size below thresholds (§FR-LSI-BG-05) | `ABSENT` | `DISABLED` | Set `uncertain_reason='BELOW_THRESHOLD'`; no build needed; searches run without index (acceptable for small repos) |
| DB lock cannot be acquired within `index_lock_timeout_ms` | Any (except `DISABLED`) | *(no state change)* | Treat index as unavailable for this call only: no exclusion, no maintenance; run non-excluding execution |
| Schema mismatch OR `index_key_hash` mismatch OR tokenizer/casefold/unicode-normalization mismatch | Any (except `DISABLED`) | `BUILDING` | Quarantine/delete old index for that key; clear dirty queue; disallow exclusion; rebuild from scratch within budget; if rebuild cannot start, stay non-excluding |
| Integrity check fails (sqlite corruption, malformed pages) | Any (except `DISABLED`) | `CORRUPT` | Disallow exclusion immediately; quarantine/delete DB; clear dirty queue; schedule rebuild (→ `BUILDING`) when possible; never read corrupt structures for candidate logic |
| Background indexer initiates build (on launch per §4.3, or rebuild after corruption/mismatch) | `ABSENT`, `CORRUPT`, `UNCERTAIN`, `COMPLETE` | `BUILDING` | Disallow exclusion; run full eligible-file enumeration on background thread; populate catalog + tokens; write transactionally. Search calls continue unblocked (§FR-LSI-BG-10) |
| Build completes successfully (full enumerate, no hiding errors) | `BUILDING` | `COMPLETE` | Set `coverage_epoch++`; clear `uncertain_reason`; clear dirty queue; enable exclusion (subject to strategy safety) |
| Initial build budget exceeded (time, file count, or byte limit per §11.1) | `BUILDING` | `UNCERTAIN` | Persist checkpoint/partial index; set `uncertain_reason='BUILD_BUDGET_EXCEEDED'`; disallow candidate exclusion; partial index MAY be used for positive acceleration only (enumeration, diagnostics) |
| Build fails before any durable write (e.g., preflight failure, immediate hard error) | `BUILDING` | `ABSENT` | Ensure no persisted partial DB exists; disallow exclusion; proceed with non-excluding execution |
| Build fails after partial durable write OR write failure indicates potential DB inconsistency (disk full mid-tx, fs error) | `BUILDING` | `CORRUPT` | Quarantine/delete DB; disallow exclusion; record diagnostic; proceed non-excluding |
| Open index succeeds (schema+key ok, integrity ok) | `ABSENT`, `CORRUPT`, `UNCERTAIN`, `COMPLETE` | `UNCERTAIN` | Treat as not coverage-safe until validated: set `uncertain_reason='OPEN_REQUIRES_VALIDATION'`; start watchers if enabled; schedule reconcile/validation within budget; no exclusion until `COMPLETE` |
| Watcher starts successfully | `UNCERTAIN`, `COMPLETE` | *(no change)* | Update dirty queue; watcher success alone does not restore coverage |
| Watcher overflow/dropped events (when NOT building) | `COMPLETE`, `UNCERTAIN` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='WATCHER_OVERFLOW'`; enqueue affected subtree(s) dirty; require reconcile/full enumerate to regain `COMPLETE` |
| Watcher overflow/dropped events during initial build | `BUILDING` | `UNCERTAIN` | Build continues but cannot reach `COMPLETE`; set `uncertain_reason='WATCHER_OVERFLOW_DURING_BUILD'`; on build completion, remain `UNCERTAIN` and require reconcile to validate coverage |
| Watcher stopped unexpectedly / unavailable | `COMPLETE` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='WATCHER_DOWN'`; require reconcile strategy (bounded) or operate non-excluding until restored |
| Ignore-control file change detected (e.g., `.gitignore`, `.ignore`, global excludes) | `COMPLETE`, `UNCERTAIN` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='ELIGIBILITY_CHANGED'`; enqueue root/subtree dirty; require reconcile/full enumerate to regain `COMPLETE` |
| Deny/sandbox policy change affecting eligibility | `COMPLETE`, `UNCERTAIN` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='POLICY_CHANGED'`; purge denied entries; require reconcile/full enumerate to regain `COMPLETE` |
| Enumeration hiding errors encountered (permission/IO error that could hide files) | `COMPLETE`, `BUILDING`, `UNCERTAIN` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='ENUMERATION_ERROR'`; remain UNCERTAIN until a successful full enumerate occurs with zero hiding errors |
| Resource eviction of this Index Key (disk cap or max indexes) | Any (except `DISABLED`) | `ABSENT` | Delete/quarantine index; clear dirty queue; disallow exclusion; treat next use as cold start |
| Memory budget exceeded for in-memory index | Any (except `DISABLED`) | `DISABLED` (for key) | Set `uncertain_reason='MEMORY_BUDGET_EXCEEDED'`; set `disabled_until_ms` if cooldown configured; stop maintenance; disallow exclusion; proceed non-excluding |
| Dirty queue non-empty at start of call AND state is `COMPLETE` | `COMPLETE` | `COMPLETE` iff update succeeds within budget; else `UNCERTAIN` | Attempt incremental update. On success: remain `COMPLETE`. On failure: transition to `UNCERTAIN` and set `uncertain_reason='DIRTY_BACKLOG'`; disallow exclusion for that call |
| Inline maintenance budget exceeded (per-request budget per §11.2 exhausted) | `COMPLETE` | `UNCERTAIN` | Skip remaining maintenance for this call; set `uncertain_reason='MAINT_BUDGET_EXCEEDED'`; disallow exclusion; proceed with non-excluding execution; see §11.3 FR-LSI-BUD-02 for deterministic action policy |
| Dirty queue drained successfully AND `uncertain_reason='DIRTY_BACKLOG'` AND watcher health is good (no overflow since last epoch) | `UNCERTAIN` | `COMPLETE` | Promote to `COMPLETE` without full reconcile because coverage was not invalidated—only maintenance lagged. Clear `uncertain_reason` |
| Reconcile scan starts | `UNCERTAIN` | `UNCERTAIN` | Disallow exclusion; perform bounded scan updating catalog/tokens for affected scope |
| Reconcile completes successfully AND resolves the active `uncertain_reason` AND no hiding errors | `UNCERTAIN` | `COMPLETE` | Set `coverage_epoch++`; clear `uncertain_reason`; clear dirty queue; allow exclusion again subject to strategy safety |
| Reconcile exceeds budget OR finds hiding errors | `UNCERTAIN` | `UNCERTAIN` | Keep exclusion disabled; retain `uncertain_reason`; proceed non-excluding until a successful reconcile/full enumerate completes |
| Rename/move event with old+new path known | `COMPLETE` | `COMPLETE` (if applied) else `UNCERTAIN` | Enqueue old+new paths dirty; update catalog path fields deterministically; if cannot apply within budget, set `uncertain_reason='DIRTY_BACKLOG'` and transition to `UNCERTAIN` |
| Rename/move detected but mapping is ambiguous (no old path, uncertain scope) | `COMPLETE` | `UNCERTAIN` | Disallow exclusion; set `uncertain_reason='RENAME_UNCERTAIN'`; require reconcile to restore coverage and path-based eligibility |

### 4.3 Background Indexer Lifecycle

Index building MUST be asynchronous and MUST NOT block tool calls. This section specifies the background indexer's lifecycle.

#### 4.3.1 Startup and Initialization

**FR-LSI-BG-01:** When the tool system initializes, it MUST spawn a background indexer task (thread, async task, or equivalent) for each configured workspace root.

**FR-LSI-BG-02:** The background indexer MUST start immediately upon tool initialization, not upon first search request.

**FR-LSI-BG-03:** Workspace roots MUST be determined by configuration (`index_roots[]`) or by defaulting to the current working directory / sandbox root if not explicitly configured.

**FR-LSI-BG-04:** If `index_mode=off`, no background indexer SHALL be spawned.

**FR-LSI-BG-05:** If `index_mode=auto`, the background indexer MUST perform a lightweight pre-scan to determine if thresholds are met before committing to a full build:
1. Enumerate files up to `index_auto_threshold_files + 1` OR accumulate size up to `index_auto_threshold_bytes + 1`
2. If either threshold is exceeded, proceed with full build
3. If neither threshold is exceeded, set state to `DISABLED` with `uncertain_reason='BELOW_THRESHOLD'` and stop (no index needed)

**FR-LSI-BG-06 (Auto-mode re-evaluation):** When `index_mode=auto` and state is `DISABLED` with `uncertain_reason='BELOW_THRESHOLD'`, the system MUST re-evaluate thresholds:
1. On process restart (cold start always re-evaluates)
2. Periodically after every `index_auto_reeval_calls` search calls (default: 100), OR
3. When watcher indicates significant file-count/size growth (if watchers are enabled even below threshold)

If thresholds are now exceeded, transition from `DISABLED` to `ABSENT`, then initiate build per normal lifecycle.

#### 4.3.2 Non-Blocking Invariant

**FR-LSI-BG-10:** Search tool calls MUST NOT wait for index build completion. If the index is in `ABSENT`, `BUILDING`, `UNCERTAIN`, `CORRUPT`, or `DISABLED` state, the search MUST proceed without index-based exclusion.

**FR-LSI-BG-11:** The background indexer and search tool calls MUST be able to execute concurrently. The indexer MUST NOT hold exclusive locks that would block searches.

**FR-LSI-BG-12:** Search calls MAY read from a partially-built index only if the implementation guarantees snapshot isolation (reads see a consistent prior state, not in-progress writes). Otherwise, searches MUST ignore the index until `COMPLETE`.

**FR-LSI-BG-13 (SQLite concurrency requirements):** When `index_storage=sqlite`, the implementation MUST ensure non-blocking concurrent access:

1. **WAL mode:** Enable Write-Ahead Logging (`PRAGMA journal_mode=WAL`) unless the filesystem does not support it.
2. **Connection model:** Use a single write connection for the indexer and separate read-only connections for search queries.
3. **Transaction duration:** Write transactions MUST be short (per-batch or per-file), not spanning the entire build.
4. **Lock/timeout policy:** Read connections MUST use `PRAGMA busy_timeout` consistent with `index_lock_timeout_ms`. Write connections MUST retry briefly on lock contention, then skip/defer if unsuccessful.
5. **Checkpoint policy:** WAL checkpoints SHOULD be scheduled during idle periods, not during active searches.

#### 4.3.3 Build Phases

**FR-LSI-BG-20:** The initial build MUST proceed through the following phases:

1. **ENUMERATE**: Walk the eligible file set, respecting ignore rules and sandbox constraints
2. **TOKENIZE**: For each eligible file, extract n-grams and build Bloom filters
3. **PERSIST**: Write the completed index transactionally (atomic commit)
4. **ACTIVATE**: Transition state from `BUILDING` to `COMPLETE`

**FR-LSI-BG-21:** Phase progress SHOULD be observable via `stats` (if enabled) or logging, including:
* `build_phase`: current phase name
* `build_files_total`: total files to process (after ENUMERATE)
* `build_files_done`: files processed so far
* `build_elapsed_ms`: wall-clock time since build start

**FR-LSI-BG-22:** If any phase fails irrecoverably (IO error, resource exhaustion), the indexer MUST:
1. Transition to `CORRUPT` or `ABSENT` as appropriate per Section 4.2.2
2. Log the failure reason
3. NOT retry automatically unless configured (`index_retry_on_failure=true`)

#### 4.3.4 Throttling and Priority

**FR-LSI-BG-30:** The background indexer SHOULD run at reduced priority to avoid impacting interactive performance:
* On systems supporting thread priority: use below-normal priority
* On systems supporting nice values: use positive nice value (e.g., +10)
* On systems supporting IO priority: use idle/low IO class

**FR-LSI-BG-31:** The background indexer MUST support throttling via configuration:
* `index_build_max_cpu_percent`: maximum CPU utilization target (default: 50%)
* `index_build_throttle_ms`: sleep interval between file batches (default: 10ms)

**FR-LSI-BG-32:** Throttling MUST be adaptive: if the system is idle (no pending search requests for `index_idle_boost_ms`), the indexer MAY temporarily increase throughput.

#### 4.3.5 Graceful Shutdown

**FR-LSI-BG-40:** On shutdown signal (SIGTERM, SIGINT, or application exit), the background indexer MUST:
1. Stop accepting new work
2. Complete or abort the current file atomically (no partial file state)
3. Persist a recoverable checkpoint if supported (`index_checkpoint_on_shutdown=true`)
4. Release all locks and file handles

**FR-LSI-BG-41:** If `index_checkpoint_on_shutdown=true` and a checkpoint is persisted, the next startup MUST resume from the checkpoint rather than restarting the build. The checkpoint MUST include:
* Last completed file (by deterministic enumeration order)
* Coverage epoch at checkpoint time
* Dirty queue state

**FR-LSI-BG-42:** If shutdown occurs before any durable write, the index MUST remain `ABSENT` (not `CORRUPT`).

**FR-LSI-BG-43:** Shutdown MUST complete within `index_shutdown_timeout_ms` (default: 5000ms). If the indexer cannot quiesce in time, it MUST be forcibly terminated and the index treated as potentially corrupt on next startup.

#### 4.3.6 Restart and Resume

**FR-LSI-BG-50:** On startup, if a persisted index exists:
1. Open and validate integrity
2. Transition to `UNCERTAIN` with `uncertain_reason='OPEN_REQUIRES_VALIDATION'`
3. Start watchers (if enabled)
4. Schedule reconcile scan to validate coverage
5. On successful reconcile, transition to `COMPLETE`

**FR-LSI-BG-51:** If a checkpoint exists from a prior incomplete build:
1. Validate checkpoint integrity
2. Resume build from checkpoint position
3. If checkpoint is invalid or stale, discard and restart build

**FR-LSI-BG-52:** If the persisted index is corrupt or schema-mismatched, delete it and start a fresh build.

#### 4.3.7 Watcher Integration

**FR-LSI-BG-60:** Filesystem watchers MUST be started by the background indexer after initial enumeration begins, to capture changes that occur during build.

**FR-LSI-BG-61:** Changes detected by watchers during build MUST be queued in the dirty set and processed after the initial build completes.

**FR-LSI-BG-62:** If watcher overflow occurs during build, the build MUST complete but transition to `UNCERTAIN` instead of `COMPLETE`, requiring a reconcile pass.

---

## 5. Index Modes, Storage, Resource Limits, and Eviction

### 5.1 Index Modes

**FR-LSI-IDX-01:** The system MUST support `index_mode` values: `off`, `auto`, `on`.

**FR-LSI-IDX-02:** `index_mode=off` MUST disable all indexing and change tracking features (watchers, dirty sets, reconcile scans).

**FR-LSI-IDX-03:** `index_mode=auto` MUST enable indexing only if the eligible file set size exceeds configured thresholds (CFG-LSI-IDX-02).

**FR-LSI-IDX-04:** `index_mode=on` MUST enable indexing regardless of thresholds, subject to safety and resource budgets.

**FR-LSI-IDX-05:** Indexing MUST be advisory; if index is unavailable, unsafe, locked beyond budget, corrupted, or disabled, the tool MUST fall back to non-excluding execution without failing the tool call.

### 5.2 Index Key, Root Selection, and Scope

#### 5.2.1 Index Root Selection (Normative)

**FR-LSI-IDX-ROOT-01:** An **Index Root** is a directory from which an index is built. Index Roots are determined as follows:

1. If `index_roots[]` is configured and non-empty, use those paths as candidate Index Roots.
2. If `index_roots[]` is empty or not configured, use the current working directory (or sandbox root) as the sole Index Root.
3. Each Index Root MUST be validated against sandbox allowed roots at startup. Invalid roots (outside sandbox, denied patterns) MUST be rejected with a diagnostic.

**FR-LSI-IDX-ROOT-02 (Request-to-root mapping):** When a search request specifies `path`, the tool MUST select an Index Root:

1. Identify all configured Index Roots that **contain** the request `path` (request path is at or below the root).
2. Select the **deepest** (most specific) Index Root that contains the request path.
3. If no Index Root contains the request path, operate without indexing for that request.
4. The selected Index Root becomes the index identity for keying purposes.

**FR-LSI-IDX-ROOT-03 (Request path as subtree filter):** The request `path` is applied as a **subtree filter** to the selected Index Root's catalog — not as a separate index key component. This enables subpath reuse without rebuild.

#### 5.2.2 Index Key Definition

**FR-LSI-IDX-KEY-01:** The index MUST be keyed by:

* **Index Root** (canonicalized via `NormalizePathForKey`, see Appendix C) — NOT the request path
* Traversal-affecting request options: `hidden`, `follow`, `no_ignore`
* Backend identity and capability signature (e.g., `ugrep` vs `rg`, versions if relevant)
* Unicode normalization form used for tokenization (e.g., `NFC`, `NFD`, or `NONE`)
* Casefold strategy identifier (e.g., `ASCII`, `UNICODE_SIMPLE`) matching backend semantics
* Tokenizer identity (algorithm and parameters, e.g., n-gram size)
* Index schema version and implementation version

**FR-LSI-IDX-KEY-01a (Query-time filters — NOT part of key):** The following request options are applied as **query-time filters** against the index catalog and Bloom filters, NOT as Index Key components:

* `glob[]` — Applied as a path filter to the candidate set. Files not matching globs are excluded at query time.
* `fixed_strings`, `word_regexp` — Affect pattern interpretation. Bloom filter lookup normalizes the pattern accordingly.
* `case` — Selects the appropriate Bloom filter variant (`SENSITIVE` or `INSENSITIVE`) at query time.

> **Rationale:** Treating these as query-time filters prevents "index explosion" where varying `glob[]` patterns cause excessive distinct keys and cache churn. The index stores both case-sensitivity variants (per FR-LSI-BLOOM-04) to support any `case` value without rebuild.

**FR-LSI-IDX-KEY-02:** Index Key components MUST be stored explicitly (not as opaque blobs) to enable deterministic key comparison and mismatch detection. A key mismatch MUST trigger rebuild under the new key.

#### 5.2.3 Index Key Scheduling Policy

The background indexer (§4.3) operates per workspace root, but Index Keys include traversal options. This section defines which key(s) are built.

**FR-LSI-IDX-SCHED-01 (Canonical key per root):** The background indexer MUST build exactly **one canonical Index Key** per Index Root, using the following **default traversal options**:

| Option | Default Value | Rationale |
| ------ | ------------- | --------- |
| `hidden` | `false` | Most searches exclude hidden files; indexing hidden files is opt-in |
| `follow` | `false` | Following symlinks risks infinite loops and expands scope unpredictably |
| `no_ignore` | `false` | Respecting ignore files aligns with typical usage and reduces index size |

**FR-LSI-IDX-SCHED-02 (Configuration override):** The canonical traversal options MAY be overridden via configuration:

```toml
[tools.search]
index_default_hidden = false
index_default_follow = false
index_default_no_ignore = false
```

**FR-LSI-IDX-SCHED-03 (On-demand key variants):** When a search request uses traversal options that differ from the canonical key:

1. The tool MUST NOT build a new index on-demand (to prevent unbounded index proliferation).
2. The tool MUST bypass index-based exclusion for that request.
3. The request proceeds with non-excluding execution using the backend directly.

**FR-LSI-IDX-SCHED-04 (Key reuse rules):** A request MAY reuse the canonical index when its traversal options are **at least as restrictive** as the canonical key:

| Request Option | Canonical Value | Reuse Allowed? |
| -------------- | --------------- | -------------- |
| `hidden=false` | `false` | Yes (same) |
| `hidden=true` | `false` | No (request is less restrictive) |
| `hidden=false` | `true` | Yes (request is more restrictive; index may over-include but superset is safe) |

> **Note:** When the request is more restrictive, the index catalog is a superset of the eligible files, which is safe for candidate exclusion but may result in extra files being scanned. The request's `glob[]` filter further narrows the candidates at query time.

#### 5.2.4 Scope and Multi-Root Handling

**FR-LSI-IDX-SCOPE-01 (Subpath support):** If request `path` is a subtree of an Index Root, the tool MUST apply a deterministic subtree filter to reuse the index (no rebuild required). The filter operates on `path_sort_key` prefix matching.

**FR-LSI-IDX-SCOPE-02 (Multi-root):** If a request path cannot be unambiguously resolved within exactly one sandbox root (or resolves outside sandbox), the tool MUST reject the request per sandbox rules; indexing MUST NOT attempt to "span" roots implicitly.

### 5.3 Storage Backends

**FR-LSI-IDX-STOR-01:** The system MUST support `index_storage=memory` and `index_storage=sqlite`.

**FR-LSI-IDX-STOR-02:** If `index_storage=sqlite`, the default index path MUST be outside the searched tree and within an approved cache directory.

**FR-LSI-IDX-STOR-03:** Persisted indexes MUST NOT store raw file contents by default; they MAY store derived token structures (e.g., n-gram bloom filters) and file fingerprints.

#### 5.3.1 Cache Root Selection and Validation (Normative)

This section defines how the **cache root** is determined for persisted indexes and temporary files.

**FR-LSI-CACHE-ROOT-01 (Selection algorithm):** The cache root MUST be selected as follows:

1. If `index_path` is explicitly configured and non-empty, use that path.
2. Otherwise, use the OS-specific user cache directory:
   * **Windows:** `%LOCALAPPDATA%\forge\search-index` (e.g., `C:\Users\<user>\AppData\Local\forge\search-index`)
   * **macOS:** `~/Library/Caches/forge/search-index`
   * **Linux/Unix:** `$XDG_CACHE_HOME/forge/search-index` or `~/.cache/forge/search-index` if `XDG_CACHE_HOME` is unset
3. If the OS cache directory cannot be determined, fall back to a temp directory: `$TMPDIR/forge-search-<uid>/` or `/tmp/forge-search-<uid>/`.

**FR-LSI-CACHE-ROOT-02 (Validation requirements):** The selected cache root MUST be validated before use:

1. The path MUST NOT be inside or below any searched tree (Index Root) to prevent self-indexing.
2. The path MUST NOT be inside any sandbox denied patterns.
3. The directory MUST be creatable with restrictive permissions (user-only on POSIX).
4. On failure to validate, persistence MUST be disabled and storage MUST fall back to `memory` (see §5.3.2).

**FR-LSI-CACHE-ROOT-03 (Temp file location):** Temporary files used for backend candidate lists (per Appendix G) MUST be created within the validated cache root or an OS temp directory, NEVER inside the searched tree.

**FR-LSI-CACHE-ROOT-04 (Sandbox interaction):** The cache root is **outside the sandbox scope** for Tool Executor purposes. Index operations do not require sandbox `allow_absolute=true` for the cache root because the cache root is tool-internal infrastructure, not user-specified search targets.

#### 5.3.2 Storage Fallback Policy

**FR-LSI-STOR-FALLBACK-01:** When `index_storage=sqlite` but persistence cannot be enabled (cache root validation fails, permissions cannot be set), the tool MUST:

1. Log a warning with the specific failure reason.
2. Fall back to `index_storage=memory` for the current session.
3. The index state becomes `BUILDING` (not `DISABLED`) if thresholds are met.
4. Set `stats.storage_fallback_reason` (if `emit_stats=true`) to indicate the fallback.

**FR-LSI-STOR-FALLBACK-02:** If `index_storage=memory` AND the memory budget would be exceeded, the tool MUST transition to `DISABLED` with `uncertain_reason='MEMORY_BUDGET_EXCEEDED'` per §4.2.2.

### 5.4 Concurrency and Locking

**FR-LSI-IDX-CONC-01:** Concurrent tool calls MUST NOT corrupt persisted index storage.

**FR-LSI-IDX-CONC-02:** Lock acquisition MUST be bounded by `index_lock_timeout_ms`. If lock cannot be acquired within budget, the tool MUST degrade gracefully (no index exclusion for that call).

**FR-LSI-IDX-CONC-03:** Writes MUST be transactional and crash-safe.

### 5.5 Resource Limits and Eviction

**FR-LSI-IDX-RM-01:** Persisted index storage MUST enforce:

* Maximum on-disk bytes (`index_max_db_bytes`), AND
* Maximum number of indexes (`index_max_indexes`)

**FR-LSI-IDX-RM-02:** When limits are exceeded, eviction MUST occur using a deterministic policy:

1. Evict least-recently-used by `last_accessed_at`
2. Tie-break by largest size
3. Tie-break by lexicographic Index Key hash

**FR-LSI-IDX-RM-03:** Eviction MUST be concurrency-safe (no partial deletion while another process is reading). If necessary, use tombstoning and deferred cleanup.

**FR-LSI-IDX-RM-04:** In-memory indexes MUST enforce a max memory budget and MUST evict or disable indexing rather than risking OOM.

---

## 6. Coverage, Change Tracking, and Reconcile

### 6.1 COMPLETE Criteria

**FR-LSI-SAFE-COMPLETE-01:** The index MAY transition to `COMPLETE` only after a successful full eligible-file enumeration for the Index Key, with **no enumeration errors** that could hide files.

**FR-LSI-SAFE-COMPLETE-02:** If enumeration errors occur (permission denied, IO failures), the index MUST remain `UNCERTAIN` unless baseline semantics for that condition would also fail the tool call.

### 6.2 Fingerprints

**FR-LSI-TRK-01:** The index MUST track per-file fingerprints sufficient to detect changes without reading full file contents.

**FR-LSI-TRK-02:** Fingerprints MUST include at minimum: file size, mtime, and a stable file identity where available (inode/device on Unix; file ID on Windows).

**FR-LSI-TRK-03:** If timestamp resolution is insufficient to reliably detect rapid edits, the system SHOULD mitigate by hashing small files or using additional metadata, while preserving budgets and privacy constraints.

### 6.3 Watchers and Dirty Sets

**FR-LSI-TRK-04:** If enabled and supported, filesystem watchers SHOULD populate a dirty set for incremental updates.

**FR-LSI-TRK-05:** Dirty set MUST support file-level and subtree-level dirtiness.

**FR-LSI-TRK-06:** Watcher event processing MUST be debounced by `index_watch_debounce_ms`.

**FR-LSI-TRK-09:** When a rename/move event is detected with both source and destination paths known, the index MAY update path mappings atomically within budget. If the mapping is ambiguous (source unknown, scope uncertain) or the update fails, the index MUST transition to `UNCERTAIN` with `uncertain_reason='RENAME_UNCERTAIN'`.

**FR-LSI-TRK-10:** Dirty queue processing MUST use a deterministic order (oldest-first by `queued_at_ms`, tie-broken by normalized path) to ensure reproducible incremental updates.

### 6.4 Ignore-File Change Invalidation

**FR-LSI-TRK-07:** Changes to ignore control files that affect eligibility (e.g., `.gitignore`, `.ignore`, and any configured global excludes if used) MUST:

* Transition index to `UNCERTAIN`, AND
* Require reconcile or rebuild before index-based exclusion is allowed again.

### 6.5 Watcher Overflow / Failure

**FR-LSI-TRK-08:** On watcher overflow, dropped events, or watcher failure, the system MUST transition to `UNCERTAIN` for the affected scope.

### 6.6 Reconcile Scan (Bounded)

**FR-LSI-REC-01:** In `UNCERTAIN`, the tool MUST either:

* Perform a reconcile scan within configured budgets, OR
* Bypass index-based exclusion for subsequent calls until coverage is restored.

**FR-LSI-REC-02:** Reconcile MUST be bounded by `reconcile_max_files` and `reconcile_max_ms`. If budgets are exceeded, the tool MUST remain `UNCERTAIN`.

**FR-LSI-REC-03:** Reconcile MUST handle adds, deletes, and modifications deterministically.

---

## 7. Candidate Filtering and Superset Safety

### 7.1 Superset Requirement

**FR-LSI-CAND-01:** Index-based candidate exclusion MUST NOT introduce false negatives: it MUST only exclude a file if it can be proven that the file cannot produce any `"match"` event under the current request semantics.

**FR-LSI-CAND-02:** If superset safety cannot be proven for a given request, the tool MUST bypass index-based exclusion.

### 7.2 Supported Safe Strategies (Conservative by Default)

The following strategies are permitted only under explicit safety rules:

#### 7.2.1 Literal Search Candidate Exclusion (Recommended)

**FR-LSI-CAND-LIT-01:** For literal searches (`fixed_strings=true`, `fuzzy` absent), the index MAY exclude files using a derived token structure such as n-gram bloom filters **only if**:

* Tokenization for the file is complete for the indexed byte range, AND
* The token structure has **no false negatives** by design.

**FR-LSI-CAND-LIT-02:** Files not tokenized (too large, unreadable, binary per policy) MUST always remain candidates (cannot be excluded).

**FR-LSI-CAND-LIT-03:** If the normalized pattern length is less than the n-gram size `k`, index-based exclusion MUST be bypassed (all eligible files remain candidates) because no complete n-gram can be extracted from the pattern. When `len(pattern_normalized) == k`, exactly one n-gram exists and the membership test operates normally.

**FR-LSI-CAND-LIT-04:** Bloom filter parameters (n-gram size `k`, bit count `m`, hash count `k_hashes`, hash seed) MUST be stored with the index metadata and MUST match across all files within an Index Key. Parameter mismatch MUST trigger rebuild under a new key.

**FR-LSI-CAND-LIT-05:** Case-sensitivity variant selection MUST be deterministic based on the request's `case` parameter and the stored `casefold_strategy`:
* `case=sensitive` → use `SENSITIVE` variant
* `case=insensitive` → use `INSENSITIVE` variant
* `case=smart` → if pattern contains uppercase, use `SENSITIVE`; otherwise use `INSENSITIVE`

The variant used MUST match the backend's equivalent behavior for that case mode.

#### 7.2.2 Regex Search Candidate Exclusion (Optional, Highly Conservative)

**FR-LSI-CAND-RE-01:** Regex-based exclusion MAY be used only if the tool can extract a literal substring that is guaranteed to occur in every match (a “required literal”), and that literal meets the same safety criteria as literal searches.

**FR-LSI-CAND-RE-02:** If required-literal extraction is uncertain, optional, or complex (alternation, lookarounds, backrefs), the tool MUST bypass exclusion.

#### 7.2.3 Fuzzy Search Candidate Exclusion

**FR-LSI-CAND-FZ-01:** For fuzzy searches (`fuzzy` present) and fuzzy fallback passes, the tool MUST NOT use index-based exclusion unless a proven-superset method exists for that fuzzy metric; otherwise it MUST scan all eligible files.

---

## 8. Backend Execution and Parsing Requirements

### 8.1 Backend Selection and Capability Handling

Baseline requires `ugrep` preferred and `rg` fallback.

**FR-LSI-BE-01:** Backend execution MUST comply with `docs/LOCAL_SEARCH_SRD.md` constraints:

* Execute without a shell
* Enforce `timeout_ms` and `max_results` (FR-LS-03, FR-LS-06, FR-LS-07)

**FR-LSI-BE-02:** If indexing requires backend capabilities (e.g., file-list mode), the system MUST detect capability availability deterministically and MUST disable index-based exclusion when unavailable.

### 8.2 Large Candidate Lists (Command-Line Limits)

**FR-LSI-EXE-01:** Candidate file lists MUST be passed to the backend using a scalable mechanism safe for large lists (e.g., stdin, a temp file list option, or equivalent backend-supported feature). The implementation MUST NOT rely solely on passing all paths as command-line arguments.

**FR-LSI-EXE-02:** Any temporary file used for file lists MUST be created in an approved temp/cache directory and MUST NOT be created inside the searched tree unless explicitly configured.

### 8.3 Chunking and Deterministic Merge

**FR-LSI-EXE-03:** If chunked backend invocations are required, chunks MUST be processed in deterministic file order, and outputs MUST be merged deterministically to match the tool’s ordering model (Section 3).

**FR-LSI-EXE-04:** Chunking MUST NOT change the truncation boundary relative to the deterministic event order. If it would, chunking MUST be adjusted (smaller chunks) or index exclusion MUST be bypassed.

### 8.4 Backend Exit Classification

**FR-LSI-EXIT-01:** The implementation MUST classify backend completion deterministically into:

* `ok_with_matches`
* `ok_no_matches`
* `truncated` (tool-stopped due to max_results)
* `timed_out`
* `backend_error`

**FR-LSI-EXIT-02:** Backend exit-code mapping and stderr parsing rules MUST be documented per backend and covered by integration tests.

**FR-LSI-EXIT-03:** Fuzzy fallback MUST trigger only on `ok_no_matches` (see Section 9).

### 8.5 Output Decoding and Sanitization

**FR-LSI-ENC-01:** Backend stdout/stderr decoding MUST be deterministic. If bytes are not valid UTF-8, decoding MUST follow a documented policy (e.g., replacement character) that is stable across platforms.

**FR-LSI-ENC-02:** Output sanitization MUST comply with Tool Executor requirements (e.g., terminal control injection prevention) and MUST NOT alter structured data semantics.

### 8.6 Parsing and Context Semantics

Baseline structured record format:

```json
{
  "type": "match" | "context",
  "data": {
    "path": { "text": "<path>" },
    "line_number": <u64>,
    "lines": { "text": "<line text>" }
  }
}
```

**FR-LSI-PARSE-01:** Parsing MUST produce structured events exactly as in `docs/LOCAL_SEARCH_SRD.md` (FR-LS-04).

**FR-LSI-CTX-01:** When `context > 0`, the tool MUST preserve backend context emission semantics for that backend and MUST represent them as `"context"` events.

**FR-LSI-CTX-02:** `max_results` MUST count both `"match"` and `"context"` events. When the limit is reached, the tool MUST stop emitting further events and MUST compute `truncated` per **FR-LSI-TRUNC-01** (attempt to observe one additional event where feasible to determine if more exist).

> **Cross-reference note:** This aligns with `docs/LOCAL_SEARCH_SRD.md` FR-LS-06 semantics. The refined `truncated` detection in FR-LSI-TRUNC-01a ensures `truncated=false` when exactly `max_results` events exist with no additional events beyond.

---

## 9. Fuzzy Fallback (Opt-in, Level-Based)

### 9.1 Opt-in and Trigger Conditions

**FR-LSI-FUZ-01:** Fuzzy fallback MUST be disabled by default.

**FR-LSI-FUZ-02:** When enabled, fuzzy fallback MUST apply only when the request is a **literal search**:

* `fixed_strings=true`
* `fuzzy` is not provided by the caller
* (Optional restriction) `word_regexp=false` unless documented as supported

**FR-LSI-FUZ-03:** Fuzzy fallback MUST trigger only when the initial literal search completes as `ok_no_matches` and the tool is not `truncated` and not `timed_out`.

**FR-LSI-FUZ-04:** If the initial literal search is `timed_out` or `backend_error`, fuzzy fallback MUST NOT run.

### 9.2 Fuzzy Pass Levels (Aligned to Baseline `fuzzy: 1–4`)

**FR-LSI-FUZ-TH-01:** Fuzzy fallback passes MUST be expressed in terms of the existing request-compatible fuzzy levels `1..=4` (as defined by the backend/tool integration), not arbitrary thresholds.

**FR-LSI-FUZ-TH-02:** The fallback attempt sequence MUST be deterministic and configured as an ordered list (e.g., `[1,2,3]`). The tool MUST stop after the first pass that yields at least one `"match"` event.

### 9.3 Guardrails

**FR-LSI-FUZ-G-01:** Fuzzy fallback MUST be skipped if `pattern` length is below `fuzzy_min_pattern_len` (configurable), and this decision SHOULD be reported in `stats` when enabled.

**FR-LSI-FUZ-G-02:** Fuzzy fallback passes MUST honor the same request options (`case`, `glob`, `hidden`, `follow`, `no_ignore`, `context`, `timeout_ms`, `max_results`) unless a specific option is documented as incompatible with fuzzy mode.

**FR-LSI-FUZ-G-03 (Total timeout budget):** The total wall-clock time for the initial search plus all fallback passes MUST NOT exceed the request's `timeout_ms`:

1. Track remaining budget: `remaining = timeout_ms - elapsed_so_far`.
2. Before each fallback pass, check if `remaining > 0`; if not, skip remaining passes.
3. Each fallback pass receives the remaining budget as its effective timeout.
4. If passes are skipped due to budget exhaustion, record `fallback_passes_skipped` in `stats` (if enabled).

**FR-LSI-FUZ-G-04 (Fallback stop condition):** The tool MUST stop fallback iteration when at least one `"match"` event (type=`match`, not `context`) is emitted. Context-only output does not satisfy this condition.

---

## 10. Output Fields, `count` Semantics, and Optional `stats`

### 10.1 `count` Definition

**FR-LSI-COUNT-01:** `count` MUST equal the number of emitted events in `matches[]` (i.e., `matches.length`).

> This aligns with FR-LS-06 (“stop after max_results match/context events”).

### 10.2 Optional `stats` Policy and Determinism Classes

**FR-LSI-STAT-01:** Emitting `stats` MUST be gated by configuration (`emit_stats=true`). If false, the `stats` field MUST be omitted.

**FR-LSI-STAT-02:** If present, `stats` MUST include the following fields:

| Field | Type | Determinism | Description |
| ----- | ---- | ----------- | ----------- |
| `stats_version` | integer | deterministic | Schema version for stats object (for forward compatibility) |
| `index_safety_state` | enum | deterministic | Current safety state: `ABSENT`, `BUILDING`, `COMPLETE`, `UNCERTAIN`, `CORRUPT`, `DISABLED` |
| `index_uncertain_reason` | enum/null | deterministic | If `index_safety_state` is `UNCERTAIN` or `DISABLED`, the reason code from Appendix D; else `null` |
| `index_exclusion_used` | boolean | deterministic | Whether candidate exclusion was applied for this request |
| `storage_mode` | enum | deterministic | Effective storage: `memory`, `sqlite`, or `none` |
| `storage_fallback_reason` | enum/null | deterministic | If storage fell back (e.g., `PERMISSION_DENIED`), the reason; else `null` |
| `fallback_used` | boolean | deterministic | Whether fuzzy fallback was attempted |
| `fallback_reason` | enum/null | deterministic | Reason for fallback (see below); `null` if `fallback_used=false` |
| `fuzzy_levels_tried` | array[int] | deterministic | Fuzzy levels attempted (e.g., `[1, 2]`); empty if no fallback |
| `elapsed_ms` | integer | best_effort | Wall-clock time for this search (may vary due to system load) |
| `candidates_total` | integer | deterministic | Total eligible files before exclusion |
| `candidates_excluded` | integer | deterministic | Files excluded by Bloom filter |
| `candidates_scanned` | integer | deterministic | Files passed to backend |

**Fallback reason enum:**

| Value | Description |
| ----- | ----------- |
| `LITERAL_NO_MATCHES` | Initial literal search found zero matches; fallback triggered |
| `INDEX_OFF` | Index disabled; fallback used as primary search strategy |
| `INDEX_UNAVAILABLE` | Index in non-COMPLETE state; fallback may have been used |
| `CAPABILITY_MISSING` | Backend lacks file-list mode; fallback to direct search |
| `null` | No fallback occurred |

**FR-LSI-STAT-03 (Determinism classes):** `stats` fields MUST be classified as:

* `deterministic` (must not depend on wall-clock or scheduling; identical inputs produce identical values), or
* `best_effort` (may vary due to timing, load, or non-deterministic factors; MUST be clearly named)

**FR-LSI-STAT-04:** Timing fields (`*_ms`) and I/O counters (`bytes_read`) MUST be `best_effort` and MUST NOT be required for semantic equivalence tests.

---

## 11. Budget Enforcement and Deterministic Decision Policy

This section distinguishes between **initial build budgets** (background indexer, not tied to requests) and **incremental maintenance budgets** (per-request, must not delay searches).

### 11.1 Initial Build Budget (Background)

The initial index build runs on a background thread and is NOT constrained by search request timeouts.

**FR-LSI-BUD-INIT-01:** Initial build MUST be governed by its own budget parameters, independent of `timeout_ms`:
* `index_build_timeout_ms`: maximum wall-clock time for initial build (default: 300000 = 5 minutes)
* `index_build_max_files`: maximum files to index before stopping (default: unlimited)
* `index_build_max_bytes`: maximum cumulative file bytes to tokenize (default: unlimited)

**FR-LSI-BUD-INIT-02:** If any initial build budget is exceeded:
1. Persist whatever progress has been made (partial index)
2. Transition to `UNCERTAIN` with `uncertain_reason='BUILD_BUDGET_EXCEEDED'`
3. The partial index MAY be used for **positive acceleration only** (e.g., faster file enumeration, pre-computed statistics for diagnostics), but MUST NOT be used for **candidate exclusion** — all eligible files MUST remain candidates per FR-LSI-SAFE-03

**FR-LSI-BUD-INIT-03:** Initial build budget enforcement MUST NOT block or delay search requests. The background indexer simply stops when budget is exhausted.

### 11.2 Incremental Maintenance Budget (Per-Request)

Incremental maintenance (dirty queue processing, reconcile scans) occurs inline with search requests and MUST be time-bounded.

**FR-LSI-BUD-MAINT-01:** Incremental maintenance during a search request MUST NOT consume more than `index_maint_budget_fraction * timeout_ms` (default: 0.25 = 25% of request timeout).

**FR-LSI-BUD-MAINT-02:** Maintenance budget applies to:
* Processing dirty queue entries accumulated since last search
* Validating fingerprints for files accessed in the query path
* Micro-reconcile operations for recently-changed files

**FR-LSI-BUD-MAINT-03:** Maintenance budget does NOT apply to:
* Initial build (governed by §11.1)
* Full reconcile scans (governed by `reconcile_max_ms`)
* Watcher event processing (background, governed by §4.3.4 throttling)

### 11.3 Deterministic Budget Actions

**FR-LSI-BUD-02:** When an incremental maintenance budget would be exceeded, the tool MUST take one of the following deterministic actions (documented and test-covered), in priority order:

1. **Skip remaining maintenance** for this call and mark index `UNCERTAIN` with `uncertain_reason='MAINT_BUDGET_EXCEEDED'` if needed for safety.
2. **Bypass index-based exclusion** and perform non-excluding execution for this call.
3. **Disable indexing** for the Index Key for a cooling-off period (optional) if repeated budget exhaustion occurs.

**FR-LSI-BUD-03:** The chosen action MUST be recorded in `stats.fallback_reason` when `stats` is enabled.

**FR-LSI-BUD-04:** If cooldown is enabled and the `DISABLED` state is entered due to resource exhaustion, the implementation MAY set a `disabled_until_ms` timestamp. The index MUST remain `DISABLED` until this timestamp expires, at which point it MUST transition to `ABSENT` and attempt re-initialization per Section 4.2.2.

### 11.4 Budget Interaction with Throttling

**FR-LSI-BUD-05:** Background indexer throttling (§FR-LSI-BG-31) and initial build budget (§11.1) are independent controls:
* Throttling controls CPU/IO intensity (how fast we work)
* Budget controls total work (how much we do before stopping)

Both apply simultaneously. A throttled indexer may still exceed its time budget if the repo is large enough.

---

## 12. Security, Privacy, and Edit-Safety Interaction

### 12.1 Sandbox and Deny Patterns

**SEC-LSI-01:** All filesystem access implied by search and indexing MUST comply with Tool Executor sandbox requirements (root containment, symlink/junction protections, denied patterns).

**SEC-LSI-02:** Persisted indexes MUST NOT include entries for denied files. If deny patterns change, coverage MUST be invalidated (`UNCERTAIN`) until reconciled.

### 12.2 Persistence Permissions and Multi-User Safety

**SEC-LSI-STORE-01:** Persisted index files MUST be created with restrictive permissions appropriate to the OS (user-only access where supported).

**SEC-LSI-STORE-02:** If secure permissions cannot be applied (e.g., due to filesystem limitations), the tool MUST apply the storage fallback policy per §5.3.2:

1. Fall back to `index_storage=memory` for the current session.
2. The index remains functional (state becomes `BUILDING`, not `DISABLED`) if thresholds are met.
3. If a high-risk configuration flag `index_allow_insecure_persistence=true` is set, persistence MAY proceed with a logged warning.

> **Note:** This differs from the §4.2.2 transition "Persistence not permitted" → `DISABLED`, which applies when the cache root itself cannot be validated (e.g., cannot determine a valid cache directory at all). When only permissions fail but a valid cache root exists, storage fallback applies.

### 12.3 Stale-File Protection Interaction (Hard Requirement)

Forge’s edit tools implement stale-file protection based on explicit reads.

**SEC-LSI-CACHE-01:** Search/indexing MUST NOT mark files as “read” for purposes of stale-file protection. Only explicit `Read`/`read_file` tool results may satisfy any “must have been read” preconditions for patch/edit tools.

**SEC-LSI-CACHE-02:** Any shared caches used by Search (fingerprints, hashes, metadata) MUST be segregated from the read-tracking mechanism used by edit tools.

---

## 13. Configuration

Configuration extends `docs/LOCAL_SEARCH_SRD.md` Section 5 (`[tools.search]`).

### 13.1 Example Configuration

```toml
[tools.search]
enabled = true
binary = "ugrep"
fallback_binary = "rg"
default_timeout_ms = 20000
default_max_results = 200

# Indexing - Mode and Storage
index_mode = "auto"                 # off | auto | on
index_storage = "sqlite"            # memory | sqlite
index_path = ""                     # if empty, choose default cache path outside searched tree
index_roots = []                    # workspace roots to index; empty = use cwd/sandbox root

# Indexing - Auto-mode thresholds
index_auto_threshold_files = 2000
index_auto_threshold_bytes = 500000000  # 500 MB
index_auto_reeval_calls = 100           # re-check thresholds every N search calls when BELOW_THRESHOLD

# Indexing - Background build (§4.3, §11.1)
index_build_timeout_ms = 300000     # 5 minutes max for initial build
index_build_max_files = 0           # 0 = unlimited
index_build_max_bytes = 0           # 0 = unlimited
index_build_max_cpu_percent = 50    # throttle to avoid hogging CPU
index_build_throttle_ms = 10        # sleep between file batches
index_idle_boost_ms = 1000          # boost speed if no searches for this long
index_retry_on_failure = false      # auto-retry build on transient errors
index_checkpoint_on_shutdown = true # persist progress on graceful shutdown
index_shutdown_timeout_ms = 5000    # max time to quiesce on shutdown

# Indexing - Watchers and change tracking
index_watch = true
index_watch_debounce_ms = 250

# Indexing - Resource limits
index_max_db_bytes = 1073741824     # 1 GiB
index_max_indexes = 10
index_lock_timeout_ms = 250
index_integrity_check = "on_open"   # off | on_open | periodic
index_integrity_period_calls = 200

# Indexing - Maintenance budgets (§11.2)
reconcile_max_files = 50000
reconcile_max_ms = 2000
index_maint_budget_fraction = 0.25  # max 25% of request timeout for inline maintenance

# Diagnostics
emit_stats = false                  # if true, include `stats` block in response

# Fuzzy fallback
fuzzy_fallback = false
fuzzy_fallback_levels = [1, 2, 3]   # ordered; stop after first yielding matches
fuzzy_min_pattern_len = 4
```

### 13.2 Configuration Requirements

**CFG-LSI-IDX-01:** Indexing config MUST be under `[tools.search]` to match the tool’s canonical config namespace.

**CFG-LSI-IDX-02:** Auto-thresholds MUST be used only when `index_mode=auto`.

**CFG-LSI-STAT-01:** `emit_stats` MUST default to false.

**CFG-LSI-FUZ-01:** `fuzzy_fallback` MUST default to false.

**CFG-LSI-VAL-01:** Configuration values MUST be validated on load (enums, ranges, non-negative integers, levels in 1..=4).

---

## 14. Non-Functional Requirements

### 14.1 Correctness and Reliability

* **NFR-LSI-COR-01:** With `fuzzy_fallback=false`, results MUST be correct and consistent with non-indexed execution under the deterministic ordering model.
* **NFR-LSI-COR-02:** Index corruption or missing indexes MUST never cause incorrect results; it must trigger bypass/fallback.
* **NFR-LSI-REL-01:** Crash during index write MUST not corrupt tool behavior; rebuild or bypass is required.

### 14.2 Performance

* **NFR-LSI-PERF-01:** Incremental updates SHOULD be proportional to changed files when watchers are healthy.
* **NFR-LSI-PERF-02:** Indexing SHOULD reduce repeated search latency on large trees where safe candidate exclusion is available.
* **NFR-LSI-PERF-03:** Index maintenance MUST be budgeted and must not violate tool timeout.

### 14.3 Portability

* **NFR-LSI-PORT-01:** Must work on Windows/macOS/Linux.
* **NFR-LSI-PORT-02:** Path normalization MUST be specified and implemented consistently (Appendix C).

---

## 15. Verification Requirements

### 15.1 Baseline Compatibility Tests

| ID            | Scenario                             | Expected                                  |
| ------------- | ------------------------------------ | ----------------------------------------- |
| T-LSI-BASE-01 | Indexing off                         | Matches baseline tool behavior and schema |
| T-LSI-BASE-02 | Index present but disabled by config | No index exclusion; correct results       |

### 15.2 Determinism and Truncation

| ID             | Scenario                                 | Expected                                                                                   |
| -------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------ |
| T-LSI-DET-01   | Same repo + request repeated             | Identical `matches[]` ordering and identical truncation boundary                           |
| T-LSI-DET-02   | Backend emits out-of-order (simulate by shuffling parsed events) | Tool output order matches FR-LSI-ORD-03 regardless of backend emission order |
| T-LSI-DET-03   | Unicode NFC/NFD filenames and patterns   | Ordering and candidate logic stable across composed/decomposed forms |
| T-LSI-DET-04   | Smartcase with non-ASCII uppercase       | Correct variant selection and deterministic ordering |
| T-LSI-TRUNC-01 | `max_results` small with context enabled | Exactly `max_results` events, `count == matches.length`, `truncated=true` when appropriate |
| T-LSI-TRUNC-02 | Chunked execution with overlapping context | Merged event stream matches non-chunked run; truncation boundary identical |
| T-LSI-TRUNC-03 | Truncation detection edge case (exactly max_results exist) | `truncated=false` when no additional events exist |

### 15.3 Index Safety and Coverage

| ID            | Scenario                         | Expected                                                               |
| ------------- | -------------------------------- | ---------------------------------------------------------------------- |
| T-LSI-SAFE-01 | Watcher overflow                 | Index becomes `UNCERTAIN`; no candidate exclusion until reconciled     |
| T-LSI-SAFE-02 | Ignore file (.gitignore) changed | Index becomes `UNCERTAIN`; exclusion disabled until reconcile          |
| T-LSI-SAFE-03 | Reconcile exceeds budget         | Remains `UNCERTAIN`; correctness preserved via non-excluding execution |
| T-LSI-SAFE-04 | Multiple simultaneous triggers   | State resolves per P1/P2 precedence; `uncertain_reason` from highest-priority trigger |
| T-LSI-SAFE-05 | Partial index (budget exceeded)  | State is `UNCERTAIN`; no candidate exclusion allowed per FR-LSI-SAFE-03 |
| T-LSI-SAFE-06 | Auto-mode re-evaluation after growth | When repo grows past thresholds, `DISABLED` with `BELOW_THRESHOLD` transitions to `BUILDING` |
| T-LSI-SAFE-07 | Persistence not permitted        | State is `DISABLED` with `uncertain_reason='PERSISTENCE_NOT_PERMITTED'`; falls back to memory or no index |

### 15.4 Backend Scaling and Exit Classification

| ID            | Scenario             | Expected                                                       |
| ------------- | -------------------- | -------------------------------------------------------------- |
| T-LSI-EXE-01  | Large candidate list | Uses scalable mechanism (no arg overflow); deterministic merge |
| T-LSI-EXIT-01 | Backend “no matches” | Classified as `ok_no_matches`; no erroneous fallback on errors |
| T-LSI-EXIT-02 | Timeout              | `timed_out=true`; fuzzy fallback does not run                  |

### 15.5 Fuzzy Fallback

| ID           | Scenario                             | Expected                                                            |
| ------------ | ------------------------------------ | ------------------------------------------------------------------- |
| T-LSI-FUZ-01 | Literal no matches, fallback enabled | Tries configured fuzzy levels in order; stops at first with matches |
| T-LSI-FUZ-02 | Pattern shorter than min             | Fallback skipped; reason reported in stats (if enabled)             |
| T-LSI-FUZ-03 | Total time budget across passes      | Total time ≤ request `timeout_ms`; remaining passes skipped if budget exhausted |
| T-LSI-FUZ-04 | Stop condition: context-only output  | Fallback continues if only context events emitted (no matches)      |

### 15.6 Tokenizer and Bloom Filter Determinism

| ID             | Scenario                                           | Expected                                                                    |
| -------------- | -------------------------------------------------- | --------------------------------------------------------------------------- |
| T-LSI-TOK-01   | Golden file tokenization test                      | Fixed input file produces exact expected n-gram list and bitset             |
| T-LSI-TOK-02   | CRLF vs LF normalization                           | Same content with different line endings produces identical n-grams         |
| T-LSI-TOK-03   | Unicode NFC vs NFD normalization                   | Composed vs decomposed input produces identical n-grams after normalization |
| T-LSI-TOK-04   | Casefolding consistency (ASCII)                    | `ABC` and `abc` produce same n-grams in INSENSITIVE variant                 |
| T-LSI-TOK-05   | Casefolding consistency (UNICODE_SIMPLE)           | `Ñ` and `ñ` produce same n-grams in INSENSITIVE variant                     |
| T-LSI-BLOOM-01 | Bloom filter bitset golden test                    | Fixed n-gram set produces exact expected bitset bytes                       |
| T-LSI-BLOOM-02 | No false negatives (exclusion soundness)           | For any excluded file, pattern n-grams truly absent from file content       |
| T-LSI-BLOOM-03 | xxHash64 determinism across platforms              | Same input bytes + seeds produce identical hash values on all platforms     |
| T-LSI-BLOOM-04 | Bit storage layout verification                    | Manual bit index calculation matches stored bitset structure                |

**T-LSI-BLOOM-02 Implementation Note:** This test must verify exclusion soundness by:
1. Running a search with candidate exclusion enabled
2. For each excluded file, running the backend directly on that file
3. Asserting zero matches for the excluded files (proving exclusion was safe)

### 15.7 Path Edge Cases

| ID             | Scenario                                  | Expected                                                     |
| -------------- | ----------------------------------------- | ------------------------------------------------------------ |
| T-LSI-PATH-01  | Newline in filename                       | File excluded from index catalog; searchable via direct backend |
| T-LSI-PATH-02  | Non-UTF-8 filename (Unix)                 | Lossy conversion to U+FFFD; deterministic ordering preserved |
| T-LSI-PATH-03  | macOS NFC/NFD filename variation          | Same logical file resolved consistently; no duplicate entries |
| T-LSI-PATH-04  | Symlink inside search tree                | Handled per sandbox symlink policy; no escape                |

### 15.8 Security / Edit-Safety

| ID           | Scenario             | Expected                                                           |
| ------------ | -------------------- | ------------------------------------------------------------------ |
| T-LSI-SEC-01 | Denied file patterns | Search/indexing never reads or indexes denied paths                |
| T-LSI-SEC-02 | Stale-file gating    | Search does not count as “read”; edits still require explicit Read |

---

## 16. Threat Model Summary

* **Symlink/junction escape:** Prevent via sandbox canonicalization and disallow index trusting stored paths without revalidation.
* **Index content leakage:** No raw contents by default; derived tokens only; secure perms; disable persistence if perms can’t be enforced.
* **Index poisoning via paths:** Canonical normalization; length caps; parameterized sqlite; strict decoding rules.
* **Eligibility drift:** Watch ignore files; invalidate coverage on changes; reconcile or bypass exclusion.

---

## 17. Rollout and Migration

**FR-LSI-ROLL-01:** Indexing and fuzzy fallback MUST be disabled by default (safe default).

**FR-LSI-ROLL-02:** Index schema MUST be versioned; schema bump triggers rebuild.

**FR-LSI-ROLL-03:** A kill-switch MUST exist: `index_mode=off` disables indexing completely.

**FR-LSI-ROLL-04:** During rollout, `emit_stats` SHOULD be enabled only in dev/test environments to validate safety-state transitions and stable truncation.

---

## 18. Resolved Questions

*The following questions were identified during initial specification and have been resolved:*

### 18.1 Backend File-List Ingestion (RESOLVED)

**Original question:** Which backend(s) are required to support file-list ingestion, and how is capability detection implemented?

**Resolution:** Appendix G specifies:
* **Supported backends:** ugrep 3.0+ and ripgrep 13.0+
* **Detection:** Deterministic version parsing via regex at startup (FR-LSI-BE-CAP-01)
* **Graceful degradation:** If file-list mode unavailable, candidate exclusion is disabled (FR-LSI-BE-CAP-02)

### 18.2 Eligible File Set Definition (RESOLVED)

**Original question:** What is the canonical definition of "eligible file set" when backends differ in ignore semantics?

**Resolution:** Forge standardizes eligibility and always supplies file lists to backends. The eligible file set is defined as follows:

**FR-LSI-ELIG-01 (Canonical eligibility definition):** The eligible file set for a request is determined by Forge's enumeration engine, NOT by the backend's ignore semantics:

1. **Enumeration:** Forge enumerates files using its own ignore engine (`.gitignore`, `.ignore`, global excludes, sandbox deny patterns) with the request's traversal options (`hidden`, `follow`, `no_ignore`).
2. **Backend invocation:** When candidate exclusion is active, Forge passes the eligible file list explicitly to the backend via `--include-from` or equivalent (Appendix G). The backend's own ignore semantics are overridden.
3. **Baseline equivalence:** When index is `DISABLED`, `ABSENT`, or exclusion is bypassed, the backend is invoked directly (without file list), and its native ignore semantics apply. This is acceptable because no exclusion occurs.

**FR-LSI-ELIG-02 (Consistency requirement):** The enumeration engine used for indexing MUST be the same engine used for non-indexed searches when possible. If divergence is unavoidable, candidate exclusion MUST be disabled for affected configurations.

> **Rationale:** This eliminates the risk of false negatives from eligibility divergence. The index's eligible file set always matches or is a superset of what the backend would search, satisfying superset safety.

### 18.3 Deterministic Ordering Scope (DEFERRED)

**Original question:** Should deterministic ordering be moved into LOCAL_SEARCH_SRD.md?

**Status:** Deferred to future specification revision. Currently, deterministic ordering is specified in this SRD as an extension. Baseline LOCAL_SEARCH_SRD does not guarantee ordering. This is acceptable because:
* Ordering is only required for stable truncation in indexed searches
* Non-indexed searches remain compatible with baseline (non-deterministic) behavior
* Moving ordering to baseline would be a breaking change requiring coordination

---

# Appendix C: Path Normalization (Normative)

This appendix defines two distinct normalization functions. **Key normalization** produces canonical absolute paths for sandbox validation and index identity. **Order normalization** produces relative paths for deterministic ordering across platforms and checkout locations.

### NormalizePathForKey(path, index_root)

Used for: Index Key identity, sandbox validation, file identity within catalog.

**Alignment with Tool Executor (FR-VAL-04 in TOOL_EXECUTOR_SRD.md):**

1. **Base directory:** If `path` is relative, resolve it relative to the **sandbox working directory** (typically the configured working directory or sandbox root). This matches Tool Executor's path resolution behavior.

2. **Absolute path handling:** Absolute paths are permitted for index operations. Unlike Tool Executor search requests (which may require `allow_absolute=true`), index-internal paths (cache root, catalog entries) routinely use absolute paths.

3. **Symlink-safe resolution:** Use symlink-safe primitives (per FR-VAL-06) when resolving paths. Do not follow symlinks outside sandbox boundaries.

**Algorithm:**

1. If `path` is relative: resolve to absolute using sandbox working directory as base.
2. Validate against sandbox rules (deny `..` traversal outside allowed roots, enforce root containment, apply deny patterns).
3. Resolve to canonical absolute path using symlink-safe canonicalization (no symlink escape).
4. Normalize path separators to `/` (forward slash).
5. Remove redundant `.` segments; preserve meaningful `..` rejection outcome.
6. On Windows: normalize drive letter casing consistently (uppercase `C:`, etc.).
7. Remove trailing `/` except for root paths (e.g., `C:/` or `/`).
8. Return the canonical absolute path.

### NormalizeRelPathForOrder(path, order_root)

Used for: Deterministic event ordering (FR-LSI-ORD-03), `path_sort_key` in storage. Uses `order_root` from §3.4, not necessarily `index_root`.

**Algorithm:**

1. Apply `NormalizePathForKey(path, order_root)` to obtain canonical absolute path.
2. Compute the relative path from `order_root` to the canonical path.
3. **Non-UTF-8 handling (Unix):** On Unix systems, path bytes MUST be converted using deterministic lossy conversion (equivalent to `to_string_lossy()` in Rust). Invalid UTF-8 byte sequences are replaced with U+FFFD. This ensures ordering is deterministic even for non-UTF-8 filenames.
4. Apply Unicode normalization: **NFC** (Canonical Decomposition, followed by Canonical Composition).
5. Encode the normalized string as **UTF-8**.
6. The resulting UTF-8 byte sequence is the `path_sort_key`.

### Path Comparison (Normative)

When ordering paths for determinism:

1. Compute `path_sort_key` for each path using `NormalizeRelPathForOrder`.
2. Compare `path_sort_key` values using **bytewise lexicographic ordering** (compare UTF-8 bytes directly, no locale).
3. Shorter paths sort before longer paths when one is a prefix of the other.

This ensures:
* Ordering is independent of the absolute checkout location.
* Ordering is stable across platforms (Windows, macOS, Linux).
* macOS NFC/NFD filesystem variations are normalized consistently.

### Legacy Compatibility Note

`NormalizePathForOrder(path)` (without `index_root`) is deprecated. Implementations MUST use `NormalizeRelPathForOrder` for ordering and MUST use `NormalizePathForKey` for identity/validation.

---

# Appendix D: `uncertain_reason` Enum (Normative)

When the index is in `UNCERTAIN` state, the `uncertain_reason` field MUST contain one of the following values:

| Value | Description |
| ----- | ----------- |
| `OPEN_REQUIRES_VALIDATION` | Index opened from persistence but coverage not yet verified against current filesystem state |
| `WATCHER_OVERFLOW` | Filesystem watcher reported event overflow; some events were dropped |
| `WATCHER_DOWN` | Filesystem watcher stopped unexpectedly or is unavailable |
| `ELIGIBILITY_CHANGED` | Ignore-control file (`.gitignore`, `.ignore`, global excludes) was modified |
| `POLICY_CHANGED` | Sandbox or deny policy was modified, affecting file eligibility |
| `DIRTY_BACKLOG` | Incremental updates are lagging; dirty queue could not be processed within budget |
| `RENAME_UNCERTAIN` | Rename/move event detected but source-destination mapping is ambiguous |
| `ENUMERATION_ERROR` | IO or permission error during enumeration that could hide files |
| `BUILD_BUDGET_EXCEEDED` | Initial build stopped due to time/file/byte budget limits; partial index exists |
| `MAINT_BUDGET_EXCEEDED` | Inline maintenance could not complete within per-request budget |
| `WATCHER_OVERFLOW_DURING_BUILD` | Watcher overflow occurred while initial build was in progress |

When the index is in `DISABLED` state, the `uncertain_reason` field MAY contain:

| Value | Description |
| ----- | ----------- |
| `BELOW_THRESHOLD` | Auto-mode determined repo is below size thresholds; indexing not needed |
| `PERSISTENCE_NOT_PERMITTED` | Cannot create index file with required permissions |
| `MEMORY_BUDGET_EXCEEDED` | In-memory index exceeded configured memory limits |

When the index transitions to `COMPLETE`, the `uncertain_reason` MUST be cleared (set to `NULL`).

---

# Appendix E: Reference SQLite Schema (Informative)

This appendix provides a reference schema for persisted indexes that satisfies the requirements in Sections 4–7. Implementations MAY use different structures provided they meet all normative requirements.

### Design Principles

For **literal searches** (`fixed_strings=true`, `fuzzy` absent), a file may be **excluded** only if:

1. It is eligible and readable (`eligible=1`, `readable=1`)
2. It is **fully tokenized** for the relevant normalization/case variant (`token_status='FULL'`, `token_complete=1`)
3. A Bloom filter exists for the needed `variant` (sensitive/insensitive)
4. At least one required n-gram from the **normalized pattern** is **absent** in that Bloom filter
5. If `len(pattern_normalized) < ngram_k`: **no exclusion** is allowed (all files remain candidates)

Bloom filters have false positives but no false negatives, so this exclusion rule cannot drop true matches.

### Reference DDL

```sql
PRAGMA foreign_keys = ON;

-- One row per Index Key.
-- Note: Per §5.2.2, glob[], case, fixed_strings, and word_regexp are query-time
-- filters and NOT part of the Index Key. They are applied at lookup time.
CREATE TABLE index_meta (
  index_key_hash          TEXT PRIMARY KEY,
  schema_version          INTEGER NOT NULL,

  root_path_canon         TEXT NOT NULL,

  -- Explicit key components (FR-LSI-IDX-KEY-02)
  -- Only traversal-affecting options are part of the key
  signature_hidden        INTEGER NOT NULL,
  signature_follow        INTEGER NOT NULL,
  signature_no_ignore     INTEGER NOT NULL,

  backend_id              TEXT NOT NULL,     -- e.g. "ugrep@3.9.0" or "rg@14.1.0"
  tokenizer_id            TEXT NOT NULL,     -- defines n-gram extraction + hashing
  casefold_strategy       TEXT NOT NULL CHECK (casefold_strategy IN ('ASCII', 'UNICODE_SIMPLE')),
  unicode_normalization   TEXT NOT NULL CHECK (unicode_normalization IN ('NONE','NFC','NFD')),

  created_at_ms           INTEGER NOT NULL,
  last_updated_at_ms      INTEGER NOT NULL,
  last_accessed_at_ms     INTEGER NOT NULL,

  safety_state            TEXT NOT NULL CHECK (
    safety_state IN ('ABSENT','BUILDING','COMPLETE','UNCERTAIN','CORRUPT','DISABLED')
  ),
  uncertain_reason        TEXT,              -- NULL when COMPLETE; else reason code per Appendix D
  disabled_until_ms       INTEGER,           -- nullable cooldown timestamp for DISABLED

  coverage_epoch          INTEGER NOT NULL,
  enumerate_error_count   INTEGER NOT NULL,

  -- Derived to avoid drift: do not store redundant booleans
  enumerate_ok            INTEGER
    GENERATED ALWAYS AS (CASE WHEN enumerate_error_count = 0 THEN 1 ELSE 0 END) STORED,

  -- Bloom filter parameters (FR-LSI-CAND-LIT-04)
  -- These are fixed at index creation and apply to all files in this key
  bloom_ngram_k           INTEGER NOT NULL,
  bloom_m_bits            INTEGER NOT NULL,
  bloom_k_hashes          INTEGER NOT NULL,
  bloom_hash_seed         INTEGER NOT NULL,

  bloom_target_fp_rate    REAL NOT NULL,     -- informational/diagnostic (sizing basis)
  bloom_basis_ngrams      INTEGER NOT NULL,  -- n used to size m/k

  max_tokenized_bytes     INTEGER NOT NULL
);

-- Inputs that can change eligibility/coverage (ignore files, global excludes).
CREATE TABLE eligibility_inputs (
  index_key_hash      TEXT NOT NULL,
  input_rel_path      TEXT NOT NULL,

  mtime_ns            INTEGER NOT NULL,
  size_bytes          INTEGER NOT NULL,

  -- Optional strong fingerprint (nullable); only populated if enabled.
  hash_algo           TEXT,                  -- e.g. "BLAKE3" (nullable)
  content_hash        BLOB,                  -- nullable

  PRIMARY KEY (index_key_hash, input_rel_path),
  FOREIGN KEY (index_key_hash) REFERENCES index_meta(index_key_hash) ON DELETE CASCADE
);

CREATE INDEX idx_elig_inputs_key ON eligibility_inputs(index_key_hash);

-- Canonical file catalog for the eligible set at last confirmed coverage_epoch.
CREATE TABLE file_entries (
  file_rowid          INTEGER PRIMARY KEY AUTOINCREMENT,
  index_key_hash      TEXT NOT NULL,

  rel_path            TEXT NOT NULL,
  norm_path           TEXT NOT NULL,
  path_sort_key       TEXT NOT NULL,         -- for deterministic ordering (FR-LSI-ORD-03)

  size_bytes          INTEGER NOT NULL,
  mtime_ns            INTEGER NOT NULL,

  -- Used for rename/move detection / reuse of tokens across renames (FR-LSI-TRK-09)
  stable_file_id      BLOB,                  -- nullable; inode/device or Windows file ID

  readable            INTEGER NOT NULL CHECK (readable IN (0,1)),
  eligible            INTEGER NOT NULL CHECK (eligible IN (0,1)),
  is_binary           INTEGER NOT NULL CHECK (is_binary IN (0,1)),

  token_status        TEXT NOT NULL CHECK (
    token_status IN ('FULL','SKIPPED_TOO_LARGE','SKIPPED_BINARY','UNREADABLE','ERROR')
  ),
  tokenized_bytes     INTEGER NOT NULL,
  token_complete      INTEGER NOT NULL CHECK (token_complete IN (0,1)),

  last_indexed_epoch  INTEGER NOT NULL,

  UNIQUE(index_key_hash, rel_path),
  FOREIGN KEY (index_key_hash) REFERENCES index_meta(index_key_hash) ON DELETE CASCADE
);

CREATE INDEX idx_file_entries_key ON file_entries(index_key_hash);
CREATE INDEX idx_file_entries_order ON file_entries(index_key_hash, path_sort_key);

-- Supports reconcile/rename by stable file identity
CREATE INDEX idx_file_entries_stable_id
  ON file_entries(index_key_hash, stable_file_id);

-- Per-file Bloom filters. Variants are tied to request semantics (FR-LSI-CAND-LIT-05).
CREATE TABLE file_bloom (
  file_rowid          INTEGER NOT NULL,
  variant             TEXT NOT NULL CHECK (variant IN ('SENSITIVE','INSENSITIVE')),

  ngram_k             INTEGER NOT NULL,
  m_bits              INTEGER NOT NULL,
  k_hashes            INTEGER NOT NULL,
  hash_seed           INTEGER NOT NULL,

  bitset              BLOB NOT NULL,
  updated_at_ms       INTEGER NOT NULL,

  PRIMARY KEY (file_rowid, variant),
  FOREIGN KEY (file_rowid) REFERENCES file_entries(file_rowid) ON DELETE CASCADE
);

-- Enforce parameter consistency with index_meta via trigger (FR-LSI-CAND-LIT-04)
CREATE TRIGGER trg_file_bloom_params_match
BEFORE INSERT ON file_bloom
FOR EACH ROW
WHEN EXISTS (
  SELECT 1 FROM file_entries f
  JOIN index_meta m ON f.index_key_hash = m.index_key_hash
  WHERE f.file_rowid = NEW.file_rowid
    AND (m.bloom_ngram_k != NEW.ngram_k
      OR m.bloom_m_bits != NEW.m_bits
      OR m.bloom_k_hashes != NEW.k_hashes
      OR m.bloom_hash_seed != NEW.hash_seed)
)
BEGIN
  SELECT RAISE(ABORT, 'file_bloom params mismatch with index_meta');
END;

-- Dirty queue for incremental updates (FR-LSI-TRK-10).
CREATE TABLE dirty_queue (
  index_key_hash      TEXT NOT NULL,
  rel_path            TEXT NOT NULL,
  dirty_kind          TEXT NOT NULL CHECK (dirty_kind IN ('FILE','SUBTREE')),
  queued_at_ms        INTEGER NOT NULL,

  PRIMARY KEY (index_key_hash, rel_path),
  FOREIGN KEY (index_key_hash) REFERENCES index_meta(index_key_hash) ON DELETE CASCADE
);

CREATE INDEX idx_dirty_queue_key ON dirty_queue(index_key_hash);
-- Supports oldest-first processing without full scan (FR-LSI-TRK-10)
CREATE INDEX idx_dirty_queue_time ON dirty_queue(index_key_hash, queued_at_ms);
```

### Literal-Search Exclusion Query (Conceptual)

**Variant selection (deterministic per FR-LSI-CAND-LIT-05):**

1. Determine `variant` for this query (`SENSITIVE` or `INSENSITIVE`) using the backend-equivalent case/smartcase rules and the stored `casefold_strategy`.
2. Normalize `pattern` using the same `unicode_normalization` and casefolding.

**Candidate set query:**

1. Start with all eligible files in deterministic order:
   ```sql
   SELECT * FROM file_entries
   WHERE index_key_hash = ?
     AND eligible = 1
     AND readable = 1
   ORDER BY path_sort_key
   ```

2. Exclude only those files that meet the full token gating and fail the Bloom test:
   * If `len(pattern_normalized) < bloom_ngram_k`: **skip exclusion entirely** (all remain candidates per FR-LSI-CAND-LIT-03).
   * Else:
     * For each file where `token_status='FULL' AND token_complete=1` and a `file_bloom` row exists for `variant`,
     * compute the pattern's n-grams and test membership against `file_bloom.bitset`;
     * if **any** required n-gram is absent → file may be excluded.

---

# Appendix F: Tokenizer and Bloom Filter Algorithm (Normative)

This appendix defines the normative tokenization and Bloom filter construction algorithm to ensure deterministic and consistent behavior across implementations.

### F.1 Configuration Parameters

| Parameter | Default | Description |
| --------- | ------- | ----------- |
| `bloom_ngram_k` | 3 | N-gram size (k-grams) |
| `bloom_target_fp_rate` | 0.01 | Target false positive rate |
| `index_max_tokenized_bytes` | 1048576 (1 MiB) | Max bytes to tokenize per file |

### F.2 Tokenization Pipeline

**FR-LSI-TOK-01 (Input decoding):** Tokenization operates on decoded text, not raw bytes:

1. Read file content as bytes (up to `index_max_tokenized_bytes`).
2. Decode as UTF-8. Replace invalid byte sequences with U+FFFD (replacement character).
3. **Binary detection:** Compute `replacement_ratio = count(U+FFFD) / max(1, len(decoded_text_in_unicode_scalars))`. If `replacement_ratio > 0.5`, treat file as binary and skip tokenization (`token_status='SKIPPED_BINARY'`). Empty files (zero scalars) are NOT binary.
4. If file is larger than `index_max_tokenized_bytes`: set `token_status='SKIPPED_TOO_LARGE'`.

**FR-LSI-TOK-02 (Unicode normalization):** Apply Unicode normalization **NFC** (Canonical Decomposition, followed by Canonical Composition) to the decoded text before n-gram extraction.

**FR-LSI-TOK-03 (Newline handling):**
1. Normalize CRLF (`\r\n`) to LF (`\n`) before tokenization.
2. Newlines (`\n`) are included in the token stream and MAY appear within n-grams.
3. N-grams that cross line boundaries are valid and MUST be included.

**FR-LSI-TOK-04 (Casefolding for INSENSITIVE variant):**

The `casefold_strategy` MUST be one of the following permitted values:

| Strategy | Description | Backend Compatibility |
| -------- | ----------- | --------------------- |
| `ASCII` | Casefold ASCII letters only (`A-Z` → `a-z`). Fastest; matches most backend defaults. | ugrep `-i`, ripgrep `-i` (default ASCII mode) |
| `UNICODE_SIMPLE` | Unicode simple casefolding per Unicode Standard Annex #21. Maps each codepoint independently (no locale). | ugrep `--ignore-case` with Unicode mode; ripgrep `-i --no-unicode` disabled |

**Selection rule:**
1. Select the `casefold_strategy` that **exactly matches** the backend's `case=insensitive` semantics for the configured `backend_id`.
2. If the backend behavior cannot be determined or matched safely, candidate exclusion MUST be disabled for `case=insensitive` and `case=smart` queries.
3. Store `casefold_strategy` in metadata (Appendix E) to detect mismatches on index open.

**Casefolding algorithm for `ASCII`:**
```
casefold_ascii(c) = if c in 'A'..'Z' then c + 32 else c
```

**Casefolding algorithm for `UNICODE_SIMPLE`:**
Apply Unicode simple casefolding mapping table (Unicode Character Database, CaseFolding.txt, type 'C' and 'S' mappings).

**FR-LSI-TOK-05 (N-gram extraction):**
1. Extract all contiguous k-character n-grams from the normalized text.
2. If `len(text) < k`: no n-grams are extracted; `token_complete=0`.
3. Number of n-grams: `max(0, len(text) - k + 1)`.

### F.3 Bloom Filter Construction

**FR-LSI-BLOOM-01 (Sizing formula):** Given expected n-gram count `n` and target false positive rate `p`:

```
m = ceil(-n * ln(p) / (ln(2)^2))        # bit count
k_raw = (m / n) * ln(2)                   # optimal hash count (floating point)
k_hashes = max(1, floor(k_raw + 0.5))     # round to nearest integer, minimum 1
```

**Determining `n` (expected unique n-grams):** For index-wide Bloom filter parameters, `n` MUST be computed deterministically from configuration:

```
n = floor(index_max_tokenized_bytes / 2)   # conservative estimate: ~2 bytes per unique n-gram
```

This fixed formula ensures all files within an Index Key use the same `m_bits` and `k_hashes`, satisfying FR-LSI-CAND-LIT-04.

**Stored parameters:**
* `n` = `bloom_basis_ngrams` (the n used for sizing)
* `p` = `bloom_target_fp_rate` (default 0.01)
* `m` = `bloom_m_bits` (bit count)
* `k_hashes` = `bloom_k_hashes` (hash function count)

**Example with defaults:** For `index_max_tokenized_bytes = 1048576` and `bloom_target_fp_rate = 0.01`:
* `n = 524288`
* `m = ceil(-524288 * ln(0.01) / (ln(2)^2)) = 5021884` bits ≈ 614 KiB per file
* `k_hashes = max(1, floor((5021884 / 524288) * ln(2) + 0.5)) = 7`

**FR-LSI-BLOOM-02 (Hash function):** Use double hashing with **xxHash64** (required, not optional):

1. Compute `h1 = xxHash64(ngram_bytes, seed=0)`.
2. Compute `h2 = xxHash64(ngram_bytes, seed=bloom_hash_seed)` where `bloom_hash_seed` defaults to `0x9E3779B97F4A7C15` (golden ratio-derived constant).
3. For each `i` ∈ {0, 1, 2, ..., k_hashes−1}: compute `bit_index = (h1 + i * h2) mod m`.
4. Set the corresponding bit in the bitset.

> **Critical:** The loop iterates exactly `k_hashes` times, from `i=0` to `i=k_hashes-1` inclusive. This sets exactly `k_hashes` bits per n-gram (accounting for collisions).

**FR-LSI-BLOOM-03 (Bit storage):** The bitset MUST be stored as a packed byte array with the following layout:

* Total bytes: `ceil(m / 8)`.
* Bit indexing: `byte_index = bit_index / 8` (integer division), `bit_offset = bit_index % 8`.
* Bit ordering: Little-endian within each byte. Bit 0 is the LSB. To set: `bitset[byte_index] |= (1 << bit_offset)`.
* Unused bits: Bits at indices ≥ m in the final byte MUST be zero and MUST be ignored during lookups.

**FR-LSI-BLOOM-04 (Variant construction):** Build two Bloom filters per file:
* `SENSITIVE`: n-grams from NFC-normalized text.
* `INSENSITIVE`: n-grams from NFC-normalized + casefolded text.

### F.4 Query-Time Matching

**FR-LSI-BLOOM-Q-01 (Pattern normalization):**
1. Apply the same NFC normalization to the pattern.
2. For `case=insensitive` or `case=smart` with lowercase pattern: apply casefolding.
3. Extract n-grams from the normalized pattern.

**FR-LSI-BLOOM-Q-02 (Membership test):**
1. For each pattern n-gram, compute bit indices using the same double-hashing.
2. If **any** n-gram has **any** bit unset in the file's Bloom filter: file MAY be excluded.
3. If **all** pattern n-grams have all bits set: file MUST remain a candidate (possible false positive).

### F.5 Minimum SQLite Version

The reference schema in Appendix E uses generated columns and triggers. Implementations MUST target **SQLite 3.31.0** or later for full compatibility. If targeting older SQLite versions, implementations MUST either:
* Avoid generated columns and compute `enumerate_ok` in application code, OR
* Document the minimum version requirement and fail gracefully on incompatible SQLite.

---

# Appendix G: Backend File-List Ingestion (Normative)

This appendix specifies how to pass candidate file lists to supported backends when using index-based candidate filtering.

### G.1 Overview

When the index excludes files from the candidate set, the remaining candidates must be passed to the backend. For large candidate sets, command-line argument limits may be exceeded. This appendix defines scalable mechanisms per backend.

### G.2 ugrep File-List Mode

**Minimum version:** ugrep 3.0+

**Mechanism:** Use the `--include-from=FILE` option or stdin with `--include-from=-`.

**FR-LSI-BE-UG-01:** When using ugrep with candidate filtering:

```bash
# Via temp file
ugrep [options] --include-from=/path/to/candidates.txt pattern

# Via stdin
cat candidates.txt | ugrep [options] --include-from=- pattern
```

**FR-LSI-BE-UG-02:** The candidates file MUST contain one file path per line (relative to the search root or absolute).

**FR-LSI-BE-UG-03 (Newline-in-path policy):** Paths containing newline characters (`\n` or `\r`) MUST be **rejected**:
1. During enumeration: files with newlines in their names MUST NOT be added to the index catalog.
2. Such files are treated as if they do not exist for indexing purposes.
3. These files remain searchable via direct backend invocation (without candidate filtering), preserving baseline equivalence.
4. The presence of such files does NOT prevent the index from reaching `COMPLETE` state.

> **Rationale:** Newline escaping is not universally supported by backends' file-list modes. Rejection is the only safe, deterministic choice.

### G.3 ripgrep File-List Mode

**Minimum version:** ripgrep 13.0+

**Mechanism:** Use the `--files-from=FILE` option.

**FR-LSI-BE-RG-01:** When using ripgrep with candidate filtering:

```bash
# Via temp file
rg [options] --files-from=/path/to/candidates.txt pattern

# Via stdin (if supported by version)
rg [options] --files-from=- pattern < candidates.txt
```

**FR-LSI-BE-RG-02:** The candidates file MUST contain one file path per line. The newline-in-path policy from FR-LSI-BE-UG-03 applies equally to ripgrep.

**FR-LSI-BE-RG-03:** ripgrep's `--files-from` expects paths relative to the current directory or absolute paths. Ensure consistency with the search root.

### G.4 Capability Detection

**FR-LSI-BE-CAP-01:** At startup, the implementation MUST detect backend capabilities:

1. Parse `ugrep --version` or `rg --version` output.
2. Extract version number using regex that captures full semver:
   * ugrep: `ugrep\s+(\d+)\.(\d+)(?:\.(\d+))?` → captures major, minor, optional patch
   * ripgrep: `ripgrep\s+(\d+)\.(\d+)(?:\.(\d+))?` → captures major, minor, optional patch
3. Compare against minimum version requirements using **semantic version comparison** (major, then minor, then patch; missing patch treated as 0).
4. Store capability signature in `backend_id` (e.g., `ugrep@3.9.0`, `rg@14.1.0`).

**FR-LSI-BE-CAP-02:** If file-list mode is unavailable (version too old or unsupported backend), index-based candidate exclusion MUST be disabled for that configuration. Searches proceed without exclusion.

### G.5 Temporary File Management

**FR-LSI-BE-TMP-01:** Temporary candidate list files MUST:
1. Be created in an approved temp/cache directory (NOT in the searched tree).
2. Use restrictive permissions (user-only on POSIX).
3. Be deleted after the search completes (success or failure).

**FR-LSI-BE-TMP-02:** If temp file creation fails, the tool MUST fall back to non-indexed execution.

### G.6 Exit Code Mapping

| Exit Code | ugrep | ripgrep | Classification |
| --------- | ----- | ------- | -------------- |
| 0 | Matches found | Matches found | `ok_with_matches` |
| 1 | No matches | No matches | `ok_no_matches` |
| 2 | Error (syntax, IO) | Error | `backend_error` |
| >2 | Varies | Varies | `backend_error` |

**FR-LSI-BE-EXIT-01:** The implementation MUST classify exit codes per this table. Unknown exit codes MUST be treated as `backend_error`.

**FR-LSI-BE-EXIT-02:** Fuzzy fallback (§9) MUST trigger only on `ok_no_matches` (exit code 1).

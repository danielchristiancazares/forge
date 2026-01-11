# search_files Indexing and Fuzzy Fallback

## Software Requirements Document

**Version:** 1.0
**Date:** 2026-01-08
**Status:** Draft
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log

### 0.1 Initial version

- Initial SRD for `search_files` indexing, change tracking, and fuzzy fallback.

---

## 1. Introduction

### 1.1 Purpose

This document specifies requirements for indexing, change tracking, and fuzzy fallback behavior used by the `search_files` tool. It extends `docs/SEARCH_FILES_SRD.md` with performance-focused strategies while preserving the tool's deterministic outputs and semantics.

### 1.2 Scope

**In Scope**

- Index modes and lifecycle
- Change tracking and incremental updates
- Candidate filtering versus final matching
- Fuzzy fallback behavior and thresholds
- Configuration keys and defaults
- Test and validation requirements

**Out of Scope**

- UI rendering changes
- Provider-specific tool-call adapter behavior
- Network or remote filesystem search
- Modifying the `search_files` request JSON schema

### 1.3 Definitions

| Term             | Definition                                                      |
| ---------------- | --------------------------------------------------------------- |
| Index            | Data structure used to accelerate candidate file selection      |
| Fingerprint      | File identity tuple (mtime, size, inode if available)           |
| Dirty Set        | Files flagged for reindex due to change events                  |
| Reconcile Scan   | Bounded walk to revalidate fingerprints after watcher overflow  |
| Candidate Set    | Files selected by the index for final content scanning          |
| Fuzzy Pass       | One attempt at approximate matching with a specific threshold   |
| Threshold Step   | Decrease applied between successive fuzzy passes                |

### 1.4 References

| Document                    | Description                                |
| --------------------------- | ------------------------------------------ |
| `docs/SEARCH_FILES_SRD.md`  | Core tool requirements and schema          |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework requirements      |
| `docs/DESIGN.md`           | Type-driven design patterns and invariants |
| RFC 2119 / RFC 8174         | Requirement level keywords                 |

### 1.5 Requirement Level Keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in RFC 2119 and RFC 8174.

---

## 2. Overall Description

### 2.1 Product Perspective

Indexing and change tracking are optional performance features for `search_files`. They improve lookup speed by avoiding full rescans while guaranteeing identical results to a full scan for the same inputs and filesystem state.

### 2.2 Product Functions

| Function    | Description                                           |
| ----------- | ----------------------------------------------------- |
| FR-SFI-IDX  | Index modes, storage, and lifecycle                   |
| FR-SFI-TRK  | Change tracking and incremental updates               |
| FR-SFI-PRE  | Candidate filtering and final match validation        |
| FR-SFI-FUZ  | Fuzzy fallback passes and thresholding                |
| FR-SFI-OUT  | Output metadata related to indexing and fallback      |

### 2.3 Constraints

| Constraint | Rationale                                                  |
| ---------- | ---------------------------------------------------------- |
| C-01       | MUST be cross-platform (Windows/macOS/Linux)               |
| C-02       | MUST respect sandbox path validation before file access    |
| C-03       | Results MUST match a full scan for the same inputs         |
| C-04       | Output MUST remain deterministic and schema-compliant      |
| C-05       | No network access                                          |
| C-06       | Indexing MUST NOT traverse symlinked directories           |

---

## 3. Functional Requirements

### 3.1 Index Modes and Storage (FR-SFI-IDX)

- **FR-SFI-IDX-01:** The system MUST support `index_mode` values `off`, `auto`, and `on`.
- **FR-SFI-IDX-02:** `index_mode=off` MUST disable all indexing and change tracking features.
- **FR-SFI-IDX-03:** `index_mode=auto` MUST enable indexing only when repository size exceeds configured thresholds (CFG-SFI-02).
- **FR-SFI-IDX-04:** `index_mode=on` MUST enable indexing regardless of repository size.
- **FR-SFI-IDX-05:** Indexing MUST be advisory only; if an index is unavailable, stale, or corrupted, the search MUST fall back to a full scan.
- **FR-SFI-IDX-06:** The index MUST be keyed by normalized root path and traversal options that affect the file set (e.g., `respect_gitignore`, `include_hidden`, `follow_symlinks`, glob filters).
- **FR-SFI-IDX-07:** The index MUST NOT store full file contents by default; it MAY store derived tokens or hashes sufficient for candidate selection.
- **FR-SFI-IDX-08:** Persisted indexes MUST be stored outside the searched tree by default to avoid self-inclusion.

### 3.2 Change Tracking and Incremental Updates (FR-SFI-TRK)

- **FR-SFI-TRK-01:** The system MUST track file fingerprints and update index entries only when fingerprints change.
- **FR-SFI-TRK-02:** When file watcher events are available, they SHOULD be used to populate a dirty set and drive incremental updates.
- **FR-SFI-TRK-03:** If watcher overflow or failure is detected, the system MUST perform a reconcile scan to revalidate fingerprints without forcing a full reindex.
- **FR-SFI-TRK-04:** Deleted files MUST be removed from the index during incremental updates or reconcile scans.
- **FR-SFI-TRK-05:** If traversal-affecting configuration changes, the system MUST invalidate the affected index and rebuild or fall back to full scan.

### 3.3 Candidate Filtering and Final Matching (FR-SFI-PRE)

- **FR-SFI-PRE-01:** Index-based candidate filtering MUST be a superset of true matches. It MUST NOT introduce false negatives.
- **FR-SFI-PRE-02:** If the index cannot guarantee a superset for a given search mode, the system MUST bypass the index and scan all eligible files.
- **FR-SFI-PRE-03:** Final matching MUST re-read file content and apply `search_files` semantics (exact/regex/fuzzy, binary detection, size caps, gitignore).
- **FR-SFI-PRE-04:** Results MUST still be ordered by `path`, then `line`, then `column`.
- **FR-SFI-PRE-05:** Indexing MUST respect `max_files`, `max_depth`, and `max_file_size_bytes` when selecting candidates.

### 3.4 Fuzzy Fallback Behavior (FR-SFI-FUZ)

- **FR-SFI-FUZ-01:** Fuzzy fallback MUST be opt-in via configuration (CFG-SFI-06).
- **FR-SFI-FUZ-02:** When enabled and the requested mode is `exact`, the tool MUST run an exact search first.
- **FR-SFI-FUZ-03:** If the exact search returns zero matches and is not truncated, the tool MUST run up to `fuzzy_passes` fuzzy passes, stopping after the first pass that yields matches.
- **FR-SFI-FUZ-04:** Each subsequent pass MUST reduce the effective threshold by `fuzzy_threshold_step` (clamped to >= 0.0) or an equivalent increase in allowed edit distance.
- **FR-SFI-FUZ-05:** Fuzzy passes MUST respect all standard limits (`max_results`, `max_matches_per_file`, `max_files`, `max_file_size_bytes`).
- **FR-SFI-FUZ-06:** If the backend provides fuzzy matching (e.g., `ugrep`), it SHOULD be used for candidate discovery; final match extraction MUST still produce accurate line, column, and context per `search_files` requirements.
- **FR-SFI-FUZ-07:** If fuzzy candidate filtering cannot guarantee a superset, fuzzy passes MUST scan all eligible files.

### 3.5 Output Metadata (FR-SFI-OUT)

- **FR-SFI-OUT-01:** Output MUST remain compliant with `docs/SEARCH_FILES_SRD.md` and its required fields.
- **FR-SFI-OUT-02:** If fuzzy fallback produces results, the response `mode` MUST be `fuzzy`.
- **FR-SFI-OUT-03:** Optional indexing or fallback metadata MUST be added only under `stats` and MUST NOT remove or rename required fields.
- **FR-SFI-OUT-04:** When fallback occurs, `stats` SHOULD include `initial_mode`, `fallback_used`, and `fuzzy_passes_used`.

---

## 4. Non-Functional Requirements

### 4.1 Performance

- **NFR-SFI-PERF-01:** Incremental updates SHOULD be proportional to changed files, not total files.
- **NFR-SFI-PERF-02:** Index warmup SHOULD be lazy by default to avoid blocking tool calls at startup.
- **NFR-SFI-PERF-03:** When enabled, indexing SHOULD reduce the number of files read for exact and regex searches.

### 4.2 Reliability

- **NFR-SFI-REL-01:** Searches MUST return correct results even if the index is partially missing or stale.
- **NFR-SFI-REL-02:** Index corruption MUST be detected and trigger fallback or rebuild without failing the tool call.

### 4.3 Security and Privacy

- **NFR-SFI-SEC-01:** Index storage MUST not bypass sandbox access controls.
- **NFR-SFI-SEC-02:** Persisted indexes SHOULD minimize stored content and avoid sensitive raw text by default.

### 4.4 Portability

- **NFR-SFI-PORT-01:** Change tracking MUST degrade gracefully on platforms without reliable filesystem events.
- **NFR-SFI-PORT-02:** Paths MUST be normalized consistently across platforms.

---

## 5. Configuration

**CFG-SFI-01:** The system SHALL support indexing configuration under `[tools.search_files]`.

Example:

```toml
[tools.search_files]
index_mode = "auto"                 # off | auto | on
index_storage = "memory"            # memory | sqlite
index_path = ""                     # optional; used when sqlite
index_auto_threshold_files = 2000
index_auto_threshold_bytes = 500000000
index_watch = true
index_watch_debounce_ms = 250
index_use_git = true

fuzzy_fallback = false
fuzzy_passes = 3
fuzzy_threshold_step = 0.1
```

**CFG-SFI-02:** `index_auto_threshold_files` and `index_auto_threshold_bytes` are soft thresholds used only when `index_mode=auto`.
**CFG-SFI-03:** If `index_storage=sqlite` and `index_path` is empty, a default cache path MUST be chosen outside the searched tree.
**CFG-SFI-04:** If `index_use_git=true`, git-aware file listing MAY be used when `respect_gitignore=true`.
**CFG-SFI-05:** If file watching is disabled or unavailable, the system MUST rely on reconcile scans and fingerprint checks.
**CFG-SFI-06:** `fuzzy_fallback` defaults to false and MUST NOT change tool behavior unless explicitly enabled.

---

## 6. Test and Validation Requirements

| ID           | Scenario                                                        | Expected Result                                   |
| ------------ | --------------------------------------------------------------- | ------------------------------------------------ |
| T-SFI-01     | Index mode off                                                  | Full scan results match baseline                 |
| T-SFI-02     | Index auto below thresholds                                     | Index not used                                   |
| T-SFI-03     | Index auto above thresholds                                     | Index used; results match baseline               |
| T-SFI-04     | File modified after index build                                 | Reindexed; matches reflect new content           |
| T-SFI-05     | File deleted                                                    | Index entry removed; no stale matches            |
| T-SFI-06     | Watcher overflow                                                | Reconcile scan restores correctness              |
| T-SFI-07     | Index corruption                                                | Fallback to full scan without tool failure       |
| T-SFI-08     | Fuzzy fallback disabled                                         | Exact search only                                |
| T-SFI-09     | Exact search no matches with fallback enabled                   | Fuzzy passes executed up to limit                |
| T-SFI-10     | Fuzzy pass finds matches                                        | Stops further passes; mode reported as fuzzy     |

---

## 7. Open Questions

- Should fuzzy fallback be exposed as a first-class `mode="auto"` in `search_files`, or remain configuration-only?
- How should `stats` metadata be standardized without expanding the base SRD output schema?
- Should the persistent index be scoped per workspace, per sandbox, or per tool instance?
- Should indexing store token-level data (trigram/FTS) or remain a file list plus fingerprints?
- How should indexing interact with the `read_file` stale-file cache to avoid implicit edit permissions?
- When `ugrep` is the backend, which fuzzy mode provides the best speed/accuracy tradeoff for fallback?

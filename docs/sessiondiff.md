# Session Diff

## Purpose

Git diff can be modified outside a session — by hooks, formatters, other processes, or the user. This document captures changes made by the assistant within a single Forge session as an independent audit trail. If `git diff` disagrees with this record, the discrepancy itself is the signal.

## Format

Each session appends a section with:
- **Session ID**: Timestamp or unique identifier
- **Files touched**: Only files the assistant created or edited (not pre-existing dirty files)
- **Change summary**: What was done and why, at the granularity needed to verify against `git diff`

Pre-existing dirty files observed at session start are noted but not detailed (they aren't ours to track).

## Session: 2026-02-05T05:xx UTC

### Pre-existing dirty files (not ours)

All files listed in `git status` at session start were already modified. The assistant did not touch these unless listed below.

### Files changed

#### `docs/bugs.md` (new file)
- Documented Search tool `--files-from` ripgrep bug when using path-scoped glob patterns

#### `providers/src/sse_types.rs` (modified)
- Added `StopReason` enum (`EndTurn`, `MaxTokens`, `StopSequence`, `ToolUse`, `Compaction`, `Unknown`) with `#[serde(other)]` forward compat
- Added `MessageDeltaInfo` struct with `stop_reason: Option<StopReason>`
- Added `delta: Option<MessageDeltaInfo>` field to `Event::MessageDelta` variant
- Added 3 tests: `deserialize_message_delta_with_compaction_stop_reason`, `deserialize_message_delta_with_end_turn_stop_reason`, `deserialize_message_delta_unknown_stop_reason`

#### `providers/src/lib.rs` (modified)
- `anthropic_beta_header`: Changed Opus 4.6 return from `"context-1m-2025-08-07"` to `"context-1m-2025-08-07,compact-2026-01-12"`
- `build_request_body`: Added `context_management.edits` with `{"type": "compact_20260112"}` in the Opus 4.6 block
- `ClaudeParser::parse`: Destructured new `delta` field from `MessageDelta`; logs `tracing::info!` when `stop_reason` is `Compaction`
- Updated test `opus_4_6_always_uses_adaptive_and_max_effort`: Changed assertion from `context_management` being absent to verifying `compact_20260112` is present
- Updated test `anthropic_beta_header_sets_context_1m_for_opus_4_6`: Updated expected value to include `compact-2026-01-12`

#### `engine/src/streaming.rs` (modified)
- Replaced first-N cache placement with geometric grid strategy
- Added `CACHE_BREAKPOINT_GRID` constant: `[3, 7, 15, 23, 31, 47, 63, 95, 127, 191, 255, 383, 511, 767, 1023]`
- Added `cache_breakpoint_positions(eligible) -> Vec<usize>` — selects highest 3 grid points that fit within eligible range
- Replaced `max_cached`/`recent_threshold`/`cached_count` logic with `breakpoints.contains(&i)` lookup
- Added 8 unit tests in `cache_breakpoint_tests` module: empty, small, single, medium, stability, boundary shift, large, max

#### `providers/src/lib.rs` — `ToolResult` cache support (modified)
- `ToolResult` handler now applies `cache_control: {type: "ephemeral"}` when `cache_hint` is `Ephemeral`
- Previously the hint was ignored, wasting breakpoint slots on ToolResult messages

#### `docs/bugs.md` (new file)
- Documented Search tool `--files-from` ripgrep bug

#### `docs/sessiondiff.md` (new file, this file)

### Test results

- `cargo test -p forge-providers` — 106 passed, 0 failed
- `cargo test -p forge-engine` — 381 passed, 0 failed (373 existing + 8 new)

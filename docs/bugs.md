# Bugs

## 2026-02-15 Focus Mode Review

### P1: Include history items when building focus review blocks

- File: `tui/src/focus/content.rs:13-15`
- Problem: `extract_blocks()` only processes `DisplayItem::Local`. Normal assistant, thinking, and tool-result messages are persisted as `DisplayItem::History` via `push_history_message*`, so Focus review can miss or lose model output and render empty content.
- Expected: Resolve history IDs through `app.history()` the same way classic rendering does.

### P1: Avoid forcing executing focus state on non-plan streams

- File: `engine/src/streaming.rs:245-249`
- Problem: `start_streaming()` always sets Focus mode to `FocusState::Executing`, but the executing renderer returns immediately when `PlanState::Inactive` (common for non-plan chats), leaving the Focus pane blank during normal streaming turns.
- Expected: Only transition to executing when a plan is active, or provide a non-plan executing fallback view.

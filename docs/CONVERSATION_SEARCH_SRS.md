# Conversation Search

## Software Requirements Document

**Version:** 1.0
**Date:** 2026-01-09
**Status:** Draft

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-18 | Header & TOC |
| 19-24 | 0. Change Log |
| 25-61 | 1. Introduction |
| 62-99 | 2. Overall Description |
| 100-200 | 3. Functional Requirements |
| 201-217 | 4. Non-Functional Requirements |
| 218-269 | 5. Data Structures |
| 270-300 | 6. UI Layout |
| 301-327 | 7. State Transitions |
| 328-360 | 8. Implementation Checklist |
| 361-383 | 9. Verification Requirements |
| 384-399 | 10. Future Considerations |

---

## 0. Change Log

### 0.1 Initial draft

* Initial requirements for in-conversation search functionality.

---

## 1. Introduction

### 1.1 Purpose

Define requirements for searching within the current conversation, allowing users to find and navigate to specific content in message history.

### 1.2 Scope

The Conversation Search feature will:

* Search across all messages in the current conversation (user, assistant, system, tool)
* Highlight matches and allow navigation between them
* Integrate with vim-modal input paradigm

Out of scope:

* Cross-session/history search (searching past conversations)
* Semantic/fuzzy search (exact substring/regex only for v1)
* Search-and-replace

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| Match | A substring in message content matching the search pattern |
| Hit | A message containing one or more matches |
| Active match | The currently focused match for navigation |

### 1.4 References

| Document | Description |
| --- | --- |
| `tui/README.md` | TUI architecture and extension guide |
| `engine/src/lib.rs` | InputMode enum, App state machine |
| `tui/src/input.rs` | Input handling patterns |
| `tui/src/lib.rs` | Full-screen rendering |

### 1.5 Requirement Keywords

The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective

Conversation Search is a TUI-only feature integrated into the full-screen alternate-screen mode. Inline mode presents architectural challenges (content pushed to terminal history cannot be overlaid), so search will be full-screen only in v1.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-CS-ENTER | Enter search mode from normal mode |
| FR-CS-INPUT | Accept and refine search query incrementally |
| FR-CS-MATCH | Find and highlight all matches in conversation |
| FR-CS-NAV | Navigate between matches (next/prev) |
| FR-CS-JUMP | Jump to selected match in message view |
| FR-CS-EXIT | Exit search mode, optionally preserving scroll position |

### 2.3 User Characteristics

* Users familiar with vim `/` search paradigm
* Users expect `n`/`N` for next/prev match navigation
* Users expect `Enter` to confirm and `Esc` to cancel

### 2.4 Constraints

* Full-screen (alternate screen) mode only for v1
* Must not block streaming or tool execution
* Must handle large conversations without UI lag

### 2.5 Inline Mode Consideration

Inline mode uses `terminal.insert_before()` to push content above the input area. This architecture does not support overlays or re-rendering previous output. Options for future versions:

| Option | Trade-off |
| --- | --- |
| Disable search in inline | Simple, but reduces functionality |
| Temporary alternate screen | Context switch, but full search UX |
| Push results to history | Poor UX, results scroll away |

**Decision:** v1 will use the full-screen alternate-screen mode for the search UI.

---

## 3. Functional Requirements

### 3.1 Search Mode Entry

**FR-CS-01:** A new `InputMode::Search` variant MUST be added to the engine.

**FR-CS-02:** In Normal mode, pressing `/` MUST enter Search mode (currently enters Command mode; see FR-CS-03).

**FR-CS-03:** The existing `/` → Command mode binding MUST be changed. Options:

| Binding | Search | Command |
| --- | --- | --- |
| Option A | `/` | `:` only |
| Option B | `Ctrl+/` or `?` | `/` and `:` |
| Option C | `/` (with empty = command) | `/` then `:` prefix |

**Recommended:** Option A — `/` for search, `:` for commands. This matches vim semantics.

**FR-CS-04:** The `/` key MUST be ignored (with status message) when in inline TUI mode.

### 3.2 Search Input

**FR-CS-05:** Search mode MUST display a search prompt at the bottom of the screen:

```
/pattern_here█
```

**FR-CS-06:** Search MUST be incremental — matches update as the user types.

**FR-CS-07:** Search input MUST support:

* Character insertion at cursor
* Backspace/Delete
* Cursor movement (Left/Right/Home/End)
* `Ctrl+U` to clear line
* `Ctrl+W` to delete word backwards

**FR-CS-08:** Search pattern MUST be case-insensitive by default.

**FR-CS-09:** Search MAY support case-sensitive mode via `\C` suffix or similar (future).

**FR-CS-10:** Search MAY support regex mode via prefix (future).

### 3.3 Match Display

**FR-CS-11:** All matches in the message area MUST be highlighted with a distinct style (e.g., `colors::SEARCH_MATCH` background).

**FR-CS-12:** The active match (current navigation target) MUST be highlighted with a different style (e.g., `colors::SEARCH_ACTIVE`).

**FR-CS-13:** A match count MUST be displayed in the search prompt or status bar:

```
/pattern█  [3/17]
```

Format: `[active_index/total_matches]` or `[total_matches matches]` if no active.

**FR-CS-14:** If no matches exist, the prompt MUST indicate this:

```
/pattern█  [No matches]
```

**FR-CS-15:** Matches MUST span the following message types:

* User messages (content)
* Assistant messages (content)
* System messages (content)
* Tool use (name and arguments JSON)
* Tool result (content)

### 3.4 Match Navigation

**FR-CS-16:** Pressing `Enter` in search mode MUST:

1. Confirm the search
2. Jump to the first match (or next match from current scroll position)
3. Transition to Normal mode with search highlights preserved

**FR-CS-17:** In Normal mode with active search, pressing `n` MUST jump to the next match.

**FR-CS-18:** In Normal mode with active search, pressing `N` (Shift+n) MUST jump to the previous match.

**FR-CS-19:** Navigation MUST wrap around (last match → first match and vice versa).

**FR-CS-20:** Jumping to a match MUST scroll the message view to show the match in the visible area (vertically centered if possible).

### 3.5 Search Exit

**FR-CS-21:** Pressing `Esc` in Search mode MUST:

1. Clear the search pattern
2. Remove all highlights
3. Return to Normal mode at previous scroll position

**FR-CS-22:** Pressing `Esc` in Normal mode with active search MUST clear the search state (remove highlights).

**FR-CS-23:** Starting a new search MUST clear any previous search state.

**FR-CS-24:** Sending a new message MUST clear search state (highlights become stale).

### 3.6 Command Integration

**FR-CS-25:** A `/find <pattern>` command MAY be provided as an alternative entry point.

**FR-CS-26:** A `/nohlsearch` or `/noh` command MAY be provided to clear highlights without leaving normal mode (vim compatibility).

---

## 4. Non-Functional Requirements

### 4.1 Performance

| Requirement | Specification |
| --- | --- |
| NFR-CS-PERF-01 | Incremental search MUST complete in <50ms for conversations up to 1000 messages |
| NFR-CS-PERF-02 | Match highlighting MUST NOT degrade render frame rate below 30fps |
| NFR-CS-PERF-03 | Search state SHOULD be computed lazily or cached between frames |

### 4.2 Accessibility

| Requirement | Specification |
| --- | --- |
| NFR-CS-A11Y-01 | Match highlight colors MUST have sufficient contrast ratio (WCAG AA) |
| NFR-CS-A11Y-02 | Status messages MUST be screen-reader friendly (no unicode-only indicators) |

---

## 5. Data Structures

### 5.1 SearchState

```rust
/// Active search state, held in App when search is active.
pub struct SearchState {
    /// The search pattern (user input).
    pattern: String,
    /// Cursor position within pattern.
    cursor: usize,
    /// Computed matches, invalidated on pattern change or new messages.
    matches: Vec<SearchMatch>,
    /// Index into matches for current active match (for n/N navigation).
    active_index: Option<usize>,
}

pub struct SearchMatch {
    /// Which display item contains this match.
    item_index: usize,
    /// Byte offset within the message content where match starts.
    byte_offset: usize,
    /// Length of match in bytes.
    byte_len: usize,
}
```

### 5.2 InputMode Extension

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Insert,
    Command,
    ModelSelect,
    Search,  // NEW
}
```

### 5.3 InputState Extension

```rust
pub enum InputState {
    Normal(DraftInput),
    Insert(DraftInput),
    Command { draft: DraftInput, command: DraftInput },
    ModelSelect { draft: DraftInput, selected: usize },
    Search { draft: DraftInput, state: SearchState },  // NEW
}
```

---

## 6. UI Layout

### 6.1 Full-Screen Search Mode

```
┌─────────────────────────────────────────────────────┐
│ ○ You                                               │
│                                                     │
│   What is the [capital] of France?                  │  ← "capital" highlighted
│                                                     │
│ ◆ Claude                                            │
│                                                     │
│   The [capital] of France is Paris. Paris has been  │  ← active match
│   the [capital] since...                            │  ← another match
│                                                     │
├─────────────────────────────────────────────────────┤
│ /capital█                              [2/3]        │  ← search prompt
├─────────────────────────────────────────────────────┤
│ claude-opus-4-6 | 1.2k/1M   | Search     │  ← status bar
└─────────────────────────────────────────────────────┘
```

### 6.2 Theme Colors (suggested)

```rust
// In tui/src/theme.rs
pub const SEARCH_MATCH: Color = Color::Rgb(60, 60, 0);      // Dark yellow bg
pub const SEARCH_ACTIVE: Color = Color::Rgb(100, 100, 0);   // Brighter yellow bg
pub const SEARCH_MATCH_FG: Color = Color::Rgb(255, 255, 200); // Light text on match
```

---

## 7. State Transitions

```
                    ┌─────────────┐
                    │   Normal    │
                    └──────┬──────┘
                           │ '/'
                           ▼
                    ┌─────────────┐
         Esc        │   Search    │        Enter
       ┌────────────┤  (typing)   ├────────────┐
       │            └─────────────┘            │
       ▼                                       ▼
┌─────────────┐                         ┌─────────────┐
│   Normal    │◄────────── Esc ─────────│   Normal    │
│ (no search) │                         │ (searching) │
└─────────────┘                         └──────┬──────┘
                                               │ n/N
                                               ▼
                                        ┌─────────────┐
                                        │ Jump to     │
                                        │ next/prev   │
                                        └─────────────┘
```

---

## 8. Implementation Checklist

### 8.1 Engine Changes (`engine/src/lib.rs`)

- [ ] Add `InputMode::Search` variant
* [ ] Add `InputState::Search` variant with `SearchState`
* [ ] Add `SearchState` struct with pattern, cursor, matches, active_index
* [ ] Add `SearchMatch` struct
* [ ] Implement `enter_search_mode()`, `exit_search_mode()`
* [ ] Implement `search_next()`, `search_prev()`
* [ ] Implement `search_pattern()` getter for TUI
* [ ] Implement `search_matches()` getter for TUI
* [ ] Implement `search_active_index()` getter for TUI
* [ ] Add `SearchToken` proof type for search mode operations
* [ ] Invalidate search on new message send

### 8.2 TUI Changes (`tui/src/lib.rs`)

- [ ] Import `SearchState` types
* [ ] Modify `draw_messages()` to apply match highlighting
* [ ] Add `draw_search_prompt()` function
* [ ] Call `draw_search_prompt()` when `InputMode::Search`
* [ ] Add search colors to `theme.rs`

### 8.3 Input Changes (`tui/src/input.rs`)

- [ ] Change `/` in Normal mode to call `enter_search_mode()` (full-screen only)
* [ ] Add `handle_search_mode()` function
* [ ] Handle `n`/`N` in Normal mode when search active
* [ ] Handle `Esc` in Normal mode to clear search

---

## 9. Verification Requirements

### 9.1 Unit Tests

| Test ID | Description |
| --- | --- |
| T-CS-MATCH-01 | Pattern finds all occurrences in message content |
| T-CS-MATCH-02 | Case-insensitive matching works correctly |
| T-CS-MATCH-03 | Empty pattern returns no matches |
| T-CS-MATCH-04 | Pattern with special chars is treated as literal |
| T-CS-NAV-01 | next() advances active_index with wrap |
| T-CS-NAV-02 | prev() decrements active_index with wrap |
| T-CS-STATE-01 | enter_search_mode() transitions correctly |
| T-CS-STATE-02 | exit_search_mode() clears state |

### 9.2 Integration Tests

| Test ID | Description |
| --- | --- |
| IT-CS-E2E-01 | Full search flow: enter, type, navigate, exit |
| IT-CS-SCROLL-01 | Jumping to match scrolls view appropriately |
| IT-CS-INLINE-01 | Search blocked in inline mode with message |

---

## 10. Future Considerations

### 10.1 v2 Enhancements

* Regex search mode (`/\v` prefix or toggle)
* Case-sensitive toggle (`\C` suffix)
* Search history (up/down arrow in search prompt)
* Cross-session search (search SQLite history)

### 10.2 Inline Mode Support

If inline mode search is desired, the recommended approach is:

1. On `/` press, temporarily switch to alternate screen
2. Display full conversation with search UI
3. On exit, return to inline mode

This requires `TerminalSession` coordination but preserves full search UX.

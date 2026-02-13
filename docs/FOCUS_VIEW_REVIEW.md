# Focus View Specification Review

## Implementation-Ready Assessment

**Verdict: Not ready.** The spec is a strong design vision but has ~6 blocking gaps where two engineers would diverge into incompatible implementations. The single biggest blocker is **type duplication** — the spec redefines `PlanStep`, `StepStatus`, and `ContentBlock` from scratch instead of mapping from the existing domain types that already exist in `types/src/plan.rs`.

---

## Discoveries and Proposals

### 1. Type Duplication — Parallel Universe Problem (BLOCKING)

**Section:** §2.1 New Types

**Observation:** The spec defines `PlanStep { text, status: StepStatus }` and `StepStatus { Done, Active, Pending, Failed }` — but these already exist in `types/src/plan.rs` with richer variants (`StepStatus::Complete(String)`, `StepStatus::Skipped(String)`, plus the full DAG dependency model). The spec's types are a flattened shadow of the real types.

Two implementers would diverge on: Do we use the spec's types (and maintain a mapping layer)? Or use the existing types directly? If mapping, who owns the conversion? Where does elapsed time live?

**Proposal:** Delete the spec's `PlanStep`/`StepStatus` entirely. `FocusState::Executing` should reference the existing plan:

```rust
Executing {
    /// Elapsed time for the currently-active step.
    step_elapsed: Duration,
},
```

The plan carousel reads `app.plan_state` directly — it already has phases, steps, statuses, and the active step. The TUI is a *view* of the engine's state, not a parallel model.

---

### 2. `FocusState` Ownership — Engine or TUI? (BLOCKING)

**Section:** §2.2 Integration with App

**Observation:** `FocusState` contains rendering-only concerns (`elapsed_ms`, `active_index` for horizontal carousel) but the spec puts it in `App` (engine). The engine currently separates this cleanly: `ViewState` lives in `engine/src/ui/view_state.rs` for rendering concerns. `App` fields are orchestration state.

Putting `FocusState` in `App` means the engine crate grows with pure-rendering state (animation indices, elapsed timers). This breaks the existing separation.

**Proposal:** `ViewMode` goes in `App` (it's a user preference that affects behavior). `FocusState` goes in `ViewState`, or better yet, lives in `tui/src/focus/mod.rs` as local rendering state. The TUI already computes everything it needs from `App` — it derives the focus sub-state from `app.plan_state`, `app.is_streaming()`, etc.

```rust
// engine/src/ui/view_state.rs
pub struct ViewState {
    pub view_mode: ViewMode,  // user preference
    // ... existing fields ...
}

// tui/src/focus/mod.rs — computed per-frame, not stored
enum FocusSubMode {
    Idle,
    Executing,
    Reviewing,
}

fn focus_sub_mode(app: &App) -> FocusSubMode {
    if app.plan_state.is_active() && app.is_streaming() {
        FocusSubMode::Executing
    } else if !app.display_items().is_empty() && !app.is_streaming() {
        FocusSubMode::Reviewing
    } else {
        FocusSubMode::Idle
    }
}
```

---

### 3. `ContentBlock` Extraction — Undefined Algorithm (BLOCKING)

**Section:** §5, §13.2

**Observation:** The spec says "Model already emits discrete units. Thinking block = one item. Response = one item. Tool result = one item. Carousel iterates `DisplayItem`s directly." But `DisplayItem` is either `History(MessageId)` or `Local(Message)`, and a single `Message` can contain multiple content segments (thinking + response text + tool calls). The spec doesn't define how to decompose messages into carousel blocks.

**Implementer's question:** If an assistant message has thinking followed by a response, is that 1 block or 2? What about a message with 3 tool calls? What about interleaved tool calls and text?

**Proposal:** Add an explicit extraction algorithm:

```
ContentBlock extraction:
1. For each DisplayItem in display_items():
   a. If it's an assistant message with thinking content: emit ContentBlock::Thought
   b. For each text segment in the message: emit ContentBlock::Response
   c. Skip tool_use entries (they're paired with results)
2. For each tool result message: emit ContentBlock::ToolResult
3. Skip system/user messages entirely (they're input, not output)
```

---

### 4. `Tab` Key Collision (BLOCKING)

**Section:** §6.3

**Observation:** `Tab` is proposed for toggling Focus/Classic. But `Tab` is almost certainly already in use — it's a standard completion/navigation key. The spec doesn't audit existing keybindings.

Checking the TUI input handler, `Tab` is used for `InputMode` transitions and file panel toggling. This is a direct collision.

**Proposal:** Audit current `Tab` usage. Consider `F2` or a `/focus` command instead. Or make `Tab` context-dependent (only toggles view in Normal mode, preserves existing behavior in Insert/Command modes). This must be explicit.

---

### 5. Opacity/Blur — Not Possible in Terminal (BLOCKING)

**Section:** §4.2, §5.2

**Observation:** The spec specifies "opacity 0.15–0.35", "blur 3px", "blur 5px", "opacity transitions 300ms linear". Terminal emulators don't support opacity or blur. ratatui renders to a cell grid with 256 or RGB colors — there's no alpha channel, no gaussian blur.

The spec seems to know this ("No literal boxes — showing concept") for §5.1 but then treats opacity as a real rendering parameter in §4.2 and §5.2.

**Proposal:** Replace opacity/blur language with concrete terminal-achievable alternatives:
- "opacity 0.15–0.35" → use `palette.text_disabled` or dim modifier
- "opacity 0.2–0.5" → use `palette.text_muted`
- "blur 3px/5px" → steps don't blur; they just get progressively dimmer colors
- Specify exact `Color` values or palette references for each distance-from-active level

```rust
fn step_style(distance_from_active: usize, status: &StepStatus, palette: &Palette) -> Style {
    match distance_from_active {
        0 => Style::default().fg(palette.text_primary).add_modifier(Modifier::BOLD),
        1 => Style::default().fg(palette.text_secondary),
        2 => Style::default().fg(palette.text_muted),
        _ => Style::default().fg(palette.text_disabled),
    }
}
```

---

### 6. Transition from Executing → Reviewing — Undefined Trigger (IMPORTANT)

**Section:** §2.3

**Observation:** The state diagram says "plan completes → Reviewing". But what if there's no plan? The LLM can respond without a plan (most interactions). What happens then? The spec's Idle→Executing transition requires "user sends message" but Executing requires a plan.

The common case — user sends a question, LLM responds with text, no plan involved — has no defined path through Focus view.

**Proposal:** Add a planless path:

```
Idle ──user sends message──► Streaming (no plan: show spinner + thinking)
                              │
                              │ response complete
                              ▼
                           Reviewing
```

`Executing` is only entered when `PlanState::Active`. Otherwise, a simpler streaming state shows the LLM's real-time text output.

---

### 7. Horizontal Carousel Auto-Advance — Race with User Input (IMPORTANT)

**Section:** §5.5

**Observation:** "1.5s pause after completion → Auto-advance to next block if streaming continues. User can interrupt auto-advance by pressing h." This creates a race: if the user is reading and presses `h` at the exact moment auto-advance fires, do we go back one (user wins) or forward then back (auto-advance wins then user corrects)?

**Proposal:** Auto-advance should be cancellable: any navigation input (`h`, `l`, `j`, `k`) within the current block cancels auto-advance for the remainder of that streaming session. Once the user takes manual control, stay manual.

```rust
struct ReviewingState {
    active_index: usize,
    auto_advance: bool,  // starts true, set false on any nav input
}
```

---

### 8. `elapsed_ms: u64` vs `Duration` (IMPORTANT)

**Section:** §2.1

**Observation:** The spec uses `u64` milliseconds. The codebase universally uses `std::time::Duration` and `std::time::Instant`. Mixing representations invites conversion bugs.

**Proposal:** Use `Duration` (or derive elapsed from `Instant` stored at step activation). Don't store elapsed at all — compute it: `Instant::now() - step_started_at`.

---

### 9. "Ready" Text — Branding Debt (REFINEMENT)

**Section:** §3

**Observation:** Hardcoded "Ready" centered on screen. Minor point, but if the app is ever branded or themed differently, this becomes a magic string.

**Proposal:** Fine as-is for v1. Just use a constant: `const IDLE_TEXT: &str = "Ready";`

---

### 10. First-Run Hints — Persistence Mechanism Unspecified (IMPORTANT)

**Section:** §8.2

**Observation:** "Show once, then never again (persist in config)." How? What config key? What happens if the config file is deleted — do hints reappear? The spec says "persist in config" but the existing `ForgeConfig` doesn't have a hints/onboarding section.

**Proposal:** Add to config schema:

```toml
[ui]
focus_hints_shown = true  # set after first render
```

Or use a simpler local state file (`~/.forge/state.json`) to avoid polluting the user's config with generated values.

---

### 11. File Structure — Extracting `classic.rs` (IMPORTANT)

**Section:** §11

**Observation:** The spec proposes extracting `draw_messages()` into `tui/src/classic.rs`. This function is ~250 lines but deeply entangled with `draw_input()`, `draw_status_bar()`, and the scroll/cache infrastructure in `lib.rs`. The extraction isn't trivial — the spec treats it as a given.

**Proposal:** Phase 1 should keep `draw_messages()` in `lib.rs` and add a `focus/` module that `draw()` dispatches to based on `view_mode`. Extract to `classic.rs` only if the two paths genuinely diverge enough to warrant it. Don't refactor for the sake of a directory listing.

---

### 12. Step Failure Recovery — Who Drives? (IMPORTANT)

**Section:** §4.6

**Observation:** "LLM reorients, proposes updated plan. User approval prompt appears." This implies the LLM autonomously detects step failure and generates a new plan. But the current plan architecture has the *engine* managing step transitions and the *tool harness* enforcing plan constraints. Who initiates the recovery? Does the engine send a recovery prompt to the LLM? Does the LLM's next response naturally include a plan edit?

**Proposal:** On step failure, the engine injects a system-level recovery prompt: `"Step N failed: {reason}. Propose an updated plan or skip remaining steps."` The LLM's response is processed through normal plan tool dispatch. The carousel pauses until `PlanState` transitions again.

---

### 13. Missing: What Happens to Notifications? (REFINEMENT)

**Section:** Not covered

**Observation:** The existing plan implementation (phase 6b+6c) pushes notifications on step/phase transitions. Focus view doesn't mention notifications at all. Are they suppressed in Focus mode (since the carousel already shows progress visually)? Or do they still appear?

**Proposal:** Suppress push notifications in Focus view — the carousel IS the notification. Only show them in Classic view.

---

## Architecture Considerations

1. **Missing state diagram for planless interactions** — The spec only covers the plan-guided flow. Most LLM interactions don't involve plans. Focus view needs a streaming/thinking sub-state.

2. **No animation framework** — The spec proposes 300ms ease-out cubic animations, but there's no frame-driven animation loop. The existing `ModalEffect`/`PanelEffect` in `effects.rs` provide this pattern — the spec should reference it explicitly and propose `FocusTransitionEffect` following the same `advance(elapsed) → progress() → is_finished()` pattern.

3. **No accessibility section** — `reduced_motion` is mentioned once (§10) but screen reader behavior, minimum contrast ratios for the dimmed steps, and keyboard-only operation aren't addressed.

---

## Prioritized Recommendations

### Blocking (must fix before implementation)

| # | Issue | Fix |
|---|-------|-----|
| 1 | Type duplication vs existing `types/src/plan.rs` | Delete spec types, reference existing plan model |
| 2 | `FocusState` ownership (engine vs TUI) | Move rendering state to `ViewState` or TUI-local |
| 3 | `ContentBlock` extraction undefined | Specify algorithm for decomposing messages into blocks |
| 4 | `Tab` key collision | Audit keybindings, pick non-conflicting key |
| 5 | Opacity/blur impossible in terminal | Replace with concrete color/modifier mappings |

### Important (should fix early)

| # | Issue | Fix |
|---|-------|-----|
| 6 | No planless streaming path | Add Streaming sub-state for non-plan interactions |
| 7 | Auto-advance race condition | Cancel auto-advance on any user navigation |
| 8 | `u64` ms vs `Duration` | Use `Instant`/`Duration` |
| 10 | Hint persistence unspecified | Define config key or state file |
| 11 | `classic.rs` extraction assumed trivial | Defer extraction, dispatch in `lib.rs` |
| 12 | Step failure recovery ownership | Engine injects recovery prompt |

### Refinement (can address iteratively)

| # | Issue | Fix |
|---|-------|-----|
| 9 | Hardcoded "Ready" | Use constant |
| 13 | Notification behavior in Focus | Suppress, carousel is the notification |

---

## Summary Table

| Section | Issue | Severity | Proposal |
|---------|-------|----------|----------|
| §2.1 | Type duplication with `types/src/plan.rs` | Blocking | Delete spec types, use existing |
| §2.2 | `FocusState` in engine breaks separation | Blocking | Move to `ViewState` or TUI |
| §5, §13.2 | `ContentBlock` extraction undefined | Blocking | Specify decomposition algorithm |
| §6.3 | `Tab` collides with existing bindings | Blocking | Audit and reassign |
| §4.2, §5.2 | Opacity/blur not possible in terminal | Blocking | Map to palette colors + modifiers |
| §2.3 | No path for planless interactions | Important | Add Streaming sub-state |
| §5.5 | Auto-advance race condition | Important | Cancel on user input |
| §2.1 | `elapsed_ms: u64` vs `Duration` | Important | Use `Instant` |
| §8.2 | Hint persistence unspecified | Important | Define storage mechanism |
| §11 | Classic extraction assumed easy | Important | Defer, dispatch in `lib.rs` |
| §4.6 | Recovery ownership unclear | Important | Engine injects prompt |
| §3 | Hardcoded "Ready" | Refinement | Use constant |
| — | Notifications in Focus mode | Refinement | Suppress in Focus |

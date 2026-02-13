# Focus View Specification

## Design Philosophy

> "The terminal was always the UI. We just forgot."

Strip chrome. Kill labels. Remove anything that describes what you're looking at instead of showing it. The OS window is the frame. The content is the interface.

**Core principles:**
- Invalid UI states should be unbuildable
- Not building something is a feature
- One viewport, one focus
- Mode as mutual exclusion, not layer

---

## 1. View Modes

Two rendering modes for the main viewport. Mutually exclusive.

### 1.1 Focus View (Default)

Minimal, carousel-based. One thing at a time.

**Sub-modes** (also mutually exclusive):
- **Idle** — Empty viewport, "Ready" centered, input at bottom
- **Executing** — Vertical plan carousel, active step centered
- **Reviewing** — Horizontal content carousel, thoughts/responses

### 1.2 Classic View

Traditional scrollable message list. Power user escape hatch. Current `draw_messages()` implementation.

---

## 2. State Model

### 2.1 New Types

```rust
/// Top-level view mode selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Focus,
    Classic,
}

/// Focus view sub-state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusState {
    /// No active operation. Shows "Ready".
    Idle,
    
    /// Executing a plan. Vertical carousel.
    Executing {
        steps: Vec<PlanStep>,
        active_index: usize,
        elapsed_ms: u64,
    },
    
    /// Reviewing completed content. Horizontal carousel.
    Reviewing {
        blocks: Vec<ContentBlock>,
        active_index: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Done,
    Active,
    Pending,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentBlock {
    pub kind: ContentKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Thought,
    Response,
    ToolResult,
}
```

### 2.2 Integration with App

```rust
// In engine/src/lib.rs App struct
pub struct App {
    // ... existing fields ...
    
    /// Current view mode (Focus vs Classic).
    pub view_mode: ViewMode,
    
    /// Focus view state (only relevant when view_mode == Focus).
    pub focus_state: FocusState,
}
```

### 2.3 State Transitions

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ┌─────────┐   user sends    ┌───────────┐                 │
│  │  Idle   │ ───message────▶ │ Executing │                 │
│  └─────────┘                 └───────────┘                 │
│       ▲                            │                        │
│       │                            │ plan completes         │
│       │      user sends            ▼                        │
│       │      new message     ┌───────────┐                 │
│       └──────────────────────│ Reviewing │                 │
│                              └───────────┘                 │
│                                                             │
│  Tab toggles ViewMode between Focus and Classic at any time │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Focus View: Idle State

### 3.1 Layout

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│                                                             │
│                                                             │
│                                                             │
│                           Ready                             │
│                                                             │
│                                                             │
│                                                             │
│                                                             │
│  > _                                                        │
└─────────────────────────────────────────────────────────────┘
```

No boxes rendered. Just viewport bounds shown for spec clarity.

### 3.2 Rendering

```rust
fn draw_focus_idle(frame: &mut Frame, app: &App, area: Rect, palette: &Palette) {
    // "Ready" - centered, dim
    let ready = Paragraph::new("Ready")
        .style(Style::default().fg(palette.text_muted))
        .alignment(Alignment::Center);
    
    let center_y = area.height / 2;
    let ready_area = Rect {
        x: area.x,
        y: area.y + center_y,
        width: area.width,
        height: 1,
    };
    
    frame.render_widget(ready, ready_area);
}
```

### 3.3 Behavior

- "Ready" vanishes when user starts typing
- On message send: transition to `Executing` (if plan generated) or stay showing spinner
- No chrome, no welcome screen, no tips

---

## 4. Focus View: Executing State (Vertical Carousel)

### 4.1 Layout

```
                                                             
                                                             
         ✓   Parse error logs to identify failure pattern    
                                                             
           ✓   Locate retry logic in src/client/fetch.rs     
                                                             
         ⠸   Add exponential backoff with jitter             
                              34s                            
                                                             
            ○   Update tests in tests/retry_test.rs          
                                                             
            ○   Run cargo test --lib to verify               
                                                             
                                                             
```

### 4.2 Visual Hierarchy

| Element | Style | Notes |
|---------|-------|-------|
| Completed step | `text_disabled`, opacity 0.15–0.35 | Fades with distance from active |
| Active step | `text_primary`, font-weight bold | Full contrast |
| Active spinner | `accent` (cyan) | Braille animation |
| Active timer | `accent` (cyan), smaller | Centered below step text |
| Pending step | `text_muted`, opacity 0.2–0.5 | Fades with distance from active |
| Pending marker | `text_disabled` | ○ glyph |
| Failed step | `error` (red) | ✗ glyph, text dimmed |

### 4.3 Carousel Math

The active step is always vertically centered. The entire list translates to maintain this anchor.

```rust
const STEP_HEIGHT: u16 = 3; // lines per step (text + timer + gap)

fn calculate_plan_offset(active_index: usize, total_steps: usize, viewport_height: u16) -> i16 {
    let center = viewport_height as i16 / 2;
    let active_position = (active_index as i16) * (STEP_HEIGHT as i16);
    center - active_position - (STEP_HEIGHT as i16 / 2)
}
```

### 4.4 Step Completion Animation

1. **Spinner freezes** — hold final frame for 100ms
2. **Spinner → ✓** — instant swap
3. **Stack translates up** — 300ms ease-out
4. **Simultaneously:**
   - Completed step dims (opacity transition)
   - Next step brightens (opacity + scale)
   - New spinner appears
   - Timer resets to `0s`

```rust
/// Easing function for carousel transitions.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
```

### 4.5 Timer Display

- Only shown on active step
- Centered below step text
- Same cyan as spinner
- Detects stuck operations (if >60s, consider warning state)

### 4.6 Step Failure

When a step fails:

1. Spinner → `✗` (red, `palette.error`)
2. Step text remains visible, dimmed
3. Carousel pauses
4. LLM reorients, proposes updated plan
5. User approval prompt appears
6. On approval: new plan replaces old, carousel resets to first pending step

---

## 5. Focus View: Reviewing State (Horizontal Carousel)

### 5.1 Layout

```
                                                             
                                                             
     ┄┄┄             ┌─────────────────────┐           ┄┄┄   
   (dim)             │                     │         (dim)   
                     │  The user wants     │                 
   Thought 1         │  exponential...     │      Response   
                     │                     │                 
                     │                     │                 
     ┄┄┄             └─────────────────────┘           ┄┄┄   
                                                             
                                                             
```

Adjacent blocks visible but blurred/dimmed. No literal boxes — showing concept.

### 5.2 Visual Hierarchy

| Element | Style | Notes |
|---------|-------|-------|
| Active block | Full opacity, no blur | Centered |
| Adjacent blocks | opacity 0.25, blur 3px | ±1 from active |
| Distant blocks | opacity 0.15, blur 5px | ±2+ from active |

### 5.3 Block Types

```rust
impl ContentKind {
    fn label_color(&self, palette: &Palette) -> Color {
        match self {
            ContentKind::Thought => palette.text_muted,
            ContentKind::Response => palette.accent,
            ContentKind::ToolResult => palette.primary,
        }
    }
}
```

**No labels rendered** — block type inferred from content styling and position.

### 5.4 Navigation

| Key | Action |
|-----|--------|
| `h` / `←` | Previous block |
| `l` / `→` | Next block |
| `j` / `↓` | Scroll down within block |
| `k` / `↑` | Scroll up within block |
| `Tab` | Switch to Classic view |

### 5.5 Streaming Behavior

When a new block streams in:
1. Current block slides left + dims
2. New block enters from right, settles center
3. 1.5s pause after completion (reading time)
4. Auto-advance to next block if streaming continues

User can interrupt auto-advance by pressing `h` to go back.

---

## 6. Classic View

### 6.1 Purpose

- Full conversation history visible
- Copy/paste friendly
- Debugging tool output
- Power user preference

### 6.2 Implementation

Existing `draw_messages()` in `tui/src/lib.rs`. No changes needed.

### 6.3 Toggle

`Tab` key switches between Focus and Classic at any time.

```rust
fn handle_tab(app: &mut App) {
    app.view_mode = match app.view_mode {
        ViewMode::Focus => ViewMode::Classic,
        ViewMode::Classic => ViewMode::Focus,
    };
}
```

---

## 7. Input Area

### 7.1 Focus View Input

Minimal prompt at viewport bottom:

```
  > hello world_
```

- Two-space indent
- `>` prompt character
- Space
- Input text
- Block cursor (`_`)

### 7.2 Classic View Input

Current input area implementation. Status bar with mode, model, context %.

### 7.3 Shared Behavior

Input handling unchanged. Both views use same `InputMode` state machine.

---

## 8. Status Bar

### 8.1 Focus View Status Bar

Minimal. Bottom edge of viewport.

```
claude-opus-4-6                                          2/5 complete
```

Or in reviewing mode:

```
claude-opus-4-6                                               2/3
```

| Left | Right |
|------|-------|
| Model name | Progress indicator |

### 8.2 Key Hints (First Run Only)

Show once, then never again (persist in config):

```
← h / l →                                              Tab: classic view
```

After first use, these disappear permanently.

---

## 9. Color Constants

All colors from existing Kanagawa palette in `tui/src/theme.rs`:

```rust
// Focus view specific usage
impl Palette {
    pub fn focus_ready(&self) -> Color { self.text_muted }
    pub fn focus_active_text(&self) -> Color { self.text_primary }
    pub fn focus_active_accent(&self) -> Color { self.accent }  // cyan
    pub fn focus_done(&self) -> Color { self.text_disabled }
    pub fn focus_pending(&self) -> Color { self.text_muted }
}
```

No new colors. Reuse existing semantic mappings.

---

## 10. Animation Timing

| Animation | Duration | Easing |
|-----------|----------|--------|
| Step completion pause | 100ms | — |
| Vertical carousel slide | 300ms | ease-out cubic |
| Horizontal block slide | 350ms | ease-out cubic |
| Opacity transitions | 300ms | linear |
| Reading pause | 1500ms | — |

All animations respect `UiOptions.reduced_motion`. When enabled:
- No transitions, instant state changes
- Static spinner (first frame only)

---

## 11. File Structure

```
tui/src/
├── lib.rs                 # Main draw(), dispatches to focus or classic
├── focus/
│   ├── mod.rs            # Focus view entry point
│   ├── idle.rs           # Idle state rendering
│   ├── executing.rs      # Vertical plan carousel
│   ├── reviewing.rs      # Horizontal content carousel
│   └── transitions.rs    # Animation math and easing
├── classic.rs            # Extracted from current lib.rs draw_messages
└── ... (existing files)
```

---

## 12. Implementation Phases

### Phase 1: State Foundation
- Add `ViewMode`, `FocusState` to engine
- Wire up `Tab` toggle
- Focus view renders "Ready" in idle, falls back to classic for everything else

### Phase 2: Executing Carousel
- Vertical plan carousel
- Step status tracking
- Timer display
- Completion animations

### Phase 3: Reviewing Carousel  
- Horizontal content carousel
- Block extraction from message history
- Navigation (h/l)
- Streaming integration

### Phase 4: Polish
- Transition animations
- Reading pause timing
- First-run hints
- Edge cases (empty plans, single blocks, very long content)

---

## 13. Resolved Design Decisions

1. **Plan source** — Plan tool (in development) emits structured steps. Focus view consumes via simple mapping:
   ```rust
   impl From<PlanToolOutput> for Vec<PlanStep> { ... }
   ```

2. **Block boundaries** — Model already emits discrete units. Thinking block = one item. Response = one item. Tool result = one item. Carousel iterates `DisplayItem`s directly.

3. **Long content** — Scrollable blocks. `h`/`l` navigates between blocks. `j`/`k` scrolls within active block. Same vim muscle memory.

4. **Multi-turn** — Reviewing carousel shows entire conversation. User can navigate back through all thoughts/responses from session start.

5. **Error states** — Failed step shows red `✗`. LLM reorients and proposes updated plan. User approves before execution continues. No automatic retry.

---

## 14. Non-Goals

- Settings panel for view preferences
- Customizable carousel direction  
- Theme picker
- Configurable animation speeds
- Toggle for individual UI elements

We decide. Ship one way. Own it.

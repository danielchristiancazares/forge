# forge-tui

Terminal user interface rendering and input handling for Forge, built on [ratatui](https://ratatui.rs) and [crossterm](https://github.com/crossterm-rs/crossterm).

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-27 | Header, Intro, LLM-TOC, Table of Contents |
| 28-39 | Purpose and Responsibility |
| 40-55 | Module Overview |
| 56-105 | Full-Screen vs Inline Rendering |
| 106-616 | Key Modules: lib.rs, ui_inline.rs, input.rs, theme.rs, markdown.rs, effects.rs, shared.rs, tool_display.rs, tool_result_Distillate.rs, diff_render.rs |
| 617-650 | Public API |
| 651-692 | Developer Notes |

## Table of Contents

1. [Purpose and Responsibility](#purpose-and-responsibility)
2. [Module Overview](#module-overview)
3. [Full-Screen vs Inline Rendering](#full-screen-vs-inline-rendering)
4. [Key Modules](#key-modules)
5. [Public API](#public-api)
6. [Developer Notes](#developer-notes)

---

## Purpose and Responsibility

The `forge-tui` crate is responsible for:

- **Rendering**: Drawing the UI to the terminal in both full-screen and inline modes
- **Input handling**: Processing keyboard events and dispatching to mode-specific handlers
- **Theming**: Providing consistent colors, styles, and glyphs across the interface
- **Markdown rendering**: Converting markdown content to styled terminal output
- **Modal effects**: Animating overlay transitions (pop-scale, slide-up, shake)

This crate is purely presentational. It renders state from `forge-engine` and forwards user input back to it. It contains no business logic, API calls, or persistence.

## Module Overview

```
tui/src/
├── lib.rs              # Full-screen rendering, message display, overlays
├── ui_inline.rs        # Inline terminal rendering with scrollback integration
├── input.rs            # Keyboard event handling and mode dispatch
├── theme.rs            # Color palette, styles, and glyphs
├── markdown.rs         # Markdown to ratatui conversion with caching
├── effects.rs          # Modal animation transforms
├── shared.rs           # Rendering helpers shared between modes
├── tool_display.rs     # Compact tool call formatting
├── tool_result_Distillate.rs  # Tool result summarization logic
└── diff_render.rs      # Diff-aware coloring for tool output
```

## Full-Screen vs Inline Rendering

### Full-Screen Mode (`lib.rs`)

Uses crossterm's alternate screen for complete terminal control:

```
┌─────────────────────────────────────────────────────────┐
│                                                         │
│                   Messages Area                         │
│              (scrollable, flex height)                  │
│                                                         │
├─────────────────────────────────────────────────────────┤
│ [MODE] │ prompt text                      key hints     │
│ model  │                                  context: 45%  │
└─────────────────────────────────────────────────────────┘
```

Features:
- Scrollable message history with keyboard navigation
- Command palette overlay for slash commands
- Model selector overlay with animation effects
- Tool approval and recovery prompts
- Welcome screen when conversation is empty

### Inline Mode (`ui_inline.rs`)

Runs within normal terminal flow, preserving scrollback history:

```rust
pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;
```

Features:
- Fixed-height viewport at terminal bottom
- Completed messages written to terminal scrollback via `terminal.insert_before()`
- Simplified rendering without markdown parsing
- Same input handling as full-screen mode

### Mode Differences

| Aspect | Full-Screen | Inline |
|--------|-------------|--------|
| Terminal | Alternate screen | Normal flow |
| Message display | Scrollable widget | Terminal scrollback |
| Markdown | Full parsing | Plain text |
| Overlays | Centered popups | Transformed input area |
| Streaming | In-place update | Input area only |

## Key Modules

### lib.rs - Full-Screen Rendering

Entry point: `draw(frame: &mut Frame, app: &mut App)`

**Layout Structure:**
1. Clear frame with background color
2. Split into messages area and input area
3. Render messages with scrolling
4. Render input with mode-specific styling
5. Overlay command palette, model selector, or approval prompts as needed

**Message Caching:**
Static message content is cached in a thread-local `MessageLinesCache` to avoid rebuilding every frame. Dynamic content (streaming, tool status) is appended separately.

```rust
thread_local! {
    static MESSAGE_CACHE: RefCell<MessageLinesCache> = RefCell::new(MessageLinesCache::default());
}
```

Cache is invalidated when:
- Display version changes (new messages)
- Terminal width changes
- UI options change (ascii_only, high_contrast, reduced_motion)

**Scrollbar Rendering:**
Only rendered when content exceeds viewport (`max_scroll > 0`). Uses `max_scroll` as content length for correct thumb positioning.

### ui_inline.rs - Inline Rendering

Entry point: `draw(frame: &mut Frame, app: &mut App)`

**InlineOutput State:**
Tracks what has been written to terminal scrollback to avoid duplicate output:

```rust
pub struct InlineOutput {
    next_display_index: usize,          // Messages already printed
    has_output: bool,                   // Any output written
    last_tool_output_len: usize,        // Tool output line count
    last_tool_status_signature: Option<String>,   // Detect changes
    last_approval_signature: Option<String>,
    last_recovery_active: bool,
}
```

**Flush Method:**
`flush()` writes new messages above the viewport using `terminal.insert_before()`, which scrolls existing content up. Signature fields prevent duplicate output when state hasn't changed.

### input.rs - Keyboard Input Handling

Entry point: `handle_events(app: &mut App, input: &mut InputPump) -> Result<bool>`

**Event Processing:**
1. `InputPump` runs a blocking reader loop (25ms poll) and pushes events into a bounded channel
2. `handle_events` drains the queue (non-blocking) and ignores `KeyEventKind::Release`
3. Handle global Ctrl+C for cancellation
4. Dispatch to mode-specific handler

**Mode Handlers:**

| Mode | Handler | Key Behaviors |
|------|---------|---------------|
| Normal | `handle_normal_mode` | Navigation, mode entry, quit |
| Insert | `handle_insert_mode` | Text editing, message send |
| Command | `handle_command_mode` | Command input, execution |
| ModelSelect | `handle_model_select_mode` | Selection, confirmation |
| FileSelect | `handle_file_select_mode` | File filtering and insertion |

**Modal Priority:**
Tool approval and recovery modals take priority over mode-specific handling. When active, they intercept key events regardless of input mode.

**Key Bindings (Normal Mode):**

| Key | Action |
|-----|--------|
| `q` | Quit |
| `i` | Insert mode |
| `a` | Insert at end |
| `o` | Insert with clear |
| `:` / `/` | Command mode |
| `m` | Model selector |
| `f` | Toggle files panel |
| `k` / `Up` | Scroll up |
| `j` / `Down` | Scroll down |
| `g` | Scroll to top |
| `G` / `End` / `Right` | Scroll to bottom |
| `PageUp` | Page up |
| `PageDown` | Page down |
| `Ctrl+U` | Page up (or scroll diff up when files panel expanded) |
| `Ctrl+D` | Page down (or scroll diff down when files panel expanded) |
| `Left` | Scroll up by chunk |
| `Tab` / `Shift+Tab` | Files panel: next/previous file |
| `Enter` / `Esc` | Files panel: collapse expanded diff |
| `s` | Toggle screen mode |

**Key Bindings (Insert Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Normal mode |
| `Enter` | Send message |
| `Ctrl+Enter` / `Shift+Enter` / `Ctrl+J` | Insert newline |
| `Up` / `Down` | Navigate prompt history |
| `Backspace` | Delete backward |
| `Delete` | Delete forward |
| `Left` / `Right` | Move cursor |
| `Ctrl+U` | Clear line |
| `Ctrl+W` | Delete word backward |
| `Home` / `End` | Jump to start/end |
| `@` | Open file selector |

**Key Bindings (Model Select Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `1`-`9` | Direct selection by index |

**Key Bindings (File Select Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Cancel and return to Insert |
| `Enter` | Insert selected file path |
| `Up` / `Down` | Move selection |
| `Backspace` | Delete filter character (or cancel if empty) |
| Typing | Filter file list |

### theme.rs - Color Palette and Styling

**Palette:**
Kanagawa Wave-inspired colors with high-contrast fallback:

```rust
pub struct Palette {
    // Backgrounds (Sumi Ink shades)
    pub bg_dark: Color,         // sumiInk0 - main background
    pub bg_panel: Color,        // sumiInk3 - overlay panels
    pub bg_highlight: Color,    // sumiInk4 - selection highlight
    pub bg_popup: Color,        // sumiInk5 - popup backgrounds
    pub bg_border: Color,       // sumiInk6 - subtle borders

    // Foregrounds (Fuji tones)
    pub text_primary: Color,    // fujiWhite - main text
    pub text_secondary: Color,  // oldWhite - assistant text
    pub text_muted: Color,      // fujiGray - hints, borders
    pub text_disabled: Color,   // katanaGray - disabled elements

    // Brand/Primary
    pub primary: Color,         // oniViolet - brand color
    pub primary_dim: Color,     // springViolet1 - dimmed primary

    // Semantic colors
    pub accent: Color,          // springBlue - tool calls
    pub success: Color,         // springGreen - ok status
    pub warning: Color,         // carpYellow - warnings
    pub error: Color,           // peachRed - errors
    pub peach: Color,           // surimiOrange - inline code

    // Convenience aliases
    pub green: Color,           // = success
    pub yellow: Color,          // = warning
    pub red: Color,             // = error

    // Provider branding
    pub provider_claude: Color, // burnt orange
    pub provider_openai: Color, // white
    pub provider_gemini: Color, // Google blue
}
```

**Glyphs:**
Unicode and ASCII-fallback symbols:

```rust
pub struct Glyphs {
    // Message icons
    pub system: &'static str,           // "●" or "S"
    pub user: &'static str,             // "○" or "U"
    pub assistant: &'static str,        // "◇" or "A"
    pub tool: &'static str,             // "⊙" or "T"
    pub tool_result_ok: &'static str,   // "✓" or "OK"
    pub tool_result_err: &'static str,  // "✗" or "ERR"
    pub tree_connector: &'static str,   // "↪" or "L"

    // Status indicators
    pub status_ready: &'static str,     // "●" or "*"
    pub status_missing: &'static str,   // "○" or "o"
    pub pending: &'static str,          // "•" or "*"
    pub denied: &'static str,           // "⊘" or "X"
    pub paused: &'static str,           // "⏸" or "||"
    pub running: &'static str,          // "▶" or ">"
    pub bullet: &'static str,           // "•" or "*"

    // Navigation
    pub arrow_up: &'static str,         // "↑" or "^"
    pub arrow_down: &'static str,       // "↓" or "v"
    pub selected: &'static str,         // "▸" or ">"

    // Scrollbar
    pub track: &'static str,            // "│" or "|"
    pub thumb: &'static str,            // "█" or "#"

    // File changes
    pub add: &'static str,              // "+"
    pub modified: &'static str,         // "~"

    // Animation
    pub spinner_frames: &'static [&'static str],  // Braille or ASCII
}
```

**Spinner Animation:**
Respects `reduced_motion` option:

```rust
pub fn spinner_frame(tick: usize, options: UiOptions) -> &'static str {
    let frames = glyphs(options).spinner_frames;
    if options.reduced_motion {
        frames[0]  // Static
    } else {
        frames[tick % frames.len()]  // Animated
    }
}
```

**Pre-defined Styles:**

```rust
pub mod styles {
    pub fn user_name(palette: &Palette) -> Style       // Green, bold
    pub fn assistant_name(palette: &Palette) -> Style  // Purple, bold
    pub fn mode_normal(palette: &Palette) -> Style     // Dark on gray
    pub fn mode_insert(palette: &Palette) -> Style     // Dark on green
    pub fn mode_command(palette: &Palette) -> Style    // Dark on yellow
    pub fn mode_model(palette: &Palette) -> Style      // Dark on purple
    pub fn key_hint(palette: &Palette) -> Style        // Muted text
    pub fn key_highlight(palette: &Palette) -> Style   // Peach, bold
}
```

### markdown.rs - Markdown Rendering

Entry point: `render_markdown(content: &str, base_style: Style, palette: &Palette) -> Vec<Line<'static>>`

**Features:**
- Headings (bold, with spacing)
- Bold / italic with proper nesting (counters, not booleans)
- Code blocks with fence markers (```language)
- Inline code (peach colored, bold)
- Ordered and unordered lists with nesting (4-space indent per level)
- Tables with box-drawing borders (unicode width aware)
- Paragraphs with automatic spacing

**Caching:**
Thread-local cache with automatic eviction:

```rust
const CACHE_MAX_ENTRIES: usize = 128;

thread_local! {
    static RENDER_CACHE: RefCell<HashMap<CacheKey, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}
```

Cache key combines content hash, style hash, and palette hash. Eviction removes half the cache when full.

**Streaming Support:**
Handles incomplete code blocks (common during streaming) by rendering partial content with opening fence.

**HTML/XML Handling:**
Renders HTML and XML-like content as plain text rather than silently dropping it. This preserves LLM output that may contain XML-like tags.

**Table Rendering:**

```
┌───────┬───────┬───────┐
│ Col A │ Col B │ Col C │
├───────┼───────┼───────┤
│ 1     │ 2     │ 3     │
└───────┴───────┴───────┘
```

Uses `unicode-width` for proper handling of CJK characters and emoji.

### effects.rs - Modal and Panel Animations

**Public Entry Point:**
- `apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect`

**Internal Entry Point (used by lib.rs):**
- `apply_files_panel_effect(effect: &PanelEffect, base: Rect) -> Rect`

**Modal Effect Types (`ModalEffectKind`):**

| Effect | Description |
|--------|-------------|
| `PopScale` | Scales from 60% to 100% with ease-out-cubic |
| `SlideUp` | Slides up from below viewport |
| `Shake` | Horizontal oscillation with decay (for errors) |

**Panel Effect Types (`PanelEffectKind`):**

| Effect | Description |
|--------|-------------|
| `SlideOutRight` | Panel slides out to the right (hide) |
| `SlideInRight` | Panel slides in from the right (show) |

**Modal Usage Pattern:**

```rust
// In draw function
let elapsed = app.frame_elapsed();
let (modal_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
    effect.advance(elapsed);
    (apply_modal_effect(effect, base_area, frame.area()), effect.is_finished())
} else {
    (base_area, false)
};

if effect_done {
    app.clear_modal_effect();
}

// Render at transformed area
frame.render_widget(content, modal_area);
```

**Panel Animation (internal):**
The files panel uses slide animations when toggling visibility. This is handled internally by `lib.rs` using `apply_files_panel_effect`.

**Easing:**
Uses cubic ease-out for smooth deceleration:

```rust
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}
```

### shared.rs - Shared Rendering Helpers

Common utilities used by both full-screen and inline rendering. All functions are `pub(crate)`.

**Provider Colors:**
```rust
pub(crate) fn provider_color(provider: Provider, palette: &Palette) -> Color
```

**Tool Call Status:**
```rust
pub(crate) enum ToolCallStatusKind {
    Denied,    // User denied the tool
    Error,     // Tool execution failed
    Ok,        // Completed successfully
    Running,   // Currently executing
    Approval,  // Awaiting user approval
    Pending,   // Queued for execution
}

pub(crate) struct ToolCallStatus {
    pub id: String,
    pub name: String,           // Compact display name from tool_display
    pub status: ToolCallStatusKind,
    pub reason: Option<String>, // First line of result/error
}

pub(crate) fn collect_tool_statuses(app: &App, reason_max_len: usize) -> Option<Vec<ToolCallStatus>>
pub(crate) fn tool_status_signature(statuses: Option<&[ToolCallStatus]>) -> Option<String>
```

**Message Headers:**
```rust
pub(crate) fn message_header_parts(msg: &Message, palette: &Palette, glyphs: &Glyphs)
    -> (String, String, Style)  // (icon, name, style)
```

Returns the appropriate icon, display name, and style for each message type:
- System: muted bold
- User: green bold
- Assistant: provider-colored
- ToolUse: accent bold with compact tool name
- ToolResult: success/error with ok/error icon

**Wrapped Line Counting:**
```rust
pub(crate) fn wrapped_line_count_exact(lines: &[Line], width: u16) -> usize
pub(crate) fn wrapped_line_count(lines: &[Line], width: u16) -> u16  // Capped to u16
pub(crate) fn wrapped_line_rows(lines: &[Line], width: u16) -> Vec<usize>
pub(crate) fn truncate_with_ellipsis(raw: &str, max: usize) -> String
```

**Approval View:**
Collects and formats tool approval requests for display:

```rust
pub(crate) struct ApprovalItem {
    pub tool_name: String,
    pub risk_label: String,     // "HIGH", "MEDIUM", "LOW"
    pub Distillate: Option<String>,
    pub details: Vec<String>,   // Expanded JSON args
}

pub(crate) struct ApprovalView {
    pub items: Vec<ApprovalItem>,
    pub selected: Vec<bool>,
    pub cursor: usize,
    pub expanded: Option<usize>,
    pub any_selected: bool,
    pub deny_confirm: bool,
}

pub(crate) fn collect_approval_view(app: &App, max_width: usize) -> Option<ApprovalView>
```

### tool_display.rs - Compact Tool Call Display

Converts verbose tool calls to compact function-call style:

```rust
pub fn format_tool_call_compact(name: &str, args: &Value) -> String
```

Examples:
- `Search("foo.*bar")` instead of `{"pattern": "foo.*bar"}`
- `Read(src/main.rs)` instead of `{"path": "src/main.rs"}`
- `GitCommit(feat(tui): add display)` for structured commit args
- `GitAdd(-A)` or `GitAdd(3 file(s))`

**Canonical Names:**
Tool names are expected in PascalCase from the tool registry. The function maps them to display names:

| Tool Name | Display |
|-----------|---------|
| `Read`, `Write`, `Edit`, `Delete`, `Move`, `Copy` | File operations |
| `ListDir`, `Outline` | Directory/code inspection |
| `Glob`, `Search` | Search operations |
| `GitStatus`, `GitDiff`, `GitAdd`, `GitCommit`, etc. | Git operations |
| `Pwsh`, `Run` | Shell commands |
| `WebFetch` | URL fetching |
| `Build`, `Test` | Build system operations |

Unknown tools pass through as-is.

### tool_result_Distillate.rs - Tool Result Summarization

Determines how to render tool results:

```rust
pub enum ToolResultRender {
    Full { diff_aware: bool },  // Show complete output
    Distillate(String),            // Show compact Distillate
}
```

**Tool Kinds:**
```rust
pub(crate) enum ToolKind {
    Read,     // File reading
    Search,   // Content search (ripgrep)
    Glob,     // File pattern matching
    Shell,    // Shell commands (Run/Pwsh)
    Edit,     // File editing
    Write,    // File creation
    GitStatus,// Git status
    Other,    // Everything else
}
```

**Render Decision:**
- `Edit` always renders full with `diff_aware: true` (never Distilled)
- `Write` always renders full with `diff_aware: false` (never Distilled)
- Content with diff markers (`---`, `+++`, `@@`) gets `Full { diff_aware: true }`
- Other tools get tool-specific Distillates

**Tool-Specific Distillates:**

| Tool | Distillate Format |
|------|----------------|
| Read | "42 lines" or "lines 1-50" (if range specified in args) |
| Search | "3 matches in 2 files" (parsed from JSON output) |
| Glob | "5 files" (from JSON array or line count) |
| Run/Pwsh | "exit 0: first output line" (parsed from JSON or text) |
| GitStatus | "1 staged, 2 modified, 3 untracked" (git porcelain format) |
| Other | Line count or truncated first line |

### diff_render.rs - Diff-Aware Coloring

Applies semantic colors to diff output:

```rust
pub fn render_tool_result_lines(content: &str, base_style: Style, palette: &Palette, indent: &'static str) -> Vec<Line<'static>>
```

**Color Mapping:**

| Line Pattern | Color |
|--------------|-------|
| `---` / `+++` / `diff --git` | Muted, bold (header) |
| `@@` | Accent, bold (hunk header) |
| `-` prefix | Error (red) |
| `+` prefix | Success (green) |
| `...` | Muted, italic (gap marker) |

## Public API

### Exports from lib.rs

```rust
// Rendering
pub fn draw(frame: &mut Frame, app: &mut App)
pub fn draw_inline(frame: &mut Frame, app: &mut App)
pub fn clear_inline_viewport<B>(terminal: &mut Terminal<B>) -> Result<(), B::Error>

// Theme
pub fn palette(options: UiOptions) -> Palette
pub fn glyphs(options: UiOptions) -> Glyphs
pub fn spinner_frame(tick: usize, options: UiOptions) -> &'static str
pub mod styles  // Pre-defined style functions

// Effects (modal overlay animations)
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect

// Input
pub struct InputPump
pub fn handle_events(app: &mut App, input: &mut InputPump) -> Result<bool>

// Inline mode
pub const INLINE_INPUT_HEIGHT: u16
pub const INLINE_VIEWPORT_HEIGHT: u16
pub fn inline_viewport_height(mode: InputMode) -> u16
pub struct InlineOutput

// Markdown
pub mod markdown
pub fn clear_render_cache()
```

## Developer Notes

### Platform Considerations

- **Windows**: Filter to `KeyEventKind::Press` only; release events are sent separately
- **Terminal compatibility**: Use `ascii_only` option for terminals without Unicode support
- **Accessibility**: `reduced_motion` disables spinner animation; `high_contrast` switches to basic colors

### Performance

- **Message caching**: Static content is cached; only dynamic content (streaming, tool status) rebuilds each frame
- **Markdown caching**: Parsed results cached by content+style hash with LRU-style eviction
- **Wrapped line counting**: Use `wrapped_line_count_exact` for accuracy; expensive for large content

### Common Pitfalls

- **Scrollbar visibility**: Only render when `max_scroll > 0` (content exceeds viewport)
- **Scrollbar position**: Use `max_scroll` as content_length, not total lines
- **No eprintln!**: Use `tracing::warn!` to avoid corrupting TUI output
- **Cursor positioning**: Convert character index to byte index for string operations

### Adding New Overlays

1. Add draw function in `lib.rs`
2. Call from main `draw()` based on app state
3. Use `apply_modal_effect` for animations
4. Handle Clear widget for background

### Adding New Input Modes

1. Add handler in `input.rs`
2. Add to dispatch in `handle_events`
3. Handle modal priority (approval/recovery)
4. Update key hints in `draw_input`

### Extending Theme

1. Add colors to `Palette` struct
2. Add style function to `styles` module
3. Update `high_contrast()` fallback
4. Consider `ascii_only` glyph fallbacks


# forge-tui

Terminal user interface rendering and input handling for Forge, built on [ratatui](https://ratatui.rs) and [crossterm](https://github.com/crossterm-rs/crossterm).

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-25 | Header, Intro, LLM-TOC, Table of Contents |
| 27-37 | Purpose and Responsibility |
| 39-77 | Module Overview and Rendering |
| 78-623 | Key Modules: lib.rs, input.rs, theme.rs, markdown.rs, effects.rs, shared.rs, tool_display.rs, tool_result_summary.rs, diff_render.rs |
| 625-651 | Public API |
| 653-696 | Developer Notes |

## Table of Contents

1. [Purpose and Responsibility](#purpose-and-responsibility)
2. [Module Overview](#module-overview)
3. [Rendering](#rendering)
4. [Key Modules](#key-modules)
5. [Public API](#public-api)
6. [Developer Notes](#developer-notes)

---

## Purpose and Responsibility

The `forge-tui` crate is responsible for:

- **Rendering**: Drawing the full-screen UI to the terminal via crossterm's alternate screen
- **Input handling**: Processing keyboard events and dispatching to mode-specific handlers
- **Theming**: Providing consistent colors, styles, and glyphs across the interface
- **Markdown rendering**: Converting markdown content to styled terminal output
- **Modal effects**: Animating overlay transitions (pop-scale, slide-up, shake)

This crate is purely presentational. It renders state from `forge-engine` and forwards user input back to it. It contains no business logic, API calls, or persistence.

## Module Overview

```
tui/src/
├── lib.rs                  # Full-screen rendering, message display, overlays
├── input.rs                # Keyboard event handling, paste detection, mode dispatch
├── theme.rs                # Color palette, styles, and glyphs
├── markdown.rs             # Markdown to ratatui conversion with caching
├── effects.rs              # Modal and panel animation transforms
├── shared.rs               # Rendering helpers shared across the crate
├── tool_display.rs         # Compact tool call formatting
├── tool_result_summary.rs  # Tool result summarization logic
└── diff_render.rs          # Diff-aware coloring for tool output
```

## Rendering

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
- Files panel with compact and expanded diff views
- Welcome screen when conversation is empty

## Key Modules

### lib.rs - Full-Screen Rendering

Entry point: `draw(frame: &mut Frame, app: &mut App)`

**Layout Structure:**
1. Clear frame with background color
2. Split into main area and optional files panel (horizontal)
3. Split main area into messages area and input area (vertical)
4. Render messages with scrolling
5. Render input with mode-specific styling
6. Overlay command palette, model selector, or approval prompts as needed

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

### input.rs - Keyboard Input Handling

Entry point: `handle_events(app: &mut App, input: &mut InputPump) -> Result<bool>`

**Event Processing:**
1. `InputPump` runs a blocking reader loop (25ms poll) and pushes events into a bounded channel (capacity 1024)
2. `handle_events` drains up to 64 events per frame (non-blocking) and ignores `KeyEventKind::Release`
3. Handle global Ctrl+C for cancellation (cancels active operation or exits)
4. Handle global Esc for cancellation (only in Normal mode with no panel/modal active)
5. Dispatch to mode-specific handler

**Paste Detection:**
On Windows, crossterm delivers paste as a burst of rapid key events rather than `Event::Paste`. The `PasteDetector` uses heuristics to identify these bursts and treats bare `Enter` as a newline insertion (instead of message submission) during a detected paste:

| Constant | Value | Purpose |
|----------|-------|---------|
| `PASTE_INTER_KEY_THRESHOLD` | 20ms | Max gap between keys to consider rapid |
| `PASTE_IDLE_TIMEOUT` | 75ms | How long paste mode stays active after last rapid key |
| `PASTE_QUEUE_THRESHOLD` | 32 | Backlog size that immediately triggers paste mode |

**Mode Handlers:**

| Mode | Handler | Key Behaviors |
|------|---------|---------------|
| Normal | `handle_normal_mode` | Navigation, mode entry, quit |
| Insert | `handle_insert_mode` | Text editing, message send, paste handling |
| Command | `handle_command_mode` | Command input, tab completion, execution |
| ModelSelect | `handle_model_select_mode` | Selection, confirmation |
| FileSelect | `handle_file_select_mode` | File filtering and insertion |

**Modal Priority:**
Tool approval and recovery modals take priority over mode-specific handling. When active, they intercept key events regardless of input mode.

**Key Bindings (Tool Approval Modal):**

| Key | Action |
|-----|--------|
| `k` / `Up` | Move cursor up |
| `j` / `Down` | Move cursor down |
| `Space` | Toggle selection |
| `Tab` | Toggle details |
| `a` | Approve all |
| `d` / `Esc` | Request deny all |
| `Enter` | Activate (approve selected) |

**Key Bindings (Tool Recovery Modal):**

| Key | Action |
|-----|--------|
| `r` / `R` | Resume recovered batch |
| `d` / `D` / `Esc` | Discard recovered batch |

**Key Bindings (Normal Mode):**

| Key | Action |
|-----|--------|
| `q` | Quit |
| `i` | Insert mode |
| `a` | Insert at end |
| `o` | Toggle thinking visibility |
| `:` / `/` | Command mode |
| `m` | Model selector (blocked during active operations) |
| `f` | Toggle files panel |
| `k` / `Up` | Scroll up |
| `j` / `Down` | Scroll down |
| `g` | Scroll to top |
| `G` / `End` / `Right` | Scroll to bottom |
| `PageUp` | Page up |
| `PageDown` | Page down |
| `Ctrl+U` | Page up (or scroll diff up when files panel expanded) |
| `Ctrl+D` | Page down (or scroll diff down when files panel expanded) |
| `Left` | Scroll up by 20% chunk |
| `Tab` / `Shift+Tab` | Files panel: next/previous file |
| `Enter` / `Esc` | Files panel: collapse expanded diff |
| `Backspace` | Files panel: collapse expanded diff, or close panel if compact |

**Key Bindings (Insert Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Normal mode |
| `Enter` | Send message (or insert newline during detected paste) |
| `Ctrl+Enter` / `Shift+Enter` / `Ctrl+J` | Insert newline |
| `Up` / `Down` | Navigate prompt history |
| `Backspace` | Exit to Normal mode if draft empty, otherwise delete backward |
| `Delete` | Delete forward |
| `Left` / `Right` | Move cursor |
| `Ctrl+U` | Clear line |
| `Ctrl+W` | Delete word backward |
| `Home` / `End` | Jump to start/end |
| `@` | Open file selector |

**Key Bindings (Command Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Normal mode |
| `Enter` | Execute command |
| `Up` / `Down` | Navigate command history |
| `Backspace` | Exit to Normal mode if empty, otherwise delete backward |
| `Left` / `Right` | Move cursor |
| `Home` / `Ctrl+A` | Jump to start |
| `End` / `Ctrl+E` | Jump to end |
| `Ctrl+W` | Delete word backward |
| `Ctrl+U` | Clear line |
| `Tab` | Tab completion |

**Key Bindings (Model Select Mode):**

| Key | Action |
|-----|--------|
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `1`-`9` | Direct selection by index (selects and confirms) |

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
    pub thinking: &'static str,         // "◦" or "?"
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

Entry point: `render_markdown(content: &str, base_style: Style, palette: &Palette, max_width: u16) -> Vec<Line<'static>>`

**Features:**
- Headings (bold, with spacing)
- Bold / italic with proper nesting (counters, not booleans)
- Code blocks with fence markers (```language)
- Inline code (peach colored, bold)
- Ordered and unordered lists with nesting (4-space indent per level)
- Tables with box-drawing borders (unicode width aware, cell wrapping)
- Paragraphs with automatic spacing
- `<br>` tag handling (converted to line breaks)

**Caching:**
Thread-local cache with automatic eviction:

```rust
const CACHE_MAX_ENTRIES: usize = 128;

thread_local! {
    static RENDER_CACHE: RefCell<HashMap<CacheKey, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}
```

Cache key combines content hash, style hash, palette hash, `soft_breaks_as_newlines` flag, and `max_width`. Eviction removes half the cache when full.

**Streaming Support:**
Handles incomplete code blocks (common during streaming) by rendering partial content with opening fence.

**HTML/XML Handling:**
Renders HTML and XML-like content as plain text rather than silently dropping it. This preserves LLM output that may contain XML-like tags. `<br>` tags are recognized and converted to actual line breaks.

**Table Rendering:**

```
┌───────┬───────┬───────┐
│ Col A │ Col B │ Col C │
├───────┼───────┼───────┤
│ 1     │ 2     │ 3     │
└───────┴───────┴───────┘
```

Uses `unicode-width` for proper handling of CJK characters and emoji. When the table exceeds `max_width`, column widths are proportionally shrunk and cell content is word-wrapped. A minimum column width of 3 characters is enforced.

**Soft Break Mode:**
An internal `render_markdown_preserve_newlines` variant treats soft breaks (single newlines) as hard line breaks. This is used for content where the original line structure should be preserved.

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

Common utilities used across the crate. All functions are `pub(crate)`.

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
```

**Message Headers:**
```rust
pub(crate) fn message_header_parts(msg: &Message, palette: &Palette, glyphs: &Glyphs)
    -> (String, String, Style)  // (icon, name, style)
```

Returns the appropriate icon, display name, and style for each message type:
- System: muted bold
- User: green bold
- Assistant: provider-colored (no name label -- color encodes provider)
- Thinking: provider-colored, italic
- ToolUse: accent bold with compact tool name
- ToolResult: success/error with ok/error icon

**Wrapped Line Counting:**
```rust
pub(crate) fn wrapped_line_count_exact(lines: &[Line], width: u16) -> usize
pub(crate) fn wrapped_line_rows(lines: &[Line], width: u16) -> Vec<usize>
```

Note: `truncate_with_ellipsis` has moved to `forge_types`.

**Approval View:**
Collects and formats tool approval requests for display:

```rust
pub(crate) struct ApprovalItem {
    pub tool_name: String,
    pub risk_label: String,          // "HIGH", "MEDIUM", "LOW"
    pub summary: Option<String>,
    pub details: Vec<String>,        // Expanded JSON args
    pub homoglyph_warnings: Vec<String>,  // Mixed-script warnings
}

pub(crate) struct ApprovalView {
    pub items: Vec<ApprovalItem>,
    pub selected: Vec<bool>,
    pub cursor: usize,
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
- `Search(foo.*bar)` instead of `{"pattern": "foo.*bar"}`
- `Read(src/main.rs)` instead of `{"path": "src/main.rs"}`
- `Edit(src/main.rs)` or `Edit(3 files)` (parses LP1 patch format)
- `GitCommit(feat(tui): add display)` for structured commit args
- `GitAdd(-A)` or `GitAdd(3 file(s))`
- `GitDiff(main..feature)` or `GitDiff(--cached)`
- `GitBranch(create feature-x)` or `GitBranch(-a)`
- `GitCheckout(main)` or `GitCheckout(-b new-branch)`

**Canonical Names:**
Tool names are expected in PascalCase from the tool registry. The function maps them to display names:

| Tool Name | Category |
|-----------|----------|
| `Read`, `Write`, `Edit`, `Delete`, `Move`, `Copy` | File operations |
| `ListDir`, `Outline` | Directory/code inspection |
| `Glob`, `Search` | Search operations |
| `GitStatus`, `GitDiff`, `GitAdd`, `GitCommit`, `GitStash`, `GitRestore`, `GitBranch`, `GitCheckout`, `GitShow`, `GitLog`, `GitBlame` | Git operations |
| `Pwsh`, `Run` | Shell commands |
| `WebFetch` | URL fetching |
| `Recall` | Memory retrieval |
| `Build`, `Test` | Build system operations |

Unknown tools pass through as-is, with fallback key extraction from common argument names (`pattern`, `path`, `query`, `command`, `url`, `file`, `name`).

### tool_result_summary.rs - Tool Result Summarization

Determines how to render tool results:

```rust
pub(crate) enum ToolResultRender {
    Full { diff_aware: bool },  // Show complete output
    Summary(String),            // Show compact summary
}
```

**Tool Kinds:**
```rust
pub(crate) enum ToolKind {
    Read,      // File reading
    Search,    // Content search (ripgrep)
    Glob,      // File pattern matching
    Shell,     // Shell commands (Run/Pwsh)
    Edit,      // File editing
    Write,     // File creation
    GitStatus, // Git status
    GitCommit, // Git commit
    Other,     // Everything else
}
```

**Render Decision:**
- `Edit` always renders full with `diff_aware: true` (never summarized)
- `Write` always renders full with `diff_aware: false` (never summarized)
- Content with diff markers (`---`, `+++`, `@@`, `diff --git`) gets `Full { diff_aware: true }`
- Other tools get tool-specific summaries

**Tool-Specific Summaries:**

| Tool | Summary Format |
|------|----------------|
| Read | "42 lines" or "lines 1-50" (if range specified in args) |
| Search | "3 matches in 2 files" (parsed from JSON output) |
| Glob | "5 files" (from JSON array or line count) |
| Run/Pwsh | "exit 0: first output line" (parsed from JSON or text) |
| GitStatus | "1 staged, 2 modified, 3 untracked" (git porcelain format) |
| GitCommit | "abc1234 feat: add feature" (short hash + commit message) |
| Other | Line count or truncated first line |

### diff_render.rs - Diff-Aware Coloring

Applies semantic colors to diff output:

```rust
pub fn render_tool_result_lines(content: &str, base_style: Style, palette: &Palette, indent: &'static str) -> Vec<Line<'static>>
```

**Color Mapping:**

| Line Pattern | Color |
|--------------|-------|
| `---` / `+++` / `diff --git` / `index ` / `new file mode` / `deleted file mode` | Muted, bold (header) |
| `@@` | Accent, bold (hunk header) |
| `-` prefix | Error (red) |
| `+` prefix | Success (green) |
| `...` | Muted, italic (gap marker) |

## Public API

### Exports from lib.rs

```rust
// Rendering
pub fn draw(frame: &mut Frame, app: &mut App)
pub fn draw_model_selector(frame: &mut Frame, app: &mut App, palette: &Palette, glyphs: &Glyphs, elapsed: Duration)

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

// Markdown
pub mod markdown
pub fn render_markdown(content: &str, base_style: Style, palette: &Palette, max_width: u16) -> Vec<Line<'static>>
pub fn clear_render_cache()
```

## Developer Notes

### Platform Considerations

- **Windows**: Filter to `KeyEventKind::Press` only; release events are sent separately
- **Windows paste**: Paste arrives as rapid key bursts (not `Event::Paste`); the `PasteDetector` heuristic handles this
- **Terminal compatibility**: Use `ascii_only` option for terminals without Unicode support
- **Accessibility**: `reduced_motion` disables spinner animation; `high_contrast` switches to basic colors

### Performance

- **Message caching**: Static content is cached; only dynamic content (streaming, tool status) rebuilds each frame
- **Markdown caching**: Parsed results cached by content+style+width hash with LRU-style eviction
- **Wrapped line counting**: Use `wrapped_line_count_exact` for accuracy; expensive for large content
- **Event throttling**: Max 64 events processed per frame to avoid starving rendering

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
2. Add to dispatch in `handle_events` (within `apply_event`)
3. Handle modal priority (approval/recovery interceptors)
4. Update key hints in `draw_input`

### Extending Theme

1. Add colors to `Palette` struct
2. Add style function to `styles` module
3. Update `high_contrast()` fallback
4. Consider `ascii_only` glyph fallbacks

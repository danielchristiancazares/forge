# Forge TUI Crate Documentation

This document provides comprehensive technical documentation for the `forge-tui` crate, which handles all terminal user interface rendering and input processing for Forge.

## Table of Contents

1. [Crate Overview](#crate-overview)
2. [Module Structure](#module-structure)
3. [Public API](#public-api)
4. [Full-Screen Rendering System](#full-screen-rendering-system)
5. [Inline Terminal Rendering](#inline-terminal-rendering)
6. [Keyboard Input Handling](#keyboard-input-handling)
7. [Theme System](#theme-system)
8. [Markdown Rendering](#markdown-rendering)
9. [Modal Animation Effects](#modal-animation-effects)
10. [Scrolling and Viewport Management](#scrolling-and-viewport-management)
11. [Widget Composition Patterns](#widget-composition-patterns)
12. [Extension Guide](#extension-guide)

---

## Crate Overview

The `forge-tui` crate is responsible for rendering the terminal user interface and handling keyboard input. It is built on top of [ratatui](https://github.com/ratatui-org/ratatui) for widget composition and [crossterm](https://github.com/crossterm-rs/crossterm) for terminal manipulation.

### Dependencies

```toml
[dependencies]
forge-engine.workspace = true      # App state, commands, types
forge-types.workspace = true       # ToolResult and shared types

ratatui.workspace = true           # TUI widget framework
crossterm.workspace = true         # Terminal backend

unicode-width.workspace = true     # Unicode display width calculation
unicode-segmentation.workspace = true  # Grapheme cluster handling
pulldown-cmark.workspace = true    # Markdown parsing
arboard.workspace = true           # Clipboard support (reserved)
serde_json.workspace = true        # Tool argument formatting

tokio = { workspace = true, features = ["sync", "rt"] }
anyhow.workspace = true
```

### Design Principles

1. **Separation of Concerns**: Rendering (`lib.rs`, `ui_inline.rs`) is separate from input handling (`input.rs`) and styling (`theme.rs`).

2. **Mode-Aware Rendering**: The UI adapts based on the current `InputMode` (Normal, Insert, Command, ModelSelect).

3. **Cached Computations**: Expensive operations like markdown parsing are cached to maintain smooth frame rates.

4. **Unicode-Aware**: All text handling uses proper Unicode width calculations for CJK characters, emoji, and combining characters.

5. **Dual Mode Support**: Both full-screen (alternate screen) and inline (terminal history) modes share common components.

---

## Module Structure

```
tui/src/
├── lib.rs          # Full-screen rendering, public exports
├── ui_inline.rs    # Inline terminal rendering
├── input.rs        # Keyboard event handling
├── theme.rs        # Color palette and style definitions
├── markdown.rs     # Markdown to ratatui conversion
└── effects.rs      # Modal animation transforms
```

### Module Responsibilities

| Module | Responsibility |
|--------|----------------|
| `lib.rs` | Full-screen layout, message rendering, overlays (command palette, model selector, tool prompts), scrollbar, welcome screen |
| `ui_inline.rs` | Fixed-height viewport rendering, incremental message output to terminal history |
| `input.rs` | Keyboard event polling, mode dispatch, key-to-action mapping |
| `theme.rs` | RGB color constants, pre-defined styles, spinner animation frames |
| `markdown.rs` | Markdown parsing with pulldown-cmark, caching, table/code block rendering |
| `effects.rs` | Modal animation transforms (PopScale, SlideUp), easing functions |

---

## Public API

The crate exports the following items from `lib.rs`:

### Functions

```rust
/// Main full-screen draw function
pub fn draw(frame: &mut Frame, app: &mut App)

/// Inline mode draw function
pub fn draw_inline(frame: &mut Frame, app: &mut App)

/// Handle keyboard events, returns true if app should quit
pub async fn handle_events(app: &mut App) -> Result<bool>

/// Clear the inline viewport (used on exit)
pub fn clear_inline_viewport<B>(terminal: &mut Terminal<B>) -> Result<(), B::Error>

/// Get spinner animation frame for given tick count
pub fn spinner_frame(tick: usize) -> &'static str

/// Apply modal animation effect to transform a rectangle
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect

/// Clear the markdown render cache
pub fn clear_render_cache()
```

### Modules

```rust
/// Color constants and style definitions
pub use theme::{colors, styles};

/// Markdown rendering (for external use if needed)
pub mod markdown;
```

### Constants

```rust
/// Height of the input area in inline mode (5 lines)
pub const INLINE_INPUT_HEIGHT: u16 = 5;

/// Total viewport height in inline mode (input + status bar)
pub const INLINE_VIEWPORT_HEIGHT: u16 = 6;

/// Height needed for model selector overlay in inline mode
pub const INLINE_MODEL_SELECTOR_HEIGHT: u16 = 18;
```

### Types

```rust
/// Tracks incremental output state for inline mode
pub struct InlineOutput { ... }

impl InlineOutput {
    pub fn new() -> Self;
    pub fn flush<B>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error>;
}

/// Returns required viewport height for current input mode
pub fn inline_viewport_height(mode: InputMode) -> u16
```

---

## Full-Screen Rendering System

The full-screen renderer in `lib.rs` manages the complete terminal display using ratatui's immediate-mode rendering.

### Main Draw Function

```rust
pub fn draw(frame: &mut Frame, app: &mut App) {
    // 1. Clear with background color
    let bg_block = Block::default().style(Style::default().bg(colors::BG_DARK));
    frame.render_widget(bg_block, frame.area());

    // 2. Calculate input height based on mode
    let input_height = match app.input_mode() {
        InputMode::Normal => 3,
        _ => 5,
    };

    // 3. Create vertical layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(1),               // Messages (flex)
            Constraint::Length(input_height), // Input
            Constraint::Length(1),            // Status bar
        ])
        .split(frame.area());

    // 4. Render main components
    draw_messages(frame, app, chunks[0]);
    draw_input(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    // 5. Render overlays (order matters - later = on top)
    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app);
    }
    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app);
    }
    if app.tool_approval_requests().is_some() {
        draw_tool_approval_prompt(frame, app);
    }
    if app.tool_recovery_calls().is_some() {
        draw_tool_recovery_prompt(frame, app);
    }
}
```

### Layout Structure

```
┌─────────────────────────────────────────────────────────┐
│ margin (1)                                              │
│ ┌─────────────────────────────────────────────────────┐ │
│ │                                                     │ │
│ │                 Messages Area                       │ │
│ │              (Constraint::Min(1))                   │ │
│ │                                                     │ │
│ │           Scrollable, wrapping text                 │ │
│ │          with scrollbar on right edge               │ │
│ │                                                     │ │
│ ├─────────────────────────────────────────────────────┤ │
│ │ [MODE] │ ❯ input text...          key hints │ usage │ │
│ │        │                                            │ │
│ │                 Input Area                          │ │
│ │          (Constraint::Length(3|5))                  │ │
│ ├─────────────────────────────────────────────────────┤ │
│ │ ● Provider │ model-name                  CI: status │ │
│ │                 Status Bar                          │ │
│ │            (Constraint::Length(1))                  │ │
│ └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

### Message Rendering

Messages are rendered with role-specific styling and icons:

```rust
fn render_message(msg: &Message, lines: &mut Vec<Line>, msg_count: &mut usize) {
    // Add spacing between messages
    if *msg_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    // Role-specific header
    let (icon, name, name_style) = match msg {
        Message::System(_) => ("●", "System", Style::default().fg(colors::TEXT_MUTED).bold()),
        Message::User(_) => ("○", "You", styles::user_name()),
        Message::Assistant(m) => ("◆", m.provider().display_name(), styles::assistant_name()),
        Message::ToolUse(call) => ("⚙", &call.name, Style::default().fg(colors::ACCENT).bold()),
        Message::ToolResult(r) => {
            if r.is_error { ("✗", "Tool Result", Style::default().fg(colors::ERROR).bold()) }
            else { ("✓", "Tool Result", Style::default().fg(colors::SUCCESS).bold()) }
        }
    };

    // Render header line
    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", icon), name_style),
        Span::styled(name, name_style),
    ]));
    lines.push(Line::from(""));

    // Render content based on message type
    match msg {
        Message::ToolUse(call) => {
            // Render as formatted JSON
            let args_str = serde_json::to_string_pretty(&call.arguments).unwrap_or_default();
            for line in args_str.lines() {
                lines.push(Line::from(Span::styled(format!("  {}", line), Style::default().fg(colors::TEXT_MUTED))));
            }
        }
        Message::ToolResult(result) => {
            let style = if result.is_error { colors::ERROR } else { colors::TEXT_SECONDARY };
            for line in result.content.lines() {
                lines.push(Line::from(Span::styled(format!("  {}", line), Style::default().fg(style))));
            }
        }
        _ => {
            // Render as markdown
            let rendered = render_markdown(msg.content(), base_style);
            lines.extend(rendered);
        }
    }
}
```

### Role Icons and Styles

| Role | Icon | Color | Style |
|------|------|-------|-------|
| System | `●` | TEXT_MUTED | Bold |
| User | `○` | GREEN | Bold |
| Assistant | `◆` | PRIMARY (purple) | Bold |
| Tool Use | `⚙` | CYAN/ACCENT | Bold |
| Tool Result (success) | `✓` | GREEN/SUCCESS | Bold |
| Tool Result (error) | `✗` | RED/ERROR | Bold |

### Streaming Message Display

When streaming is active, the renderer shows either a spinner or partial content:

```rust
if let Some(streaming) = app.streaming() {
    // Render header
    let header_line = Line::from(vec![
        Span::styled(" ◆ ", styles::assistant_name()),
        Span::styled(streaming.provider().display_name(), styles::assistant_name()),
    ]);
    lines.push(header_line);
    lines.push(Line::from(""));

    if streaming.content().is_empty() {
        // Show animated spinner
        let spinner = spinner_frame(app.tick_count());
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(spinner, Style::default().fg(colors::PRIMARY)),
            Span::styled(" Thinking...", Style::default().fg(colors::TEXT_MUTED)),
        ]));
    } else {
        // Show partial content as markdown
        let rendered = render_markdown(streaming.content(), Style::default().fg(colors::TEXT_SECONDARY));
        lines.extend(rendered);
    }
}
```

### Tool Execution Status

The tool loop status shows real-time progress with visual indicators:

```rust
// Status icons for tool execution
let icon = if let Some(result) = results_map.get(call.id.as_str()) {
    if !execute_ids.contains(call.id.as_str()) {
        "⊘"  // Denied/skipped
    } else if result.is_error {
        "✗"  // Error
    } else {
        "✓"  // Success
    }
} else if current_id == Some(call.id.as_str()) {
    spinner  // Currently executing (animated)
} else if approval_pending && !execute_ids.contains(call.id.as_str()) {
    "⏸"  // Awaiting approval
} else {
    "•"  // Queued
};
```

### Input Area Rendering

The input area adapts to the current mode with appropriate styling:

```rust
pub(crate) fn draw_input(frame: &mut Frame, app: &mut App, area: Rect) {
    let mode = app.input_mode();

    // Mode-specific configuration
    let (mode_text, mode_style, border_style, prompt_char) = match mode {
        InputMode::Normal | InputMode::ModelSelect => (
            " NORMAL ", styles::mode_normal(), Style::default().fg(colors::TEXT_MUTED), ""
        ),
        InputMode::Insert => (
            " INSERT ", styles::mode_insert(), Style::default().fg(colors::GREEN), "❯"
        ),
        InputMode::Command => (
            " COMMAND ", styles::mode_command(), Style::default().fg(colors::YELLOW), "/"
        ),
    };

    // Mode-specific key hints
    let hints = match mode {
        InputMode::Normal => vec![
            Span::styled("i", styles::key_highlight()),
            Span::styled(" insert  ", styles::key_hint()),
            Span::styled("/", styles::key_highlight()),
            Span::styled(" command  ", styles::key_hint()),
            Span::styled("q", styles::key_highlight()),
            Span::styled(" quit ", styles::key_hint()),
        ],
        // ... other modes
    };

    // Context usage indicator with severity coloring
    let usage_status = app.context_usage_status();
    let (usage, severity) = match &usage_status {
        ContextUsageStatus::Ready(u) => (u, 0),
        ContextUsageStatus::NeedsSummarization { usage, .. } => (usage, 1),
        ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => (usage, 2),
    };
    let usage_color = match severity {
        1 | 2 => colors::RED,
        _ => match usage.severity() {
            0 => colors::GREEN,   // < 70%
            1 => colors::YELLOW,  // 70-90%
            _ => colors::RED,     // > 90%
        },
    };

    // Build and render input widget with block decorations
    let input = Paragraph::new(Line::from(input_content)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(Line::from(vec![Span::styled(mode_text, mode_style)]))
            .title_top(Line::from(hints).alignment(Alignment::Right))
            .title_bottom(Line::from(usage_indicator).alignment(Alignment::Right))
            .padding(input_padding),
    );

    frame.render_widget(input, area);

    // Set cursor position in Insert/Command mode
    if mode == InputMode::Insert || mode == InputMode::Command {
        let cursor_x = area.x + 4 + cursor_display_pos.saturating_sub(horizontal_scroll);
        let cursor_y = area.y + 2;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}
```

### Horizontal Scroll for Long Input

When the input text exceeds the visible width, horizontal scrolling keeps the cursor visible:

```rust
let visible_content_width = area.width.saturating_sub(6) as usize;

let (display_text, horizontal_scroll) = if mode == InputMode::Insert {
    let cursor_index = app.draft_cursor_byte_index();
    let draft = app.draft_text();
    let text_before_cursor = &draft[..cursor_index];
    let cursor_display_pos = text_before_cursor.width();

    if cursor_display_pos >= visible_content_width {
        // Calculate scroll offset to keep cursor visible
        let scroll_target = cursor_display_pos - visible_content_width + 1;
        let mut byte_offset = 0;
        let mut skipped_width = 0;
        for (idx, grapheme) in draft.grapheme_indices(true) {
            if skipped_width >= scroll_target {
                byte_offset = idx;
                break;
            }
            skipped_width += grapheme.width();
        }
        (draft[byte_offset..].to_string(), skipped_width as u16)
    } else {
        (draft.to_string(), 0u16)
    }
} else {
    (app.draft_text().to_string(), 0u16)
};
```

### Overlay Rendering

Overlays are rendered after the main layout, using `Clear` to erase the background:

```rust
fn draw_command_palette(frame: &mut Frame, _app: &App) {
    let area = frame.area();

    // Calculate centered position
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 10;
    let palette_area = Rect {
        x: area.x + (area.width.saturating_sub(palette_width) / 2),
        y: area.y + (area.height / 3),
        width: palette_width,
        height: palette_height,
    };

    // Clear the overlay area
    frame.render_widget(Clear, palette_area);

    // Build command list
    let commands = vec![
        ("q, quit", "Exit the application"),
        ("clear", "Clear conversation history"),
        ("model <name>", "Change the model"),
        // ...
    ];

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (cmd, desc) in commands {
        lines.push(Line::from(vec![
            Span::styled(format!("  /{cmd}"), Style::default().fg(colors::PEACH)),
            Span::styled(format!("  {desc}"), Style::default().fg(colors::TEXT_MUTED)),
        ]));
    }

    let palette = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(colors::PRIMARY))
            .style(Style::default().bg(colors::BG_PANEL))
            .title(" Commands "),
    );

    frame.render_widget(palette, palette_area);
}
```

### Model Selector with Animation

The model selector demonstrates animated overlay rendering:

```rust
pub fn draw_model_selector(frame: &mut Frame, app: &mut App) {
    // Calculate base area
    let base_area = Rect { /* ... */ };

    // Apply animation effect
    let elapsed = app.frame_elapsed();
    let (selector_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, frame.area()),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    // Clear and render
    frame.render_widget(Clear, selector_area);

    // Build model list with selection highlighting
    let models = PredefinedModel::all();
    for (i, model) in models.iter().enumerate() {
        let is_selected = i == selected_index;
        let bg = if is_selected { Some(colors::BG_HIGHLIGHT) } else { None };
        let style = if is_selected {
            Style::default().fg(colors::TEXT_PRIMARY).bold().bg(bg.unwrap())
        } else {
            Style::default().fg(colors::TEXT_SECONDARY)
        };
        // ... build line with prefix indicator
    }

    frame.render_widget(selector, selector_area);
}
```

---

## Inline Terminal Rendering

The inline renderer in `ui_inline.rs` provides a minimal viewport that preserves terminal history.

### Design Philosophy

Unlike full-screen mode which takes over the entire terminal, inline mode:

1. Uses a fixed-height viewport at the bottom of the terminal
2. Writes completed messages above the viewport (into terminal scrollback)
3. Shows only the input area and status bar in the viewport
4. Preserves terminal history when Forge exits

### InlineOutput State

```rust
#[derive(Default)]
pub struct InlineOutput {
    next_display_index: usize,           // Track which messages have been printed
    has_output: bool,                    // Whether any output has been written
    last_tool_output_len: usize,         // Track tool output lines for incremental display
    last_tool_status_signature: Option<String>,   // Detect tool status changes
    last_pending_tool_signature: Option<String>,  // Detect pending tool changes
    last_approval_signature: Option<String>,      // Detect approval state changes
    last_recovery_active: bool,          // Track recovery prompt visibility
}
```

### Incremental Output

The `flush` method writes new messages to the terminal history:

```rust
pub fn flush<B>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error>
where
    B: Backend,
{
    let items = app.display_items();
    let mut lines: Vec<Line> = Vec::new();
    let mut msg_count = if self.has_output { 1 } else { 0 };

    // Process new messages since last flush
    if self.next_display_index < items.len() {
        for item in &items[self.next_display_index..] {
            let msg = match item {
                DisplayItem::History(id) => app.history().get_entry(*id).message(),
                DisplayItem::Local(msg) => msg,
            };
            append_message_lines(&mut lines, msg, &mut msg_count);
        }
        self.next_display_index = items.len();
    }

    // Check for tool status changes
    let tool_signature = tool_status_signature(app);
    if tool_signature != self.last_tool_status_signature {
        if tool_signature.is_some() {
            append_tool_status_lines(&mut lines, app);
        }
        self.last_tool_status_signature = tool_signature;
    }

    // ... similar checks for pending tools, approvals, recovery

    if lines.is_empty() {
        return Ok(());
    }

    // Calculate wrapped height and insert above viewport
    let width = terminal.size()?.width.max(1);
    let height = wrapped_line_count(&lines, width);

    terminal.insert_before(height, |buf| {
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(buf.area, buf);
    })?;

    self.has_output = true;
    Ok(())
}
```

### Signature-Based Change Detection

To avoid duplicate output, inline mode uses "signatures" to detect state changes:

```rust
fn tool_status_signature(app: &App) -> Option<String> {
    let calls = app.tool_loop_calls()?;
    // Build a string representing current state
    let mut parts = Vec::with_capacity(calls.len());
    for call in calls {
        let status = if let Some(result) = results_map.get(call.id.as_str()) {
            if result.is_error { "error" } else { "ok" }
        } else if current_id == Some(call.id.as_str()) {
            "running"
        } else {
            "pending"
        };
        parts.push(format!("{}:{status}", call.id));
    }
    Some(parts.join("|"))
}
```

### Viewport Drawing

```rust
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let input_height = match app.input_mode() {
        InputMode::Normal => 3,
        _ => INLINE_INPUT_HEIGHT,  // 5
    };
    let total_height = input_height + 1;

    // Position content at bottom of viewport
    let top_padding = area.height.saturating_sub(total_height);
    let content_area = Rect {
        x: area.x,
        y: area.y.saturating_add(top_padding),
        width: area.width,
        height: area.height.saturating_sub(top_padding),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(input_height), Constraint::Length(1)])
        .split(content_area);

    draw_input(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);

    // Model selector overlay if active
    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app);
    }
}
```

### Differences from Full-Screen Mode

| Aspect | Full-Screen | Inline |
|--------|-------------|--------|
| Message display | Scrollable in viewport | Written to terminal history |
| Viewport size | Entire terminal | Fixed 6-line area |
| Markdown | Full rendering | Plain text with indentation |
| Role icons | Unicode symbols (◆, ●) | ASCII-friendly (*, S) |
| Terminal history | Preserved (alternate screen) | Extended with messages |
| Streaming display | In-place updates | Only input area updates |

---

## Keyboard Input Handling

The `input.rs` module handles all keyboard event processing.

### Main Entry Point

```rust
pub async fn handle_events(app: &mut App) -> Result<bool> {
    // Poll for events without blocking the async runtime
    let event = tokio::task::spawn_blocking(|| -> Result<Option<Event>> {
        if event::poll(Duration::from_millis(100))? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    })
    .await??;

    if let Some(Event::Key(key)) = event {
        // Windows compatibility: only handle Press events
        if key.kind != KeyEventKind::Press {
            return Ok(app.should_quit());
        }

        // Global handler: Ctrl+C always quits
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }

        // Dispatch to mode-specific handler
        match app.input_mode() {
            InputMode::Normal => handle_normal_mode(app, key),
            InputMode::Insert => handle_insert_mode(app, key),
            InputMode::Command => handle_command_mode(app, key),
            InputMode::ModelSelect => handle_model_select_mode(app, key),
        }
    }

    Ok(app.should_quit())
}
```

### Normal Mode Handler

```rust
fn handle_normal_mode(app: &mut App, key: KeyEvent) {
    // Tool approval takes priority
    if app.tool_approval_requests().is_some() {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => app.tool_approval_move_up(),
            KeyCode::Char('j') | KeyCode::Down => app.tool_approval_move_down(),
            KeyCode::Char(' ') => app.tool_approval_toggle(),
            KeyCode::Char('a') => app.tool_approval_approve_all(),
            KeyCode::Char('d') => app.tool_approval_deny_all(),
            KeyCode::Enter => app.tool_approval_confirm_selected(),
            KeyCode::Esc => app.tool_approval_deny_all(),
            _ => {}
        }
        return;
    }

    // Tool recovery takes priority
    if app.tool_recovery_calls().is_some() {
        match key.code {
            KeyCode::Char('r') => app.tool_recovery_resume(),
            KeyCode::Char('d') => app.tool_recovery_discard(),
            KeyCode::Esc => app.tool_recovery_discard(),
            _ => {}
        }
        return;
    }

    // Standard normal mode keys
    match key.code {
        KeyCode::Char('q') => app.request_quit(),
        KeyCode::Char('i') => { app.enter_insert_mode(); app.clear_status(); }
        KeyCode::Char('a') => { app.enter_insert_mode_at_end(); app.clear_status(); }
        KeyCode::Char('o') => { app.enter_insert_mode_with_clear(); app.clear_status(); }
        KeyCode::Char(':') | KeyCode::Char('/') => app.enter_command_mode(),
        KeyCode::Char('k') | KeyCode::Up => app.scroll_up(),
        KeyCode::Char('j') => app.scroll_down(),
        KeyCode::Down | KeyCode::End => app.scroll_to_bottom(),
        KeyCode::Char('g') => app.scroll_to_top(),
        KeyCode::Char('G') => app.scroll_to_bottom(),
        _ => {}
    }
}
```

### Insert Mode Handler with Token Pattern

```rust
fn handle_insert_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.enter_normal_mode(),
        KeyCode::Enter => {
            // Acquire proof token before accessing insert mode
            let Some(token) = app.insert_token() else { return; };
            let queued = app.insert_mode(token).queue_message();
            if let Some(queued) = queued {
                app.start_streaming(queued);
            }
        }
        _ => {
            let Some(token) = app.insert_token() else { return; };
            let mut insert = app.insert_mode(token);

            match key.code {
                KeyCode::Backspace => insert.delete_char(),
                KeyCode::Delete => insert.delete_char_forward(),
                KeyCode::Left => insert.move_cursor_left(),
                KeyCode::Right => insert.move_cursor_right(),
                KeyCode::Home => insert.reset_cursor(),
                KeyCode::End => insert.move_cursor_end(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert.clear_line();
                }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert.delete_word_backwards();
                }
                KeyCode::Char(c) => insert.enter_char(c),
                _ => {}
            }
        }
    }
}
```

### Key Binding Summary

**Normal Mode:**

| Key | Action |
|-----|--------|
| `q` | Quit application |
| `i` | Enter insert mode |
| `a` | Enter insert mode at end |
| `o` | Enter insert mode with clear |
| `:` or `/` | Enter command mode |
| `k` / `Up` | Scroll up |
| `j` | Scroll down |
| `g` | Scroll to top |
| `G` / `Down` / `End` | Scroll to bottom |

**Insert Mode:**

| Key | Action |
|-----|--------|
| `Esc` | Return to normal mode |
| `Enter` | Send message |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start/end |
| `Ctrl+U` | Clear entire line |
| `Ctrl+W` | Delete word backwards |

**Command Mode:**

| Key | Action |
|-----|--------|
| `Esc` | Cancel and return to normal |
| `Enter` | Execute command |
| `Backspace` | Delete last character |

**Model Select Mode:**

| Key | Action |
|-----|--------|
| `Esc` | Cancel selection |
| `Enter` | Confirm selection |
| `Up` / `k` | Move up |
| `Down` / `j` | Move down |
| `1` / `2` | Direct selection |

**Tool Approval Mode:**

| Key | Action |
|-----|--------|
| `j` / `k` / `Up` / `Down` | Navigate |
| `Space` | Toggle selection |
| `a` | Approve all |
| `d` | Deny all |
| `Enter` | Confirm selected |
| `Esc` | Deny all |

---

## Theme System

The `theme.rs` module defines the visual styling for the entire TUI.

### Color Palette

```rust
pub mod colors {
    use ratatui::style::Color;

    // Primary brand colors
    pub const PRIMARY: Color = Color::Rgb(139, 92, 246);      // Purple (violet-500)
    pub const PRIMARY_DIM: Color = Color::Rgb(109, 72, 206); // Darker purple

    // Background colors
    pub const BG_DARK: Color = Color::Rgb(17, 17, 27);       // Near black
    pub const BG_PANEL: Color = Color::Rgb(30, 30, 46);      // Panel background
    pub const BG_HIGHLIGHT: Color = Color::Rgb(44, 46, 68);  // Row highlight

    // Text colors
    pub const TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244);   // Main text
    pub const TEXT_SECONDARY: Color = Color::Rgb(147, 153, 178); // Dimmed text
    pub const TEXT_MUTED: Color = Color::Rgb(88, 91, 112);       // Very dim

    // Accent colors
    pub const GREEN: Color = Color::Rgb(166, 227, 161);   // Success/user
    pub const YELLOW: Color = Color::Rgb(249, 226, 175);  // Warning
    pub const RED: Color = Color::Rgb(243, 139, 168);     // Error
    pub const PEACH: Color = Color::Rgb(250, 179, 135);   // Accent
    pub const CYAN: Color = Color::Rgb(137, 220, 235);    // Tools/links

    // Semantic aliases
    pub const ACCENT: Color = CYAN;
    pub const SUCCESS: Color = GREEN;
    pub const ERROR: Color = RED;
    pub const WARNING: Color = YELLOW;
}
```

### Pre-defined Styles

```rust
pub mod styles {
    use super::*;

    pub fn user_name() -> Style {
        Style::default()
            .fg(colors::GREEN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn assistant_name() -> Style {
        Style::default()
            .fg(colors::PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_normal() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::TEXT_SECONDARY)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_insert() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::GREEN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_command() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::YELLOW)
            .add_modifier(Modifier::BOLD)
    }

    pub fn key_hint() -> Style {
        Style::default().fg(colors::TEXT_MUTED)
    }

    pub fn key_highlight() -> Style {
        Style::default()
            .fg(colors::PEACH)
            .add_modifier(Modifier::BOLD)
    }
}
```

### Spinner Animation

```rust
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}
```

The spinner cycles through Braille dot patterns. With a 100ms event poll timeout, the animation runs at approximately 10 FPS.

---

## Markdown Rendering

The `markdown.rs` module converts markdown text to ratatui `Line` and `Span` types with caching for performance.

### Caching System

```rust
/// Maximum number of cached renders before eviction.
const CACHE_MAX_ENTRIES: usize = 128;

/// Cache key combining content hash and style.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    content_hash: u64,
    style_hash: u64,
}

thread_local! {
    static RENDER_CACHE: RefCell<HashMap<CacheKey, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}

pub fn clear_render_cache() {
    RENDER_CACHE.with(|cache| cache.borrow_mut().clear());
}
```

### Main Render Function

```rust
pub fn render_markdown(content: &str, base_style: Style) -> Vec<Line<'static>> {
    let key = CacheKey::new(content, base_style);

    // Check cache first
    let cached = RENDER_CACHE.with(|cache| cache.borrow().get(&key).cloned());
    if let Some(lines) = cached {
        return lines;
    }

    // Cache miss - render and store
    let renderer = MarkdownRenderer::new(base_style);
    let lines = renderer.render(content);

    RENDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();

        // Simple eviction: clear half when full
        if cache.len() >= CACHE_MAX_ENTRIES {
            let keys_to_remove: Vec<_> = cache.keys().take(CACHE_MAX_ENTRIES / 2).cloned().collect();
            for k in keys_to_remove {
                cache.remove(&k);
            }
        }

        cache.insert(key, lines.clone());
    });

    lines
}
```

### MarkdownRenderer State

```rust
struct MarkdownRenderer {
    base_style: Style,
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,

    // Style stack using counters for proper nesting
    bold_count: usize,
    italic_count: usize,
    code_count: usize,

    // Block state
    in_code_block: bool,
    code_block_content: Vec<String>,

    // Table state
    in_table: bool,
    table_rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    table_alignments: Vec<Alignment>,

    // List state
    list_stack: Vec<Option<u64>>,  // None = bullet, Some(n) = numbered starting at n
}
```

### Supported Markdown Features

| Feature | Rendering |
|---------|-----------|
| Headings | Bold text |
| Bold (`**text**`) | Modifier::BOLD |
| Italic (`*text*`) | Modifier::ITALIC |
| Inline code (`` `code` ``) | Peach color with backticks |
| Code blocks | Muted color with fence markers |
| Bullet lists | `•` prefix with indentation |
| Numbered lists | `1.` prefix with auto-increment |
| Tables | Box-drawing character borders |
| HTML/XML tags | Rendered as plain text (not stripped) |

### Nested Style Handling

Style counters allow proper nesting:

```rust
fn start_tag(&mut self, tag: Tag) {
    match tag {
        Tag::Heading { .. } => self.bold_count += 1,
        Tag::Strong => self.bold_count += 1,
        Tag::Emphasis => self.italic_count += 1,
        // ...
    }
}

fn end_tag(&mut self, tag: TagEnd) {
    match tag {
        TagEnd::Heading(_) => self.bold_count = self.bold_count.saturating_sub(1),
        TagEnd::Strong => self.bold_count = self.bold_count.saturating_sub(1),
        TagEnd::Emphasis => self.italic_count = self.italic_count.saturating_sub(1),
        // ...
    }
}

fn current_style(&self) -> Style {
    let mut style = self.base_style;
    if self.bold_count > 0 {
        style = style.add_modifier(Modifier::BOLD);
    }
    if self.italic_count > 0 {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}
```

This allows `# Heading with **bold**` to render correctly: "Heading with" is bold (from heading), "bold" is bold (from heading + strong), and trailing text is still bold (from heading after strong ends).

### Table Rendering

Tables are rendered with Unicode box-drawing characters:

```rust
fn render_table(&mut self) {
    // Calculate column widths
    let num_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; num_cols];

    for row in &self.table_rows {
        for (i, cell) in row.iter().enumerate() {
            col_widths[i] = col_widths[i].max(cell.trim().width());
        }
    }

    // Render borders with box-drawing characters
    // Top:    ┌───┬───┐
    // Header: │ A │ B │
    // Sep:    ├───┼───┤
    // Row:    │ 1 │ 2 │
    // Bottom: └───┴───┘
}
```

Example output:

```
    ┌───────┬───────┬───────┐
    │ Col A │ Col B │ Col C │
    ├───────┼───────┼───────┤
    │ 1     │ 2     │ 3     │
    │ 4     │ 5     │ 6     │
    └───────┴───────┴───────┘
```

### HTML/XML Content Handling

LLM responses often contain XML-like tags. These are rendered as plain text rather than being stripped:

```rust
Event::Html(html) | Event::InlineHtml(html) => self.handle_text(&html),
```

---

## Modal Animation Effects

The `effects.rs` module provides animation transforms for modal overlays.

### Effect Types

```rust
pub enum ModalEffectKind {
    PopScale,  // Scales from 60% to 100%
    SlideUp,   // Slides up from below
}
```

### Apply Effect Function

```rust
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect {
    match effect.kind() {
        ModalEffectKind::PopScale => {
            let t = ease_out_cubic(effect.progress());
            let scale = 0.6 + 0.4 * t;  // 60% to 100%
            scale_rect(base, scale)
        }
        ModalEffectKind::SlideUp => {
            let t = ease_out_cubic(effect.progress());
            let viewport_bottom = viewport.y.saturating_add(viewport.height);
            let base_bottom = base.y.saturating_add(base.height);
            let max_offset = viewport_bottom.saturating_sub(base_bottom);
            let offset = max_offset.min(base.height.saturating_div(2)).min(6);
            let y_offset = ((1.0 - t) * offset as f32).round() as u16;
            Rect {
                x: base.x,
                y: base.y.saturating_add(y_offset),
                width: base.width,
                height: base.height,
            }
        }
    }
}
```

### Scale Rectangle Helper

```rust
fn scale_rect(base: Rect, scale: f32) -> Rect {
    let width = ((base.width as f32) * scale).round() as u16;
    let height = ((base.height as f32) * scale).round() as u16;
    let width = width.max(1).min(base.width);
    let height = height.max(1).min(base.height);
    // Center the scaled rectangle within the base
    let x = base.x + (base.width.saturating_sub(width) / 2);
    let y = base.y + (base.height.saturating_sub(height) / 2);
    Rect { x, y, width, height }
}
```

### Easing Function

```rust
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}
```

The cubic ease-out provides smooth deceleration, making animations feel natural.

---

## Scrolling and Viewport Management

### Scroll State

The engine tracks scroll position with two states:

```rust
enum ScrollState {
    AutoBottom,                        // Follow new content automatically
    Manual { offset_from_top: u16 },   // User-controlled position
}
```

### Calculating Scroll Parameters

```rust
fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let messages_block = Block::default().borders(Borders::ALL).padding(Padding::horizontal(1));

    // Calculate dimensions
    let inner = messages_block.inner(area);
    let total_lines = wrapped_line_count(&lines, inner.width);
    let visible_height = inner.height;

    // Maximum scroll is how far we can scroll down
    let max_scroll = total_lines.saturating_sub(visible_height);
    app.update_scroll_max(max_scroll);

    // Get current scroll position
    let scroll_offset = app.scroll_offset_from_top();

    // Render with scroll offset
    let messages = Paragraph::new(lines)
        .block(messages_block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));

    frame.render_widget(messages, area);
}
```

### Wrapped Line Counting

```rust
fn wrapped_line_count(lines: &[Line], width: u16) -> u16 {
    let width = width.max(1) as usize;
    let mut total: u16 = 0;

    for line in lines {
        let line_width = line.width();  // Uses unicode_width
        let rows = if line_width == 0 {
            1  // Empty lines still take space
        } else {
            ((line_width - 1) / width) + 1  // Ceiling division
        };
        total = total.saturating_add(rows as u16);
    }

    total
}
```

### Scrollbar Rendering

```rust
// Only render scrollbar when content exceeds viewport
if max_scroll > 0 {
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"))
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .style(Style::default().fg(colors::TEXT_MUTED));

    // Key insight: content_length = max_scroll (scrollable range), not total_lines
    // This ensures the thumb reaches the bottom when fully scrolled
    let mut scrollbar_state = ScrollbarState::new(max_scroll as usize)
        .position(scroll_offset as usize);

    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin { vertical: 1, horizontal: 0 }),
        &mut scrollbar_state,
    );
}
```

---

## Widget Composition Patterns

### Block Decoration Pattern

Most components use a Block for borders and titles:

```rust
let widget = Paragraph::new(content).block(
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(colors::BG_PANEL))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(" Title ", title_style)]))
        .title_bottom(Line::from(footer).alignment(Alignment::Right)),
);
```

### Centered Overlay Pattern

```rust
fn draw_overlay(frame: &mut Frame, content_width: u16, content_height: u16) {
    let area = frame.area();

    // Calculate centered position
    let overlay_width = content_width.min(area.width.saturating_sub(4));
    let overlay_height = content_height;

    let overlay_area = Rect {
        x: area.x + (area.width.saturating_sub(overlay_width) / 2),
        y: area.y + (area.height.saturating_sub(overlay_height) / 2),
        width: overlay_width,
        height: overlay_height,
    };

    // Clear background and render
    frame.render_widget(Clear, overlay_area);
    frame.render_widget(content_widget, overlay_area);
}
```

### Dynamic Content Sizing Pattern

```rust
fn draw_dynamic_content(frame: &mut Frame, lines: &[Line]) {
    // Calculate required dimensions from content
    let content_width = lines.iter().map(|l| l.width()).max().unwrap_or(10) as u16;
    let content_width = content_width.min(frame.area().width.saturating_sub(4));
    let content_height = lines.len() as u16;

    // Add border/padding overhead
    let total_width = content_width.saturating_add(4);
    let total_height = content_height.saturating_add(4);

    // Position and render
    let rect = Rect { /* centered calculation */ };
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines.to_vec()).block(block), rect);
}
```

### Selection Highlighting Pattern

```rust
for (i, item) in items.iter().enumerate() {
    let is_selected = i == selected_index;

    let bg = if is_selected { Some(colors::BG_HIGHLIGHT) } else { None };
    let mut style = if is_selected {
        Style::default().fg(colors::TEXT_PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(colors::TEXT_SECONDARY)
    };

    if let Some(bg) = bg {
        style = style.bg(bg);
    }

    let prefix = if is_selected { "▸" } else { " " };
    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", prefix), style),
        Span::styled(item.label.clone(), style),
    ]));
}
```

---

## Extension Guide

### Adding a New Overlay

1. **Create the draw function in `lib.rs`:**

```rust
fn draw_my_overlay(frame: &mut Frame, app: &App) {
    // Build content
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(" Title ", Style::default().bold())));
    lines.push(Line::from(""));
    // ... add content lines

    // Calculate dimensions
    let content_width = lines.iter().map(|l| l.width()).max().unwrap_or(20) as u16;
    let content_width = content_width.min(frame.area().width.saturating_sub(4));
    let content_height = lines.len() as u16;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::PRIMARY))
        .style(Style::default().bg(colors::BG_PANEL))
        .padding(Padding::uniform(1));

    let height = content_height.saturating_add(4);
    let width = content_width.saturating_add(4);
    let area = frame.area();
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width) / 2),
        y: area.y + (area.height.saturating_sub(height) / 2),
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
```

2. **Add to main draw function:**

```rust
pub fn draw(frame: &mut Frame, app: &mut App) {
    // ... existing rendering

    // Add overlay check (order matters - later = on top)
    if app.should_show_my_overlay() {
        draw_my_overlay(frame, app);
    }
}
```

### Adding Animation to an Overlay

1. **Trigger animation when entering the mode (in engine crate):**

```rust
pub fn enter_my_mode(&mut self) {
    self.input = std::mem::take(&mut self.input).into_my_mode();
    self.modal_effect = Some(ModalEffect::pop_scale(Duration::from_millis(300)));
}
```

2. **Apply effect in draw function:**

```rust
fn draw_my_animated_overlay(frame: &mut Frame, app: &mut App) {
    let base_area = Rect { /* calculate base position */ };

    // Advance animation and transform area
    let elapsed = app.frame_elapsed();
    let (overlay_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, frame.area()),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    frame.render_widget(Clear, overlay_area);
    // ... render content at overlay_area
}
```

### Adding a New Input Mode Handler

1. **Add handler function in `input.rs`:**

```rust
fn handle_my_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.enter_normal_mode(),
        KeyCode::Enter => {
            // Action on confirm
            let Some(token) = app.my_mode_token() else { return };
            let result = app.my_mode(token).confirm();
            // ... handle result
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(token) = app.my_mode_token() {
                app.my_mode(token).move_up();
            }
        }
        // ... other keys
        _ => {}
    }
}
```

2. **Add to dispatch in `handle_events`:**

```rust
match app.input_mode() {
    // ... existing modes
    InputMode::MyMode => handle_my_mode(app, key),
}
```

### Adding New Colors

In `theme.rs`:

```rust
pub mod colors {
    // ... existing colors

    // Add semantic colors
    pub const INFO: Color = Color::Rgb(137, 180, 250);      // Blue
    pub const HIGHLIGHT: Color = Color::Rgb(180, 190, 254); // Light blue

    // Or specific-purpose colors
    pub const TOOL_RUNNING: Color = Color::Rgb(137, 220, 235);
    pub const TOOL_COMPLETE: Color = Color::Rgb(166, 227, 161);
}
```

### Adding New Styles

In `theme.rs`:

```rust
pub mod styles {
    // ... existing styles

    pub fn info_badge() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::INFO)
            .add_modifier(Modifier::BOLD)
    }

    pub fn muted_italic() -> Style {
        Style::default()
            .fg(colors::TEXT_MUTED)
            .add_modifier(Modifier::ITALIC)
    }
}
```

### Extending Markdown Rendering

To add support for new markdown elements:

1. **Add state tracking in `MarkdownRenderer`:**

```rust
struct MarkdownRenderer {
    // ... existing fields
    in_blockquote: bool,
    blockquote_depth: usize,
}
```

2. **Handle start/end tags:**

```rust
fn start_tag(&mut self, tag: Tag) {
    match tag {
        // ... existing matches
        Tag::BlockQuote(_) => {
            self.flush_line();
            self.blockquote_depth += 1;
        }
    }
}

fn end_tag(&mut self, tag: TagEnd) {
    match tag {
        // ... existing matches
        TagEnd::BlockQuote => {
            self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
        }
    }
}
```

3. **Apply styling in text handling:**

```rust
fn handle_text(&mut self, text: &str) {
    let style = if self.blockquote_depth > 0 {
        self.base_style.fg(colors::TEXT_MUTED).add_modifier(Modifier::ITALIC)
    } else {
        self.current_style()
    };

    let prefix = if self.blockquote_depth > 0 {
        "│ ".repeat(self.blockquote_depth)
    } else {
        String::new()
    };

    self.current_spans.push(Span::styled(format!("{}{}", prefix, text), style));
}
```

### Adding New Animation Effect Types

1. **Add variant to `ModalEffectKind` (in engine crate):**

```rust
pub enum ModalEffectKind {
    PopScale,
    SlideUp,
    FadeIn,  // New
}
```

2. **Implement transform in `effects.rs`:**

```rust
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect {
    match effect.kind() {
        // ... existing matches
        ModalEffectKind::FadeIn => {
            // For fade effects, return the base rect unchanged
            // The actual fade is handled via style alpha if supported
            base
        }
    }
}
```

Note: True alpha-based fading requires terminal support. Most implementations use alternative visual cues like border style changes.

---

## Performance Considerations

### Markdown Cache Management

The markdown cache holds up to 128 entries before evicting half. Call `clear_render_cache()` when:

- Switching themes (if implemented)
- Memory pressure is detected
- Major context changes occur

### Wrapped Line Count Caching

The `wrapped_line_count` function is called every frame. For very long conversations, consider caching this calculation and invalidating on:

- New message added
- Terminal resize
- Scroll position change

### Avoid Recomputing in Render Loop

Expensive computations should be done in `App::tick()` or cached:

```rust
// Bad: Recomputes every frame
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let usage = app.compute_context_usage();  // Expensive!
    // ...
}

// Good: Use cached value
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let usage = app.context_usage_status();  // Returns cached value
    // ...
}
```

---

## Testing

The `markdown.rs` module includes unit tests for rendering correctness:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_simple_text() { ... }

    #[test]
    fn test_table() { ... }

    #[test]
    fn test_cache_returns_same_result() { ... }

    #[test]
    fn test_nested_bold_in_heading() { ... }

    #[test]
    fn test_html_xml_content_rendered() { ... }
}
```

Run tests with:

```bash
cargo test -p forge-tui
```

---

## Summary

The `forge-tui` crate provides a complete terminal UI implementation with:

- **Dual rendering modes**: Full-screen and inline
- **Rich text rendering**: Markdown with tables, code blocks, and proper styling
- **Vim-style input**: Modal editing with Normal, Insert, Command, ModelSelect modes
- **Animated overlays**: Pop-scale and slide-up effects for modal dialogs
- **Performance optimizations**: Cached markdown rendering, efficient scroll calculations
- **Extensible design**: Clear patterns for adding overlays, modes, styles, and animations

The architecture separates concerns cleanly, with rendering (`lib.rs`, `ui_inline.rs`), input handling (`input.rs`), styling (`theme.rs`), markdown conversion (`markdown.rs`), and animation effects (`effects.rs`) each in dedicated modules.

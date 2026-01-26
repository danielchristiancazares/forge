//! TUI rendering for Forge using ratatui.

mod diff_render;
mod effects;
mod input;
pub mod markdown;
mod shared;
mod theme;
mod tool_display;
mod tool_result_summary;
mod ui_inline;

pub use effects::apply_modal_effect;
pub use input::handle_events;
pub use theme::{Glyphs, Palette, glyphs, palette, spinner_frame, styles};
pub use ui_inline::{
    INLINE_INPUT_HEIGHT, INLINE_VIEWPORT_HEIGHT, InlineOutput, clear_inline_viewport,
    draw as draw_inline, inline_viewport_height,
};

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use forge_engine::{
    App, ChangeKind, ContextUsageStatus, DisplayItem, FileDiff, InputMode, Message,
    PredefinedModel, Provider, UiOptions, command_specs,
};
use forge_types::{ToolResult, sanitize_terminal_text};

use self::diff_render::render_tool_result_lines;
pub use self::markdown::clear_render_cache;
use self::markdown::render_markdown;
use self::shared::{
    ToolCallStatus, ToolCallStatusKind, collect_approval_view, collect_tool_statuses,
    message_header_parts, provider_color, wrapped_line_count_exact, wrapped_line_rows,
};
use self::tool_result_summary::{ToolCallMeta, ToolResultRender, tool_result_render_decision};

/// Cache for rendered message lines to avoid rebuilding every frame.
/// Stores static (history/local) content keyed by display + UI options.
#[derive(Default)]
struct MessageLinesCache {
    /// Cache is valid only when key matches.
    key: MessageCacheKey,
    lines: Vec<Line<'static>>,
    total_rows: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct MessageCacheKey {
    display_version: usize,
    width: u16,
    ascii_only: bool,
    high_contrast: bool,
    reduced_motion: bool,
}

impl MessageLinesCache {
    fn get(&self, key: MessageCacheKey) -> Option<(&[Line<'static>], usize)> {
        if self.key == key && !self.lines.is_empty() {
            Some((&self.lines, self.total_rows))
        } else {
            None
        }
    }

    fn set(&mut self, key: MessageCacheKey, lines: Vec<Line<'static>>, total_rows: usize) {
        self.key = key;
        self.lines = lines;
        self.total_rows = total_rows;
    }

    fn invalidate(&mut self) {
        self.lines.clear();
        self.total_rows = 0;
    }
}

impl MessageCacheKey {
    fn new(display_version: usize, width: u16, options: UiOptions) -> Self {
        Self {
            display_version,
            width,
            ascii_only: options.ascii_only,
            high_contrast: options.high_contrast,
            reduced_motion: options.reduced_motion,
        }
    }
}

thread_local! {
    static MESSAGE_CACHE: RefCell<MessageLinesCache> = RefCell::new(MessageLinesCache::default());
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let options = app.ui_options();
    let palette = palette(options);
    let glyphs = glyphs(options);
    let bg_block = Block::default().style(Style::default().bg(palette.bg_dark));
    frame.render_widget(bg_block, frame.area());

    let input_height = match app.input_mode() {
        InputMode::Normal => 3,
        _ => 5,
    };

    let elapsed = app.frame_elapsed();

    // Panel width depends on expansion state: 35 chars collapsed, 50% expanded
    let panel_constraint = if app.files_panel_expanded() {
        Constraint::Percentage(50)
    } else {
        Constraint::Length(35)
    };
    let panel_layout = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints([Constraint::Min(40), panel_constraint])
        .split(frame.area());
    let base_panel_area = panel_layout[1];
    let full_main_area = frame.area().inner(Margin::new(1, 1));

    let mut files_panel_area = None;
    if let Some(effect) = app.files_panel_effect_mut() {
        effect.advance(elapsed);
        let animated = effects::apply_files_panel_effect(effect, base_panel_area);
        if animated.width > 0 && animated.height > 0 {
            files_panel_area = Some(animated);
        }
        if effect.is_finished() {
            app.finish_files_panel_effect();
        }
    } else if app.files_panel_visible() {
        files_panel_area = Some(base_panel_area);
    }

    let main_area = if let Some(panel_area) = files_panel_area {
        let width = panel_area.x.saturating_sub(full_main_area.x).max(1);
        Rect {
            x: full_main_area.x,
            y: full_main_area.y,
            width,
            height: full_main_area.height,
        }
    } else {
        full_main_area
    };

    // Vertical split within main area for messages + input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(input_height)])
        .split(main_area);

    draw_messages(frame, app, chunks[0], &palette, &glyphs);
    draw_input(frame, app, chunks[1], &palette, &glyphs, false);

    if let Some(panel_area) = files_panel_area {
        draw_files_panel(frame, app, panel_area, &palette, &glyphs);
    }

    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app, &palette);
    }

    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app, &palette, &glyphs, elapsed);
    }

    if app.tool_approval_requests().is_some() {
        draw_tool_approval_prompt(frame, app, &palette);
    }

    if app.tool_recovery_calls().is_some() {
        draw_tool_recovery_prompt(frame, app, &palette, &glyphs);
    }
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect, palette: &Palette, glyphs: &Glyphs) {
    let messages_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.text_muted))
        .padding(Padding::horizontal(1));

    if app.is_empty() {
        app.update_scroll_max(0);
        MESSAGE_CACHE.with(|cache| cache.borrow_mut().invalidate());
        let welcome = create_welcome_screen(app, palette, glyphs);
        frame.render_widget(welcome.block(messages_block), area);
        return;
    }

    let inner = messages_block.inner(area);
    let display_version = app.display_version();
    let options = app.ui_options();
    let cache_width = inner.width.max(1);
    let cache_key = MessageCacheKey::new(display_version, cache_width, options);

    let tool_statuses = collect_tool_statuses(app, 80);
    let is_streaming = app.streaming().is_some();
    let has_tool_activity = tool_statuses.is_some();
    let has_dynamic = is_streaming || has_tool_activity;
    let static_message_count = app.display_items().len();

    let (mut lines, mut total_rows) = MESSAGE_CACHE.with(|cache| {
        let cache_ref = cache.borrow();
        if let Some((cached_lines, cached_total)) = cache_ref.get(cache_key) {
            // Cache hit - clone the cached data
            let lines = cached_lines.to_vec();
            return (lines, cached_total);
        }
        drop(cache_ref);

        let (lines, total_rows) = build_message_lines(app, palette, glyphs, cache_width);

        cache.borrow_mut().set(cache_key, lines.clone(), total_rows);

        (lines, total_rows)
    });

    if has_dynamic {
        let (dynamic_lines, dynamic_total) = build_dynamic_message_lines(
            app,
            palette,
            glyphs,
            cache_width,
            static_message_count,
            tool_statuses.as_deref(),
        );
        if !dynamic_lines.is_empty() {
            lines.extend(dynamic_lines);
            total_rows = total_rows.saturating_add(dynamic_total);
        }
    }

    if !lines.is_empty() {
        lines.push(Line::from(""));
        total_rows = total_rows.saturating_add(1);
    }

    // Handle u16 overflow for very long conversations
    let max_rows = u16::MAX as usize;

    // Ratatui scroll offsets are u16; trim oldest rows if content exceeds that range.
    if total_rows > max_rows {
        let line_rows = wrapped_line_rows(&lines, cache_width);
        let mut drop_count = 0;
        let mut trimmed_rows = total_rows;
        while trimmed_rows > max_rows && drop_count < line_rows.len() {
            trimmed_rows = trimmed_rows.saturating_sub(line_rows[drop_count]);
            drop_count += 1;
        }

        if drop_count > 0 {
            lines.drain(0..drop_count);
        }
        total_rows = trimmed_rows;
    }

    let visible_height = inner.height as usize;
    let max_scroll = total_rows.saturating_sub(visible_height).min(max_rows) as u16;
    app.update_scroll_max(max_scroll);
    let scroll_offset = app.scroll_offset_from_top();

    let messages = Paragraph::new(lines)
        .block(messages_block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));

    frame.render_widget(messages, area);

    if max_scroll > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some(glyphs.arrow_up))
            .end_symbol(Some(glyphs.arrow_down))
            .track_symbol(Some(glyphs.track))
            .thumb_symbol(glyphs.thumb)
            .style(Style::default().fg(palette.text_muted));

        // content_length = scrollable range (max_scroll), not total_lines
        // This ensures thumb is at bottom when scroll_offset == max_scroll
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll as usize).position(scroll_offset as usize);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// Build message lines for static content only (no streaming, no tool status).
/// Used for caching across frames; dynamic sections are appended separately.
fn build_message_lines(
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
    width: u16,
) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    let mut msg_count = 0;
    let mut tool_calls: HashMap<&str, ToolCallMeta> = HashMap::new();

    for item in app.display_items() {
        let msg = match item {
            DisplayItem::History(id) => app.history().get_entry(*id).message(),
            DisplayItem::Local(msg) => msg,
        };
        match msg {
            Message::ToolUse(call) => {
                tool_calls.insert(call.id.as_str(), ToolCallMeta::from_call(call));
                render_message_static(msg, &mut lines, &mut msg_count, palette, glyphs, None);
            }
            Message::ToolResult(result) => {
                let meta = tool_calls.get(result.tool_call_id.as_str());
                render_message_static(msg, &mut lines, &mut msg_count, palette, glyphs, meta);
            }
            _ => render_message_static(msg, &mut lines, &mut msg_count, palette, glyphs, None),
        }
    }

    let total_rows = wrapped_line_count_exact(&lines, width);

    (lines, total_rows)
}

/// Build message lines for dynamic content (streaming, tool status).
/// Static history/local content is appended separately from cache.
fn build_dynamic_message_lines(
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
    width: u16,
    static_message_count: usize,
    tool_statuses: Option<&[ToolCallStatus]>,
) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    let has_static = static_message_count > 0;

    if let Some(streaming) = app.streaming() {
        if has_static {
            lines.push(Line::from(""));
        }

        let icon = glyphs.assistant;
        let provider = streaming.provider();
        let color = provider_color(provider, palette);
        let name_style = Style::default().fg(color);

        let show_thinking = app.ui_options().show_thinking;
        let has_thinking = show_thinking
            && matches!(provider, Provider::Claude | Provider::Gemini)
            && !streaming.thinking().is_empty();
        let is_empty = streaming.content().is_empty();
        let indent = "   ";

        if has_thinking {
            let header_tail = if is_empty {
                " Thinking..."
            } else {
                " Thinking"
            };

            let mut header_spans = vec![Span::styled(format!(" {icon} "), name_style)];
            if is_empty {
                let spinner = spinner_frame(app.tick_count(), app.ui_options());
                header_spans.push(Span::styled(spinner, Style::default().fg(palette.primary)));
            }
            header_spans.push(Span::styled(
                header_tail,
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::ITALIC),
            ));
            lines.push(Line::from(header_spans));

            let thinking_style = Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::ITALIC);
            let thinking = sanitize_terminal_text(streaming.thinking());
            let mut rendered_thinking = render_markdown(thinking.as_ref(), thinking_style, palette);

            if !rendered_thinking.is_empty() {
                let first_line = &mut rendered_thinking[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }
                for line in &mut rendered_thinking {
                    line.spans.insert(0, Span::raw(indent));
                }
                lines.extend(rendered_thinking);
            }

            if !is_empty {
                lines.push(Line::from(""));
            }
        }

        if is_empty {
            if !has_thinking {
                let spinner = spinner_frame(app.tick_count(), app.ui_options());
                lines.push(Line::from(vec![
                    Span::styled(format!(" {icon} "), name_style),
                    Span::styled(spinner, Style::default().fg(palette.primary)),
                    Span::styled(" Thinking...", Style::default().fg(palette.text_muted)),
                ]));
            }
        } else {
            let content_style = Style::default().fg(palette.text_secondary);
            let content = sanitize_terminal_text(streaming.content());
            let mut rendered = render_markdown(content.as_ref(), content_style, palette);

            if rendered.is_empty() {
                if has_thinking {
                    lines.push(Line::from(Span::raw(indent)));
                } else {
                    lines.push(Line::from(vec![Span::styled(
                        format!(" {icon} "),
                        name_style,
                    )]));
                }
            } else {
                let spinner = spinner_frame(app.tick_count(), app.ui_options());
                let first_line = &mut rendered[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }

                if has_thinking {
                    for line in &mut rendered {
                        line.spans.insert(0, Span::raw(indent));
                    }
                } else {
                    first_line
                        .spans
                        .insert(0, Span::styled(format!(" {icon} "), name_style));
                }

                let first_line = &mut rendered[0];
                first_line.spans.push(Span::styled(
                    format!(" {spinner}"),
                    Style::default().fg(palette.text_muted),
                ));
                lines.extend(rendered);
            }
        }
    }

    if let Some(statuses) = tool_statuses {
        if has_static || app.streaming().is_some() {
            lines.push(Line::from(""));
        }
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        let approval_pending = app.tool_approval_requests().is_some();
        let header = if approval_pending {
            format!("{spinner} Tool approval required")
        } else {
            format!("{spinner} Tool execution")
        };
        lines.push(Line::from(Span::styled(
            header,
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::ITALIC),
        )));

        for status in statuses {
            let (icon, style, label) = match status.status {
                ToolCallStatusKind::Denied => (
                    glyphs.denied,
                    Style::default()
                        .fg(palette.warning)
                        .add_modifier(Modifier::BOLD),
                    "denied",
                ),
                ToolCallStatusKind::Error => (
                    glyphs.tool_result_err,
                    Style::default()
                        .fg(palette.error)
                        .add_modifier(Modifier::BOLD),
                    "error",
                ),
                ToolCallStatusKind::Ok => (
                    glyphs.tool_result_ok,
                    Style::default()
                        .fg(palette.success)
                        .add_modifier(Modifier::BOLD),
                    "ok",
                ),
                ToolCallStatusKind::Running => (
                    spinner,
                    Style::default()
                        .fg(palette.primary)
                        .add_modifier(Modifier::BOLD),
                    "running",
                ),
                ToolCallStatusKind::Approval => (
                    glyphs.paused,
                    Style::default()
                        .fg(palette.warning)
                        .add_modifier(Modifier::BOLD),
                    "paused",
                ),
                ToolCallStatusKind::Pending => (
                    glyphs.bullet,
                    Style::default().fg(palette.text_muted),
                    "pending",
                ),
            };

            let name = sanitize_terminal_text(&status.name);
            let id = sanitize_terminal_text(&status.id);
            lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), style),
                Span::styled(
                    format!("{} ({}) [{label}]", name.as_ref(), id.as_ref()),
                    Style::default().fg(palette.text_muted),
                ),
            ]));

            if let Some(reason) = status.reason.as_ref() {
                lines.push(Line::from(Span::styled(
                    format!("    ↳ {reason}"),
                    Style::default().fg(palette.text_muted),
                )));
            }
        }

        if let Some(output_lines) = app.tool_loop_output_lines()
            && !output_lines.is_empty()
        {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Tool output:",
                Style::default().fg(palette.text_muted),
            )));
            for line in output_lines {
                let safe_line = sanitize_terminal_text(line);
                lines.push(Line::from(Span::styled(
                    format!("    {}", safe_line.as_ref()),
                    Style::default().fg(palette.text_secondary),
                )));
            }
        }
    }

    let total_rows = wrapped_line_count_exact(&lines, width);

    (lines, total_rows)
}

/// Render a single message to lines (static helper for both cached and uncached paths).
fn render_message_static(
    msg: &Message,
    lines: &mut Vec<Line<'static>>,
    msg_count: &mut usize,
    palette: &Palette,
    glyphs: &Glyphs,
    tool_call_meta: Option<&ToolCallMeta>,
) {
    let is_tool_result = matches!(msg, Message::ToolResult(_));
    if *msg_count > 0 && !is_tool_result {
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    // Message header with role icon and name
    let (icon, name, name_style) = message_header_parts(msg, palette, glyphs);

    match msg {
        Message::User(_) => {
            let content_style = Style::default().fg(palette.text_primary);
            let content = sanitize_terminal_text(msg.content());
            let mut rendered = render_markdown(content.as_ref(), content_style, palette);

            if rendered.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} "),
                    name_style,
                )]));
            } else {
                let first_line = &mut rendered[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }
                first_line
                    .spans
                    .insert(0, Span::styled(format!(" {icon} "), name_style));
                lines.extend(rendered);
            }
        }
        Message::ToolUse(_) => {
            // Tool call: icon + compact name (args already in name)
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), name_style),
                Span::styled(name, name_style),
            ]));
        }
        Message::ToolResult(result) => {
            let content = sanitize_terminal_text(&result.content);

            match tool_result_render_decision(tool_call_meta, &content, result.is_error, 80) {
                ToolResultRender::Full { diff_aware } => {
                    let content_style = if result.is_error {
                        Style::default().fg(palette.error)
                    } else {
                        Style::default().fg(palette.text_secondary)
                    };
                    if diff_aware {
                        lines.extend(render_tool_result_lines(
                            content.as_ref(),
                            content_style,
                            palette,
                            "  ",
                        ));
                    } else {
                        for line in content.lines() {
                            lines.push(Line::from(vec![
                                Span::raw("  "),
                                Span::styled(line.to_string(), content_style),
                            ]));
                        }
                    }
                }
                ToolResultRender::Summary(summary) => {
                    let style = if result.is_error {
                        Style::default().fg(palette.error)
                    } else {
                        Style::default().fg(palette.text_muted)
                    };
                    let connector = glyphs.tree_connector;
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {connector} "), style),
                        Span::styled(summary, style),
                    ]));
                }
            }
        }
        Message::System(_) | Message::Assistant(_) => {
            let content_style = match msg {
                Message::Assistant(_) => Style::default().fg(palette.text_secondary),
                _ => Style::default().fg(palette.text_muted),
            };
            let content = sanitize_terminal_text(msg.content());
            let mut rendered = render_markdown(content.as_ref(), content_style, palette);

            if rendered.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} "),
                    name_style,
                )]));
            } else {
                let first_line = &mut rendered[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }
                first_line
                    .spans
                    .insert(0, Span::styled(format!(" {icon} "), name_style));
                lines.extend(rendered);
            }
        }
    }
}

pub(crate) fn draw_input(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    glyphs: &Glyphs,
    inline_mode: bool,
) {
    let mode = app.input_mode();

    // In inline mode, ModelSelect transforms the input area entirely
    if inline_mode && mode == InputMode::ModelSelect {
        draw_inline_model_selector(frame, app, area, palette, glyphs);
        return;
    }

    let options = app.ui_options();
    // Clone command text to avoid borrow conflict with mutable context_usage_status()
    let (command_line, command_cursor_byte_index) = if mode == InputMode::Command {
        (
            app.command_text().map(str::to_string),
            app.command_cursor_byte_index(),
        )
    } else {
        (None, None)
    };

    let multiline = mode == InputMode::Insert && app.draft_text().contains('\n');
    let prompt_char = if mode == InputMode::Insert {
        if options.ascii_only { ">" } else { "❯" }
    } else {
        ""
    };

    let (mode_label, mode_style, border_style) = match mode {
        InputMode::Normal | InputMode::ModelSelect => (
            "NORMAL",
            styles::mode_normal(palette),
            Style::default().fg(palette.text_muted),
        ),
        InputMode::Insert => (
            "INSERT",
            styles::mode_insert(palette),
            Style::default().fg(palette.green),
        ),
        InputMode::Command => (
            "COMMAND",
            styles::mode_command(palette),
            Style::default().fg(palette.yellow),
        ),
    };
    let mode_text = if multiline {
        format!(" {mode_label} · MULTI ")
    } else {
        format!(" {mode_label} ")
    };

    // Key hints based on mode
    let hints = match mode {
        InputMode::Normal => vec![
            Span::styled("i", styles::key_highlight(palette)),
            Span::styled(" insert  ", styles::key_hint(palette)),
            Span::styled("/", styles::key_highlight(palette)),
            Span::styled(" command  ", styles::key_hint(palette)),
            Span::styled("f", styles::key_highlight(palette)),
            Span::styled(" files  ", styles::key_hint(palette)),
            Span::styled("PgUp/PgDn", styles::key_highlight(palette)),
            Span::styled(" scroll  ", styles::key_hint(palette)),
            Span::styled("q", styles::key_highlight(palette)),
            Span::styled(" quit ", styles::key_hint(palette)),
        ],
        InputMode::Insert => vec![
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" send  ", styles::key_hint(palette)),
            Span::styled("Ctrl+Enter/Shift+Enter", styles::key_highlight(palette)),
            Span::styled(" newline  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" normal ", styles::key_hint(palette)),
        ],
        InputMode::Command => vec![
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" execute  ", styles::key_hint(palette)),
            Span::styled("Tab", styles::key_highlight(palette)),
            Span::styled(" complete  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" cancel ", styles::key_hint(palette)),
        ],
        InputMode::ModelSelect => vec![
            Span::styled("↑↓", styles::key_highlight(palette)),
            Span::styled(" select  ", styles::key_hint(palette)),
            Span::styled("1-9", styles::key_highlight(palette)),
            Span::styled(" quick pick  ", styles::key_hint(palette)),
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" confirm  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" cancel ", styles::key_hint(palette)),
        ],
    };

    let usage_status = app.context_usage_status();
    // 0 = ready, 1 = needs summarization, 2 = recent messages too large (unrecoverable)
    let (usage, severity_override) = match &usage_status {
        ContextUsageStatus::Ready(usage) => (usage, 0),
        ContextUsageStatus::NeedsSummarization { usage, .. } => (usage, 1),
        ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => (usage, 2),
    };
    let pct = usage.percentage();
    let remaining = (100.0 - pct).clamp(0.0, 100.0);
    let base_usage = format!("Context {remaining:.0}% left");
    let usage_str = match severity_override {
        2 => format!("{base_usage} !!"), // Double bang for unrecoverable
        1 => format!("{base_usage} !"),
        _ => base_usage,
    };
    let usage_color = match severity_override {
        1 | 2 => palette.red,
        _ => match usage.severity() {
            0 => palette.green,  // < 70%
            1 => palette.yellow, // 70-90%
            _ => palette.red,    // > 90%
        },
    };

    let padding_v: u16 = match mode {
        InputMode::Normal | InputMode::ModelSelect => 0,
        InputMode::Insert if multiline => 0,
        _ => 1,
    };
    let input_padding = Padding::vertical(padding_v);
    let inner_height = area
        .height
        .saturating_sub(2 + padding_v.saturating_mul(2))
        .max(1);

    let prefix = match mode {
        InputMode::Command => " / ".to_string(),
        _ => format!(" {prompt_char} "),
    };
    let prefix_width = prefix.width() as u16;
    let content_width = area
        .width
        .saturating_sub(2)
        .saturating_sub(prefix_width)
        .max(1) as usize;

    let mut cursor_pos: Option<(u16, u16)> = None;
    let input_lines: Vec<Line> = if mode == InputMode::Insert && multiline {
        let draft = app.draft_text();
        let cursor_index = app.draft_cursor_byte_index();
        let before_cursor = &draft[..cursor_index];
        let cursor_line_index = before_cursor.matches('\n').count();
        let cursor_line_start = before_cursor.rsplit('\n').next().unwrap_or("");
        let cursor_display_pos = cursor_line_start.width();

        let raw_lines: Vec<&str> = draft.split('\n').collect();
        let visible_lines = inner_height as usize;
        let start_line = (cursor_line_index + 1).saturating_sub(visible_lines);
        let end_line = (start_line + visible_lines).min(raw_lines.len());

        let mut display_lines = Vec::new();
        let mut horizontal_scroll: u16 = 0;

        for (idx, line) in raw_lines[start_line..end_line].iter().enumerate() {
            let is_cursor_line = start_line + idx == cursor_line_index;
            let mut line_text = (*line).to_string();
            if is_cursor_line && cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
                let mut byte_offset = 0;
                let mut skipped_width = 0;
                for (i, grapheme) in line.grapheme_indices(true) {
                    if skipped_width >= scroll_target {
                        byte_offset = i;
                        break;
                    }
                    skipped_width += grapheme.width();
                }
                line_text = line[byte_offset..].to_string();
                horizontal_scroll = skipped_width as u16;
            }

            let prefix_text = if idx == 0 {
                prefix.clone()
            } else {
                " ".repeat(prefix_width as usize)
            };
            let prefix_style = if mode == InputMode::Command {
                Style::default().fg(palette.yellow)
            } else {
                Style::default().fg(palette.primary)
            };
            display_lines.push(Line::from(vec![
                Span::styled(prefix_text, prefix_style),
                Span::styled(line_text, Style::default().fg(palette.text_primary)),
            ]));
        }

        let cursor_row = cursor_line_index.saturating_sub(start_line) as u16;
        let cursor_x = area
            .x
            .saturating_add(1 + prefix_width)
            .saturating_add(cursor_display_pos as u16)
            .saturating_sub(horizontal_scroll);
        let cursor_y = area
            .y
            .saturating_add(1 + padding_v)
            .saturating_add(cursor_row);
        cursor_pos = Some((cursor_x, cursor_y));

        display_lines
    } else {
        let (display_text, horizontal_scroll) = if mode == InputMode::Insert {
            let cursor_index = app.draft_cursor_byte_index();
            let draft = app.draft_text();
            let text_before_cursor = &draft[..cursor_index];
            let cursor_display_pos = text_before_cursor.width();

            if cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
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
        } else if mode == InputMode::Command
            && let Some(cmd) = &command_line
        {
            let cursor_byte_index = command_cursor_byte_index
                .unwrap_or(cmd.len())
                .min(cmd.len());
            let text_before_cursor = &cmd[..cursor_byte_index];
            let cursor_display_pos = text_before_cursor.width();
            if cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
                let mut byte_offset = 0;
                let mut skipped_width = 0;
                for (idx, grapheme) in cmd.grapheme_indices(true) {
                    if skipped_width >= scroll_target {
                        byte_offset = idx;
                        break;
                    }
                    skipped_width += grapheme.width();
                }
                (cmd[byte_offset..].to_string(), skipped_width as u16)
            } else {
                (cmd.clone(), 0u16)
            }
        } else {
            (
                match mode {
                    InputMode::Insert | InputMode::Normal | InputMode::ModelSelect => {
                        app.draft_text().to_string()
                    }
                    InputMode::Command => command_line.clone().unwrap_or_default(),
                },
                0u16,
            )
        };

        let prefix_style = if mode == InputMode::Command {
            Style::default().fg(palette.yellow)
        } else {
            Style::default().fg(palette.primary)
        };
        let spans = vec![
            Span::styled(prefix, prefix_style),
            Span::styled(display_text, Style::default().fg(palette.text_primary)),
        ];

        if mode == InputMode::Insert {
            let cursor_index = app.draft_cursor_byte_index();
            let text_before_cursor = &app.draft_text()[..cursor_index];
            let cursor_display_pos = text_before_cursor.width() as u16;
            let cursor_x = area
                .x
                .saturating_add(1 + prefix_width)
                .saturating_add(cursor_display_pos)
                .saturating_sub(horizontal_scroll);
            let cursor_y = area.y.saturating_add(1 + padding_v);
            cursor_pos = Some((cursor_x, cursor_y));
        } else if mode == InputMode::Command
            && let Some(command_line) = command_line.as_ref()
        {
            let cursor_byte_index = command_cursor_byte_index
                .unwrap_or(command_line.len())
                .min(command_line.len());
            let text_before_cursor = &command_line[..cursor_byte_index];
            let cursor_display_pos = text_before_cursor.width() as u16;
            let cursor_x = area
                .x
                .saturating_add(1 + prefix_width)
                .saturating_add(cursor_display_pos)
                .saturating_sub(horizontal_scroll);
            let cursor_y = area.y.saturating_add(1 + padding_v);
            cursor_pos = Some((cursor_x, cursor_y));
        }

        vec![Line::from(spans)]
    };

    let (model_text, model_style) = if app.is_loading() {
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        (
            format!("{spinner} {}", app.model()),
            Style::default().fg(palette.primary),
        )
    } else if app.current_api_key().is_some() {
        (
            format!("{} {}", glyphs.status_ready, app.model()),
            Style::default().fg(palette.success),
        )
    } else {
        (
            format!("{} No API key", glyphs.status_missing),
            Style::default().fg(palette.error),
        )
    };

    let input = Paragraph::new(input_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(Line::from(vec![Span::styled(mode_text, mode_style)]))
            .title_top(Line::from(hints).alignment(Alignment::Right))
            .title_bottom(Line::from(vec![Span::styled(model_text, model_style)]))
            .title_bottom(
                Line::from(vec![Span::styled(
                    usage_str,
                    Style::default().fg(usage_color),
                )])
                .alignment(Alignment::Right),
            )
            .padding(input_padding),
    );

    frame.render_widget(input, area);

    if let Some((cursor_x, cursor_y)) = cursor_pos {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_command_palette(frame: &mut Frame, app: &App, palette: &Palette) {
    let area = frame.area();
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 14;

    let palette_area = Rect {
        x: area.x + (area.width.saturating_sub(palette_width) / 2),
        y: area.y + (area.height / 3),
        width: palette_width,
        height: palette_height,
    };

    frame.render_widget(Clear, palette_area);

    let filter_raw = app.command_text().unwrap_or("").trim();
    let filter = filter_raw.trim_start_matches('/').to_ascii_lowercase();

    let commands = command_specs();

    let filtered: Vec<_> = if filter.is_empty() {
        commands.iter().collect()
    } else {
        commands
            .iter()
            .filter(|spec| {
                spec.palette_label.to_ascii_lowercase().contains(&filter)
                    || spec.description.to_ascii_lowercase().contains(&filter)
            })
            .collect()
    };

    let mut lines: Vec<Line> = vec![Line::from("")];
    let filter_line = if filter.is_empty() {
        "  Type to filter commands..."
    } else {
        "  Filter active"
    };
    lines.push(Line::from(Span::styled(
        filter_line,
        Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));

    if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matching commands",
            Style::default().fg(palette.text_muted),
        )));
    } else {
        for spec in filtered {
            let cmd = spec.palette_label;
            let desc = spec.description;
            lines.push(Line::from(vec![
                Span::styled(format!("  /{cmd}"), Style::default().fg(palette.peach)),
                Span::styled(format!("  {desc}"), Style::default().fg(palette.text_muted)),
            ]));
        }
    }

    let palette = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.primary))
            .style(Style::default().bg(palette.bg_panel))
            .title(Line::from(vec![Span::styled(
                " Commands ",
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
            )])),
    );

    frame.render_widget(palette, palette_area);
}

/// Draws the model selector inline, replacing the input area (inline mode only).
fn draw_inline_model_selector(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    glyphs: &Glyphs,
) {
    let selected_index = app.model_select_index().unwrap_or(0);
    let models = PredefinedModel::all();

    let mut lines: Vec<Line> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        let is_selected = i == selected_index;
        let marker = if is_selected { glyphs.selected } else { " " };
        let num = i + 1;

        let style = if is_selected {
            Style::default()
                .fg(palette.text_primary)
                .bg(palette.bg_highlight)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text_secondary)
        };

        lines.push(Line::from(vec![Span::styled(
            format!(" {marker} {num}  {}", model.display_name()),
            style,
        )]));
    }

    let keybindings = Line::from(vec![
        Span::styled("↑↓", styles::key_highlight(palette)),
        Span::styled(" select  ", styles::key_hint(palette)),
        Span::styled("1-9", styles::key_highlight(palette)),
        Span::styled(" pick  ", styles::key_hint(palette)),
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" confirm  ", styles::key_hint(palette)),
        Span::styled("Esc", styles::key_highlight(palette)),
        Span::styled(" cancel ", styles::key_hint(palette)),
    ]);

    let (model_text, model_style) = if app.is_loading() {
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        (
            format!("{spinner} {}", app.model()),
            Style::default().fg(palette.primary),
        )
    } else if app.current_api_key().is_some() {
        (
            format!("{} {}", glyphs.status_ready, app.model()),
            Style::default().fg(palette.success),
        )
    } else {
        (
            format!("{} No API key", glyphs.status_missing),
            Style::default().fg(palette.error),
        )
    };

    let usage_status = app.context_usage_status();
    let (usage, severity_override) = match &usage_status {
        ContextUsageStatus::Ready(usage) => (usage, 0),
        ContextUsageStatus::NeedsSummarization { usage, .. } => (usage, 1),
        ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => (usage, 2),
    };
    let pct = usage.percentage();
    let remaining = (100.0 - pct).clamp(0.0, 100.0);
    let base_usage = format!("Context {remaining:.0}% left");
    let usage_str = match severity_override {
        2 => format!("{base_usage} !!"),
        1 => format!("{base_usage} !"),
        _ => base_usage,
    };
    let usage_color = match severity_override {
        1 | 2 => palette.red,
        _ => match usage.severity() {
            0 => palette.green,
            1 => palette.yellow,
            _ => palette.red,
        },
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .title_top(Line::from(vec![Span::styled(
            " MODEL ",
            styles::mode_model(palette),
        )]))
        .title_top(keybindings.alignment(Alignment::Right))
        .title_bottom(Line::from(vec![Span::styled(model_text, model_style)]))
        .title_bottom(
            Line::from(vec![Span::styled(
                usage_str,
                Style::default().fg(usage_color),
            )])
            .alignment(Alignment::Right),
        );

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_files_panel(frame: &mut Frame, app: &App, area: Rect, palette: &Palette, glyphs: &Glyphs) {
    let files = app.ordered_files();
    let panel = app.files_panel_state().clone();
    let is_expanded = panel.expanded.is_some();

    let hint = if is_expanded {
        " Tab/S-Tab │ Enter: collapse │ C-D/U "
    } else {
        " Tab: cycle files "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.text_muted))
        .title(" Files ")
        .title_style(Style::default().fg(palette.text_secondary))
        .title_bottom(
            Line::from(hint)
                .centered()
                .style(Style::default().fg(palette.text_muted)),
        )
        .style(Style::default().bg(palette.bg_dark));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if files.is_empty() {
        let text = Paragraph::new(Line::styled(
            "  No files modified",
            Style::default().fg(palette.text_muted),
        ));
        frame.render_widget(text, inner);
        return;
    }

    // Split inner area: file list (top) and diff (bottom, if expanded)
    let (list_area, diff_area) = if is_expanded {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(files.len() as u16 + 1),
                Constraint::Min(3),
            ])
            .split(inner);
        (chunks[0], Some(chunks[1]))
    } else {
        (inner, None)
    };

    draw_file_list(frame, list_area, &files, &panel, palette, glyphs);

    if let Some(diff_area) = diff_area {
        draw_diff_view(frame, diff_area, app, &panel, palette);
    }
}

fn draw_file_list(
    frame: &mut Frame,
    area: Rect,
    files: &[(std::path::PathBuf, ChangeKind)],
    panel: &forge_engine::FilesPanelState,
    palette: &Palette,
    _glyphs: &Glyphs,
) {
    let inner_width = area.width.saturating_sub(2) as usize;

    let lines: Vec<Line> = files
        .iter()
        .enumerate()
        .map(|(i, (path, kind))| {
            let display = truncate_path_display(path, inner_width.saturating_sub(4));
            let is_selected = i == panel.selected;
            let is_file_expanded = panel.expanded.as_ref() == Some(path);

            let prefix = if is_selected {
                if is_file_expanded {
                    " ▶ ".to_string()
                } else {
                    " › ".to_string()
                }
            } else {
                "   ".to_string()
            };

            let kind_color = match kind {
                ChangeKind::Modified => palette.warning,
                ChangeKind::Created => palette.success,
            };

            let style = if is_selected {
                Style::default()
                    .fg(kind_color)
                    .bg(palette.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(kind_color)
            };

            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(display, style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_diff_view(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    panel: &forge_engine::FilesPanelState,
    palette: &Palette,
) {
    // Horizontal divider at top
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    let divider_str: String = "─".repeat(area.width as usize);
    let divider =
        Paragraph::new(Line::from(divider_str)).style(Style::default().fg(palette.text_muted));
    frame.render_widget(divider, chunks[0]);

    let diff_area = chunks[1];

    match app.files_panel_diff() {
        Some(FileDiff::Diff(text) | FileDiff::Created(text)) => {
            let lines = render_tool_result_lines(&text, Style::default(), palette, " ");
            let total_lines = lines.len();

            let max_scroll = total_lines.saturating_sub(diff_area.height as usize);
            let scroll = panel.diff_scroll.min(max_scroll);

            let visible: Vec<Line> = lines
                .into_iter()
                .skip(scroll)
                .take(diff_area.height as usize)
                .collect();

            frame.render_widget(Paragraph::new(visible), diff_area);
        }
        Some(FileDiff::Deleted) => {
            let text = Paragraph::new(Line::styled(
                " File no longer exists",
                Style::default().fg(palette.text_muted),
            ));
            frame.render_widget(text, diff_area);
        }
        Some(FileDiff::Binary(size)) => {
            let text = Paragraph::new(Line::styled(
                format!(" Binary file ({size} bytes)"),
                Style::default().fg(palette.text_muted),
            ));
            frame.render_widget(text, diff_area);
        }
        Some(FileDiff::Error(e)) => {
            let text = Paragraph::new(Line::styled(
                format!(" Error: {e}"),
                Style::default().fg(palette.error),
            ));
            frame.render_widget(text, diff_area);
        }
        None => {}
    }
}

/// Strip the Windows extended-length path prefix (`\\?\`) for display.
fn strip_windows_prefix(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// Truncate a path for display, keeping the filename and as much of the parent as fits.
fn truncate_path_display(path: &std::path::Path, max_width: usize) -> String {
    let display = strip_windows_prefix(&path.display().to_string());
    if display.width() <= max_width {
        return display;
    }
    // Path doesn't fit - try to show just the filename
    if let Some(name) = path.file_name() {
        let name_str = name.to_string_lossy();
        // Check if filename alone fits
        if name_str.width() <= max_width {
            return name_str.into_owned();
        }
        // Filename doesn't fit - truncate it
        if max_width > 3 {
            let truncated: String = name_str
                .graphemes(true)
                .take(max_width.saturating_sub(3))
                .collect();
            return format!("{truncated}...");
        }
    }
    // Fallback: truncate from the right
    if max_width > 3 {
        let truncated: String = display
            .graphemes(true)
            .take(max_width.saturating_sub(3))
            .collect();
        return format!("{truncated}...");
    }
    display
}

pub fn draw_model_selector(
    frame: &mut Frame,
    app: &mut App,
    palette: &Palette,
    glyphs: &Glyphs,
    elapsed: Duration,
) {
    let area = frame.area();
    let selected_index = app.model_select_index().unwrap_or(0);

    let selector_width = 60.min(area.width.saturating_sub(4)).max(40);
    let content_width = selector_width.saturating_sub(4).max(1) as usize; // borders + padding

    let divider = Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    ));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(divider);
    lines.push(Line::from(""));

    let models = PredefinedModel::all();
    let mut row_index = 0usize;
    let mut push_row = |label: &str, selected: bool, muted: bool, tag: Option<(&str, Style)>| {
        row_index += 1;
        let prefix = if selected { glyphs.selected } else { " " };
        let left = format!(" {prefix} {row_index:>2}  {label}");
        let left_width = left.width();
        let (right_text, right_style) = tag.unwrap_or(("", Style::default()));
        let right_width = right_text.width();
        let gap = if right_text.is_empty() { 0 } else { 2 };
        let filler = content_width.saturating_sub(left_width + right_width + gap);

        let bg = if selected {
            Some(palette.bg_highlight)
        } else {
            None
        };
        let mut left_style = if selected {
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD)
        } else if muted {
            Style::default().fg(palette.text_muted)
        } else {
            Style::default().fg(palette.text_secondary)
        };
        if let Some(bg) = bg {
            left_style = left_style.bg(bg);
        }

        let mut filler_style = Style::default();
        if let Some(bg) = bg {
            filler_style = filler_style.bg(bg);
        }

        let mut right_style = right_style;
        if let Some(bg) = bg {
            right_style = right_style.bg(bg);
        }

        let mut spans = Vec::new();
        spans.push(Span::styled(left, left_style));
        if filler > 0 {
            spans.push(Span::styled(" ".repeat(filler), filler_style));
        }
        if !right_text.is_empty() {
            spans.push(Span::styled(" ".repeat(gap), filler_style));
            spans.push(Span::styled(right_text.to_string(), right_style));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(""));
    };

    for (i, model) in models.iter().enumerate() {
        let is_selected = i == selected_index;
        push_row(model.display_name(), is_selected, false, None);
    }

    if matches!(lines.last(), Some(line) if line.width() == 0) {
        lines.pop();
    }

    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("  ↑↓", styles::key_highlight(palette)),
        Span::styled(" select  ", styles::key_hint(palette)),
        Span::styled("1-9", styles::key_highlight(palette)),
        Span::styled(" quick pick  ", styles::key_hint(palette)),
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" confirm  ", styles::key_hint(palette)),
        Span::styled("Esc", styles::key_highlight(palette)),
        Span::styled(" cancel", styles::key_hint(palette)),
    ]));

    let inner_height = lines.len() as u16;
    let selector_height = inner_height.saturating_add(4); // borders + vertical padding
    let desired_y = area.y + area.height.saturating_sub(12);
    let max_y = area.y + area.height.saturating_sub(selector_height);
    let y = desired_y.min(max_y);

    let base_area = Rect {
        x: area.x + (area.width.saturating_sub(selector_width) / 2),
        y,
        width: selector_width,
        height: selector_height,
    };

    let (selector_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, area),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    frame.render_widget(Clear, selector_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(
            " Select Model ",
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )]));

    let selector = Paragraph::new(lines).block(block);

    frame.render_widget(selector, selector_area);
}

fn draw_tool_approval_prompt(frame: &mut Frame, app: &App, palette: &Palette) {
    let max_width = frame.area().width.saturating_sub(6).clamp(20, 80) as usize;
    let Some(view) = collect_approval_view(app, max_width) else {
        return;
    };

    let selected = &view.selected;
    let cursor = view.cursor;
    let confirm_deny = view.deny_confirm;
    let any_selected = view.any_selected;

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Tool approval required ",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, item) in view.items.iter().enumerate() {
        let is_selected = selected.get(i).copied().unwrap_or(false);
        let pointer = if i == cursor { ">" } else { " " };
        let checkbox = if is_selected { "[x]" } else { "[ ]" };
        let risk_label = item.risk_label.as_str();
        let risk_style = match risk_label {
            "HIGH" => Style::default()
                .fg(palette.error)
                .add_modifier(Modifier::BOLD),
            "MEDIUM" => Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(palette.success)
                .add_modifier(Modifier::BOLD),
        };
        let name_style = if i == cursor {
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text_primary)
        };

        let tool_name = item.tool_name.as_str();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{pointer} {checkbox} "),
                Style::default().fg(palette.text_muted),
            ),
            Span::styled(tool_name.to_string(), name_style),
            Span::raw(" "),
            Span::styled(risk_label.to_string(), risk_style),
        ]));

        if let Some(summary) = item.summary.as_ref() {
            lines.push(Line::from(Span::styled(
                format!("    {summary}"),
                Style::default().fg(palette.text_muted),
            )));
        }

        for line in &item.details {
            lines.push(Line::from(Span::styled(
                format!("      {line}"),
                Style::default().fg(palette.text_muted),
            )));
        }
    }

    if confirm_deny {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Confirm Deny All: press Enter again",
            Style::default()
                .fg(palette.error)
                .add_modifier(Modifier::BOLD),
        )));
    }

    if !any_selected {
        lines.push(Line::from(Span::styled(
            " No tools selected — approving will deny all.",
            Style::default().fg(palette.warning),
        )));
    }

    // Render Approve and Deny buttons
    lines.push(Line::from(""));
    let submit_cursor = view.items.len();
    let deny_cursor = view.items.len() + 1;

    let submit_pointer = if cursor == submit_cursor { ">" } else { " " };
    let deny_pointer = if cursor == deny_cursor { ">" } else { " " };

    let submit_style = if cursor == submit_cursor {
        Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.text_muted)
    };
    let deny_style = if cursor == deny_cursor {
        Style::default()
            .fg(palette.error)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.text_muted)
    };

    lines.push(Line::from(vec![
        Span::styled(
            format!("{submit_pointer} "),
            Style::default().fg(palette.text_muted),
        ),
        Span::styled("[ Approve selected ]", submit_style),
        Span::raw("    "),
        Span::styled(
            format!("{deny_pointer} "),
            Style::default().fg(palette.text_muted),
        ),
        Span::styled("[ Deny All ]", deny_style),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Space", styles::key_highlight(palette)),
        Span::styled(" toggle  ", styles::key_hint(palette)),
        Span::styled("↑↓", styles::key_highlight(palette)),
        Span::styled(" navigate  ", styles::key_hint(palette)),
        Span::styled("Tab", styles::key_highlight(palette)),
        Span::styled(" details  ", styles::key_hint(palette)),
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" activate  ", styles::key_hint(palette)),
        Span::styled("a", styles::key_highlight(palette)),
        Span::styled(" approve all  ", styles::key_hint(palette)),
        Span::styled("d/Esc", styles::key_highlight(palette)),
        Span::styled(" deny", styles::key_hint(palette)),
    ]));

    let content_width = lines
        .iter()
        .map(ratatui::prelude::Line::width)
        .max()
        .unwrap_or(10) as u16;
    let content_width = content_width.min(frame.area().width.saturating_sub(4));
    let content_height = lines.len() as u16;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
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

fn draw_tool_recovery_prompt(frame: &mut Frame, app: &App, palette: &Palette, glyphs: &Glyphs) {
    let Some(calls) = app.tool_recovery_calls() else {
        return;
    };
    let results = app.tool_recovery_results().unwrap_or(&[]);

    let mut results_map: std::collections::HashMap<&str, &ToolResult> =
        std::collections::HashMap::new();
    for result in results {
        results_map.insert(result.tool_call_id.as_str(), result);
    }

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            " Tool recovery detected ",
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Tools will not be re-run.",
            Style::default().fg(palette.text_muted),
        )),
        Line::from(Span::styled(
            " Resume keeps recovered results and continues.",
            Style::default().fg(palette.text_muted),
        )),
        Line::from(Span::styled(
            " Discard drops recovered results and continues.",
            Style::default().fg(palette.text_muted),
        )),
        Line::from(""),
    ];

    for call in calls {
        let (icon, style) = if let Some(result) = results_map.get(call.id.as_str()) {
            if result.is_error {
                (
                    glyphs.tool_result_err,
                    Style::default()
                        .fg(palette.error)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    glyphs.tool_result_ok,
                    Style::default()
                        .fg(palette.success)
                        .add_modifier(Modifier::BOLD),
                )
            }
        } else {
            (glyphs.bullet, Style::default().fg(palette.text_muted))
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} "), style),
            Span::styled(
                format!("{} ({})", call.name, call.id),
                Style::default().fg(palette.text_muted),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("r", styles::key_highlight(palette)),
        Span::styled(
            " resume with recovered results  ",
            styles::key_hint(palette),
        ),
        Span::styled("d", styles::key_highlight(palette)),
        Span::styled(" discard results", styles::key_hint(palette)),
    ]));

    let content_width = lines
        .iter()
        .map(ratatui::prelude::Line::width)
        .max()
        .unwrap_or(10) as u16;
    let content_width = content_width.min(frame.area().width.saturating_sub(4));
    let content_height = lines.len() as u16;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
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

fn create_welcome_screen(app: &App, palette: &Palette, glyphs: &Glyphs) -> Paragraph<'static> {
    let version = env!("CARGO_PKG_VERSION");
    let build_profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let logo = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Forge",
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!("  v{version} ({build_profile})"),
            Style::default().fg(palette.text_secondary),
        )]),
        Line::from(vec![Span::styled(
            "  Your AI Assistant Interface",
            Style::default().fg(palette.text_secondary),
        )]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Quick Start:",
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "    i",
                Style::default()
                    .fg(palette.green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Enter insert mode to type",
                Style::default().fg(palette.text_secondary),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Enter",
                Style::default()
                    .fg(palette.green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Send your message",
                Style::default().fg(palette.text_secondary),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Esc",
                Style::default()
                    .fg(palette.yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Return to normal mode",
                Style::default().fg(palette.text_secondary),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    /",
                Style::default()
                    .fg(palette.peach)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Open command palette",
                Style::default().fg(palette.text_secondary),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    q",
                Style::default()
                    .fg(palette.red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Quit", Style::default().fg(palette.text_secondary)),
        ]),
        Line::from(""),
        Line::from(""),
    ];

    let mut lines = logo;

    // Show status for each provider
    for provider in Provider::all() {
        let has_key = app.has_api_key(*provider);
        let is_current = app.provider() == *provider;

        let status_line = if has_key {
            Line::from(vec![
                Span::styled(
                    if is_current {
                        format!("  {} ", glyphs.status_ready)
                    } else {
                        format!("  {} ", glyphs.status_missing)
                    },
                    Style::default().fg(if is_current {
                        palette.success
                    } else {
                        palette.text_muted
                    }),
                ),
                Span::styled(
                    provider.display_name(),
                    Style::default().fg(if is_current {
                        palette.success
                    } else {
                        palette.text_secondary
                    }),
                ),
                Span::styled(" - Ready", Style::default().fg(palette.text_muted)),
                if is_current {
                    Span::styled(" (active)", Style::default().fg(palette.success))
                } else {
                    Span::styled("", Style::default())
                },
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    format!("  {} ", glyphs.status_missing),
                    Style::default().fg(palette.text_muted),
                ),
                Span::styled(
                    provider.display_name(),
                    Style::default().fg(palette.text_muted),
                ),
                Span::styled(" - Set ", Style::default().fg(palette.text_muted)),
                Span::styled(provider.env_var(), Style::default().fg(palette.peach)),
            ])
        };
        lines.push(status_line);
    }

    if app.current_api_key().is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Next: ", Style::default().fg(palette.text_muted)),
            Span::styled(
                format!(
                    "Set {} and restart to begin chatting.",
                    app.provider().env_var()
                ),
                Style::default().fg(palette.peach),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Tip: ", Style::default().fg(palette.text_muted)),
        Span::styled("/p claude", Style::default().fg(palette.peach)),
        Span::styled(" or ", Style::default().fg(palette.text_muted)),
        Span::styled("/p gpt", Style::default().fg(palette.peach)),
        Span::styled(
            " to switch providers",
            Style::default().fg(palette.text_muted),
        ),
    ]));

    Paragraph::new(lines).alignment(Alignment::Left)
}

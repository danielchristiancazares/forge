//! TUI rendering for Forge using ratatui.

mod effects;
mod input;
pub mod markdown;
mod theme;
mod tool_display;
mod ui_inline;

pub use effects::apply_modal_effect;
pub use input::handle_events;
pub use theme::{glyphs, palette, spinner_frame, styles, Glyphs, Palette};
pub use ui_inline::{
    INLINE_INPUT_HEIGHT, INLINE_MODEL_SELECTOR_HEIGHT, INLINE_VIEWPORT_HEIGHT, InlineOutput,
    clear_inline_viewport, draw as draw_inline, inline_viewport_height,
};

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
    App, ContextUsageStatus, DisplayItem, InputMode, Message, PredefinedModel, Provider,
};
use forge_types::{ToolResult, sanitize_terminal_text};

pub use self::markdown::clear_render_cache;
use self::markdown::render_markdown;

/// Main draw function
pub fn draw(frame: &mut Frame, app: &mut App) {
    let options = app.ui_options();
    let palette = palette(options);
    let glyphs = glyphs(options);
    // Clear with background color
    let bg_block = Block::default().style(Style::default().bg(palette.bg_dark));
    frame.render_widget(bg_block, frame.area());

    let input_height = match app.input_mode() {
        InputMode::Normal => 3,
        _ => 5,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(1),               // Messages
            Constraint::Length(input_height), // Input
            Constraint::Length(1),            // Status bar
        ])
        .split(frame.area());

    draw_messages(frame, app, chunks[0], &palette, &glyphs);
    draw_input(frame, app, chunks[1], &palette);
    draw_status_bar(frame, app, chunks[2], &palette, &glyphs);

    // Draw command palette if in command mode
    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app, &palette);
    }

    // Draw model selector if in model select mode
    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app, &palette, &glyphs);
    }

    if app.tool_approval_requests().is_some() {
        draw_tool_approval_prompt(frame, app, &palette);
    }

    if app.tool_recovery_calls().is_some() {
        draw_tool_recovery_prompt(frame, app, &palette, &glyphs);
    }
}

fn draw_messages(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    glyphs: &Glyphs,
) {
    // Helper to render a single message (defined at function start to satisfy clippy)
    fn render_message(
        msg: &Message,
        lines: &mut Vec<Line>,
        msg_count: &mut usize,
        palette: &Palette,
        glyphs: &Glyphs,
    ) {
        // Add spacing between messages (except first)
        if *msg_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(""));
        }
        *msg_count += 1;

        // Message header with role icon and name
        let (icon, name, name_style) = match msg {
            Message::System(_) => (
                glyphs.system.to_string(),
                "System".to_string(),
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Message::User(_) => (
                glyphs.user.to_string(),
                "You".to_string(),
                styles::user_name(palette),
            ),
            Message::Assistant(m) => (
                glyphs.assistant.to_string(),
                m.provider().display_name().to_string(),
                styles::assistant_name(palette),
            ),
            Message::ToolUse(call) => {
                let compact = tool_display::format_tool_call_compact(&call.name, &call.arguments);
                let compact = sanitize_terminal_text(&compact).into_owned();
                (
                    glyphs.tool.to_string(),
                    compact,
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                )
            }
            Message::ToolResult(result) => {
                let (icon, style, label) = if result.is_error {
                    (
                        glyphs.tool_result_err,
                        Style::default()
                            .fg(palette.error)
                            .add_modifier(Modifier::BOLD),
                        "Tool Result (error)",
                    )
                } else {
                    (
                        glyphs.tool_result_ok,
                        Style::default()
                            .fg(palette.success)
                            .add_modifier(Modifier::BOLD),
                        "Tool Result (ok)",
                    )
                };
                (icon.to_string(), label.to_string(), style)
            }
        };

        let header_line = Line::from(vec![
            Span::styled(format!(" {icon} "), name_style),
            Span::styled(name, name_style),
        ]);
        lines.push(header_line);
        lines.push(Line::from("")); // Space after header

        // Message content - render based on type
        match msg {
            Message::ToolUse(_) => {
                // Compact format: args are in the header line, no body needed
            }
            Message::ToolResult(result) => {
                // Render result content with appropriate styling
                let content_style = if result.is_error {
                    Style::default().fg(palette.error)
                } else {
                    Style::default().fg(palette.text_secondary)
                };
                let content = sanitize_terminal_text(&result.content);
                for result_line in content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {result_line}"),
                        content_style,
                    )));
                }
            }
            _ => {
                // Regular messages - render as markdown
                let content_style = match msg {
                    Message::User(_) => Style::default().fg(palette.text_primary),
                    Message::Assistant(_) => Style::default().fg(palette.text_secondary),
                    _ => Style::default().fg(palette.text_muted),
                };
                let content = sanitize_terminal_text(msg.content());
                let rendered = render_markdown(content.as_ref(), content_style, palette);
                lines.extend(rendered);
            }
        }
    }

    let messages_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.text_muted))
        .padding(Padding::horizontal(1));

    // Show welcome screen if no messages
    if app.is_empty() {
        app.update_scroll_max(0);
        let welcome = create_welcome_screen(app, palette, glyphs);
        frame.render_widget(welcome.block(messages_block), area);
        return;
    }

    // Build message content
    let mut lines: Vec<Line> = Vec::new();
    let mut msg_count = 0;

    // Render complete messages from display items
    for item in app.display_items() {
        let msg = match item {
            DisplayItem::History(id) => app.history().get_entry(*id).message(),
            DisplayItem::Local(msg) => msg,
        };
        render_message(msg, &mut lines, &mut msg_count, palette, glyphs);
    }

    // Render streaming message if present (State as Location)
    if let Some(streaming) = app.streaming() {
        if msg_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(""));
        }

        let (icon, name, name_style) = (
            glyphs.assistant,
            streaming.provider().display_name(),
            styles::assistant_name(palette),
        );

        let header_line = Line::from(vec![
            Span::styled(format!(" {icon} "), name_style),
            Span::styled(name, name_style),
        ]);
        lines.push(header_line);
        lines.push(Line::from(""));

        if streaming.content().is_empty() {
            // Show animated spinner for loading
            let spinner = spinner_frame(app.tick_count(), app.ui_options());
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(spinner, Style::default().fg(palette.primary)),
                Span::styled(" Thinking...", Style::default().fg(palette.text_muted)),
            ]));
        } else {
            let content_style = Style::default().fg(palette.text_secondary);
            let content = sanitize_terminal_text(streaming.content());
            let rendered = render_markdown(content.as_ref(), content_style, palette);
            lines.extend(rendered);
        }
    }

    // Render awaiting tool results status if present
    if let Some(pending_calls) = app.pending_tool_calls() {
        if msg_count > 0 || app.streaming().is_some() {
            lines.push(Line::from(""));
        }
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        let status_line = Line::from(vec![
            Span::styled(
                format!("{spinner} "),
                Style::default().fg(palette.warning),
            ),
            Span::styled(
                format!("Awaiting {} tool result(s)...", pending_calls.len()),
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]);
        lines.push(status_line);

        // List pending tools
        for call in pending_calls {
            let name = sanitize_terminal_text(&call.name);
            let id = sanitize_terminal_text(&call.id);
            lines.push(Line::from(Span::styled(
                format!("  {} {} ({})", glyphs.bullet, name.as_ref(), id.as_ref()),
                Style::default().fg(palette.text_muted),
            )));
        }

        lines.push(Line::from(Span::styled(
            "  Use /tool <id> <result> or /tool error <id> <message>",
            Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    if let Some(calls) = app.tool_loop_calls() {
        if msg_count > 0 || app.streaming().is_some() || app.pending_tool_calls().is_some() {
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

        let mut results_map: std::collections::HashMap<&str, &ToolResult> =
            std::collections::HashMap::new();
        if let Some(results) = app.tool_loop_results() {
            for result in results {
                results_map.insert(result.tool_call_id.as_str(), result);
            }
        }

        let execute_ids: std::collections::HashSet<&str> = app
            .tool_loop_execute_calls()
            .map(|exec_calls| exec_calls.iter().map(|c| c.id.as_str()).collect())
            .unwrap_or_default();

        let current_id = app.tool_loop_current_call_id();

        for call in calls {
            let result = results_map.get(call.id.as_str());
            let mut reason: Option<String> = None;
            let (icon, style, label) = if let Some(result) = result {
                if !execute_ids.contains(call.id.as_str()) {
                    let content = sanitize_terminal_text(&result.content);
                    reason = content
                        .lines()
                        .next()
                        .map(|line| truncate_with_ellipsis(line, 80));
                    (
                        glyphs.denied,
                        Style::default()
                            .fg(palette.warning)
                            .add_modifier(Modifier::BOLD),
                        "denied",
                    )
                } else if result.is_error {
                    let content = sanitize_terminal_text(&result.content);
                    reason = content
                        .lines()
                        .next()
                        .map(|line| truncate_with_ellipsis(line, 80));
                    (
                        glyphs.tool_result_err,
                        Style::default()
                            .fg(palette.error)
                            .add_modifier(Modifier::BOLD),
                        "error",
                    )
                } else {
                    (
                        glyphs.tool_result_ok,
                        Style::default()
                            .fg(palette.success)
                            .add_modifier(Modifier::BOLD),
                        "ok",
                    )
                }
            } else if current_id == Some(call.id.as_str()) {
                (
                    spinner,
                    Style::default()
                        .fg(palette.primary)
                        .add_modifier(Modifier::BOLD),
                    "running",
                )
            } else if approval_pending && !execute_ids.contains(call.id.as_str()) {
                (
                    glyphs.paused,
                    Style::default()
                        .fg(palette.warning)
                        .add_modifier(Modifier::BOLD),
                    "paused",
                )
            } else {
                (
                    glyphs.bullet,
                    Style::default().fg(palette.text_muted),
                    "pending",
                )
            };

            let name = sanitize_terminal_text(&call.name);
            let id = sanitize_terminal_text(&call.id);
            lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), style),
                Span::styled(
                    format!("{} ({}) [{label}]", name.as_ref(), id.as_ref()),
                    Style::default().fg(palette.text_muted),
                ),
            ]));

            if let Some(reason) = reason {
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

    // Calculate content height and visible height for scrolling
    let inner = messages_block.inner(area);
    let total_lines = wrapped_line_count(&lines, inner.width);
    let visible_height = inner.height;

    let max_scroll = total_lines.saturating_sub(visible_height);
    app.update_scroll_max(max_scroll);
    let scroll_offset = app.scroll_offset_from_top();

    let messages = Paragraph::new(lines)
        .block(messages_block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));

    frame.render_widget(messages, area);

    // Only render scrollbar when content exceeds viewport
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

fn wrapped_line_count(lines: &[Line], width: u16) -> u16 {
    let width = width.max(1) as usize;
    let mut total: u16 = 0;

    for line in lines {
        let line_width = line.width();
        let rows = if line_width == 0 {
            1
        } else {
            ((line_width - 1) / width) + 1
        };
        total = total.saturating_add(rows as u16);
    }

    total
}

fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max - 3).collect();
        format!("{head}...")
    }
}

pub(crate) fn draw_input(frame: &mut Frame, app: &mut App, area: Rect, palette: &Palette) {
    let mode = app.input_mode();
    let options = app.ui_options();
    // Clone command text to avoid borrow conflict with mutable context_usage_status()
    let command_line: Option<String> = if mode == InputMode::Command {
        app.command_text().map(str::to_string)
    } else {
        None
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
    let usage_str = match severity_override {
        2 => format!("{} !!", usage.format_compact()), // Double bang for unrecoverable
        1 => format!("{} !", usage.format_compact()),
        _ => usage.format_compact(),
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
    let inner_height = area.height.saturating_sub(2 + padding_v.saturating_mul(2)).max(1);

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
            let cursor_display_pos = cmd.width();
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
                    InputMode::Command => command_line.clone()
                        .unwrap_or_default(),
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
            && let Some(command_line) = command_line.as_ref() {
                let cursor_display_pos = command_line.width() as u16;
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

    let input = Paragraph::new(input_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(Line::from(vec![Span::styled(mode_text, mode_style)]))
            .title_top(Line::from(hints).alignment(Alignment::Right))
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

pub(crate) fn draw_status_bar(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    palette: &Palette,
    glyphs: &Glyphs,
) {
    let (status_text, status_style) = if let Some(msg) = app.status_message() {
        let kind = app.status_kind();
        let (prefix, color) = match kind {
            forge_engine::StatusKind::Error => ("Error: ", palette.error),
            forge_engine::StatusKind::Warning => ("Warning: ", palette.warning),
            forge_engine::StatusKind::Success => ("Success: ", palette.success),
            forge_engine::StatusKind::Info => ("", palette.text_secondary),
        };
        (format!("{prefix}{msg}"), Style::default().fg(color))
    } else if app.is_loading() {
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        (
            format!("{spinner} Processing request..."),
            Style::default().fg(palette.primary),
        )
    } else if app.current_api_key().is_some() {
        (
            format!("{} {} │ {}", glyphs.status_ready, app.provider().display_name(), app.model()),
            Style::default().fg(palette.success),
        )
    } else {
        (
            format!(
                "{} No API key │ Set {}",
                glyphs.status_missing,
                app.provider().env_var()
            ),
            Style::default().fg(palette.error),
        )
    };

    let status_text = if app.context_infinity_enabled() {
        status_text
    } else {
        format!("{status_text} │ CI: off")
    };

    // Build status line
    let status = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(status_text, status_style),
    ]));
    frame.render_widget(status, area);
}

fn draw_command_palette(frame: &mut Frame, app: &App, palette: &Palette) {
    let area = frame.area();

    // Center the palette
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 14;

    let palette_area = Rect {
        x: area.x + (area.width.saturating_sub(palette_width) / 2),
        y: area.y + (area.height / 3),
        width: palette_width,
        height: palette_height,
    };

    // Clear background
    frame.render_widget(Clear, palette_area);

    let filter_raw = app.command_text().unwrap_or("").trim();
    let filter = filter_raw.trim_start_matches('/').to_ascii_lowercase();

    let commands = vec![
        ("q, quit", "Exit the application"),
        ("clear", "Clear conversation history"),
        ("cancel", "Cancel streaming or tool execution"),
        ("tool <id> <result>", "Submit a tool result"),
        ("tools", "Show tool status"),
        ("model <name>", "Change the model"),
        ("p, provider <name>", "Switch provider (claude/gpt)"),
        ("ctx", "Show context usage"),
        ("jrnl", "Show stream journal stats"),
        ("sum", "Summarize older messages"),
        ("screen", "Toggle fullscreen/inline mode"),
        ("help", "Show available commands"),
    ];

    let filtered: Vec<_> = if filter.is_empty() {
        commands
    } else {
        commands
            .into_iter()
            .filter(|(cmd, desc)| {
                cmd.to_ascii_lowercase().contains(&filter)
                    || desc.to_ascii_lowercase().contains(&filter)
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
        for (cmd, desc) in filtered {
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

pub fn draw_model_selector(frame: &mut Frame, app: &mut App, palette: &Palette, glyphs: &Glyphs) {
    let area = frame.area();
    let selected_index = app.model_select_index().unwrap_or(0);

    // Center the selector over the input area
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

    push_row(
        "Google Gemini 3 Pro",
        false,
        true,
        Some((
            "preview",
            Style::default()
                .fg(palette.peach)
                .add_modifier(Modifier::BOLD),
        )),
    );

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

    let elapsed = app.frame_elapsed();
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

    // Clear background
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
    let Some(requests) = app.tool_approval_requests() else {
        return;
    };
    let selected = app.tool_approval_selected().unwrap_or(&[]);
    let cursor = app.tool_approval_cursor().unwrap_or(0);
    let confirm_deny = app.tool_approval_deny_confirm();
    let expanded = app.tool_approval_expanded();
    let any_selected = selected.iter().any(|flag| *flag);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Tool approval required ",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let max_width = frame.area().width.saturating_sub(6).clamp(20, 80) as usize;

    for (i, req) in requests.iter().enumerate() {
        let is_selected = selected.get(i).copied().unwrap_or(false);
        let pointer = if i == cursor { ">" } else { " " };
        let checkbox = if is_selected { "[x]" } else { "[ ]" };
        let risk_label = format!("{:?}", req.risk_level).to_uppercase();
        let risk_style = match risk_label.as_str() {
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

        let tool_name = sanitize_terminal_text(&req.tool_name).into_owned();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{pointer} {checkbox} "),
                Style::default().fg(palette.text_muted),
            ),
            Span::styled(tool_name, name_style),
            Span::raw(" "),
            Span::styled(risk_label, risk_style),
        ]));

        if !req.summary.trim().is_empty() {
            let summary = sanitize_terminal_text(&req.summary);
            let summary = truncate_with_ellipsis(summary.as_ref(), max_width.saturating_sub(6));
            lines.push(Line::from(Span::styled(
                format!("    {summary}"),
                Style::default().fg(palette.text_muted),
            )));
        }

        if expanded == Some(i)
            && let Ok(details) = serde_json::to_string_pretty(&req.arguments) {
                for line in details.lines() {
                    let truncated = truncate_with_ellipsis(line, max_width.saturating_sub(6));
                    lines.push(Line::from(Span::styled(
                        format!("      {truncated}"),
                        Style::default().fg(palette.text_muted),
                    )));
                }
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
    let submit_cursor = requests.len();
    let deny_cursor = requests.len() + 1;

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

    let content_width = lines.iter().map(ratatui::prelude::Line::width).max().unwrap_or(10) as u16;
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

fn draw_tool_recovery_prompt(
    frame: &mut Frame,
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
) {
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
        Span::styled(" resume with recovered results  ", styles::key_hint(palette)),
        Span::styled("d", styles::key_highlight(palette)),
        Span::styled(" discard results", styles::key_hint(palette)),
    ]));

    let content_width = lines.iter().map(ratatui::prelude::Line::width).max().unwrap_or(10) as u16;
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
    let logo = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  ╭─────────────────────────────────────╮",
            Style::default().fg(palette.primary_dim),
        )]),
        Line::from(vec![
            Span::styled("  │", Style::default().fg(palette.primary_dim)),
            Span::styled(
                "     ✨ LLM API Harness ✨              ",
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│", Style::default().fg(palette.primary_dim)),
        ]),
        Line::from(vec![
            Span::styled("  │", Style::default().fg(palette.primary_dim)),
            Span::styled(
                "     Your AI Assistant Interface       ",
                Style::default().fg(palette.text_secondary),
            ),
            Span::styled("│", Style::default().fg(palette.primary_dim)),
        ]),
        Line::from(vec![Span::styled(
            "  ╰─────────────────────────────────────╯",
            Style::default().fg(palette.primary_dim),
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

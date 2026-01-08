//! TUI rendering for Forge using ratatui.

mod effects;
mod input;
pub mod markdown;
mod theme;
mod ui_inline;

pub use effects::apply_modal_effect;
pub use input::handle_events;
pub use theme::{colors, spinner_frame, styles};
pub use ui_inline::{
    INLINE_INPUT_HEIGHT, INLINE_MODEL_SELECTOR_HEIGHT, INLINE_VIEWPORT_HEIGHT, InlineOutput,
    draw as draw_inline, inline_viewport_height,
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
use forge_types::ToolResult;

pub use self::markdown::clear_render_cache;
use self::markdown::render_markdown;

/// Main draw function
pub fn draw(frame: &mut Frame, app: &mut App) {
    // Clear with background color
    let bg_block = Block::default().style(Style::default().bg(colors::BG_DARK));
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

    draw_messages(frame, app, chunks[0]);
    draw_input(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    // Draw command palette if in command mode
    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app);
    }

    // Draw model selector if in model select mode
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

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let messages_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::TEXT_MUTED))
        .padding(Padding::horizontal(1));

    // Show welcome screen if no messages
    if app.is_empty() {
        app.update_scroll_max(0);
        let welcome = create_welcome_screen(app);
        frame.render_widget(welcome.block(messages_block), area);
        return;
    }

    // Build message content
    let mut lines: Vec<Line> = Vec::new();
    let mut msg_count = 0;

    // Helper to render a single message
    fn render_message(msg: &Message, lines: &mut Vec<Line>, msg_count: &mut usize) {
        // Add spacing between messages (except first)
        if *msg_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(""));
        }
        *msg_count += 1;

        // Message header with role icon and name
        let (icon, name, name_style) = match msg {
            Message::System(_) => (
                "●".to_string(),
                "System".to_string(),
                Style::default()
                    .fg(colors::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Message::User(_) => ("○".to_string(), "You".to_string(), styles::user_name()),
            Message::Assistant(m) => (
                "◆".to_string(),
                m.provider().display_name().to_string(),
                styles::assistant_name(),
            ),
            Message::ToolUse(call) => (
                "⚙".to_string(),
                call.name.clone(),
                Style::default()
                    .fg(colors::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Message::ToolResult(result) => {
                let (icon, style) = if result.is_error {
                    (
                        "✗",
                        Style::default()
                            .fg(colors::ERROR)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    (
                        "✓",
                        Style::default()
                            .fg(colors::SUCCESS)
                            .add_modifier(Modifier::BOLD),
                    )
                };
                (icon.to_string(), "Tool Result".to_string(), style)
            }
        };

        let header_line = Line::from(vec![
            Span::styled(format!(" {} ", icon), name_style),
            Span::styled(name, name_style),
        ]);
        lines.push(header_line);
        lines.push(Line::from("")); // Space after header

        // Message content - render based on type
        match msg {
            Message::ToolUse(call) => {
                // Render tool arguments as formatted JSON
                let args_str = serde_json::to_string_pretty(&call.arguments)
                    .unwrap_or_else(|_| "{}".to_string());
                let args_style = Style::default().fg(colors::TEXT_MUTED);
                for arg_line in args_str.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", arg_line),
                        args_style,
                    )));
                }
            }
            Message::ToolResult(result) => {
                // Render result content with appropriate styling
                let content_style = if result.is_error {
                    Style::default().fg(colors::ERROR)
                } else {
                    Style::default().fg(colors::TEXT_SECONDARY)
                };
                for result_line in result.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", result_line),
                        content_style,
                    )));
                }
            }
            _ => {
                // Regular messages - render as markdown
                let content_style = match msg {
                    Message::System(_) => Style::default().fg(colors::TEXT_MUTED),
                    Message::User(_) => Style::default().fg(colors::TEXT_PRIMARY),
                    Message::Assistant(_) => Style::default().fg(colors::TEXT_SECONDARY),
                    _ => Style::default().fg(colors::TEXT_MUTED),
                };
                let rendered = render_markdown(msg.content(), content_style);
                lines.extend(rendered);
            }
        }
    }

    // Render complete messages from display items
    for item in app.display_items() {
        let msg = match item {
            DisplayItem::History(id) => app.history().get_entry(*id).message(),
            DisplayItem::Local(msg) => msg,
        };
        render_message(msg, &mut lines, &mut msg_count);
    }

    // Render streaming message if present (State as Location)
    if let Some(streaming) = app.streaming() {
        if msg_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(""));
        }

        let (icon, name, name_style) = (
            "◆",
            streaming.provider().display_name(),
            styles::assistant_name(),
        );

        let header_line = Line::from(vec![
            Span::styled(format!(" {icon} "), name_style),
            Span::styled(name, name_style),
        ]);
        lines.push(header_line);
        lines.push(Line::from(""));

        if streaming.content().is_empty() {
            // Show animated spinner for loading
            let spinner = spinner_frame(app.tick_count());
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(spinner, Style::default().fg(colors::PRIMARY)),
                Span::styled(" Thinking...", Style::default().fg(colors::TEXT_MUTED)),
            ]));
        } else {
            let content_style = Style::default().fg(colors::TEXT_SECONDARY);
            let rendered = render_markdown(streaming.content(), content_style);
            lines.extend(rendered);
        }
    }

    // Render awaiting tool results status if present
    if let Some(pending_calls) = app.pending_tool_calls() {
        if msg_count > 0 || app.streaming().is_some() {
            lines.push(Line::from(""));
        }
        let spinner = spinner_frame(app.tick_count());
        let status_line = Line::from(vec![
            Span::styled(
                format!("{} ", spinner),
                Style::default().fg(colors::WARNING),
            ),
            Span::styled(
                format!("Awaiting {} tool result(s)...", pending_calls.len()),
                Style::default()
                    .fg(colors::TEXT_MUTED)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]);
        lines.push(status_line);

        // List pending tools
        for call in pending_calls {
            lines.push(Line::from(Span::styled(
                format!("  • {} ({})", call.name, call.id),
                Style::default().fg(colors::TEXT_MUTED),
            )));
        }

        lines.push(Line::from(Span::styled(
            "  Use /tool <id> <result> or /tool error <id> <message>",
            Style::default()
                .fg(colors::TEXT_MUTED)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    if let Some(calls) = app.tool_loop_calls() {
        if msg_count > 0 || app.streaming().is_some() || app.pending_tool_calls().is_some() {
            lines.push(Line::from(""));
        }
        let spinner = spinner_frame(app.tick_count());
        let approval_pending = app.tool_approval_requests().is_some();
        let header = if approval_pending {
            format!("{spinner} Tool approval required")
        } else {
            format!("{spinner} Tool execution")
        };
        lines.push(Line::from(Span::styled(
            header,
            Style::default()
                .fg(colors::WARNING)
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
            let (icon, style) = if let Some(result) = result {
                if !execute_ids.contains(call.id.as_str()) {
                    reason = result
                        .content
                        .lines()
                        .next()
                        .map(|line| truncate_with_ellipsis(line, 80));
                    (
                        "⊘",
                        Style::default()
                            .fg(colors::WARNING)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if result.is_error {
                    reason = result
                        .content
                        .lines()
                        .next()
                        .map(|line| truncate_with_ellipsis(line, 80));
                    (
                        "✗",
                        Style::default()
                            .fg(colors::ERROR)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    (
                        "✓",
                        Style::default()
                            .fg(colors::SUCCESS)
                            .add_modifier(Modifier::BOLD),
                    )
                }
            } else if current_id == Some(call.id.as_str()) {
                (
                    spinner,
                    Style::default()
                        .fg(colors::PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )
            } else if approval_pending && !execute_ids.contains(call.id.as_str()) {
                (
                    "⏸",
                    Style::default()
                        .fg(colors::WARNING)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("•", Style::default().fg(colors::TEXT_MUTED))
            };

            lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), style),
                Span::styled(
                    format!("{} ({})", call.name, call.id),
                    Style::default().fg(colors::TEXT_MUTED),
                ),
            ]));

            if let Some(reason) = reason {
                lines.push(Line::from(Span::styled(
                    format!("    ↳ {reason}"),
                    Style::default().fg(colors::TEXT_MUTED),
                )));
            }
        }

        if let Some(output_lines) = app.tool_loop_output_lines() {
            if !output_lines.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  Tool output:",
                    Style::default().fg(colors::TEXT_MUTED),
                )));
                for line in output_lines {
                    lines.push(Line::from(Span::styled(
                        format!("    {line}"),
                        Style::default().fg(colors::TEXT_SECONDARY),
                    )));
                }
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
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(colors::TEXT_MUTED));

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

pub(crate) fn draw_input(frame: &mut Frame, app: &mut App, area: Rect) {
    let mode = app.input_mode();
    // Clone command text to avoid borrow conflict with mutable context_usage_status()
    let command_line: Option<String> = if mode == InputMode::Command {
        app.command_text().map(str::to_string)
    } else {
        None
    };

    let (mode_text, mode_style, border_style, prompt_char) = match mode {
        InputMode::Normal | InputMode::ModelSelect => (
            " NORMAL ",
            styles::mode_normal(),
            Style::default().fg(colors::TEXT_MUTED),
            "",
        ),
        InputMode::Insert => (
            " INSERT ",
            styles::mode_insert(),
            Style::default().fg(colors::GREEN),
            "❯",
        ),
        InputMode::Command => (
            " COMMAND ",
            styles::mode_command(),
            Style::default().fg(colors::YELLOW),
            "/",
        ),
    };

    // Key hints based on mode
    let hints = match mode {
        InputMode::Normal => vec![
            Span::styled("i", styles::key_highlight()),
            Span::styled(" insert  ", styles::key_hint()),
            Span::styled("/", styles::key_highlight()),
            Span::styled(" command  ", styles::key_hint()),
            Span::styled("q", styles::key_highlight()),
            Span::styled(" quit ", styles::key_hint()),
        ],
        InputMode::Insert => vec![
            Span::styled("Enter", styles::key_highlight()),
            Span::styled(" send  ", styles::key_hint()),
            Span::styled("Esc", styles::key_highlight()),
            Span::styled(" normal ", styles::key_hint()),
        ],
        InputMode::Command => vec![
            Span::styled("Enter", styles::key_highlight()),
            Span::styled(" execute  ", styles::key_hint()),
            Span::styled("Esc", styles::key_highlight()),
            Span::styled(" cancel ", styles::key_hint()),
        ],
        InputMode::ModelSelect => vec![
            Span::styled("↑↓", styles::key_highlight()),
            Span::styled(" select  ", styles::key_hint()),
            Span::styled("Enter", styles::key_highlight()),
            Span::styled(" confirm  ", styles::key_hint()),
            Span::styled("Esc", styles::key_highlight()),
            Span::styled(" cancel ", styles::key_hint()),
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
        1 | 2 => colors::RED,
        _ => match usage.severity() {
            0 => colors::GREEN,  // < 70%
            1 => colors::YELLOW, // 70-90%
            _ => colors::RED,    // > 90%
        },
    };

    let input_padding = if mode == InputMode::Normal {
        Padding::vertical(0)
    } else {
        Padding::vertical(1)
    };

    // Calculate horizontal scroll offset to keep cursor visible.
    // The visible content width is: area.width - borders (2) - prompt (3) - right padding (1)
    let visible_content_width = area.width.saturating_sub(6) as usize;

    // Calculate scroll offset and prepare text for display.
    // We slice the text to show the portion that includes the cursor, keeping the prompt fixed.
    let (display_text, horizontal_scroll) = if mode == InputMode::Insert {
        let cursor_index = app.draft_cursor_byte_index();
        let draft = app.draft_text();
        let text_before_cursor = &draft[..cursor_index];
        let cursor_display_pos = text_before_cursor.width();

        if cursor_display_pos >= visible_content_width {
            // Need to scroll: find the byte offset to start from
            let scroll_target = cursor_display_pos - visible_content_width + 1;
            // Find byte index corresponding to scroll_target display width
            let mut byte_offset = 0;
            let mut skipped_width = 0;
            for (idx, grapheme) in draft.grapheme_indices(true) {
                if skipped_width >= scroll_target {
                    byte_offset = idx;
                    break;
                }
                skipped_width += grapheme.width();
            }
            // Return actual skipped width, not target (handles wide graphemes correctly)
            (draft[byte_offset..].to_string(), skipped_width as u16)
        } else {
            (draft.to_string(), 0u16)
        }
    } else if mode == InputMode::Command
        && let Some(cmd) = &command_line
    {
        let cursor_display_pos = cmd.width();
        if cursor_display_pos >= visible_content_width {
            let scroll_target = cursor_display_pos - visible_content_width + 1;
            let mut byte_offset = 0;
            let mut skipped_width = 0;
            for (idx, grapheme) in cmd.grapheme_indices(true) {
                if skipped_width >= scroll_target {
                    byte_offset = idx;
                    break;
                }
                skipped_width += grapheme.width();
            }
            // Return actual skipped width, not target (handles wide graphemes correctly)
            (cmd[byte_offset..].to_string(), skipped_width as u16)
        } else {
            (cmd.to_string(), 0u16)
        }
    } else {
        (
            match mode {
                InputMode::Insert | InputMode::Normal | InputMode::ModelSelect => {
                    app.draft_text().to_string()
                }
                InputMode::Command => command_line
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            },
            0u16,
        )
    };

    // Rebuild input_content with potentially scrolled text
    let input_content = match mode {
        InputMode::Insert | InputMode::Normal | InputMode::ModelSelect => vec![
            Span::styled(
                format!(" {prompt_char} "),
                Style::default().fg(colors::PRIMARY),
            ),
            Span::styled(display_text, Style::default().fg(colors::TEXT_PRIMARY)),
        ],
        InputMode::Command => {
            vec![
                Span::styled(" / ", Style::default().fg(colors::YELLOW)),
                Span::styled(display_text, Style::default().fg(colors::TEXT_PRIMARY)),
            ]
        }
    };

    let input = Paragraph::new(Line::from(input_content)).block(
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

    // Show cursor in insert mode
    if mode == InputMode::Insert {
        // Calculate cursor position using display width (handles Unicode properly)
        let cursor_index = app.draft_cursor_byte_index();
        let text_before_cursor = &app.draft_text()[..cursor_index];
        let cursor_display_pos = text_before_cursor.width() as u16;
        // Subtract scroll offset to get visible position
        let cursor_x = area.x + 4 + cursor_display_pos.saturating_sub(horizontal_scroll);
        let cursor_y = area.y + 2;
        frame.set_cursor_position((cursor_x, cursor_y));
    } else if mode == InputMode::Command {
        let Some(command_line) = command_line else {
            return;
        };
        let cursor_display_pos = command_line.width() as u16;
        let cursor_x = area.x + 4 + cursor_display_pos.saturating_sub(horizontal_scroll);
        let cursor_y = area.y + 2;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

pub(crate) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (status_text, status_style) = if let Some(msg) = app.status_message() {
        let lower = msg.to_ascii_lowercase();
        let is_error = lower.contains("error")
            || lower.contains("failed")
            || lower.contains("no api key")
            || lower.contains("cannot")
            || lower.contains("invalid")
            || lower.contains("unauthorized")
            || lower.contains("auth ");
        let style = if is_error {
            Style::default().fg(colors::RED)
        } else {
            Style::default().fg(colors::YELLOW)
        };
        (msg.to_string(), style)
    } else if app.is_loading() {
        let spinner = spinner_frame(app.tick_count());
        (
            format!("{spinner} Processing request..."),
            Style::default().fg(colors::PRIMARY),
        )
    } else if app.current_api_key().is_some() {
        (
            format!("● {} │ {}", app.provider().display_name(), app.model()),
            Style::default().fg(colors::GREEN),
        )
    } else {
        (
            format!("○ No API key │ Set {}", app.provider().env_var()),
            Style::default().fg(colors::RED),
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

fn draw_command_palette(frame: &mut Frame, _app: &App) {
    let area = frame.area();

    // Center the palette
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 10;

    let palette_area = Rect {
        x: area.x + (area.width.saturating_sub(palette_width) / 2),
        y: area.y + (area.height / 3),
        width: palette_width,
        height: palette_height,
    };

    // Clear background
    frame.render_widget(Clear, palette_area);

    let commands = vec![
        ("q, quit", "Exit the application"),
        ("clear", "Clear conversation history"),
        ("model <name>", "Change the model"),
        ("p, provider <name>", "Switch provider (claude/gpt)"),
        ("screen", "Toggle fullscreen/inline mode"),
        ("help", "Show available commands"),
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
            .title(Line::from(vec![Span::styled(
                " Commands ",
                Style::default()
                    .fg(colors::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )])),
    );

    frame.render_widget(palette, palette_area);
}

pub fn draw_model_selector(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let selected_index = app.model_select_index().unwrap_or(0);

    // Center the selector over the input area
    let selector_width = 60.min(area.width.saturating_sub(4)).max(40);
    let content_width = selector_width.saturating_sub(4).max(1) as usize; // borders + padding

    let divider = Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(colors::PRIMARY_DIM),
    ));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(divider);
    lines.push(Line::from(""));

    let models = PredefinedModel::all();
    let mut row_index = 0usize;
    let mut push_row = |label: &str, selected: bool, muted: bool, tag: Option<(&str, Style)>| {
        row_index += 1;
        let prefix = if selected { "▸" } else { " " };
        let left = format!(" {} {:>2}  {}", prefix, row_index, label);
        let left_width = left.width();
        let (right_text, right_style) = tag.unwrap_or(("", Style::default()));
        let right_width = right_text.width();
        let gap = if right_text.is_empty() { 0 } else { 2 };
        let filler = content_width.saturating_sub(left_width + right_width + gap);

        let bg = if selected {
            Some(colors::BG_HIGHLIGHT)
        } else {
            None
        };
        let mut left_style = if selected {
            Style::default()
                .fg(colors::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if muted {
            Style::default().fg(colors::TEXT_MUTED)
        } else {
            Style::default().fg(colors::TEXT_SECONDARY)
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
                .fg(colors::PEACH)
                .add_modifier(Modifier::BOLD),
        )),
    );

    if matches!(lines.last(), Some(line) if line.width() == 0) {
        lines.pop();
    }

    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(colors::PRIMARY_DIM),
    )));
    lines.push(Line::from(vec![
        Span::styled("  ↑↓", styles::key_highlight()),
        Span::styled(" select  ", styles::key_hint()),
        Span::styled("Enter", styles::key_highlight()),
        Span::styled(" confirm  ", styles::key_hint()),
        Span::styled("Esc", styles::key_highlight()),
        Span::styled(" cancel", styles::key_hint()),
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
        .border_style(Style::default().fg(colors::PRIMARY))
        .style(Style::default().bg(colors::BG_PANEL))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(
            " Select Model ",
            Style::default()
                .fg(colors::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )]));

    let selector = Paragraph::new(lines).block(block);

    frame.render_widget(selector, selector_area);
}

fn draw_tool_approval_prompt(frame: &mut Frame, app: &App) {
    let Some(requests) = app.tool_approval_requests() else {
        return;
    };
    let selected = app.tool_approval_selected().unwrap_or(&[]);
    let cursor = app.tool_approval_cursor().unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Tool approval required ",
        Style::default()
            .fg(colors::TEXT_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let max_width = frame.area().width.saturating_sub(6).min(80).max(20) as usize;

    for (i, req) in requests.iter().enumerate() {
        let is_selected = selected.get(i).copied().unwrap_or(false);
        let pointer = if i == cursor { ">" } else { " " };
        let checkbox = if is_selected { "[x]" } else { "[ ]" };
        let risk_label = format!("{:?}", req.risk_level).to_uppercase();
        let risk_style = match risk_label.as_str() {
            "HIGH" => Style::default().fg(colors::ERROR).add_modifier(Modifier::BOLD),
            "MEDIUM" => Style::default().fg(colors::WARNING).add_modifier(Modifier::BOLD),
            _ => Style::default().fg(colors::SUCCESS).add_modifier(Modifier::BOLD),
        };
        let name_style = if i == cursor {
            Style::default()
                .fg(colors::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colors::TEXT_PRIMARY)
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{pointer} {checkbox} "),
                Style::default().fg(colors::TEXT_MUTED),
            ),
            Span::styled(req.tool_name.clone(), name_style),
            Span::raw(" "),
            Span::styled(risk_label, risk_style),
        ]));

        if !req.summary.trim().is_empty() {
            let summary = truncate_with_ellipsis(&req.summary, max_width.saturating_sub(6));
            lines.push(Line::from(Span::styled(
                format!("    {summary}"),
                Style::default().fg(colors::TEXT_MUTED),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("A", styles::key_highlight()),
        Span::styled(" approve all  ", styles::key_hint()),
        Span::styled("D", styles::key_highlight()),
        Span::styled(" deny all  ", styles::key_hint()),
        Span::styled("Space", styles::key_highlight()),
        Span::styled(" toggle  ", styles::key_hint()),
        Span::styled("Enter", styles::key_highlight()),
        Span::styled(" confirm selected", styles::key_hint()),
    ]));

    let content_width = lines.iter().map(|line| line.width()).max().unwrap_or(10) as u16;
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

fn draw_tool_recovery_prompt(frame: &mut Frame, app: &App) {
    let Some(calls) = app.tool_recovery_calls() else {
        return;
    };
    let results = app.tool_recovery_results().unwrap_or(&[]);

    let mut results_map: std::collections::HashMap<&str, &ToolResult> =
        std::collections::HashMap::new();
    for result in results {
        results_map.insert(result.tool_call_id.as_str(), result);
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Tool recovery detected ",
        Style::default()
            .fg(colors::TEXT_PRIMARY)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " Tools will not be re-run.",
        Style::default().fg(colors::TEXT_MUTED),
    )));
    lines.push(Line::from(""));

    for call in calls {
        let (icon, style) = if let Some(result) = results_map.get(call.id.as_str()) {
            if result.is_error {
                (
                    "✗",
                    Style::default()
                        .fg(colors::ERROR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "✓",
                    Style::default()
                        .fg(colors::SUCCESS)
                        .add_modifier(Modifier::BOLD),
                )
            }
        } else {
            ("•", Style::default().fg(colors::TEXT_MUTED))
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} "), style),
            Span::styled(
                format!("{} ({})", call.name, call.id),
                Style::default().fg(colors::TEXT_MUTED),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("R", styles::key_highlight()),
        Span::styled(" resume with recovered results  ", styles::key_hint()),
        Span::styled("D", styles::key_highlight()),
        Span::styled(" discard results", styles::key_hint()),
    ]));

    let content_width = lines.iter().map(|line| line.width()).max().unwrap_or(10) as u16;
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

fn create_welcome_screen(app: &App) -> Paragraph<'static> {
    let logo = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  ╭─────────────────────────────────────╮",
            Style::default().fg(colors::PRIMARY_DIM),
        )]),
        Line::from(vec![
            Span::styled("  │", Style::default().fg(colors::PRIMARY_DIM)),
            Span::styled(
                "     ✨ LLM API Harness ✨              ",
                Style::default()
                    .fg(colors::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│", Style::default().fg(colors::PRIMARY_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  │", Style::default().fg(colors::PRIMARY_DIM)),
            Span::styled(
                "     Your AI Assistant Interface       ",
                Style::default().fg(colors::TEXT_SECONDARY),
            ),
            Span::styled("│", Style::default().fg(colors::PRIMARY_DIM)),
        ]),
        Line::from(vec![Span::styled(
            "  ╰─────────────────────────────────────╯",
            Style::default().fg(colors::PRIMARY_DIM),
        )]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Quick Start:",
            Style::default()
                .fg(colors::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "    i",
                Style::default()
                    .fg(colors::GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Enter insert mode to type",
                Style::default().fg(colors::TEXT_SECONDARY),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Enter",
                Style::default()
                    .fg(colors::GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Send your message",
                Style::default().fg(colors::TEXT_SECONDARY),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Esc",
                Style::default()
                    .fg(colors::YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Return to normal mode",
                Style::default().fg(colors::TEXT_SECONDARY),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    /",
                Style::default()
                    .fg(colors::PEACH)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Open command palette",
                Style::default().fg(colors::TEXT_SECONDARY),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    q",
                Style::default()
                    .fg(colors::RED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Quit", Style::default().fg(colors::TEXT_SECONDARY)),
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
                    if is_current { "  ● " } else { "  ○ " },
                    Style::default().fg(if is_current {
                        colors::GREEN
                    } else {
                        colors::TEXT_MUTED
                    }),
                ),
                Span::styled(
                    provider.display_name(),
                    Style::default().fg(if is_current {
                        colors::GREEN
                    } else {
                        colors::TEXT_SECONDARY
                    }),
                ),
                Span::styled(" - Ready", Style::default().fg(colors::TEXT_MUTED)),
                if is_current {
                    Span::styled(" (active)", Style::default().fg(colors::GREEN))
                } else {
                    Span::styled("", Style::default())
                },
            ])
        } else {
            Line::from(vec![
                Span::styled("  ○ ", Style::default().fg(colors::TEXT_MUTED)),
                Span::styled(
                    provider.display_name(),
                    Style::default().fg(colors::TEXT_MUTED),
                ),
                Span::styled(" - Set ", Style::default().fg(colors::TEXT_MUTED)),
                Span::styled(provider.env_var(), Style::default().fg(colors::PEACH)),
            ])
        };
        lines.push(status_line);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Tip: ", Style::default().fg(colors::TEXT_MUTED)),
        Span::styled("/p claude", Style::default().fg(colors::PEACH)),
        Span::styled(" or ", Style::default().fg(colors::TEXT_MUTED)),
        Span::styled("/p gpt", Style::default().fg(colors::PEACH)),
        Span::styled(
            " to switch providers",
            Style::default().fg(colors::TEXT_MUTED),
        ),
    ]));

    Paragraph::new(lines).alignment(Alignment::Left)
}

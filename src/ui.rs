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
use unicode_width::UnicodeWidthStr;

use crate::app::{App, InputMode};
use crate::context_infinity::ContextUsageStatus;
use crate::markdown::render_markdown;
use crate::message::Message;
use crate::provider::Provider;
use crate::theme::{colors, spinner_frame, styles};

/// Main draw function
pub fn draw(frame: &mut Frame, app: &mut App) {
    // Clear with background color
    let bg_block = Block::default().style(Style::default().bg(colors::BG_DARK));
    frame.render_widget(bg_block, frame.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(1),    // Messages
            Constraint::Length(5), // Input
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_input(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    // Draw command palette if in command mode
    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app);
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
                "●",
                "System",
                Style::default()
                    .fg(colors::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Message::User(_) => ("▶", "You", styles::user_name()),
            Message::Assistant(m) => ("◆", m.provider().display_name(), styles::assistant_name()),
        };

        let header_line = Line::from(vec![
            Span::styled(format!(" {icon} "), name_style),
            Span::styled(name, name_style),
        ]);
        lines.push(header_line);
        lines.push(Line::from("")); // Space after header

        // Message content - render as markdown
        let content_style = match msg {
            Message::System(_) => Style::default().fg(colors::TEXT_MUTED),
            Message::User(_) => Style::default().fg(colors::TEXT_PRIMARY),
            Message::Assistant(_) => Style::default().fg(colors::TEXT_SECONDARY),
        };

        let rendered = render_markdown(msg.content(), content_style);
        lines.extend(rendered);
    }

    // Render complete messages from display items
    for item in app.display_items() {
        let msg = match item {
            crate::app::DisplayItem::History(id) => app.history().get_entry(*id).message(),
            crate::app::DisplayItem::Local(msg) => msg,
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

    // Render scrollbar
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"))
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .style(Style::default().fg(colors::TEXT_MUTED));

    let mut scrollbar_state =
        ScrollbarState::new(total_lines as usize).position(scroll_offset as usize);

    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scrollbar_state,
    );
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

pub(crate) fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let mode = app.input_mode();
    let command_line = if mode == InputMode::Command {
        app.command_text()
    } else {
        None
    };

    let (mode_text, mode_style, border_style, prompt_char) = match mode {
        InputMode::Normal => (
            " NORMAL ",
            styles::mode_normal(),
            Style::default().fg(colors::TEXT_MUTED),
            "│",
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
            ":",
        ),
    };

    // Build input content with prompt
    let input_content = match mode {
        InputMode::Insert | InputMode::Normal => vec![
            Span::styled(
                format!(" {prompt_char} "),
                Style::default().fg(colors::PRIMARY),
            ),
            Span::styled(app.draft_text(), Style::default().fg(colors::TEXT_PRIMARY)),
        ],
        InputMode::Command => {
            let Some(command_line) = command_line else {
                return;
            };
            vec![
                Span::styled(" : ", Style::default().fg(colors::YELLOW)),
                Span::styled(command_line, Style::default().fg(colors::TEXT_PRIMARY)),
            ]
        }
    };

    // Key hints based on mode
    let hints = match mode {
        InputMode::Normal => vec![
            Span::styled("i", styles::key_highlight()),
            Span::styled(" insert  ", styles::key_hint()),
            Span::styled(":", styles::key_highlight()),
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
    };

    let input = Paragraph::new(Line::from(input_content)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(vec![Span::styled(mode_text, mode_style)]))
            .title_bottom(Line::from(hints).alignment(Alignment::Right))
            .padding(Padding::vertical(1)),
    );

    frame.render_widget(input, area);

    // Show cursor in insert mode
    if mode == InputMode::Insert {
        // Calculate cursor position using display width (handles Unicode properly)
        let text_before_cursor: String =
            app.draft_text().chars().take(app.draft_cursor()).collect();
        let cursor_x = area.x + 4 + text_before_cursor.width() as u16;
        let cursor_y = area.y + 2;
        frame.set_cursor_position((cursor_x, cursor_y));
    } else if mode == InputMode::Command {
        let Some(command_line) = command_line else {
            return;
        };
        let cursor_x = area.x + 4 + command_line.width() as u16;
        let cursor_y = area.y + 2;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

pub(crate) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let usage_status = app.context_usage_status();
    let (usage, needs_summary) = match &usage_status {
        ContextUsageStatus::Ready(usage) => (usage, false),
        ContextUsageStatus::NeedsSummarization { usage, .. } => (usage, true),
    };
    let usage_str = if needs_summary {
        format!("{} !", usage.format_compact())
    } else {
        usage.format_compact()
    };
    let usage_color = if needs_summary {
        colors::RED
    } else {
        match usage.severity() {
            0 => colors::GREEN,  // < 70%
            1 => colors::YELLOW, // 70-90%
            _ => colors::RED,    // > 90%
        }
    };

    let (status_text, status_style) = if let Some(msg) = app.status_message() {
        (msg.to_string(), Style::default().fg(colors::YELLOW))
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

    // Build status line with context usage on the right
    let status = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(status_text, status_style),
    ]));

    // Context usage indicator on the right side
    let usage_width = usage_str.len() as u16 + 2;
    let status_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(usage_width),
        height: area.height,
    };
    let usage_area = Rect {
        x: area.x + area.width.saturating_sub(usage_width),
        y: area.y,
        width: usage_width,
        height: area.height,
    };

    frame.render_widget(status, status_area);

    let usage_widget = Paragraph::new(Line::from(vec![
        Span::styled(usage_str, Style::default().fg(usage_color)),
        Span::raw(" "),
    ]))
    .alignment(ratatui::layout::Alignment::Right);

    frame.render_widget(usage_widget, usage_area);
}

fn draw_command_palette(frame: &mut Frame, _app: &App) {
    let area = frame.area();

    // Center the palette
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 9;

    let palette_area = Rect {
        x: (area.width - palette_width) / 2,
        y: area.height / 3,
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
        ("help", "Show available commands"),
    ];

    let mut lines: Vec<Line> = vec![Line::from("")];

    for (cmd, desc) in commands {
        lines.push(Line::from(vec![
            Span::styled(format!("  :{cmd}"), Style::default().fg(colors::PEACH)),
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
                "    :",
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
        Span::styled(":p claude", Style::default().fg(colors::PEACH)),
        Span::styled(" or ", Style::default().fg(colors::TEXT_MUTED)),
        Span::styled(":p gpt", Style::default().fg(colors::PEACH)),
        Span::styled(
            " to switch providers",
            Style::default().fg(colors::TEXT_MUTED),
        ),
    ]));

    Paragraph::new(lines).alignment(Alignment::Left)
}

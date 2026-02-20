use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::repeat_n;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use forge_core::{DisplayItem, sanitize_display_text};
use forge_engine::App;
use forge_types::ui::UiOptions;
use forge_types::{Message, Provider, ToolResultOutcome, sanitize_terminal_text};

use crate::diff_render::render_tool_result_lines;
use crate::markdown::{render_markdown, render_markdown_preserve_newlines};
use crate::shared::{
    ToolCallStatus, ToolCallStatusKind, collect_tool_statuses, message_header_parts,
    provider_color, wrapped_line_count_exact, wrapped_line_rows,
};
use crate::theme::{Glyphs, Palette, spinner_frame};
use crate::tool_display;
use crate::tool_result_summary::{ToolCallMeta, ToolResultRender, tool_result_render_decision};

#[derive(Default)]
struct MessageLinesCache {
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

pub(crate) fn draw_messages(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    glyphs: &Glyphs,
) {
    let messages_block = Block::default().padding(Padding::horizontal(1));

    if app.is_empty() && app.display_items().is_empty() {
        app.update_scroll_max(0);
        MESSAGE_CACHE.with(|cache| cache.borrow_mut().invalidate());
        let ready = Paragraph::new("Ready")
            .style(Style::default().fg(palette.text_muted))
            .alignment(Alignment::Center)
            .block(messages_block);
        let center_y = area.height / 2;
        let ready_area = Rect {
            x: area.x,
            y: area.y + center_y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(ready, ready_area);
        return;
    }

    let inner = messages_block.inner(area);
    let display_version = app.display_version();
    let options = app.ui_options();
    let cache_width = inner.width.max(1);
    let cache_key = MessageCacheKey::new(display_version, cache_width, options);

    let tool_statuses = collect_tool_statuses(app, 80);
    let is_streaming = matches!(
        app.streaming_access(),
        forge_engine::StreamingAccess::Active(_)
    );
    let has_tool_activity = tool_statuses.is_some();
    let has_dynamic = is_streaming || has_tool_activity;
    let static_message_count = app.display_items().len();

    let (mut lines, mut total_rows) = MESSAGE_CACHE.with(|cache| {
        let cache_ref = cache.borrow();
        if let Some((cached_lines, cached_total)) = cache_ref.get(cache_key) {
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

    let max_rows = u16::MAX as usize;

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

        let mut scrollbar_state =
            ScrollbarState::new(max_scroll as usize).position(scroll_offset as usize);

        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn build_message_lines(
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
    width: u16,
) -> (Vec<Line<'static>>, usize) {
    let mut lines: Vec<Line> = Vec::new();
    let mut msg_count = 0;
    let mut buffered_tool_uses: HashMap<&str, (&Message, ToolCallMeta)> = HashMap::new();

    let mut last_was_thinking = false;

    for item in app.display_items() {
        let msg = match item {
            DisplayItem::History(id) => app.history().get_entry(*id).message(),
            DisplayItem::Local(msg) => msg,
        };
        match msg {
            Message::ToolUse(call) => {
                if app.is_tool_hidden(&call.name) {
                    continue;
                }
                buffered_tool_uses.insert(call.id.as_str(), (msg, ToolCallMeta::from_call(call)));
            }
            Message::ToolResult(result) => {
                if app.is_tool_hidden(&result.tool_name) {
                    continue;
                }
                if let Some((tool_use_msg, meta)) =
                    buffered_tool_uses.remove(result.tool_call_id.as_str())
                {
                    render_message_static(
                        tool_use_msg,
                        RenderMessageStaticCtx {
                            lines: &mut lines,
                            msg_count: &mut msg_count,
                            palette,
                            glyphs,
                            tool_call_meta: None,
                            max_width: width,
                            follows_thinking: false,
                        },
                    );
                    render_message_static(
                        msg,
                        RenderMessageStaticCtx {
                            lines: &mut lines,
                            msg_count: &mut msg_count,
                            palette,
                            glyphs,
                            tool_call_meta: Some(&meta),
                            max_width: width,
                            follows_thinking: false,
                        },
                    );
                } else {
                    render_message_static(
                        msg,
                        RenderMessageStaticCtx {
                            lines: &mut lines,
                            msg_count: &mut msg_count,
                            palette,
                            glyphs,
                            tool_call_meta: None,
                            max_width: width,
                            follows_thinking: false,
                        },
                    );
                }
                last_was_thinking = false;
            }
            Message::Thinking(_) => {
                if app.ui_options().show_thinking {
                    render_message_static(
                        msg,
                        RenderMessageStaticCtx {
                            lines: &mut lines,
                            msg_count: &mut msg_count,
                            palette,
                            glyphs,
                            tool_call_meta: None,
                            max_width: width,
                            follows_thinking: false,
                        },
                    );
                    last_was_thinking = true;
                }
            }
            _ => {
                let follows = last_was_thinking && matches!(msg, Message::Assistant(_));
                render_message_static(
                    msg,
                    RenderMessageStaticCtx {
                        lines: &mut lines,
                        msg_count: &mut msg_count,
                        palette,
                        glyphs,
                        tool_call_meta: None,
                        max_width: width,
                        follows_thinking: follows,
                    },
                );
                last_was_thinking = false;
            }
        }
    }

    for (_, (msg, _)) in buffered_tool_uses {
        render_message_static(
            msg,
            RenderMessageStaticCtx {
                lines: &mut lines,
                msg_count: &mut msg_count,
                palette,
                glyphs,
                tool_call_meta: None,
                max_width: width,
                follows_thinking: false,
            },
        );
    }

    let total_rows = wrapped_line_count_exact(&lines, width);

    (lines, total_rows)
}

const TOOL_OUTPUT_WINDOW_LINES: usize = 5;

fn tool_output_window(output_lines: Option<&[String]>, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = output_lines
        .unwrap_or(&[])
        .iter()
        .filter(|line| !line.starts_with("▶ ") && !line.starts_with("✓ Tool completed"))
        .cloned()
        .collect();

    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    if lines.len() < max_lines {
        lines.extend(repeat_n(String::new(), max_lines - lines.len()));
    }

    lines
}

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

    if let forge_engine::StreamingAccess::Active(streaming) = app.streaming_access() {
        if has_static {
            lines.push(Line::from(""));
        }

        let icon = glyphs.assistant;
        let provider = streaming.provider();
        let color = provider_color(provider, palette);
        let name_style = Style::default().fg(color);

        let show_thinking = app.ui_options().show_thinking;
        let has_thinking = show_thinking
            && matches!(
                provider,
                Provider::Claude | Provider::Gemini | Provider::OpenAI
            )
            && !streaming.thinking().is_empty();
        let is_empty = streaming.content().is_empty();
        let indent = "   ";

        if has_thinking {
            let header_tail = if is_empty {
                " Thinking..."
            } else {
                " Thinking"
            };

            let spinner = spinner_frame(app.tick_count(), app.ui_options());
            let header_spans = vec![
                Span::styled(format!(" {spinner} "), Style::default().fg(color)),
                Span::styled(
                    header_tail,
                    Style::default()
                        .fg(palette.text_muted)
                        .add_modifier(Modifier::ITALIC),
                ),
            ];
            lines.push(Line::from(header_spans));

            let thinking_style = Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::ITALIC);
            let thinking = sanitize_display_text(streaming.thinking());
            let mut rendered_thinking =
                render_markdown_preserve_newlines(&thinking, thinking_style, palette, width);

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
                    Span::styled(format!(" {spinner} "), Style::default().fg(color)),
                    Span::styled(" Thinking...", Style::default().fg(palette.text_muted)),
                ]));
            }
        } else {
            let content_style = Style::default().fg(palette.text_secondary);
            let content = sanitize_display_text(streaming.content());
            let mut rendered = render_markdown(&content, content_style, palette, width);

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
        if has_static
            || matches!(
                app.streaming_access(),
                forge_engine::StreamingAccess::Active(_)
            )
        {
            lines.push(Line::from(""));
        }

        let mut rendered_shell_view = false;
        if let forge_engine::ToolLoopAccess::Active {
            calls: tl_calls,
            output_lines: tl_output,
            execution: forge_engine::ToolLoopExecution::Active { current_call_id },
            ..
        } = app.tool_loop_access()
            && let Some(call) = tl_calls.iter().find(|c| c.id == current_call_id)
        {
            let canonical = tool_display::canonical_tool_name(&call.name);
            if matches!(canonical.as_ref(), "Run" | "Pwsh") {
                rendered_shell_view = true;
                let spinner = spinner_frame(app.tick_count(), app.ui_options());
                let display = tool_display::format_tool_call_compact(&call.name, &call.arguments);
                let display = sanitize_display_text(&display);
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {spinner} "),
                        Style::default()
                            .fg(palette.primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        display,
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                let current_output = tl_output.get(current_call_id).map(Vec::as_slice);
                let output_window = tool_output_window(current_output, TOOL_OUTPUT_WINDOW_LINES);
                let connector_style = Style::default().fg(palette.text_muted);
                let output_style = Style::default().fg(palette.text_secondary);
                for (index, line) in output_window.iter().enumerate() {
                    let safe_line = sanitize_display_text(line);
                    if index == 0 {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {} ", glyphs.tree_connector), connector_style),
                            Span::styled(safe_line, output_style),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw("   "),
                            Span::styled(safe_line, output_style),
                        ]));
                    }
                }
            }
        }

        if !rendered_shell_view {
            let spinner = spinner_frame(app.tick_count(), app.ui_options());
            let approval_pending = !matches!(
                app.tool_approval_access(),
                forge_engine::ToolApprovalAccess::Inactive
            );
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

                let name = sanitize_display_text(&status.name);
                lines.push(Line::from(vec![
                    Span::styled(format!("  {icon} "), style),
                    Span::styled(
                        format!("{name} [{label}]"),
                        Style::default().fg(palette.text_muted),
                    ),
                ]));

                if let Some(reason) = status.reason.as_ref() {
                    lines.push(Line::from(Span::styled(
                        format!("    ↳ {reason}"),
                        Style::default().fg(palette.text_muted),
                    )));
                }

                let per_call_output = match app.tool_loop_access() {
                    forge_engine::ToolLoopAccess::Active { output_lines, .. } => {
                        output_lines.get(&status.id).map(Vec::as_slice)
                    }
                    forge_engine::ToolLoopAccess::Inactive => None,
                };
                if let Some(output_lines) = per_call_output
                    && !output_lines.is_empty()
                {
                    let is_running = matches!(status.status, ToolCallStatusKind::Running);
                    let output_style = Style::default().fg(palette.text_secondary);
                    let connector = glyphs.tree_connector;

                    if is_running {
                        let window =
                            tool_output_window(Some(output_lines), TOOL_OUTPUT_WINDOW_LINES);
                        for (i, line) in window.iter().enumerate() {
                            if line.is_empty() {
                                continue;
                            }
                            let safe_line = sanitize_display_text(line);
                            if i == 0 {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        format!("    {connector} "),
                                        Style::default().fg(palette.text_muted),
                                    ),
                                    Span::styled(safe_line, output_style),
                                ]));
                            } else {
                                lines.push(Line::from(vec![
                                    Span::raw("      "),
                                    Span::styled(safe_line, output_style),
                                ]));
                            }
                        }
                    } else {
                        let last_line = output_lines.iter().rev().find(|l| {
                            !l.starts_with("▶ ") && !l.starts_with("✓ ") && !l.trim().is_empty()
                        });
                        if let Some(line) = last_line {
                            let safe_line = sanitize_display_text(line);
                            lines.push(Line::from(vec![
                                Span::styled(
                                    format!("    {connector} "),
                                    Style::default().fg(palette.text_muted),
                                ),
                                Span::styled(safe_line, output_style),
                            ]));
                        }
                    }
                }
            }
        }
    }

    let total_rows = wrapped_line_count_exact(&lines, width);

    (lines, total_rows)
}

struct RenderMessageStaticCtx<'a> {
    lines: &'a mut Vec<Line<'static>>,
    msg_count: &'a mut usize,
    palette: &'a Palette,
    glyphs: &'a Glyphs,
    tool_call_meta: Option<&'a ToolCallMeta>,
    max_width: u16,
    follows_thinking: bool,
}

fn render_message_static(msg: &Message, ctx: RenderMessageStaticCtx<'_>) {
    let RenderMessageStaticCtx {
        lines,
        msg_count,
        palette,
        glyphs,
        tool_call_meta,
        max_width,
        follows_thinking,
    } = ctx;

    let is_tool_result = matches!(msg, Message::ToolResult(_));
    if *msg_count > 0 && !is_tool_result {
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    let (icon, name, name_style) = message_header_parts(msg, palette, glyphs);
    match msg {
        Message::User(_) => {
            let content_style = Style::default().fg(palette.text_primary);
            let content = sanitize_terminal_text(msg.display_content()).into_owned();
            let mut rendered = render_markdown(&content, content_style, palette, max_width);

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
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), name_style),
                Span::styled(name, name_style),
            ]));
        }
        Message::ToolResult(result) => {
            let content = sanitize_display_text(&result.content);
            let outcome = result.outcome();
            let is_error = matches!(outcome, ToolResultOutcome::Error);

            match tool_result_render_decision(tool_call_meta, &content, outcome, 80) {
                ToolResultRender::Full { diff_aware } => {
                    let content_style = if is_error {
                        Style::default().fg(palette.error)
                    } else {
                        Style::default().fg(palette.text_secondary)
                    };
                    if diff_aware {
                        lines.extend(render_tool_result_lines(
                            &content,
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
                    let style = if is_error {
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
            let content = sanitize_display_text(msg.content());
            let mut rendered = render_markdown(&content, content_style, palette, max_width);

            if rendered.is_empty() {
                if follows_thinking && matches!(msg, Message::Assistant(_)) {
                    lines.push(Line::from(Span::raw("   ")));
                } else {
                    lines.push(Line::from(vec![Span::styled(
                        format!(" {icon} "),
                        name_style,
                    )]));
                }
            } else if follows_thinking && matches!(msg, Message::Assistant(_)) {
                let first_line = &mut rendered[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }
                for line in &mut rendered {
                    line.spans.insert(0, Span::raw("   "));
                }
                lines.extend(rendered);
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
        Message::Thinking(_) => {
            let content_style = Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::ITALIC);
            let content = sanitize_display_text(msg.content());
            let mut rendered =
                render_markdown_preserve_newlines(&content, content_style, palette, max_width);

            if rendered.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} {name}"),
                    name_style,
                )]));
            } else {
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} {name}"),
                    name_style,
                )]));
                let first_line = &mut rendered[0];
                if !first_line.spans.is_empty() && first_line.spans[0].content == "    " {
                    first_line.spans.remove(0);
                }
                for line in &mut rendered {
                    line.spans.insert(0, Span::raw("   "));
                }
                lines.extend(rendered);
            }
        }
    }
}

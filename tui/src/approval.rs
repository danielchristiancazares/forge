use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
};

use forge_core::sanitize_display_text;
use forge_engine::App;
use forge_types::ToolResult;

use crate::shared::collect_approval_view;
use crate::theme::{Glyphs, Palette, styles};

pub(crate) fn draw_plan_approval_prompt(frame: &mut Frame, app: &App, palette: &Palette) {
    let Some(kind_label) = app.plan_approval_kind() else {
        return;
    };
    let rendered = app.plan_approval_rendered().unwrap_or_default();

    let title = if kind_label == "create" {
        " Plan approval required "
    } else {
        " Plan edit approval required "
    };

    let max_content_width = frame.area().width.saturating_sub(8).max(20) as usize;

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            title,
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for text_line in rendered.lines() {
        let sanitized = sanitize_display_text(text_line);
        let display_line = if sanitized.len() > max_content_width {
            sanitized[..sanitized.floor_char_boundary(max_content_width)].to_string()
        } else {
            sanitized
        };
        lines.push(Line::from(Span::styled(
            format!(" {display_line}"),
            Style::default().fg(palette.text_secondary),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Enter/a", styles::key_highlight(palette)),
        Span::styled(" approve  ", styles::key_hint(palette)),
        Span::styled("d/Esc", styles::key_highlight(palette)),
        Span::styled(" reject  ", styles::key_hint(palette)),
        Span::styled("↑↓", styles::key_highlight(palette)),
        Span::styled(" scroll", styles::key_hint(palette)),
    ]));

    let content_width = lines
        .iter()
        .map(ratatui::prelude::Line::width)
        .max()
        .unwrap_or(10) as u16;
    let content_width = content_width.min(frame.area().width.saturating_sub(4));

    let max_height = frame.area().height.saturating_sub(4);
    let content_height = (lines.len() as u16).min(max_height);

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

pub(crate) fn draw_tool_approval_prompt(frame: &mut Frame, app: &App, palette: &Palette) {
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
        let is_selected = selected
            .get(i)
            .copied()
            .map(forge_engine::ApprovalSelection::is_approved)
            .unwrap_or(false);
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

        for warning in &item.homoglyph_warnings {
            lines.push(Line::from(vec![
                Span::styled(
                    "    ⚠ ",
                    Style::default()
                        .fg(palette.warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(warning.clone(), Style::default().fg(palette.warning)),
            ]));
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

    let area = frame.area();
    let max_height = area.height.saturating_sub(2);
    let height = content_height.saturating_add(4).min(max_height);
    let width = content_width.saturating_add(4);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width) / 2),
        y: area.y + (area.height.saturating_sub(height) / 2),
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((view.scroll_offset as u16, 0)),
        rect,
    );
}

pub(crate) fn draw_tool_recovery_prompt(
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
                {
                    let safe_name = sanitize_display_text(&call.name);
                    let safe_id = sanitize_display_text(&call.id);
                    format!("{safe_name} ({safe_id})")
                },
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

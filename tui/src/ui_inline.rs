//! Inline TUI mode - minimal viewport for shell integration.

use ratatui::prelude::{Backend, Terminal};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget, Wrap},
};

use forge_engine::{App, DisplayItem, InputMode, Message};
use forge_types::sanitize_terminal_text;

use crate::diff_render::render_tool_result_lines;
use crate::shared::{
    ApprovalView, ToolCallStatus, ToolCallStatusKind, collect_approval_view, collect_tool_statuses,
    message_header_parts, tool_status_signature, wrapped_line_count,
};
use crate::theme::{Glyphs, Palette, glyphs, palette};
use crate::{draw_input, draw_model_selector, draw_status_delineator};

pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;

/// Height needed for the model selector overlay in inline mode.
/// Calculated as: inner content + borders + padding.
pub const INLINE_MODEL_SELECTOR_HEIGHT: u16 = 18;

/// Returns the viewport height needed for inline mode based on current input mode.
#[must_use]
pub fn inline_viewport_height(mode: InputMode) -> u16 {
    match mode {
        InputMode::ModelSelect => INLINE_MODEL_SELECTOR_HEIGHT,
        _ => INLINE_VIEWPORT_HEIGHT,
    }
}

pub fn clear_inline_viewport<B>(terminal: &mut Terminal<B>) -> Result<(), B::Error>
where
    B: Backend,
{
    terminal.draw(|frame| {
        let area = frame.area();
        frame.render_widget(Clear, area);
    })?;
    Ok(())
}

#[derive(Default)]
pub struct InlineOutput {
    next_display_index: usize,
    has_output: bool,
    last_tool_output_len: usize,
    last_tool_status_signature: Option<String>,
    last_approval_signature: Option<String>,
    last_recovery_active: bool,
}

impl InlineOutput {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_display_index: 0,
            has_output: false,
            last_tool_output_len: 0,
            last_tool_status_signature: None,
            last_approval_signature: None,
            last_recovery_active: false,
        }
    }

    pub fn reset(&mut self) {
        self.next_display_index = 0;
        self.has_output = false;
        self.last_tool_output_len = 0;
        self.last_tool_status_signature = None;
        self.last_approval_signature = None;
        self.last_recovery_active = false;
    }

    pub fn flush<B>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error>
    where
        B: Backend,
    {
        let options = app.ui_options();
        let palette = palette(options);
        let glyphs = glyphs(options);

        let items = app.display_items();
        let mut lines: Vec<Line> = Vec::new();
        let mut msg_count = usize::from(self.has_output);

        if self.next_display_index < items.len() {
            for item in &items[self.next_display_index..] {
                let msg = match item {
                    DisplayItem::History(id) => app.history().get_entry(*id).message(),
                    DisplayItem::Local(msg) => msg,
                };

                append_message_lines(&mut lines, msg, &mut msg_count, &palette, &glyphs);
            }

            self.next_display_index = items.len();
        }

        let tool_statuses = collect_tool_statuses(app, 80);
        let tool_signature = tool_status_signature(tool_statuses.as_deref());
        if tool_signature != self.last_tool_status_signature {
            if let Some(statuses) = tool_statuses.as_ref() {
                append_tool_status_lines(&mut lines, statuses, &glyphs);
            }
            self.last_tool_status_signature = tool_signature;
        }

        if let Some(output_lines) = app.tool_loop_output_lines() {
            if output_lines.len() > self.last_tool_output_len {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                if self.last_tool_output_len == 0 {
                    lines.push(Line::from("Tool output:"));
                }
                for line in &output_lines[self.last_tool_output_len..] {
                    let safe_line = sanitize_terminal_text(line);
                    lines.push(Line::from(format!("  {}", safe_line.as_ref())));
                }
                self.last_tool_output_len = output_lines.len();
            }
        } else {
            self.last_tool_output_len = 0;
        }

        let approval_view = collect_approval_view(app, 80);
        let approval_signature = approval_signature(approval_view.as_ref());
        if approval_signature != self.last_approval_signature {
            if let Some(view) = approval_view.as_ref() {
                append_approval_lines(&mut lines, view, &palette);
            }
            self.last_approval_signature = approval_signature;
        }

        let recovery_active = app.tool_recovery_calls().is_some();
        if recovery_active && !self.last_recovery_active {
            append_recovery_prompt(&mut lines, app, &palette);
        }
        self.last_recovery_active = recovery_active;

        if lines.is_empty() {
            return Ok(());
        }

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
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let options = app.ui_options();
    let palette = palette(options);
    let glyphs = glyphs(options);

    let input_height = match app.input_mode() {
        InputMode::Normal => 3,
        _ => INLINE_INPUT_HEIGHT,
    };

    // Show status delineator only when there's a status message
    let has_status = app.status_message().is_some();
    let status_height = u16::from(has_status);

    let total_height = input_height + status_height;
    let top_padding = area.height.saturating_sub(total_height);
    let content_area = Rect {
        x: area.x,
        y: area.y.saturating_add(top_padding),
        width: area.width,
        height: area.height.saturating_sub(top_padding),
    };

    let constraints: Vec<Constraint> = if has_status {
        vec![
            Constraint::Length(status_height),
            Constraint::Length(input_height),
        ]
    } else {
        vec![Constraint::Length(input_height)]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(content_area);

    if has_status {
        draw_status_delineator(frame, app, chunks[0], &palette);
        draw_input(frame, app, chunks[1], &palette, &glyphs);
    } else {
        draw_input(frame, app, chunks[0], &palette, &glyphs);
    }

    // Draw model selector overlay if in model select mode
    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app, &palette, &glyphs);
    }
}

fn append_message_lines(
    lines: &mut Vec<Line>,
    msg: &Message,
    msg_count: &mut usize,
    palette: &Palette,
    glyphs: &Glyphs,
) {
    if *msg_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    let (icon, name, name_style) = message_header_parts(msg, palette, glyphs);

    let header_line = Line::from(vec![
        Span::styled(format!(" {icon} "), name_style),
        Span::styled(name, name_style),
    ]);
    lines.push(header_line);
    lines.push(Line::from(""));

    // Message content - render based on type
    match msg {
        Message::ToolUse(_) => {
            // Compact format: args are in the header line, no body needed
        }
        Message::ToolResult(result) => {
            // Render result content with diff-aware coloring
            let content_style = if result.is_error {
                Style::default().fg(palette.error)
            } else {
                Style::default().fg(palette.text_secondary)
            };
            let content = sanitize_terminal_text(&result.content);
            lines.extend(render_tool_result_lines(
                content.as_ref(),
                content_style,
                palette,
                "    ",
            ));
        }
        _ => {
            // Regular messages
            let content_style = match msg {
                Message::User(_) => Style::default().fg(palette.text_primary),
                Message::Assistant(_) => Style::default().fg(palette.text_secondary),
                _ => Style::default().fg(palette.text_muted),
            };

            let content = sanitize_terminal_text(msg.content());
            for content_line in content.lines() {
                if content_line.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(content_line.to_string(), content_style),
                    ]));
                }
            }
        }
    }
}

fn append_tool_status_lines(lines: &mut Vec<Line>, statuses: &[ToolCallStatus], glyphs: &Glyphs) {
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Tool status:"));

    for status in statuses {
        let icon = match status.status {
            ToolCallStatusKind::Denied => glyphs.denied,
            ToolCallStatusKind::Error => glyphs.tool_result_err,
            ToolCallStatusKind::Ok => glyphs.tool_result_ok,
            ToolCallStatusKind::Running => glyphs.running,
            ToolCallStatusKind::Approval => glyphs.paused,
            ToolCallStatusKind::Pending => glyphs.bullet,
        };

        let name = sanitize_terminal_text(&status.name);
        let id = sanitize_terminal_text(&status.id);
        lines.push(Line::from(format!(
            "  {icon} {} ({})",
            name.as_ref(),
            id.as_ref()
        )));
        if let Some(reason) = status.reason.as_ref() {
            lines.push(Line::from(format!("     {reason}")));
        }
    }
}

fn approval_signature(view: Option<&ApprovalView>) -> Option<String> {
    let view = view?;
    let mut sig = format!("{}|{}|", view.items.len(), view.cursor);
    for flag in &view.selected {
        sig.push(if *flag { '1' } else { '0' });
    }
    if let Some(expanded) = view.expanded {
        sig.push('|');
        sig.push_str(&expanded.to_string());
    }
    Some(sig)
}

fn append_approval_lines(lines: &mut Vec<Line>, view: &ApprovalView, palette: &Palette) {
    let selected = &view.selected;
    let cursor = view.cursor;
    let any_selected = view.any_selected;

    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Tool approval required:"));

    for (i, item) in view.items.iter().enumerate() {
        let is_selected = selected.get(i).copied().unwrap_or(false);
        let pointer = if i == cursor { ">" } else { " " };
        let checkbox = if is_selected { "[x]" } else { "[ ]" };
        let risk = item.risk_label.as_str();
        let tool_name = item.tool_name.as_str();
        lines.push(Line::from(format!(
            " {pointer} {checkbox} {tool_name} ({risk})"
        )));
        if let Some(summary) = item.summary.as_ref() {
            lines.push(Line::from(format!("     {summary}")));
        }

        for line in &item.details {
            lines.push(Line::from(format!("       {line}")));
        }
    }

    // Submit and Deny buttons
    let submit_cursor = view.items.len();
    let deny_cursor = view.items.len() + 1;
    let submit_pointer = if cursor == submit_cursor { ">" } else { " " };
    let deny_pointer = if cursor == deny_cursor { ">" } else { " " };
    lines.push(Line::from(format!(
        " {submit_pointer} [ Approve selected ]    {deny_pointer} [ Deny All ]"
    )));

    if view.deny_confirm {
        lines.push(Line::from(Span::styled(
            "Confirm Deny All: press Enter again",
            Style::default().fg(palette.error),
        )));
    }
    if !any_selected {
        lines.push(Line::from(Span::styled(
            "No tools selected — approving will deny all.",
            Style::default().fg(palette.warning),
        )));
    }
    lines.push(Line::from(
        "Keys: Space toggle, ↑/↓ navigate, Tab details, Enter activate, a approve all, d/Esc deny",
    ));
}

fn append_recovery_prompt(lines: &mut Vec<Line>, app: &App, palette: &Palette) {
    if app.tool_recovery_calls().is_none() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(
        "Tool recovery detected. Tools will not be re-run.",
    ));
    lines.push(Line::from(Span::styled(
        "Resume keeps recovered results; discard drops them.",
        Style::default().fg(palette.text_muted),
    )));
    lines.push(Line::from("Press r to resume or d to discard."));
}

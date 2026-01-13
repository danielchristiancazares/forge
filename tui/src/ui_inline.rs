//! Inline TUI mode - minimal viewport for shell integration.

use ratatui::prelude::{Backend, Terminal};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget, Wrap},
};

use forge_engine::{App, DisplayItem, InputMode, Message};
use forge_types::sanitize_terminal_text;

use crate::theme::{glyphs, palette, styles, Glyphs, Palette};
use crate::tool_display;
use crate::{draw_input, draw_model_selector, draw_status_bar};

pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;

/// Height needed for the model selector overlay in inline mode.
/// Calculated as: inner content + borders + padding.
pub const INLINE_MODEL_SELECTOR_HEIGHT: u16 = 18;

/// Returns the viewport height needed for inline mode based on current input mode.
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
    last_pending_tool_signature: Option<String>,
    last_approval_signature: Option<String>,
    last_recovery_active: bool,
}

impl InlineOutput {
    pub fn new() -> Self {
        Self {
            next_display_index: 0,
            has_output: false,
            last_tool_output_len: 0,
            last_tool_status_signature: None,
            last_pending_tool_signature: None,
            last_approval_signature: None,
            last_recovery_active: false,
        }
    }

    pub fn reset(&mut self) {
        self.next_display_index = 0;
        self.has_output = false;
        self.last_tool_output_len = 0;
        self.last_tool_status_signature = None;
        self.last_pending_tool_signature = None;
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

        let tool_signature = tool_status_signature(app);
        if tool_signature != self.last_tool_status_signature {
            if tool_signature.is_some() {
                append_tool_status_lines(&mut lines, app, &glyphs);
            }
            self.last_tool_status_signature = tool_signature;
        }

        let pending_signature = pending_tool_signature(app);
        if pending_signature != self.last_pending_tool_signature {
            if pending_signature.is_some() {
                append_pending_tool_lines(&mut lines, app, &glyphs);
            }
            self.last_pending_tool_signature = pending_signature;
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

        let approval_signature = approval_signature(app);
        if approval_signature != self.last_approval_signature {
            if approval_signature.is_some() {
                append_approval_lines(&mut lines, app, &palette);
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
    let total_height = input_height + 1;
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

    draw_input(frame, app, chunks[0], &palette);
    draw_status_bar(frame, app, chunks[1], &palette, &glyphs);

    // TODO: Inline model picker needs a compact layout (bug report: pretty modal is cramped).
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
    lines.push(Line::from(""));

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
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(result_line.to_string(), content_style),
                ]));
            }
        }
        _ => {
            // Regular messages
            let content_style = match msg {
                Message::System(_) => Style::default().fg(palette.text_muted),
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

fn tool_status_signature(app: &App) -> Option<String> {
    let calls = app.tool_loop_calls()?;
    let mut results_map: std::collections::HashMap<&str, &forge_types::ToolResult> =
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
    let approval_pending = app.tool_approval_requests().is_some();

    let mut parts = Vec::with_capacity(calls.len());
    for call in calls {
        let status = if let Some(result) = results_map.get(call.id.as_str()) {
            if !execute_ids.contains(call.id.as_str()) {
                "denied"
            } else if result.is_error {
                "error"
            } else {
                "ok"
            }
        } else if current_id == Some(call.id.as_str()) {
            "running"
        } else if approval_pending && !execute_ids.contains(call.id.as_str()) {
            "approval"
        } else {
            "pending"
        };
        parts.push(format!("{}:{status}", call.id));
    }

    Some(parts.join("|"))
}

fn pending_tool_signature(app: &App) -> Option<String> {
    let calls = app.pending_tool_calls()?;
    let mut parts = Vec::with_capacity(calls.len());
    for call in calls {
        parts.push(call.id.clone());
    }
    Some(parts.join("|"))
}

fn append_tool_status_lines(lines: &mut Vec<Line>, app: &App, glyphs: &Glyphs) {
    let Some(calls) = app.tool_loop_calls() else {
        return;
    };
    let mut results_map: std::collections::HashMap<&str, &forge_types::ToolResult> =
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
    let approval_pending = app.tool_approval_requests().is_some();

    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Tool status:"));

    for call in calls {
        let mut reason: Option<String> = None;
        let icon = if let Some(result) = results_map.get(call.id.as_str()) {
            if !execute_ids.contains(call.id.as_str()) {
                let content = sanitize_terminal_text(&result.content);
                reason = content
                    .lines()
                    .next()
                    .map(|line| truncate_with_ellipsis(line, 80));
                glyphs.denied
            } else if result.is_error {
                let content = sanitize_terminal_text(&result.content);
                reason = content
                    .lines()
                    .next()
                    .map(|line| truncate_with_ellipsis(line, 80));
                glyphs.tool_result_err
            } else {
                glyphs.tool_result_ok
            }
        } else if current_id == Some(call.id.as_str()) {
            glyphs.running
        } else if approval_pending && !execute_ids.contains(call.id.as_str()) {
            glyphs.paused
        } else {
            glyphs.bullet
        };

        let name = sanitize_terminal_text(&call.name);
        let id = sanitize_terminal_text(&call.id);
        lines.push(Line::from(format!(
            "  {icon} {} ({})",
            name.as_ref(),
            id.as_ref()
        )));
        if let Some(reason) = reason {
            lines.push(Line::from(format!("     {reason}")));
        }
    }
}

fn append_pending_tool_lines(lines: &mut Vec<Line>, app: &App, glyphs: &Glyphs) {
    let Some(calls) = app.pending_tool_calls() else {
        return;
    };
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Awaiting tool results:"));
    for call in calls {
        let name = sanitize_terminal_text(&call.name);
        let id = sanitize_terminal_text(&call.id);
        lines.push(Line::from(format!(
            "  {} {} ({})",
            glyphs.bullet,
            name.as_ref(),
            id.as_ref()
        )));
    }
    lines.push(Line::from(
        "Use /tool <id> <result> or /tool error <id> <message>",
    ));
}

fn approval_signature(app: &App) -> Option<String> {
    let requests = app.tool_approval_requests()?;
    let selected = app.tool_approval_selected().unwrap_or(&[]);
    let cursor = app.tool_approval_cursor().unwrap_or(0);
    let mut sig = format!("{}|{}|", requests.len(), cursor);
    for flag in selected {
        sig.push(if *flag { '1' } else { '0' });
    }
    Some(sig)
}

fn append_approval_lines(lines: &mut Vec<Line>, app: &App, palette: &Palette) {
    let Some(requests) = app.tool_approval_requests() else {
        return;
    };
    let selected = app.tool_approval_selected().unwrap_or(&[]);
    let cursor = app.tool_approval_cursor().unwrap_or(0);

    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Tool approval required:"));

    for (i, req) in requests.iter().enumerate() {
        let is_selected = selected.get(i).copied().unwrap_or(false);
        let pointer = if i == cursor { ">" } else { " " };
        let checkbox = if is_selected { "[x]" } else { "[ ]" };
        let risk = format!("{:?}", req.risk_level).to_uppercase();
        let tool_name = sanitize_terminal_text(&req.tool_name);
        lines.push(Line::from(format!(
            " {pointer} {checkbox} {} ({risk})",
            tool_name.as_ref()
        )));
        if !req.summary.trim().is_empty() {
            let summary = sanitize_terminal_text(&req.summary);
            let summary = truncate_with_ellipsis(summary.as_ref(), 80);
            lines.push(Line::from(format!("     {summary}")));
        }
    }

    // Submit and Deny buttons
    let submit_cursor = requests.len();
    let deny_cursor = requests.len() + 1;
    let submit_pointer = if cursor == submit_cursor { ">" } else { " " };
    let deny_pointer = if cursor == deny_cursor { ">" } else { " " };
    lines.push(Line::from(format!(
        " {submit_pointer} [ Submit ]    {deny_pointer} [ Deny All ]"
    )));

    if app.tool_approval_deny_confirm() {
        lines.push(Line::from(Span::styled(
            "Confirm Deny All: press Enter again",
            Style::default().fg(palette.error),
        )));
    }
    lines.push(Line::from("Keys: Space toggle, j/k navigate, Enter select"));
}

fn append_recovery_prompt(lines: &mut Vec<Line>, app: &App, palette: &Palette) {
    if app.tool_recovery_calls().is_none() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from("Tool recovery detected. Tools will not be re-run."));
    lines.push(Line::from(Span::styled(
        "Resume keeps recovered results; discard drops them.",
        Style::default().fg(palette.text_muted),
    )));
    lines.push(Line::from("Press r to resume or d to discard."));
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

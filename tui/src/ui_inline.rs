//! Inline TUI mode - minimal viewport for shell integration.

use std::collections::HashMap;

use ratatui::prelude::{Backend, Terminal};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget, Wrap},
};

use forge_engine::{App, DisplayItem, InputMode, Message, PredefinedModel};
use forge_types::sanitize_terminal_text;

use crate::draw_input;
use crate::shared::{
    ApprovalView, ToolCallStatus, ToolCallStatusKind, collect_approval_view, collect_tool_statuses,
    message_header_parts, tool_status_signature, wrapped_line_count,
};
use crate::theme::{Glyphs, Palette, glyphs, palette};
use crate::tool_result_summary::{ToolCallMeta, ToolResultRender, tool_result_render_decision};

pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;

fn inline_input_height(mode: InputMode) -> u16 {
    match mode {
        // Keep normal mode compact in inline view.
        InputMode::Normal => 3,
        // The inline model selector needs enough rows to show every entry.
        //
        // `draw_inline_model_selector()` renders a bordered block with the model options as
        // paragraph lines. The block consumes 2 rows for borders, so we need:
        //   model_count + 2
        //
        // We also clamp to `INLINE_INPUT_HEIGHT` so other overlays keep their baseline.
        InputMode::ModelSelect => {
            let model_count = u16::try_from(PredefinedModel::all().len()).unwrap_or(0);
            (model_count + 2).max(INLINE_INPUT_HEIGHT)
        }
        _ => INLINE_INPUT_HEIGHT,
    }
}

/// Returns the viewport height needed for inline mode.
#[must_use]
pub fn inline_viewport_height(mode: InputMode) -> u16 {
    // +1 line above the input for a minimal output/status line.
    inline_input_height(mode) + 1
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
    tool_calls: HashMap<String, ToolCallMeta>,
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
            tool_calls: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.next_display_index = 0;
        self.has_output = false;
        self.last_tool_output_len = 0;
        self.last_tool_status_signature = None;
        self.last_approval_signature = None;
        self.last_recovery_active = false;
        self.tool_calls.clear();
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
            // Buffer ToolUse messages until we see their paired ToolResult.
            // History stores all ToolUses before ToolResults for API correctness,
            // but we want to render each ToolUse immediately before its result.
            let mut buffered_tool_uses: HashMap<String, (DisplayItem, ToolCallMeta)> =
                HashMap::new();

            for item in &items[self.next_display_index..] {
                let msg = match item {
                    DisplayItem::History(id) => app.history().get_entry(*id).message(),
                    DisplayItem::Local(msg) => msg,
                };

                match msg {
                    Message::ToolUse(call) => {
                        let meta = ToolCallMeta::from_call(call);
                        // Store metadata for result lookup (persists across render passes)
                        self.tool_calls.insert(call.id.clone(), meta.clone());
                        // Buffer for rendering when we see the result
                        buffered_tool_uses.insert(call.id.clone(), (item.clone(), meta));
                    }
                    Message::ToolResult(result) => {
                        // Render paired ToolUse first (if buffered in this pass)
                        if let Some((tool_item, _)) =
                            buffered_tool_uses.remove(&result.tool_call_id)
                        {
                            let tool_msg = match &tool_item {
                                DisplayItem::History(id) => app.history().get_entry(*id).message(),
                                DisplayItem::Local(m) => m,
                            };
                            append_message_lines(
                                &mut lines,
                                tool_msg,
                                &mut msg_count,
                                &palette,
                                &glyphs,
                                None,
                            );
                        }
                        // Render the result with metadata lookup
                        let meta = self.tool_calls.get(&result.tool_call_id);
                        append_message_lines(
                            &mut lines,
                            msg,
                            &mut msg_count,
                            &palette,
                            &glyphs,
                            meta,
                        );
                        self.tool_calls.remove(&result.tool_call_id);
                    }
                    _ => append_message_lines(
                        &mut lines,
                        msg,
                        &mut msg_count,
                        &palette,
                        &glyphs,
                        None,
                    ),
                }
            }

            // Render any orphaned ToolUse messages (in-flight, no result yet)
            for (_, (item, _)) in buffered_tool_uses {
                let msg = match &item {
                    DisplayItem::History(id) => app.history().get_entry(*id).message(),
                    DisplayItem::Local(m) => m,
                };
                append_message_lines(&mut lines, msg, &mut msg_count, &palette, &glyphs, None);
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

    let input_height = inline_input_height(app.input_mode());

    // Anchor input to the bottom: position it so it occupies the last `input_height` rows
    let content_area = Rect {
        x: area.x,
        y: area
            .y
            .saturating_add(area.height.saturating_sub(input_height)),
        width: area.width,
        height: input_height,
    };

    draw_input(frame, app, content_area, &palette, &glyphs, true);
}

fn append_message_lines(
    lines: &mut Vec<Line>,
    msg: &Message,
    msg_count: &mut usize,
    palette: &Palette,
    glyphs: &Glyphs,
    tool_call_meta: Option<&ToolCallMeta>,
) {
    // Single blank line between messages, but not before tool results (they attach to their call)
    let is_tool_result = matches!(msg, Message::ToolResult(_));
    if *msg_count > 0 && !is_tool_result {
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    let (icon, name, name_style) = message_header_parts(msg, palette, glyphs);

    match msg {
        Message::User(_) => {
            let content_style = Style::default().fg(palette.text_primary);
            let content = sanitize_terminal_text(msg.content());
            let mut first = true;

            for content_line in content.lines() {
                if first {
                    // First line gets the icon
                    if content_line.is_empty() {
                        lines.push(Line::from(vec![Span::styled(
                            format!(" {icon} "),
                            name_style,
                        )]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {icon} "), name_style),
                            Span::styled(content_line.to_string(), content_style),
                        ]));
                    }
                    first = false;
                } else {
                    // Subsequent lines with 2-space indent
                    if content_line.is_empty() {
                        lines.push(Line::from(""));
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(content_line.to_string(), content_style),
                        ]));
                    }
                }
            }

            if first {
                // Message was empty
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} "),
                    name_style,
                )]));
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

            match tool_result_render_decision(tool_call_meta, &content, result.is_error, 60) {
                ToolResultRender::Full { diff_aware } => {
                    let content_style = if result.is_error {
                        Style::default().fg(palette.error)
                    } else {
                        Style::default().fg(palette.text_secondary)
                    };
                    for raw_line in content.lines() {
                        if raw_line.is_empty() {
                            lines.push(Line::from(""));
                            continue;
                        }
                        let style = if diff_aware {
                            diff_style_for_line(raw_line, content_style, palette)
                        } else {
                            content_style
                        };
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(raw_line.to_string(), style),
                        ]));
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
            // Inline content with icon (no header line)
            let content_style = match msg {
                Message::Assistant(_) => Style::default().fg(palette.text_secondary),
                _ => Style::default().fg(palette.text_muted),
            };
            let content = sanitize_terminal_text(msg.content());
            let mut first = true;

            for content_line in content.lines() {
                if first {
                    if content_line.is_empty() {
                        lines.push(Line::from(vec![Span::styled(
                            format!(" {icon} "),
                            name_style,
                        )]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {icon} "), name_style),
                            Span::styled(content_line.to_string(), content_style),
                        ]));
                    }
                    first = false;
                } else if content_line.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(content_line.to_string(), content_style),
                    ]));
                }
            }

            if first {
                lines.push(Line::from(vec![Span::styled(
                    format!(" {icon} "),
                    name_style,
                )]));
            }
        }
        Message::Thinking(_) => {
            // Provider thinking/reasoning - styled italic and muted
            let content_style = Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::ITALIC);
            let content = sanitize_terminal_text(msg.content());

            // Header line with icon and "Thinking" label
            lines.push(Line::from(vec![Span::styled(
                format!(" {icon} {name}"),
                name_style,
            )]));

            // Content lines indented
            for content_line in content.lines() {
                if content_line.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("   "),
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

/// Get the style for a diff line.
fn diff_style_for_line(line: &str, base_style: Style, palette: &Palette) -> Style {
    use ratatui::style::Modifier;

    if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file mode")
        || line.starts_with("deleted file mode")
    {
        return Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::BOLD);
    }

    if line.starts_with("@@") {
        return Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD);
    }

    if line == "..." {
        return Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::ITALIC);
    }

    if line.starts_with('-') {
        return Style::default().fg(palette.error);
    }
    if line.starts_with('+') {
        return Style::default().fg(palette.success);
    }

    base_style
}

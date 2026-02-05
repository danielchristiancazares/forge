//! Shared rendering helpers between full-screen and inline TUI views.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use forge_engine::{App, Message, Provider, ToolResult, sanitize_terminal_text};

use crate::theme::{Glyphs, Palette, styles};
use crate::tool_display;

pub(crate) fn provider_color(provider: Provider, palette: &Palette) -> Color {
    match provider {
        Provider::Claude => palette.provider_claude,
        Provider::OpenAI => palette.provider_openai,
        Provider::Gemini => palette.provider_gemini,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCallStatusKind {
    Denied,
    Error,
    Ok,
    Running,
    Approval,
    Pending,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolCallStatus {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: ToolCallStatusKind,
    pub(crate) reason: Option<String>,
}

pub(crate) fn collect_tool_statuses(
    app: &App,
    reason_max_len: usize,
) -> Option<Vec<ToolCallStatus>> {
    let calls = app.tool_loop_calls()?;
    let mut results_map: HashMap<&str, &ToolResult> = HashMap::new();
    if let Some(results) = app.tool_loop_results() {
        for result in results {
            results_map.insert(result.tool_call_id.as_str(), result);
        }
    }
    let execute_ids: HashSet<&str> = app
        .tool_loop_execute_calls()
        .map(|exec_calls| exec_calls.iter().map(|c| c.id.as_str()).collect())
        .unwrap_or_default();
    let current_id = app.tool_loop_current_call_id();
    let approval_pending = app.tool_approval_requests().is_some();

    let mut statuses = Vec::with_capacity(calls.len());
    for call in calls {
        let mut reason: Option<String> = None;
        let status = if let Some(result) = results_map.get(call.id.as_str()) {
            if !execute_ids.contains(call.id.as_str()) {
                reason = first_result_line(result, reason_max_len);
                ToolCallStatusKind::Denied
            } else if result.is_error {
                reason = first_result_line(result, reason_max_len);
                ToolCallStatusKind::Error
            } else {
                ToolCallStatusKind::Ok
            }
        } else if current_id == Some(call.id.as_str()) {
            ToolCallStatusKind::Running
        } else if approval_pending && !execute_ids.contains(call.id.as_str()) {
            ToolCallStatusKind::Approval
        } else {
            ToolCallStatusKind::Pending
        };

        let display_name = tool_display::format_tool_call_compact(&call.name, &call.arguments);
        statuses.push(ToolCallStatus {
            id: call.id.clone(),
            name: display_name,
            status,
            reason,
        });
    }

    Some(statuses)
}

pub(crate) fn tool_status_signature(statuses: Option<&[ToolCallStatus]>) -> Option<String> {
    let statuses = statuses?;
    let mut parts = Vec::with_capacity(statuses.len());
    for status in statuses {
        let status_label = match status.status {
            ToolCallStatusKind::Denied => "denied",
            ToolCallStatusKind::Error => "error",
            ToolCallStatusKind::Ok => "ok",
            ToolCallStatusKind::Running => "running",
            ToolCallStatusKind::Approval => "approval",
            ToolCallStatusKind::Pending => "pending",
        };
        parts.push(format!("{}:{status_label}", status.id));
    }

    Some(parts.join("|"))
}

fn first_result_line(result: &ToolResult, max_len: usize) -> Option<String> {
    let content = sanitize_terminal_text(&result.content);
    content
        .lines()
        .next()
        .map(|line| truncate_with_ellipsis(line, max_len))
}

fn wrapped_rows_for_line(line: &Line, width: u16) -> usize {
    Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
}

pub(crate) fn wrapped_line_rows(lines: &[Line], width: u16) -> Vec<usize> {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| wrapped_rows_for_line(line, width))
        .collect()
}

pub(crate) fn wrapped_line_count_exact(lines: &[Line], width: u16) -> usize {
    if lines.is_empty() {
        return 0;
    }

    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
}

pub(crate) fn wrapped_line_count(lines: &[Line], width: u16) -> u16 {
    wrapped_line_count_exact(lines, width).min(u16::MAX as usize) as u16
}

pub(crate) fn truncate_with_ellipsis(raw: &str, max: usize) -> String {
    let max = max.max(3);
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max - 3).collect();
        format!("{head}...")
    }
}

pub(crate) fn message_header_parts(
    msg: &Message,
    palette: &Palette,
    glyphs: &Glyphs,
) -> (String, String, Style) {
    match msg {
        Message::System(_) => (
            glyphs.system.to_string(),
            "System".to_string(),
            Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Message::User(_) => (
            glyphs.user.to_string(),
            String::new(),
            styles::user_name(palette),
        ),
        Message::Assistant(m) => {
            let color = provider_color(m.provider(), palette);
            (
                glyphs.assistant.to_string(),
                String::new(), // No name label - color encodes provider
                Style::default().fg(color),
            )
        }
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
        Message::Thinking(m) => {
            let color = provider_color(m.provider(), palette);
            (
                glyphs.thinking.to_string(),
                "Thinking".to_string(),
                Style::default().fg(color).add_modifier(Modifier::ITALIC),
            )
        }
    }
}

pub(crate) struct ApprovalView {
    pub(crate) items: Vec<ApprovalItem>,
    pub(crate) selected: Vec<bool>,
    pub(crate) cursor: usize,
    pub(crate) expanded: Option<usize>,
    pub(crate) any_selected: bool,
    pub(crate) deny_confirm: bool,
}

pub(crate) struct ApprovalItem {
    pub(crate) tool_name: String,
    pub(crate) risk_label: String,
    pub(crate) summary: Option<String>,
    pub(crate) details: Vec<String>,
    /// Homoglyph warnings formatted for display.
    pub(crate) homoglyph_warnings: Vec<String>,
}

pub(crate) fn collect_approval_view(app: &App, max_width: usize) -> Option<ApprovalView> {
    let requests = app.tool_approval_requests()?;
    let selected = app.tool_approval_selected().unwrap_or(&[]);
    let cursor = app.tool_approval_cursor().unwrap_or(0);
    let expanded = app.tool_approval_expanded();
    let deny_confirm = app.tool_approval_deny_confirm();
    let any_selected = selected.iter().any(|flag| *flag);

    let mut items = Vec::with_capacity(requests.len());
    for (i, req) in requests.iter().enumerate() {
        let tool_name = sanitize_terminal_text(&req.tool_name).into_owned();
        let risk_label = format!("{:?}", req.risk_level).to_uppercase();
        let summary = if req.summary.trim().is_empty() {
            None
        } else {
            let summary = sanitize_terminal_text(&req.summary);
            Some(truncate_with_ellipsis(
                summary.as_ref(),
                max_width.saturating_sub(6),
            ))
        };
        let details = if expanded == Some(i) {
            if let Ok(details) = serde_json::to_string_pretty(&req.arguments) {
                details
                    .lines()
                    .map(|line| truncate_with_ellipsis(line, max_width.saturating_sub(6)))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // Format homoglyph warnings for display
        let homoglyph_warnings: Vec<String> = req
            .warnings
            .iter()
            .map(|w| {
                format!(
                    "Mixed scripts in '{}': {}",
                    w.field_name,
                    w.scripts_display()
                )
            })
            .collect();

        items.push(ApprovalItem {
            tool_name,
            risk_label,
            summary,
            details,
            homoglyph_warnings,
        });
    }

    Some(ApprovalView {
        items,
        selected: selected.to_vec(),
        cursor,
        expanded,
        any_selected,
        deny_confirm,
    })
}

#[cfg(test)]
mod tests {
    use super::wrapped_rows_for_line;
    use ratatui::text::Line;

    #[test]
    fn wrapped_rows_long_word_uses_remaining_space() {
        let line = Line::from("hello abcdefghijklmnopqrst");
        assert_eq!(wrapped_rows_for_line(&line, 10), 3);
    }

    #[test]
    fn wrapped_rows_long_word_exact_fill() {
        let line = Line::from("hello abcdefghijklmn");
        assert_eq!(wrapped_rows_for_line(&line, 10), 3);
    }
}

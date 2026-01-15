//! Shared rendering helpers between full-screen and inline TUI views.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use forge_engine::{App, Message, ToolResult, sanitize_terminal_text};

use crate::theme::{Glyphs, Palette, styles};
use crate::tool_display;

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

        statuses.push(ToolCallStatus {
            id: call.id.clone(),
            name: call.name.clone(),
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

pub(crate) fn wrapped_line_count(lines: &[Line], width: u16) -> u16 {
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

        items.push(ApprovalItem {
            tool_name,
            risk_label,
            summary,
            details,
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

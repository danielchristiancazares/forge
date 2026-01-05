use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use ratatui::prelude::{Backend, Terminal};

use crate::app::{App, DisplayItem};
use crate::message::Message;
use crate::theme::{colors, styles};
use crate::ui::{draw_input, draw_status_bar};

pub const INLINE_INPUT_HEIGHT: u16 = 5;
pub const INLINE_VIEWPORT_HEIGHT: u16 = INLINE_INPUT_HEIGHT + 1;

pub struct InlineOutput {
    next_display_index: usize,
    has_output: bool,
}

impl InlineOutput {
    pub fn new() -> Self {
        Self {
            next_display_index: 0,
            has_output: false,
        }
    }

    pub fn flush<B>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error>
    where
        B: Backend,
    {
        let items = app.display_items();
        if self.next_display_index >= items.len() {
            return Ok(());
        }

        let mut lines: Vec<Line> = Vec::new();
        let mut msg_count = if self.has_output { 1 } else { 0 };

        for item in &items[self.next_display_index..] {
            let msg = match item {
                DisplayItem::History(id) => app.history().get_entry(*id).message(),
                DisplayItem::Local(msg) => msg,
            };

            append_message_lines(&mut lines, msg, &mut msg_count);
        }

        self.next_display_index = items.len();
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

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(INLINE_INPUT_HEIGHT),
            Constraint::Length(1),
        ])
        .split(area);

    draw_input(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);
}

fn append_message_lines(lines: &mut Vec<Line>, msg: &Message, msg_count: &mut usize) {
    if *msg_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(""));
    }
    *msg_count += 1;

    let (icon, name, name_style) = match msg {
        Message::System(_) => (
            "S",
            "System",
            Style::default()
                .fg(colors::TEXT_MUTED)
                .add_modifier(Modifier::BOLD),
        ),
        Message::User(_) => (">", "You", styles::user_name()),
        Message::Assistant(m) => ("*", m.provider().display_name(), styles::assistant_name()),
    };

    let header_line = Line::from(vec![
        Span::styled(format!(" {icon} "), name_style),
        Span::styled(name, name_style),
    ]);
    lines.push(header_line);
    lines.push(Line::from(""));

    let content_style = match msg {
        Message::System(_) => Style::default().fg(colors::TEXT_MUTED),
        Message::User(_) => Style::default().fg(colors::TEXT_PRIMARY),
        Message::Assistant(_) => Style::default().fg(colors::TEXT_SECONDARY),
    };

    for content_line in msg.content().lines() {
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

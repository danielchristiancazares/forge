//! Inline TUI mode - minimal viewport for shell integration.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget, Wrap},
};
use ratatui::prelude::{Backend, Terminal};

use forge_engine::{App, DisplayItem, InputMode, Message};

use crate::theme::{colors, styles};
use crate::{draw_input, draw_model_selector, draw_status_bar};

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

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Clear, area);

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
        .constraints([
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(content_area);

    draw_input(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);

    // TODO: Inline model picker needs a compact layout (bug report: pretty modal is cramped).
    // Draw model selector overlay if in model select mode
    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app);
    }
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
        Message::User(_) => ("○", "You", styles::user_name()),
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

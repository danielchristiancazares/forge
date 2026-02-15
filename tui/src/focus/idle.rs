use crate::theme::Palette;
use forge_engine::App;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

const IDLE_TEXT: &str = "Ready";

pub fn draw(frame: &mut Frame, _app: &App, area: Rect, palette: &Palette) {
    let ready = Paragraph::new(IDLE_TEXT)
        .style(Style::default().fg(palette.text_muted))
        .alignment(Alignment::Center);

    let center_y = area.height / 2;
    // Simple centering
    let ready_area = Rect {
        x: area.x,
        y: area.y + center_y,
        width: area.width,
        height: 1,
    };

    frame.render_widget(ready, ready_area);
}

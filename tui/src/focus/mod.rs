pub mod content;
pub mod executing;
pub mod idle;
pub mod reviewing;

use crate::theme::Palette;
use forge_engine::{App, FocusState};
use ratatui::Frame;
use ratatui::layout::Rect;

pub fn draw(frame: &mut Frame, app: &App, area: Rect, palette: &Palette) {
    if app.plan_state().is_active() {
        executing::draw(frame, app, area, palette);
        return;
    }

    // Default dispatch
    match app.focus_state() {
        FocusState::Idle => idle::draw(frame, app, area, palette),
        FocusState::Executing { .. } => executing::draw(frame, app, area, palette),
        FocusState::Reviewing { .. } => reviewing::draw(frame, app, area, palette),
    }
}

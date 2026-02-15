use forge_engine::{App, FocusState};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::markdown::render_markdown;
use crate::theme::Palette;

use super::content::{ContentBlock, extract_blocks};

pub fn draw(frame: &mut Frame, app: &App, area: Rect, palette: &Palette) {
    let blocks = extract_blocks(app);
    if blocks.is_empty() {
        return;
    }

    let active_index = match app.focus_state() {
        FocusState::Reviewing { active_index, .. } => *active_index,
        _ => 0,
    };

    let active_index = active_index.min(blocks.len().saturating_sub(1));

    // Render the active block centered, with dimmed adjacent blocks on either side.
    // Layout: | prev (dimmed) | active (full contrast) | next (dimmed) |
    // Each block gets a proportional width: active gets ~60%, adjacents get ~20% each.

    let total_width = area.width;
    let active_width = total_width * 3 / 5;
    let side_width = (total_width - active_width) / 2;

    // Previous block (left side, dimmed)
    if active_index > 0 {
        let prev_area = Rect {
            x: area.x,
            y: area.y,
            width: side_width.saturating_sub(1),
            height: area.height,
        };
        draw_block(
            frame,
            &blocks[active_index - 1],
            prev_area,
            palette.text_disabled,
            palette,
        );
    }

    // Active block (center, full contrast)
    let active_x = area.x + side_width;
    let active_area = Rect {
        x: active_x,
        y: area.y,
        width: active_width,
        height: area.height,
    };
    draw_block(
        frame,
        &blocks[active_index],
        active_area,
        palette.text_primary,
        palette,
    );

    // Next block (right side, dimmed)
    if active_index + 1 < blocks.len() {
        let next_x = active_x + active_width + 1;
        let next_width = area.x + total_width - next_x;
        let next_area = Rect {
            x: next_x,
            y: area.y,
            width: next_width,
            height: area.height,
        };
        draw_block(
            frame,
            &blocks[active_index + 1],
            next_area,
            palette.text_disabled,
            palette,
        );
    }

    // Block counter at bottom
    if area.height > 2 {
        let counter = format!("{}/{}", active_index + 1, blocks.len());
        let counter_area = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(counter)
                .style(Style::default().fg(palette.text_muted))
                .alignment(Alignment::Center),
            counter_area,
        );
    }
}

fn draw_block(
    frame: &mut Frame,
    block: &ContentBlock,
    area: Rect,
    fg: ratatui::style::Color,
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (label, content) = match block {
        ContentBlock::Thought(text) => ("thought", text.as_str()),
        ContentBlock::Response(text) => ("response", text.as_str()),
        ContentBlock::ToolResult { name, content } => (name.as_str(), content.as_str()),
    };

    // Header line
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(label.to_string())
            .style(Style::default().fg(fg))
            .alignment(Alignment::Center),
        header_area,
    );

    // Content body (rendered as markdown)
    if area.height > 1 {
        let body_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height - 1,
        };

        let lines = render_markdown(content, Style::default().fg(fg), palette, body_area.width);

        // Truncate to fit available height
        let visible: Vec<_> = lines.into_iter().take(body_area.height as usize).collect();

        frame.render_widget(Paragraph::new(visible), body_area);
    }
}

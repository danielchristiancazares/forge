//! TUI snapshot tests using vt100 virtual terminal.

mod vt100_backend;

use insta::assert_snapshot;
use ratatui::Terminal;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::{TerminalOptions, Viewport};

use vt100_backend::VT100Backend;

/// Helper to render a widget and capture the output as a snapshot.
fn snapshot_widget<F>(name: &str, width: u16, height: u16, render_fn: F)
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = VT100Backend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    terminal.draw(render_fn).expect("failed to draw");

    assert_snapshot!(name, terminal.backend().to_string());
}

#[test]
fn snapshot_empty_screen() {
    snapshot_widget("empty_screen", 40, 10, |_frame| {
        // Empty - just captures blank terminal
    });
}

#[test]
fn snapshot_simple_paragraph() {
    snapshot_widget("simple_paragraph", 40, 10, |frame| {
        let text = Paragraph::new("Hello, Forge!")
            .block(Block::default().borders(Borders::ALL).title(" Test "));
        frame.render_widget(text, frame.area());
    });
}

#[test]
fn snapshot_styled_text() {
    snapshot_widget("styled_text", 50, 8, |frame| {
        let lines = vec![
            Line::from(vec![
                Span::styled("Bold", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" and "),
                Span::styled("colored", Style::default().fg(Color::Cyan)),
            ]),
            Line::from("Plain text line"),
        ];

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Styled "),
        );

        frame.render_widget(paragraph, frame.area());
    });
}

#[test]
fn snapshot_model_selector_layout() {
    snapshot_widget("model_selector_layout", 60, 15, |frame| {
        let area = frame.area();

        // Simulate the model selector modal positioning
        let selector_width = 40.min(area.width.saturating_sub(4));
        let selector_height = 6;

        let selector_area = Rect {
            x: (area.width - selector_width) / 2,
            y: area.height.saturating_sub(8),
            width: selector_width,
            height: selector_height,
        };

        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "▸ ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("1 ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Claude Sonnet",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("2 ", Style::default().fg(Color::DarkGray)),
                Span::raw("GPT-5.1"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  ↑↓", Style::default().fg(Color::Yellow)),
                Span::raw(" select  "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(" confirm"),
            ]),
        ];

        let selector = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Magenta))
                .title(" Select Model "),
        );

        frame.render_widget(selector, selector_area);
    });
}

#[test]
fn snapshot_status_bar() {
    snapshot_widget("status_bar", 80, 3, |frame| {
        let area = frame.area();

        let status = Line::from(vec![
            Span::styled(
                " NORMAL ",
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("claude-sonnet-4", Style::default().fg(Color::Cyan)),
            Span::raw(" │ "),
            Span::styled("142 / 128k (0%)", Style::default().fg(Color::Green)),
        ]);

        let paragraph = Paragraph::new(status);
        frame.render_widget(paragraph, Rect::new(0, area.height - 1, area.width, 1));
    });
}

#[test]
fn snapshot_input_box_insert_mode() {
    snapshot_widget("input_box_insert", 60, 5, |frame| {
        let input_text = "Hello, how can you help me today?";
        let cursor_pos = input_text.len();

        let input = Paragraph::new(input_text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Green))
                .title(Line::from(vec![Span::styled(
                    " INSERT ",
                    Style::default()
                        .bg(Color::Green)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )])),
        );

        frame.render_widget(input, frame.area());

        // Position cursor (though we can't really show it in snapshot)
        frame.set_cursor_position((cursor_pos as u16 + 1, 1));
    });
}

#[test]
fn snapshot_message_thread() {
    snapshot_widget("message_thread", 60, 20, |frame| {
        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    " ○ ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "You",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from("    What is Rust?"),
            Line::from(""),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    " ◆ ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Claude",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from("    Rust is a systems programming language focused on"),
            Line::from("    safety, speed, and concurrency. It achieves memory"),
            Line::from("    safety without garbage collection."),
            Line::from(""),
        ];

        let messages = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

        frame.render_widget(messages, frame.area());
    });
}

#[test]
fn snapshot_inline_viewport_cleared() {
    let backend = VT100Backend::new(50, 6);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(3),
        },
    )
    .expect("failed to create terminal with inline viewport");

    terminal
        .draw(|frame| {
            let paragraph = Paragraph::new("Inline Panel")
                .block(Block::default().borders(Borders::ALL).title(" Inline "));
            frame.render_widget(paragraph, frame.area());
        })
        .expect("failed to draw inline panel");

    forge_tui::clear_inline_viewport(&mut terminal).expect("failed to clear viewport");

    assert_snapshot!("inline_viewport_cleared", terminal.backend().to_string());
}

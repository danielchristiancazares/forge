//! Diff-oriented rendering helpers for tool results.
//!
//! The engine emits some tool results in unified-diff-style output (notably
//! `apply_patch`, and optionally `git diff`). The TUI can make these much more
//! readable by applying semantic colors:
//!
//! - `-` removed lines -> red
//! - `+` added lines   -> green
//! - `@@` hunk headers -> accent
//!
//! This module is intentionally conservative: it only styles obvious diff
//! patterns and otherwise falls back to the provided base style.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::Palette;

/// Render tool result content into ratatui lines, applying diff-aware coloring.
#[must_use]
pub fn render_tool_result_lines(
    content: &str,
    base_style: Style,
    palette: &Palette,
    indent: &'static str,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    for raw_line in content.lines() {
        // Preserve blank lines (common in long diffs or multi-section output).
        if raw_line.is_empty() {
            out.push(Line::from(""));
            continue;
        }

        let style = diff_style_for_line(raw_line, base_style, palette);
        out.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(raw_line.to_string(), style),
        ]));
    }

    out
}

fn diff_style_for_line(line: &str, base_style: Style, palette: &Palette) -> Style {
    // File headers: do not treat as additions/deletions even though they start
    // with '+'/'-'.
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

    // Hunk headers.
    if line.starts_with("@@") {
        return Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD);
    }

    // Gap marker (collapsed context).
    if line == "..." {
        return Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::ITALIC);
    }

    // Deletions / additions.
    if line.starts_with('-') {
        return Style::default().fg(palette.error); // red
    }
    if line.starts_with('+') {
        return Style::default().fg(palette.success); // green
    }

    base_style
}

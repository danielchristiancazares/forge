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

    if line.starts_with("@@") {
        return Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD);
    }

    // Gap marker (collapsed context) — standard or line-numbered (`  ...`).
    let trimmed = line.trim_start();
    if trimmed == "..." {
        return Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::ITALIC);
    }

    // Standard unified diff: marker at column 0.
    if line.starts_with('-') {
        return Style::default().fg(palette.error);
    }
    if line.starts_with('+') {
        return Style::default().fg(palette.success);
    }

    // LP1 line-numbered format: `{line_no} -{text}` or `{line_no} +{text}`.
    // The line number is right-aligned digits followed by ` -` or ` +`.
    if let Some(marker) = lp1_diff_marker(line) {
        return match marker {
            '-' => Style::default().fg(palette.error),
            '+' => Style::default().fg(palette.success),
            _ => base_style,
        };
    }

    base_style
}

///
/// Format: `{digits...} {marker}{text}` where marker is `-` or `+`.
/// Context lines use `{digits...}  {text}` (two spaces, no marker).
fn lp1_diff_marker(line: &str) -> Option<char> {
    let bytes = line.as_bytes();
    // Skip leading whitespace + digits
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i].is_ascii_digit()) {
        i += 1;
    }
    // Must have consumed at least one digit
    if i == 0 || !bytes[..i].iter().any(u8::is_ascii_digit) {
        return None;
    }
    // Next char should be the marker
    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        Some(bytes[i] as char)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lp1_marker_detects_add_and_remove() {
        assert_eq!(lp1_diff_marker(" 42 -old line"), Some('-'));
        assert_eq!(lp1_diff_marker(" 42 +new line"), Some('+'));
        assert_eq!(lp1_diff_marker("  1 -x"), Some('-'));
        assert_eq!(lp1_diff_marker("999 +y"), Some('+'));
    }

    #[test]
    fn lp1_marker_ignores_context_lines() {
        // Context lines: `{line_no}  {text}` (space, not +/-)
        assert_eq!(lp1_diff_marker(" 42  context text"), None);
    }

    #[test]
    fn lp1_marker_ignores_non_diff_lines() {
        assert_eq!(lp1_diff_marker("plain text"), None);
        assert_eq!(lp1_diff_marker(""), None);
        assert_eq!(lp1_diff_marker("no digits here -x"), None);
    }

    #[test]
    fn lp1_gap_marker_detected() {
        // LP1 gap markers are right-aligned: "  ..."
        assert_eq!("  ...".trim_start(), "...");
    }

    #[test]
    fn diff_style_colors_lp1_lines() {
        let palette = Palette::standard();
        let base = Style::default();

        let del_style = diff_style_for_line("  5 -removed", base, &palette);
        assert_eq!(del_style.fg, Some(palette.error));

        let add_style = diff_style_for_line("  5 +added", base, &palette);
        assert_eq!(add_style.fg, Some(palette.success));

        let ctx_style = diff_style_for_line("  5  context", base, &palette);
        assert_eq!(ctx_style, base);
    }

    #[test]
    fn diff_style_still_handles_standard_unified() {
        let palette = Palette::standard();
        let base = Style::default();

        assert_eq!(
            diff_style_for_line("-old", base, &palette).fg,
            Some(palette.error)
        );
        assert_eq!(
            diff_style_for_line("+new", base, &palette).fg,
            Some(palette.success)
        );
    }
}

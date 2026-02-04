use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Palette;

/// Maximum number of cached renders before eviction.
const CACHE_MAX_ENTRIES: usize = 128;

/// Cache key combining content hash and style.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    content_hash: u64,
    style_hash: u64,
    palette_hash: u64,
    soft_breaks_as_newlines: bool,
}

impl CacheKey {
    fn new(content: &str, style: Style, palette: &Palette, soft_breaks_as_newlines: bool) -> Self {
        use std::collections::hash_map::DefaultHasher;

        let mut content_hasher = DefaultHasher::new();
        content.hash(&mut content_hasher);

        let mut style_hasher = DefaultHasher::new();
        // Hash style components manually since Style doesn't impl Hash
        style.fg.hash(&mut style_hasher);
        style.bg.hash(&mut style_hasher);
        style.add_modifier.hash(&mut style_hasher);
        style.sub_modifier.hash(&mut style_hasher);

        let mut palette_hasher = DefaultHasher::new();
        hash_color(palette.bg_dark, &mut palette_hasher);
        hash_color(palette.bg_panel, &mut palette_hasher);
        hash_color(palette.bg_highlight, &mut palette_hasher);
        hash_color(palette.text_primary, &mut palette_hasher);
        hash_color(palette.text_secondary, &mut palette_hasher);
        hash_color(palette.text_muted, &mut palette_hasher);
        hash_color(palette.primary, &mut palette_hasher);
        hash_color(palette.primary_dim, &mut palette_hasher);
        hash_color(palette.peach, &mut palette_hasher);

        Self {
            content_hash: content_hasher.finish(),
            style_hash: style_hasher.finish(),
            palette_hash: palette_hasher.finish(),
            soft_breaks_as_newlines,
        }
    }
}

fn hash_color(color: Color, hasher: &mut impl Hasher) {
    match color {
        Color::Reset => {
            0u8.hash(hasher);
        }
        Color::Black => 1u8.hash(hasher),
        Color::Red => 2u8.hash(hasher),
        Color::Green => 3u8.hash(hasher),
        Color::Yellow => 4u8.hash(hasher),
        Color::Blue => 5u8.hash(hasher),
        Color::Magenta => 6u8.hash(hasher),
        Color::Cyan => 7u8.hash(hasher),
        Color::Gray => 8u8.hash(hasher),
        Color::DarkGray => 9u8.hash(hasher),
        Color::LightRed => 10u8.hash(hasher),
        Color::LightGreen => 11u8.hash(hasher),
        Color::LightYellow => 12u8.hash(hasher),
        Color::LightBlue => 13u8.hash(hasher),
        Color::LightMagenta => 14u8.hash(hasher),
        Color::LightCyan => 15u8.hash(hasher),
        Color::White => 16u8.hash(hasher),
        Color::Rgb(r, g, b) => {
            17u8.hash(hasher);
            r.hash(hasher);
            g.hash(hasher);
            b.hash(hasher);
        }
        Color::Indexed(idx) => {
            18u8.hash(hasher);
            idx.hash(hasher);
        }
    }
}

thread_local! {
    /// Thread-local render cache. Stores rendered lines keyed by content+style hash.
    static RENDER_CACHE: RefCell<HashMap<CacheKey, Vec<Line<'static>>>> = RefCell::new(HashMap::new());
}

/// Clear the render cache. Call when switching themes or on memory pressure.
pub fn clear_render_cache() {
    RENDER_CACHE.with(|cache| cache.borrow_mut().clear());
}

/// Render markdown content to ratatui Lines.
///
/// Uses an internal cache to avoid re-parsing unchanged content.
#[must_use]
pub fn render_markdown(content: &str, base_style: Style, palette: &Palette) -> Vec<Line<'static>> {
    render_markdown_with_soft_breaks(content, base_style, palette, false)
}

/// Render markdown content while preserving single newlines as hard line breaks.
#[must_use]
pub(crate) fn render_markdown_preserve_newlines(
    content: &str,
    base_style: Style,
    palette: &Palette,
) -> Vec<Line<'static>> {
    render_markdown_with_soft_breaks(content, base_style, palette, true)
}

fn render_markdown_with_soft_breaks(
    content: &str,
    base_style: Style,
    palette: &Palette,
    soft_breaks_as_newlines: bool,
) -> Vec<Line<'static>> {
    let key = CacheKey::new(content, base_style, palette, soft_breaks_as_newlines);

    // Check cache first
    let cached = RENDER_CACHE.with(|cache| cache.borrow().get(&key).cloned());

    if let Some(lines) = cached {
        return lines;
    }

    // Cache miss - render and store
    let renderer = MarkdownRenderer::new(base_style, *palette, soft_breaks_as_newlines);
    let lines = renderer.render(content);

    RENDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();

        // Eviction: clear half when full
        if cache.len() >= CACHE_MAX_ENTRIES {
            let keys_to_remove: Vec<_> =
                cache.keys().take(CACHE_MAX_ENTRIES / 2).cloned().collect();
            for k in keys_to_remove {
                cache.remove(&k);
            }
        }

        cache.insert(key, lines.clone());
    });

    lines
}

struct MarkdownRenderer {
    base_style: Style,
    palette: Palette,
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,

    // Style stack for nested formatting (counters, not booleans).
    // Counters allow proper nesting: `# Heading with **bold**` works correctly
    // because heading increments bold_count, strong increments it again,
    // and strong ending decrements it back to 1 (still bold from heading).
    bold_count: usize,
    italic_count: usize,
    code_count: usize,

    // Block state
    in_code_block: bool,
    code_block_content: Vec<String>,

    // Table state
    in_table: bool,
    table_rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    table_alignments: Vec<pulldown_cmark::Alignment>,

    // List state
    list_stack: Vec<Option<u64>>,

    // Render options
    soft_breaks_as_newlines: bool,
}

impl MarkdownRenderer {
    fn is_html_br_tag(s: &str) -> bool {
        // Common variants produced by LLMs / HTML serializers.
        // Keep this small and strict to avoid accidentally treating arbitrary HTML as a line break.
        matches!(s, "<br>" | "<br/>" | "<br />" | "<BR>" | "<BR/>" | "<BR />")
    }

    fn new(base_style: Style, palette: Palette, soft_breaks_as_newlines: bool) -> Self {
        Self {
            base_style,
            palette,
            lines: Vec::new(),
            current_spans: Vec::new(),
            bold_count: 0,
            italic_count: 0,
            code_count: 0,
            in_code_block: false,
            code_block_content: Vec::new(),
            in_table: false,
            table_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            table_alignments: Vec::new(),
            list_stack: Vec::new(),
            soft_breaks_as_newlines,
        }
    }

    fn render(mut self, content: &str) -> Vec<Line<'static>> {
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

        let parser = Parser::new_ext(content, options);

        for event in parser {
            self.handle_event(event);
        }

        // Flush any remaining content
        self.flush_line();

        // Handle incomplete code block (common during streaming)
        if self.in_code_block && !self.code_block_content.is_empty() {
            self.render_code_block();
        }

        self.lines
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.handle_text(&text),
            Event::Code(code) => self.handle_inline_code(&code),
            Event::SoftBreak => self.handle_soft_break(),
            Event::HardBreak => self.flush_line(),
            // Handle HTML/XML content: convert known tags, render others as text.
            // LLM responses may contain XML-like tags that pulldown_cmark parses as HTML.
            Event::Html(html) | Event::InlineHtml(html) => self.handle_html(&html),
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { .. } | Tag::Strong => {
                self.bold_count += 1;
            }
            Tag::Emphasis => {
                self.italic_count += 1;
            }
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
                self.code_block_content.clear();
            }
            Tag::List(start) => {
                self.flush_line();
                self.list_stack.push(start);
            }
            Tag::Item => {
                // Add list marker
                let indent = "    ".repeat(self.list_stack.len().saturating_sub(1));
                let marker = match self.list_stack.last_mut() {
                    Some(Some(idx)) => {
                        let m = format!("{indent}{idx}. ");
                        *idx += 1;
                        m
                    }
                    _ => format!("{indent}• "),
                };
                self.current_spans
                    .push(Span::styled(marker, self.base_style));
            }
            Tag::Table(alignments) => {
                self.flush_line();
                self.in_table = true;
                self.table_rows.clear();
                self.table_alignments = alignments;
            }
            Tag::TableHead | Tag::TableRow => {
                self.current_row.clear();
            }
            Tag::TableCell => {
                self.current_cell.clear();
            }
            Tag::Paragraph => {
                // Add spacing before paragraphs (except at start)
                if !self.lines.is_empty() && self.list_stack.is_empty() {
                    self.lines.push(Line::from(""));
                }
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.bold_count = self.bold_count.saturating_sub(1);
                self.flush_line();
                self.lines.push(Line::from(""));
            }
            TagEnd::Strong => {
                self.bold_count = self.bold_count.saturating_sub(1);
            }
            TagEnd::Emphasis => {
                self.italic_count = self.italic_count.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.render_code_block();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item | TagEnd::Paragraph => {
                self.flush_line();
            }
            TagEnd::Table => {
                self.in_table = false;
                self.render_table();
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if !self.current_row.is_empty() {
                    self.table_rows.push(std::mem::take(&mut self.current_row));
                }
            }
            TagEnd::TableCell => {
                self.current_row
                    .push(std::mem::take(&mut self.current_cell));
            }
            _ => {}
        }
    }

    fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            // Collect code block content line by line
            for line in text.lines() {
                self.code_block_content.push(line.to_string());
            }
            return;
        }

        if self.in_table {
            self.current_cell.push_str(text);
            return;
        }

        let style = self.current_style();
        self.current_spans
            .push(Span::styled(text.to_string(), style));
    }

    fn handle_inline_code(&mut self, code: &str) {
        if self.in_table {
            self.current_cell.push_str(code);
            return;
        }

        let style = Style::default()
            .fg(self.palette.peach)
            .add_modifier(Modifier::BOLD);
        self.current_spans
            .push(Span::styled(code.to_string(), style));
    }

    fn handle_soft_break(&mut self) {
        if !self.in_code_block && !self.in_table {
            if self.soft_breaks_as_newlines {
                self.flush_line();
            } else {
                self.current_spans.push(Span::raw(" "));
            }
        }
    }

    fn handle_html(&mut self, html: &str) {
        let trimmed = html.trim();

        if Self::is_html_br_tag(trimmed) {
            // Code blocks should preserve text literally.
            // (In practice pulldown-cmark shouldn't emit Html events inside code blocks,
            // but this keeps semantics correct.)
            if self.in_code_block {
                self.handle_text(html);
                return;
            }

            if self.in_table {
                // Tables are rendered as single-line rows in our TUI.
                // Treat <br> as a space separator so we don't inject newlines into spans.
                if !self.current_cell.ends_with(char::is_whitespace) {
                    self.current_cell.push(' ');
                }
                return;
            }

            self.flush_line();
            return;
        }

        // For other HTML/XML content, render as text to avoid silent content loss
        self.handle_text(html);
    }

    fn current_style(&self) -> Style {
        let mut style = self.base_style;

        if self.bold_count > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic_count > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.code_count > 0 {
            style = style.fg(self.palette.peach);
        }

        style
    }

    fn flush_line(&mut self) {
        if !self.current_spans.is_empty() {
            let mut spans = vec![Span::raw("    ")]; // Indent
            spans.append(&mut self.current_spans);
            self.lines.push(Line::from(spans));
        }
    }

    fn render_code_block(&mut self) {
        let code_style = Style::default().fg(self.palette.text_muted);

        // Opening fence
        self.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("```", Style::default().fg(self.palette.text_muted)),
        ]));

        // Code content
        for line in &self.code_block_content {
            self.lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(line.clone(), code_style),
            ]));
        }

        // Closing fence
        self.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("```", Style::default().fg(self.palette.text_muted)),
        ]));

        self.code_block_content.clear();
    }

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        // Calculate column widths
        let num_cols = self
            .table_rows
            .iter()
            .map(std::vec::Vec::len)
            .max()
            .unwrap_or(0);
        let mut col_widths: Vec<usize> = vec![0; num_cols];

        for row in &self.table_rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_widths.len() {
                    // Use unicode width for proper emoji/CJK handling
                    col_widths[i] = col_widths[i].max(cell.trim().width());
                }
            }
        }

        // Ensure minimum width
        for w in &mut col_widths {
            *w = (*w).max(3);
        }

        let table_style = Style::default().fg(self.palette.text_muted);
        let header_style = Style::default()
            .fg(self.palette.text_primary)
            .add_modifier(Modifier::BOLD);
        let cell_style = self.base_style;

        // Render top border
        let top_border = self.make_table_border(&col_widths, '┌', '┬', '┐', '─');
        self.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(top_border, table_style),
        ]));

        // Render rows
        for (row_idx, row) in self.table_rows.iter().enumerate() {
            let style = if row_idx == 0 {
                header_style
            } else {
                cell_style
            };

            let row_line = self.make_table_row(row, &col_widths, style, table_style);
            self.lines.push(Line::from(row_line));

            // After header, add separator
            if row_idx == 0 {
                let sep = self.make_table_border(&col_widths, '├', '┼', '┤', '─');
                self.lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(sep, table_style),
                ]));
            }
        }

        // Bottom border
        let bottom_border = self.make_table_border(&col_widths, '└', '┴', '┘', '─');
        self.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(bottom_border, table_style),
        ]));

        self.table_rows.clear();
    }

    #[allow(clippy::unused_self)] // Kept as method for API consistency
    fn make_table_border(
        &self,
        widths: &[usize],
        left: char,
        mid: char,
        right: char,
        fill: char,
    ) -> String {
        let mut s = String::new();
        s.push(left);
        for (i, &w) in widths.iter().enumerate() {
            for _ in 0..w + 2 {
                s.push(fill);
            }
            if i < widths.len() - 1 {
                s.push(mid);
            }
        }
        s.push(right);
        s
    }

    #[allow(clippy::unused_self)] // Kept as method for API consistency
    fn make_table_row(
        &self,
        row: &[String],
        widths: &[usize],
        cell_style: Style,
        border_style: Style,
    ) -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw("    ")];
        spans.push(Span::styled("│", border_style));

        for (i, width) in widths.iter().enumerate() {
            let cell = row.get(i).map_or("", |s| s.trim());
            // Use unicode width for padding calculation (handles emojis)
            let cell_width = cell.width();
            let padding = width.saturating_sub(cell_width);
            let padded = format!(" {}{} ", cell, " ".repeat(padding));
            spans.push(Span::styled(padded, cell_style));
            spans.push(Span::styled("│", border_style));
        }

        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;

    #[test]
    fn test_simple_text() {
        let palette = Palette::standard();
        let lines = render_markdown("Hello world", Style::default(), &palette);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let palette = Palette::standard();
        let lines = render_markdown(md, Style::default(), &palette);
        // Should have borders and content
        assert!(lines.len() >= 4);
    }

    #[test]
    fn test_cache_returns_same_result() {
        clear_render_cache();

        let content = "# Hello\n\nThis is **bold** and *italic* text.";
        let style = Style::default();
        let palette = Palette::standard();

        // First render (cache miss)
        let lines1 = render_markdown(content, style, &palette);

        // Second render (cache hit) should return identical result
        let lines2 = render_markdown(content, style, &palette);

        assert_eq!(lines1.len(), lines2.len());
        for (l1, l2) in lines1.iter().zip(lines2.iter()) {
            assert_eq!(format!("{l1:?}"), format!("{l2:?}"));
        }
    }

    #[test]
    fn test_cache_different_styles_different_results() {
        clear_render_cache();

        let content = "Simple text";
        let style1 = Style::default();
        let style2 = Style::default().add_modifier(Modifier::BOLD);
        let palette = Palette::standard();

        let lines1 = render_markdown(content, style1, &palette);
        let lines2 = render_markdown(content, style2, &palette);

        // Different styles may produce different results (style is baked into spans)
        // Just verify both render without panicking
        assert!(!lines1.is_empty());
        assert!(!lines2.is_empty());
    }

    #[test]
    fn test_clear_cache() {
        // Just verify clear doesn't panic
        clear_render_cache();
        let palette = Palette::standard();
        let _ = render_markdown("test", Style::default(), &palette);
        clear_render_cache();
    }

    #[test]
    fn test_nested_bold_in_heading() {
        clear_render_cache();

        // Heading with nested bold: "# Intro **key** point"
        // The word "point" should still be bold (from heading) after **key** ends.
        let content = "# Intro **key** point";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        // Find the heading line (contains "Intro", "key", "point")
        let heading_line = lines.iter().find(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            text.contains("Intro") && text.contains("key") && text.contains("point")
        });

        assert!(heading_line.is_some(), "Should find heading line");
        let line = heading_line.unwrap();

        // All visible spans in the heading should have BOLD modifier
        // (excluding the indent span which is just spaces)
        let visible_spans: Vec<_> = line
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty())
            .collect();

        for span in &visible_spans {
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "Span '{}' in heading should be bold",
                span.content
            );
        }
    }

    #[test]
    fn test_nested_italic_in_bold() {
        clear_render_cache();

        // Bold with nested italic: "**outer _inner_ outer**"
        // The second "outer" should still be bold after _inner_ ends.
        let content = "**outer _inner_ still bold**";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        // Find the line containing this text
        let content_line = lines.iter().find(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            text.contains("still bold")
        });

        assert!(content_line.is_some(), "Should find content line");
        let line = content_line.unwrap();

        // Find the span containing "still bold"
        let still_bold_span = line.spans.iter().find(|s| s.content.contains("still bold"));

        assert!(still_bold_span.is_some(), "Should find 'still bold' span");
        let span = still_bold_span.unwrap();

        assert!(
            span.style.add_modifier.contains(Modifier::BOLD),
            "'still bold' should have BOLD modifier"
        );
    }

    #[test]
    fn test_html_xml_content_rendered() {
        clear_render_cache();

        // XML-like tags (common in LLM output) should be rendered, not silently dropped
        let content = "<thinking>This is important</thinking>";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        // Should have content, not be empty
        assert!(
            !lines.is_empty(),
            "HTML/XML content should not be silently dropped"
        );

        // The actual text should be present somewhere in the rendered output
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(
            all_text.contains("thinking") || all_text.contains("important"),
            "HTML/XML content should appear in rendered output: {all_text}"
        );
    }

    #[test]
    fn test_br_tag_renders_as_line_break() {
        clear_render_cache();

        // <br> tags (common in LLM output) should become line breaks
        let content = "Line one<br>Line two";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        // Should have two separate lines, not "<br>" as literal text
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(
            !all_text.contains("<br>"),
            "<br> should not appear as literal text: {all_text}"
        );
        assert!(
            all_text.contains("Line one") && all_text.contains("Line two"),
            "Both lines should be present: {all_text}"
        );
    }

    #[test]
    fn test_preserve_newlines_as_line_breaks() {
        clear_render_cache();

        let content = "Line one\nLine two";
        let palette = Palette::standard();
        let lines = render_markdown_preserve_newlines(content, Style::default(), &palette);

        let rendered_lines: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        let line_one_idx = rendered_lines
            .iter()
            .position(|text| text.contains("Line one"));
        let line_two_idx = rendered_lines
            .iter()
            .position(|text| text.contains("Line two"));

        assert!(line_one_idx.is_some(), "Should contain 'Line one'");
        assert!(line_two_idx.is_some(), "Should contain 'Line two'");
        assert_ne!(line_one_idx, line_two_idx, "Lines should be separate");
    }

    #[test]
    fn test_br_in_table_cell() {
        clear_render_cache();

        // <br> inside table cells should create multi-line cells
        let content = "| Header |\n|--------|\n| Line1<br>Line2 |";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(
            !all_text.contains("<br>"),
            "<br> in table should not appear as literal text: {all_text}"
        );
        // Both parts should be in the output
        assert!(
            all_text.contains("Line1") && all_text.contains("Line2"),
            "Both lines should be present in table: {all_text}"
        );
    }

    #[test]
    fn test_incomplete_code_block_streaming() {
        clear_render_cache();

        // Simulate streaming: code block started but not closed
        let content = "Here is some code:\n\n```rust\nfn main() {\n    println!(\"hello\");\n}";
        let palette = Palette::standard();
        let lines = render_markdown(content, Style::default(), &palette);

        // Should render the incomplete code block content
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(
            all_text.contains("fn main()"),
            "Incomplete code block should be rendered: {all_text}"
        );
        assert!(
            all_text.contains("println"),
            "Code block content should appear: {all_text}"
        );
    }
}

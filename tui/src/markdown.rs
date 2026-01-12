//! Markdown to ratatui rendering
//!
//! Includes a simple render cache to avoid re-parsing unchanged markdown content.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::theme::colors;

/// Maximum number of cached renders before eviction.
const CACHE_MAX_ENTRIES: usize = 128;

/// Cache key combining content hash and style.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    content_hash: u64,
    style_hash: u64,
}

impl CacheKey {
    fn new(content: &str, style: Style) -> Self {
        use std::collections::hash_map::DefaultHasher;

        let mut content_hasher = DefaultHasher::new();
        content.hash(&mut content_hasher);

        let mut style_hasher = DefaultHasher::new();
        // Hash style components manually since Style doesn't impl Hash
        style.fg.hash(&mut style_hasher);
        style.bg.hash(&mut style_hasher);
        style.add_modifier.hash(&mut style_hasher);
        style.sub_modifier.hash(&mut style_hasher);

        Self {
            content_hash: content_hasher.finish(),
            style_hash: style_hasher.finish(),
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
pub fn render_markdown(content: &str, base_style: Style) -> Vec<Line<'static>> {
    let key = CacheKey::new(content, base_style);

    // Check cache first
    let cached = RENDER_CACHE.with(|cache| cache.borrow().get(&key).cloned());

    if let Some(lines) = cached {
        return lines;
    }

    // Cache miss - render and store
    let renderer = MarkdownRenderer::new(base_style);
    let lines = renderer.render(content);

    RENDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();

        // Simple eviction: clear half the cache when full
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
}

impl MarkdownRenderer {
    fn new(base_style: Style) -> Self {
        Self {
            base_style,
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
            // Render HTML/XML-like content as plain text to avoid silent content loss.
            // LLM responses may contain XML-like tags that pulldown_cmark parses as HTML.
            Event::Html(html) | Event::InlineHtml(html) => self.handle_text(&html),
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { .. } => {
                self.bold_count += 1;
            }
            Tag::Strong => {
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
                        let m = format!("{}{}. ", indent, idx);
                        *idx += 1;
                        m
                    }
                    _ => format!("{}• ", indent),
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
            TagEnd::Item => {
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
            TagEnd::Paragraph => {
                self.flush_line();
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
            .fg(colors::PEACH)
            .add_modifier(Modifier::BOLD);
        self.current_spans
            .push(Span::styled(code.to_string(), style));
    }

    fn handle_soft_break(&mut self) {
        if !self.in_code_block && !self.in_table {
            self.current_spans.push(Span::raw(" "));
        }
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
            style = style.fg(colors::PEACH);
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
        let code_style = Style::default().fg(colors::TEXT_MUTED);

        // Opening fence
        self.lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("```", Style::default().fg(colors::TEXT_MUTED)),
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
            Span::styled("```", Style::default().fg(colors::TEXT_MUTED)),
        ]));

        self.code_block_content.clear();
    }

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        // Calculate column widths
        let num_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
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

        let table_style = Style::default().fg(colors::TEXT_MUTED);
        let header_style = Style::default()
            .fg(colors::TEXT_PRIMARY)
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
            let cell = row.get(i).map(|s| s.trim()).unwrap_or("");
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

    #[test]
    fn test_simple_text() {
        let lines = render_markdown("Hello world", Style::default());
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let lines = render_markdown(md, Style::default());
        // Should have borders and content
        assert!(lines.len() >= 4);
    }

    #[test]
    fn test_cache_returns_same_result() {
        clear_render_cache();

        let content = "# Hello\n\nThis is **bold** and *italic* text.";
        let style = Style::default();

        // First render (cache miss)
        let lines1 = render_markdown(content, style);

        // Second render (cache hit) should return identical result
        let lines2 = render_markdown(content, style);

        assert_eq!(lines1.len(), lines2.len());
        for (l1, l2) in lines1.iter().zip(lines2.iter()) {
            assert_eq!(format!("{:?}", l1), format!("{:?}", l2));
        }
    }

    #[test]
    fn test_cache_different_styles_different_results() {
        clear_render_cache();

        let content = "Simple text";
        let style1 = Style::default();
        let style2 = Style::default().add_modifier(Modifier::BOLD);

        let lines1 = render_markdown(content, style1);
        let lines2 = render_markdown(content, style2);

        // Different styles may produce different results (style is baked into spans)
        // Just verify both render without panicking
        assert!(!lines1.is_empty());
        assert!(!lines2.is_empty());
    }

    #[test]
    fn test_clear_cache() {
        // Just verify clear doesn't panic
        clear_render_cache();
        render_markdown("test", Style::default());
        clear_render_cache();
    }

    #[test]
    fn test_nested_bold_in_heading() {
        clear_render_cache();

        // Heading with nested bold: "# Intro **key** point"
        // The word "point" should still be bold (from heading) after **key** ends.
        let content = "# Intro **key** point";
        let lines = render_markdown(content, Style::default());

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
        let lines = render_markdown(content, Style::default());

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
        let lines = render_markdown(content, Style::default());

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
}

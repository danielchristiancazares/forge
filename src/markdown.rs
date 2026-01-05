//! Markdown to ratatui rendering

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::theme::colors;

/// Render markdown content to ratatui Lines
pub fn render_markdown(content: &str, base_style: Style) -> Vec<Line<'static>> {
    let renderer = MarkdownRenderer::new(base_style);
    renderer.render(content)
}

struct MarkdownRenderer {
    base_style: Style,
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,

    // Style stack for nested formatting
    bold: bool,
    italic: bool,
    code: bool,

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
    list_depth: usize,
    list_index: Option<u64>,
}

impl MarkdownRenderer {
    fn new(base_style: Style) -> Self {
        Self {
            base_style,
            lines: Vec::new(),
            current_spans: Vec::new(),
            bold: false,
            italic: false,
            code: false,
            in_code_block: false,
            code_block_content: Vec::new(),
            in_table: false,
            table_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            table_alignments: Vec::new(),
            list_depth: 0,
            list_index: None,
        }
    }

    fn render(mut self, content: &str) -> Vec<Line<'static>> {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS;

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
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { .. } => {
                self.bold = true;
            }
            Tag::Strong => {
                self.bold = true;
            }
            Tag::Emphasis => {
                self.italic = true;
            }
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
                self.code_block_content.clear();
            }
            Tag::List(start) => {
                self.flush_line();
                self.list_depth += 1;
                self.list_index = start;
            }
            Tag::Item => {
                // Add list marker
                let indent = "    ".repeat(self.list_depth.saturating_sub(1));
                let marker = if let Some(ref mut idx) = self.list_index {
                    let m = format!("{}{}. ", indent, idx);
                    *idx += 1;
                    m
                } else {
                    format!("{}• ", indent)
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
                if !self.lines.is_empty() && self.list_depth == 0 {
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
                self.bold = false;
                self.flush_line();
                self.lines.push(Line::from(""));
            }
            TagEnd::Strong => {
                self.bold = false;
            }
            TagEnd::Emphasis => {
                self.italic = false;
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.render_code_block();
            }
            TagEnd::List(_) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                if self.list_depth == 0 {
                    self.list_index = None;
                }
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
                self.current_row.push(std::mem::take(&mut self.current_cell));
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
            .push(Span::styled(format!("`{}`", code), style));
    }

    fn handle_soft_break(&mut self) {
        if !self.in_code_block && !self.in_table {
            self.current_spans.push(Span::raw(" "));
        }
    }

    fn current_style(&self) -> Style {
        let mut style = self.base_style;

        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.code {
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
}

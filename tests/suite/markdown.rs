//! Markdown rendering tests

use forge_tui::markdown::render_markdown;
use ratatui::style::Style;

#[test]
fn renders_plain_text() {
    let lines = render_markdown("Hello world", Style::default());
    assert!(!lines.is_empty());
    
    // Should have indentation
    let text: String = lines.iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(text.contains("Hello world"));
}

#[test]
fn renders_bold_text() {
    let lines = render_markdown("This is **bold** text", Style::default());
    assert!(!lines.is_empty());
}

#[test]
fn renders_italic_text() {
    let lines = render_markdown("This is *italic* text", Style::default());
    assert!(!lines.is_empty());
}

#[test]
fn renders_inline_code() {
    let lines = render_markdown("Use `println!` macro", Style::default());
    let text: String = lines.iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(text.contains("`println!`"));
}

#[test]
fn renders_code_block() {
    let md = r#"```rust
fn main() {
    println!("Hello");
}
```"#;
    
    let lines = render_markdown(md, Style::default());
    // Should have opening fence, code lines, closing fence
    assert!(lines.len() >= 4);
}

#[test]
fn renders_simple_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |";
    let lines = render_markdown(md, Style::default());
    
    // Should have: top border, header, separator, data row, bottom border
    assert!(lines.len() >= 5);
    
    // Check for box drawing characters
    let text: String = lines.iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(text.contains("┌") || text.contains("│"));
}

#[test]
fn renders_multi_column_table() {
    let md = r#"| Test | Result | Notes |
|------|--------|-------|
| A    | Pass   | Good  |
| B    | Fail   | Bad   |"#;
    
    let lines = render_markdown(md, Style::default());
    assert!(lines.len() >= 6); // borders + header + separator + 2 data rows
}

#[test]
fn renders_bullet_list() {
    let md = "- Item 1\n- Item 2\n- Item 3";
    let lines = render_markdown(md, Style::default());
    
    let text: String = lines.iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(text.contains("•") || text.contains("-"));
}

#[test]
fn renders_numbered_list() {
    let md = "1. First\n2. Second\n3. Third";
    let lines = render_markdown(md, Style::default());
    
    let text: String = lines.iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect();
    assert!(text.contains("1.") || text.contains("First"));
}

#[test]
fn renders_header() {
    let md = "# Main Header\n\nSome content";
    let lines = render_markdown(md, Style::default());
    assert!(!lines.is_empty());
}

#[test]
fn empty_content_produces_no_lines() {
    let lines = render_markdown("", Style::default());
    assert!(lines.is_empty());
}

#[test]
fn whitespace_only_produces_minimal_output() {
    let lines = render_markdown("   \n\n   ", Style::default());
    // May produce empty lines or nothing
    assert!(lines.len() <= 3);
}

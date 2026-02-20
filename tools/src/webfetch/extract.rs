//! HTML → Markdown extraction pipeline.
//!
//! This module implements content extraction per FR-WF-12 through FR-WF-13:
//! - Boilerplate removal (tag-level, attribute-level, class/ID token matching)
//! - Main content detection with fallback cascade
//! - Deterministic HTML → Markdown conversion
//! - Title and language extraction
//! - URL resolution for links/images

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

use super::types::{ErrorCode, WebFetchError};
use forge_types::strip_steganographic_chars;

///
/// Per FR-WF-EXT-EMPTY-01, content < 50 scalar values after boilerplate
/// removal is considered "empty."
pub const MIN_EXTRACTED_CHARS: usize = 50;

///
/// Per FR-WF-12b: "nav" matches `class="nav"` but not `class="navigate"`.
const BOILERPLATE_TOKENS: &[&str] = &[
    "nav",
    "navbar",
    "navigation",
    "header",
    "footer",
    "sidebar",
    "menu",
    "breadcrumb",
    "breadcrumbs",
    "advertisement",
    "ad",
    "ads",
    "social",
    "share",
    "sharing",
    "comment",
    "comments",
    "related",
    "recommended",
    "popular",
    "trending",
    "subscribe",
    "newsletter",
    "cookie",
    "cookies",
    "banner",
    "popup",
    "modal",
    "overlay",
];

#[derive(Debug)]
pub struct ExtractedContent {
    /// Extracted Markdown content.
    pub markdown: String,

    /// Page title (from <title> or first <h1>).
    pub title: Option<String>,

    /// Page language (from <html lang>).
    pub language: Option<String>,
}

/// Context for Markdown conversion, tracking state across recursive calls.
struct ConversionContext {
    /// Base URL for resolving relative links.
    base_url: Url,

    /// Current list nesting depth.
    list_depth: usize,

    /// Whether we're inside a <pre> block (preserve whitespace).
    in_preformatted: bool,
}

impl ConversionContext {
    fn new(base_url: Url) -> Self {
        Self {
            base_url,
            list_depth: 0,
            in_preformatted: false,
        }
    }

    /// Resolve a URL against base or final URL.
    fn resolve_url(&self, href: &str) -> String {
        if let Ok(resolved) = self.base_url.join(href) {
            return resolved.to_string();
        }

        href.to_string()
    }

    fn resolve_http_url(&self, href: &str) -> Option<String> {
        let resolved = self.resolve_url(href.trim());
        let parsed = Url::parse(&resolved).ok()?;
        is_allowed_web_scheme(parsed.scheme()).then_some(parsed.to_string())
    }

    /// Get indent string for current list depth.
    fn list_indent(&self) -> String {
        "  ".repeat(self.list_depth.saturating_sub(1))
    }
}

///
/// Implements FR-WF-12 through FR-WF-13e:
/// 1. Parse HTML (lenient, handle malformed)
/// 2. Extract metadata (title, language)
/// 3. Find main content root
/// 4. Remove boilerplate (nav, footer, ads, etc.)
/// 5. Convert to Markdown
///
/// # Arguments
/// * `html` - Raw HTML content
/// * `final_url` - Final URL after redirects (for resolving relative URLs)
pub fn extract(html: &str, final_url: &Url) -> Result<ExtractedContent, WebFetchError> {
    // FR-WF-10h: Strip BOM and leading whitespace before processing
    let html = strip_bom_and_whitespace(html);

    // Parse HTML
    let document = Html::parse_document(html);

    // Extract metadata
    let title = extract_title(&document);
    let language = extract_language(&document);

    // Extract base URL from <base href> if present
    let base_url = extract_base_url(&document, final_url).unwrap_or_else(|| final_url.clone());

    // Find main content root per FR-WF-12a
    let root = find_content_root(&document);

    // Extract and convert to Markdown
    let markdown = match root {
        Some(element) => {
            let ctx = ConversionContext::new(base_url);
            let raw = convert_element_to_markdown(element, ctx);
            let normalized = normalize_whitespace_final(&raw);
            // Strip steganographic characters from web content before LLM ingestion
            strip_steganographic_chars(&normalized).into_owned()
        }
        None => {
            // FR-WF-EXT-EMPTY-02: All root candidates empty
            return Err(WebFetchError::new(
                ErrorCode::ExtractionFailed,
                "no extractable content found",
                false,
            ));
        }
    };

    // FR-WF-EXT-EMPTY-01: Check minimum content length
    let char_count = markdown.chars().filter(|c| !c.is_whitespace()).count();
    if char_count < MIN_EXTRACTED_CHARS {
        return Err(WebFetchError::new(
            ErrorCode::ExtractionFailed,
            format!(
                "extracted content too short ({char_count} non-whitespace chars, minimum {MIN_EXTRACTED_CHARS})"
            ),
            false,
        ));
    }

    Ok(ExtractedContent {
        markdown,
        title,
        language,
    })
}

fn strip_bom_and_whitespace(html: &str) -> &str {
    html.strip_prefix('\u{FEFF}').unwrap_or(html).trim_start()
}

///
/// Priority: <title> tag, then first <h1>.
/// Normalizes whitespace (trim + collapse).
fn extract_title(document: &Html) -> Option<String> {
    // Try <title> first
    if let Ok(selector) = Selector::parse("title")
        && let Some(title) = document.select(&selector).next()
    {
        let text = collapse_whitespace(&title.text().collect::<String>());
        if !text.is_empty() {
            return Some(text);
        }
    }

    // Fall back to first <h1>
    if let Ok(selector) = Selector::parse("h1")
        && let Some(h1) = document.select(&selector).next()
    {
        let text = collapse_whitespace(&h1.text().collect::<String>());
        if !text.is_empty() {
            return Some(text);
        }
    }

    None
}

fn extract_language(document: &Html) -> Option<String> {
    let selector = Selector::parse("html").ok()?;
    document
        .select(&selector)
        .next()?
        .value()
        .attr("lang")
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn extract_base_url(document: &Html, final_url: &Url) -> Option<Url> {
    let selector = Selector::parse("base[href]").ok()?;
    let base_elem = document.select(&selector).next()?;
    let href = base_elem.value().attr("href")?;

    // If href is relative, resolve against final_url
    let base = final_url
        .join(href)
        .ok()
        .or_else(|| Url::parse(href).ok())?;
    is_allowed_web_scheme(base.scheme()).then_some(base)
}

///
/// Per FR-WF-EXT-ROOT-01, tries in order:
/// 1. <main>
/// 2. <article>
/// 3. [role="main"]
/// 4. #content
/// 5. .content
/// 6. <body>
fn find_content_root(document: &Html) -> Option<ElementRef<'_>> {
    let selectors = [
        "main",
        "article",
        "[role=\"main\"]",
        "#content",
        ".content",
        "body",
    ];

    let mut best: Option<(usize, ElementRef<'_>)> = None;

    for selector_str in selectors {
        if let Ok(selector) = Selector::parse(selector_str) {
            for element in document.select(&selector) {
                let len = non_boilerplate_text_len(element);
                if len >= MIN_EXTRACTED_CHARS {
                    return Some(element);
                }

                // If nothing meets the threshold, fall back to the "best" non-empty candidate.
                // This keeps extraction robust for borderline pages where Markdown markup pushes
                // the final character count over the minimum.
                if len > 0 && best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                    best = Some((len, element));
                }
            }
        }
    }

    best.map(|(_, element)| element)
}

fn non_boilerplate_text_len(element: ElementRef<'_>) -> usize {
    if is_boilerplate_element(element) {
        return 0;
    }

    let mut count = 0;
    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                count += text.chars().filter(|c| !c.is_whitespace()).count();
            }
            Node::Element(_) => {
                if let Some(el) = ElementRef::wrap(child) {
                    count += non_boilerplate_text_len(el);
                }
            }
            _ => {}
        }
    }
    count
}

fn is_boilerplate_element(element: ElementRef<'_>) -> bool {
    let tag = element.value().name();

    // Tag-level boilerplate
    if matches!(
        tag,
        "script" | "style" | "noscript" | "nav" | "footer" | "header" | "aside" | "form"
    ) {
        return true;
    }

    // Attribute-level boilerplate
    if element.value().attr("aria-hidden") == Some("true") {
        return true;
    }
    if element.value().attr("hidden").is_some() {
        return true;
    }
    if element.value().attr("role") == Some("navigation") {
        return true;
    }

    // Class token matching (case-insensitive)
    if let Some(class) = element.value().attr("class")
        && has_boilerplate_token(class)
    {
        return true;
    }

    // ID token matching (case-insensitive)
    if let Some(id) = element.value().attr("id")
        && has_boilerplate_token(id)
    {
        return true;
    }

    false
}

///
/// TOKEN matching means space-separated, not substring:
/// - "nav" matches in "nav sidebar" but not in "navigate"
fn has_boilerplate_token(attr: &str) -> bool {
    let lower = attr.to_lowercase();
    for token in lower.split_whitespace() {
        if BOILERPLATE_TOKENS.contains(&token) {
            return true;
        }
    }
    false
}

fn convert_element_to_markdown(root: ElementRef<'_>, mut ctx: ConversionContext) -> String {
    let mut output = String::new();
    convert_children(&mut output, root, &mut ctx);
    output
}

fn convert_children(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(el) = ElementRef::wrap(child) {
                    convert_element(output, el, ctx);
                }
            }
            Node::Text(text) => {
                if ctx.in_preformatted {
                    output.push_str(text);
                } else {
                    // Collapse whitespace in normal text
                    let collapsed = collapse_inline_whitespace(text);
                    if !collapsed.is_empty() {
                        output.push_str(&collapsed);
                    }
                }
            }
            _ => {}
        }
    }
}

fn convert_element(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    // Skip boilerplate elements
    if is_boilerplate_element(element) {
        return;
    }

    let tag = element.value().name();

    match tag {
        // Headings
        "h1" => convert_heading(output, element, ctx, 1),
        "h2" => convert_heading(output, element, ctx, 2),
        "h3" => convert_heading(output, element, ctx, 3),
        "h4" => convert_heading(output, element, ctx, 4),
        "h5" => convert_heading(output, element, ctx, 5),
        "h6" => convert_heading(output, element, ctx, 6),

        // Block elements
        "p" => convert_paragraph(output, element, ctx),
        "blockquote" => convert_blockquote(output, element, ctx),
        "div" | "section" | "article" | "main" => {
            convert_children(output, element, ctx);
            ensure_blank_line(output);
        }

        // Lists
        "ul" => convert_unordered_list(output, element, ctx),
        "ol" => convert_ordered_list(output, element, ctx),

        // Code
        "pre" => convert_pre(output, element, ctx),
        "code" => {
            // Inline code (not inside <pre>)
            if ctx.in_preformatted {
                output.push_str(&element.text().collect::<String>());
            } else {
                output.push('`');
                output.push_str(&element.text().collect::<String>());
                output.push('`');
            }
        }

        // Links and images
        "a" => convert_link(output, element, ctx),
        "img" => convert_image(output, element, ctx),

        // Tables
        "table" => convert_table(output, element, ctx),

        // Inline formatting
        "strong" | "b" => {
            output.push_str("**");
            convert_children(output, element, ctx);
            output.push_str("**");
        }
        "em" | "i" => {
            output.push('*');
            convert_children(output, element, ctx);
            output.push('*');
        }
        "del" | "s" | "strike" => {
            output.push_str("~~");
            convert_children(output, element, ctx);
            output.push_str("~~");
        }

        // Line breaks
        "br" => output.push('\n'),
        "hr" => {
            ensure_blank_line(output);
            output.push_str("---\n\n");
        }

        // Definition lists
        "dl" => convert_definition_list(output, element, ctx),

        // Figure with caption
        "figure" => convert_figure(output, element, ctx),

        // Skip known non-content elements
        "script" | "style" | "noscript" | "nav" | "footer" | "header" | "aside" | "form"
        | "input" | "button" | "select" | "textarea" | "iframe" | "object" | "embed" | "canvas"
        | "svg" | "video" | "audio" | "source" | "track" | "map" | "area" => {}

        // Inline elements and unknown elements - just recurse
        _ => {
            convert_children(output, element, ctx);
        }
    }
}

fn convert_heading(
    output: &mut String,
    element: ElementRef<'_>,
    ctx: &mut ConversionContext,
    level: usize,
) {
    ensure_blank_line(output);
    for _ in 0..level {
        output.push('#');
    }
    output.push(' ');

    // Collect heading text with inline formatting
    let mut heading_text = String::new();
    convert_children(&mut heading_text, element, ctx);
    output.push_str(collapse_whitespace(&heading_text).trim());
    output.push_str("\n\n");
}

fn convert_paragraph(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    ensure_blank_line(output);
    convert_children(output, element, ctx);
    output.push_str("\n\n");
}

fn convert_blockquote(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    ensure_blank_line(output);

    let mut content = String::new();
    convert_children(&mut content, element, ctx);

    // Prefix each line with >
    for line in content.lines() {
        output.push_str("> ");
        output.push_str(line);
        output.push('\n');
    }
    output.push('\n');
}

fn convert_unordered_list(
    output: &mut String,
    element: ElementRef<'_>,
    ctx: &mut ConversionContext,
) {
    if ctx.list_depth == 0 {
        ensure_blank_line(output);
    }

    ctx.list_depth += 1;
    let indent = ctx.list_indent();

    for child in element.children() {
        if let Some(li) = ElementRef::wrap(child)
            && li.value().name() == "li"
        {
            output.push_str(&indent);
            output.push_str("- ");
            convert_list_item_content(output, li, ctx);
        }
    }

    ctx.list_depth -= 1;

    if ctx.list_depth == 0 {
        output.push('\n');
    }
}

fn convert_ordered_list(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    if ctx.list_depth == 0 {
        ensure_blank_line(output);
    }

    ctx.list_depth += 1;
    let indent = ctx.list_indent();

    let start: usize = element
        .value()
        .attr("start")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let mut i = start;
    for child in element.children() {
        if let Some(li) = ElementRef::wrap(child)
            && li.value().name() == "li"
        {
            output.push_str(&indent);
            output.push_str(&format!("{i}. "));
            convert_list_item_content(output, li, ctx);
            i += 1;
        }
    }

    ctx.list_depth -= 1;

    if ctx.list_depth == 0 {
        output.push('\n');
    }
}

fn convert_list_item_content(output: &mut String, li: ElementRef<'_>, ctx: &mut ConversionContext) {
    let has_nested_list = li
        .children()
        .any(|c| ElementRef::wrap(c).is_some_and(|e| matches!(e.value().name(), "ul" | "ol")));

    if has_nested_list {
        let mut first_text = true;
        for child in li.children() {
            if let Some(el) = ElementRef::wrap(child) {
                let tag = el.value().name();
                if tag == "ul" || tag == "ol" {
                    if first_text {
                        output.push('\n');
                        first_text = false;
                    }
                    convert_element(output, el, ctx);
                } else {
                    let mut text = String::new();
                    convert_element(&mut text, el, ctx);
                    if first_text && !text.trim().is_empty() {
                        output.push_str(text.trim());
                        first_text = false;
                    }
                }
            } else if let Some(text) = child.value().as_text() {
                let trimmed = collapse_inline_whitespace(text);
                if first_text && !trimmed.is_empty() {
                    output.push_str(&trimmed);
                    first_text = false;
                }
            }
        }
        if first_text {
            output.push('\n');
        }
    } else {
        let mut content = String::new();
        convert_children(&mut content, li, ctx);
        output.push_str(collapse_whitespace(&content).trim());
        output.push('\n');
    }
}

fn convert_pre(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    ensure_blank_line(output);

    // Try to find language hint from <code class="language-*">
    let mut language = String::new();
    for child in element.children() {
        if let Some(code_el) = ElementRef::wrap(child)
            && code_el.value().name() == "code"
            && let Some(class) = code_el.value().attr("class")
        {
            for cls in class.split_whitespace() {
                if let Some(lang) = cls.strip_prefix("language-") {
                    language = lang.to_string();
                    break;
                }
                if let Some(lang) = cls.strip_prefix("lang-") {
                    language = lang.to_string();
                    break;
                }
            }
        }
    }

    output.push_str("```");
    output.push_str(&language);
    output.push('\n');

    // Preserve whitespace in preformatted blocks
    let was_preformatted = ctx.in_preformatted;
    ctx.in_preformatted = true;

    let code_text: String = element.text().collect();
    // Trim a single leading/trailing newline but preserve internal whitespace
    let trimmed = code_text
        .strip_prefix('\n')
        .unwrap_or(&code_text)
        .strip_suffix('\n')
        .unwrap_or(&code_text);
    output.push_str(trimmed);

    ctx.in_preformatted = was_preformatted;

    // Ensure newline before closing fence
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("```\n\n");
}

fn convert_link(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    let href = element.value().attr("href").unwrap_or("");

    if href.is_empty() {
        convert_children(output, element, ctx);
        return;
    }

    let Some(resolved_href) = ctx.resolve_http_url(href) else {
        convert_children(output, element, ctx);
        return;
    };

    // Collect link text
    let mut text = String::new();
    convert_children(&mut text, element, ctx);
    let text = collapse_whitespace(&text);

    if text.is_empty() {
        output.push_str(&resolved_href);
    } else {
        output.push('[');
        output.push_str(&text);
        output.push_str("](");
        output.push_str(&resolved_href);
        output.push(')');
    }
}

fn convert_image(output: &mut String, element: ElementRef<'_>, ctx: &ConversionContext) {
    let src = element.value().attr("src").unwrap_or("");
    if src.is_empty() {
        return;
    }

    let alt = element.value().attr("alt").unwrap_or("");
    let Some(resolved_src) = ctx.resolve_http_url(src) else {
        return;
    };

    // FR-WF-EXT-IMG-01: Only include if alt is non-empty
    if !alt.is_empty() {
        output.push_str("![");
        output.push_str(alt);
        output.push_str("](");
        output.push_str(&resolved_src);
        output.push(')');
    }
}

fn convert_table(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    ensure_blank_line(output);

    // Collect all rows (from thead, tbody, or direct tr children)
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut header_row_count = 0;

    // Process thead
    if let Ok(thead_sel) = Selector::parse("thead") {
        for thead in element.select(&thead_sel) {
            if let Ok(tr_sel) = Selector::parse("tr") {
                for tr in thead.select(&tr_sel) {
                    rows.push(collect_table_cells(tr, ctx));
                    header_row_count += 1;
                }
            }
        }
    }

    // Process tbody or direct tr children
    if let Ok(tbody_sel) = Selector::parse("tbody") {
        for tbody in element.select(&tbody_sel) {
            if let Ok(tr_sel) = Selector::parse("tr") {
                for tr in tbody.select(&tr_sel) {
                    rows.push(collect_table_cells(tr, ctx));
                }
            }
        }
    }

    // Direct tr children (no thead/tbody)
    if rows.is_empty() {
        if let Ok(tr_sel) = Selector::parse("tr") {
            for tr in element.select(&tr_sel) {
                rows.push(collect_table_cells(tr, ctx));
            }
        }
        // First row is header if we have no explicit thead
        if !rows.is_empty() {
            header_row_count = 1;
        }
    }

    if rows.is_empty() {
        return;
    }

    // Find max column count
    let col_count = rows.iter().map(std::vec::Vec::len).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }

    // Ensure all rows have same column count
    for row in &mut rows {
        while row.len() < col_count {
            row.push(String::new());
        }
    }

    // Calculate column widths for alignment
    let col_widths: Vec<usize> = (0..col_count)
        .map(|col| {
            rows.iter()
                .map(|row| row.get(col).map_or(0, std::string::String::len))
                .max()
                .unwrap_or(3)
                .max(3)
        })
        .collect();

    // Output header row(s)
    if header_row_count > 0 {
        for row in rows.iter().take(header_row_count) {
            output.push('|');
            for (i, cell) in row.iter().enumerate() {
                output.push(' ');
                output.push_str(cell);
                // Pad to column width
                for _ in cell.len()..col_widths[i] {
                    output.push(' ');
                }
                output.push_str(" |");
            }
            output.push('\n');
        }

        // Separator row
        output.push('|');
        for width in &col_widths {
            output.push(' ');
            for _ in 0..*width {
                output.push('-');
            }
            output.push_str(" |");
        }
        output.push('\n');
    }

    // Output body rows
    for row in rows.iter().skip(header_row_count) {
        output.push('|');
        for (i, cell) in row.iter().enumerate() {
            output.push(' ');
            output.push_str(cell);
            for _ in cell.len()..col_widths[i] {
                output.push(' ');
            }
            output.push_str(" |");
        }
        output.push('\n');
    }

    output.push('\n');
}

fn collect_table_cells(tr: ElementRef<'_>, ctx: &mut ConversionContext) -> Vec<String> {
    let mut cells = Vec::new();

    for child in tr.children() {
        if let Some(cell_el) = ElementRef::wrap(child) {
            let tag = cell_el.value().name();
            if tag == "td" || tag == "th" {
                let mut cell_text = String::new();
                convert_children(&mut cell_text, cell_el, ctx);
                // Escape pipes in cell content
                let escaped = collapse_whitespace(&cell_text).trim().replace('|', "\\|");
                cells.push(escaped);
            }
        }
    }

    cells
}

fn convert_definition_list(
    output: &mut String,
    element: ElementRef<'_>,
    ctx: &mut ConversionContext,
) {
    ensure_blank_line(output);

    for child in element.children() {
        if let Some(el) = ElementRef::wrap(child) {
            match el.value().name() {
                "dt" => {
                    let mut term = String::new();
                    convert_children(&mut term, el, ctx);
                    output.push_str("**");
                    output.push_str(collapse_whitespace(&term).trim());
                    output.push_str("**\n");
                }
                "dd" => {
                    let mut def = String::new();
                    convert_children(&mut def, el, ctx);
                    output.push_str(": ");
                    output.push_str(collapse_whitespace(&def).trim());
                    output.push_str("\n\n");
                }
                _ => {}
            }
        }
    }
}

fn convert_figure(output: &mut String, element: ElementRef<'_>, ctx: &mut ConversionContext) {
    ensure_blank_line(output);

    // Look for img
    if let Ok(img_sel) = Selector::parse("img") {
        for img in element.select(&img_sel) {
            convert_image(output, img, ctx);
            output.push('\n');
        }
    }

    // Look for figcaption
    if let Ok(caption_sel) = Selector::parse("figcaption") {
        for caption in element.select(&caption_sel) {
            let mut text = String::new();
            convert_children(&mut text, caption, ctx);
            let text = collapse_whitespace(&text);
            if !text.is_empty() {
                output.push('*');
                output.push_str(text.trim());
                output.push_str("*\n");
            }
        }
    }

    output.push('\n');
}

/// Ensure output ends with a blank line (for block elements).
fn ensure_blank_line(output: &mut String) {
    if output.is_empty() {
        return;
    }

    // Count trailing newlines
    let trailing_newlines = output.chars().rev().take_while(|&c| c == '\n').count();

    if trailing_newlines == 0 {
        output.push_str("\n\n");
    } else if trailing_newlines == 1 {
        output.push('\n');
    }
    // If >= 2, already has blank line
}

/// Collapse whitespace to single spaces (for inline text).
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapse inline whitespace but preserve leading/trailing single space.
fn collapse_inline_whitespace(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    let has_leading = s.chars().next().is_some_and(char::is_whitespace);
    let has_trailing = s.chars().last().is_some_and(char::is_whitespace);

    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");

    if collapsed.is_empty() {
        if has_leading || has_trailing {
            return " ".to_string();
        }
        return String::new();
    }

    let mut result = String::new();
    if has_leading {
        result.push(' ');
    }
    result.push_str(&collapsed);
    if has_trailing && !has_leading {
        result.push(' ');
    } else if has_trailing && has_leading && collapsed.len() > 1 {
        // Both leading and trailing, preserve trailing too
        result.push(' ');
    }

    result
}

/// Final whitespace normalization per FR-WF-EXT-WS-01.
///
/// - CRLF → LF
/// - Collapse >2 consecutive blank lines to 2
/// - Trim trailing whitespace from each line
/// - Ensure single final newline
fn normalize_whitespace_final(s: &str) -> String {
    // CRLF → LF
    let s = s.replace("\r\n", "\n");

    let mut lines: Vec<&str> = Vec::new();
    let mut blank_count = 0;

    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                lines.push("");
            }
        } else {
            blank_count = 0;
            lines.push(trimmed);
        }
    }

    // Remove trailing blank lines
    while lines.last() == Some(&"") {
        lines.pop();
    }

    // Join and add single final newline
    let mut result = lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }

    result
}

fn is_allowed_web_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

#[cfg(test)]
mod tests {
    use super::{
        ConversionContext, Html, MIN_EXTRACTED_CHARS, Url, collapse_whitespace, extract,
        extract_language, extract_title, has_boilerplate_token, normalize_whitespace_final,
        strip_bom_and_whitespace,
    };

    #[test]
    fn extract_falls_back_when_main_is_boilerplate_heavy() {
        let nav = "nav ".repeat(MIN_EXTRACTED_CHARS);
        let article = "real content ".repeat(MIN_EXTRACTED_CHARS);
        let html = format!(
            r"
            <html>
              <body>
                <main>
                  <nav>{nav}</nav>
                  <p>tiny</p>
                </main>
                <article><p>{article}</p></article>
              </body>
            </html>
            "
        );

        let final_url = Url::parse("https://example.com/").unwrap();
        let extracted = extract(&html, &final_url).expect("extract should succeed via <article>");

        assert!(extracted.markdown.contains("real content"));
        assert!(!extracted.markdown.contains("tiny"));
        assert!(!extracted.markdown.contains("nav"));
    }

    #[test]
    fn test_strip_bom() {
        let html = "\u{FEFF}<html><body>test</body></html>";
        let stripped = strip_bom_and_whitespace(html);
        assert!(stripped.starts_with("<html>"));
    }

    #[test]
    fn test_collapse_whitespace() {
        assert_eq!(collapse_whitespace("  hello   world  "), "hello world");
        assert_eq!(collapse_whitespace("\n\t foo \n bar \t"), "foo bar");
    }

    #[test]
    fn test_normalize_whitespace_final() {
        // CRLF conversion
        assert_eq!(normalize_whitespace_final("a\r\nb"), "a\nb\n");

        // Collapse >2 blank lines
        assert_eq!(normalize_whitespace_final("a\n\n\n\nb"), "a\n\n\nb\n");

        // Trim trailing whitespace
        assert_eq!(
            normalize_whitespace_final("hello   \nworld  "),
            "hello\nworld\n"
        );

        // Single final newline
        assert_eq!(normalize_whitespace_final("hello\n\n\n"), "hello\n");
    }

    #[test]
    fn test_boilerplate_token_matching() {
        // Token match
        assert!(has_boilerplate_token("nav sidebar"));
        assert!(has_boilerplate_token("footer"));
        assert!(has_boilerplate_token("HEADER")); // case-insensitive

        // Not substring match
        assert!(!has_boilerplate_token("navigation-link")); // "navigation-link" is one token
        assert!(!has_boilerplate_token("navigate"));
        assert!(!has_boilerplate_token("advertising")); // not exact "ad"
    }

    #[test]
    fn test_extract_title_from_title_tag() {
        let html = "<html><head><title>Test Page</title></head><body><h1>Hello</h1><p>Content here that is long enough to pass minimum.</p></body></html>";
        let doc = Html::parse_document(html);
        assert_eq!(extract_title(&doc), Some("Test Page".to_string()));
    }

    #[test]
    fn test_extract_title_fallback_to_h1() {
        let html = "<html><head></head><body><h1>Fallback Title</h1><p>Content</p></body></html>";
        let doc = Html::parse_document(html);
        assert_eq!(extract_title(&doc), Some("Fallback Title".to_string()));
    }

    #[test]
    fn test_extract_language() {
        let html = "<html lang=\"en-US\"><body><p>Content</p></body></html>";
        let doc = Html::parse_document(html);
        assert_eq!(extract_language(&doc), Some("en-US".to_string()));
    }

    #[test]
    fn test_url_resolution() {
        let base = Url::parse("https://example.com/page/").unwrap();
        let ctx = ConversionContext::new(base);

        assert_eq!(
            ctx.resolve_url("../images/foo.png"),
            "https://example.com/images/foo.png"
        );
        assert_eq!(
            ctx.resolve_url("/absolute/path"),
            "https://example.com/absolute/path"
        );
        assert_eq!(
            ctx.resolve_url("https://other.com/external"),
            "https://other.com/external"
        );
    }

    #[test]
    fn test_table_pipe_escaping() {
        let escaped = "a|b".replace('|', "\\|");
        assert_eq!(escaped, "a\\|b");
    }

    #[test]
    fn test_mixed_case_javascript_links_are_not_emitted() {
        let html = r#"
            <html>
              <body>
                <main>
                  <p>
                    <a href="JaVaScRiPt:alert(1)">Click me</a>
                    This filler text ensures extraction clears the minimum character threshold.
                  </p>
                </main>
              </body>
            </html>
        "#;
        let final_url = Url::parse("https://example.com/").unwrap();
        let extracted = extract(html, &final_url).expect("extract");
        assert!(extracted.markdown.contains("Click me"));
        assert!(!extracted.markdown.contains("javascript:"));
        assert!(!extracted.markdown.contains("]("));
    }

    #[test]
    fn test_non_http_base_href_is_ignored_for_resolution() {
        let html = r#"
            <html>
              <head>
                <base href="file:///etc/">
              </head>
              <body>
                <main>
                  <p>
                    <a href="passwd">passwd link</a>
                    This filler text ensures extraction clears the minimum character threshold.
                  </p>
                </main>
              </body>
            </html>
        "#;
        let final_url = Url::parse("https://example.com/docs/").unwrap();
        let extracted = extract(html, &final_url).expect("extract");
        assert!(
            extracted
                .markdown
                .contains("[passwd link](https://example.com/docs/passwd)")
        );
        assert!(!extracted.markdown.contains("file:///"));
    }

    #[test]
    fn test_non_http_image_sources_are_not_emitted() {
        let html = r#"
            <html>
              <body>
                <main>
                  <p>
                    <img alt="badge" src="DaTa:text/plain;base64,Zm9v">
                    This filler text ensures extraction clears the minimum character threshold.
                  </p>
                </main>
              </body>
            </html>
        "#;
        let final_url = Url::parse("https://example.com/").unwrap();
        let extracted = extract(html, &final_url).expect("extract");
        assert!(!extracted.markdown.contains("![badge]("));
    }
}

//! Token-aware content chunking.
//!
//! This module implements chunking per FR-WF-14 through FR-WF-15:
//! - Token-bounded chunks
//! - Block-based splitting (paragraphs, code fences, lists)
//! - Heading-aware state machine
//! - Code block atomicity
//! - List block detection and splitting

use forge_context::TokenCounter;

use super::types::FetchChunk;

/// Block types detected during parsing.
#[derive(Debug, Clone)]
enum Block {
    /// ATX heading with normalized text (without # prefix).
    Heading { text: String, raw: String },
    /// Paragraph: non-blank, non-heading, non-code, non-list content.
    Paragraph(String),
    /// Fenced code block: language hint and content lines.
    CodeFence {
        fence: String,
        language: String,
        content: Vec<String>,
    },
    /// List block: consecutive list items with continuation lines.
    List(String),
    /// One or more blank lines (separator).
    BlankLines(String),
}

/// Parse Markdown content into blocks.
fn parse_blocks(markdown: &str) -> Vec<Block> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Check for blank lines
        if line.trim().is_empty() {
            let mut blank_text = String::new();
            while i < lines.len() && lines[i].trim().is_empty() {
                if !blank_text.is_empty() {
                    blank_text.push('\n');
                }
                blank_text.push_str(lines[i]);
                i += 1;
            }
            blocks.push(Block::BlankLines(blank_text));
            continue;
        }

        // Check for ATX heading
        if let Some((_level, text, raw)) = parse_atx_heading(line) {
            blocks.push(Block::Heading { text, raw });
            i += 1;
            continue;
        }

        // Check for fenced code block
        if let Some((fence, language)) = parse_fence_start(line) {
            let mut content = Vec::new();
            let opening_line = line.to_string();
            i += 1;

            // Collect lines until closing fence or EOF
            while i < lines.len() {
                let current = lines[i];
                if is_fence_close(current, &fence) {
                    content.push(current.to_string()); // Include closing fence
                    i += 1;
                    break;
                }
                content.push(current.to_string());
                i += 1;
            }

            // Build full content with opening fence
            let mut full_content = vec![opening_line];
            full_content.extend(content);

            blocks.push(Block::CodeFence {
                fence,
                language,
                content: full_content,
            });
            continue;
        }

        // Check for list block (consecutive list items + continuations)
        if is_list_item_start(line) {
            let mut list_text = String::new();

            while i < lines.len() {
                let current = lines[i];

                // Check if this line is a list item or continuation
                if is_list_item_start(current) {
                    if !list_text.is_empty() {
                        list_text.push('\n');
                    }
                    list_text.push_str(current);
                    i += 1;
                } else if is_list_continuation(current) && !list_text.is_empty() {
                    list_text.push('\n');
                    list_text.push_str(current);
                    i += 1;
                } else if current.trim().is_empty() {
                    // Blank line might be followed by more list items
                    // Look ahead to see if list continues
                    let mut lookahead = i + 1;
                    while lookahead < lines.len() && lines[lookahead].trim().is_empty() {
                        lookahead += 1;
                    }
                    if lookahead < lines.len() && is_list_item_start(lines[lookahead]) {
                        // List continues after blank lines - include blanks
                        while i < lookahead {
                            list_text.push('\n');
                            list_text.push_str(lines[i]);
                            i += 1;
                        }
                    } else {
                        // List ends here
                        break;
                    }
                } else {
                    // Non-list, non-blank line ends the list
                    break;
                }
            }

            blocks.push(Block::List(list_text));
            continue;
        }

        // Default: paragraph (collect until blank line or special block)
        let mut para_text = String::new();
        while i < lines.len() {
            let current = lines[i];

            // End paragraph on blank line, heading, fence, or list
            if current.trim().is_empty()
                || parse_atx_heading(current).is_some()
                || parse_fence_start(current).is_some()
                || is_list_item_start(current)
            {
                break;
            }

            if !para_text.is_empty() {
                para_text.push('\n');
            }
            para_text.push_str(current);
            i += 1;
        }

        if !para_text.is_empty() {
            blocks.push(Block::Paragraph(para_text));
        }
    }

    blocks
}

fn parse_atx_heading(line: &str) -> Option<(u8, String, String)> {
    let trimmed = line.trim_start();

    if !trimmed.starts_with('#') {
        return None;
    }

    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }

    let after_hashes = &trimmed[level..];

    // Must have space after # (or be just # with nothing after)
    if !after_hashes.is_empty() && !after_hashes.starts_with(' ') && !after_hashes.starts_with('\t')
    {
        return None;
    }

    // FR-WF-CHK-HEAD-03: Strip trailing hashes preceded by whitespace
    let text = after_hashes.trim();
    let text = text
        .trim_end_matches('#')
        .trim_end_matches(|c: char| c.is_whitespace())
        .trim();

    if text.is_empty() && after_hashes.trim().is_empty() {
        // Heading like "##" with no content - not valid
        return None;
    }

    Some((level as u8, normalize_whitespace(text), line.to_string()))
}

fn parse_fence_start(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();

    // Check for ``` or ~~~
    let fence_char = if trimmed.starts_with('`') {
        '`'
    } else if trimmed.starts_with('~') {
        '~'
    } else {
        return None;
    };

    let fence_len = trimmed.chars().take_while(|c| *c == fence_char).count();
    if fence_len < 3 {
        return None;
    }

    let fence = fence_char.to_string().repeat(fence_len);
    let after_fence = &trimmed[fence_len..];
    let language = after_fence.split_whitespace().next().unwrap_or("");

    Some((fence, language.to_string()))
}

fn is_fence_close(line: &str, opening_fence: &str) -> bool {
    let trimmed = line.trim();
    let fence_char = opening_fence.chars().next().unwrap_or('`');

    // Must start with same fence character
    if !trimmed.starts_with(fence_char) {
        return false;
    }

    let fence_len = trimmed.chars().take_while(|c| *c == fence_char).count();

    // Closing fence must be at least as long as opening
    if fence_len < opening_fence.len() {
        return false;
    }

    // Nothing after the fence except whitespace
    trimmed[fence_len..].trim().is_empty()
}

///
/// Matches: `^\s{0,3}(?:[-+*]|\d+[.)])\s+`
fn is_list_item_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    let leading_spaces = line.len() - trimmed.len();

    // At most 3 spaces of indentation for a new list item
    if leading_spaces > 3 {
        return false;
    }

    // Unordered: - + *
    if trimmed.starts_with("- ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("* ")
        || trimmed == "-"
        || trimmed == "+"
        || trimmed == "*"
    {
        return true;
    }

    // Ordered: 1. 1) etc.
    let mut chars = trimmed.chars().peekable();
    let mut has_digit = false;

    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            has_digit = true;
            chars.next();
        } else {
            break;
        }
    }

    if has_digit
        && let Some(marker) = chars.next()
        && (marker == '.' || marker == ')')
    {
        // Must have space after or be at end
        return chars.next().is_none_or(|c| c == ' ' || c == '\t');
    }

    false
}

fn is_list_continuation(line: &str) -> bool {
    // Not blank, not a new list item, but indented
    if line.trim().is_empty() {
        return false;
    }

    let trimmed = line.trim_start();
    let leading = line.len() - trimmed.len();

    // Must have at least 2 spaces or a tab
    (leading >= 2 || line.starts_with('\t')) && !is_list_item_start(line)
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

///
/// Implements FR-WF-14 through FR-WF-15:
/// - Block-based splitting
/// - Heading state machine (FR-WF-CHK-HEAD-01)
/// - Code block atomicity (FR-WF-CHK-CODE-01)
/// - List block handling (FR-WF-CHK-LIST-01)
/// - Token counting via tiktoken
pub fn chunk(markdown: &str, max_tokens: u32) -> Vec<FetchChunk> {
    let counter = TokenCounter::new();
    let blocks = parse_blocks(markdown);

    let mut chunks = Vec::new();
    let mut current_heading = String::new();
    let mut current_text = String::new();
    let mut current_tokens: u32 = 0;

    for block in blocks {
        match block {
            Block::Heading { text, raw, .. } => {
                // Flush current chunk before updating heading
                if has_content(&current_text) {
                    chunks.push(FetchChunk {
                        heading: current_heading.clone(),
                        text: current_text.clone(),
                        token_count: current_tokens,
                    });
                    current_text.clear();
                }

                // Update heading state (FR-WF-CHK-HEAD-01)
                current_heading = text;

                // Start new chunk with heading line
                current_text = raw;
                current_tokens = counter.count_str(&current_text);
            }

            Block::BlankLines(blanks) => {
                // Preserve blank lines as separators (FR-WF-15a1)
                if !current_text.is_empty() {
                    current_text.push_str(&blanks);
                    if !blanks.is_empty() {
                        current_text.push('\n');
                    }
                    // Don't count blank lines toward token budget (they're minimal)
                }
            }

            Block::Paragraph(text) => {
                let block_tokens = counter.count_str(&text);

                if current_tokens + block_tokens > max_tokens && has_content(&current_text) {
                    chunks.push(FetchChunk {
                        heading: current_heading.clone(),
                        text: trim_block_separators(&current_text),
                        token_count: current_tokens,
                    });
                    current_text.clear();
                    current_tokens = 0;
                }

                // Handle oversized paragraph
                if block_tokens > max_tokens {
                    // Flush any accumulated content first
                    if has_content(&current_text) {
                        chunks.push(FetchChunk {
                            heading: current_heading.clone(),
                            text: trim_block_separators(&current_text),
                            token_count: current_tokens,
                        });
                        current_text.clear();
                        current_tokens = 0;
                    }

                    // Split oversized paragraph
                    let split_chunks =
                        split_oversized_text(&text, max_tokens, &counter, &current_heading);
                    chunks.extend(split_chunks);
                } else {
                    // Append to current chunk
                    append_block(&mut current_text, &text);
                    current_tokens = counter.count_str(&trim_block_separators(&current_text));
                }
            }

            Block::CodeFence {
                fence,
                language,
                content,
            } => {
                let block_text = content.join("\n");
                let block_tokens = counter.count_str(&block_text);

                if current_tokens + block_tokens > max_tokens && has_content(&current_text) {
                    chunks.push(FetchChunk {
                        heading: current_heading.clone(),
                        text: trim_block_separators(&current_text),
                        token_count: current_tokens,
                    });
                    current_text.clear();
                    current_tokens = 0;
                }

                // Handle oversized code block (FR-WF-CHK-CODE-01)
                if block_tokens > max_tokens {
                    // Flush any accumulated content first
                    if has_content(&current_text) {
                        chunks.push(FetchChunk {
                            heading: current_heading.clone(),
                            text: trim_block_separators(&current_text),
                            token_count: current_tokens,
                        });
                        current_text.clear();
                        current_tokens = 0;
                    }

                    // Split oversized code block at line boundaries
                    let split_chunks = split_oversized_code(
                        &fence,
                        &language,
                        &content,
                        max_tokens,
                        &counter,
                        &current_heading,
                    );
                    chunks.extend(split_chunks);
                } else {
                    // Append to current chunk
                    append_block(&mut current_text, &block_text);
                    current_tokens = counter.count_str(&trim_block_separators(&current_text));
                }
            }

            Block::List(text) => {
                let block_tokens = counter.count_str(&text);

                if current_tokens + block_tokens > max_tokens && has_content(&current_text) {
                    chunks.push(FetchChunk {
                        heading: current_heading.clone(),
                        text: trim_block_separators(&current_text),
                        token_count: current_tokens,
                    });
                    current_text.clear();
                    current_tokens = 0;
                }

                // Handle oversized list (FR-WF-CHK-LIST-02)
                if block_tokens > max_tokens {
                    // Flush any accumulated content first
                    if has_content(&current_text) {
                        chunks.push(FetchChunk {
                            heading: current_heading.clone(),
                            text: trim_block_separators(&current_text),
                            token_count: current_tokens,
                        });
                        current_text.clear();
                        current_tokens = 0;
                    }

                    // Split oversized list at item boundaries
                    let split_chunks =
                        split_oversized_list(&text, max_tokens, &counter, &current_heading);
                    chunks.extend(split_chunks);
                } else {
                    // Append to current chunk
                    append_block(&mut current_text, &text);
                    current_tokens = counter.count_str(&trim_block_separators(&current_text));
                }
            }
        }
    }

    // Flush final chunk
    if has_content(&current_text) {
        let trimmed = trim_block_separators(&current_text);
        chunks.push(FetchChunk {
            heading: current_heading,
            text: trimmed.clone(),
            token_count: counter.count_str(&trimmed),
        });
    }

    chunks
}

fn has_content(text: &str) -> bool {
    text.chars().any(|c| !c.is_whitespace())
}

fn append_block(current: &mut String, block: &str) {
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    current.push_str(block);
}

fn trim_block_separators(text: &str) -> String {
    text.trim_end().to_string()
}

///
/// Split oversized text at sentence/whitespace/char boundaries (FR-WF-15b).
fn split_oversized_text(
    text: &str,
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    let mut chunks = Vec::new();

    // Try sentence boundaries first
    let sentences = split_at_sentences(text);
    if sentences.len() > 1 {
        let mut current = String::new();

        for sentence in sentences {
            let candidate = if current.is_empty() {
                sentence.clone()
            } else {
                format!("{current} {sentence}")
            };

            let candidate_tokens = counter.count_str(&candidate);

            if candidate_tokens > max_tokens && !current.is_empty() {
                chunks.push(FetchChunk {
                    heading: heading.to_string(),
                    text: current.clone(),
                    token_count: counter.count_str(&current),
                });
                current = sentence;
            } else if candidate_tokens > max_tokens {
                // Single sentence exceeds budget - split at whitespace
                let sub_chunks = split_at_whitespace(&sentence, max_tokens, counter, heading);
                chunks.extend(sub_chunks);
                current.clear();
            } else {
                current = candidate;
            }
        }

        if !current.is_empty() {
            chunks.push(FetchChunk {
                heading: heading.to_string(),
                text: current.clone(),
                token_count: counter.count_str(&current),
            });
        }

        return chunks;
    }

    // Fall back to whitespace splitting
    split_at_whitespace(text, max_tokens, counter, heading)
}

fn split_at_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        current.push(chars[i]);

        // Check for sentence-ending punctuation followed by space or EOL
        if chars[i] == '.' || chars[i] == '!' || chars[i] == '?' {
            let next = chars.get(i + 1);
            if next.is_none() || next == Some(&' ') || next == Some(&'\n') {
                sentences.push(current.trim().to_string());
                current.clear();
            }
        }

        i += 1;
    }

    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }

    sentences
}

fn split_at_whitespace(
    text: &str,
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };

        let candidate_tokens = counter.count_str(&candidate);

        if candidate_tokens > max_tokens && !current.is_empty() {
            chunks.push(FetchChunk {
                heading: heading.to_string(),
                text: current.clone(),
                token_count: counter.count_str(&current),
            });
            current = word.to_string();

            // Handle single word exceeding budget
            if counter.count_str(&current) > max_tokens {
                let char_chunks = split_at_chars(&current, max_tokens, counter, heading);
                chunks.extend(char_chunks);
                current.clear();
            }
        } else if candidate_tokens > max_tokens {
            // Single word exceeds budget
            let char_chunks = split_at_chars(word, max_tokens, counter, heading);
            chunks.extend(char_chunks);
        } else {
            current = candidate;
        }
    }

    if !current.is_empty() {
        chunks.push(FetchChunk {
            heading: heading.to_string(),
            text: current.clone(),
            token_count: counter.count_str(&current),
        });
    }

    chunks
}

fn split_at_chars(
    text: &str,
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        let candidate = format!("{current}{ch}");
        let candidate_tokens = counter.count_str(&candidate);

        if candidate_tokens > max_tokens && !current.is_empty() {
            chunks.push(FetchChunk {
                heading: heading.to_string(),
                text: current.clone(),
                token_count: counter.count_str(&current),
            });
            current = ch.to_string();
        } else {
            current = candidate;
        }
    }

    if !current.is_empty() {
        chunks.push(FetchChunk {
            heading: heading.to_string(),
            text: current.clone(),
            token_count: counter.count_str(&current),
        });
    }

    chunks
}

fn split_oversized_code(
    fence: &str,
    language: &str,
    content: &[String],
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    let mut chunks = Vec::new();

    // Content includes opening and closing fence
    // Extract just the code lines (skip first and potentially last fence)
    let code_lines: Vec<&str> = content
        .iter()
        .skip(1) // Skip opening fence
        .filter_map(|line| {
            // Skip closing fence
            if is_fence_close(line, fence) {
                None
            } else {
                Some(line.as_str())
            }
        })
        .collect();

    let opening = if language.is_empty() {
        fence.to_string()
    } else {
        format!("{fence}{language}")
    };

    let mut current_lines: Vec<&str> = Vec::new();

    for line in &code_lines {
        // Build candidate chunk
        let mut candidate = opening.clone();
        for l in &current_lines {
            candidate.push('\n');
            candidate.push_str(l);
        }
        candidate.push('\n');
        candidate.push_str(line);
        candidate.push('\n');
        candidate.push_str(fence);

        let candidate_tokens = counter.count_str(&candidate);

        if candidate_tokens > max_tokens && !current_lines.is_empty() {
            // Emit current chunk
            let mut chunk_text = opening.clone();
            for l in &current_lines {
                chunk_text.push('\n');
                chunk_text.push_str(l);
            }
            chunk_text.push('\n');
            chunk_text.push_str(fence);

            chunks.push(FetchChunk {
                heading: heading.to_string(),
                text: chunk_text.clone(),
                token_count: counter.count_str(&chunk_text),
            });

            current_lines.clear();
        }

        current_lines.push(line);
    }

    // Emit final chunk
    if !current_lines.is_empty() {
        let mut chunk_text = opening;
        for l in &current_lines {
            chunk_text.push('\n');
            chunk_text.push_str(l);
        }
        chunk_text.push('\n');
        chunk_text.push_str(fence);

        chunks.push(FetchChunk {
            heading: heading.to_string(),
            text: chunk_text.clone(),
            token_count: counter.count_str(&chunk_text),
        });
    }

    chunks
}

fn split_oversized_list(
    text: &str,
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    let mut chunks = Vec::new();

    // Parse into individual list items with their continuations
    let items = parse_list_items(text);

    let mut current_text = String::new();

    for item in &items {
        let candidate = if current_text.is_empty() {
            item.clone()
        } else {
            format!("{current_text}\n{item}")
        };

        let candidate_tokens = counter.count_str(&candidate);

        if candidate_tokens > max_tokens && !current_text.is_empty() {
            // Emit current chunk
            chunks.push(FetchChunk {
                heading: heading.to_string(),
                text: current_text.clone(),
                token_count: counter.count_str(&current_text),
            });
            current_text.clear();
        }

        // Check if single item exceeds budget
        if counter.count_str(item) > max_tokens {
            // Flush accumulated content
            if !current_text.is_empty() {
                chunks.push(FetchChunk {
                    heading: heading.to_string(),
                    text: current_text.clone(),
                    token_count: counter.count_str(&current_text),
                });
                current_text.clear();
            }

            // Split the oversized item (FR-WF-CHK-LIST-03)
            let item_chunks = split_oversized_list_item(item, max_tokens, counter, heading);
            chunks.extend(item_chunks);
        } else if current_text.is_empty() {
            current_text = item.clone();
        } else {
            current_text = candidate;
        }
    }

    // Emit final chunk
    if !current_text.is_empty() {
        chunks.push(FetchChunk {
            heading: heading.to_string(),
            text: current_text.clone(),
            token_count: counter.count_str(&current_text),
        });
    }

    chunks
}

fn parse_list_items(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current_item = String::new();

    for line in text.lines() {
        if is_list_item_start(line) {
            if !current_item.is_empty() {
                items.push(current_item);
            }
            current_item = line.to_string();
        } else if !current_item.is_empty() {
            current_item.push('\n');
            current_item.push_str(line);
        }
    }

    if !current_item.is_empty() {
        items.push(current_item);
    }

    items
}

fn split_oversized_list_item(
    item: &str,
    max_tokens: u32,
    counter: &TokenCounter,
    heading: &str,
) -> Vec<FetchChunk> {
    // Extract the marker and determine continuation indent
    let (marker, rest) = extract_list_marker(item);
    let continuation_indent = "  "; // 2 spaces beyond marker position

    // Split the content
    let content_chunks = split_oversized_text(rest, max_tokens, counter, heading);

    // Reformat: first chunk keeps marker, rest get continuation indent
    let mut result = Vec::new();
    for (i, chunk) in content_chunks.into_iter().enumerate() {
        let formatted_text = if i == 0 {
            format!("{marker}{}", chunk.text)
        } else {
            // Format as continuation lines
            chunk
                .text
                .lines()
                .map(|line| format!("{continuation_indent}{line}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        result.push(FetchChunk {
            heading: chunk.heading,
            text: formatted_text.clone(),
            token_count: counter.count_str(&formatted_text),
        });
    }

    result
}

/// Extract list marker from item start.
fn extract_list_marker(item: &str) -> (String, &str) {
    let trimmed = item.trim_start();
    let leading_ws = &item[..item.len() - trimmed.len()];

    // Unordered markers
    for marker in &["- ", "+ ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            let full_marker = format!("{leading_ws}{marker}");
            return (full_marker, rest);
        }
    }

    // Ordered markers: digits followed by . or )
    let mut i = 0;
    let chars: Vec<char> = trimmed.chars().collect();

    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }

    if i > 0 && i < chars.len() && (chars[i] == '.' || chars[i] == ')') {
        let marker_end = i + 1;
        if marker_end < chars.len() && chars[marker_end] == ' ' {
            // Include the space after . or ) in the marker
            let marker_str: String = chars[..=marker_end].iter().collect();
            let full_marker = format!("{leading_ws}{marker_str}");
            let rest_start = leading_ws.len() + marker_str.len();
            return (full_marker, &item[rest_start..]);
        }
    }

    // Fallback: no marker found
    (String::new(), item)
}

#[cfg(test)]
mod tests {
    use super::{
        Block, chunk, extract_list_marker, is_fence_close, is_list_item_start, parse_atx_heading,
        parse_blocks, parse_fence_start, parse_list_items, split_at_sentences,
    };

    #[test]
    fn test_parse_atx_heading() {
        assert_eq!(
            parse_atx_heading("# Hello"),
            Some((1, "Hello".to_string(), "# Hello".to_string()))
        );
        assert_eq!(
            parse_atx_heading("## World"),
            Some((2, "World".to_string(), "## World".to_string()))
        );
        assert_eq!(
            parse_atx_heading("### Test ###"),
            Some((3, "Test".to_string(), "### Test ###".to_string()))
        );
        assert_eq!(parse_atx_heading("Not a heading"), None);
        assert_eq!(parse_atx_heading("#NoSpace"), None);
    }

    #[test]
    fn test_parse_fence_start() {
        assert_eq!(
            parse_fence_start("```rust"),
            Some(("```".to_string(), "rust".to_string()))
        );
        assert_eq!(
            parse_fence_start("~~~"),
            Some(("~~~".to_string(), String::new()))
        );
        assert_eq!(
            parse_fence_start("````python"),
            Some(("````".to_string(), "python".to_string()))
        );
        assert_eq!(parse_fence_start("``not enough"), None);
    }

    #[test]
    fn test_is_fence_close() {
        assert!(is_fence_close("```", "```"));
        assert!(is_fence_close("````", "```"));
        assert!(is_fence_close("~~~", "~~~"));
        assert!(!is_fence_close("``", "```"));
        assert!(!is_fence_close("``` extra", "```"));
    }

    #[test]
    fn test_is_list_item_start() {
        assert!(is_list_item_start("- item"));
        assert!(is_list_item_start("* item"));
        assert!(is_list_item_start("+ item"));
        assert!(is_list_item_start("1. item"));
        assert!(is_list_item_start("99. item"));
        assert!(is_list_item_start("  - nested"));
        assert!(!is_list_item_start("    - too indented"));
        assert!(!is_list_item_start("not a list"));
    }

    #[test]
    fn test_parse_blocks_basic() {
        let md = "# Heading\n\nParagraph text.\n\n- list item";
        let blocks = parse_blocks(md);

        assert!(matches!(blocks[0], Block::Heading { .. }));
        assert!(matches!(blocks[1], Block::BlankLines(_)));
        assert!(matches!(blocks[2], Block::Paragraph(_)));
        assert!(matches!(blocks[3], Block::BlankLines(_)));
        assert!(matches!(blocks[4], Block::List(_)));
    }

    #[test]
    fn test_parse_blocks_code_fence() {
        let md = "```rust\nfn main() {}\n```";
        let blocks = parse_blocks(md);

        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], Block::CodeFence { .. }));
    }

    #[test]
    fn test_chunk_basic() {
        let markdown = "# Heading\n\nParagraph one.\n\nParagraph two.";
        let chunks = chunk(markdown, 1000);

        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].heading, "Heading");
    }

    #[test]
    fn test_chunk_respects_budget() {
        let markdown = "# Test\n\n".to_string() + &"word ".repeat(200);
        let chunks = chunk(&markdown, 50);

        // Should be split into multiple chunks
        assert!(chunks.len() > 1);
        for c in &chunks {
            // Allow some tolerance for edge cases
            assert!(
                c.token_count <= 60,
                "Chunk exceeded budget: {} tokens",
                c.token_count
            );
        }
    }

    #[test]
    fn test_chunk_code_fence_atomic() {
        let markdown = "# Code\n\n```rust\nfn foo() {}\n```\n\nMore text.";
        let chunks = chunk(markdown, 1000);

        // Code fence should stay together
        let code_chunk = chunks.iter().find(|c| c.text.contains("```rust"));
        assert!(code_chunk.is_some());
        let code = code_chunk.unwrap();
        assert!(code.text.contains("fn foo()"));
        assert!(code.text.matches("```").count() >= 2);
    }

    #[test]
    fn test_chunk_heading_tracking() {
        let markdown = "# First\n\nContent under first.\n\n## Second\n\nContent under second.";
        let chunks = chunk(markdown, 1000);

        // First chunk should have "First" heading
        assert!(chunks.iter().any(|c| c.heading == "First"));

        // Chunk with "Content under second" should have "Second" heading
        if let Some(second_chunk) = chunks
            .iter()
            .find(|c| c.text.contains("Content under second"))
        {
            assert_eq!(second_chunk.heading, "Second");
        }
    }

    #[test]
    fn test_split_at_sentences() {
        let text = "First sentence. Second sentence! Third? Yes.";
        let sentences = split_at_sentences(text);

        assert_eq!(sentences.len(), 4);
        assert_eq!(sentences[0], "First sentence.");
        assert_eq!(sentences[1], "Second sentence!");
        assert_eq!(sentences[2], "Third?");
        assert_eq!(sentences[3], "Yes.");
    }

    #[test]
    fn test_parse_list_items() {
        let text = "- item 1\n- item 2\n  continuation\n- item 3";
        let items = parse_list_items(text);

        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "- item 1");
        assert_eq!(items[1], "- item 2\n  continuation");
        assert_eq!(items[2], "- item 3");
    }

    #[test]
    fn test_extract_list_marker() {
        let (marker, rest) = extract_list_marker("- item text");
        assert_eq!(marker, "- ");
        assert_eq!(rest, "item text");

        let (marker, rest) = extract_list_marker("1. numbered item");
        assert_eq!(marker, "1. ");
        assert_eq!(rest, "numbered item");
    }
}

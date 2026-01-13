//! Terminal text sanitization for secure display.
//!
//! This module provides functions to sanitize untrusted text (model output,
//! error messages, recovered content) before rendering in a terminal.
//!
//! # Security Rationale
//!
//! Terminal emulators interpret escape sequences that can:
//! - Manipulate clipboard (OSC 52)
//! - Create deceptive hyperlinks (OSC 8)
//! - Rewrite displayed content (CSI cursor movement)
//! - Alter terminal state/configuration
//!
//! All text from untrusted sources (LLM output, network errors, persisted
//! history) MUST be sanitized before display.

use std::borrow::Cow;

/// ASCII escape character that starts ANSI sequences.
const ESC: char = '\x1b';
/// ASCII bell character that can terminate OSC sequences.
const BEL: char = '\x07';

/// Sanitize text for safe terminal display.
///
/// Strips:
/// - ANSI escape sequences (CSI, OSC, etc.)
/// - C0 control characters except `\n`, `\t`, `\r`
/// - C1 control characters (`\x80`-`\x9F`)
/// - DEL character (`\x7F`)
///
/// Preserves:
/// - All printable ASCII and UTF-8 characters
/// - Newlines, tabs, and carriage returns
///
/// # Performance
///
/// Returns `Cow::Borrowed` when no sanitization is needed (common case for
/// well-behaved model output), avoiding allocation.
///
/// # Examples
///
/// ```
/// use forge_types::sanitize_terminal_text;
///
/// // Clean text passes through without allocation
/// let clean = "Hello, world!";
/// assert_eq!(sanitize_terminal_text(clean), clean);
///
/// // Escape sequences are stripped
/// let dirty = "Hello\x1b[2JWorld";
/// assert_eq!(sanitize_terminal_text(dirty), "HelloWorld");
/// ```
#[must_use] 
pub fn sanitize_terminal_text(input: &str) -> Cow<'_, str> {
    // Fast path: check if any sanitization is needed
    if !needs_sanitization(input) {
        return Cow::Borrowed(input);
    }

    // Slow path: build sanitized string
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == ESC {
            // Skip the escape sequence
            skip_escape_sequence(&mut chars);
        } else if is_allowed_control(c) {
            result.push(c);
        } else if is_c0_control(c) || is_c1_control(c) || c == '\x7f' {
            // Skip disallowed control characters (C0, C1, DEL)
            // C1 controls can also start escape sequences in some terminals
            if is_c1_csi(c) {
                // C1 CSI equivalent - skip following sequence
                skip_csi_params(&mut chars);
            }
        } else {
            result.push(c);
        }
    }

    Cow::Owned(result)
}

/// Check if text contains any characters that need sanitization.
fn needs_sanitization(input: &str) -> bool {
    input.chars().any(|c| {
        c == ESC
            || c == BEL
            || (is_c0_control(c) && !is_allowed_control(c))
            || is_c1_control(c)
            || c == '\x7f'
    })
}

/// Check if character is a C0 control character (0x00-0x1F).
fn is_c0_control(c: char) -> bool {
    c <= '\x1f'
}

/// Check if character is an allowed control character.
fn is_allowed_control(c: char) -> bool {
    matches!(c, '\n' | '\t' | '\r')
}

/// Check if character is a C1 control character (0x80-0x9F).
fn is_c1_control(c: char) -> bool {
    ('\u{0080}'..='\u{009f}').contains(&c)
}

/// Check if C1 character is the CSI equivalent (0x9B).
fn is_c1_csi(c: char) -> bool {
    c == '\u{009b}'
}

/// Skip an escape sequence starting after ESC.
fn skip_escape_sequence<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) {
    let Some(&next) = chars.peek() else {
        return;
    };

    match next {
        // CSI sequence: ESC [ ... <final byte>
        '[' => {
            chars.next(); // consume '['
            skip_csi_params(chars);
        }
        // OSC sequence: ESC ] ... (BEL | ESC \)
        ']' => {
            chars.next(); // consume ']'
            skip_osc_sequence(chars);
        }
        // DCS, PM, APC sequences: ESC P/^/_ ... (ST)
        'P' | '^' | '_' => {
            chars.next();
            skip_until_st(chars);
        }
        // Two-character sequences: ESC <char>
        // Includes: ESC ( for G0, ESC ) for G1, ESC # for line attrs, etc.
        '(' | ')' | '*' | '+' | '#' | ' ' => {
            chars.next(); // consume the character
            chars.next(); // consume the following character
        }
        // Single-character commands: ESC 7, ESC 8, ESC c, etc.
        '7' | '8' | 'c' | 'D' | 'E' | 'H' | 'M' | 'N' | 'O' | 'Z' | '=' | '>' | '<' => {
            chars.next();
        }
        // Unknown sequence - just skip the ESC, next char will be processed normally
        _ => {}
    }
}

/// Skip CSI parameters until final byte (0x40-0x7E).
fn skip_csi_params<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) {
    // CSI format: parameter bytes (0x30-0x3F) + intermediate bytes (0x20-0x2F) + final byte (0x40-0x7E)
    // We skip until we see a final byte or run out of valid sequence chars
    while let Some(&c) = chars.peek() {
        if ('\x40'..='\x7e').contains(&c) {
            // Final byte - consume it and we're done
            chars.next();
            return;
        } else if ('\x20'..='\x3f').contains(&c) {
            // Parameter or intermediate byte - continue
            chars.next();
        } else {
            // Invalid sequence or end - stop
            return;
        }
    }
}

/// Skip OSC sequence until BEL or ST (ESC \).
fn skip_osc_sequence<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) {
    while let Some(c) = chars.next() {
        if c == BEL {
            return;
        }
        if c == ESC {
            // Check for ST (string terminator: ESC \)
            if chars.peek() == Some(&'\\') {
                chars.next();
                return;
            }
        }
    }
}

/// Skip until ST (string terminator: ESC \) for DCS/PM/APC sequences.
fn skip_until_st<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) {
    while let Some(c) = chars.next() {
        if c == ESC && chars.peek() == Some(&'\\') {
            chars.next();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_no_allocation() {
        let input = "Hello, world! This is clean text.";
        match sanitize_terminal_text(input) {
            Cow::Borrowed(s) => assert_eq!(s, input),
            Cow::Owned(_) => panic!("Should not allocate for clean input"),
        }
    }

    #[test]
    fn preserves_newlines_tabs_cr() {
        let input = "Line 1\nLine 2\tTabbed\r\nCRLF";
        assert_eq!(sanitize_terminal_text(input), input);
    }

    #[test]
    fn preserves_unicode() {
        let input = "Hello 👋 World 中文 العربية";
        assert_eq!(sanitize_terminal_text(input), input);
    }

    #[test]
    fn strips_csi_clear_screen() {
        let input = "Before\x1b[2JAfter";
        assert_eq!(sanitize_terminal_text(input), "BeforeAfter");
    }

    #[test]
    fn strips_csi_cursor_movement() {
        let input = "Text\x1b[10;20HMoved";
        assert_eq!(sanitize_terminal_text(input), "TextMoved");
    }

    #[test]
    fn strips_csi_color_codes() {
        let input = "\x1b[31mRed\x1b[0m Normal";
        assert_eq!(sanitize_terminal_text(input), "Red Normal");
    }

    #[test]
    fn strips_osc52_clipboard_bel() {
        // OSC 52 with BEL terminator
        let input = "text\x1b]52;c;SGVsbG8=\x07more";
        assert_eq!(sanitize_terminal_text(input), "textmore");
    }

    #[test]
    fn strips_osc52_clipboard_st() {
        // OSC 52 with ST terminator (ESC \)
        let input = "text\x1b]52;c;SGVsbG8=\x1b\\more";
        assert_eq!(sanitize_terminal_text(input), "textmore");
    }

    #[test]
    fn strips_osc8_hyperlinks() {
        // OSC 8 hyperlink
        let input = "\x1b]8;;http://evil.com\x1b\\Click here\x1b]8;;\x1b\\";
        assert_eq!(sanitize_terminal_text(input), "Click here");
    }

    #[test]
    fn strips_osc_title() {
        // OSC 0 (set title)
        let input = "\x1b]0;Evil Title\x07Normal text";
        assert_eq!(sanitize_terminal_text(input), "Normal text");
    }

    #[test]
    fn strips_c0_controls() {
        // NUL, SOH, STX, etc. (except allowed ones)
        let input = "A\x00B\x01C\x02D\x03E";
        assert_eq!(sanitize_terminal_text(input), "ABCDE");
    }

    #[test]
    fn strips_c1_controls() {
        // C1 control characters
        let input = "Hello\u{0080}World\u{009a}Test\u{009f}";
        assert_eq!(sanitize_terminal_text(input), "HelloWorldTest");
    }

    #[test]
    fn strips_c1_csi_equivalent() {
        // C1 CSI (0x9B) followed by parameters
        let input = "Text\u{009b}31mColored";
        assert_eq!(sanitize_terminal_text(input), "TextColored");
    }

    #[test]
    fn strips_del_character() {
        let input = "Hello\x7fWorld";
        assert_eq!(sanitize_terminal_text(input), "HelloWorld");
    }

    #[test]
    fn strips_dcs_sequence() {
        // DCS (ESC P ... ST)
        let input = "Before\x1bPsome;data\x1b\\After";
        assert_eq!(sanitize_terminal_text(input), "BeforeAfter");
    }

    #[test]
    fn handles_incomplete_escape() {
        // ESC at end of string
        let input = "Text\x1b";
        assert_eq!(sanitize_terminal_text(input), "Text");
    }

    #[test]
    fn handles_incomplete_csi() {
        // CSI without final byte - parameter bytes (0x30-0x3F) are consumed
        let input = "Text\x1b[31";
        assert_eq!(sanitize_terminal_text(input), "Text");
    }

    #[test]
    fn handles_incomplete_osc() {
        // OSC without terminator
        let input = "Text\x1b]52;data";
        assert_eq!(sanitize_terminal_text(input), "Text");
    }

    #[test]
    fn complex_mixed_content() {
        let input = "Hello\x1b[31m World\x1b]52;c;data\x07\nNewline\x00Null\x1b[H";
        assert_eq!(sanitize_terminal_text(input), "Hello World\nNewlineNull");
    }

    #[test]
    fn empty_string() {
        assert_eq!(sanitize_terminal_text(""), "");
    }
}

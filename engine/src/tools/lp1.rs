//! LP1 (Line Patch v1) parsing and application.

use std::fmt;

#[derive(Debug, Clone)]
pub struct Patch {
    pub files: Vec<FilePatch>,
}

#[derive(Debug, Clone)]
pub struct FilePatch {
    pub path: String,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone)]
pub enum Op {
    Replace {
        occ: Option<usize>,
        find: Vec<String>,
        replace: Vec<String>,
    },
    InsertAfter {
        occ: Option<usize>,
        find: Vec<String>,
        insert: Vec<String>,
    },
    InsertBefore {
        occ: Option<usize>,
        find: Vec<String>,
        insert: Vec<String>,
    },
    Erase {
        occ: Option<usize>,
        find: Vec<String>,
    },
    Append {
        block: Vec<String>,
    },
    Prepend {
        block: Vec<String>,
    },
    SetFinalNewline(bool),
}

#[derive(Debug, Clone)]
pub struct PatchError {
    pub message: String,
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for PatchError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EolKind {
    Lf,
    CrLf,
}

#[derive(Debug, Clone)]
pub struct FileContent {
    pub lines: Vec<String>,
    pub final_newline: bool,
    pub eol_kind: Option<EolKind>,
}

pub fn parse_patch(input: &str) -> Result<Patch, PatchError> {
    let lines: Vec<String> = input
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    let mut i = 0;

    // Header
    while i < lines.len() && is_blank_or_comment(&lines[i]) {
        i += 1;
    }
    if i >= lines.len() {
        return Err(err("Missing LP1 header"));
    }
    if lines[i].trim() != "LP1" {
        return Err(err("Invalid LP1 header"));
    }
    i += 1;

    let mut files: Vec<FilePatch> = Vec::new();

    loop {
        while i < lines.len() && is_blank_or_comment(&lines[i]) {
            i += 1;
        }
        if i >= lines.len() {
            return Err(err("Missing END footer"));
        }
        let trimmed = lines[i].trim();
        if trimmed == "END" {
            break;
        }
        if !trimmed.starts_with('F') {
            return Err(err("Expected file section (F <path>)"));
        }
        let path = parse_file_line(&lines[i])?;
        i += 1;
        let mut ops = Vec::new();

        while i < lines.len() {
            if is_blank_or_comment(&lines[i]) {
                i += 1;
                continue;
            }
            let peek = lines[i].trim();
            if peek == "END" || peek.starts_with('F') {
                break;
            }
            let (op, next_index) = parse_op(&lines, i)?;
            ops.push(op);
            i = next_index;
        }

        files.push(FilePatch { path, ops });
    }

    Ok(Patch { files })
}

fn parse_file_line(line: &str) -> Result<String, PatchError> {
    let mut s = line.trim_start();
    if !s.starts_with('F') {
        return Err(err("Expected file section"));
    }
    s = s[1..].trim_start();
    if s.is_empty() {
        return Err(err("Missing file path"));
    }

    if s.starts_with('"') {
        parse_quoted_path(s)
    } else {
        let path = s.split_whitespace().next().unwrap_or("");
        if path.is_empty() {
            return Err(err("Missing file path"));
        }
        Ok(path.to_string())
    }
}

fn parse_quoted_path(s: &str) -> Result<String, PatchError> {
    let mut chars = s.chars();
    if chars.next() != Some('"') {
        return Err(err("Invalid quoted path"));
    }
    let mut out = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            match ch {
                '"' | '\\' => out.push(ch),
                _ => return Err(err("Invalid escape in quoted path")),
            }
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Ok(out);
        }
        out.push(ch);
    }
    Err(err("Unterminated quoted path"))
}

fn parse_op(lines: &[String], start: usize) -> Result<(Op, usize), PatchError> {
    let line = lines[start].trim_start();
    if line.is_empty() {
        return Err(err("Unexpected blank line in op"));
    }
    let mut chars = line.chars();
    let cmd = chars.next().unwrap();
    let rest = chars.as_str().trim();
    match cmd {
        'R' => {
            let occ = parse_occ(rest)?;
            let (find, idx) = parse_block(lines, start + 1)?;
            let (replace, idx) = parse_block(lines, idx)?;
            Ok((Op::Replace { occ, find, replace }, idx))
        }
        'I' => {
            let occ = parse_occ(rest)?;
            let (find, idx) = parse_block(lines, start + 1)?;
            let (insert, idx) = parse_block(lines, idx)?;
            Ok((Op::InsertAfter { occ, find, insert }, idx))
        }
        'P' => {
            let occ = parse_occ(rest)?;
            let (find, idx) = parse_block(lines, start + 1)?;
            let (insert, idx) = parse_block(lines, idx)?;
            Ok((Op::InsertBefore { occ, find, insert }, idx))
        }
        'E' => {
            let occ = parse_occ(rest)?;
            let (find, idx) = parse_block(lines, start + 1)?;
            Ok((Op::Erase { occ, find }, idx))
        }
        'T' => {
            let (block, idx) = parse_block(lines, start + 1)?;
            Ok((Op::Append { block }, idx))
        }
        'B' => {
            let (block, idx) = parse_block(lines, start + 1)?;
            Ok((Op::Prepend { block }, idx))
        }
        'N' => {
            let flag = rest;
            let value = match flag {
                "+" => true,
                "-" => false,
                _ => return Err(err("Invalid N flag; expected + or -")),
            };
            Ok((Op::SetFinalNewline(value), start + 1))
        }
        _ => Err(err("Unknown LP1 operation")),
    }
}

fn parse_occ(rest: &str) -> Result<Option<usize>, PatchError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(None);
    }
    let value: usize = rest.parse().map_err(|_| err("Invalid occurrence"))?;
    if value == 0 {
        return Err(err("Occurrence must be >= 1"));
    }
    Ok(Some(value))
}

fn parse_block(lines: &[String], mut i: usize) -> Result<(Vec<String>, usize), PatchError> {
    let mut block: Vec<String> = Vec::new();
    while i < lines.len() {
        let raw = &lines[i];
        if is_terminator(raw) {
            return Ok((block, i + 1));
        }
        if let Some(stripped) = raw.strip_prefix('.') {
            if stripped.starts_with('.') {
                block.push(stripped.to_string());
            } else {
                return Err(err("Dot-stuffed line must start with '..'"));
            }
        } else {
            block.push(raw.clone());
        }
        i += 1;
    }
    Err(err("Unterminated LP1 block"))
}

fn is_terminator(line: &str) -> bool {
    if !line.starts_with('.') {
        return false;
    }
    line[1..].trim_matches([' ', '\t']).is_empty()
}

fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn err(message: &str) -> PatchError {
    PatchError {
        message: message.to_string(),
    }
}

// ============================
// File content representation
// ============================

pub fn parse_file(bytes: &[u8]) -> Result<FileContent, PatchError> {
    let mut lines: Vec<String> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut lf_count: usize = 0;
    let mut crlf_count: usize = 0;
    let mut final_newline = false;

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 >= bytes.len() || bytes[i + 1] != b'\n' {
                    return Err(err("Bare CR in file"));
                }
                push_line(&mut lines, &mut buf)?;
                final_newline = true;
                crlf_count += 1;
                i += 2;
            }
            b'\n' => {
                push_line(&mut lines, &mut buf)?;
                final_newline = true;
                lf_count += 1;
                i += 1;
            }
            b => {
                buf.push(b);
                final_newline = false;
                i += 1;
            }
        }
    }

    if !buf.is_empty() {
        push_line(&mut lines, &mut buf)?;
    }

    if lines.is_empty() {
        final_newline = false;
    }

    // Determine EOL kind via majority vote; LF wins ties (more portable).
    // This normalizes mixed EOL files at the boundary per IFA §10.1.
    let eol_kind = match (lf_count, crlf_count) {
        (0, 0) => None,
        (lf, crlf) if crlf > lf => Some(EolKind::CrLf),
        _ => Some(EolKind::Lf),
    };

    Ok(FileContent {
        lines,
        final_newline,
        eol_kind,
    })
}

pub fn emit_file(content: &FileContent) -> Vec<u8> {
    let mut out = Vec::new();
    let eol = match content.eol_kind.unwrap_or(EolKind::Lf) {
        EolKind::Lf => "\n",
        EolKind::CrLf => "\r\n",
    };

    for (idx, line) in content.lines.iter().enumerate() {
        if idx > 0 {
            out.extend_from_slice(eol.as_bytes());
        }
        out.extend_from_slice(line.as_bytes());
    }
    if content.final_newline && !content.lines.is_empty() {
        out.extend_from_slice(eol.as_bytes());
    }
    out
}

pub fn apply_ops(content: &mut FileContent, ops: &[Op]) -> Result<(), PatchError> {
    for op in ops {
        match op {
            Op::Replace { occ, find, replace } => {
                let idx = find_match(&content.lines, find, *occ)?;
                replace_range(&mut content.lines, idx, find.len(), replace.clone());
            }
            Op::InsertAfter { occ, find, insert } => {
                let idx = find_match(&content.lines, find, *occ)?;
                let insert_at = idx + find.len();
                insert_block(&mut content.lines, insert_at, insert.clone());
            }
            Op::InsertBefore { occ, find, insert } => {
                let idx = find_match(&content.lines, find, *occ)?;
                insert_block(&mut content.lines, idx, insert.clone());
            }
            Op::Erase { occ, find } => {
                let idx = find_match(&content.lines, find, *occ)?;
                replace_range(&mut content.lines, idx, find.len(), Vec::new());
            }
            Op::Append { block } => {
                let len = content.lines.len();
                insert_block(&mut content.lines, len, block.clone());
            }
            Op::Prepend { block } => {
                insert_block(&mut content.lines, 0, block.clone());
            }
            Op::SetFinalNewline(value) => {
                apply_final_newline(content, *value);
            }
        }
    }

    if content.lines.is_empty() {
        content.final_newline = false;
    }

    Ok(())
}

fn push_line(lines: &mut Vec<String>, buf: &mut Vec<u8>) -> Result<(), PatchError> {
    let line = String::from_utf8(buf.clone()).map_err(|_| err("Invalid UTF-8 in file"))?;
    lines.push(line);
    buf.clear();
    Ok(())
}

fn find_match(lines: &[String], block: &[String], occ: Option<usize>) -> Result<usize, PatchError> {
    let mut matches: Vec<usize> = Vec::new();
    if block.is_empty() {
        // Empty match only allowed if occ is specified.
        if let Some(index) = occ {
            let idx = index.saturating_sub(1);
            if idx > lines.len() {
                return Err(err("Occurrence out of range"));
            }
            return Ok(idx);
        }
        return Err(err("Empty match requires occurrence"));
    }

    for i in 0..=lines.len().saturating_sub(block.len()) {
        if lines[i..i + block.len()] == *block {
            matches.push(i);
        }
    }

    if matches.is_empty() {
        return Err(err("Match not found"));
    }

    if let Some(occ) = occ {
        let index = occ - 1;
        return matches
            .get(index)
            .copied()
            .ok_or_else(|| err("Occurrence out of range"));
    }

    if matches.len() != 1 {
        return Err(err("Match is not unique"));
    }

    Ok(matches[0])
}

fn replace_range(lines: &mut Vec<String>, start: usize, len: usize, replace: Vec<String>) {
    lines.splice(start..start + len, replace);
}

fn insert_block(lines: &mut Vec<String>, index: usize, block: Vec<String>) {
    lines.splice(index..index, block);
}

fn apply_final_newline(content: &mut FileContent, value: bool) {
    if value {
        if content.lines.is_empty() {
            content.lines.push(String::new());
            content.final_newline = true;
            if content.eol_kind.is_none() {
                content.eol_kind = Some(EolKind::Lf);
            }
        } else {
            content.final_newline = true;
        }
    } else if content.lines == [String::new()] && content.final_newline {
        content.lines.clear();
        content.final_newline = false;
    } else {
        content.final_newline = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // parse_patch tests
    // ========================================================================

    #[test]
    fn parse_minimal_patch() {
        let input = "LP1\nF test.txt\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 1);
        assert_eq!(patch.files[0].path, "test.txt");
        assert!(patch.files[0].ops.is_empty());
    }

    #[test]
    fn parse_patch_with_comments() {
        let input = "# Comment line\nLP1\n# Another comment\nF file.txt\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files[0].path, "file.txt");
    }

    #[test]
    fn parse_patch_with_blank_lines() {
        let input = "\n\nLP1\n\nF test.txt\n\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 1);
    }

    #[test]
    fn parse_missing_header() {
        let input = "F test.txt\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("header"));
    }

    #[test]
    fn parse_missing_end() {
        let input = "LP1\nF test.txt\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("END"));
    }

    #[test]
    fn parse_invalid_header() {
        let input = "LP2\nF test.txt\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Invalid LP1 header"));
    }

    #[test]
    fn parse_multiple_files() {
        let input = "LP1\nF first.txt\nF second.txt\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files.len(), 2);
        assert_eq!(patch.files[0].path, "first.txt");
        assert_eq!(patch.files[1].path, "second.txt");
    }

    #[test]
    fn parse_quoted_path() {
        let input = "LP1\nF \"path with spaces.txt\"\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files[0].path, "path with spaces.txt");
    }

    #[test]
    fn parse_quoted_path_with_escapes() {
        let input = "LP1\nF \"path\\\"with\\\\quotes.txt\"\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files[0].path, "path\"with\\quotes.txt");
    }

    #[test]
    fn parse_unterminated_quoted_path() {
        let input = "LP1\nF \"unterminated\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Unterminated"));
    }

    #[test]
    fn parse_missing_file_path() {
        let input = "LP1\nF \nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Missing file path"));
    }

    // ========================================================================
    // Operation parsing tests
    // ========================================================================

    #[test]
    fn parse_replace_op() {
        let input = "LP1\nF test.txt\nR\nold line\n.\nnew line\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files[0].ops.len(), 1);
        match &patch.files[0].ops[0] {
            Op::Replace { occ, find, replace } => {
                assert!(occ.is_none());
                assert_eq!(find, &vec!["old line".to_string()]);
                assert_eq!(replace, &vec!["new line".to_string()]);
            }
            _ => panic!("Expected Replace op"),
        }
    }

    #[test]
    fn parse_replace_with_occurrence() {
        let input = "LP1\nF test.txt\nR 2\nfind\n.\nreplace\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Replace { occ, .. } => {
                assert_eq!(*occ, Some(2));
            }
            _ => panic!("Expected Replace op"),
        }
    }

    #[test]
    fn parse_insert_after_op() {
        let input = "LP1\nF test.txt\nI\nanchor\n.\ninserted\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::InsertAfter { find, insert, .. } => {
                assert_eq!(find, &vec!["anchor".to_string()]);
                assert_eq!(insert, &vec!["inserted".to_string()]);
            }
            _ => panic!("Expected InsertAfter op"),
        }
    }

    #[test]
    fn parse_insert_before_op() {
        let input = "LP1\nF test.txt\nP\nanchor\n.\ninserted\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::InsertBefore { find, insert, .. } => {
                assert_eq!(find, &vec!["anchor".to_string()]);
                assert_eq!(insert, &vec!["inserted".to_string()]);
            }
            _ => panic!("Expected InsertBefore op"),
        }
    }

    #[test]
    fn parse_erase_op() {
        let input = "LP1\nF test.txt\nE\nto delete\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Erase { find, .. } => {
                assert_eq!(find, &vec!["to delete".to_string()]);
            }
            _ => panic!("Expected Erase op"),
        }
    }

    #[test]
    fn parse_append_op() {
        let input = "LP1\nF test.txt\nT\nappended line\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Append { block } => {
                assert_eq!(block, &vec!["appended line".to_string()]);
            }
            _ => panic!("Expected Append op"),
        }
    }

    #[test]
    fn parse_prepend_op() {
        let input = "LP1\nF test.txt\nB\nprepended line\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Prepend { block } => {
                assert_eq!(block, &vec!["prepended line".to_string()]);
            }
            _ => panic!("Expected Prepend op"),
        }
    }

    #[test]
    fn parse_set_final_newline_on() {
        let input = "LP1\nF test.txt\nN +\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::SetFinalNewline(value) => {
                assert!(*value);
            }
            _ => panic!("Expected SetFinalNewline op"),
        }
    }

    #[test]
    fn parse_set_final_newline_off() {
        let input = "LP1\nF test.txt\nN -\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::SetFinalNewline(value) => {
                assert!(!*value);
            }
            _ => panic!("Expected SetFinalNewline op"),
        }
    }

    #[test]
    fn parse_invalid_final_newline_flag() {
        let input = "LP1\nF test.txt\nN x\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Invalid N flag"));
    }

    #[test]
    fn parse_multiline_block() {
        let input = "LP1\nF test.txt\nT\nline 1\nline 2\nline 3\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Append { block } => {
                assert_eq!(block.len(), 3);
                assert_eq!(block[0], "line 1");
                assert_eq!(block[1], "line 2");
                assert_eq!(block[2], "line 3");
            }
            _ => panic!("Expected Append op"),
        }
    }

    #[test]
    fn parse_dot_stuffed_lines() {
        let input = "LP1\nF test.txt\nT\n..literal dot\n...\n.\nEND\n";
        let patch = parse_patch(input).unwrap();
        match &patch.files[0].ops[0] {
            Op::Append { block } => {
                assert_eq!(block[0], ".literal dot");
                assert_eq!(block[1], "..");
            }
            _ => panic!("Expected Append op"),
        }
    }

    #[test]
    fn parse_invalid_dot_stuffing() {
        let input = "LP1\nF test.txt\nT\n.invalid\n.\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Dot-stuffed"));
    }

    #[test]
    fn parse_unknown_operation() {
        let input = "LP1\nF test.txt\nX\n.\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Unknown LP1 operation"));
    }

    #[test]
    fn parse_invalid_occurrence() {
        let input = "LP1\nF test.txt\nR abc\nfind\n.\nreplace\n.\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Invalid occurrence"));
    }

    #[test]
    fn parse_zero_occurrence_rejected() {
        let input = "LP1\nF test.txt\nR 0\nfind\n.\nreplace\n.\nEND\n";
        let err = parse_patch(input).unwrap_err();
        assert!(err.message.contains("Occurrence must be >= 1"));
    }

    #[test]
    fn parse_crlf_input() {
        let input = "LP1\r\nF test.txt\r\nEND\r\n";
        let patch = parse_patch(input).unwrap();
        assert_eq!(patch.files[0].path, "test.txt");
    }

    // ========================================================================
    // parse_file tests
    // ========================================================================

    #[test]
    fn parse_file_empty() {
        let content = parse_file(b"").unwrap();
        assert!(content.lines.is_empty());
        assert!(!content.final_newline);
        assert!(content.eol_kind.is_none());
    }

    #[test]
    fn parse_file_single_line_no_newline() {
        let content = parse_file(b"hello").unwrap();
        assert_eq!(content.lines, vec!["hello"]);
        assert!(!content.final_newline);
    }

    #[test]
    fn parse_file_single_line_with_newline() {
        let content = parse_file(b"hello\n").unwrap();
        assert_eq!(content.lines, vec!["hello"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::Lf));
    }

    #[test]
    fn parse_file_multiple_lines_lf() {
        let content = parse_file(b"line1\nline2\nline3\n").unwrap();
        assert_eq!(content.lines, vec!["line1", "line2", "line3"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::Lf));
    }

    #[test]
    fn parse_file_multiple_lines_crlf() {
        let content = parse_file(b"line1\r\nline2\r\n").unwrap();
        assert_eq!(content.lines, vec!["line1", "line2"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::CrLf));
    }

    #[test]
    fn parse_file_mixed_eol_normalized_to_majority() {
        // 1 LF, 1 CRLF -> tie -> LF wins
        let content = parse_file(b"line1\nline2\r\n").unwrap();
        assert_eq!(content.lines, vec!["line1", "line2"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::Lf));
    }

    #[test]
    fn parse_file_mixed_eol_crlf_majority() {
        // 1 LF, 2 CRLF -> CRLF wins
        let content = parse_file(b"line1\r\nline2\nline3\r\n").unwrap();
        assert_eq!(content.lines, vec!["line1", "line2", "line3"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::CrLf));
    }

    #[test]
    fn parse_file_mixed_eol_lf_majority() {
        // 2 LF, 1 CRLF -> LF wins
        let content = parse_file(b"line1\nline2\r\nline3\n").unwrap();
        assert_eq!(content.lines, vec!["line1", "line2", "line3"]);
        assert!(content.final_newline);
        assert_eq!(content.eol_kind, Some(EolKind::Lf));
    }

    #[test]
    fn parse_file_bare_cr_rejected() {
        let result = parse_file(b"hello\rworld");
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Bare CR"));
    }

    #[test]
    fn parse_file_invalid_utf8_rejected() {
        let result = parse_file(b"\xff\xfe invalid utf8");
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Invalid UTF-8"));
    }

    // ========================================================================
    // emit_file tests
    // ========================================================================

    #[test]
    fn emit_file_empty() {
        let content = FileContent {
            lines: vec![],
            final_newline: false,
            eol_kind: None,
        };
        let bytes = emit_file(&content);
        assert!(bytes.is_empty());
    }

    #[test]
    fn emit_file_single_line_no_newline() {
        let content = FileContent {
            lines: vec!["hello".to_string()],
            final_newline: false,
            eol_kind: Some(EolKind::Lf),
        };
        let bytes = emit_file(&content);
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn emit_file_single_line_with_newline() {
        let content = FileContent {
            lines: vec!["hello".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let bytes = emit_file(&content);
        assert_eq!(bytes, b"hello\n");
    }

    #[test]
    fn emit_file_multiple_lines_lf() {
        let content = FileContent {
            lines: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let bytes = emit_file(&content);
        assert_eq!(bytes, b"a\nb\nc\n");
    }

    #[test]
    fn emit_file_multiple_lines_crlf() {
        let content = FileContent {
            lines: vec!["a".to_string(), "b".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::CrLf),
        };
        let bytes = emit_file(&content);
        assert_eq!(bytes, b"a\r\nb\r\n");
    }

    #[test]
    fn emit_file_defaults_to_lf() {
        let content = FileContent {
            lines: vec!["test".to_string()],
            final_newline: true,
            eol_kind: None,
        };
        let bytes = emit_file(&content);
        assert_eq!(bytes, b"test\n");
    }

    // ========================================================================
    // apply_ops tests
    // ========================================================================

    #[test]
    fn apply_replace_single_line() {
        let mut content = FileContent {
            lines: vec!["old".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: None,
            find: vec!["old".to_string()],
            replace: vec!["new".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["new"]);
    }

    #[test]
    fn apply_replace_multiline() {
        let mut content = FileContent {
            lines: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
            ],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: None,
            find: vec!["b".to_string(), "c".to_string()],
            replace: vec!["X".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["a", "X", "d"]);
    }

    #[test]
    fn apply_replace_with_occurrence() {
        let mut content = FileContent {
            lines: vec!["dup".to_string(), "middle".to_string(), "dup".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: Some(2),
            find: vec!["dup".to_string()],
            replace: vec!["replaced".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["dup", "middle", "replaced"]);
    }

    #[test]
    fn apply_insert_after() {
        let mut content = FileContent {
            lines: vec!["anchor".to_string(), "after".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::InsertAfter {
            occ: None,
            find: vec!["anchor".to_string()],
            insert: vec!["inserted".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["anchor", "inserted", "after"]);
    }

    #[test]
    fn apply_insert_before() {
        let mut content = FileContent {
            lines: vec!["before".to_string(), "anchor".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::InsertBefore {
            occ: None,
            find: vec!["anchor".to_string()],
            insert: vec!["inserted".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["before", "inserted", "anchor"]);
    }

    #[test]
    fn apply_erase() {
        let mut content = FileContent {
            lines: vec!["keep".to_string(), "delete".to_string(), "keep".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Erase {
            occ: None,
            find: vec!["delete".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["keep", "keep"]);
    }

    #[test]
    fn apply_append() {
        let mut content = FileContent {
            lines: vec!["existing".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Append {
            block: vec!["appended".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["existing", "appended"]);
    }

    #[test]
    fn apply_prepend() {
        let mut content = FileContent {
            lines: vec!["existing".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Prepend {
            block: vec!["prepended".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert_eq!(content.lines, vec!["prepended", "existing"]);
    }

    #[test]
    fn apply_set_final_newline_true() {
        let mut content = FileContent {
            lines: vec!["line".to_string()],
            final_newline: false,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::SetFinalNewline(true)];
        apply_ops(&mut content, &ops).unwrap();
        assert!(content.final_newline);
    }

    #[test]
    fn apply_set_final_newline_false() {
        let mut content = FileContent {
            lines: vec!["line".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::SetFinalNewline(false)];
        apply_ops(&mut content, &ops).unwrap();
        assert!(!content.final_newline);
    }

    #[test]
    fn apply_ops_match_not_found() {
        let mut content = FileContent {
            lines: vec!["line".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: None,
            find: vec!["nonexistent".to_string()],
            replace: vec!["new".to_string()],
        }];
        let err = apply_ops(&mut content, &ops).unwrap_err();
        assert!(err.message.contains("Match not found"));
    }

    #[test]
    fn apply_ops_match_not_unique() {
        let mut content = FileContent {
            lines: vec!["dup".to_string(), "dup".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: None,
            find: vec!["dup".to_string()],
            replace: vec!["new".to_string()],
        }];
        let err = apply_ops(&mut content, &ops).unwrap_err();
        assert!(err.message.contains("not unique"));
    }

    #[test]
    fn apply_ops_occurrence_out_of_range() {
        let mut content = FileContent {
            lines: vec!["line".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Replace {
            occ: Some(5),
            find: vec!["line".to_string()],
            replace: vec!["new".to_string()],
        }];
        let err = apply_ops(&mut content, &ops).unwrap_err();
        assert!(err.message.contains("Occurrence out of range"));
    }

    #[test]
    fn apply_ops_clears_final_newline_on_empty() {
        let mut content = FileContent {
            lines: vec!["only".to_string()],
            final_newline: true,
            eol_kind: Some(EolKind::Lf),
        };
        let ops = vec![Op::Erase {
            occ: None,
            find: vec!["only".to_string()],
        }];
        apply_ops(&mut content, &ops).unwrap();
        assert!(content.lines.is_empty());
        assert!(!content.final_newline);
    }

    // ========================================================================
    // Roundtrip tests
    // ========================================================================

    #[test]
    fn roundtrip_lf() {
        let original = b"line1\nline2\nline3\n";
        let content = parse_file(original).unwrap();
        let emitted = emit_file(&content);
        assert_eq!(emitted, original);
    }

    #[test]
    fn roundtrip_crlf() {
        let original = b"line1\r\nline2\r\n";
        let content = parse_file(original).unwrap();
        let emitted = emit_file(&content);
        assert_eq!(emitted, original);
    }

    #[test]
    fn roundtrip_no_final_newline() {
        let original = b"line1\nline2";
        let content = parse_file(original).unwrap();
        let emitted = emit_file(&content);
        assert_eq!(emitted, original);
    }

    #[test]
    fn roundtrip_mixed_eol_normalized() {
        // Mixed EOL input: 2 CRLF, 1 LF -> normalizes to CRLF
        let mixed = b"line1\r\nline2\nline3\r\n";
        let content = parse_file(mixed).unwrap();
        assert_eq!(content.eol_kind, Some(EolKind::CrLf));
        let emitted = emit_file(&content);
        // Output is normalized to consistent CRLF
        assert_eq!(emitted, b"line1\r\nline2\r\nline3\r\n");
    }

    // ========================================================================
    // PatchError tests
    // ========================================================================

    #[test]
    fn patch_error_display() {
        let err = PatchError {
            message: "Test error".to_string(),
        };
        assert_eq!(format!("{err}"), "Test error");
    }

    #[test]
    fn patch_error_is_error() {
        let err: Box<dyn std::error::Error> = Box::new(PatchError {
            message: "error".to_string(),
        });
        assert_eq!(err.to_string(), "error");
    }
}

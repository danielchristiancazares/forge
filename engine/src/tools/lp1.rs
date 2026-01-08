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
    let cmd = line.chars().next().unwrap();
    let rest = line[1..].trim();
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
        if raw.starts_with('.') {
            if raw.starts_with("..") {
                block.push(raw[1..].to_string());
            } else {
                return Err(err("Dot-stuffed line must start with '..'"));
            }
        } else {
            block.push(raw.to_string());
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
    let mut eol_kind: Option<EolKind> = None;
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
                set_eol(&mut eol_kind, EolKind::CrLf)?;
                i += 2;
                continue;
            }
            b'\n' => {
                push_line(&mut lines, &mut buf)?;
                final_newline = true;
                set_eol(&mut eol_kind, EolKind::Lf)?;
                i += 1;
                continue;
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
    let line = String::from_utf8(buf.clone())
        .map_err(|_| err("Invalid UTF-8 in file"))?;
    lines.push(line);
    buf.clear();
    Ok(())
}

fn set_eol(target: &mut Option<EolKind>, new: EolKind) -> Result<(), PatchError> {
    if let Some(existing) = target {
        if *existing != new {
            return Err(err("Mixed EOL detected"));
        }
    } else {
        *target = Some(new);
    }
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

//! Unified diff formatting and stats.

use std::fmt::Write as _;
use std::str;

use similar::{ChangeTag, TextDiff};

/// Format a unified diff between old and new file content.
///
/// Produces output with:
/// - 1 line of context around each change
/// - `...` between changes separated by >3 unchanged lines
/// - Red (`-`) for deletions, green (`+`) for additions
#[must_use]
pub fn format_unified_diff(
    path: &str,
    old_bytes: &[u8],
    new_bytes: &[u8],
    existed: bool,
) -> String {
    format_unified_diff_width(path, old_bytes, new_bytes, existed, 0)
}

/// Like `format_unified_diff`, but accepts a minimum line-number column width.
///
/// When multiple files are displayed together, callers should pre-compute the
/// max line count across all files and pass the resulting digit width so that
/// every section aligns consistently.  Pass `0` to auto-detect from the file.
#[must_use]
pub fn format_unified_diff_width(
    _path: &str,
    old_bytes: &[u8],
    new_bytes: &[u8],
    _existed: bool,
    min_line_num_width: usize,
) -> String {
    let old_text = str::from_utf8(old_bytes).unwrap_or("");
    let new_text = str::from_utf8(new_bytes).unwrap_or("");

    let diff = TextDiff::from_lines(old_text, new_text);

    let mut out = String::new();

    let changes: Vec<_> = diff.iter_all_changes().collect();
    if changes.is_empty() {
        return String::new();
    }

    let max_line = old_text.lines().count().max(new_text.lines().count());
    let auto_width = if max_line == 0 {
        1
    } else {
        ((max_line as f64).log10().floor() as usize) + 1
    };
    let line_num_width = auto_width.max(min_line_num_width);

    let gap_marker = format!("{:>line_num_width$}\n", "...");

    let mut i = 0;
    let mut last_output_idx: Option<usize> = None;

    while i < changes.len() {
        let change = &changes[i];

        match change.tag() {
            ChangeTag::Equal => {
                let near_prev_change = i > 0 && changes[i - 1].tag() != ChangeTag::Equal;
                let near_next_change = changes
                    .get(i + 1)
                    .is_some_and(|c| c.tag() != ChangeTag::Equal);

                if near_prev_change || near_next_change {
                    if let Some(last_idx) = last_output_idx {
                        let gap = i - last_idx - 1;
                        if gap > 3 {
                            out.push_str(&gap_marker);
                        }
                    }
                    let line_no = change
                        .old_index()
                        .expect("Equal change always has old_index")
                        + 1;
                    write!(out, "{line_no:>line_num_width$}  ").unwrap();
                    out.push_str(change.value().trim_end_matches('\n'));
                    out.push('\n');
                    last_output_idx = Some(i);
                }
            }
            ChangeTag::Delete => {
                if let Some(last_idx) = last_output_idx {
                    let gap = i - last_idx - 1;
                    if gap > 3 {
                        out.push_str(&gap_marker);
                    }
                }
                let line_no = change
                    .old_index()
                    .expect("Delete change always has old_index")
                    + 1;
                write!(out, "{line_no:>line_num_width$} -").unwrap();
                out.push_str(change.value().trim_end_matches('\n'));
                out.push('\n');
                last_output_idx = Some(i);
            }
            ChangeTag::Insert => {
                if let Some(last_idx) = last_output_idx {
                    let gap = i - last_idx - 1;
                    if gap > 3 {
                        out.push_str(&gap_marker);
                    }
                }
                let line_no = change
                    .new_index()
                    .expect("Insert change always has new_index")
                    + 1;
                write!(out, "{line_no:>line_num_width$} +").unwrap();
                out.push_str(change.value().trim_end_matches('\n'));
                out.push('\n');
                last_output_idx = Some(i);
            }
        }

        i += 1;
    }

    out
}

/// Compute diff stats (additions and deletions) between old and new content.
#[must_use]
pub fn compute_diff_stats(old_bytes: &[u8], new_bytes: &[u8]) -> (u32, u32) {
    let old_text = str::from_utf8(old_bytes).unwrap_or("");
    let new_text = str::from_utf8(new_bytes).unwrap_or("");

    let diff = TextDiff::from_lines(old_text, new_text);

    let mut additions: u32 = 0;
    let mut deletions: u32 = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }

    (additions, deletions)
}

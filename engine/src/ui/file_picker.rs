//! File picker state and fuzzy filtering for the "@" reference feature.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Maximum number of files to scan (prevents slowdown on huge repos).
const MAX_FILES_SCAN: usize = 10_000;

/// Maximum number of results to display in the picker.
const MAX_DISPLAY_RESULTS: usize = 20;

/// Scanned file entry with display path and full path.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Display path (relative to root, using forward slashes).
    pub display: String,
    /// Full absolute path.
    pub path: PathBuf,
}

/// File picker state for the "@" reference feature.
#[derive(Debug, Clone, Default)]
pub struct FilePickerState {
    /// All scanned files from the project.
    all_files: Vec<FileEntry>,
    /// Filtered results based on current filter text.
    filtered: Vec<usize>,
    /// Whether files have been scanned.
    scanned: bool,
}

impl FilePickerState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan files from the given root directory, respecting .gitignore.
    pub fn scan_files(&mut self, root: &Path) {
        self.all_files.clear();
        self.filtered.clear();
        self.scanned = true;

        let walker = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                // Skip .git directory and common build/cache directories
                !matches!(
                    name.as_ref(),
                    ".git" | "node_modules" | "target" | "__pycache__" | ".venv" | "venv"
                )
            })
            .build();

        let root_str = root.to_string_lossy();

        for entry in walker.flatten() {
            if self.all_files.len() >= MAX_FILES_SCAN {
                break;
            }

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path().to_path_buf();
            let display = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");

            if display.is_empty() || display == root_str {
                continue;
            }

            self.all_files.push(FileEntry { display, path });
        }

        self.all_files.sort_by(|a, b| a.display.cmp(&b.display));

        self.filtered = (0..self.all_files.len().min(MAX_DISPLAY_RESULTS)).collect();
    }

    /// Update filtered results based on filter text.
    pub fn update_filter(&mut self, filter: &str) {
        self.filtered.clear();

        if filter.is_empty() {
            self.filtered = (0..self.all_files.len().min(MAX_DISPLAY_RESULTS)).collect();
            return;
        }

        let filter_lower = filter.to_lowercase();
        let filter_chars: Vec<char> = filter_lower.chars().collect();

        let mut scored: Vec<(usize, i32)> = self
            .all_files
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                let score = fuzzy_match_score(&entry.display, &filter_chars);
                if score > 0 { Some((idx, score)) } else { None }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                self.all_files[a.0]
                    .display
                    .len()
                    .cmp(&self.all_files[b.0].display.len())
            })
        });

        self.filtered = scored
            .into_iter()
            .take(MAX_DISPLAY_RESULTS)
            .map(|(idx, _)| idx)
            .collect();
    }

    /// Get filtered file entries for display.
    #[must_use]
    pub fn filtered_files(&self) -> Vec<&FileEntry> {
        self.filtered
            .iter()
            .filter_map(|&idx| self.all_files.get(idx))
            .collect()
    }

    #[must_use]
    pub fn get_selected(&self, selected: usize) -> Option<&FileEntry> {
        self.filtered
            .get(selected)
            .and_then(|&idx| self.all_files.get(idx))
    }

    #[must_use]
    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    /// Check if files have been scanned.
    #[must_use]
    pub fn is_scanned(&self) -> bool {
        self.scanned
    }

    /// Get total file count (before filtering).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.all_files.len()
    }
}

/// Compute a fuzzy match score for a path against filter characters.
/// Returns 0 if no match, higher scores for better matches.
fn fuzzy_match_score(path: &str, filter_chars: &[char]) -> i32 {
    if filter_chars.is_empty() {
        return 1;
    }

    let path_lower = path.to_lowercase();
    let path_chars: Vec<char> = path_lower.chars().collect();

    let mut filter_idx = 0;
    let mut match_positions: Vec<usize> = Vec::new();

    for (i, &c) in path_chars.iter().enumerate() {
        if filter_idx < filter_chars.len() && c == filter_chars[filter_idx] {
            match_positions.push(i);
            filter_idx += 1;
        }
    }

    if filter_idx != filter_chars.len() {
        return 0;
    }

    let mut score = 100;

    for window in match_positions.windows(2) {
        if window[1] == window[0] + 1 {
            score += 10;
        }
    }

    for &pos in &match_positions {
        if pos == 0 {
            score += 15;
        } else {
            let prev = path_chars.get(pos.saturating_sub(1));
            if matches!(prev, Some('/' | '.' | '_' | '-')) {
                score += 15;
            }
        }
    }

    if let Some(last_slash) = path.rfind('/') {
        let filename_start = last_slash + 1;
        if match_positions
            .first()
            .is_some_and(|&pos| pos >= filename_start)
        {
            score += 20;
        }
    }

    score -= (path.len() / 10) as i32;

    score.max(1)
}

/// Find match positions for highlighting in the UI.
#[must_use]
pub fn find_match_positions(path: &str, filter: &str) -> Vec<usize> {
    if filter.is_empty() {
        return Vec::new();
    }

    let path_lower = path.to_lowercase();
    let filter_lower = filter.to_lowercase();
    let path_chars: Vec<char> = path_lower.chars().collect();
    let filter_chars: Vec<char> = filter_lower.chars().collect();

    let mut positions = Vec::new();
    let mut filter_idx = 0;

    for (i, &c) in path_chars.iter().enumerate() {
        if filter_idx < filter_chars.len() && c == filter_chars[filter_idx] {
            positions.push(i);
            filter_idx += 1;
        }
    }

    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_basic() {
        let filter: Vec<char> = "lib".chars().collect();
        assert!(fuzzy_match_score("src/lib.rs", &filter) > 0);
        assert!(fuzzy_match_score("library/mod.rs", &filter) > 0);
        assert_eq!(fuzzy_match_score("main.rs", &filter), 0);
    }

    #[test]
    fn fuzzy_match_subsequence() {
        // "sclr" does match "src/lib.rs": s-r-c from "src", l from "lib", r from "rs"
        let filter: Vec<char> = "sclr".chars().collect();
        assert!(fuzzy_match_score("src/lib.rs", &filter) > 0);

        // "xyz" does not match "src/lib.rs"
        let filter: Vec<char> = "xyz".chars().collect();
        assert_eq!(fuzzy_match_score("src/lib.rs", &filter), 0);

        let filter: Vec<char> = "slr".chars().collect();
        // "src/lib.rs" contains s, l, r in order
        assert!(fuzzy_match_score("src/lib.rs", &filter) > 0);
    }

    #[test]
    fn fuzzy_match_prefers_filename() {
        let filter: Vec<char> = "lib".chars().collect();
        let score_filename = fuzzy_match_score("src/lib.rs", &filter);
        let score_path = fuzzy_match_score("library/util/helpers.rs", &filter);
        // lib.rs should score higher since "lib" matches the filename
        assert!(score_filename > score_path);
    }

    #[test]
    fn find_positions_basic() {
        let positions = find_match_positions("src/lib.rs", "lib");
        assert_eq!(positions, vec![4, 5, 6]);
    }

    #[test]
    fn find_positions_case_insensitive() {
        let positions = find_match_positions("README.md", "read");
        assert_eq!(positions, vec![0, 1, 2, 3]);
    }
}

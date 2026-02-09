//! Change recording capability tokens for tool executors.
//!
//! `TurnContext` is a proof object (IFA §10) that a user turn is active.
//! `ChangeRecorder` is a capability token derived from `TurnContext` that
//! grants tool executors the ability to record file changes during execution.
//!
//! The turn lifecycle is:
//! 1. Engine creates `TurnContext` when a user message is queued
//! 2. Engine derives `ChangeRecorder` tokens and passes them into `ToolCtx`
//! 3. Tool executors call `record_created`/`record_modified`/`record_stats`
//! 4. Engine consumes `TurnContext` via `finish()` to get the report

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use forge_types::NonEmptyString;

/// Stats for a single file change (lines added/removed).
#[derive(Debug, Clone, Copy, Default)]
pub struct DiffStats {
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Default)]
struct TurnChangeLog {
    created: BTreeSet<PathBuf>,
    modified: BTreeSet<PathBuf>,
    stats: HashMap<PathBuf, DiffStats>,
}

/// Proof that a user turn is active (IFA §10 capability token).
///
/// Consumed by `finish()` to produce a `TurnChangeReport`.
#[derive(Debug)]
pub struct TurnContext {
    changes: Arc<Mutex<TurnChangeLog>>,
}

/// Capability token that allows tool executors to record file changes.
///
/// Derived from `TurnContext` — cannot be constructed without an active turn.
#[derive(Debug, Clone)]
pub struct ChangeRecorder {
    changes: Arc<Mutex<TurnChangeLog>>,
}

/// Result of finishing a turn.
#[derive(Debug)]
pub enum TurnChangeReport {
    NoChanges,
    Changes(TurnChangeSummary),
}

/// Non-empty summary of changes made during a turn.
#[derive(Debug)]
pub struct TurnChangeSummary {
    content: NonEmptyString,
}

impl Default for TurnContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnContext {
    #[must_use]
    pub fn new() -> Self {
        Self {
            changes: Arc::new(Mutex::new(TurnChangeLog::default())),
        }
    }

    #[must_use]
    pub fn new_for_recovery() -> Self {
        Self::new()
    }

    #[must_use]
    pub fn new_for_tests() -> Self {
        Self::new()
    }

    #[must_use]
    pub fn recorder(&self) -> ChangeRecorder {
        ChangeRecorder {
            changes: Arc::clone(&self.changes),
        }
    }

    /// Consume the turn and produce a report with the raw path sets.
    ///
    /// The raw sets are used for session-wide aggregation in the files panel.
    pub fn finish(
        self,
        working_dir: &Path,
    ) -> (TurnChangeReport, BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
        let mut log = self
            .changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let log = std::mem::take(&mut *log);
        let created = log.created.clone();
        let modified = log.modified.clone();
        let report = log.into_report(working_dir);
        (report, created, modified)
    }
}

impl ChangeRecorder {
    pub fn record_created(&self, path: PathBuf) {
        let mut log = self
            .changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.record_created(path);
    }

    pub fn record_modified(&self, path: PathBuf) {
        let mut log = self
            .changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.record_modified(path);
    }

    pub fn record_stats(&self, path: PathBuf, additions: u32, deletions: u32) {
        let mut log = self
            .changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = log.stats.entry(path).or_default();
        entry.additions = entry.additions.saturating_add(additions);
        entry.deletions = entry.deletions.saturating_add(deletions);
    }
}

impl TurnChangeSummary {
    #[must_use]
    pub fn into_message(self) -> NonEmptyString {
        self.content
    }
}

impl TurnChangeLog {
    fn record_created(&mut self, path: PathBuf) {
        self.modified.remove(&path);
        self.created.insert(path);
    }

    fn record_modified(&mut self, path: PathBuf) {
        if !self.created.contains(&path) {
            self.modified.insert(path);
        }
    }

    fn into_report(self, working_dir: &Path) -> TurnChangeReport {
        if self.created.is_empty() && self.modified.is_empty() {
            return TurnChangeReport::NoChanges;
        }

        let created = format_paths_with_stats(&self.created, &self.stats, working_dir);
        let modified = format_paths_with_stats(&self.modified, &self.stats, working_dir);
        TurnChangeReport::Changes(TurnChangeSummary::new(created, modified))
    }
}

fn format_paths_with_stats(
    paths: &BTreeSet<PathBuf>,
    stats: &HashMap<PathBuf, DiffStats>,
    working_dir: &Path,
) -> Vec<(String, Option<DiffStats>)> {
    paths
        .iter()
        .map(|path| {
            let display = format_path(path, working_dir);
            let file_stats = stats.get(path).copied();
            (display, file_stats)
        })
        .collect()
}

fn format_path(path: &Path, working_dir: &Path) -> String {
    path.strip_prefix(working_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

impl TurnChangeSummary {
    fn new(
        created: Vec<(String, Option<DiffStats>)>,
        modified: Vec<(String, Option<DiffStats>)>,
    ) -> Self {
        let mut lines: Vec<String> = Vec::new();

        if !created.is_empty() {
            lines.push(format!("Created files ({}):", created.len()));
            lines.extend(
                created
                    .into_iter()
                    .map(|(path, stats)| format_file_line(&path, stats)),
            );
        }

        if !modified.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("Modified files ({}):", modified.len()));
            lines.extend(
                modified
                    .into_iter()
                    .map(|(path, stats)| format_file_line(&path, stats)),
            );
        }

        let content = NonEmptyString::new(lines.join("\n"))
            .expect("summary must be non-empty when changes exist");
        Self { content }
    }
}

fn format_file_line(path: &str, stats: Option<DiffStats>) -> String {
    match stats {
        Some(s) if s.additions > 0 || s.deletions > 0 => {
            format!("- {path} (+{}, -{})", s.additions, s.deletions)
        }
        _ => format!("- {path}"),
    }
}

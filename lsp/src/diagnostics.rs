//! Diagnostics store — accumulates per-file diagnostics from language servers.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::{DiagnosticsSnapshot, ForgeDiagnostic};

pub(crate) struct DiagnosticsStore {
    data: HashMap<PathBuf, Vec<ForgeDiagnostic>>,
}

impl DiagnosticsStore {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn update(&mut self, path: PathBuf, items: Vec<ForgeDiagnostic>) {
        if items.is_empty() {
            self.data.remove(&path);
        } else {
            self.data.insert(path, items);
        }
    }

    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        let mut files: Vec<(PathBuf, Vec<ForgeDiagnostic>)> = self
            .data
            .iter()
            .map(|(path, items)| (path.clone(), items.clone()))
            .collect();

        // Sort: files with errors first, then alphabetically
        files.sort_by(|a, b| {
            let a_has_errors = a.1.iter().any(|d| d.severity().is_error());
            let b_has_errors = b.1.iter().any(|d| d.severity().is_error());
            b_has_errors.cmp(&a_has_errors).then_with(|| a.0.cmp(&b.0))
        });

        DiagnosticsSnapshot::new(files)
    }

    pub fn errors_for_files(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Vec<ForgeDiagnostic>)> {
        let mut result = Vec::new();
        for path in paths {
            if let Some(items) = self.data.get(path) {
                let errors: Vec<ForgeDiagnostic> = items
                    .iter()
                    .filter(|d| d.severity().is_error())
                    .cloned()
                    .collect();
                if !errors.is_empty() {
                    result.push((path.clone(), errors));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiagnosticSeverity;

    fn make_diag(severity: DiagnosticSeverity, msg: &str, line: u32) -> ForgeDiagnostic {
        ForgeDiagnostic::new(severity, msg.to_string(), line, 0, "test".to_string())
    }

    #[test]
    fn test_empty_snapshot() {
        let store = DiagnosticsStore::new();
        let snap = store.snapshot();
        assert!(snap.is_empty());
        assert_eq!(snap.error_count(), 0);
        assert_eq!(snap.warning_count(), 0);
    }

    #[test]
    fn test_update_and_snapshot() {
        let mut store = DiagnosticsStore::new();
        let path = PathBuf::from("src/main.rs");
        store.update(
            path.clone(),
            vec![
                make_diag(DiagnosticSeverity::Error, "expected `;`", 10),
                make_diag(DiagnosticSeverity::Warning, "unused variable", 20),
            ],
        );

        let snap = store.snapshot();
        assert_eq!(snap.error_count(), 1);
        assert_eq!(snap.warning_count(), 1);
        assert_eq!(snap.files().len(), 1);
        assert_eq!(snap.files()[0].0, path);
    }

    #[test]
    fn test_empty_diagnostics_removes_file() {
        let mut store = DiagnosticsStore::new();
        let path = PathBuf::from("src/main.rs");
        store.update(
            path.clone(),
            vec![make_diag(DiagnosticSeverity::Error, "err", 1)],
        );
        assert_eq!(store.snapshot().files().len(), 1);

        store.update(path, vec![]);
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn test_errors_first_sorting() {
        let mut store = DiagnosticsStore::new();
        store.update(
            PathBuf::from("b.rs"),
            vec![make_diag(DiagnosticSeverity::Warning, "warn", 1)],
        );
        store.update(
            PathBuf::from("a.rs"),
            vec![make_diag(DiagnosticSeverity::Error, "err", 1)],
        );

        let snap = store.snapshot();
        // a.rs has error → should come first despite alphabetical
        assert_eq!(snap.files()[0].0, PathBuf::from("a.rs"));
        assert_eq!(snap.files()[1].0, PathBuf::from("b.rs"));
    }

    #[test]
    fn test_errors_for_files() {
        let mut store = DiagnosticsStore::new();
        let path_a = PathBuf::from("a.rs");
        let path_b = PathBuf::from("b.rs");
        store.update(
            path_a.clone(),
            vec![
                make_diag(DiagnosticSeverity::Error, "err", 1),
                make_diag(DiagnosticSeverity::Warning, "warn", 2),
            ],
        );
        store.update(
            path_b.clone(),
            vec![make_diag(DiagnosticSeverity::Warning, "warn only", 1)],
        );

        let result = store.errors_for_files(&[path_a.clone(), path_b.clone()]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, path_a);
        assert_eq!(result[0].1.len(), 1);
    }

    #[test]
    fn test_replace_overwrites_previous() {
        let mut store = DiagnosticsStore::new();
        let path = PathBuf::from("main.rs");
        store.update(
            path.clone(),
            vec![
                make_diag(DiagnosticSeverity::Error, "err1", 1),
                make_diag(DiagnosticSeverity::Error, "err2", 2),
            ],
        );
        assert_eq!(store.snapshot().error_count(), 2);

        // Server re-publishes with only one error
        store.update(path, vec![make_diag(DiagnosticSeverity::Error, "err1", 1)]);
        assert_eq!(store.snapshot().error_count(), 1);
    }

    #[test]
    fn test_status_string() {
        let mut store = DiagnosticsStore::new();
        assert_eq!(store.snapshot().status_string(), "");

        store.update(
            PathBuf::from("a.rs"),
            vec![
                make_diag(DiagnosticSeverity::Error, "e", 1),
                make_diag(DiagnosticSeverity::Warning, "w", 2),
                make_diag(DiagnosticSeverity::Warning, "w2", 3),
            ],
        );
        assert_eq!(store.snapshot().status_string(), "E:1 W:2");
    }
}

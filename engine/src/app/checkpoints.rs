//! Checkpointing and rewind support.
//!
//! # Design notes (DESIGN.md)
//!
//! This module follows the repo's invariant-first / type-driven style:
//!
//! - A checkpoint only supports **code rewind** when it contains a `WorkspaceSnapshot`.
//!   Callers must obtain a `PreparedCodeRewind` proof before they can restore files.
//! - File existence is encoded as an enum (`FileSnapshot`), avoiding "maybe-bytes" states.
//! - Parsing happens at the boundary (`CheckpointId::parse`, `RewindScope::parse`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::Local;

use crate::tools::{self, lp1};

/// Opaque identifier for a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct CheckpointId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointIdParse {
    Valid(CheckpointId),
    Invalid,
}

impl CheckpointId {
    pub(crate) fn parse(raw: &str) -> CheckpointIdParse {
        match raw.parse::<u64>() {
            Ok(value) => CheckpointIdParse::Valid(Self(value)),
            Err(_) => CheckpointIdParse::Invalid,
        }
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a checkpoint exists (used for UX like /undo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointKind {
    /// Automatic checkpoint taken at the start of each user turn.
    Turn,
    /// Automatic checkpoint taken before tool-driven workspace edits.
    ToolEdit,
    /// Checkpoint taken when a plan step completes (advance/skip).
    PlanStep(forge_types::PlanStepId),
}

impl CheckpointKind {
    fn label(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::ToolEdit => "tool",
            Self::PlanStep(_) => "plan",
        }
    }
}

/// What to rewind when restoring a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindScope {
    Conversation,
    Code,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindScopeParse {
    Valid(RewindScope),
    Invalid,
}

impl RewindScope {
    pub(crate) fn parse(raw: &str) -> RewindScopeParse {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "both" => RewindScopeParse::Valid(Self::Both),
            "code" => RewindScopeParse::Valid(Self::Code),
            "conversation" | "chat" => RewindScopeParse::Valid(Self::Conversation),
            _ => RewindScopeParse::Invalid,
        }
    }

    pub(crate) fn includes_conversation(self) -> bool {
        matches!(self, Self::Conversation | Self::Both)
    }

    pub(crate) fn includes_code(self) -> bool {
        matches!(self, Self::Code | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointTarget<'a> {
    Latest,
    Id(&'a str),
}

#[derive(Debug)]
pub(crate) enum CheckpointTargetResolution {
    Resolved(PreparedRewind),
    Rejected,
}

/// A compact, user-facing view of a checkpoint.
#[derive(Debug, Clone)]
pub(crate) struct CheckpointSummary {
    pub id: CheckpointId,
    pub created_at: SystemTime,
    pub kind: CheckpointKind,
    pub has_code: bool,
    pub file_count: usize,
    pub total_bytes: usize,
}

impl CheckpointSummary {
    pub(crate) fn format_line(&self) -> String {
        let ts_utc = chrono::DateTime::<chrono::Utc>::from(self.created_at);
        let when = ts_utc
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let kind = self.kind.label();
        let code = if self.has_code { "code+chat" } else { "chat" };
        let size = format_bytes(self.total_bytes);
        format!(
            "#{id}  {when}  {kind}  {code}  files:{files}  {size}",
            id = self.id,
            files = self.file_count
        )
    }
}

#[derive(Debug)]
pub(crate) struct Checkpoint {
    id: CheckpointId,
    created_at: SystemTime,
    kind: CheckpointKind,
    /// Number of history entries present when the checkpoint was created.
    conversation_len: usize,
    /// Present iff this checkpoint supports rewinding the workspace.
    workspace: Option<WorkspaceSnapshot>,
}

impl Checkpoint {
    pub(crate) fn id(&self) -> CheckpointId {
        self.id
    }

    pub(crate) fn conversation_len(&self) -> usize {
        self.conversation_len
    }

    pub(crate) fn summary(&self) -> CheckpointSummary {
        let (has_code, file_count, total_bytes) = match &self.workspace {
            Some(ws) => (true, ws.files.len(), ws.total_bytes),
            None => (false, 0, 0),
        };
        CheckpointSummary {
            id: self.id,
            created_at: self.created_at,
            kind: self.kind,
            has_code,
            file_count,
            total_bytes,
        }
    }
}

#[derive(Debug)]
pub(crate) struct WorkspaceSnapshot {
    files: BTreeMap<PathBuf, FileSnapshot>,
    total_bytes: usize,
}

#[derive(Debug)]
pub(crate) enum FileSnapshot {
    Existed {
        bytes: Vec<u8>,
        permissions: Option<std::fs::Permissions>,
    },
    Missing,
}

/// Proof that a rewind target exists.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparedRewind {
    id: CheckpointId,
}

/// Proof that a rewind target exists *and* contains a code snapshot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparedCodeRewind {
    id: CheckpointId,
}

/// Proof that a file baseline exists in a checkpoint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparedFileBaseline {
    checkpoint_id: CheckpointId,
}

#[derive(Debug, Clone)]
pub(crate) struct CreatedCheckpoint {
    pub id: CheckpointId,
    pub file_count: usize,
    pub total_bytes: usize,
    pub warning: Option<String>,
}

/// Intentionally simple (`QoL` feature). If/when persistence is needed,
/// we can add a journal without changing the public proof-oriented API.
#[derive(Debug, Default)]
pub(crate) struct CheckpointStore {
    next_id: u64,
    checkpoints: Vec<Checkpoint>,
}

impl CheckpointStore {
    /// Max checkpoints retained in memory.
    const MAX_CHECKPOINTS: usize = 50;

    pub(crate) fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    pub(crate) fn latest_id(&self) -> Option<CheckpointId> {
        self.checkpoints.last().map(|c| c.id)
    }

    pub(crate) fn latest_id_of_kind(&self, kind: CheckpointKind) -> Option<CheckpointId> {
        self.checkpoints
            .iter()
            .rev()
            .find(|c| c.kind == kind)
            .map(|c| c.id)
    }

    pub(crate) fn summaries(&self) -> Vec<CheckpointSummary> {
        self.checkpoints.iter().map(Checkpoint::summary).collect()
    }

    pub(crate) fn get(&self, id: CheckpointId) -> Option<&Checkpoint> {
        self.checkpoints.iter().find(|c| c.id == id)
    }

    pub(crate) fn prepare(&self, id: CheckpointId) -> Option<PreparedRewind> {
        self.get(id).map(|_| PreparedRewind { id })
    }

    pub(crate) fn prepare_latest(&self) -> Option<PreparedRewind> {
        self.latest_id().and_then(|id| self.prepare(id))
    }

    pub(crate) fn prepare_latest_of_kind(&self, kind: CheckpointKind) -> Option<PreparedRewind> {
        self.latest_id_of_kind(kind).and_then(|id| self.prepare(id))
    }

    pub(crate) fn prepare_code(&self, rewind: PreparedRewind) -> Option<PreparedCodeRewind> {
        let cp = self.get(rewind.id)?;
        cp.workspace.as_ref()?;
        Some(PreparedCodeRewind { id: rewind.id })
    }

    pub(crate) fn checkpoint(&self, proof: PreparedRewind) -> &Checkpoint {
        // Proof ensures existence; absence is a logic bug at the call site.
        self.get(proof.id).expect("checkpoint exists")
    }

    pub(crate) fn checkpoint_code(
        &self,
        proof: PreparedCodeRewind,
    ) -> (&Checkpoint, &WorkspaceSnapshot) {
        let cp = self.get(proof.id).expect("checkpoint exists");
        let ws = cp.workspace.as_ref().expect("code snapshot exists");
        (cp, ws)
    }

    /// Create a checkpoint for the given set of files.
    ///
    /// - If all files can be snapshotted, the checkpoint supports code rewind.
    /// - If any file snapshot fails, we still create a **conversation-only** checkpoint and
    ///   return a warning.
    pub(crate) fn create_for_files(
        &mut self,
        kind: CheckpointKind,
        conversation_len: usize,
        files: impl IntoIterator<Item = PathBuf>,
    ) -> CreatedCheckpoint {
        let id = CheckpointId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);

        let created_at = SystemTime::now();

        let unique: BTreeSet<PathBuf> = files.into_iter().collect();

        let mut snapshots: BTreeMap<PathBuf, FileSnapshot> = BTreeMap::new();
        let mut total_bytes: usize = 0;
        let mut warning: Option<String> = None;

        for path in &unique {
            match snapshot_file(path) {
                Ok((snap, bytes)) => {
                    total_bytes = total_bytes.saturating_add(bytes);
                    snapshots.insert(path.clone(), snap);
                }
                Err(e) => {
                    warning = Some(format!(
                        "Checkpoint {id} created without code snapshot (failed to read {}: {e})",
                        path.display()
                    ));
                    snapshots.clear();
                    total_bytes = 0;
                    break;
                }
            }
        }

        // Empty target set is intentionally conversation-only (no code snapshot to restore).
        let workspace = if unique.is_empty() {
            None
        } else if warning.is_none() {
            Some(WorkspaceSnapshot {
                files: snapshots,
                total_bytes,
            })
        } else {
            None
        };

        let file_count = workspace.as_ref().map_or(0, |w| w.files.len());
        self.checkpoints.push(Checkpoint {
            id,
            created_at,
            kind,
            conversation_len,
            workspace,
        });

        if self.checkpoints.len() > Self::MAX_CHECKPOINTS {
            let overflow = self.checkpoints.len().saturating_sub(Self::MAX_CHECKPOINTS);
            self.checkpoints.drain(0..overflow);
        }

        CreatedCheckpoint {
            id,
            file_count,
            total_bytes,
            warning,
        }
    }

    /// Drop checkpoints that occur *after* the provided id.
    ///
    /// Used when rewinding conversation to avoid keeping checkpoints that point into
    /// a discarded timeline.
    pub(crate) fn prune_after(&mut self, id: CheckpointId) {
        if let Some(pos) = self.checkpoints.iter().position(|c| c.id == id) {
            self.checkpoints.truncate(pos + 1);
        }
    }

    /// Find the most recent `ToolEdit` checkpoint containing a snapshot of `path`.
    pub(crate) fn find_baseline_for_file(&self, path: &Path) -> Option<PreparedFileBaseline> {
        let normalized = normalize_path(path);

        for checkpoint in self.checkpoints.iter().rev() {
            if checkpoint.kind == CheckpointKind::ToolEdit
                && let Some(ws) = &checkpoint.workspace
            {
                let has_file = ws
                    .files
                    .keys()
                    .any(|stored| stored == path || normalize_path(stored) == normalized);
                if has_file {
                    return Some(PreparedFileBaseline {
                        checkpoint_id: checkpoint.id,
                    });
                }
            }
        }
        None
    }

    /// Get file content from a checkpoint, given a baseline proof.
    /// Returns None if file was marked as Missing (didn't exist at checkpoint time).
    pub(crate) fn baseline_content(
        &self,
        proof: PreparedFileBaseline,
        path: &Path,
    ) -> Option<&[u8]> {
        let cp = self.get(proof.checkpoint_id)?;
        let ws = cp.workspace.as_ref()?;

        let normalized = normalize_path(path);
        let snapshot = ws.files.get(path).or_else(|| {
            ws.files
                .iter()
                .find(|(k, _)| normalize_path(k) == normalized)
                .map(|(_, v)| v)
        })?;

        match snapshot {
            FileSnapshot::Existed { bytes, .. } => Some(bytes),
            FileSnapshot::Missing => None,
        }
    }
}

/// Normalize path for comparison.
fn normalize_path(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Collect file targets that will be edited by tool calls.
///
/// This is a boundary function: it parses untrusted arguments and performs sandbox
/// resolution, returning canonical paths suitable for snapshotting.
pub(crate) fn collect_edit_targets<'a>(
    calls: impl IntoIterator<Item = &'a forge_types::ToolCall>,
    sandbox: &tools::sandbox::Sandbox,
    working_dir: &Path,
) -> Result<Vec<PathBuf>, tools::ToolError> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();

    for call in calls {
        match call.name.as_str() {
            "Edit" => {
                let Some(patch) = call.arguments.get("patch").and_then(|v| v.as_str()) else {
                    continue;
                };
                let parsed = lp1::parse_patch(patch).map_err(|e| tools::ToolError::BadArgs {
                    message: e.to_string(),
                })?;
                for fp in parsed.files {
                    let resolved = sandbox.resolve_path(&fp.path, working_dir)?;
                    out.insert(resolved);
                }
            }
            "Write" => {
                let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) else {
                    continue;
                };
                let resolved = sandbox.resolve_path_for_create(path, working_dir)?;
                out.insert(resolved);
            }
            _ => {}
        }
    }

    Ok(out.into_iter().collect())
}

/// Report returned after restoring a workspace snapshot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkspaceRestoreReport {
    pub restored_files: usize,
    pub removed_files: usize,
}

/// Restore a workspace snapshot onto disk.
///
/// This is a boundary operation. It can fail due to filesystem permissions,
/// concurrent modifications, etc.
pub(crate) fn restore_workspace(
    snapshot: &WorkspaceSnapshot,
) -> std::io::Result<WorkspaceRestoreReport> {
    let mut restored_files: usize = 0;
    let mut removed_files: usize = 0;

    for (path, snap) in &snapshot.files {
        match snap {
            FileSnapshot::Existed { bytes, permissions } => {
                restore_file(path, bytes, permissions.as_ref())?;
                restored_files = restored_files.saturating_add(1);
            }
            FileSnapshot::Missing => {
                removed_files = removed_files.saturating_add(remove_if_exists(path)?);
            }
        }
    }

    Ok(WorkspaceRestoreReport {
        restored_files,
        removed_files,
    })
}

fn snapshot_file(path: &Path) -> std::io::Result<(FileSnapshot, usize)> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let bytes = std::fs::read(path)?;
            let perms = Some(meta.permissions());
            let len = bytes.len();
            Ok((
                FileSnapshot::Existed {
                    bytes,
                    permissions: perms,
                },
                len,
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((FileSnapshot::Missing, 0)),
        Err(e) => Err(e),
    }
}

fn restore_file(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&std::fs::Permissions>,
) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.is_dir()
    {
        return Err(std::io::Error::other(format!(
            "Refusing to overwrite directory: {}",
            path.display()
        )));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, bytes)?;
    if let Some(perms) = permissions {
        std::fs::set_permissions(path, perms.clone())?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> std::io::Result<usize> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Err(std::io::Error::other(format!(
            "Refusing to remove directory: {}",
            path.display()
        ))),
        Ok(_meta) => {
            std::fs::remove_file(path)?;
            Ok(1)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1}GB", b / GB)
    } else if b >= MB {
        format!("{:.1}MB", b / MB)
    } else if b >= KB {
        format!("{:.1}KB", b / KB)
    } else {
        format!("{bytes}B")
    }
}

impl crate::App {
    /// Create an automatic conversation-only checkpoint at the start of a user turn.
    ///
    /// This is intentionally silent (no notification spam). It is discoverable via /rewind list,
    /// and consumed by /undo and /retry.
    pub(crate) fn create_turn_checkpoint(&mut self) {
        let conversation_len = self.core.context_manager.history().len();
        let _ = self.core.checkpoints.create_for_files(
            CheckpointKind::Turn,
            conversation_len,
            Vec::<PathBuf>::new(),
        );
    }

    /// Create a conversation-only checkpoint when a plan step completes (advance/skip).
    pub(crate) fn create_plan_step_checkpoint(&mut self, step_id: forge_types::PlanStepId) {
        let conversation_len = self.core.context_manager.history().len();
        let _ = self.core.checkpoints.create_for_files(
            CheckpointKind::PlanStep(step_id),
            conversation_len,
            Vec::<PathBuf>::new(),
        );
    }

    /// Obtain a proof for the latest per-turn checkpoint (used by /undo, /retry).
    pub(crate) fn prepare_latest_turn_checkpoint(&mut self) -> Option<PreparedRewind> {
        let Some(proof) = self
            .core
            .checkpoints
            .prepare_latest_of_kind(CheckpointKind::Turn)
        else {
            self.push_notification("No turn checkpoints available");
            return None;
        };
        Some(proof)
    }

    /// Create an automatic checkpoint if the tool batch includes file edits.
    ///
    /// This is called from the tool loop before any side-effecting file write tools run.
    pub(crate) fn maybe_create_checkpoint_for_tool_calls<'a>(
        &mut self,
        calls: impl IntoIterator<Item = &'a forge_types::ToolCall>,
    ) {
        let working_dir = self.runtime.tool_settings.sandbox.working_dir();
        let targets =
            match collect_edit_targets(calls, &self.runtime.tool_settings.sandbox, &working_dir) {
                Ok(t) => t,
                Err(e) => {
                    // A failed checkpoint must not block tool execution.
                    self.push_notification(format!("Checkpointing skipped (sandbox error): {e}"));
                    return;
                }
            };

        if targets.is_empty() {
            return;
        }

        let conversation_len = self.core.context_manager.history().len();
        let created = self.core.checkpoints.create_for_files(
            CheckpointKind::ToolEdit,
            conversation_len,
            targets,
        );

        if let Some(warning) = created.warning {
            self.push_notification(warning);
        } else {
            self.push_notification(format!(
                "Checkpoint #{id} created ({files} files, {size})",
                id = created.id,
                files = created.file_count,
                size = format_bytes(created.total_bytes)
            ));
        }
    }

    /// Show a short list of available checkpoints.
    pub(crate) fn show_checkpoint_list(&mut self) {
        if self.core.checkpoints.is_empty() {
            self.push_notification(
                "No checkpoints yet. Forge creates them automatically at the start of each turn and before apply_patch/write_file.",
            );
            return;
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("Checkpoints (newest last):".to_string());
        for s in self
            .core
            .checkpoints
            .summaries()
            .into_iter()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            lines.push(format!("- {}", s.format_line()));
        }
        lines.push("Usage: /rewind <id|last> [code|conversation|both]".to_string());
        lines.push(
            "Shortcuts: /undo (rewind last turn), /retry (undo + restore prompt)".to_string(),
        );

        self.push_notification(lines.join("\n"));
    }

    /// Parse /rewind target argument into a proof that the checkpoint exists.
    pub(crate) fn parse_checkpoint_target(
        &mut self,
        target: CheckpointTarget<'_>,
    ) -> CheckpointTargetResolution {
        match target {
            CheckpointTarget::Latest => {
                if let Some(proof) = self.core.checkpoints.prepare_latest() {
                    return CheckpointTargetResolution::Resolved(proof);
                }
                self.push_notification("No checkpoints available");
                CheckpointTargetResolution::Rejected
            }
            CheckpointTarget::Id(raw_id) => {
                let normalized = raw_id.trim();
                let normalized = normalized.strip_prefix('#').unwrap_or(normalized);
                let id = match CheckpointId::parse(normalized) {
                    CheckpointIdParse::Valid(id) => id,
                    CheckpointIdParse::Invalid => {
                        self.push_notification(format!("Invalid checkpoint id: {normalized}"));
                        return CheckpointTargetResolution::Rejected;
                    }
                };

                if let Some(p) = self.core.checkpoints.prepare(id) {
                    CheckpointTargetResolution::Resolved(p)
                } else {
                    self.push_notification(format!("Unknown checkpoint id: {id}"));
                    CheckpointTargetResolution::Rejected
                }
            }
        }
    }

    /// Apply a checkpoint rewind.
    ///
    /// Returns Err with a user-facing message on failure.
    pub(crate) fn apply_rewind(
        &mut self,
        proof: PreparedRewind,
        scope: RewindScope,
    ) -> Result<(), String> {
        let (id, conversation_len) = {
            let cp = self.core.checkpoints.checkpoint(proof);
            (cp.id(), cp.conversation_len())
        };

        // Preflight conversation rewind so a /rewind both doesn't partially succeed.
        if scope.includes_conversation() {
            self.can_truncate_history_to(conversation_len)?;
        }

        if scope.includes_code() {
            let Some(code_proof) = self.core.checkpoints.prepare_code(proof) else {
                return Err(format!("Checkpoint #{id} does not contain a code snapshot"));
            };
            let (_cp, ws) = self.core.checkpoints.checkpoint_code(code_proof);
            let report = restore_workspace(ws).map_err(|e| format!("Code rewind failed: {e}"))?;

            // Clear tool file cache to avoid stale-file protection false positives.
            if let Ok(mut guard) = self.runtime.tool_file_cache.try_lock() {
                guard.clear();
            }

            self.push_notification(format!(
                "Workspace restored from checkpoint #{id} (restored:{r} removed:{d})",
                r = report.restored_files,
                d = report.removed_files,
            ));
        }

        if scope.includes_conversation() {
            self.truncate_history_to(conversation_len)?;
            self.core.checkpoints.prune_after(id);
            self.autosave_history();
            self.push_notification(format!("Conversation rewound to checkpoint #{id}"));
        }

        Ok(())
    }

    fn truncate_history_to(&mut self, target_len: usize) -> Result<(), String> {
        self.can_truncate_history_to(target_len)?;

        // The ContextManager only supports rollback_last_message for the most recent entry.
        // We need to call it repeatedly to truncate down to target_len.
        let mut current = self.core.context_manager.history().len();
        while current > target_len {
            let last_id = self
                .core
                .context_manager
                .history()
                .entries()
                .last()
                .map(forge_context::HistoryEntry::id)
                .ok_or_else(|| "History unexpectedly empty".to_string())?;

            // rollback_last_message returns None if the entry is Distilled or not the last
            let _removed = self
                .core
                .context_manager
                .rollback_last_message(last_id)
                .ok_or_else(|| {
                    "Cannot rewind conversation (unexpected Distilled tail)".to_string()
                })?;
            current = self.core.context_manager.history().len();
        }

        self.rebuild_display_from_history();
        self.invalidate_usage_cache();
        self.core.pending_user_message = None;
        self.scroll_to_bottom();
        Ok(())
    }

    /// Verify we can truncate history down to `target_len` without mutating state.
    fn can_truncate_history_to(&self, target_len: usize) -> Result<(), String> {
        let entries = self.core.context_manager.history().entries();
        if target_len >= entries.len() {
            return Ok(());
        }

        // Cannot rewind past a compaction point — the pre-compaction messages
        // are display-only and no longer part of the API view.
        let history = self.core.context_manager.history();
        if history.is_compacted() {
            let api_len = history.api_entries().len();
            if target_len < entries.len().saturating_sub(api_len) {
                return Err("Cannot rewind conversation past the compaction point".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CheckpointId, CheckpointIdParse, CheckpointKind, CheckpointStore, RewindScope,
        RewindScopeParse, format_bytes,
    };
    use std::path::PathBuf;

    #[test]
    fn parse_rewind_scope() {
        assert_eq!(
            RewindScope::parse(""),
            RewindScopeParse::Valid(RewindScope::Both)
        );
        assert_eq!(
            RewindScope::parse("both"),
            RewindScopeParse::Valid(RewindScope::Both)
        );
        assert_eq!(
            RewindScope::parse("BOTH"),
            RewindScopeParse::Valid(RewindScope::Both)
        );
        assert_eq!(
            RewindScope::parse("code"),
            RewindScopeParse::Valid(RewindScope::Code)
        );
        assert_eq!(
            RewindScope::parse("CODE"),
            RewindScopeParse::Valid(RewindScope::Code)
        );
        assert_eq!(
            RewindScope::parse("conversation"),
            RewindScopeParse::Valid(RewindScope::Conversation)
        );
        assert_eq!(
            RewindScope::parse("chat"),
            RewindScopeParse::Valid(RewindScope::Conversation)
        );
        assert_eq!(RewindScope::parse("invalid"), RewindScopeParse::Invalid);
    }

    #[test]
    fn parse_checkpoint_id() {
        assert_eq!(
            CheckpointId::parse("0"),
            CheckpointIdParse::Valid(CheckpointId(0))
        );
        assert_eq!(
            CheckpointId::parse("42"),
            CheckpointIdParse::Valid(CheckpointId(42))
        );
        assert_eq!(CheckpointId::parse("abc"), CheckpointIdParse::Invalid);
        assert_eq!(CheckpointId::parse("-1"), CheckpointIdParse::Invalid);
    }

    #[test]
    fn checkpoint_store_basic_ops() {
        let mut store = CheckpointStore::default();
        assert!(store.is_empty());
        assert!(store.latest_id().is_none());

        let created = store.create_for_files(CheckpointKind::Turn, 5, Vec::<PathBuf>::new());
        assert_eq!(created.id, CheckpointId(0));
        assert_eq!(created.file_count, 0);
        assert_eq!(created.total_bytes, 0);
        assert!(!store.is_empty());
        assert_eq!(store.latest_id(), Some(CheckpointId(0)));

        let proof = store.prepare(CheckpointId(0)).unwrap();
        let cp = store.checkpoint(proof);
        assert_eq!(cp.conversation_len(), 5);
    }

    #[test]
    fn checkpoint_store_prune_after() {
        let mut store = CheckpointStore::default();

        store.create_for_files(CheckpointKind::Turn, 1, Vec::<PathBuf>::new());
        store.create_for_files(CheckpointKind::Turn, 2, Vec::<PathBuf>::new());
        store.create_for_files(CheckpointKind::Turn, 3, Vec::<PathBuf>::new());

        assert_eq!(store.summaries().len(), 3);

        store.prune_after(CheckpointId(1));
        assert_eq!(store.summaries().len(), 2);
        assert_eq!(store.latest_id(), Some(CheckpointId(1)));
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1536), "1.5KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GB");
    }
}

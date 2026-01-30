// Tool Journal - tool batch durability for crash recovery
//
// Records tool calls/results so pending tool batches can be recovered.
// This is intentionally minimal: one active uncommitted batch at a time.

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::OpenOptions;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use forge_types::{ToolCall, ToolResult};

/// Unique identifier for a tool batch.
pub type ToolBatchId = i64;

/// Recovered tool batch data after a crash.
#[derive(Debug, Clone)]
pub struct RecoveredToolBatch {
    pub batch_id: ToolBatchId,
    pub model_name: String,
    pub assistant_text: String,
    pub calls: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
}

/// Tool journal for durable tool batch tracking.
///
/// Guarantees that tool calls and results are persisted so recovery can
/// reconstruct partial tool batches after a crash.
pub struct ToolJournal {
    db: Connection,
}

impl ToolJournal {
    const SCHEMA: &'static str = r"
        CREATE TABLE IF NOT EXISTS tool_batches (
            batch_id INTEGER PRIMARY KEY,
            model_name TEXT NOT NULL,
            assistant_text TEXT NOT NULL,
            committed INTEGER DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tool_calls (
            batch_id INTEGER NOT NULL,
            seq INTEGER NOT NULL,
            tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            arguments_json TEXT NOT NULL,
            thought_signature TEXT,
            PRIMARY KEY (batch_id, seq)
        );

        CREATE TABLE IF NOT EXISTS tool_results (
            batch_id INTEGER NOT NULL,
            tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            content TEXT NOT NULL,
            is_error INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (batch_id, tool_call_id)
        );

        CREATE INDEX IF NOT EXISTS idx_tool_batches_committed
        ON tool_batches(committed) WHERE committed = 0;

        CREATE INDEX IF NOT EXISTS idx_tool_calls_batch
        ON tool_calls(batch_id, seq);

        CREATE INDEX IF NOT EXISTS idx_tool_results_batch
        ON tool_results(batch_id);
    ";

    /// Open or create tool journal database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
        if let Some(parent) = path.parent() {
            ensure_secure_dir(parent)?;
        }
        ensure_secure_db_files(path)?;

        let db = Connection::open(path)
            .with_context(|| format!("Failed to open tool journal at {}", path.display()))?;
        Self::initialize(db)
    }

    /// Open an in-memory journal (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().context("Failed to open in-memory tool journal")?;
        Self::initialize(db)
    }

    fn initialize(db: Connection) -> Result<Self> {
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .context("Failed to set tool journal pragmas")?;
        db.execute_batch(Self::SCHEMA)
            .context("Failed to create tool journal schema")?;
        ensure_tool_calls_signature(&db)?;
        ensure_tool_results_name(&db)?;
        Ok(Self { db })
    }

    /// Begin a new tool batch and persist its tool calls.
    ///
    /// Returns the new batch ID.
    pub fn begin_batch(
        &mut self,
        model_name: &str,
        assistant_text: &str,
        calls: &[ToolCall],
    ) -> Result<ToolBatchId> {
        if let Some(existing) = self.pending_batch_id()? {
            bail!("Cannot begin tool batch: pending batch {existing} exists");
        }

        let created_at = system_time_to_iso8601(SystemTime::now());
        let tx = self
            .db
            .transaction()
            .context("Failed to start tool batch transaction")?;

        tx.execute(
            "INSERT INTO tool_batches (model_name, assistant_text, committed, created_at)
             VALUES (?1, ?2, 0, ?3)",
            params![model_name, assistant_text, created_at],
        )
        .context("Failed to insert tool batch")?;

        let batch_id = tx.last_insert_rowid();

        for (seq, call) in calls.iter().enumerate() {
            let args_json = serde_json::to_string(&call.arguments)
                .context("Failed to serialize tool call arguments")?;
            tx.execute(
                "INSERT INTO tool_calls (batch_id, seq, tool_call_id, tool_name, arguments_json, thought_signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    batch_id,
                    seq as i64,
                    &call.id,
                    &call.name,
                    args_json,
                    call.thought_signature.as_deref()
                ],
            )
            .with_context(|| format!("Failed to insert tool call {}", call.id))?;
        }

        tx.commit()
            .context("Failed to commit tool batch transaction")?;

        Ok(batch_id)
    }

    /// Begin a new tool batch during streaming (before tool arguments are complete).
    ///
    /// Tool calls will be recorded incrementally via `record_call_start` and
    /// `append_call_args`, and assistant text can be updated via
    /// `update_assistant_text`.
    pub fn begin_streaming_batch(&mut self, model_name: &str) -> Result<ToolBatchId> {
        if let Some(existing) = self.pending_batch_id()? {
            bail!("Cannot begin tool batch: pending batch {existing} exists");
        }

        let created_at = system_time_to_iso8601(SystemTime::now());
        let tx = self
            .db
            .transaction()
            .context("Failed to start streaming tool batch transaction")?;

        tx.execute(
            "INSERT INTO tool_batches (model_name, assistant_text, committed, created_at)
             VALUES (?1, ?2, 0, ?3)",
            params![model_name, "", created_at],
        )
        .context("Failed to insert streaming tool batch")?;

        let batch_id = tx.last_insert_rowid();
        tx.commit()
            .context("Failed to commit streaming tool batch transaction")?;

        Ok(batch_id)
    }

    /// Record the start of a tool call in a streaming batch.
    pub fn record_call_start(
        &mut self,
        batch_id: ToolBatchId,
        seq: usize,
        tool_call_id: &str,
        tool_name: &str,
        thought_signature: Option<&str>,
    ) -> Result<()> {
        self.db
            .execute(
                "INSERT INTO tool_calls (batch_id, seq, tool_call_id, tool_name, arguments_json, thought_signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![batch_id, seq as i64, tool_call_id, tool_name, "", thought_signature],
            )
            .with_context(|| format!("Failed to insert tool call {tool_call_id}"))?;
        Ok(())
    }

    /// Append streamed JSON arguments for a tool call.
    pub fn append_call_args(
        &mut self,
        batch_id: ToolBatchId,
        tool_call_id: &str,
        delta: &str,
    ) -> Result<()> {
        let updated = self
            .db
            .execute(
                "UPDATE tool_calls
                 SET arguments_json = arguments_json || ?1
                 WHERE batch_id = ?2 AND tool_call_id = ?3",
                params![delta, batch_id, tool_call_id],
            )
            .with_context(|| format!("Failed to append tool args {tool_call_id}"))?;
        if updated == 0 {
            bail!("No tool call found for id {tool_call_id}");
        }
        Ok(())
    }

    /// Update assistant text for a streaming batch (full replacement).
    ///
    /// Use `append_assistant_delta` instead for streaming deltas to avoid O(n²) rewrites.
    pub fn update_assistant_text(
        &mut self,
        batch_id: ToolBatchId,
        assistant_text: &str,
    ) -> Result<()> {
        let updated = self
            .db
            .execute(
                "UPDATE tool_batches SET assistant_text = ?1 WHERE batch_id = ?2",
                params![assistant_text, batch_id],
            )
            .with_context(|| format!("Failed to update assistant text for batch {batch_id}"))?;
        if updated == 0 {
            bail!("No tool batch found for id {batch_id}");
        }
        Ok(())
    }

    /// Append a text delta to the assistant text for a streaming batch.
    ///
    /// Uses SQL concatenation (`||`) to avoid rewriting the full text on every delta,
    /// improving performance from O(n²) to O(n).
    pub fn append_assistant_delta(&mut self, batch_id: ToolBatchId, delta: &str) -> Result<()> {
        let updated = self
            .db
            .execute(
                "UPDATE tool_batches SET assistant_text = assistant_text || ?1 WHERE batch_id = ?2",
                params![delta, batch_id],
            )
            .with_context(|| format!("Failed to append assistant delta for batch {batch_id}"))?;
        if updated == 0 {
            bail!("No tool batch found for id {batch_id}");
        }
        Ok(())
    }

    /// Record a tool result for a batch.
    pub fn record_result(&mut self, batch_id: ToolBatchId, result: &ToolResult) -> Result<()> {
        let created_at = system_time_to_iso8601(SystemTime::now());
        self.db
            .execute(
                "INSERT INTO tool_results (batch_id, tool_call_id, tool_name, content, is_error, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    batch_id,
                    &result.tool_call_id,
                    &result.tool_name,
                    &result.content,
                    i32::from(result.is_error),
                    created_at,
                ],
            )
            .with_context(|| format!("Failed to record tool result {}", result.tool_call_id))?;
        Ok(())
    }

    /// Commit and prune a completed batch.
    pub fn commit_batch(&mut self, batch_id: ToolBatchId) -> Result<()> {
        let tx = self
            .db
            .transaction()
            .context("Failed to start tool batch commit")?;

        tx.execute(
            "UPDATE tool_batches SET committed = 1 WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to mark tool batch {batch_id} committed"))?;

        tx.execute(
            "DELETE FROM tool_calls WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool calls for batch {batch_id}"))?;

        tx.execute(
            "DELETE FROM tool_results WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool results for batch {batch_id}"))?;

        tx.execute(
            "DELETE FROM tool_batches WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool batch {batch_id}"))?;

        tx.commit().context("Failed to commit tool batch pruning")?;
        Ok(())
    }

    /// Discard an incomplete batch (used on cancel or user discard).
    pub fn discard_batch(&mut self, batch_id: ToolBatchId) -> Result<()> {
        let tx = self
            .db
            .transaction()
            .context("Failed to start tool batch discard")?;

        tx.execute(
            "DELETE FROM tool_calls WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool calls for batch {batch_id}"))?;

        tx.execute(
            "DELETE FROM tool_results WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool results for batch {batch_id}"))?;

        tx.execute(
            "DELETE FROM tool_batches WHERE batch_id = ?1",
            params![batch_id],
        )
        .with_context(|| format!("Failed to delete tool batch {batch_id}"))?;

        tx.commit().context("Failed to commit tool batch discard")?;
        Ok(())
    }

    /// Recover the most recent incomplete tool batch, if any.
    pub fn recover(&self) -> Result<Option<RecoveredToolBatch>> {
        // Maximum args size to parse during recovery (prevents OOM on corrupted/malicious data)
        const RECOVERY_MAX_ARGS_BYTES: usize = 1024 * 1024; // 1MB

        let batch_id: Option<ToolBatchId> = self
            .db
            .query_row(
                "SELECT batch_id FROM tool_batches WHERE committed = 0 ORDER BY batch_id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query pending tool batch")?;

        let Some(batch_id) = batch_id else {
            return Ok(None);
        };

        let (model_name, assistant_text): (String, String) = self
            .db
            .query_row(
                "SELECT model_name, assistant_text FROM tool_batches WHERE batch_id = ?1",
                params![batch_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("Failed to load tool batch metadata")?;

        let mut calls: Vec<ToolCall> = Vec::new();
        let mut stmt = self
            .db
            .prepare(
                "SELECT tool_call_id, tool_name, arguments_json, thought_signature
                 FROM tool_calls WHERE batch_id = ?1 ORDER BY seq ASC",
            )
            .context("Failed to prepare tool calls query")?;
        let rows = stmt
            .query_map(params![batch_id], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let args_json: String = row.get(2)?;
                let thought_signature: Option<String> = row.get(3)?;
                Ok((id, name, args_json, thought_signature))
            })
            .context("Failed to query tool calls")?;

        for row in rows {
            let (id, name, args_json, thought_signature) = row?;
            let args = if args_json.trim().is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else if args_json.len() > RECOVERY_MAX_ARGS_BYTES {
                // Skip parsing excessively large arguments
                tracing::warn!(
                    "Tool call {} has oversized arguments ({} bytes), using empty object",
                    id,
                    args_json.len()
                );
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                match serde_json::from_str(&args_json) {
                    Ok(value) => value,
                    Err(_) => serde_json::Value::Object(serde_json::Map::new()),
                }
            };
            calls.push(ToolCall::new_with_thought_signature(
                id,
                name,
                args,
                thought_signature,
            ));
        }

        let mut results: Vec<ToolResult> = Vec::new();
        let mut stmt = self
            .db
            .prepare(
                "SELECT tool_call_id, tool_name, content, is_error
                 FROM tool_results WHERE batch_id = ?1",
            )
            .context("Failed to prepare tool results query")?;
        let rows = stmt
            .query_map(params![batch_id], |row| {
                let id: String = row.get(0)?;
                let tool_name: String = row.get(1)?;
                let content: String = row.get(2)?;
                let is_error: i32 = row.get(3)?;
                Ok((id, tool_name, content, is_error != 0))
            })
            .context("Failed to query tool results")?;

        for row in rows {
            let (id, tool_name, content, is_error) = row?;
            let result = if is_error {
                ToolResult::error(id, tool_name, content)
            } else {
                ToolResult::success(id, tool_name, content)
            };
            results.push(result);
        }

        Ok(Some(RecoveredToolBatch {
            batch_id,
            model_name,
            assistant_text,
            calls,
            results,
        }))
    }

    fn pending_batch_id(&self) -> Result<Option<ToolBatchId>> {
        let batch_id: Option<ToolBatchId> = self
            .db
            .query_row(
                "SELECT batch_id FROM tool_batches WHERE committed = 0 ORDER BY batch_id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query pending tool batch")?;
        Ok(batch_id)
    }
}

fn ensure_tool_calls_signature(db: &Connection) -> Result<()> {
    if tool_calls_has_signature(db)? {
        return Ok(());
    }
    db.execute(
        "ALTER TABLE tool_calls ADD COLUMN thought_signature TEXT",
        [],
    )
    .context("Failed to add thought_signature column to tool_calls")?;
    Ok(())
}

fn tool_calls_has_signature(db: &Connection) -> Result<bool> {
    let mut stmt = db
        .prepare("PRAGMA table_info(tool_calls)")
        .context("Failed to inspect tool_calls schema")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("Failed to query tool_calls columns")?;
    for name in rows {
        if name? == "thought_signature" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Migration: add tool_name column to tool_results table for existing databases.
fn ensure_tool_results_name(db: &Connection) -> Result<()> {
    if tool_results_has_name(db)? {
        return Ok(());
    }
    // Add the column with a default value for existing rows
    db.execute(
        "ALTER TABLE tool_results ADD COLUMN tool_name TEXT NOT NULL DEFAULT ''",
        [],
    )
    .context("Failed to add tool_name column to tool_results")?;
    Ok(())
}

fn tool_results_has_name(db: &Connection) -> Result<bool> {
    let mut stmt = db
        .prepare("PRAGMA table_info(tool_results)")
        .context("Failed to inspect tool_results schema")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("Failed to query tool_results columns")?;
    for name in rows {
        if name? == "tool_name" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn system_time_to_iso8601(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    chrono_lite_format(secs, millis)
}

fn chrono_lite_format(secs: u64, millis: u32) -> String {
    const SECS_PER_DAY: u64 = 86400;
    const SECS_PER_HOUR: u64 = 3600;
    const SECS_PER_MINUTE: u64 = 60;

    let days = secs / SECS_PER_DAY;
    let remaining = secs % SECS_PER_DAY;

    let hours = remaining / SECS_PER_HOUR;
    let remaining = remaining % SECS_PER_HOUR;

    let minutes = remaining / SECS_PER_MINUTE;
    let seconds = remaining % SECS_PER_MINUTE;

    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

fn ensure_secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Failed to read directory metadata: {}", path.display()))?;

        // Only modify permissions if we own the directory
        let our_uid = unsafe { libc::getuid() };
        if metadata.uid() != our_uid {
            // Not our directory - skip silently (e.g., /tmp)
            return Ok(());
        }

        // Check if permissions are already secure (0o700 or stricter)
        let current_mode = metadata.permissions().mode() & 0o777;
        if current_mode & 0o077 != 0 {
            // Group or other has some access - tighten to 0o700
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).with_context(
                || format!("Failed to set directory permissions: {}", path.display()),
            )?;
        }
    }
    Ok(())
}

fn ensure_secure_db_files(path: &Path) -> Result<()> {
    if !path.exists() {
        let _file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("Failed to create database file: {}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set database permissions: {}", path.display()))?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = sqlite_sidecar_path(path, suffix);
            if sidecar.exists() {
                let _ = std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sqlite_sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let file_name = path.file_name().map(|name| name.to_string_lossy());
    match file_name {
        Some(name) => path.with_file_name(format!("{name}{suffix}")),
        None => std::path::PathBuf::from(format!("{}{suffix}", path.display())),
    }
}

fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    (year as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begins_and_recovers_batch() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "Read",
            serde_json::json!({"path": "foo"}),
        )];
        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.batch_id, batch_id);
        assert_eq!(recovered.calls.len(), 1);
        assert_eq!(recovered.calls[0].name, "Read");
    }

    #[test]
    fn records_and_commits_results() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "Read",
            serde_json::json!({"path": "foo"}),
        )];
        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        let result = ToolResult::success("1", "Read", "ok");
        journal.record_result(batch_id, &result).unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.results.len(), 1);
        assert_eq!(recovered.results[0].tool_name, "Read");

        journal.commit_batch(batch_id).unwrap();
        assert!(journal.recover().unwrap().is_none());
    }

    #[test]
    fn begin_batch_fails_when_pending_exists() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new("1", "test", serde_json::json!({}))];

        // First batch succeeds
        let _batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        // Second batch should fail
        let result = journal.begin_batch("test-model", "another", &calls);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pending batch"));
    }

    #[test]
    fn streaming_batch_workflow() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        // Begin streaming batch
        let batch_id = journal.begin_streaming_batch("test-model").unwrap();

        // Record tool call start
        journal
            .record_call_start(batch_id, 0, "call_1", "Read", None)
            .unwrap();

        // Append arguments in chunks
        journal
            .append_call_args(batch_id, "call_1", r#"{"path":"#)
            .unwrap();
        journal
            .append_call_args(batch_id, "call_1", r#""foo.txt"}"#)
            .unwrap();

        // Update assistant text
        journal
            .update_assistant_text(batch_id, "Let me read the file")
            .unwrap();

        // Recover and verify
        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.batch_id, batch_id);
        assert_eq!(recovered.model_name, "test-model");
        assert_eq!(recovered.assistant_text, "Let me read the file");
        assert_eq!(recovered.calls.len(), 1);
        assert_eq!(recovered.calls[0].name, "Read");
        assert_eq!(recovered.calls[0].arguments["path"], "foo.txt");
    }

    #[test]
    fn discard_batch_removes_all_data() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new("1", "test", serde_json::json!({}))];

        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        // Add a result
        let result = ToolResult::success("1", "test", "done");
        journal.record_result(batch_id, &result).unwrap();

        // Discard the batch
        journal.discard_batch(batch_id).unwrap();

        // Should be no pending batches
        assert!(journal.recover().unwrap().is_none());
    }

    #[test]
    fn records_error_result() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new("1", "test", serde_json::json!({}))];

        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        // Record an error result
        let result = ToolResult::error("1", "test", "Something went wrong");
        journal.record_result(batch_id, &result).unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.results.len(), 1);
        assert!(recovered.results[0].is_error);
        assert_eq!(recovered.results[0].tool_name, "test");
        assert_eq!(recovered.results[0].content, "Something went wrong");
    }

    #[test]
    fn append_call_args_fails_for_unknown_call() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        let batch_id = journal.begin_streaming_batch("test-model").unwrap();

        // Try to append to non-existent call
        let result = journal.append_call_args(batch_id, "nonexistent", "data");
        assert!(result.is_err());
    }

    #[test]
    fn update_assistant_text_fails_for_unknown_batch() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        // Try to update a non-existent batch
        let result = journal.update_assistant_text(999, "text");
        assert!(result.is_err());
    }

    #[test]
    fn recover_handles_empty_arguments_json() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        let batch_id = journal.begin_streaming_batch("test-model").unwrap();
        journal
            .record_call_start(batch_id, 0, "call_1", "test_tool", None)
            .unwrap();
        // Don't append any arguments - leave it empty

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.calls.len(), 1);
        // Empty args should become empty object
        assert!(recovered.calls[0].arguments.is_object());
    }

    #[test]
    fn recover_handles_invalid_arguments_json() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        let batch_id = journal.begin_streaming_batch("test-model").unwrap();
        journal
            .record_call_start(batch_id, 0, "call_1", "test_tool", None)
            .unwrap();
        // Append invalid JSON
        journal
            .append_call_args(batch_id, "call_1", "not valid json {{{")
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        // Invalid JSON should become empty object
        assert!(recovered.calls[0].arguments.is_object());
    }
}

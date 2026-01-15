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
            PRIMARY KEY (batch_id, seq)
        );

        CREATE TABLE IF NOT EXISTS tool_results (
            batch_id INTEGER NOT NULL,
            tool_call_id TEXT NOT NULL,
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
                "INSERT INTO tool_calls (batch_id, seq, tool_call_id, tool_name, arguments_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![batch_id, seq as i64, &call.id, &call.name, args_json],
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
    ) -> Result<()> {
        self.db
            .execute(
                "INSERT INTO tool_calls (batch_id, seq, tool_call_id, tool_name, arguments_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![batch_id, seq as i64, tool_call_id, tool_name, ""],
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

    /// Update assistant text for a streaming batch.
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

    /// Record a tool result for a batch.
    pub fn record_result(&mut self, batch_id: ToolBatchId, result: &ToolResult) -> Result<()> {
        let created_at = system_time_to_iso8601(SystemTime::now());
        self.db
            .execute(
                "INSERT INTO tool_results (batch_id, tool_call_id, content, is_error, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    batch_id,
                    &result.tool_call_id,
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
                "SELECT tool_call_id, tool_name, arguments_json
                 FROM tool_calls WHERE batch_id = ?1 ORDER BY seq ASC",
            )
            .context("Failed to prepare tool calls query")?;
        let rows = stmt
            .query_map(params![batch_id], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let args_json: String = row.get(2)?;
                Ok((id, name, args_json))
            })
            .context("Failed to query tool calls")?;

        for row in rows {
            let (id, name, args_json) = row?;
            let args = if args_json.trim().is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                match serde_json::from_str(&args_json) {
                    Ok(value) => value,
                    Err(_) => serde_json::Value::Object(serde_json::Map::new()),
                }
            };
            calls.push(ToolCall::new(id, name, args));
        }

        let mut results: Vec<ToolResult> = Vec::new();
        let mut stmt = self
            .db
            .prepare(
                "SELECT tool_call_id, content, is_error
                 FROM tool_results WHERE batch_id = ?1",
            )
            .context("Failed to prepare tool results query")?;
        let rows = stmt
            .query_map(params![batch_id], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let is_error: i32 = row.get(2)?;
                Ok((id, content, is_error != 0))
            })
            .context("Failed to query tool results")?;

        for row in rows {
            let (id, content, is_error) = row?;
            let result = if is_error {
                ToolResult::error(id, content)
            } else {
                ToolResult::success(id, content)
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
            "read_file",
            serde_json::json!({"path": "foo"}),
        )];
        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.batch_id, batch_id);
        assert_eq!(recovered.calls.len(), 1);
        assert_eq!(recovered.calls[0].name, "read_file");
    }

    #[test]
    fn records_and_commits_results() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "read_file",
            serde_json::json!({"path": "foo"}),
        )];
        let batch_id = journal
            .begin_batch("test-model", "assistant", &calls)
            .unwrap();

        let result = ToolResult::success("1", "ok");
        journal.record_result(batch_id, &result).unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.results.len(), 1);

        journal.commit_batch(batch_id).unwrap();
        assert!(journal.recover().unwrap().is_none());
    }
}

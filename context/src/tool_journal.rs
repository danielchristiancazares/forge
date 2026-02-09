//! Tool Journal - Durable tool batch tracking for crash recovery.
//!
//! This module provides SQLite-backed persistence for tool execution batches,
//! enabling recovery of interrupted tool operations after crashes.
//!
//! # Design Constraints
//!
//! - **Single pending batch**: Only one uncommitted batch can exist at a time
//! - **Journal-before-execute**: Tool calls are persisted before execution begins
//! - **Atomic commit**: Batch commit and cleanup occur in a single transaction
//!
//! # Streaming Support
//!
//! For tool calls created during streaming responses (before arguments are complete),
//! use the streaming batch workflow:
//!
//! 1. [`ToolJournal::begin_streaming_batch`] - Create batch before tool calls arrive
//! 2. [`ToolJournal::record_call_start`] - Record each tool call as it begins
//! 3. [`ToolJournal::append_call_args`] - Append argument chunks as they stream
//! 4. [`ToolJournal::append_assistant_delta`] - Append assistant text deltas
//! 5. [`ToolJournal::record_result`] - Record results as tools complete
//! 6. [`ToolJournal::commit_batch`] - Commit when all tools finish
//!
//! # Recovery
//!
//! On startup, call [`ToolJournal::recover`] to check for incomplete batches.
//! The [`RecoveredToolBatch`] contains all persisted state, allowing the user
//! to resume execution or discard the batch.

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::OpenOptions;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::StepId;
use forge_types::{ThoughtSignature, ThoughtSignatureState, ToolCall, ToolResult};

/// Unique identifier for a tool batch.
pub type ToolBatchId = i64;

/// Per-tool-call execution metadata captured for crash recovery.
///
/// This data is best-effort and may be partially populated depending on the
/// tool type and platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveredToolCallExecution {
    /// When Forge began executing the tool call (Unix epoch milliseconds).
    pub started_at_unix_ms: Option<i64>,
    /// OS process id for subprocess-backed tools (e.g., `Run`) when available.
    pub process_id: Option<i64>,
    /// Process creation timestamp (Unix epoch milliseconds) when available.
    ///
    /// Used to reduce PID reuse risk when attempting recovery cleanup.
    pub process_started_at_unix_ms: Option<i64>,
}

/// Information about corrupted tool call arguments during recovery.
#[derive(Debug, Clone)]
pub struct CorruptedToolArgs {
    pub tool_call_id: String,
    pub raw_json: String,
    pub parse_error: String,
}

/// Recovered tool batch data after a crash.
#[derive(Debug, Clone)]
pub struct RecoveredToolBatch {
    pub batch_id: ToolBatchId,
    pub stream_step_id: Option<StepId>,
    pub model_name: String,
    pub assistant_text: String,
    pub calls: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
    /// Tool calls whose arguments failed to parse (substituted with {})
    pub corrupted_args: Vec<CorruptedToolArgs>,
    /// Best-effort execution metadata keyed by tool_call_id.
    pub call_execution: std::collections::HashMap<String, RecoveredToolCallExecution>,
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
            stream_step_id INTEGER,
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
            started_at_unix_ms INTEGER,
            process_id INTEGER,
            process_started_at_unix_ms INTEGER,
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
        ensure_tool_batches_step_id(&db)?;
        ensure_tool_calls_signature(&db)?;
        ensure_tool_calls_execution_metadata(&db)?;
        ensure_tool_results_name(&db)?;
        Ok(Self { db })
    }

    /// Begin a new tool batch and persist its tool calls.
    ///
    /// Returns the new batch ID.
    pub fn begin_batch(
        &mut self,
        stream_step_id: StepId,
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
            "INSERT INTO tool_batches (stream_step_id, model_name, assistant_text, committed, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![stream_step_id, model_name, assistant_text, created_at],
        )
        .context("Failed to insert tool batch")?;

        let batch_id = tx.last_insert_rowid();

        for (seq, call) in calls.iter().enumerate() {
            let args_json = serde_json::to_string(&call.arguments)
                .context("Failed to serialize tool call arguments")?;
            let thought_signature = match call.signature_state() {
                ThoughtSignatureState::Signed(signature) => Some(signature.as_str()),
                ThoughtSignatureState::Unsigned => None,
            };
            tx.execute(
                "INSERT INTO tool_calls (batch_id, seq, tool_call_id, tool_name, arguments_json, thought_signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    batch_id,
                    seq as i64,
                    &call.id,
                    &call.name,
                    args_json,
                    thought_signature
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
    pub fn begin_streaming_batch(
        &mut self,
        stream_step_id: StepId,
        model_name: &str,
    ) -> Result<ToolBatchId> {
        if let Some(existing) = self.pending_batch_id()? {
            bail!("Cannot begin tool batch: pending batch {existing} exists");
        }

        let created_at = system_time_to_iso8601(SystemTime::now());
        let tx = self
            .db
            .transaction()
            .context("Failed to start streaming tool batch transaction")?;

        tx.execute(
            "INSERT INTO tool_batches (stream_step_id, model_name, assistant_text, committed, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![stream_step_id, model_name, "", created_at],
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
        thought_signature: &ThoughtSignatureState,
    ) -> Result<()> {
        let thought_signature = match thought_signature {
            ThoughtSignatureState::Signed(signature) => Some(signature.as_str()),
            ThoughtSignatureState::Unsigned => None,
        };
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

    /// Append streamed JSON argument deltas for multiple tool calls in a single transaction.
    ///
    /// This is a performance optimization for streaming providers that emit many tiny
    /// argument chunks: buffering in the engine and flushing here reduces SQLite write
    /// frequency and improves UI responsiveness.
    pub fn append_call_args_batch(
        &mut self,
        batch_id: ToolBatchId,
        deltas: Vec<(String, String)>,
    ) -> Result<()> {
        if deltas.is_empty() {
            return Ok(());
        }

        let tx = self
            .db
            .transaction()
            .context("Failed to start tool args append transaction")?;

        for (tool_call_id, delta) in deltas {
            let updated = tx
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
        }

        tx.commit()
            .context("Failed to commit tool args append transaction")?;
        Ok(())
    }

    /// Mark a tool call as started (durable "journal-before-execute" metadata).
    ///
    /// This update is idempotent: if the call was already marked started, the existing
    /// timestamp is preserved.
    pub fn mark_call_started(
        &mut self,
        batch_id: ToolBatchId,
        tool_call_id: &str,
        started_at_unix_ms: i64,
    ) -> Result<()> {
        let updated = self
            .db
            .execute(
                "UPDATE tool_calls
                 SET started_at_unix_ms = COALESCE(started_at_unix_ms, ?1)
                 WHERE batch_id = ?2 AND tool_call_id = ?3",
                params![started_at_unix_ms, batch_id, tool_call_id],
            )
            .with_context(|| format!("Failed to mark tool call started {tool_call_id}"))?;
        if updated == 0 {
            bail!("No tool call found for id {tool_call_id}");
        }
        Ok(())
    }

    /// Record subprocess metadata for a tool call (e.g., `Run` PID and creation time).
    ///
    /// This is idempotent: if the metadata was already recorded with identical values,
    /// this is a no-op. If conflicting metadata exists, this returns an error.
    pub fn record_call_process(
        &mut self,
        batch_id: ToolBatchId,
        tool_call_id: &str,
        process_id: i64,
        process_started_at_unix_ms: Option<i64>,
    ) -> Result<()> {
        let (existing_pid, existing_started_at): (Option<i64>, Option<i64>) = self
            .db
            .query_row(
                "SELECT process_id, process_started_at_unix_ms
                 FROM tool_calls
                 WHERE batch_id = ?1 AND tool_call_id = ?2",
                params![batch_id, tool_call_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .with_context(|| format!("Failed to load tool call {tool_call_id} for PID update"))?;

        let start_time_matches = match process_started_at_unix_ms {
            None => true,
            Some(value) => existing_started_at == Some(value),
        };
        if existing_pid == Some(process_id) && start_time_matches {
            return Ok(());
        }
        if existing_pid.is_some() && existing_pid != Some(process_id) {
            bail!("Tool call {tool_call_id} already has a different recorded PID");
        }
        if existing_started_at.is_some()
            && process_started_at_unix_ms.is_some()
            && existing_started_at != process_started_at_unix_ms
        {
            bail!("Tool call {tool_call_id} already has a different recorded process start time");
        }

        let updated = self
            .db
            .execute(
                "UPDATE tool_calls
                 SET process_id = COALESCE(process_id, ?1),
                     process_started_at_unix_ms = COALESCE(process_started_at_unix_ms, ?2)
                 WHERE batch_id = ?3 AND tool_call_id = ?4",
                params![
                    process_id,
                    process_started_at_unix_ms,
                    batch_id,
                    tool_call_id
                ],
            )
            .with_context(|| format!("Failed to record tool call PID metadata {tool_call_id}"))?;
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
        let inserted = self
            .db
            .execute(
                "INSERT OR IGNORE INTO tool_results (batch_id, tool_call_id, tool_name, content, is_error, created_at)
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

        if inserted == 1 {
            return Ok(());
        }
        if inserted != 0 {
            bail!("Unexpected insert count when recording tool result");
        }

        let (existing_tool_name, existing_content, existing_is_error): (String, String, i32) = self
            .db
            .query_row(
                "SELECT tool_name, content, is_error
                     FROM tool_results
                     WHERE batch_id = ?1 AND tool_call_id = ?2",
                params![batch_id, &result.tool_call_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .with_context(|| {
                format!(
                    "Failed to load existing tool result {} for idempotency check",
                    result.tool_call_id
                )
            })?;

        let tool_name_matches =
            existing_tool_name.is_empty() || existing_tool_name == result.tool_name;
        let content_matches = existing_content == result.content;
        let is_error_matches = existing_is_error == i32::from(result.is_error);

        if tool_name_matches && content_matches && is_error_matches {
            return Ok(());
        }

        bail!(
            "Tool result {} already recorded with different content",
            result.tool_call_id
        )
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

        let (stream_step_id, model_name, assistant_text): (Option<StepId>, String, String) = self
            .db
            .query_row(
                "SELECT stream_step_id, model_name, assistant_text FROM tool_batches WHERE batch_id = ?1",
                params![batch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("Failed to load tool batch metadata")?;

        let mut calls: Vec<ToolCall> = Vec::new();
        let mut call_execution: std::collections::HashMap<String, RecoveredToolCallExecution> =
            std::collections::HashMap::new();
        let mut corrupted_args: Vec<CorruptedToolArgs> = Vec::new();
        let mut stmt = self
            .db
            .prepare(
                "SELECT tool_call_id, tool_name, arguments_json, thought_signature,
                        started_at_unix_ms, process_id, process_started_at_unix_ms
                 FROM tool_calls WHERE batch_id = ?1 ORDER BY seq ASC",
            )
            .context("Failed to prepare tool calls query")?;
        let rows = stmt
            .query_map(params![batch_id], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let args_json: String = row.get(2)?;
                let thought_signature: Option<String> = row.get(3)?;
                let started_at_unix_ms: Option<i64> = row.get(4)?;
                let process_id: Option<i64> = row.get(5)?;
                let process_started_at_unix_ms: Option<i64> = row.get(6)?;
                Ok((
                    id,
                    name,
                    args_json,
                    thought_signature,
                    started_at_unix_ms,
                    process_id,
                    process_started_at_unix_ms,
                ))
            })
            .context("Failed to query tool calls")?;

        for row in rows {
            let (
                id,
                name,
                args_json,
                thought_signature,
                started_at_unix_ms,
                process_id,
                process_started_at_unix_ms,
            ) = row?;
            let args = if args_json.trim().is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else if args_json.len() > RECOVERY_MAX_ARGS_BYTES {
                tracing::warn!(
                    "Tool call {} has oversized arguments ({} bytes), using empty object",
                    id,
                    args_json.len()
                );
                corrupted_args.push(CorruptedToolArgs {
                    tool_call_id: id.clone(),
                    raw_json: format!("[{} bytes, truncated]", args_json.len()),
                    parse_error: "oversized".to_string(),
                });
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                match serde_json::from_str(&args_json) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(
                            "Tool call {} has invalid JSON arguments: {}, using empty object",
                            id,
                            err
                        );
                        corrupted_args.push(CorruptedToolArgs {
                            tool_call_id: id.clone(),
                            raw_json: args_json.clone(),
                            parse_error: err.to_string(),
                        });
                        serde_json::Value::Object(serde_json::Map::new())
                    }
                }
            };
            let signature_state = match thought_signature {
                Some(signature) if !signature.is_empty() => {
                    ThoughtSignatureState::Signed(ThoughtSignature::new(signature))
                }
                _ => ThoughtSignatureState::Unsigned,
            };
            calls.push(match signature_state {
                ThoughtSignatureState::Signed(signature) => {
                    ToolCall::new_signed(id, name, args, signature)
                }
                ThoughtSignatureState::Unsigned => ToolCall::new(id, name, args),
            });

            let tool_call_id = calls.last().expect("pushed call above").id.clone();
            call_execution.insert(
                tool_call_id,
                RecoveredToolCallExecution {
                    started_at_unix_ms,
                    process_id,
                    process_started_at_unix_ms,
                },
            );
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
            stream_step_id,
            model_name,
            assistant_text,
            calls,
            results,
            corrupted_args,
            call_execution,
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

fn ensure_tool_calls_execution_metadata(db: &Connection) -> Result<()> {
    ensure_tool_calls_started_at(db)?;
    ensure_tool_calls_process_id(db)?;
    ensure_tool_calls_process_started_at(db)?;
    Ok(())
}

fn ensure_tool_calls_started_at(db: &Connection) -> Result<()> {
    if tool_calls_has_started_at(db)? {
        return Ok(());
    }
    db.execute(
        "ALTER TABLE tool_calls ADD COLUMN started_at_unix_ms INTEGER",
        [],
    )
    .context("Failed to add started_at_unix_ms column to tool_calls")?;
    Ok(())
}

fn ensure_tool_calls_process_id(db: &Connection) -> Result<()> {
    if tool_calls_has_process_id(db)? {
        return Ok(());
    }
    db.execute("ALTER TABLE tool_calls ADD COLUMN process_id INTEGER", [])
        .context("Failed to add process_id column to tool_calls")?;
    Ok(())
}

fn ensure_tool_calls_process_started_at(db: &Connection) -> Result<()> {
    if tool_calls_has_process_started_at(db)? {
        return Ok(());
    }
    db.execute(
        "ALTER TABLE tool_calls ADD COLUMN process_started_at_unix_ms INTEGER",
        [],
    )
    .context("Failed to add process_started_at_unix_ms column to tool_calls")?;
    Ok(())
}

fn ensure_tool_batches_step_id(db: &Connection) -> Result<()> {
    if tool_batches_has_step_id(db)? {
        return Ok(());
    }
    db.execute(
        "ALTER TABLE tool_batches ADD COLUMN stream_step_id INTEGER",
        [],
    )
    .context("Failed to add stream_step_id column to tool_batches")?;
    Ok(())
}

fn tool_batches_has_step_id(db: &Connection) -> Result<bool> {
    let mut stmt = db
        .prepare("PRAGMA table_info(tool_batches)")
        .context("Failed to inspect tool_batches schema")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("Failed to query tool_batches columns")?;
    for name in rows {
        if name? == "stream_step_id" {
            return Ok(true);
        }
    }
    Ok(false)
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

fn tool_calls_has_started_at(db: &Connection) -> Result<bool> {
    tool_calls_has_column(db, "started_at_unix_ms")
}

fn tool_calls_has_process_id(db: &Connection) -> Result<bool> {
    tool_calls_has_column(db, "process_id")
}

fn tool_calls_has_process_started_at(db: &Connection) -> Result<bool> {
    tool_calls_has_column(db, "process_started_at_unix_ms")
}

fn tool_calls_has_column(db: &Connection, column: &str) -> Result<bool> {
    let mut stmt = db
        .prepare("PRAGMA table_info(tool_calls)")
        .context("Failed to inspect tool_calls schema")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("Failed to query tool_calls columns")?;
    for name in rows {
        if name? == column {
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
        // Use atomic mode setting on Unix to avoid TOCTOU race between create and chmod
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("Failed to create database file: {}", path.display()))?;
        }
        #[cfg(not(unix))]
        {
            let _file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("Failed to create database file: {}", path.display()))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Ensure permissions are correct even for pre-existing files
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
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.batch_id, batch_id);
        assert_eq!(recovered.calls.len(), 1);
        assert_eq!(recovered.calls[0].name, "Read");
        let exec = recovered
            .call_execution
            .get("1")
            .expect("execution metadata keyed by tool_call_id");
        assert_eq!(
            *exec,
            RecoveredToolCallExecution {
                started_at_unix_ms: None,
                process_id: None,
                process_started_at_unix_ms: None,
            }
        );
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
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
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
    fn record_result_is_idempotent() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "Read",
            serde_json::json!({ "path": "foo" }),
        )];
        let batch_id = journal
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
            .unwrap();

        let result = ToolResult::success("1", "Read", "ok");
        journal.record_result(batch_id, &result).unwrap();
        journal.record_result(batch_id, &result).unwrap();
    }

    #[test]
    fn record_result_errors_on_mismatch() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "Read",
            serde_json::json!({ "path": "foo" }),
        )];
        let batch_id = journal
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
            .unwrap();

        journal
            .record_result(batch_id, &ToolResult::success("1", "Read", "ok"))
            .unwrap();

        let err = journal.record_result(
            batch_id,
            &ToolResult::success("1", "Read", "different content"),
        );
        assert!(err.is_err());
    }

    #[test]
    fn begin_batch_fails_when_pending_exists() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new("1", "test", serde_json::json!({}))];

        // First batch succeeds
        let _batch_id = journal
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
            .unwrap();

        // Second batch should fail
        let result = journal.begin_batch(StepId::new(2), "test-model", "another", &calls);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pending batch"));
    }

    #[test]
    fn streaming_batch_workflow() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        // Begin streaming batch
        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();

        // Record tool call start
        journal
            .record_call_start(
                batch_id,
                0,
                "call_1",
                "Read",
                &ThoughtSignatureState::Unsigned,
            )
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
    fn tool_call_start_metadata_round_trips() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new(
            "1",
            "Run",
            serde_json::json!({ "command": "echo hi" }),
        )];
        let batch_id = journal
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
            .unwrap();

        journal
            .mark_call_started(batch_id, "1", 1_700_000_000_000)
            .unwrap();
        journal
            .record_call_process(batch_id, "1", 4242, Some(1_700_000_000_123))
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        let exec = recovered
            .call_execution
            .get("1")
            .expect("execution metadata keyed by tool_call_id");
        assert_eq!(
            *exec,
            RecoveredToolCallExecution {
                started_at_unix_ms: Some(1_700_000_000_000),
                process_id: Some(4242),
                process_started_at_unix_ms: Some(1_700_000_000_123),
            }
        );
    }

    #[test]
    fn append_call_args_batch_appends_multiple_calls_in_one_txn() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();
        journal
            .record_call_start(
                batch_id,
                0,
                "call_1",
                "Read",
                &ThoughtSignatureState::Unsigned,
            )
            .unwrap();
        journal
            .record_call_start(
                batch_id,
                1,
                "call_2",
                "Read",
                &ThoughtSignatureState::Unsigned,
            )
            .unwrap();

        journal
            .append_call_args_batch(
                batch_id,
                vec![
                    ("call_1".to_string(), r#"{"path":"a.txt"}"#.to_string()),
                    ("call_2".to_string(), r#"{"path":"b.txt"}"#.to_string()),
                ],
            )
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.calls.len(), 2);
        assert_eq!(recovered.calls[0].arguments["path"], "a.txt");
        assert_eq!(recovered.calls[1].arguments["path"], "b.txt");
    }

    #[test]
    fn discard_batch_removes_all_data() {
        let mut journal = ToolJournal::open_in_memory().unwrap();
        let calls = vec![ToolCall::new("1", "test", serde_json::json!({}))];

        let batch_id = journal
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
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
            .begin_batch(StepId::new(1), "test-model", "assistant", &calls)
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

        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();

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

        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();
        journal
            .record_call_start(
                batch_id,
                0,
                "call_1",
                "test_tool",
                &ThoughtSignatureState::Unsigned,
            )
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

        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();
        journal
            .record_call_start(
                batch_id,
                0,
                "call_1",
                "test_tool",
                &ThoughtSignatureState::Unsigned,
            )
            .unwrap();
        // Append invalid JSON
        journal
            .append_call_args(batch_id, "call_1", "not valid json {{{")
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        // Invalid JSON should become empty object
        assert!(recovered.calls[0].arguments.is_object());
    }

    #[test]
    fn recover_deserializes_escaped_arguments_json() {
        let mut journal = ToolJournal::open_in_memory().unwrap();

        let batch_id = journal
            .begin_streaming_batch(StepId::new(1), "test-model")
            .unwrap();
        journal
            .record_call_start(
                batch_id,
                0,
                "call_1",
                "test_tool",
                &ThoughtSignatureState::Unsigned,
            )
            .unwrap();
        journal
            .append_call_args(batch_id, "call_1", r#"{"unicode":"\u26"#)
            .unwrap();
        journal
            .append_call_args(
                batch_id,
                "call_1",
                r#"3A","path":"https:\/\/example.com\/x","msg":"slash\/ok"}"#,
            )
            .unwrap();

        let recovered = journal.recover().unwrap().expect("should recover");
        assert_eq!(recovered.calls.len(), 1);
        assert_eq!(recovered.calls[0].arguments["unicode"], "\u{263A}");
        assert_eq!(
            recovered.calls[0].arguments["path"],
            "https://example.com/x"
        );
        assert_eq!(recovered.calls[0].arguments["msg"], "slash/ok");
    }
}

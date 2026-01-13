//! Stream Journal - Streaming durability system for crash recovery
//!
//! This module ensures that streaming deltas are persisted to `SQLite` BEFORE
//! being displayed to the user. This guarantees that after a crash, we can
//! recover the partial response and resume or replay.
//!
//! # Key Invariant
//!
//! **Deltas MUST be persisted BEFORE being displayed to the user.**
//!
//! This write-ahead logging approach ensures durability at the cost of
//! slightly higher latency per delta.
//!
//! # Performance Consideration
//!
//! Currently, `SQLite` writes are synchronous and run in the async UI loop.
//! For high-frequency deltas on slow disks, this could cause UI stutter.
//! Future optimization: move journaling to a dedicated thread with channel-based
//! event submission, batching commits per N deltas or time interval.

#[cfg(test)]
use anyhow::anyhow;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Unique identifier for a streaming step/session
pub type StepId = i64;

/// A single delta event in a streaming response.
///
/// This makes invalid deltas unrepresentable (e.g., a "done" event with text content).
#[derive(Clone, Debug)]
enum StreamDeltaEvent {
    TextDelta(String),
    Done,
    Error(String),
}

impl StreamDeltaEvent {
    const fn event_type(&self) -> &'static str {
        match self {
            StreamDeltaEvent::TextDelta(_) => "text_delta",
            StreamDeltaEvent::Done => "done",
            StreamDeltaEvent::Error(_) => "error",
        }
    }

    fn content(&self) -> &str {
        match self {
            StreamDeltaEvent::TextDelta(text) | StreamDeltaEvent::Error(text) => text,
            StreamDeltaEvent::Done => "",
        }
    }
}

#[derive(Clone, Debug)]
struct StreamDelta {
    /// The step this delta belongs to
    step_id: StepId,
    /// Sequence number within the step (monotonically increasing)
    seq: u64,
    event: StreamDeltaEvent,
    /// When this delta was created
    timestamp: SystemTime,
}

impl StreamDelta {
    fn new(step_id: StepId, seq: u64, event: StreamDeltaEvent) -> Self {
        Self {
            step_id,
            seq,
            event,
            timestamp: SystemTime::now(),
        }
    }
}

/// Active streaming journal.
///
/// Possessing this type is the proof that a stream is in-flight.
#[derive(Debug)]
pub struct ActiveJournal {
    journal_id: u64,
    step_id: StepId,
    next_seq: u64,
    model_name: String,
}

impl ActiveJournal {
    #[must_use] 
    pub fn step_id(&self) -> StepId {
        self.step_id
    }

    /// Get the model name associated with this streaming session.
    #[must_use] 
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn append_text(
        &mut self,
        journal: &mut StreamJournal,
        content: impl Into<String>,
    ) -> Result<()> {
        journal.append_event(self, StreamDeltaEvent::TextDelta(content.into()))
    }

    pub fn append_done(&mut self, journal: &mut StreamJournal) -> Result<()> {
        journal.append_event(self, StreamDeltaEvent::Done)
    }

    pub fn append_error(
        &mut self,
        journal: &mut StreamJournal,
        message: impl Into<String>,
    ) -> Result<()> {
        journal.append_event(self, StreamDeltaEvent::Error(message.into()))
    }

    pub fn seal(self, journal: &mut StreamJournal) -> Result<String> {
        journal.seal_active(self)
    }

    pub fn discard(self, journal: &mut StreamJournal) -> Result<u64> {
        journal.discard_active(self)
    }
}

/// Recovered stream data after a crash.
#[derive(Debug, Clone)]
pub enum RecoveredStream {
    /// The stream ended cleanly but was not sealed.
    Complete {
        /// The step ID that was interrupted
        step_id: StepId,
        /// Accumulated text from `text_delta` events
        partial_text: String,
        /// Last sequence number seen
        last_seq: u64,
        /// Model name from the original stream (for accurate attribution)
        model_name: Option<String>,
    },
    /// The stream ended with an error but was not sealed.
    Errored {
        /// The step ID that was interrupted
        step_id: StepId,
        /// Accumulated text from `text_delta` events
        partial_text: String,
        /// Last sequence number seen
        last_seq: u64,
        /// Error message captured from the stream
        error: String,
        /// Model name from the original stream (for accurate attribution)
        model_name: Option<String>,
    },
    /// The stream ended mid-flight.
    Incomplete {
        /// The step ID that was interrupted
        step_id: StepId,
        /// Accumulated text from `text_delta` events
        partial_text: String,
        /// Last sequence number seen
        last_seq: u64,
        /// Model name from the original stream (for accurate attribution)
        model_name: Option<String>,
    },
}

static JOURNAL_ID: AtomicU64 = AtomicU64::new(1);

/// Stream journal for durable streaming with crash recovery
///
/// This journal persists every streaming delta to `SQLite` before it's
/// displayed to the user, enabling recovery after crashes.
pub struct StreamJournal {
    db: Connection,
    journal_id: u64,
    active_step: Option<StepId>,
}

impl StreamJournal {
    /// SQL to initialize the database schema
    const SCHEMA: &'static str = r"
        CREATE TABLE IF NOT EXISTS stream_journal (
            step_id INTEGER NOT NULL,
            seq INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL,
            sealed INTEGER DEFAULT 0,
            PRIMARY KEY(step_id, seq)
        );

        CREATE INDEX IF NOT EXISTS idx_journal_unsealed
        ON stream_journal(sealed) WHERE sealed = 0;

        CREATE TABLE IF NOT EXISTS step_counter (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            next_step_id INTEGER NOT NULL DEFAULT 1
        );

        INSERT OR IGNORE INTO step_counter (id, next_step_id) VALUES (1, 1);

        -- Track which steps have been committed to history (for crash recovery)
        CREATE TABLE IF NOT EXISTS step_metadata (
            step_id INTEGER PRIMARY KEY,
            model_name TEXT,
            committed INTEGER DEFAULT 0,
            created_at TEXT NOT NULL
        );
    ";

    /// Open or create journal database at the given path
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Ensure parent directory exists
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
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        Self::initialize(db)
    }

    /// Open an in-memory journal (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().context("Failed to open in-memory database")?;
        Self::initialize(db)
    }

    /// Initialize database with schema and determine current state
    fn initialize(db: Connection) -> Result<Self> {
        // Enable WAL mode for better concurrent performance
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .context("Failed to set pragmas")?;

        // Create schema
        db.execute_batch(Self::SCHEMA)
            .context("Failed to create schema")?;

        Ok(Self {
            db,
            journal_id: JOURNAL_ID.fetch_add(1, Ordering::Relaxed),
            active_step: None,
        })
    }

    /// Begin a new journal session for streaming.
    ///
    /// # Arguments
    ///
    /// * `model_name` - The model being used for this stream (stored for crash recovery attribution)
    ///
    /// # Errors
    ///
    /// Returns an error if a session is already active, uncommitted entries exist,
    /// or a new step ID cannot be allocated.
    pub fn begin_session(&mut self, model_name: impl Into<String>) -> Result<ActiveJournal> {
        if let Some(step_id) = self.active_step {
            bail!("Cannot begin session: already streaming step {step_id}");
        }
        // Check for any recoverable steps (unsealed OR uncommitted)
        if let Some(step_id) = self.latest_recoverable_step_id()? {
            bail!("Cannot begin session: recoverable step {step_id} exists");
        }

        let step_id = self.next_step_id()?;
        let model_name = model_name.into();

        // Record step metadata for crash recovery
        let created_at = system_time_to_iso8601(SystemTime::now());
        self.db
            .execute(
                "INSERT INTO step_metadata (step_id, model_name, committed, created_at)
                 VALUES (?1, ?2, 0, ?3)",
                params![step_id, &model_name, created_at],
            )
            .context("Failed to insert step metadata")?;

        self.active_step = Some(step_id);
        Ok(ActiveJournal {
            journal_id: self.journal_id,
            step_id,
            next_seq: 1,
            model_name,
        })
    }

    /// Find the most recent recoverable step (unsealed OR sealed but uncommitted).
    fn latest_recoverable_step_id(&self) -> Result<Option<StepId>> {
        // A step is recoverable if:
        // 1. It has unsealed journal entries, OR
        // 2. It has metadata with committed=0
        let step_id: Option<StepId> = self
            .db
            .query_row(
                "SELECT step_id FROM (
                    SELECT DISTINCT step_id FROM stream_journal WHERE sealed = 0
                    UNION
                    SELECT step_id FROM step_metadata WHERE committed = 0
                 ) ORDER BY step_id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query recoverable step")?;

        Ok(step_id)
    }

    /// Seal unsealed entries for a step (used for crash recovery).
    pub fn seal_unsealed(&mut self, step_id: StepId) -> Result<String> {
        self.ensure_idle()?;
        let accumulated_text = collect_text(&self.db, step_id)?;
        seal_step(&self.db, step_id)?;
        Ok(accumulated_text)
    }

    /// Atomically commit and prune a step.
    ///
    /// This should be called AFTER the history has been successfully persisted to disk.
    /// Performs an atomic transaction that:
    /// 1. Marks the step as committed
    /// 2. Deletes all journal entries for the step
    /// 3. Deletes the step metadata
    ///
    /// If any part fails, nothing is changed (transactional safety).
    /// Once committed, the step will not be recovered on restart.
    pub fn commit_and_prune_step(&mut self, step_id: StepId) -> Result<u64> {
        let tx = self
            .db
            .transaction()
            .context("Failed to start commit-prune transaction")?;

        // Mark as committed
        tx.execute(
            "UPDATE step_metadata SET committed = 1 WHERE step_id = ?1",
            params![step_id],
        )
        .with_context(|| format!("Failed to mark step {step_id} as committed"))?;

        // Delete journal entries
        let deleted = tx
            .execute(
                "DELETE FROM stream_journal WHERE step_id = ?1",
                params![step_id],
            )
            .with_context(|| format!("Failed to delete journal entries for step {step_id}"))?;

        // Delete metadata (now that it's committed)
        tx.execute(
            "DELETE FROM step_metadata WHERE step_id = ?1",
            params![step_id],
        )
        .with_context(|| format!("Failed to delete metadata for step {step_id}"))?;

        tx.commit()
            .context("Failed to commit commit-prune transaction")?;

        Ok(deleted as u64)
    }

    /// Discard a step that was never committed (error/cancel path).
    ///
    /// Use this when a stream fails or is cancelled and should not be recovered.
    /// Unlike `commit_and_prune_step`, this does NOT require successful history persistence.
    pub fn discard_step(&mut self, step_id: StepId) -> Result<u64> {
        let tx = self
            .db
            .transaction()
            .context("Failed to start discard transaction")?;

        // Delete journal entries
        let deleted = tx
            .execute(
                "DELETE FROM stream_journal WHERE step_id = ?1",
                params![step_id],
            )
            .with_context(|| format!("Failed to delete journal entries for step {step_id}"))?;

        // Delete metadata
        tx.execute(
            "DELETE FROM step_metadata WHERE step_id = ?1",
            params![step_id],
        )
        .with_context(|| format!("Failed to delete metadata for step {step_id}"))?;

        tx.commit()
            .context("Failed to commit discard transaction")?;

        Ok(deleted as u64)
    }

    /// Get the model name for a step from metadata.
    fn get_step_model_name(&self, step_id: StepId) -> Result<Option<String>> {
        let model_name: Option<String> = self
            .db
            .query_row(
                "SELECT model_name FROM step_metadata WHERE step_id = ?1",
                params![step_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query step model name")?;
        Ok(model_name)
    }

    /// Delete all unsealed entries for a step (discard recovery data).
    pub fn discard_unsealed(&mut self, step_id: StepId) -> Result<u64> {
        self.ensure_idle()?;
        discard_step(&self.db, step_id)
    }

    /// Get statistics about the journal.
    pub fn stats(&self) -> Result<JournalStats> {
        stats_for_db(&self.db)
    }

    /// Allocate the next step ID
    ///
    /// This atomically increments the step counter and returns the new ID.
    pub fn next_step_id(&mut self) -> Result<StepId> {
        let tx = self
            .db
            .transaction()
            .context("Failed to start step-id transaction")?;

        let step_id: StepId = tx
            .query_row(
                "SELECT next_step_id FROM step_counter WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .context("Failed to read next step ID")?;

        tx.execute(
            "UPDATE step_counter SET next_step_id = next_step_id + 1 WHERE id = 1",
            [],
        )
        .context("Failed to increment step counter")?;

        tx.commit()
            .context("Failed to commit step-id transaction")?;

        Ok(step_id)
    }

    fn append_event(&mut self, session: &mut ActiveJournal, event: StreamDeltaEvent) -> Result<()> {
        self.ensure_active(session)?;
        let seq = session.next_seq;
        let delta = StreamDelta::new(session.step_id, seq, event);
        append_delta(&self.db, &delta)?;
        session.next_seq += 1;
        Ok(())
    }

    fn seal_active(&mut self, session: ActiveJournal) -> Result<String> {
        self.ensure_active(&session)?;
        let text = collect_text(&self.db, session.step_id)?;
        seal_step(&self.db, session.step_id)?;
        self.active_step = None;
        Ok(text)
    }

    fn discard_active(&mut self, session: ActiveJournal) -> Result<u64> {
        self.ensure_active(&session)?;
        let deleted = discard_step(&self.db, session.step_id)?;
        self.active_step = None;
        Ok(deleted)
    }

    fn ensure_active(&self, session: &ActiveJournal) -> Result<()> {
        if session.journal_id != self.journal_id {
            bail!("Journal session does not belong to this journal");
        }
        match self.active_step {
            Some(step_id) if step_id == session.step_id => Ok(()),
            Some(step_id) => bail!(
                "Active step {} does not match session {}",
                step_id,
                session.step_id
            ),
            None => bail!("No active streaming session"),
        }
    }

    fn ensure_idle(&self) -> Result<()> {
        if self.active_step.is_some() {
            bail!("Cannot perform recovery while streaming");
        }
        Ok(())
    }

    /// Check for and recover incomplete streams after a crash
    ///
    /// Looks for recoverable entries in the journal (unsealed OR sealed but uncommitted)
    /// and reconstructs the partial stream state.
    ///
    /// # Returns
    ///
    /// `Some(RecoveredStream)` if there are recoverable entries, `None` otherwise.
    pub fn recover(&self) -> Result<Option<RecoveredStream>> {
        if self.active_step.is_some() {
            return Ok(None);
        }

        // Find the most recent recoverable step (unsealed OR uncommitted)
        let Some(step_id) = self.latest_recoverable_step_id()? else {
            return Ok(None);
        };

        // Get model name from metadata (if available)
        let model_name = self.get_step_model_name(step_id)?;

        // Get the last sequence number for this step
        let last_seq: u64 = self
            .db
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM stream_journal WHERE step_id = ?1",
                params![step_id],
                |row| row.get(0),
            )
            .context("Failed to query last sequence")?;

        // Collect partial text (from both sealed and unsealed entries)
        let partial_text = collect_text_all(&self.db, step_id)?;

        if let Some(error) = latest_error(&self.db, step_id)? {
            return Ok(Some(RecoveredStream::Errored {
                step_id,
                partial_text,
                last_seq,
                error,
                model_name,
            }));
        }

        // Check if stream has a terminal event (done)
        let is_complete: bool = self
            .db
            .query_row(
                "SELECT 1 FROM stream_journal
                 WHERE step_id = ?1 AND event_type = 'done'
                 LIMIT 1",
                params![step_id],
                |_| Ok(true),
            )
            .optional()
            .context("Failed to query completion status")?
            .unwrap_or(false);

        let recovered = if is_complete {
            RecoveredStream::Complete {
                step_id,
                partial_text,
                last_seq,
                model_name,
            }
        } else {
            RecoveredStream::Incomplete {
                step_id,
                partial_text,
                last_seq,
                model_name,
            }
        };

        Ok(Some(recovered))
    }

    /// Clean up sealed entries older than the retention period
    ///
    /// # Returns
    ///
    /// The number of rows deleted.
    #[cfg(test)]
    pub fn prune(&mut self, older_than: Duration) -> Result<u64> {
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .ok_or_else(|| anyhow!("Duration overflow calculating cutoff time"))?;

        let cutoff_str = system_time_to_iso8601(cutoff);

        let deleted = self
            .db
            .execute(
                "DELETE FROM stream_journal
                 WHERE sealed = 1 AND created_at <= ?1",
                params![cutoff_str],
            )
            .context("Failed to prune old entries")?;

        Ok(deleted as u64)
    }
}

fn append_delta(db: &Connection, delta: &StreamDelta) -> Result<()> {
    let created_at = system_time_to_iso8601(delta.timestamp);
    let seq_i64 = i64::try_from(delta.seq).context("seq overflow")?;

    db.execute(
        "INSERT INTO stream_journal (step_id, seq, event_type, content, created_at, sealed)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        params![
            delta.step_id,
            seq_i64,
            delta.event.event_type(),
            delta.event.content(),
            created_at
        ],
    )
    .with_context(|| {
        format!(
            "Failed to insert delta for step {} seq {}",
            delta.step_id, delta.seq
        )
    })?;

    Ok(())
}

fn collect_text(db: &Connection, step_id: StepId) -> Result<String> {
    let mut stmt = db
        .prepare(
            "SELECT content FROM stream_journal
             WHERE step_id = ?1 AND event_type = 'text_delta'
             ORDER BY seq ASC",
        )
        .context("Failed to prepare text collection query")?;

    let contents: Vec<String> = stmt
        .query_map(params![step_id], |row| row.get(0))
        .context("Failed to query text deltas")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to collect text deltas")?;

    Ok(contents.join(""))
}

/// Collect text from all entries (both sealed and unsealed) for recovery.
fn collect_text_all(db: &Connection, step_id: StepId) -> Result<String> {
    // Same as collect_text - neither filters by sealed status
    collect_text(db, step_id)
}

fn latest_error(db: &Connection, step_id: StepId) -> Result<Option<String>> {
    // Look at all entries (not just unsealed) for recovery scenarios
    let mut stmt = db
        .prepare(
            "SELECT content FROM stream_journal
             WHERE step_id = ?1 AND event_type = 'error'
             ORDER BY seq DESC
             LIMIT 1",
        )
        .context("Failed to prepare error query")?;

    let error = stmt
        .query_row(params![step_id], |row| row.get(0))
        .optional()
        .context("Failed to query error event")?;

    Ok(error)
}

fn seal_step(db: &Connection, step_id: StepId) -> Result<()> {
    db.execute(
        "UPDATE stream_journal SET sealed = 1 WHERE step_id = ?1 AND sealed = 0",
        params![step_id],
    )
    .with_context(|| format!("Failed to seal step {step_id}"))?;

    Ok(())
}

fn discard_step(db: &Connection, step_id: StepId) -> Result<u64> {
    let deleted = db
        .execute(
            "DELETE FROM stream_journal WHERE step_id = ?1 AND sealed = 0",
            params![step_id],
        )
        .with_context(|| format!("Failed to discard unsealed entries for step {step_id}"))?;

    // Also clean up the metadata entry for this discarded step
    db.execute(
        "DELETE FROM step_metadata WHERE step_id = ?1",
        params![step_id],
    )
    .with_context(|| format!("Failed to delete metadata for discarded step {step_id}"))?;

    Ok(deleted as u64)
}

fn stats_for_db(db: &Connection) -> Result<JournalStats> {
    let total_entries: i64 = db
        .query_row("SELECT COUNT(*) FROM stream_journal", [], |row| row.get(0))
        .context("Failed to count entries")?;

    let sealed_entries: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM stream_journal WHERE sealed = 1",
            [],
            |row| row.get(0),
        )
        .context("Failed to count sealed entries")?;

    let unsealed_entries = total_entries - sealed_entries;

    let current_step_id: StepId = db
        .query_row(
            "SELECT next_step_id - 1 FROM step_counter WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .context("Failed to get current step ID")?;

    Ok(JournalStats {
        total_entries: total_entries as u64,
        sealed_entries: sealed_entries as u64,
        unsealed_entries: unsealed_entries as u64,
        current_step_id,
    })
}

/// Statistics about the journal
#[derive(Debug, Clone)]
pub struct JournalStats {
    /// Total number of entries
    pub total_entries: u64,
    /// Number of sealed entries
    pub sealed_entries: u64,
    /// Number of unsealed entries
    pub unsealed_entries: u64,
    /// Current (last allocated) step ID
    pub current_step_id: StepId,
}

/// Convert a `SystemTime` to ISO 8601 string
fn system_time_to_iso8601(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Simple ISO 8601 format without timezone
    chrono_lite_format(secs, millis)
}

/// Minimal ISO 8601 formatting without external dependencies
fn chrono_lite_format(secs: u64, millis: u32) -> String {
    // Calculate date/time components from unix timestamp
    const SECS_PER_DAY: u64 = 86400;
    const SECS_PER_HOUR: u64 = 3600;
    const SECS_PER_MINUTE: u64 = 60;

    let days = secs / SECS_PER_DAY;
    let remaining = secs % SECS_PER_DAY;

    let hours = remaining / SECS_PER_HOUR;
    let remaining = remaining % SECS_PER_HOUR;

    let minutes = remaining / SECS_PER_MINUTE;
    let seconds = remaining % SECS_PER_MINUTE;

    // Calculate year, month, day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
    )
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
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("Failed to set directory permissions: {}", path.display()))?;
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
        Some(name) => path.with_file_name(format!("{}{}", name, suffix)),
        None => std::path::PathBuf::from(format!("{}{}", path.display(), suffix)),
    }
}

/// Convert days since Unix epoch to (year, month, day)
fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's date algorithms
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

/// Parse ISO 8601 string to `SystemTime` (for internal use)
#[allow(dead_code)]
fn iso8601_to_system_time(s: &str) -> Option<SystemTime> {
    // Parse format: YYYY-MM-DDTHH:MM:SS.mmmZ
    if s.len() < 19 {
        return None;
    }

    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let minute: u32 = s.get(14..16)?.parse().ok()?;
    let second: u32 = s.get(17..19)?.parse().ok()?;

    let millis: u32 = if s.len() >= 23 && s.get(19..20) == Some(".") {
        s.get(20..23)?.parse().ok()?
    } else {
        0
    };

    let days = ymd_to_days(year, month, day)?;
    let secs = days as u64 * 86400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second);
    let duration = Duration::from_secs(secs) + Duration::from_millis(u64::from(millis));

    UNIX_EPOCH.checked_add(duration)
}

/// Convert (year, month, day) to days since Unix epoch
fn ymd_to_days(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m = month;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    Some(era * 146_097 + i64::from(doe) - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn unique_db_path(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!("forge_{label}_{stamp}.db"));
        path
    }

    #[test]
    fn test_open_in_memory() {
        let journal = StreamJournal::open_in_memory().unwrap();
        assert!(journal.recover().unwrap().is_none());
    }

    #[test]
    fn test_next_step_id_increments() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let id1 = journal.next_step_id().unwrap();
        let id2 = journal.next_step_id().unwrap();
        let id3 = journal.next_step_id().unwrap();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_begin_session_returns_active_journal() {
        let mut journal = StreamJournal::open_in_memory().unwrap();
        let active = journal.begin_session("test-model").unwrap();
        assert_eq!(active.step_id(), 1);
    }

    #[test]
    fn test_begin_session_fails_when_unsealed_exists() {
        let db_path = unique_db_path("begin_session_unsealed");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Hello").unwrap();
            // Drop without sealing to leave unsealed entries.
        }

        let mut journal = StreamJournal::open(&db_path).unwrap();
        let result = journal.begin_session("test-model");
        assert!(result.is_err());

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_append_delta_succeeds() {
        let mut journal = StreamJournal::open_in_memory().unwrap();
        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "Hello").unwrap();

        let stats = journal.stats().unwrap();
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.unsealed_entries, 1);
    }

    #[test]
    fn test_seal_returns_accumulated_text() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "Hello").unwrap();
        active.append_text(&mut journal, " ").unwrap();
        active.append_text(&mut journal, "World").unwrap();
        active.append_done(&mut journal).unwrap();

        let text = active.seal(&mut journal).unwrap();
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_seal_records_entries() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "Test").unwrap();
        let _text = active.seal(&mut journal).unwrap();

        let stats = journal.stats().unwrap();
        assert_eq!(stats.sealed_entries, 1);
        assert_eq!(stats.unsealed_entries, 0);
    }

    #[test]
    fn test_recover_finds_unsealed_stream() {
        let db_path = unique_db_path("recover_incomplete");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Partial").unwrap();
            active.append_text(&mut journal, " response").unwrap();
            // Drop without sealing to simulate a crash.
        }

        let journal = StreamJournal::open(&db_path).unwrap();
        let recovered = journal.recover().unwrap().expect("recovered stream");
        match recovered {
            RecoveredStream::Incomplete {
                step_id: recovered_step,
                partial_text,
                last_seq,
                ..
            } => {
                assert_eq!(recovered_step, 1);
                assert_eq!(partial_text, "Partial response");
                assert_eq!(last_seq, 2);
            }
            _ => panic!("Expected incomplete recovery"),
        }

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_recover_detects_complete_but_unsealed() {
        let db_path = unique_db_path("recover_complete");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Complete").unwrap();
            active.append_done(&mut journal).unwrap();
            // Drop without sealing.
        }

        let journal = StreamJournal::open(&db_path).unwrap();
        let recovered = journal.recover().unwrap().expect("recovered stream");
        match recovered {
            RecoveredStream::Complete { partial_text, .. } => {
                assert_eq!(partial_text, "Complete");
            }
            _ => panic!("Expected complete recovery"),
        }

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_recover_returns_none_when_all_sealed() {
        let db_path = unique_db_path("recover_none");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Test").unwrap();
            let step_id = active.step_id();
            let _text = active.seal(&mut journal).unwrap();
            // Commit and prune - simulating successful history persistence
            journal.commit_and_prune_step(step_id).unwrap();
        }

        let journal = StreamJournal::open(&db_path).unwrap();
        assert!(journal.recover().unwrap().is_none());

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_prune_removes_old_sealed_entries() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "Old").unwrap();
        let _text = active.seal(&mut journal).unwrap();

        let deleted = journal.prune(Duration::from_secs(0)).unwrap();
        assert!(deleted > 0);

        let stats = journal.stats().unwrap();
        assert_eq!(stats.sealed_entries, 0);
    }

    #[test]
    fn test_prune_preserves_unsealed_entries() {
        let db_path = unique_db_path("prune_unsealed");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Unsealed").unwrap();
            // Drop without sealing.
        }

        let mut journal = StreamJournal::open(&db_path).unwrap();
        let deleted = journal.prune(Duration::from_secs(0)).unwrap();
        assert_eq!(deleted, 0);

        let stats = journal.stats().unwrap();
        assert_eq!(stats.unsealed_entries, 1);

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_discard_unsealed() {
        let db_path = unique_db_path("discard_unsealed");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Discard me").unwrap();
            active.append_text(&mut journal, "!").unwrap();
            // Drop without sealing.
        }

        let mut journal = StreamJournal::open(&db_path).unwrap();
        let deleted = journal.discard_unsealed(1).unwrap();
        assert_eq!(deleted, 2);
        assert!(journal.recover().unwrap().is_none());

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_multiple_steps() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "First").unwrap();
        let step_id = active.step_id();
        let _text = active.seal(&mut journal).unwrap();
        // Commit and prune first step before starting second
        journal.commit_and_prune_step(step_id).unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "Second").unwrap();
        let text = active.seal(&mut journal).unwrap();

        assert_eq!(text, "Second");
    }

    #[test]
    fn test_stats() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "A").unwrap();
        active.append_text(&mut journal, "B").unwrap();
        let step_id = active.step_id();
        let _text = active.seal(&mut journal).unwrap();
        // Commit and prune first step before starting second
        journal.commit_and_prune_step(step_id).unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_text(&mut journal, "C").unwrap();

        let stats = journal.stats().unwrap();
        // First step was pruned, so only 1 entry from second step
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.sealed_entries, 0);
        assert_eq!(stats.unsealed_entries, 1);
        assert_eq!(stats.current_step_id, active.step_id());
    }

    #[test]
    fn test_date_conversion_roundtrip() {
        let original = SystemTime::now();
        let iso = system_time_to_iso8601(original);
        let parsed = iso8601_to_system_time(&iso).unwrap();

        let diff = if original > parsed {
            original.duration_since(parsed).unwrap()
        } else {
            parsed.duration_since(original).unwrap()
        };
        assert!(diff.as_millis() < 1000);
    }

    #[test]
    fn test_persistence_across_instances() {
        let db_path = unique_db_path("persist");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Persisted").unwrap();
            // Drop without sealing.
        }

        {
            let journal = StreamJournal::open(&db_path).unwrap();
            let recovered = journal.recover().unwrap().expect("recovered stream");
            match recovered {
                RecoveredStream::Incomplete {
                    partial_text,
                    step_id,
                    ..
                } => {
                    assert_eq!(partial_text, "Persisted");
                    assert_eq!(step_id, 1);
                }
                _ => panic!("Expected incomplete recovery"),
            }
        }

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_error_event_in_stream() {
        let db_path = unique_db_path("error_event");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let mut active = journal.begin_session("test-model").unwrap();
            active.append_text(&mut journal, "Start").unwrap();
            active.append_error(&mut journal, "API Error").unwrap();
            // Drop without sealing.
        }

        let journal = StreamJournal::open(&db_path).unwrap();
        let recovered = journal.recover().unwrap().expect("recovered stream");
        match recovered {
            RecoveredStream::Errored {
                partial_text,
                error,
                ..
            } => {
                assert_eq!(partial_text, "Start");
                assert_eq!(error, "API Error");
            }
            _ => panic!("Expected error recovery"),
        }

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_empty_stream_seals_to_empty_string() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let mut active = journal.begin_session("test-model").unwrap();
        active.append_done(&mut journal).unwrap();

        let text = active.seal(&mut journal).unwrap();
        assert!(text.is_empty());
    }

    #[test]
    fn test_step_id_counter_persistence() {
        let db_path = unique_db_path("counter");
        let _ = fs::remove_file(&db_path);

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            assert_eq!(journal.next_step_id().unwrap(), 1);
            assert_eq!(journal.next_step_id().unwrap(), 2);
            assert_eq!(journal.next_step_id().unwrap(), 3);
        }

        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            assert_eq!(journal.next_step_id().unwrap(), 4);
            assert_eq!(journal.next_step_id().unwrap(), 5);
        }

        let _ = fs::remove_file(&db_path);
    }
}

//! Stream Journal - Streaming durability system for crash recovery
//!
//! This module ensures that streaming deltas are persisted to SQLite BEFORE
//! being displayed to the user. This guarantees that after a crash, we can
//! recover the partial response and resume or replay.
//!
//! # Key Invariant
//!
//! **Deltas MUST be persisted BEFORE being displayed to the user.**
//!
//! This write-ahead logging approach ensures durability at the cost of
//! slightly higher latency per delta.

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Unique identifier for a streaming step/session
pub type StepId = i64;

/// A single delta event in a streaming response.
///
/// This makes invalid deltas unrepresentable (e.g., a "done" event with text content).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamDeltaEvent {
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamDelta {
    /// The step this delta belongs to
    pub step_id: StepId,
    /// Sequence number within the step (monotonically increasing)
    pub seq: u64,
    pub event: StreamDeltaEvent,
    /// When this delta was created
    pub timestamp: SystemTime,
}

impl StreamDelta {
    /// Create a new text delta
    pub fn text(step_id: StepId, seq: u64, content: impl Into<String>) -> Self {
        Self {
            step_id,
            seq,
            event: StreamDeltaEvent::TextDelta(content.into()),
            timestamp: SystemTime::now(),
        }
    }

    /// Create a "done" marker
    pub fn done(step_id: StepId, seq: u64) -> Self {
        Self {
            step_id,
            seq,
            event: StreamDeltaEvent::Done,
            timestamp: SystemTime::now(),
        }
    }

    /// Create an error marker
    pub fn error(step_id: StepId, seq: u64, message: impl Into<String>) -> Self {
        Self {
            step_id,
            seq,
            event: StreamDeltaEvent::Error(message.into()),
            timestamp: SystemTime::now(),
        }
    }
}

/// Current state of the journal
#[derive(Clone, Debug)]
pub enum JournalState {
    /// Actively streaming with current step and sequence number
    Streaming { step_id: StepId, seq: u64 },
    /// Stream completed and sealed
    Sealed { step_id: StepId },
    /// No active stream
    Empty,
}

/// A live journal session required for streaming.
#[derive(Debug, Clone)]
pub struct JournalSession {
    step_id: StepId,
    next_seq: u64,
}

impl JournalSession {
    fn new(step_id: StepId) -> Self {
        Self {
            step_id,
            next_seq: 1,
        }
    }

    pub fn step_id(&self) -> StepId {
        self.step_id
    }

    pub fn next_text(&mut self, content: impl Into<String>) -> StreamDelta {
        let seq = self.next_seq;
        self.next_seq += 1;
        StreamDelta::text(self.step_id, seq, content)
    }

    pub fn next_done(&mut self) -> StreamDelta {
        let seq = self.next_seq;
        self.next_seq += 1;
        StreamDelta::done(self.step_id, seq)
    }

    pub fn next_error(&mut self, message: impl Into<String>) -> StreamDelta {
        let seq = self.next_seq;
        self.next_seq += 1;
        StreamDelta::error(self.step_id, seq, message)
    }
}

/// Recovered stream data after a crash.
#[derive(Debug, Clone)]
pub enum RecoveredStream {
    /// The stream ended cleanly but was not sealed.
    Complete {
        /// The step ID that was interrupted
        step_id: StepId,
        /// Accumulated text from text_delta events
        partial_text: String,
        /// Last sequence number seen
        last_seq: u64,
    },
    /// The stream ended mid-flight.
    Incomplete {
        /// The step ID that was interrupted
        step_id: StepId,
        /// Accumulated text from text_delta events
        partial_text: String,
        /// Last sequence number seen
        last_seq: u64,
    },
}

/// Stream journal for durable streaming with crash recovery
///
/// This journal persists every streaming delta to SQLite before it's
/// displayed to the user, enabling recovery after crashes.
pub struct StreamJournal {
    db: Connection,
    state: JournalState,
}

impl StreamJournal {
    /// SQL to initialize the database schema
    const SCHEMA: &'static str = r#"
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
    "#;

    /// Open or create journal database at the given path
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        let db = Connection::open(path)
            .with_context(|| format!("Failed to open database at {:?}", path))?;

        Self::initialize(db)
    }

    /// Open an in-memory journal (for testing)
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().context("Failed to open in-memory database")?;
        Self::initialize(db)
    }

    /// Initialize database with schema and determine current state
    fn initialize(db: Connection) -> Result<Self> {
        // Enable WAL mode for better concurrent performance
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .context("Failed to set pragmas")?;

        // Create schema
        db.execute_batch(Self::SCHEMA)
            .context("Failed to create schema")?;

        // Determine current state by checking for unsealed entries
        let state = Self::determine_state(&db)?;

        Ok(Self { db, state })
    }

    /// Determine the current journal state from the database
    fn determine_state(db: &Connection) -> Result<JournalState> {
        // Check for any unsealed entries (most recent row across all steps).
        // This avoids undefined GROUP BY behavior when selecting non-aggregated columns.
        let mut stmt = db
            .prepare(
                "SELECT step_id, seq
                 FROM stream_journal
                 WHERE sealed = 0
                 ORDER BY step_id DESC, seq DESC
                 LIMIT 1",
            )
            .context("Failed to prepare state query")?;

        let result: Option<(StepId, u64)> = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok();

        match result {
            Some((step_id, seq)) => Ok(JournalState::Streaming { step_id, seq }),
            None => Ok(JournalState::Empty),
        }
    }

    /// Start a new streaming session
    ///
    /// # Errors
    ///
    /// Returns an error if there's already an active streaming session.
    fn begin_step(&mut self, step_id: StepId) -> Result<()> {
        match &self.state {
            JournalState::Streaming {
                step_id: active_id, ..
            } => {
                bail!(
                    "Cannot begin step {}: already streaming step {}",
                    step_id,
                    active_id
                );
            }
            JournalState::Sealed { .. } | JournalState::Empty => {
                self.state = JournalState::Streaming { step_id, seq: 0 };
                Ok(())
            }
        }
    }

    /// Begin a new journal session for streaming.
    ///
    /// # Errors
    ///
    /// Returns an error if no step ID can be allocated or the journal is already streaming.
    pub fn begin_session(&mut self) -> Result<JournalSession> {
        let step_id = self.next_step_id()?;
        self.begin_step(step_id)?;
        Ok(JournalSession::new(step_id))
    }

    /// Append a delta to the journal
    ///
    /// **CRITICAL**: This operation MUST complete successfully before the delta
    /// is displayed to the user. This ensures crash recovery is possible.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No active streaming session
    /// - Delta's step_id doesn't match active step
    /// - Database write fails
    pub fn append_delta(&mut self, delta: StreamDelta) -> Result<()> {
        let (active_step_id, current_seq) = match &self.state {
            JournalState::Streaming { step_id, seq } => (*step_id, *seq),
            JournalState::Empty => {
                bail!("Cannot append delta: no active streaming session");
            }
            JournalState::Sealed { step_id } => {
                bail!("Cannot append delta: step {} is already sealed", step_id);
            }
        };

        if delta.step_id != active_step_id {
            bail!(
                "Delta step_id {} doesn't match active step {}",
                delta.step_id,
                active_step_id
            );
        }

        let expected_seq = current_seq + 1;
        if delta.seq != expected_seq {
            bail!(
                "Delta seq {} doesn't match expected seq {}",
                delta.seq,
                expected_seq
            );
        }

        // Convert timestamp to ISO 8601 string
        let created_at = system_time_to_iso8601(delta.timestamp);

        // Insert the delta - this MUST complete before returning
        self.db
            .execute(
                "INSERT INTO stream_journal (step_id, seq, event_type, content, created_at, sealed)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                params![
                    delta.step_id,
                    delta.seq as i64,
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

        // Update state with new sequence number
        self.state = JournalState::Streaming {
            step_id: active_step_id,
            seq: delta.seq,
        };

        Ok(())
    }

    /// Seal the journal when streaming completes
    ///
    /// Marks all rows for the current step as sealed and returns the
    /// accumulated text content from all text_delta events.
    ///
    /// # Returns
    ///
    /// The concatenated text content from all text_delta events.
    ///
    /// # Errors
    ///
    /// Returns an error if no active streaming session or database operation fails.
    pub fn seal(&mut self) -> Result<String> {
        let step_id = match &self.state {
            JournalState::Streaming { step_id, .. } => *step_id,
            JournalState::Empty => {
                bail!("Cannot seal: no active streaming session");
            }
            JournalState::Sealed { step_id } => {
                bail!("Cannot seal: step {} is already sealed", step_id);
            }
        };

        // Collect all text content before sealing
        let accumulated_text = self.collect_text(step_id)?;

        // Mark all entries for this step as sealed
        self.db
            .execute(
                "UPDATE stream_journal SET sealed = 1 WHERE step_id = ?1 AND sealed = 0",
                params![step_id],
            )
            .with_context(|| format!("Failed to seal step {}", step_id))?;

        self.state = JournalState::Sealed { step_id };

        Ok(accumulated_text)
    }

    /// Collect text content from all text_delta events for a step
    fn collect_text(&self, step_id: StepId) -> Result<String> {
        let mut stmt = self
            .db
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

    /// Check for and recover incomplete streams after a crash
    ///
    /// Looks for unsealed entries in the journal and reconstructs the
    /// partial stream state.
    ///
    /// # Returns
    ///
    /// `Some(RecoveredStream)` if there are unsealed entries, `None` otherwise.
    pub fn recover(&self) -> Option<RecoveredStream> {
        // Find the most recent unsealed step
        let result: Option<(StepId, u64)> = self
            .db
            .query_row(
                "SELECT step_id, MAX(seq) FROM stream_journal
                 WHERE sealed = 0
                 GROUP BY step_id
                 ORDER BY step_id DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let (step_id, last_seq) = result?;

        // Collect partial text
        let partial_text = self.collect_text(step_id).ok()?;

        // Check if stream has a terminal event (done or error)
        let is_complete: bool = self
            .db
            .query_row(
                "SELECT 1 FROM stream_journal
                 WHERE step_id = ?1 AND sealed = 0
                   AND (event_type = 'done' OR event_type = 'error')
                 LIMIT 1",
                params![step_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        let recovered = if is_complete {
            RecoveredStream::Complete {
                step_id,
                partial_text,
                last_seq,
            }
        } else {
            RecoveredStream::Incomplete {
                step_id,
                partial_text,
                last_seq,
            }
        };

        Some(recovered)
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

    /// Get the current journal state
    pub fn state(&self) -> &JournalState {
        &self.state
    }

    /// Allocate the next step ID
    ///
    /// This atomically increments the step counter and returns the new ID.
    pub fn next_step_id(&self) -> Result<StepId> {
        let step_id: StepId = self
            .db
            .query_row(
                "UPDATE step_counter SET next_step_id = next_step_id + 1
                 WHERE id = 1
                 RETURNING next_step_id - 1",
                [],
                |row| row.get(0),
            )
            .context("Failed to allocate next step ID")?;

        Ok(step_id)
    }

    /// Abandon the current streaming session without sealing
    ///
    /// This leaves the entries unsealed so they can be recovered later.
    /// Useful when an error occurs and we want to abort.
    #[cfg(test)]
    pub fn abandon(&mut self) {
        if let JournalState::Streaming { .. } = &self.state {
            self.state = JournalState::Empty;
        }
    }

    /// Delete all unsealed entries for a step (discard recovery data)
    pub fn discard_unsealed(&mut self, step_id: StepId) -> Result<u64> {
        let deleted = self
            .db
            .execute(
                "DELETE FROM stream_journal WHERE step_id = ?1 AND sealed = 0",
                params![step_id],
            )
            .with_context(|| format!("Failed to discard unsealed entries for step {}", step_id))?;

        // If we discarded the current streaming step, reset state
        if let JournalState::Streaming {
            step_id: current, ..
        } = &self.state
            && *current == step_id
        {
            self.state = JournalState::Empty;
        }

        Ok(deleted as u64)
    }

    /// Get statistics about the journal
    pub fn stats(&self) -> Result<JournalStats> {
        let total_entries: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM stream_journal", [], |row| row.get(0))
            .context("Failed to count entries")?;

        let sealed_entries: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM stream_journal WHERE sealed = 1",
                [],
                |row| row.get(0),
            )
            .context("Failed to count sealed entries")?;

        let unsealed_entries = total_entries - sealed_entries;

        let current_step_id: StepId = self
            .db
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

/// Convert a SystemTime to ISO 8601 string
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
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hours, minutes, seconds, millis
    )
}

/// Convert days since Unix epoch to (year, month, day)
fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's date algorithms
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    (year as i32, m, d)
}

/// Parse ISO 8601 string to SystemTime (for internal use)
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
    let secs = days as u64 * 86400 + hour as u64 * 3600 + minute as u64 * 60 + second as u64;
    let duration = Duration::from_secs(secs) + Duration::from_millis(millis as u64);

    UNIX_EPOCH.checked_add(duration)
}

/// Convert (year, month, day) to days since Unix epoch
fn ymd_to_days(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m = month;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    Some(era * 146097 + doe as i64 - 719468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let journal = StreamJournal::open_in_memory().unwrap();
        assert!(matches!(journal.state(), JournalState::Empty));
    }

    #[test]
    fn test_next_step_id_increments() {
        let journal = StreamJournal::open_in_memory().unwrap();

        let id1 = journal.next_step_id().unwrap();
        let id2 = journal.next_step_id().unwrap();
        let id3 = journal.next_step_id().unwrap();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_begin_step_sets_streaming_state() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let expected_step_id = journal.begin_session().unwrap().step_id();

        match journal.state() {
            JournalState::Streaming { step_id, seq } => {
                assert_eq!(*step_id, expected_step_id);
                assert_eq!(*seq, 0);
            }
            _ => panic!("Expected Streaming state"),
        }
    }

    #[test]
    fn test_begin_step_fails_when_already_streaming() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let _session = journal.begin_session().unwrap();
        let result = journal.begin_session();

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already streaming")
        );
    }

    #[test]
    fn test_append_delta_succeeds() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let expected_step_id = journal.begin_session().unwrap().step_id();
        let delta = StreamDelta::text(expected_step_id, 1, "Hello");
        journal.append_delta(delta).unwrap();

        match journal.state() {
            JournalState::Streaming { step_id, seq } => {
                assert_eq!(*step_id, expected_step_id);
                assert_eq!(*seq, 1);
            }
            _ => panic!("Expected Streaming state"),
        }
    }

    #[test]
    fn test_append_delta_fails_wrong_step() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        let delta = StreamDelta::text(step_id + 1, 1, "Hello"); // Wrong step_id

        let result = journal.append_delta(delta);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("doesn't match"));
    }

    #[test]
    fn test_append_delta_fails_wrong_seq() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        let delta = StreamDelta::text(step_id, 5, "Hello"); // Wrong seq (should be 1)

        let result = journal.append_delta(delta);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("seq"));
    }

    #[test]
    fn test_append_delta_fails_no_session() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let delta = StreamDelta::text(1, 1, "Hello");
        let result = journal.append_delta(delta);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no active"));
    }

    #[test]
    fn test_seal_returns_accumulated_text() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Hello"))
            .unwrap();
        journal
            .append_delta(StreamDelta::text(step_id, 2, " "))
            .unwrap();
        journal
            .append_delta(StreamDelta::text(step_id, 3, "World"))
            .unwrap();
        journal.append_delta(StreamDelta::done(step_id, 4)).unwrap();

        let text = journal.seal().unwrap();
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_seal_sets_sealed_state() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Test"))
            .unwrap();
        journal.seal().unwrap();

        match journal.state() {
            JournalState::Sealed {
                step_id: sealed_step,
            } => {
                assert_eq!(*sealed_step, step_id);
            }
            _ => panic!("Expected Sealed state"),
        }
    }

    #[test]
    fn test_seal_fails_no_session() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let result = journal.seal();
        assert!(result.is_err());
    }

    #[test]
    fn test_recover_finds_unsealed_stream() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // Start a stream but don't seal it
        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Partial"))
            .unwrap();
        journal
            .append_delta(StreamDelta::text(step_id, 2, " response"))
            .unwrap();
        journal.abandon(); // Simulate crash by abandoning

        // Recovery should find the unsealed stream
        let recovered = journal.recover().unwrap();
        match recovered {
            RecoveredStream::Incomplete {
                step_id: recovered_step,
                partial_text,
                last_seq,
            } => {
                assert_eq!(recovered_step, step_id);
                assert_eq!(partial_text, "Partial response");
                assert_eq!(last_seq, 2);
            }
            _ => panic!("Expected incomplete recovery"),
        }
    }

    #[test]
    fn test_recover_detects_complete_but_unsealed() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // Start a stream with done marker but don't seal
        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Complete"))
            .unwrap();
        journal.append_delta(StreamDelta::done(step_id, 2)).unwrap();
        journal.abandon();

        let recovered = journal.recover().unwrap();
        match recovered {
            RecoveredStream::Complete { partial_text, .. } => {
                assert_eq!(partial_text, "Complete");
            }
            _ => panic!("Expected complete recovery"),
        }
    }

    #[test]
    fn test_recover_returns_none_when_all_sealed() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Test"))
            .unwrap();
        journal.seal().unwrap();

        assert!(journal.recover().is_none());
    }

    #[test]
    fn test_prune_removes_old_sealed_entries() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // Create and seal a stream
        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Old"))
            .unwrap();
        journal.seal().unwrap();

        // Prune with zero duration should remove everything sealed
        let deleted = journal.prune(Duration::from_secs(0)).unwrap();
        assert!(deleted > 0);

        // Stats should show no sealed entries
        let stats = journal.stats().unwrap();
        assert_eq!(stats.sealed_entries, 0);
    }

    #[test]
    fn test_prune_preserves_unsealed_entries() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // Create but don't seal
        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Unsealed"))
            .unwrap();
        journal.abandon();

        // Prune should not affect unsealed entries
        let deleted = journal.prune(Duration::from_secs(0)).unwrap();
        assert_eq!(deleted, 0);

        let stats = journal.stats().unwrap();
        assert_eq!(stats.unsealed_entries, 1);
    }

    #[test]
    fn test_discard_unsealed() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Discard me"))
            .unwrap();
        journal
            .append_delta(StreamDelta::text(step_id, 2, "!"))
            .unwrap();

        let deleted = journal.discard_unsealed(step_id).unwrap();
        assert_eq!(deleted, 2);

        assert!(matches!(journal.state(), JournalState::Empty));
        assert!(journal.recover().is_none());
    }

    #[test]
    fn test_multiple_steps() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // First step
        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "First"))
            .unwrap();
        journal.seal().unwrap();

        // Second step
        let step_id2 = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id2, 1, "Second"))
            .unwrap();
        let text = journal.seal().unwrap();

        assert_eq!(text, "Second");
    }

    #[test]
    fn test_stats() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        // Create and seal a step
        let id = journal.begin_session().unwrap().step_id();
        journal.append_delta(StreamDelta::text(id, 1, "A")).unwrap();
        journal.append_delta(StreamDelta::text(id, 2, "B")).unwrap();
        journal.seal().unwrap();

        // Create another step, unsealed
        let id2 = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(id2, 1, "C"))
            .unwrap();
        journal.abandon();

        let stats = journal.stats().unwrap();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.sealed_entries, 2);
        assert_eq!(stats.unsealed_entries, 1);
        assert_eq!(stats.current_step_id, id2);
    }

    #[test]
    fn test_date_conversion_roundtrip() {
        let original = SystemTime::now();
        let iso = system_time_to_iso8601(original);
        let parsed = iso8601_to_system_time(&iso).unwrap();

        // Should be within 1 second (we lose sub-millisecond precision)
        let diff = if original > parsed {
            original.duration_since(parsed).unwrap()
        } else {
            parsed.duration_since(original).unwrap()
        };
        assert!(diff.as_millis() < 1000);
    }

    #[test]
    fn test_stream_delta_constructors() {
        let text = StreamDelta::text(1, 1, "Hello");
        assert!(matches!(
            text.event,
            StreamDeltaEvent::TextDelta(ref t) if t == "Hello"
        ));

        let done = StreamDelta::done(1, 2);
        assert!(matches!(done.event, StreamDeltaEvent::Done));

        let error = StreamDelta::error(1, 3, "Something went wrong");
        assert!(matches!(
            error.event,
            StreamDeltaEvent::Error(ref e) if e == "Something went wrong"
        ));
    }

    #[test]
    fn test_persistence_across_instances() {
        use std::fs;
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join("test_journal.db");

        // Clean up any previous test
        let _ = fs::remove_file(&db_path);

        // Create and populate journal
        {
            let mut journal = StreamJournal::open(&db_path).unwrap();
            let step_id = journal.begin_session().unwrap().step_id();
            journal
                .append_delta(StreamDelta::text(step_id, 1, "Persisted"))
                .unwrap();
            // Don't seal - simulate crash
        }

        // Open new instance and recover
        {
            let journal = StreamJournal::open(&db_path).unwrap();
            let recovered = journal.recover().unwrap();
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

        // Clean up
        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_error_event_in_stream() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal
            .append_delta(StreamDelta::text(step_id, 1, "Start"))
            .unwrap();
        journal
            .append_delta(StreamDelta::error(step_id, 2, "API Error"))
            .unwrap();
        journal.abandon();

        let recovered = journal.recover().unwrap();
        match recovered {
            RecoveredStream::Complete { partial_text, .. } => {
                // Error is a terminal event
                assert_eq!(partial_text, "Start");
            }
            _ => panic!("Expected complete recovery"),
        }
    }

    #[test]
    fn test_empty_stream_seals_to_empty_string() {
        let mut journal = StreamJournal::open_in_memory().unwrap();

        let step_id = journal.begin_session().unwrap().step_id();
        journal.append_delta(StreamDelta::done(step_id, 1)).unwrap();

        let text = journal.seal().unwrap();
        assert!(text.is_empty());
    }

    #[test]
    fn test_step_id_counter_persistence() {
        use std::fs;
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join("test_counter.db");

        let _ = fs::remove_file(&db_path);

        // Allocate some IDs
        {
            let journal = StreamJournal::open(&db_path).unwrap();
            assert_eq!(journal.next_step_id().unwrap(), 1);
            assert_eq!(journal.next_step_id().unwrap(), 2);
            assert_eq!(journal.next_step_id().unwrap(), 3);
        }

        // Reopen and continue
        {
            let journal = StreamJournal::open(&db_path).unwrap();
            assert_eq!(journal.next_step_id().unwrap(), 4);
            assert_eq!(journal.next_step_id().unwrap(), 5);
        }

        let _ = fs::remove_file(&db_path);
    }
}

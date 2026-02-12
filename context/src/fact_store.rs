//! Fact Store - Persistent storage for Librarian-extracted facts.
//!
//! This module provides SQLite-backed storage for facts extracted by The Librarian.
//! Facts are stored with their type, content, entities, and metadata for efficient
//! retrieval during pre-flight context assembly.
//!
//! Source tracking allows facts to be linked to the files they were derived from,
//! enabling staleness detection when files change externally.

use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use super::librarian::{Fact, FactType};
use crate::sqlite_security::prepare_db_path;

/// Unique identifier for a stored fact.
pub type FactId = i64;

/// A stored fact with its metadata.
#[derive(Debug, Clone)]
pub struct StoredFact {
    pub id: FactId,
    pub fact: Fact,
    pub turn_number: u64,
    pub created_at: String,
}

/// A source file linked to facts.
#[derive(Debug, Clone)]
pub struct FactSource {
    pub id: i64,
    pub file_path: String,
    pub sha256: String,
    pub updated_at: String,
}

/// A fact with staleness information.
#[derive(Debug, Clone)]
pub struct FactWithStaleness {
    pub fact: StoredFact,
    /// Source files that have changed since the fact was extracted.
    pub stale_sources: Vec<String>,
}

impl FactWithStaleness {
    /// Returns true if any source files have changed.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.stale_sources.is_empty()
    }
}

/// Persistent store for Librarian-extracted facts.
pub struct FactStore {
    db: Connection,
}

impl FactStore {
    const SCHEMA: &'static str = r"
        CREATE TABLE IF NOT EXISTS facts (
            id INTEGER PRIMARY KEY,
            fact_type TEXT NOT NULL,
            content TEXT NOT NULL,
            turn_number INTEGER NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS fact_entities (
            fact_id INTEGER NOT NULL,
            entity TEXT NOT NULL,
            PRIMARY KEY (fact_id, entity),
            FOREIGN KEY (fact_id) REFERENCES facts(id) ON DELETE CASCADE
        );

        -- Source file tracking for staleness detection
        CREATE TABLE IF NOT EXISTS fact_sources (
            id INTEGER PRIMARY KEY,
            file_path TEXT NOT NULL UNIQUE,
            sha256 TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Links facts to their source files (many-to-many)
        CREATE TABLE IF NOT EXISTS fact_source_links (
            fact_id INTEGER NOT NULL,
            source_id INTEGER NOT NULL,
            PRIMARY KEY (fact_id, source_id),
            FOREIGN KEY (fact_id) REFERENCES facts(id) ON DELETE CASCADE,
            FOREIGN KEY (source_id) REFERENCES fact_sources(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_facts_turn
        ON facts(turn_number);

        CREATE INDEX IF NOT EXISTS idx_facts_type
        ON facts(fact_type);

        CREATE INDEX IF NOT EXISTS idx_fact_entities_entity
        ON fact_entities(entity);

        CREATE INDEX IF NOT EXISTS idx_fact_sources_path
        ON fact_sources(file_path);
    ";

    /// Open or create fact store database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        prepare_db_path(path)?;

        let db = Connection::open(path)
            .with_context(|| format!("Failed to open fact store at {}", path.display()))?;
        Self::initialize(db)
    }

    /// Open an in-memory fact store (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().context("Failed to open in-memory fact store")?;
        Self::initialize(db)
    }

    fn initialize(db: Connection) -> Result<Self> {
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )
        .context("Failed to set fact store pragmas")?;
        db.execute_batch(Self::SCHEMA)
            .context("Failed to create fact store schema")?;
        Ok(Self { db })
    }

    /// Store facts extracted from a conversation turn.
    pub fn store_facts(&mut self, facts: &[Fact], turn_number: u64) -> Result<Vec<FactId>> {
        let created_at = system_time_to_iso8601(SystemTime::now());
        let tx = self
            .db
            .transaction()
            .context("Failed to start fact store transaction")?;

        let mut ids = Vec::with_capacity(facts.len());

        for fact in facts {
            let fact_type_str = fact.fact_type.as_str();

            tx.execute(
                "INSERT INTO facts (fact_type, content, turn_number, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    fact_type_str,
                    &fact.content,
                    turn_number as i64,
                    &created_at
                ],
            )
            .context("Failed to insert fact")?;

            let fact_id = tx.last_insert_rowid();
            ids.push(fact_id);

            for entity in &fact.entities {
                tx.execute(
                    "INSERT OR IGNORE INTO fact_entities (fact_id, entity)
                     VALUES (?1, ?2)",
                    params![fact_id, entity],
                )
                .with_context(|| format!("Failed to insert entity: {entity}"))?;
            }
        }

        tx.commit()
            .context("Failed to commit fact store transaction")?;

        Ok(ids)
    }

    /// Get all stored facts.
    pub fn get_all_facts(&self) -> Result<Vec<StoredFact>> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT id, fact_type, content, turn_number, created_at
                 FROM facts
                 ORDER BY turn_number ASC, id ASC",
            )
            .context("Failed to prepare get_all_facts query")?;

        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let fact_type_str: String = row.get(1)?;
                let content: String = row.get(2)?;
                let turn_number: i64 = row.get(3)?;
                let created_at: String = row.get(4)?;
                Ok((id, fact_type_str, content, turn_number, created_at))
            })
            .context("Failed to query facts")?;

        let mut facts = Vec::new();
        for row in rows {
            let (id, fact_type_str, content, turn_number, created_at) =
                row.context("Failed to read fact row")?;

            let fact_type = match fact_type_str.as_str() {
                "entity" => FactType::Entity,
                "decision" => FactType::Decision,
                "constraint" => FactType::Constraint,
                "code_state" => FactType::CodeState,
                "pinned" => FactType::Pinned,
                _ => continue,
            };

            let entities = self.get_entities_for_fact(id)?;

            facts.push(StoredFact {
                id,
                fact: Fact {
                    fact_type,
                    content,
                    entities,
                },
                turn_number: turn_number as u64,
                created_at,
            });
        }

        Ok(facts)
    }

    /// Get entities for a specific fact.
    fn get_entities_for_fact(&self, fact_id: FactId) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT entity FROM fact_entities WHERE fact_id = ?1")
            .context("Failed to prepare entity query")?;

        let entities: Vec<String> = stmt
            .query_map([fact_id], |row| row.get(0))
            .context("Failed to query entities")?
            .filter_map(Result::ok)
            .collect();

        Ok(entities)
    }

    /// Search facts by entity keyword.
    pub fn search_by_entity(&self, keyword: &str) -> Result<Vec<StoredFact>> {
        let pattern = format!("%{keyword}%");
        let mut stmt = self
            .db
            .prepare(
                "SELECT DISTINCT f.id, f.fact_type, f.content, f.turn_number, f.created_at
                 FROM facts f
                 JOIN fact_entities e ON f.id = e.fact_id
                 WHERE e.entity LIKE ?1
                 ORDER BY f.turn_number DESC, f.id DESC",
            )
            .context("Failed to prepare search query")?;

        let rows = stmt
            .query_map([&pattern], |row| {
                let id: i64 = row.get(0)?;
                let fact_type_str: String = row.get(1)?;
                let content: String = row.get(2)?;
                let turn_number: i64 = row.get(3)?;
                let created_at: String = row.get(4)?;
                Ok((id, fact_type_str, content, turn_number, created_at))
            })
            .context("Failed to execute search query")?;

        let mut facts = Vec::new();
        for row in rows {
            let (id, fact_type_str, content, turn_number, created_at) =
                row.context("Failed to read search result")?;

            let fact_type = match fact_type_str.as_str() {
                "entity" => FactType::Entity,
                "decision" => FactType::Decision,
                "constraint" => FactType::Constraint,
                "code_state" => FactType::CodeState,
                "pinned" => FactType::Pinned,
                _ => continue,
            };

            let entities = self.get_entities_for_fact(id)?;

            facts.push(StoredFact {
                id,
                fact: Fact {
                    fact_type,
                    content,
                    entities,
                },
                turn_number: turn_number as u64,
                created_at,
            });
        }

        Ok(facts)
    }

    #[must_use]
    pub fn fact_count(&self) -> usize {
        self.db
            .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Query the highest turn number stored in the facts table.
    /// Returns 0 if no facts exist.
    pub fn max_turn_number(&self) -> Result<u64> {
        let max: i64 = self
            .db
            .query_row(
                "SELECT COALESCE(MAX(turn_number), 0) FROM facts",
                [],
                |row| row.get(0),
            )
            .context("Failed to query max turn number")?;
        Ok(max as u64)
    }

    /// Delete all facts (for testing/reset).
    pub fn clear(&mut self) -> Result<()> {
        self.db
            .execute_batch("DELETE FROM fact_entities; DELETE FROM facts;")
            .context("Failed to clear fact store")?;
        Ok(())
    }

    /// Add a pinned fact (user-explicitly marked as important).
    pub fn add_pinned_fact(
        &mut self,
        content: &str,
        entities: &[String],
        turn_number: u64,
    ) -> Result<FactId> {
        let fact = Fact {
            fact_type: FactType::Pinned,
            content: content.to_string(),
            entities: entities.to_vec(),
        };
        let ids = self.store_facts(&[fact], turn_number)?;
        Ok(ids.into_iter().next().unwrap_or(0))
    }

    /// Record or update a source file's SHA256 hash.
    ///
    /// Returns the source ID (existing or newly created).
    pub fn upsert_source(&mut self, file_path: &str, sha256: &str) -> Result<i64> {
        let updated_at = system_time_to_iso8601(SystemTime::now());

        let updated = self
            .db
            .execute(
                "UPDATE fact_sources SET sha256 = ?1, updated_at = ?2 WHERE file_path = ?3",
                params![sha256, &updated_at, file_path],
            )
            .context("Failed to update source")?;

        if updated > 0 {
            let id: i64 = self
                .db
                .query_row(
                    "SELECT id FROM fact_sources WHERE file_path = ?1",
                    [file_path],
                    |row| row.get(0),
                )
                .context("Failed to get source ID")?;
            return Ok(id);
        }

        self.db
            .execute(
                "INSERT INTO fact_sources (file_path, sha256, updated_at) VALUES (?1, ?2, ?3)",
                params![file_path, sha256, &updated_at],
            )
            .context("Failed to insert source")?;

        Ok(self.db.last_insert_rowid())
    }

    /// Link facts to their source files.
    ///
    /// Call this after storing facts to associate them with the files
    /// that were accessed during the turn.
    pub fn link_facts_to_sources(
        &mut self,
        fact_ids: &[FactId],
        source_paths: &[String],
    ) -> Result<()> {
        if fact_ids.is_empty() || source_paths.is_empty() {
            return Ok(());
        }

        let tx = self
            .db
            .transaction()
            .context("Failed to start transaction")?;

        for path in source_paths {
            let sha256 = match compute_file_sha256(path) {
                Ok(hash) => hash,
                Err(_) => continue,
            };

            let updated_at = system_time_to_iso8601(SystemTime::now());
            tx.execute(
                "INSERT INTO fact_sources (file_path, sha256, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(file_path) DO UPDATE SET sha256 = ?2, updated_at = ?3",
                params![path, &sha256, &updated_at],
            )
            .context("Failed to upsert source")?;

            let source_id: i64 = tx
                .query_row(
                    "SELECT id FROM fact_sources WHERE file_path = ?1",
                    [path],
                    |row| row.get(0),
                )
                .context("Failed to get source ID")?;

            for &fact_id in fact_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO fact_source_links (fact_id, source_id) VALUES (?1, ?2)",
                    params![fact_id, source_id],
                )
                .context("Failed to link fact to source")?;
            }
        }

        tx.commit().context("Failed to commit source links")?;
        Ok(())
    }

    /// Get source files for a specific fact.
    pub fn get_sources_for_fact(&self, fact_id: FactId) -> Result<Vec<FactSource>> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT s.id, s.file_path, s.sha256, s.updated_at
             FROM fact_sources s
             JOIN fact_source_links l ON s.id = l.source_id
             WHERE l.fact_id = ?1",
            )
            .context("Failed to prepare sources query")?;

        let sources: Vec<FactSource> = stmt
            .query_map([fact_id], |row| {
                Ok(FactSource {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    sha256: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .context("Failed to query sources")?
            .filter_map(Result::ok)
            .collect();

        Ok(sources)
    }

    /// Check staleness for a list of facts.
    ///
    /// Returns facts with information about which source files have changed.
    pub fn check_staleness(&self, facts: &[StoredFact]) -> Result<Vec<FactWithStaleness>> {
        let mut results = Vec::with_capacity(facts.len());

        for fact in facts {
            let sources = self.get_sources_for_fact(fact.id)?;
            let mut stale_sources = Vec::new();

            for source in sources {
                match compute_file_sha256(&source.file_path) {
                    Ok(current_sha) => {
                        if current_sha != source.sha256 {
                            stale_sources.push(source.file_path);
                        }
                    }
                    Err(_) => {
                        stale_sources.push(source.file_path);
                    }
                }
            }

            results.push(FactWithStaleness {
                fact: fact.clone(),
                stale_sources,
            });
        }

        Ok(results)
    }

    /// Search facts by entity and return with staleness info.
    pub fn search_with_staleness(&self, keyword: &str) -> Result<Vec<FactWithStaleness>> {
        let facts = self.search_by_entity(keyword)?;
        self.check_staleness(&facts)
    }
}

/// Convert SystemTime to ISO 8601 string.
fn system_time_to_iso8601(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    // Format as ISO 8601: 2024-01-15T10:30:00Z
    let days_since_epoch = secs / 86400;
    let secs_in_day = secs % 86400;
    let hours = secs_in_day / 3600;
    let minutes = (secs_in_day % 3600) / 60;
    let seconds = secs_in_day % 60;

    // Approximate year/month/day calculation (good enough for timestamps)
    let mut year = 1970i32;
    let mut remaining_days = days_since_epoch as i32;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let (month, day) = day_of_year_to_month_day(remaining_days, is_leap_year(year));

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn day_of_year_to_month_day(day_of_year: i32, leap: bool) -> (i32, i32) {
    let days_in_months: [i32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut remaining = day_of_year;
    for (i, &days) in days_in_months.iter().enumerate() {
        if remaining < days {
            return ((i + 1) as i32, remaining + 1);
        }
        remaining -= days;
    }
    (12, 31) // Fallback
}

/// Compute SHA256 hash of a file.
fn compute_file_sha256(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open file for hashing: {}", path.display()))?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read file for hashing: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let hash = hasher.finalize();
    // Manual hex encoding to avoid hex crate dependency
    let hex_chars: Vec<String> = hash.iter().map(|b| format!("{b:02x}")).collect();
    Ok(hex_chars.join(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_retrieve_facts() {
        let mut store = FactStore::open_in_memory().expect("open store");

        let facts = vec![
            Fact {
                fact_type: FactType::Entity,
                content: "File src/lib.rs contains main App struct".to_string(),
                entities: vec!["src/lib.rs".to_string(), "App".to_string()],
            },
            Fact {
                fact_type: FactType::Decision,
                content: "Chose async/await for concurrency".to_string(),
                entities: vec!["async".to_string(), "concurrency".to_string()],
            },
        ];

        let ids = store.store_facts(&facts, 1).expect("store facts");
        assert_eq!(ids.len(), 2);

        let retrieved = store.get_all_facts().expect("get all facts");
        assert_eq!(retrieved.len(), 2);
        assert_eq!(
            retrieved[0].fact.content,
            "File src/lib.rs contains main App struct"
        );
        assert_eq!(
            retrieved[1].fact.content,
            "Chose async/await for concurrency"
        );
    }

    #[test]
    fn test_search_by_entity() {
        let mut store = FactStore::open_in_memory().expect("open store");

        let facts = vec![
            Fact {
                fact_type: FactType::Entity,
                content: "src/lib.rs is the main entry point".to_string(),
                entities: vec!["src/lib.rs".to_string()],
            },
            Fact {
                fact_type: FactType::Entity,
                content: "src/main.rs is the binary".to_string(),
                entities: vec!["src/main.rs".to_string()],
            },
        ];

        store.store_facts(&facts, 1).expect("store facts");

        let results = store.search_by_entity("lib").expect("search");
        assert_eq!(results.len(), 1);
        assert!(results[0].fact.content.contains("lib.rs"));

        let results = store.search_by_entity("src").expect("search");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_fact_count() {
        let mut store = FactStore::open_in_memory().expect("open store");
        assert_eq!(store.fact_count(), 0);

        let facts = vec![Fact {
            fact_type: FactType::Entity,
            content: "Test fact".to_string(),
            entities: vec![],
        }];
        store.store_facts(&facts, 1).expect("store");
        assert_eq!(store.fact_count(), 1);
    }

    #[test]
    fn test_clear() {
        let mut store = FactStore::open_in_memory().expect("open store");

        let facts = vec![Fact {
            fact_type: FactType::Entity,
            content: "Test".to_string(),
            entities: vec!["test".to_string()],
        }];
        store.store_facts(&facts, 1).expect("store");
        assert_eq!(store.fact_count(), 1);

        store.clear().expect("clear");
        assert_eq!(store.fact_count(), 0);
    }

    #[test]
    fn test_pinned_fact() {
        let mut store = FactStore::open_in_memory().expect("open store");

        let id = store
            .add_pinned_fact(
                "Important: Never modify the API contract",
                &["API".to_string()],
                5,
            )
            .expect("add pinned");

        assert!(id > 0);

        let facts = store.get_all_facts().expect("get all");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact.fact_type, FactType::Pinned);
        assert_eq!(facts[0].turn_number, 5);
    }

    #[test]
    fn max_turn_number_empty_db() {
        let store = FactStore::open_in_memory().expect("open store");
        assert_eq!(store.max_turn_number().unwrap(), 0);
    }

    #[test]
    fn max_turn_number_tracks_highest() {
        let mut store = FactStore::open_in_memory().expect("open store");
        let facts = vec![Fact {
            fact_type: FactType::Entity,
            content: "test".to_string(),
            entities: vec![],
        }];
        store.store_facts(&facts, 5).expect("store at turn 5");
        assert_eq!(store.max_turn_number().unwrap(), 5);

        store.store_facts(&facts, 3).expect("store at turn 3");
        assert_eq!(store.max_turn_number().unwrap(), 5);

        store.store_facts(&facts, 8).expect("store at turn 8");
        assert_eq!(store.max_turn_number().unwrap(), 8);
    }
}

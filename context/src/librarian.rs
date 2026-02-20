//! The Librarian - persistent fact storage and recall.
//!
//! The Librarian wraps `FactStore` and exposes a high-level API used by
//! memory tools. Fact extraction/retrieval model calls are intentionally
//! out of scope here.

use std::fmt;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::fact_store::{FactStore, FactWithStaleness};

/// Types of facts the Librarian can store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactType {
    /// Entities: files, functions, variables, paths, URLs, etc.
    Entity,
    /// Decisions: "we chose X because Y"
    Decision,
    /// Constraints: "must stay compatible with Z", "don't modify X"
    Constraint,
    /// Code state: what was created, modified, deleted
    CodeState,
    /// User-pinned facts (explicitly marked important)
    Pinned,
}

impl FactType {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            FactType::Entity => "entity",
            FactType::Decision => "decision",
            FactType::Constraint => "constraint",
            FactType::CodeState => "code_state",
            FactType::Pinned => "pinned",
        }
    }
}

/// A stored fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// The type of fact.
    pub fact_type: FactType,
    /// The fact content.
    pub content: String,
    /// Searchable entities/keywords mentioned in this fact.
    pub entities: Vec<String>,
}

/// The Librarian manages fact storage and retrieval.
pub struct Librarian {
    store: FactStore,
    turn_counter: u64,
}

impl Librarian {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = FactStore::open(path)?;
        let turn_counter = store.max_turn_number()?;
        Ok(Self {
            store,
            turn_counter,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = FactStore::open_in_memory()?;
        let turn_counter = store.max_turn_number()?;
        Ok(Self {
            store,
            turn_counter,
        })
    }

    #[must_use]
    pub fn fact_count(&self) -> usize {
        self.store.fact_count()
    }

    /// Add a user-pinned fact.
    pub fn pin_fact(&mut self, content: &str, entities: &[String]) -> Result<()> {
        self.store
            .add_pinned_fact(content, entities, self.turn_counter)?;
        Ok(())
    }

    /// Search facts by keyword with staleness information.
    pub fn search_with_staleness(&self, keyword: &str) -> Result<Vec<FactWithStaleness>> {
        self.store.search_with_staleness(keyword)
    }

    /// Clear all facts (for testing/reset).
    pub fn clear(&mut self) -> Result<()> {
        self.store.clear()
    }
}

impl fmt::Debug for Librarian {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Librarian")
            .field("turn_counter", &self.turn_counter)
            .field("fact_count", &self.store.fact_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{FactType, Librarian};

    #[test]
    fn test_fact_type_as_str() {
        assert_eq!(FactType::Entity.as_str(), "entity");
        assert_eq!(FactType::Decision.as_str(), "decision");
        assert_eq!(FactType::Constraint.as_str(), "constraint");
        assert_eq!(FactType::CodeState.as_str(), "code_state");
        assert_eq!(FactType::Pinned.as_str(), "pinned");
    }

    #[test]
    fn test_librarian_pin_and_recall() {
        let mut librarian = Librarian::open_in_memory().expect("open in-memory librarian");
        librarian
            .pin_fact("Never delete migrations", &["migrations".to_string()])
            .expect("pin fact");

        let facts = librarian
            .search_with_staleness("migrations")
            .expect("search facts");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact.fact.content, "Never delete migrations");
    }
}

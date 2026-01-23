//! The Librarian - Intelligent context distillation and retrieval.
//!
//! The Librarian is a background component that:
//! 1. **Extracts** structured facts from conversation exchanges (post-turn)
//! 2. **Retrieves** relevant facts for new queries (pre-flight)
//!
//! This enables effectively unlimited conversation length while keeping
//! API costs low - instead of sending full history, we send:
//! - System prompt
//! - Retrieved facts (what Librarian determines is relevant)
//! - Recent N messages (immediate context)
//! - Current user message
//!
//! The Librarian uses a cheap, fast model (gemini-3-flash-preview) and runs
//! invisibly in the background - users never see it.

use std::fmt::Write;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;

use forge_providers::http_client_with_timeout;

/// Gemini Flash for cheap, fast Librarian operations.
const LIBRARIAN_MODEL: &str = "gemini-3-flash-preview";
const LIBRARIAN_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const LIBRARIAN_TIMEOUT_SECS: u64 = 30;

/// Types of facts the Librarian can extract.
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

/// A distilled fact extracted from conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// The type of fact.
    pub fact_type: FactType,
    /// The fact content.
    pub content: String,
    /// Searchable entities/keywords mentioned in this fact.
    pub entities: Vec<String>,
}

/// Result of fact extraction from a conversation exchange.
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    /// Extracted facts.
    pub facts: Vec<Fact>,
}

/// Result of relevance retrieval for a user query.
#[derive(Debug, Clone, Default)]
pub struct RetrievalResult {
    /// Facts deemed relevant to the query, ordered by relevance.
    pub relevant_facts: Vec<Fact>,
    /// Total token estimate for the retrieved facts.
    pub token_estimate: u32,
}

/// Extraction prompt loaded from cli/assets/contextinfinity_extraction.md
const EXTRACTION_PROMPT: &str = include_str!("../../cli/assets/contextinfinity_extraction.md");

/// Build the extraction prompt for post-turn fact distillation.
fn build_extraction_prompt(user_message: &str, assistant_message: &str) -> (String, String) {
    let user_input =
        format!("USER MESSAGE:\n{user_message}\n\nASSISTANT RESPONSE:\n{assistant_message}");

    (EXTRACTION_PROMPT.to_string(), user_input)
}

/// Retrieval prompt loaded from cli/assets/contextinfinity_retrieval.md
const RETRIEVAL_PROMPT: &str = include_str!("../../cli/assets/contextinfinity_retrieval.md");

/// Build the retrieval prompt for pre-flight relevance scoring.
fn build_retrieval_prompt(user_query: &str, available_facts: &[Fact]) -> (String, String) {
    let mut facts_text = String::new();
    for (i, fact) in available_facts.iter().enumerate() {
        let _ = writeln!(
            facts_text,
            "[{i}] ({}) {}: entities={}",
            fact.fact_type.as_str(),
            fact.content,
            fact.entities.join(", ")
        );
    }

    let user_input = format!("USER QUERY:\n{user_query}\n\nAVAILABLE FACTS:\n{facts_text}");

    (RETRIEVAL_PROMPT.to_string(), user_input)
}

/// Extract facts from a conversation exchange.
///
/// Called after each assistant response to distill the exchange into
/// structured facts for future retrieval.
pub async fn extract_facts(
    api_key: &str,
    user_message: &str,
    assistant_message: &str,
) -> Result<ExtractionResult> {
    let (system, user_input) = build_extraction_prompt(user_message, assistant_message);
    let response = call_librarian(api_key, &system, &user_input).await?;

    // Parse JSON response
    let facts: Vec<FactJson> = serde_json::from_str(&response)
        .map_err(|e| anyhow!("Failed to parse extraction response: {e}\nResponse: {response}"))?;

    let facts = facts
        .into_iter()
        .filter_map(|f| {
            let fact_type = match f.r#type.as_str() {
                "entity" => FactType::Entity,
                "decision" => FactType::Decision,
                "constraint" => FactType::Constraint,
                "code_state" => FactType::CodeState,
                "pinned" => FactType::Pinned,
                _ => return None,
            };
            Some(Fact {
                fact_type,
                content: f.content,
                entities: f.entities,
            })
        })
        .collect();

    Ok(ExtractionResult { facts })
}

/// Retrieve relevant facts for a user query.
///
/// Called before each API call to determine what context to inject.
pub async fn retrieve_relevant(
    api_key: &str,
    user_query: &str,
    available_facts: &[Fact],
) -> Result<RetrievalResult> {
    if available_facts.is_empty() {
        return Ok(RetrievalResult::default());
    }

    let (system, user_input) = build_retrieval_prompt(user_query, available_facts);
    let response = call_librarian(api_key, &system, &user_input).await?;

    // Parse JSON response (array of indices)
    let indices: Vec<usize> = serde_json::from_str(&response)
        .map_err(|e| anyhow!("Failed to parse retrieval response: {e}\nResponse: {response}"))?;

    let relevant_facts: Vec<Fact> = indices
        .into_iter()
        .filter_map(|i| available_facts.get(i).cloned())
        .collect();

    // Rough token estimate: ~4 chars per token
    let token_estimate = relevant_facts
        .iter()
        .map(|f| (f.content.len() / 4) as u32 + 10) // +10 for type/entity overhead
        .sum();

    Ok(RetrievalResult {
        relevant_facts,
        token_estimate,
    })
}

/// Call the Librarian model (Gemini Flash).
async fn call_librarian(
    api_key: &str,
    system_instruction: &str,
    user_input: &str,
) -> Result<String> {
    let client = http_client_with_timeout(LIBRARIAN_TIMEOUT_SECS)?;

    let body = json!({
        "system_instruction": {
            "parts": [{ "text": system_instruction }]
        },
        "contents": [
            {
                "role": "user",
                "parts": [{ "text": user_input }]
            }
        ],
        "generationConfig": {
            "maxOutputTokens": 2048,
            "temperature": 0.1,  // Low temperature for consistent extraction
            "thinkingConfig": {
                "thinkingLevel": "low"
            }
        }
    });

    let url = format!("{LIBRARIAN_API_BASE}/models/{LIBRARIAN_MODEL}:generateContent");

    let response = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read error: {e}>"));
        return Err(anyhow!("Librarian API error {status}: {error_text}"));
    }

    let json: serde_json::Value = response.json().await?;

    // Extract text from Gemini's response format
    let text = json["candidates"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|candidate| candidate["content"]["parts"].as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part["text"].as_str())
        .ok_or_else(|| anyhow!("Failed to extract text from Librarian response: {json:?}"))?;

    Ok(text.to_string())
}

/// JSON structure for parsing extraction results.
#[derive(Debug, Deserialize)]
struct FactJson {
    r#type: String,
    content: String,
    entities: Vec<String>,
}

/// Format retrieved facts for injection into context.
#[must_use]
pub fn format_facts_for_context(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }

    let mut output = String::from("## Relevant Context\n\n");

    for fact in facts {
        let type_label = match fact.fact_type {
            FactType::Entity => "📁",
            FactType::Decision => "🔧",
            FactType::Constraint => "⚠️",
            FactType::CodeState => "📝",
            FactType::Pinned => "📌",
        };
        let _ = writeln!(output, "{type_label} {}", fact.content);
    }

    output
}

// ============================================================================
// High-Level Librarian API
// ============================================================================

use super::fact_store::FactStore;
use std::path::Path;

/// The Librarian - manages fact extraction, storage, and retrieval.
///
/// This struct provides the high-level API for Context Infinity's
/// intelligent context management. It should be owned by the engine
/// and called at appropriate points in the turn lifecycle.
pub struct Librarian {
    store: FactStore,
    api_key: String,
    turn_counter: u64,
}

impl Librarian {
    /// Create a new Librarian with persistent storage.
    pub fn open(path: impl AsRef<Path>, api_key: String) -> Result<Self> {
        let store = FactStore::open(path)?;
        Ok(Self {
            store,
            api_key,
            turn_counter: 0,
        })
    }

    /// Create a new Librarian with in-memory storage (for testing).
    pub fn open_in_memory(api_key: String) -> Result<Self> {
        let store = FactStore::open_in_memory()?;
        Ok(Self {
            store,
            api_key,
            turn_counter: 0,
        })
    }

    /// Set the turn counter (for recovery/loading).
    pub fn set_turn_counter(&mut self, turn: u64) {
        self.turn_counter = turn;
    }

    /// Get the current turn counter.
    #[must_use]
    pub fn turn_counter(&self) -> u64 {
        self.turn_counter
    }

    /// Get the number of stored facts.
    #[must_use]
    pub fn fact_count(&self) -> usize {
        self.store.fact_count()
    }

    /// Get the API key for direct API calls.
    ///
    /// This is used when callers need to make async API calls without
    /// holding the Librarian lock (to avoid Send/Sync issues with SQLite).
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Increment the turn counter.
    ///
    /// Call this before storing facts from a new exchange.
    pub fn increment_turn(&mut self) {
        self.turn_counter += 1;
    }

    /// Store extracted facts (sync operation).
    ///
    /// This is the sync portion of fact storage - call after async extraction.
    pub fn store_facts(&mut self, facts: &[Fact]) -> Result<()> {
        if !facts.is_empty() {
            self.store.store_facts(facts, self.turn_counter)?;
        }
        Ok(())
    }

    /// Store extracted facts and link them to source files.
    ///
    /// This enables staleness detection - facts can be marked as stale
    /// when their source files change externally.
    pub fn store_facts_with_sources(
        &mut self,
        facts: &[Fact],
        source_paths: &[String],
    ) -> Result<()> {
        if facts.is_empty() {
            return Ok(());
        }
        let fact_ids = self.store.store_facts(facts, self.turn_counter)?;
        if !source_paths.is_empty() {
            self.store.link_facts_to_sources(&fact_ids, source_paths)?;
        }
        Ok(())
    }

    /// Pre-flight: Retrieve relevant facts for a user query.
    ///
    /// Call this BEFORE building context for an API call.
    /// Returns facts to inject into the context.
    pub async fn retrieve_context(&self, user_query: &str) -> Result<RetrievalResult> {
        let stored = self.store.get_all_facts()?;
        if stored.is_empty() {
            return Ok(RetrievalResult::default());
        }

        let facts: Vec<Fact> = stored.into_iter().map(|sf| sf.fact).collect();
        retrieve_relevant(&self.api_key, user_query, &facts).await
    }

    /// Post-turn: Extract and store facts from a conversation exchange.
    ///
    /// Call this AFTER an assistant response completes.
    pub async fn extract_and_store(
        &mut self,
        user_message: &str,
        assistant_message: &str,
    ) -> Result<ExtractionResult> {
        self.turn_counter += 1;
        let result = extract_facts(&self.api_key, user_message, assistant_message).await?;

        if !result.facts.is_empty() {
            self.store.store_facts(&result.facts, self.turn_counter)?;
        }

        Ok(result)
    }

    /// Add a user-pinned fact.
    pub fn pin_fact(&mut self, content: &str, entities: &[String]) -> Result<()> {
        self.store
            .add_pinned_fact(content, entities, self.turn_counter)?;
        Ok(())
    }

    /// Get all stored facts (for debugging/inspection).
    pub fn all_facts(&self) -> Result<Vec<Fact>> {
        let stored = self.store.get_all_facts()?;
        Ok(stored.into_iter().map(|sf| sf.fact).collect())
    }

    /// Search facts by keyword.
    pub fn search(&self, keyword: &str) -> Result<Vec<Fact>> {
        let stored = self.store.search_by_entity(keyword)?;
        Ok(stored.into_iter().map(|sf| sf.fact).collect())
    }

    /// Search facts by keyword with staleness information.
    ///
    /// Returns facts along with information about which source files
    /// have changed since the facts were extracted.
    pub fn search_with_staleness(
        &self,
        keyword: &str,
    ) -> Result<Vec<super::fact_store::FactWithStaleness>> {
        self.store.search_with_staleness(keyword)
    }

    /// Clear all facts (for testing/reset).
    pub fn clear(&mut self) -> Result<()> {
        self.store.clear()
    }
}

impl std::fmt::Debug for Librarian {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Librarian")
            .field("turn_counter", &self.turn_counter)
            .field("fact_count", &self.store.fact_count())
            .finish_non_exhaustive() // api_key intentionally hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fact_type_as_str() {
        assert_eq!(FactType::Entity.as_str(), "entity");
        assert_eq!(FactType::Decision.as_str(), "decision");
        assert_eq!(FactType::Constraint.as_str(), "constraint");
        assert_eq!(FactType::CodeState.as_str(), "code_state");
        assert_eq!(FactType::Pinned.as_str(), "pinned");
    }

    #[test]
    fn test_format_facts_empty() {
        let facts: Vec<Fact> = vec![];
        assert_eq!(format_facts_for_context(&facts), "");
    }

    #[test]
    fn test_format_facts_with_content() {
        let facts = vec![
            Fact {
                fact_type: FactType::Entity,
                content: "File src/lib.rs contains main App struct".to_string(),
                entities: vec!["src/lib.rs".to_string(), "App".to_string()],
            },
            Fact {
                fact_type: FactType::Decision,
                content: "Chose async/await for concurrency".to_string(),
                entities: vec!["async".to_string(), "await".to_string()],
            },
        ];

        let formatted = format_facts_for_context(&facts);
        assert!(formatted.contains("Relevant Context"));
        assert!(formatted.contains("📁 File src/lib.rs"));
        assert!(formatted.contains("🔧 Chose async/await"));
    }

    #[test]
    fn test_build_extraction_prompt() {
        let (system, user) = build_extraction_prompt("Help me with X", "Here's how to do X...");

        assert!(system.contains("fact extractor"));
        assert!(system.contains("entity"));
        assert!(system.contains("decision"));
        assert!(user.contains("Help me with X"));
        assert!(user.contains("Here's how to do X"));
    }

    #[test]
    fn test_build_retrieval_prompt() {
        let facts = vec![Fact {
            fact_type: FactType::Entity,
            content: "Test fact".to_string(),
            entities: vec!["test".to_string()],
        }];

        let (system, user) = build_retrieval_prompt("How do I test?", &facts);

        assert!(system.contains("relevance scorer"));
        assert!(user.contains("How do I test?"));
        assert!(user.contains("Test fact"));
    }
}

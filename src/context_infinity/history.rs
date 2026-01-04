//! Full conversation history storage.
//!
//! The history is append-only - messages are never discarded.
//! Summarization adds `Summary` entries and links messages to them,
//! but original messages remain accessible.

use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::time::SystemTime;

use crate::message::{Message, NonEmptyString};

/// Unique identifier for a message in history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(u64);

impl MessageId {
    pub(super) const fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Unique identifier for a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SummaryId(u64);

impl SummaryId {
    pub(super) const fn new(id: u64) -> Self {
        Self(id)
    }
}

/// A message with its computed token count (cached).
#[derive(Debug, Clone)]
pub enum HistoryEntry {
    Original {
        id: MessageId,
        message: Message,
        token_count: u32,
        created_at: SystemTime,
    },
    Summarized {
        id: MessageId,
        message: Message,
        token_count: u32,
        summary_id: SummaryId,
        created_at: SystemTime,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntrySerde {
    id: MessageId,
    message: Message,
    token_count: u32,
    #[serde(default)]
    summary_id: Option<SummaryId>,
    created_at: SystemTime,
}

impl From<&HistoryEntry> for HistoryEntrySerde {
    fn from(entry: &HistoryEntry) -> Self {
        match entry {
            HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
            } => Self {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                summary_id: None,
                created_at: *created_at,
            },
            HistoryEntry::Summarized {
                id,
                message,
                token_count,
                summary_id,
                created_at,
            } => Self {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                summary_id: Some(*summary_id),
                created_at: *created_at,
            },
        }
    }
}

impl From<HistoryEntrySerde> for HistoryEntry {
    fn from(entry: HistoryEntrySerde) -> Self {
        let HistoryEntrySerde {
            id,
            message,
            token_count,
            summary_id,
            created_at,
        } = entry;

        match summary_id {
            Some(summary_id) => HistoryEntry::Summarized {
                id,
                message,
                token_count,
                summary_id,
                created_at,
            },
            None => HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
            },
        }
    }
}

impl Serialize for HistoryEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        HistoryEntrySerde::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HistoryEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entry = HistoryEntrySerde::deserialize(deserializer)?;
        Ok(HistoryEntry::from(entry))
    }
}

impl HistoryEntry {
    pub fn new(id: MessageId, message: Message, token_count: u32) -> Self {
        HistoryEntry::Original {
            id,
            message,
            token_count,
            created_at: SystemTime::now(),
        }
    }

    pub fn id(&self) -> MessageId {
        match self {
            HistoryEntry::Original { id, .. } | HistoryEntry::Summarized { id, .. } => *id,
        }
    }

    pub fn message(&self) -> &Message {
        match self {
            HistoryEntry::Original { message, .. } | HistoryEntry::Summarized { message, .. } => {
                message
            }
        }
    }

    pub fn token_count(&self) -> u32 {
        match self {
            HistoryEntry::Original { token_count, .. }
            | HistoryEntry::Summarized { token_count, .. } => *token_count,
        }
    }

    pub fn summary_id(&self) -> Option<SummaryId> {
        match self {
            HistoryEntry::Summarized { summary_id, .. } => Some(*summary_id),
            HistoryEntry::Original { .. } => None,
        }
    }

    pub fn is_summarized(&self) -> bool {
        matches!(self, HistoryEntry::Summarized { .. })
    }

    pub fn mark_summarized(&mut self, summary_id: SummaryId) {
        let updated = match self {
            HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
            } => HistoryEntry::Summarized {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                summary_id,
                created_at: *created_at,
            },
            HistoryEntry::Summarized {
                id,
                message,
                token_count,
                created_at,
                ..
            } => HistoryEntry::Summarized {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                summary_id,
                created_at: *created_at,
            },
        };

        *self = updated;
    }
}

/// A summary that represents a range of messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    id: SummaryId,
    /// The range of message IDs this summary covers [start, end).
    covers: Range<MessageId>,
    /// The summarized content.
    content: NonEmptyString,
    /// Token count of the summary.
    token_count: u32,
    /// Total tokens of original messages (for compression ratio tracking).
    original_tokens: u32,
    /// When this summary was created.
    created_at: SystemTime,
    /// Which model generated this summary.
    generated_by: String,
}

impl Summary {
    pub fn new(
        id: SummaryId,
        covers: Range<MessageId>,
        content: NonEmptyString,
        token_count: u32,
        original_tokens: u32,
        generated_by: String,
    ) -> Self {
        Self {
            id,
            covers,
            content,
            token_count,
            original_tokens,
            created_at: SystemTime::now(),
            generated_by,
        }
    }

    pub fn id(&self) -> SummaryId {
        self.id
    }

    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    #[cfg(test)]
    /// Compression ratio (summary tokens / original tokens).
    /// Lower is better compression.
    pub fn compression_ratio(&self) -> f32 {
        if self.original_tokens == 0 {
            1.0
        } else {
            self.token_count as f32 / self.original_tokens as f32
        }
    }

    #[cfg(test)]
    /// Tokens saved by using this summary instead of originals.
    pub fn tokens_saved(&self) -> u32 {
        self.original_tokens.saturating_sub(self.token_count)
    }
}

/// Complete conversation history - append-only, never discards messages.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FullHistory {
    entries: Vec<HistoryEntry>,
    summaries: Vec<Summary>,
    next_message_id: u64,
    next_summary_id: u64,
}

impl FullHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a message to history, returns its ID.
    pub fn push(&mut self, message: Message, token_count: u32) -> MessageId {
        let id = MessageId::new(self.next_message_id);
        self.next_message_id += 1;
        self.entries
            .push(HistoryEntry::new(id, message, token_count));
        id
    }

    /// Add a summary for a range of messages.
    pub fn add_summary(&mut self, summary: Summary) -> SummaryId {
        let id = summary.id;

        // Mark covered messages as summarized
        let start = summary.covers.start.as_u64();
        let end = summary.covers.end.as_u64();
        for entry in &mut self.entries {
            let entry_id = entry.id().as_u64();
            if entry_id >= start && entry_id < end {
                entry.mark_summarized(id);
            }
        }

        self.summaries.push(summary);
        id
    }

    /// Allocate a new summary ID without creating the summary yet.
    pub fn allocate_summary_id(&mut self) -> SummaryId {
        let id = SummaryId::new(self.next_summary_id);
        self.next_summary_id += 1;
        id
    }

    /// Get all history entries.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Get a specific entry by ID.
    pub fn get_entry(&self, id: MessageId) -> Option<&HistoryEntry> {
        let index = id.as_u64() as usize;
        match self.entries.get(index) {
            Some(entry) if entry.id() == id => Some(entry),
            _ => self.entries.iter().find(|e| e.id() == id),
        }
    }

    /// Get a specific summary by ID.
    pub fn get_summary(&self, id: SummaryId) -> Option<&Summary> {
        self.summaries.iter().find(|s| s.id() == id)
    }

    /// Total tokens across all original messages.
    pub fn total_tokens(&self) -> u32 {
        self.entries.iter().map(|e| e.token_count()).sum()
    }

    /// Number of messages in history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Count of summarized messages.
    pub fn summarized_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_summarized()).count()
    }

    #[cfg(test)]
    /// Get the most recent N entries.
    pub fn recent_entries(&self, n: usize) -> &[HistoryEntry] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_message(content: &str) -> Message {
        Message::try_user(content).expect("non-empty test message")
    }

    #[test]
    fn test_message_id() {
        let id = MessageId::new(42);
        assert_eq!(id.as_u64(), 42);
    }

    #[test]
    fn test_history_push() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("Hello"), 10);
        let id2 = history.push(make_test_message("World"), 10);

        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);
        assert_eq!(history.len(), 2);
        assert_eq!(history.total_tokens(), 20);
    }

    #[test]
    fn test_summary_creation() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 100);
        let id2 = history.push(make_test_message("Second"), 100);
        let _id3 = history.push(make_test_message("Third"), 100);

        let summary_id = history.allocate_summary_id();
        let summary = Summary::new(
            summary_id,
            id1..MessageId::new(id2.as_u64() + 1),
            NonEmptyString::new("Summary of first two").expect("non-empty summary"),
            30,
            200,
            "test-model".to_string(),
        );

        history.add_summary(summary);

        // First two should be summarized
        assert!(history.get_entry(id1).unwrap().is_summarized());
        assert!(history.get_entry(id2).unwrap().is_summarized());
        // Third should not
        assert!(!history.entries()[2].is_summarized());

        assert_eq!(history.summarized_count(), 2);
    }

    #[test]
    fn test_compression_ratio() {
        let summary = Summary::new(
            SummaryId::new(0),
            MessageId::new(0)..MessageId::new(10),
            NonEmptyString::new("Summary").expect("non-empty summary"),
            50,
            500,
            "test-model".to_string(),
        );

        assert!((summary.compression_ratio() - 0.1).abs() < 0.001);
        assert_eq!(summary.tokens_saved(), 450);
    }

    #[test]
    fn test_recent_entries() {
        let mut history = FullHistory::new();

        for i in 0..10 {
            history.push(make_test_message(&format!("Message {}", i)), 10);
        }

        let recent = history.recent_entries(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].message().content(), "Message 7");
        assert_eq!(recent[2].message().content(), "Message 9");
    }
}

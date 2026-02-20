//! Full conversation history storage.
//!
//! The history is append-only — messages are never discarded.
//! When context is exhausted, a compaction point is set: messages before
//! it become display-only, and a summary replaces them for API calls.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use forge_types::{Message, MessageId, NonEmptyString, PersistableContent, StepId};

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    id: MessageId,
    message: Message,
    token_count: u32,
    created_at: SystemTime,
    /// Stream journal step ID for crash recovery linkage (assistant messages only)
    stream_step_id: Option<StepId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntrySerde {
    id: MessageId,
    message: Message,
    token_count: u32,
    created_at: SystemTime,
    #[serde(default)]
    stream_step_id: Option<StepId>,
}

impl From<&HistoryEntry> for HistoryEntrySerde {
    fn from(entry: &HistoryEntry) -> Self {
        Self {
            id: entry.id,
            message: entry.message.normalized_for_persistence(),
            token_count: entry.token_count,
            created_at: entry.created_at,
            stream_step_id: entry.stream_step_id,
        }
    }
}

impl From<HistoryEntrySerde> for HistoryEntry {
    fn from(entry: HistoryEntrySerde) -> Self {
        HistoryEntry {
            id: entry.id,
            message: entry.message,
            token_count: entry.token_count,
            created_at: entry.created_at,
            stream_step_id: entry.stream_step_id,
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
    #[must_use]
    pub fn new(id: MessageId, message: Message, token_count: u32) -> Self {
        Self {
            id,
            message,
            token_count,
            created_at: SystemTime::now(),
            stream_step_id: None,
        }
    }

    #[must_use]
    pub fn new_with_step_id(
        id: MessageId,
        message: Message,
        token_count: u32,
        stream_step_id: StepId,
    ) -> Self {
        Self {
            id,
            message,
            token_count,
            created_at: SystemTime::now(),
            stream_step_id: Some(stream_step_id),
        }
    }

    #[must_use]
    pub fn id(&self) -> MessageId {
        self.id
    }

    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    #[must_use]
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    #[must_use]
    pub fn stream_step_id(&self) -> Option<StepId> {
        self.stream_step_id
    }
}

/// A compaction summary that replaces all messages before the compaction point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSummary {
    content: NonEmptyString,
    token_count: u32,
    created_at: SystemTime,
    generated_by: String,
}

impl CompactionSummary {
    #[must_use]
    pub fn new(content: NonEmptyString, token_count: u32, generated_by: String) -> Self {
        Self {
            content,
            token_count,
            created_at: SystemTime::now(),
            generated_by,
        }
    }

    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    #[must_use]
    pub fn content_non_empty(&self) -> &NonEmptyString {
        &self.content
    }

    #[must_use]
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    fn normalized_for_persistence(&self) -> Self {
        let content = match PersistableContent::normalize_borrowed(self.content.as_str()) {
            std::borrow::Cow::Borrowed(_) => self.content.clone(),
            std::borrow::Cow::Owned(normalized) => {
                NonEmptyString::new(normalized).unwrap_or_else(|_| self.content.clone())
            }
        };
        Self {
            content,
            token_count: self.token_count,
            created_at: self.created_at,
            generated_by: self.generated_by.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub struct FullHistory {
    entries: Vec<HistoryEntry>,
    next_message_id: u64,
    /// Index into `entries` — messages before this index are display-only
    /// and not sent to the API. The compaction summary replaces them.
    compaction_point: Option<usize>,
    compaction_summary: Option<CompactionSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FullHistorySerde {
    entries: Vec<HistoryEntrySerde>,
    next_message_id: u64,
    #[serde(default)]
    compaction_point: Option<usize>,
    #[serde(default)]
    compaction_summary: Option<CompactionSummary>,
}

impl From<&FullHistory> for FullHistorySerde {
    fn from(history: &FullHistory) -> Self {
        Self {
            entries: history
                .entries
                .iter()
                .map(HistoryEntrySerde::from)
                .collect(),
            next_message_id: history.next_message_id,
            compaction_point: history.compaction_point,
            compaction_summary: history
                .compaction_summary
                .as_ref()
                .map(CompactionSummary::normalized_for_persistence),
        }
    }
}

impl Serialize for FullHistory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        FullHistorySerde::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FullHistory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let serde = FullHistorySerde::deserialize(deserializer)?;
        serde.into_history().map_err(serde::de::Error::custom)
    }
}

impl FullHistorySerde {
    fn into_history(self) -> Result<FullHistory, String> {
        let expected_next_message_id = self.entries.len() as u64;
        if self.next_message_id != expected_next_message_id {
            return Err(format!(
                "next_message_id {} does not match entry count {expected_next_message_id}",
                self.next_message_id
            ));
        }

        for (index, entry) in self.entries.iter().enumerate() {
            let expected_id = index as u64;
            if entry.id.value() != expected_id {
                return Err(format!(
                    "entry id {} does not match position {}",
                    entry.id.value(),
                    expected_id
                ));
            }
        }

        if let Some(cp) = self.compaction_point
            && cp > self.entries.len()
        {
            return Err(format!(
                "compaction_point {cp} exceeds entry count {}",
                self.entries.len()
            ));
        }

        let entries = self.entries.into_iter().map(HistoryEntry::from).collect();

        Ok(FullHistory {
            entries,
            next_message_id: self.next_message_id,
            compaction_point: self.compaction_point,
            compaction_summary: self.compaction_summary,
        })
    }
}

impl FullHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message, token_count: u32) -> MessageId {
        let id = MessageId::new(self.next_message_id);
        self.next_message_id += 1;
        self.entries
            .push(HistoryEntry::new(id, message, token_count));
        id
    }

    pub fn push_with_step_id(
        &mut self,
        message: Message,
        token_count: u32,
        stream_step_id: StepId,
    ) -> MessageId {
        let id = MessageId::new(self.next_message_id);
        self.next_message_id += 1;
        self.entries.push(HistoryEntry::new_with_step_id(
            id,
            message,
            token_count,
            stream_step_id,
        ));
        id
    }

    #[must_use]
    pub fn has_step_id(&self, step_id: StepId) -> bool {
        self.entries
            .iter()
            .any(|e| e.stream_step_id() == Some(step_id))
    }

    #[must_use]
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Entries visible to the API (after the compaction point).
    #[must_use]
    pub fn api_entries(&self) -> &[HistoryEntry] {
        let start = self.compaction_point.unwrap_or(0);
        &self.entries[start..]
    }

    #[must_use]
    pub fn compaction_summary(&self) -> Option<&CompactionSummary> {
        self.compaction_summary.as_ref()
    }

    #[must_use]
    pub fn is_compacted(&self) -> bool {
        self.compaction_point.is_some()
    }

    #[must_use]
    pub fn get_entry(&self, id: MessageId) -> &HistoryEntry {
        let index = id.value() as usize;
        &self.entries[index]
    }

    /// Total tokens across all entries visible to the API.
    pub fn api_tokens(&self) -> u32 {
        self.api_entries()
            .iter()
            .map(HistoryEntry::token_count)
            .sum()
    }

    /// Total tokens across all entries (including display-only).
    pub fn total_tokens(&self) -> u32 {
        self.entries.iter().map(HistoryEntry::token_count).sum()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn pop_if_last(&mut self, id: MessageId) -> Option<Message> {
        let last = self.entries.last()?;
        if last.id() != id {
            return None;
        }

        let entry = self.entries.pop()?;
        self.next_message_id = self.entries.len() as u64;
        Some(entry.message().clone())
    }

    /// All existing entries become display-only. New entries after this point
    /// are API-visible, prefixed by the summary.
    pub fn compact(&mut self, summary: CompactionSummary) {
        self.compaction_point = Some(self.entries.len());
        self.compaction_summary = Some(summary);
    }

    #[cfg(test)]
    #[must_use]
    pub fn recent_entries(&self, n: usize) -> &[HistoryEntry] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{CompactionSummary, FullHistory};
    use forge_types::{Message, MessageId, NonEmptyString};

    fn make_test_message(content: &str) -> Message {
        Message::try_user(content, SystemTime::now()).expect("non-empty test message")
    }

    #[test]
    fn test_message_id() {
        let id = MessageId::new(42);
        assert_eq!(id.value(), 42);
    }

    #[test]
    fn test_history_push() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("Hello"), 10);
        let id2 = history.push(make_test_message("World"), 10);

        assert_eq!(id1.value(), 0);
        assert_eq!(id2.value(), 1);
        assert_eq!(history.len(), 2);
        assert_eq!(history.total_tokens(), 20);
    }

    #[test]
    fn test_recent_entries() {
        let mut history = FullHistory::new();

        for i in 0..10 {
            history.push(make_test_message(&format!("Message {i}")), 10);
        }

        let recent = history.recent_entries(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].message().content(), "Message 7");
        assert_eq!(recent[2].message().content(), "Message 9");
    }

    #[test]
    fn test_pop_if_last_success() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 10);
        let id2 = history.push(make_test_message("Second"), 20);

        assert_eq!(history.len(), 2);

        let popped = history.pop_if_last(id2);
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().content(), "Second");
        assert_eq!(history.len(), 1);

        let popped = history.pop_if_last(id1);
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().content(), "First");
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_pop_if_last_wrong_id() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 10);
        let _id2 = history.push(make_test_message("Second"), 20);

        let popped = history.pop_if_last(id1);
        assert!(popped.is_none());
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_pop_if_last_empty_history() {
        let mut history = FullHistory::new();

        let popped = history.pop_if_last(MessageId::new(0));
        assert!(popped.is_none());
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_pop_if_last_updates_next_id() {
        let mut history = FullHistory::new();

        let _id1 = history.push(make_test_message("First"), 10);
        let id2 = history.push(make_test_message("Second"), 20);

        history.pop_if_last(id2);

        let id3 = history.push(make_test_message("Third"), 30);
        assert_eq!(id3.value(), 1);
    }

    #[test]
    fn test_compact() {
        let mut history = FullHistory::new();

        history.push(make_test_message("Old 1"), 100);
        history.push(make_test_message("Old 2"), 200);

        assert!(!history.is_compacted());
        assert_eq!(history.api_entries().len(), 2);
        assert_eq!(history.api_tokens(), 300);

        let summary = CompactionSummary::new(
            NonEmptyString::new("Summary of old messages".to_string()).unwrap(),
            50,
            "test-model".to_string(),
        );
        history.compact(summary);

        assert!(history.is_compacted());
        assert_eq!(history.len(), 2);
        assert_eq!(history.api_entries().len(), 0);
        assert_eq!(history.api_tokens(), 0);
        assert_eq!(history.compaction_summary().unwrap().token_count(), 50);

        history.push(make_test_message("New 1"), 75);

        assert_eq!(history.len(), 3);
        assert_eq!(history.api_entries().len(), 1);
        assert_eq!(history.api_tokens(), 75);
        assert_eq!(history.entries().len(), 3);
    }
}

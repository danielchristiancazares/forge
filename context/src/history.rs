//! Full conversation history storage.
//!
//! The history is append-only - messages are never discarded.
//! Distillation adds `Distillate` entries and links messages to them,
//! but original messages remain accessible.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::time::SystemTime;

use forge_types::{Message, NonEmptyString};

use crate::StepId;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(u64);

impl MessageId {
    const fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test(id: u64) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DistillateId(u64);

impl DistillateId {
    const fn new(id: u64) -> Self {
        Self(id)
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test(id: u64) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone)]
pub enum HistoryEntry {
    Original {
        id: MessageId,
        message: Message,
        token_count: u32,
        created_at: SystemTime,
        /// Stream journal step ID for crash recovery linkage (assistant messages only)
        stream_step_id: Option<StepId>,
    },
    Distilled {
        id: MessageId,
        message: Message,
        token_count: u32,
        distillate_id: DistillateId,
        created_at: SystemTime,
        stream_step_id: Option<StepId>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntrySerde {
    id: MessageId,
    message: Message,
    token_count: u32,
    #[serde(default)]
    distillate_id: Option<DistillateId>,
    created_at: SystemTime,
    #[serde(default)]
    stream_step_id: Option<StepId>,
}

impl From<&HistoryEntry> for HistoryEntrySerde {
    fn from(entry: &HistoryEntry) -> Self {
        match entry {
            HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
                stream_step_id,
            } => Self {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                distillate_id: None,
                created_at: *created_at,
                stream_step_id: *stream_step_id,
            },
            HistoryEntry::Distilled {
                id,
                message,
                token_count,
                distillate_id,
                created_at,
                stream_step_id,
            } => Self {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                distillate_id: Some(*distillate_id),
                created_at: *created_at,
                stream_step_id: *stream_step_id,
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
            distillate_id,
            created_at,
            stream_step_id,
        } = entry;

        match distillate_id {
            Some(distillate_id) => HistoryEntry::Distilled {
                id,
                message,
                token_count,
                distillate_id,
                created_at,
                stream_step_id,
            },
            None => HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
                stream_step_id,
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
    #[must_use]
    pub fn new(id: MessageId, message: Message, token_count: u32) -> Self {
        HistoryEntry::Original {
            id,
            message,
            token_count,
            created_at: SystemTime::now(),
            stream_step_id: None,
        }
    }

    /// Used for assistant messages from streaming responses to enable
    /// idempotent crash recovery.
    #[must_use]
    pub fn new_with_step_id(
        id: MessageId,
        message: Message,
        token_count: u32,
        stream_step_id: StepId,
    ) -> Self {
        HistoryEntry::Original {
            id,
            message,
            token_count,
            created_at: SystemTime::now(),
            stream_step_id: Some(stream_step_id),
        }
    }

    #[must_use]
    pub fn id(&self) -> MessageId {
        match self {
            HistoryEntry::Original { id, .. } | HistoryEntry::Distilled { id, .. } => *id,
        }
    }

    #[must_use]
    pub fn message(&self) -> &Message {
        match self {
            HistoryEntry::Original { message, .. } | HistoryEntry::Distilled { message, .. } => {
                message
            }
        }
    }

    #[must_use]
    pub fn token_count(&self) -> u32 {
        match self {
            HistoryEntry::Original { token_count, .. }
            | HistoryEntry::Distilled { token_count, .. } => *token_count,
        }
    }

    #[must_use]
    pub fn distillate_id(&self) -> Option<DistillateId> {
        match self {
            HistoryEntry::Distilled { distillate_id, .. } => Some(*distillate_id),
            HistoryEntry::Original { .. } => None,
        }
    }

    #[must_use]
    pub fn stream_step_id(&self) -> Option<StepId> {
        match self {
            HistoryEntry::Original { stream_step_id, .. }
            | HistoryEntry::Distilled { stream_step_id, .. } => *stream_step_id,
        }
    }

    #[must_use]
    pub fn is_distilled(&self) -> bool {
        matches!(self, HistoryEntry::Distilled { .. })
    }

    pub fn mark_distilled(&mut self, distillate_id: DistillateId) {
        let updated = match self {
            HistoryEntry::Original {
                id,
                message,
                token_count,
                created_at,
                stream_step_id,
            }
            | HistoryEntry::Distilled {
                id,
                message,
                token_count,
                created_at,
                stream_step_id,
                ..
            } => HistoryEntry::Distilled {
                id: *id,
                message: message.clone(),
                token_count: *token_count,
                distillate_id,
                created_at: *created_at,
                stream_step_id: *stream_step_id,
            },
        };

        *self = updated;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Distillate {
    id: DistillateId,
    covers: Range<MessageId>,
    content: NonEmptyString,
    token_count: u32,
    original_tokens: u32,
    created_at: SystemTime,
    generated_by: String,
}

impl Distillate {
    #[must_use]
    pub fn new(
        id: DistillateId,
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

    #[cfg(test)]
    #[must_use]
    /// Lower is better compression.
    pub fn compression_ratio(&self) -> f32 {
        if self.original_tokens == 0 {
            1.0
        } else {
            self.token_count as f32 / self.original_tokens as f32
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn tokens_saved(&self) -> u32 {
        self.original_tokens.saturating_sub(self.token_count)
    }
}

#[derive(Debug, Default)]
pub struct FullHistory {
    entries: Vec<HistoryEntry>,
    distillates: Vec<Distillate>,
    next_message_id: u64,
    next_distillate_id: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct FullHistorySerde {
    entries: Vec<HistoryEntrySerde>,
    distillates: Vec<Distillate>,
    next_message_id: u64,
    next_distillate_id: u64,
}

impl From<&FullHistory> for FullHistorySerde {
    fn from(history: &FullHistory) -> Self {
        Self {
            entries: history
                .entries
                .iter()
                .map(HistoryEntrySerde::from)
                .collect(),
            distillates: history.distillates.clone(),
            next_message_id: history.next_message_id,
            next_distillate_id: history.next_distillate_id,
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
        let FullHistorySerde {
            entries,
            distillates,
            next_message_id,
            next_distillate_id,
        } = self;

        let expected_next_message_id = entries.len() as u64;
        if next_message_id != expected_next_message_id {
            return Err(format!(
                "next_message_id {next_message_id} does not match entry count {expected_next_message_id}"
            ));
        }

        let expected_next_distillate_id = distillates.len() as u64;
        if next_distillate_id != expected_next_distillate_id {
            return Err(format!(
                "next_distillate_id {next_distillate_id} does not match Distillate count {expected_next_distillate_id}"
            ));
        }

        for (index, distillate) in distillates.iter().enumerate() {
            let expected_id = index as u64;
            if distillate.id.0 != expected_id {
                return Err(format!(
                    "distillate id {} does not match position {}",
                    distillate.id.0, expected_id
                ));
            }

            let start = distillate.covers.start.as_u64();
            let end = distillate.covers.end.as_u64();
            if start > end {
                return Err(format!(
                    "distillate id {} has invalid range {}..{}",
                    distillate.id.0, start, end
                ));
            }
            if end > next_message_id {
                return Err(format!(
                    "distillate id {} covers past last message ({})",
                    distillate.id.0, next_message_id
                ));
            }
        }

        for (index, entry) in entries.iter().enumerate() {
            let expected_id = index as u64;
            if entry.id.as_u64() != expected_id {
                return Err(format!(
                    "entry id {} does not match position {}",
                    entry.id.as_u64(),
                    expected_id
                ));
            }

            if let Some(distillate_id) = entry.distillate_id {
                let distillate_index = distillate_id.0 as usize;
                if distillate_index >= distillates.len() {
                    return Err(format!(
                        "entry {} references missing Distillate {}",
                        entry.id.as_u64(),
                        distillate_id.0
                    ));
                }

                let distillate = &distillates[distillate_index];
                let entry_id = entry.id.as_u64();
                let start = distillate.covers.start.as_u64();
                let end = distillate.covers.end.as_u64();
                if entry_id < start || entry_id >= end {
                    return Err(format!(
                        "entry {} references distillate {} but is outside {}..{}",
                        entry_id, distillate_id.0, start, end
                    ));
                }
            }
        }

        let entries = entries.into_iter().map(HistoryEntry::from).collect();

        Ok(FullHistory {
            entries,
            distillates,
            next_message_id,
            next_distillate_id,
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

    /// Under normal operation, distillates should only cover previously-undistilled...
    pub fn add_distillate(&mut self, distillate: Distillate) -> Result<DistillateId> {
        let expected_id = DistillateId::new(self.distillates.len() as u64);
        if distillate.id != expected_id {
            return Err(anyhow!(
                "distillate id {} does not match expected id {}",
                distillate.id.0,
                expected_id.0
            ));
        }

        let start = distillate.covers.start.as_u64();
        let end = distillate.covers.end.as_u64();
        if start >= end {
            return Err(anyhow!(
                "Distillate id {} has invalid range {}..{}",
                distillate.id.0,
                start,
                end
            ));
        }
        if end > self.entries.len() as u64 {
            return Err(anyhow!(
                "distillate id {} covers past last message ({})",
                distillate.id.0,
                self.entries.len()
            ));
        }

        // Check for already-Distilled messages in the range.
        let mut orphaned_distillates: Vec<DistillateId> = Vec::new();
        for entry in &self.entries {
            let entry_id = entry.id().as_u64();
            if entry_id >= start
                && entry_id < end
                && let Some(old_distillate_id) = entry.distillate_id()
                && !orphaned_distillates.contains(&old_distillate_id)
            {
                orphaned_distillates.push(old_distillate_id);
            }
        }

        if !orphaned_distillates.is_empty() {
            // Log but proceed - the old distillates will become orphaned.
            // This indicates either hierarchical distillation or a bug.
            tracing::warn!(
                "distillate {} overlaps with existing distillates {:?}. \
                 Old distillates will become orphaned.",
                distillate.id.0,
                orphaned_distillates.iter().map(|s| s.0).collect::<Vec<_>>()
            );
        }

        // Mark covered messages as distilled (overwrites any previous distillate_id).
        for entry in &mut self.entries {
            let entry_id = entry.id().as_u64();
            if entry_id >= start && entry_id < end {
                entry.mark_distilled(distillate.id);
            }
        }

        self.distillates.push(distillate);
        self.next_distillate_id = self.distillates.len() as u64;
        Ok(expected_id)
    }

    #[must_use]
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    #[must_use]
    pub fn distillates_len(&self) -> usize {
        self.distillates.len()
    }

    #[must_use]
    pub fn next_distillate_id(&self) -> DistillateId {
        DistillateId::new(self.distillates.len() as u64)
    }

    #[must_use]
    pub fn get_entry(&self, id: MessageId) -> &HistoryEntry {
        let index = id.as_u64() as usize;
        &self.entries[index]
    }

    #[must_use]
    pub fn distillate(&self, id: DistillateId) -> &Distillate {
        &self.distillates[id.0 as usize]
    }

    /// Total tokens across all original messages.
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

    /// This is used for transactional rollback when a stream fails...
    pub fn pop_if_last(&mut self, id: MessageId) -> Option<Message> {
        let last = self.entries.last()?;
        if last.id() != id {
            return None;
        }

        // Verify the entry is not Distilled - we should never rollback Distilled messages
        if last.is_distilled() {
            tracing::warn!("Attempted to rollback Distilled message {:?}, refusing", id);
            return None;
        }

        let entry = self.entries.pop()?;
        self.next_message_id = self.entries.len() as u64;
        Some(entry.message().clone())
    }

    #[must_use]
    pub fn distilled_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_distilled()).count()
    }

    /// Find orphaned distillates (distillates with no messages referencing them).
    ///
    /// Under normal operation, this should return an empty vector. Non-empty
    /// results indicate either hierarchical re-distillation occurred or a bug.
    #[must_use]
    pub fn orphaned_distillates(&self) -> Vec<DistillateId> {
        let mut referenced: std::collections::HashSet<DistillateId> =
            std::collections::HashSet::new();

        for entry in &self.entries {
            if let Some(distillate_id) = entry.distillate_id() {
                referenced.insert(distillate_id);
            }
        }

        self.distillates
            .iter()
            .map(|s| s.id)
            .filter(|id| !referenced.contains(id))
            .collect()
    }

    #[cfg(test)]
    #[must_use]
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
    fn test_distillate_creation() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 100);
        let id2 = history.push(make_test_message("Second"), 100);
        let _id3 = history.push(make_test_message("Third"), 100);

        let distillate_id = DistillateId::new(history.distillates_len() as u64);
        let distillate = Distillate::new(
            distillate_id,
            id1..MessageId::new(id2.as_u64() + 1),
            NonEmptyString::new("Distillate of first two").expect("non-empty Distillate"),
            30,
            200,
            "test-model".to_string(),
        );

        history.add_distillate(distillate).expect("Distillate add");

        // First two should be Distilled
        assert!(history.get_entry(id1).is_distilled());
        assert!(history.get_entry(id2).is_distilled());
        // Third should not
        assert!(!history.entries()[2].is_distilled());

        assert_eq!(history.distilled_count(), 2);
    }

    #[test]
    fn test_compression_ratio() {
        let distillate = Distillate::new(
            DistillateId::new(0),
            MessageId::new(0)..MessageId::new(10),
            NonEmptyString::new("Distillate").expect("non-empty Distillate"),
            50,
            500,
            "test-model".to_string(),
        );

        assert!((distillate.compression_ratio() - 0.1).abs() < 0.001);
        assert_eq!(distillate.tokens_saved(), 450);
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
    fn test_orphaned_distillates_none_initially() {
        let mut history = FullHistory::new();

        history.push(make_test_message("First"), 100);
        history.push(make_test_message("Second"), 100);

        // No distillates yet, so no orphans
        assert!(history.orphaned_distillates().is_empty());

        // Add a Distillate
        let distillate_id = history.next_distillate_id();
        let distillate = Distillate::new(
            distillate_id,
            MessageId::new(0)..MessageId::new(2),
            NonEmptyString::new("Distillate").expect("non-empty"),
            30,
            200,
            "test-model".to_string(),
        );
        history.add_distillate(distillate).expect("add Distillate");

        // Distillate is referenced, so still no orphans
        assert!(history.orphaned_distillates().is_empty());
    }

    #[test]
    fn test_overlapping_distillate_creates_orphan() {
        let mut history = FullHistory::new();

        // Add 4 messages
        for i in 0..4 {
            history.push(make_test_message(&format!("Message {i}")), 100);
        }

        // Create first Distillate covering messages 0-2
        let distillate0_id = history.next_distillate_id();
        let distillate0 = Distillate::new(
            distillate0_id,
            MessageId::new(0)..MessageId::new(2),
            NonEmptyString::new("First Distillate").expect("non-empty"),
            30,
            200,
            "test-model".to_string(),
        );
        history
            .add_distillate(distillate0)
            .expect("add Distillate 0");

        // Verify messages 0-1 point to Distillate 0
        assert_eq!(
            history.get_entry(MessageId::new(0)).distillate_id(),
            Some(distillate0_id)
        );
        assert_eq!(
            history.get_entry(MessageId::new(1)).distillate_id(),
            Some(distillate0_id)
        );
        assert!(history.orphaned_distillates().is_empty());

        // Create overlapping Distillate covering messages 0-4 (includes already-Distilled 0-1)
        let distillate1_id = history.next_distillate_id();
        let distillate1 = Distillate::new(
            distillate1_id,
            MessageId::new(0)..MessageId::new(4),
            NonEmptyString::new("Bigger Distillate").expect("non-empty"),
            50,
            400,
            "test-model".to_string(),
        );
        history
            .add_distillate(distillate1)
            .expect("add Distillate 1");

        // Now messages 0-3 should point to Distillate 1
        for i in 0..4 {
            assert_eq!(
                history.get_entry(MessageId::new(i)).distillate_id(),
                Some(distillate1_id),
                "message {i} should point to Distillate 1"
            );
        }

        // Distillate 0 should now be orphaned (no messages reference it)
        let orphans = history.orphaned_distillates();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0], distillate0_id);
    }

    #[test]
    fn test_pop_if_last_success() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 10);
        let id2 = history.push(make_test_message("Second"), 20);

        assert_eq!(history.len(), 2);

        // Pop the last message
        let popped = history.pop_if_last(id2);
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().content(), "Second");
        assert_eq!(history.len(), 1);

        // Pop the remaining message
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

        // Try to pop with wrong ID (not the last message)
        let popped = history.pop_if_last(id1);
        assert!(popped.is_none());
        assert_eq!(history.len(), 2); // Nothing was removed
    }

    #[test]
    fn test_pop_if_last_empty_history() {
        let mut history = FullHistory::new();

        // Try to pop from empty history
        let popped = history.pop_if_last(MessageId::new_for_test(0));
        assert!(popped.is_none());
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_pop_if_last_refuses_distilled() {
        let mut history = FullHistory::new();

        let id1 = history.push(make_test_message("First"), 100);
        let id2 = history.push(make_test_message("Second"), 100);

        // Create a Distillate covering both messages
        let distillate_id = history.next_distillate_id();
        let distillate = Distillate::new(
            distillate_id,
            id1..MessageId::new(id2.as_u64() + 1),
            NonEmptyString::new("Distillate").expect("non-empty"),
            30,
            200,
            "test-model".to_string(),
        );
        history.add_distillate(distillate).expect("add Distillate");

        // Both messages should now be Distilled
        assert!(history.get_entry(id2).is_distilled());

        // Try to pop the last (Distilled) message - should refuse
        let popped = history.pop_if_last(id2);
        assert!(popped.is_none());
        assert_eq!(history.len(), 2); // Nothing was removed
    }

    #[test]
    fn test_pop_if_last_updates_next_id() {
        let mut history = FullHistory::new();

        let _id1 = history.push(make_test_message("First"), 10);
        let id2 = history.push(make_test_message("Second"), 20);

        // Pop the last message
        history.pop_if_last(id2);

        // Push a new message - should get the recycled ID
        let id3 = history.push(make_test_message("Third"), 30);
        assert_eq!(id3.as_u64(), 1); // Same ID as the popped message
    }
}

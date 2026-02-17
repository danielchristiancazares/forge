//! Display items for the message view.

use forge_types::Message;

use forge_context::MessageId;

/// An item to display in the message view.
///
/// Display items can either reference persisted history entries by ID,
/// or contain local messages that haven't been persisted yet.
#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// A message from persisted history, referenced by ID.
    History(MessageId),
    /// A local message not yet in history (e.g., error messages, system notices).
    Local(Message),
}

/// Display log with a monotonic revision counter.
///
/// This pairs the display buffer with a revision number that is bumped whenever
/// the buffer mutates. The TUI uses the revision as a cache key.
///
/// Keeping the revision with the buffer makes it impossible to forget to bump
/// it after a mutation (IFA: cached derived values should be owned and updated
/// by the same authority).
#[derive(Debug, Clone, Default)]
pub(crate) struct DisplayLog {
    items: Vec<DisplayItem>,
    revision: usize,
}

impl DisplayLog {
    #[inline]
    pub(crate) fn items(&self) -> &[DisplayItem] {
        &self.items
    }

    #[inline]
    pub(crate) fn revision(&self) -> usize {
        self.revision
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    #[inline]
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, DisplayItem> {
        self.items.iter()
    }

    #[inline]
    pub(crate) fn last(&self) -> Option<&DisplayItem> {
        self.items.last()
    }

    pub(crate) fn clear(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.items.clear();
        self.bump();
    }

    pub(crate) fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
        self.bump();
    }

    pub(crate) fn pop(&mut self) -> Option<DisplayItem> {
        let popped = self.items.pop();
        if popped.is_some() {
            self.bump();
        }
        popped
    }

    pub(crate) fn set_items(&mut self, items: Vec<DisplayItem>) {
        self.items = items;
        self.bump();
    }

    #[inline]
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

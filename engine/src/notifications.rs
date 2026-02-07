//! System notifications for trusted agent-to-model communication.
//!
//! This module provides a secure channel for Forge to communicate system-level
//! information to the LLM. Notifications are injected as assistant messages,
//! which cannot be forged by user input, files, or tool outputs.
//!
//! # Security
//!
//! Unlike system reminders embedded in user content (which are vulnerable to
//! prompt injection), assistant messages can only come from:
//! - API responses (trusted)
//! - Forge's injection layer (this module)
//!
//! This creates a clean trust boundary: the small, finite set of notification
//! variants defined here are trusted; everything else is untrusted.

/// A system notification that Forge can inject into the conversation.
///
/// This is a closed enum - only Forge code can construct these variants.
/// Each variant represents a specific system event that the model should
/// be aware of. Only events that affect model behavior belong here;
/// pure UI feedback should use `push_notification` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemNotification {
    /// User approved tool calls.
    ToolsApproved {
        /// Number of tools approved.
        count: u8,
    },
    /// User denied tool calls.
    ToolsDenied {
        /// Number of tools denied.
        count: u8,
    },
    /// Compiler/linter diagnostics found in recently edited files.
    DiagnosticsFound {
        /// Pre-formatted summary (e.g. "src/main.rs:42: error: expected `;`").
        summary: String,
    },
}

impl SystemNotification {
    /// Format the notification as a human-readable string.
    ///
    /// All notifications are prefixed with `[System: ...]` to clearly mark
    /// them as system-level messages distinct from user or model content.
    #[must_use]
    pub fn format(&self) -> String {
        match self {
            Self::ToolsApproved { count } => {
                format!("[System: User approved {count} tool call(s)]")
            }
            Self::ToolsDenied { count } => {
                format!("[System: User denied {count} tool call(s)]")
            }
            Self::DiagnosticsFound { summary } => {
                format!("[System: Compiler errors detected]\n{summary}")
            }
        }
    }
}

/// Queue for pending system notifications.
///
/// Notifications are accumulated here and drained at the start of each
/// API request, then injected as an assistant message.
#[derive(Debug, Default)]
pub struct NotificationQueue {
    pending: Vec<SystemNotification>,
}

impl NotificationQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a notification to the queue.
    ///
    /// Duplicate notifications are deduplicated to avoid redundant messages.
    pub fn push(&mut self, notification: SystemNotification) {
        // Deduplicate: don't add if already present
        if !self.pending.contains(&notification) {
            self.pending.push(notification);
        }
    }

    /// Take all pending notifications, clearing the queue.
    ///
    /// Returns the notifications in the order they were added.
    #[allow(dead_code)] // Public API for future use
    pub fn take(&mut self) -> Vec<SystemNotification> {
        std::mem::take(&mut self.pending)
    }

    /// Check if the queue is empty.
    #[must_use]
    #[allow(dead_code)] // Public API for future use
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    #[must_use]
    #[allow(dead_code)] // Public API for future use
    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_format() {
        assert_eq!(
            SystemNotification::ToolsApproved { count: 3 }.format(),
            "[System: User approved 3 tool call(s)]"
        );
        assert_eq!(
            SystemNotification::ToolsDenied { count: 1 }.format(),
            "[System: User denied 1 tool call(s)]"
        );
    }

    #[test]
    fn test_queue_push_and_take() {
        let mut queue = NotificationQueue::new();
        assert!(queue.is_empty());

        queue.push(SystemNotification::ToolsApproved { count: 1 });
        queue.push(SystemNotification::ToolsDenied { count: 2 });
        assert_eq!(queue.len(), 2);

        let notifications = queue.take();
        assert_eq!(notifications.len(), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_queue_deduplication() {
        let mut queue = NotificationQueue::new();

        queue.push(SystemNotification::ToolsApproved { count: 1 });
        queue.push(SystemNotification::ToolsApproved { count: 1 }); // duplicate
        queue.push(SystemNotification::ToolsDenied { count: 1 });
        queue.push(SystemNotification::ToolsApproved { count: 1 }); // duplicate

        // Should only have 2 unique notifications
        assert_eq!(queue.len(), 2);

        let notifications = queue.take();
        assert_eq!(notifications.len(), 2);
        assert_eq!(
            notifications[0],
            SystemNotification::ToolsApproved { count: 1 }
        );
        assert_eq!(
            notifications[1],
            SystemNotification::ToolsDenied { count: 1 }
        );
    }

    #[test]
    fn test_tools_approved_denied_not_deduplicated_with_different_counts() {
        let mut queue = NotificationQueue::new();

        queue.push(SystemNotification::ToolsApproved { count: 1 });
        queue.push(SystemNotification::ToolsApproved { count: 2 });
        queue.push(SystemNotification::ToolsDenied { count: 1 });

        // Different counts are different notifications
        assert_eq!(queue.len(), 3);
    }
}

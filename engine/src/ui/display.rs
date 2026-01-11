//! Display items for the message view.

use forge_context::MessageId;
use forge_types::Message;

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

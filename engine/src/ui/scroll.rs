//! Scroll state for the message view.

/// Scroll position for the message view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollState {
    /// Always keep the newest content visible.
    #[default]
    AutoBottom,
    /// Manual scroll offset from the top of the rendered message buffer.
    Manual { offset_from_top: u16 },
}

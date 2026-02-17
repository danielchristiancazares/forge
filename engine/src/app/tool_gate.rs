//! Session-scoped tool execution gate.
//!
//! When the tool journal is unhealthy, tool execution is fail-closed:
//! tool calls are pre-resolved to errors rather than executed.
//!
//! This is a separate policy dimension from `OperationState`—it cannot
//! be duplicated across state encodings (IFA §8.1).

/// Tool execution disabled due to tool journal errors.
///
/// Session-scoped safety latch. When active, tool calls are pre-resolved
/// to errors rather than executed (fail-closed).
#[derive(Debug, Clone)]
pub(crate) struct ToolsDisabledState {
    reason: String,
}

impl ToolsDisabledState {
    #[must_use]
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

/// Session-wide tool execution gate.
///
/// Tools may be disabled (fail-closed) when tool journal health is compromised.
#[derive(Debug, Clone)]
pub(crate) enum ToolGate {
    Enabled,
    Disabled(ToolsDisabledState),
}

impl ToolGate {
    pub(crate) fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled(_))
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Enabled => None,
            Self::Disabled(state) => Some(state.reason()),
        }
    }

    /// Disable tool execution and store the latest reason.
    ///
    /// Returns `true` if this transitioned from Enabled -> Disabled.
    pub(crate) fn disable(&mut self, reason: impl Into<String>) -> bool {
        let was_enabled = matches!(self, Self::Enabled);
        *self = Self::Disabled(ToolsDisabledState::new(reason));
        was_enabled
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::Enabled;
    }
}

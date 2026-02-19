//! Core resource budget invariant types.
//!
//! These types guarantee valid output/thinking/cache configurations by construction.

use thiserror::Error;

/// Hint for whether content should be cached by the provider.
///
/// Different providers handle caching differently:
/// - Claude: Explicit `cache_control: { type: "ephemeral" }` markers
/// - `OpenAI`: Automatic server-side prefix caching (hints ignored)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheHint {
    #[default]
    Default,
    /// Content is stable and should be cached if supported.
    ///
    /// Named "Ephemeral" to match Anthropic's API terminology. Despite the name,
    /// this actually means "cache this content" - Anthropic uses "ephemeral" to
    /// indicate the cache entry has a limited TTL (~5 min) rather than permanent
    /// storage. The content itself should be stable/unchanging for caching to help.
    Ephemeral,
}

/// Cache slot budget for a Claude API request.
///
/// Claude allows at most 4 `cache_control` blocks per request. This type
/// makes >4 unrepresentable by construction (IFA §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudget(u8);

impl CacheBudget {
    pub const MAX: u8 = 4;

    #[must_use]
    pub fn new(slots: u8) -> Self {
        Self(slots.min(Self::MAX))
    }

    #[must_use]
    pub fn full() -> Self {
        Self(Self::MAX)
    }

    #[must_use]
    pub fn remaining(self) -> u8 {
        self.0
    }

    /// Consume one slot. Returns the decremented budget, or `None` if exhausted.
    #[must_use]
    pub fn take_one(self) -> Option<CacheBudget> {
        if self.0 > 0 {
            Some(Self(self.0 - 1))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum OutputLimitsError {
    #[error("thinking budget ({budget}) must be less than max output tokens ({max_output})")]
    ThinkingBudgetTooLarge { budget: u32, max_output: u32 },
    #[error("thinking budget must be at least 1024 tokens")]
    ThinkingBudgetTooSmall,
}

/// Validated thinking budget for extended reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinkingBudget(u32);

impl ThinkingBudget {
    pub const MIN_TOKENS: u32 = 1024;

    pub fn new(value: u32) -> Result<Self, OutputLimitsError> {
        if value < Self::MIN_TOKENS {
            return Err(OutputLimitsError::ThinkingBudgetTooSmall);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingState {
    Disabled,
    Enabled(ThinkingBudget),
}

impl ThinkingState {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, ThinkingState::Enabled(_))
    }
}

/// Validated output configuration that guarantees invariants.
///
/// If thinking is enabled, `thinking_budget < max_output_tokens` is guaranteed
/// by construction. You cannot create an invalid `OutputLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLimits {
    Standard {
        max_output_tokens: u32,
    },
    WithThinking {
        max_output_tokens: u32,
        thinking_budget: ThinkingBudget,
    },
}

impl OutputLimits {
    #[must_use]
    pub const fn new(max_output_tokens: u32) -> Self {
        Self::Standard { max_output_tokens }
    }

    ///
    /// Returns an error if `thinking_budget >= max_output_tokens` or `thinking_budget < 1024`.
    pub fn with_thinking(
        max_output_tokens: u32,
        thinking_budget: u32,
    ) -> Result<Self, OutputLimitsError> {
        let budget = ThinkingBudget::new(thinking_budget)?;
        if budget.as_u32() >= max_output_tokens {
            return Err(OutputLimitsError::ThinkingBudgetTooLarge {
                budget: budget.as_u32(),
                max_output: max_output_tokens,
            });
        }
        Ok(Self::WithThinking {
            max_output_tokens,
            thinking_budget: budget,
        })
    }

    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        match self {
            OutputLimits::Standard { max_output_tokens }
            | OutputLimits::WithThinking {
                max_output_tokens, ..
            } => *max_output_tokens,
        }
    }

    #[must_use]
    pub const fn thinking(&self) -> ThinkingState {
        match self {
            OutputLimits::Standard { .. } => ThinkingState::Disabled,
            OutputLimits::WithThinking {
                thinking_budget, ..
            } => ThinkingState::Enabled(*thinking_budget),
        }
    }

    #[must_use]
    pub const fn has_thinking(&self) -> bool {
        matches!(self, OutputLimits::WithThinking { .. })
    }
}

//! Working context - the derived view sent to the API.
//!
//! The working context is rebuilt when:
//! - The model changes (different budget)
//! - Messages are added
//! - Summaries are created or restored
//!
//! It represents what will actually be sent to the LLM API,
//! mixing original messages and summaries to fit within the token budget.

use forge_types::{Message, NonEmptyStaticStr, NonEmptyString};

use super::history::{FullHistory, MessageId, SummaryId};

pub(crate) const SUMMARY_PREFIX: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Earlier conversation summary]");

/// Represents a segment of the working context.
#[derive(Debug, Clone)]
pub enum ContextSegment {
    /// Use the original message from history.
    Original { id: MessageId, tokens: u32 },
    /// Use a summary instead of original messages.
    Summarized {
        summary_id: SummaryId,
        /// Original message IDs that this replaces.
        replaces: Vec<MessageId>,
        tokens: u32,
    },
}

impl ContextSegment {
    /// Create an original message segment.
    pub fn original(id: MessageId, tokens: u32) -> Self {
        Self::Original { id, tokens }
    }

    /// Create a summarized segment.
    pub fn summarized(summary_id: SummaryId, replaces: Vec<MessageId>, tokens: u32) -> Self {
        Self::Summarized {
            summary_id,
            replaces,
            tokens,
        }
    }

    /// Returns true if this is an original message.
    #[cfg(test)]
    pub fn is_original(&self) -> bool {
        matches!(self, Self::Original { .. })
    }

    /// Returns true if this is a summary.
    pub fn is_summarized(&self) -> bool {
        matches!(self, Self::Summarized { .. })
    }

    /// Tokens attributed to this segment.
    pub fn tokens(&self) -> u32 {
        match self {
            ContextSegment::Original { tokens, .. } | ContextSegment::Summarized { tokens, .. } => {
                *tokens
            }
        }
    }
}

/// The working context: a plan for what to send to the API.
#[derive(Debug)]
pub struct WorkingContext {
    /// Ordered segments that comprise the context.
    segments: Vec<ContextSegment>,
    /// Budget this context was built against.
    token_budget: u32,
}

impl WorkingContext {
    /// Create a new empty working context with a budget.
    pub fn new(token_budget: u32) -> Self {
        Self {
            segments: Vec::new(),
            token_budget,
        }
    }

    /// Add an original message segment.
    pub fn push_original(&mut self, id: MessageId, tokens: u32) {
        self.segments.push(ContextSegment::original(id, tokens));
    }

    /// Add a summary segment.
    pub fn push_summary(&mut self, summary_id: SummaryId, replaces: Vec<MessageId>, tokens: u32) {
        self.segments
            .push(ContextSegment::summarized(summary_id, replaces, tokens));
    }

    /// Get all segments.
    pub fn segments(&self) -> &[ContextSegment] {
        &self.segments
    }

    /// Get total tokens used.
    pub fn total_tokens(&self) -> u32 {
        self.segments.iter().map(ContextSegment::tokens).sum()
    }

    /// Get the token budget.
    pub fn token_budget(&self) -> u32 {
        self.token_budget
    }

    /// How much space remains in the budget.
    #[cfg(test)]
    pub fn remaining_budget(&self) -> u32 {
        self.token_budget.saturating_sub(self.total_tokens())
    }

    /// Check if context fits within budget.
    #[cfg(test)]
    pub fn fits_budget(&self) -> bool {
        self.total_tokens() <= self.token_budget
    }

    /// Count of original message segments.
    #[cfg(test)]
    pub fn original_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_original()).count()
    }

    /// Count of summary segments.
    pub fn summary_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_summarized()).count()
    }

    /// Materialize into actual messages for API call.
    ///
    /// Summaries are injected as system messages with a prefix.
    /// Empty assistant messages are filtered out (API rejects them).
    pub fn materialize(&self, history: &FullHistory) -> Vec<Message> {
        let mut messages = Vec::new();

        for segment in &self.segments {
            match segment {
                ContextSegment::Original { id, .. } => {
                    let entry = history.get_entry(*id);
                    messages.push(entry.message().clone());
                }
                ContextSegment::Summarized { summary_id, .. } => {
                    let summary = history.summary(*summary_id);
                    // Inject summary as a system message.
                    let content = NonEmptyString::from(SUMMARY_PREFIX)
                        .append("\n")
                        .append(summary.content());
                    messages.push(Message::system(content));
                }
            }
        }

        messages
    }
}

/// Usage statistics for display in UI.
#[derive(Debug, Clone, Copy)]
pub struct ContextUsage {
    /// Tokens currently used in working context.
    pub used_tokens: u32,
    /// Token budget for current model.
    pub budget_tokens: u32,
    /// Count of summaries in context.
    pub summarized_segments: usize,
}

impl ContextUsage {
    /// Create usage stats from working context.
    pub fn from_context(ctx: &WorkingContext) -> Self {
        Self {
            used_tokens: ctx.total_tokens(),
            budget_tokens: ctx.token_budget(),
            summarized_segments: ctx.summary_count(),
        }
    }

    /// Usage as a percentage (0.0 - 100.0).
    pub fn percentage(&self) -> f32 {
        if self.budget_tokens == 0 {
            0.0
        } else {
            (self.used_tokens as f32 / self.budget_tokens as f32) * 100.0
        }
    }

    /// Format for status bar: "2.1k / 200k (1%)"
    pub fn format_compact(&self) -> String {
        fn format_k(n: u32) -> String {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f32 / 1_000_000.0)
            } else if n >= 1000 {
                format!("{:.1}k", n as f32 / 1000.0)
            } else {
                n.to_string()
            }
        }

        let pct = self.percentage();

        if self.summarized_segments > 0 {
            format!(
                "{} / {} ({:.0}%) [{}S]",
                format_k(self.used_tokens),
                format_k(self.budget_tokens),
                pct,
                self.summarized_segments
            )
        } else {
            format!(
                "{} / {} ({:.0}%)",
                format_k(self.used_tokens),
                format_k(self.budget_tokens),
                pct
            )
        }
    }

    /// Returns a severity level for UI coloring.
    /// 0 = green (< 70%), 1 = yellow (70-90%), 2 = red (> 90%)
    pub fn severity(&self) -> u8 {
        let pct = self.percentage();
        if pct > 90.0 {
            2
        } else if pct > 70.0 {
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_context_new() {
        let ctx = WorkingContext::new(200_000);
        assert_eq!(ctx.total_tokens(), 0);
        assert_eq!(ctx.token_budget(), 200_000);
        assert_eq!(ctx.remaining_budget(), 200_000);
        assert!(ctx.fits_budget());
    }

    #[test]
    fn test_push_segments() {
        let mut ctx = WorkingContext::new(1000);

        ctx.push_original(MessageId::new_for_test(0), 100);
        ctx.push_original(MessageId::new_for_test(1), 150);
        ctx.push_summary(
            super::super::history::SummaryId::new_for_test(0),
            vec![MessageId::new_for_test(2), MessageId::new_for_test(3)],
            50,
        );

        assert_eq!(ctx.total_tokens(), 300);
        assert_eq!(ctx.remaining_budget(), 700);
        assert_eq!(ctx.original_count(), 2);
        assert_eq!(ctx.summary_count(), 1);
    }

    #[test]
    fn test_context_usage_format() {
        let usage = ContextUsage {
            used_tokens: 2100,
            budget_tokens: 200_000,
            summarized_segments: 0,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("2.1k"));
        assert!(formatted.contains("200.0k"));
        assert!(formatted.contains("1%"));
    }

    #[test]
    fn test_context_usage_with_summaries() {
        let usage = ContextUsage {
            used_tokens: 50_000,
            budget_tokens: 200_000,
            summarized_segments: 2,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("[2S]"));
    }

    #[test]
    fn test_severity_levels() {
        // Low usage
        let low = ContextUsage {
            used_tokens: 10_000,
            budget_tokens: 200_000,
            summarized_segments: 0,
        };
        assert_eq!(low.severity(), 0);

        // Medium usage
        let medium = ContextUsage {
            used_tokens: 160_000,
            budget_tokens: 200_000,
            summarized_segments: 0,
        };
        assert_eq!(medium.severity(), 1);

        // High usage
        let high = ContextUsage {
            used_tokens: 190_000,
            budget_tokens: 200_000,
            summarized_segments: 0,
        };
        assert_eq!(high.severity(), 2);
    }
}

//! Working context - the derived view sent to the API.
//!
//! The working context is rebuilt when:
//! - The model changes (different budget)
//! - Messages are added
//! - Compaction occurs
//!
//! It represents what will actually be sent to the LLM API:
//! either all messages (pre-compaction) or compaction summary + post-compaction messages.

use std::time::SystemTime;

use forge_types::Message;

use super::history::{FullHistory, MessageId};

/// A message selected for inclusion in the API context.
#[derive(Debug, Clone)]
pub struct ContextSegment {
    id: MessageId,
    tokens: u32,
}

impl ContextSegment {
    #[must_use]
    pub fn new(id: MessageId, tokens: u32) -> Self {
        Self { id, tokens }
    }

    #[must_use]
    pub fn tokens(&self) -> u32 {
        self.tokens
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
    #[must_use]
    pub fn new(token_budget: u32) -> Self {
        Self {
            segments: Vec::new(),
            token_budget,
        }
    }

    pub fn push(&mut self, id: MessageId, tokens: u32) {
        self.segments.push(ContextSegment::new(id, tokens));
    }

    #[must_use]
    pub fn segments(&self) -> &[ContextSegment] {
        &self.segments
    }

    #[must_use]
    pub fn total_tokens(&self) -> u32 {
        self.segments.iter().map(ContextSegment::tokens).sum()
    }

    #[must_use]
    pub fn token_budget(&self) -> u32 {
        self.token_budget
    }

    #[cfg(test)]
    #[must_use]
    pub fn remaining_budget(&self) -> u32 {
        self.token_budget.saturating_sub(self.total_tokens())
    }

    #[cfg(test)]
    #[must_use]
    pub fn fits_budget(&self) -> bool {
        self.total_tokens() <= self.token_budget
    }

    /// Materialize into actual messages for API call.
    ///
    /// If the history is compacted, prepends the compaction summary as a
    /// system message. Then adds all selected entries in order.
    /// Empty assistant messages are filtered out (API rejects them).
    #[must_use]
    pub fn materialize(&self, history: &FullHistory) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.segments.len() + 1);

        if let Some(summary) = history.compaction_summary() {
            messages.push(Message::system(
                summary.content_non_empty().clone(),
                SystemTime::now(),
            ));
        }

        for segment in &self.segments {
            messages.push(history.get_entry(segment.id).message().clone());
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
    pub compacted: bool,
}

impl ContextUsage {
    #[must_use]
    pub fn from_context(ctx: &WorkingContext, compacted: bool) -> Self {
        Self {
            used_tokens: ctx.total_tokens(),
            budget_tokens: ctx.token_budget(),
            compacted,
        }
    }

    #[must_use]
    pub fn percentage(&self) -> f32 {
        if self.budget_tokens == 0 {
            0.0
        } else {
            (self.used_tokens as f32 / self.budget_tokens as f32) * 100.0
        }
    }

    /// Format for status bar: "2.1k / 200k (1%)" or "2.1k / 200k (1%) \[C\]"
    #[must_use]
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

        if self.compacted {
            format!(
                "{} / {} ({:.0}%) [C]",
                format_k(self.used_tokens),
                format_k(self.budget_tokens),
                pct,
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

    #[must_use]
    pub fn severity(&self) -> u8 {
        let pct = self.percentage();
        if pct > 90.0 { 2 } else { u8::from(pct > 70.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextSegment, ContextUsage, WorkingContext};
    use crate::history::MessageId;

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

        ctx.push(MessageId::new_for_test(0), 100);
        ctx.push(MessageId::new_for_test(1), 150);
        ctx.push(MessageId::new_for_test(2), 50);

        assert_eq!(ctx.total_tokens(), 300);
        assert_eq!(ctx.remaining_budget(), 700);
        assert_eq!(ctx.segments().len(), 3);
    }

    #[test]
    fn test_context_usage_format() {
        let usage = ContextUsage {
            used_tokens: 2100,
            budget_tokens: 200_000,
            compacted: false,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("2.1k"));
        assert!(formatted.contains("200.0k"));
        assert!(formatted.contains("1%"));
        assert!(!formatted.contains("[C]"));
    }

    #[test]
    fn test_context_usage_compacted() {
        let usage = ContextUsage {
            used_tokens: 50_000,
            budget_tokens: 200_000,
            compacted: true,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("[C]"));
    }

    #[test]
    fn test_severity_levels() {
        let low = ContextUsage {
            used_tokens: 10_000,
            budget_tokens: 200_000,
            compacted: false,
        };
        assert_eq!(low.severity(), 0);

        let medium = ContextUsage {
            used_tokens: 160_000,
            budget_tokens: 200_000,
            compacted: false,
        };
        assert_eq!(medium.severity(), 1);

        let high = ContextUsage {
            used_tokens: 190_000,
            budget_tokens: 200_000,
            compacted: false,
        };
        assert_eq!(high.severity(), 2);
    }

    #[test]
    fn test_percentage_zero_budget() {
        let usage = ContextUsage {
            used_tokens: 1000,
            budget_tokens: 0,
            compacted: false,
        };
        assert!(usage.percentage().abs() < 0.01);
    }

    #[test]
    fn test_percentage_full_usage() {
        let usage = ContextUsage {
            used_tokens: 100_000,
            budget_tokens: 100_000,
            compacted: false,
        };
        assert!((usage.percentage() - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_format_compact_small_tokens() {
        let usage = ContextUsage {
            used_tokens: 500,
            budget_tokens: 900,
            compacted: false,
        };
        let formatted = usage.format_compact();
        assert!(formatted.contains("500"));
        assert!(formatted.contains("900"));
    }

    #[test]
    fn test_format_compact_million_tokens() {
        let usage = ContextUsage {
            used_tokens: 1_500_000,
            budget_tokens: 2_000_000,
            compacted: false,
        };
        let formatted = usage.format_compact();
        assert!(formatted.contains("1.5M"));
        assert!(formatted.contains("2.0M"));
    }

    #[test]
    fn test_context_segment_tokens() {
        let segment = ContextSegment::new(MessageId::new_for_test(0), 150);
        assert_eq!(segment.tokens(), 150);
    }

    #[test]
    fn test_severity_boundary_exactly_70_percent() {
        let usage = ContextUsage {
            used_tokens: 70_000,
            budget_tokens: 100_000,
            compacted: false,
        };
        assert_eq!(usage.severity(), 0);
    }

    #[test]
    fn test_severity_boundary_just_over_70_percent() {
        let usage = ContextUsage {
            used_tokens: 70_001,
            budget_tokens: 100_000,
            compacted: false,
        };
        assert_eq!(usage.severity(), 1);
    }

    #[test]
    fn test_severity_boundary_exactly_90_percent() {
        let usage = ContextUsage {
            used_tokens: 90_000,
            budget_tokens: 100_000,
            compacted: false,
        };
        assert_eq!(usage.severity(), 1);
    }

    #[test]
    fn test_severity_boundary_just_over_90_percent() {
        let usage = ContextUsage {
            used_tokens: 90_001,
            budget_tokens: 100_000,
            compacted: false,
        };
        assert_eq!(usage.severity(), 2);
    }
}

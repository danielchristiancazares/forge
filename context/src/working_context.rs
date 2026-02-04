//! Working context - the derived view sent to the API.
//!
//! The working context is rebuilt when:
//! - The model changes (different budget)
//! - Messages are added
//! - distillates are created or restored
//!
//! It represents what will actually be sent to the LLM API,
//! mixing original messages and distillates to fit within the token budget.

use forge_types::{Message, NonEmptyStaticStr, NonEmptyString};

use super::history::{DistillateId, FullHistory, MessageId};

pub(crate) const DISTILLATE_PREFIX: NonEmptyStaticStr =
    NonEmptyStaticStr::new("[Earlier conversation Distillate]");

/// Represents a segment of the working context.
#[derive(Debug, Clone)]
pub enum ContextSegment {
    Original {
        id: MessageId,
        tokens: u32,
    },
    Distilled {
        distillate_id: DistillateId,
        /// Original message IDs that this replaces.
        replaces: Vec<MessageId>,
        tokens: u32,
    },
}

impl ContextSegment {
    #[must_use]
    pub fn original(id: MessageId, tokens: u32) -> Self {
        Self::Original { id, tokens }
    }

    #[must_use]
    pub fn distilled(distillate_id: DistillateId, replaces: Vec<MessageId>, tokens: u32) -> Self {
        Self::Distilled {
            distillate_id,
            replaces,
            tokens,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_original(&self) -> bool {
        matches!(self, Self::Original { .. })
    }

    #[must_use]
    pub fn is_distilled(&self) -> bool {
        matches!(self, Self::Distilled { .. })
    }

    #[must_use]
    pub fn tokens(&self) -> u32 {
        match self {
            ContextSegment::Original { tokens, .. } | ContextSegment::Distilled { tokens, .. } => {
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
    #[must_use]
    pub fn new(token_budget: u32) -> Self {
        Self {
            segments: Vec::new(),
            token_budget,
        }
    }

    pub fn push_original(&mut self, id: MessageId, tokens: u32) {
        self.segments.push(ContextSegment::original(id, tokens));
    }

    /// Add a Distillate segment.
    pub fn push_distillate(
        &mut self,
        distillate_id: DistillateId,
        replaces: Vec<MessageId>,
        tokens: u32,
    ) {
        self.segments
            .push(ContextSegment::distilled(distillate_id, replaces, tokens));
    }

    #[must_use]
    pub fn segments(&self) -> &[ContextSegment] {
        &self.segments
    }

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

    #[cfg(test)]
    #[must_use]
    pub fn original_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_original()).count()
    }

    #[must_use]
    pub fn distillate_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_distilled()).count()
    }

    /// Materialize into actual messages for API call.
    ///
    /// distillates are injected as system messages with a prefix.
    /// Empty assistant messages are filtered out (API rejects them).
    #[must_use]
    pub fn materialize(&self, history: &FullHistory) -> Vec<Message> {
        let mut messages = Vec::new();

        for segment in &self.segments {
            match segment {
                ContextSegment::Original { id, .. } => {
                    let entry = history.get_entry(*id);
                    messages.push(entry.message().clone());
                }
                ContextSegment::Distilled { distillate_id, .. } => {
                    let distillate = history.distillate(*distillate_id);
                    // Inject Distillate as a system message.
                    let content = NonEmptyString::prefixed(
                        DISTILLATE_PREFIX,
                        "\n",
                        distillate.content_non_empty(),
                    );
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
    /// Count of distillates in context.
    pub distilled_segments: usize,
}

impl ContextUsage {
    #[must_use]
    pub fn from_context(ctx: &WorkingContext) -> Self {
        Self {
            used_tokens: ctx.total_tokens(),
            budget_tokens: ctx.token_budget(),
            distilled_segments: ctx.distillate_count(),
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

    /// Format for status bar: "2.1k / 200k (1%)"
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

        if self.distilled_segments > 0 {
            format!(
                "{} / {} ({:.0}%) [{}S]",
                format_k(self.used_tokens),
                format_k(self.budget_tokens),
                pct,
                self.distilled_segments
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
        ctx.push_distillate(
            super::super::history::DistillateId::new_for_test(0),
            vec![MessageId::new_for_test(2), MessageId::new_for_test(3)],
            50,
        );

        assert_eq!(ctx.total_tokens(), 300);
        assert_eq!(ctx.remaining_budget(), 700);
        assert_eq!(ctx.original_count(), 2);
        assert_eq!(ctx.distillate_count(), 1);
    }

    #[test]
    fn test_context_usage_format() {
        let usage = ContextUsage {
            used_tokens: 2100,
            budget_tokens: 200_000,
            distilled_segments: 0,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("2.1k"));
        assert!(formatted.contains("200.0k"));
        assert!(formatted.contains("1%"));
    }

    #[test]
    fn test_context_usage_with_distillates() {
        let usage = ContextUsage {
            used_tokens: 50_000,
            budget_tokens: 200_000,
            distilled_segments: 2,
        };

        let formatted = usage.format_compact();
        assert!(formatted.contains("[2S]"));
    }

    #[test]
    fn test_severity_levels() {
        let low = ContextUsage {
            used_tokens: 10_000,
            budget_tokens: 200_000,
            distilled_segments: 0,
        };
        assert_eq!(low.severity(), 0);

        let medium = ContextUsage {
            used_tokens: 160_000,
            budget_tokens: 200_000,
            distilled_segments: 0,
        };
        assert_eq!(medium.severity(), 1);

        let high = ContextUsage {
            used_tokens: 190_000,
            budget_tokens: 200_000,
            distilled_segments: 0,
        };
        assert_eq!(high.severity(), 2);
    }

    #[test]
    fn test_percentage_zero_budget() {
        let usage = ContextUsage {
            used_tokens: 1000,
            budget_tokens: 0,
            distilled_segments: 0,
        };
        assert!(usage.percentage().abs() < 0.01);
    }

    #[test]
    fn test_percentage_full_usage() {
        let usage = ContextUsage {
            used_tokens: 100_000,
            budget_tokens: 100_000,
            distilled_segments: 0,
        };
        assert!((usage.percentage() - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_format_compact_small_tokens() {
        let usage = ContextUsage {
            used_tokens: 500,
            budget_tokens: 900,
            distilled_segments: 0,
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
            distilled_segments: 0,
        };
        let formatted = usage.format_compact();
        assert!(formatted.contains("1.5M"));
        assert!(formatted.contains("2.0M"));
    }

    #[test]
    fn test_context_segment_tokens() {
        let original = ContextSegment::original(MessageId::new_for_test(0), 150);
        assert_eq!(original.tokens(), 150);

        let distilled = ContextSegment::distilled(
            super::super::history::DistillateId::new_for_test(0),
            vec![MessageId::new_for_test(1)],
            200,
        );
        assert_eq!(distilled.tokens(), 200);
    }

    #[test]
    fn test_context_segment_type_checks() {
        let original = ContextSegment::original(MessageId::new_for_test(0), 100);
        assert!(original.is_original());
        assert!(!original.is_distilled());

        let distilled = ContextSegment::distilled(
            super::super::history::DistillateId::new_for_test(0),
            vec![],
            50,
        );
        assert!(!distilled.is_original());
        assert!(distilled.is_distilled());
    }

    #[test]
    fn test_severity_boundary_exactly_70_percent() {
        let usage = ContextUsage {
            used_tokens: 70_000,
            budget_tokens: 100_000,
            distilled_segments: 0,
        };
        assert_eq!(usage.severity(), 0);
    }

    #[test]
    fn test_severity_boundary_just_over_70_percent() {
        let usage = ContextUsage {
            used_tokens: 70_001,
            budget_tokens: 100_000,
            distilled_segments: 0,
        };
        assert_eq!(usage.severity(), 1);
    }

    #[test]
    fn test_severity_boundary_exactly_90_percent() {
        let usage = ContextUsage {
            used_tokens: 90_000,
            budget_tokens: 100_000,
            distilled_segments: 0,
        };
        assert_eq!(usage.severity(), 1);
    }

    #[test]
    fn test_severity_boundary_just_over_90_percent() {
        let usage = ContextUsage {
            used_tokens: 90_001,
            budget_tokens: 100_000,
            distilled_segments: 0,
        };
        assert_eq!(usage.severity(), 2);
    }
}

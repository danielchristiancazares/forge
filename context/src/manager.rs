//! Context Manager - orchestrates all context management components.
//!
//! The ContextManager is the main entry point for:
//! - Adding messages to history
//! - Switching models (triggers adaptation)
//! - Building working context for API calls
//! - Managing summarization
//! - Persistence

use anyhow::Result;
use std::path::Path;

use forge_types::{Message, NonEmptyString};

use super::history::{FullHistory, MessageId, Summary, SummaryId};
use super::model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry};
use super::token_counter::TokenCounter;
use super::working_context::{ContextSegment, ContextUsage, SUMMARY_PREFIX, WorkingContext};

const MIN_SUMMARY_RATIO: f32 = 0.01;
const MAX_SUMMARY_RATIO: f32 = 0.95;
const MIN_SUMMARY_TOKENS: u32 = 64;
const MAX_SUMMARY_TOKENS: u32 = 2048;

/// Configuration for summarization behavior.
#[derive(Debug, Clone)]
pub struct SummarizationConfig {
    /// Target compression ratio (e.g., 0.15 = 15% of original size).
    pub target_ratio: f32,
    /// Don't summarize the N most recent messages.
    pub preserve_recent: usize,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            target_ratio: 0.15,
            preserve_recent: 4,
        }
    }
}

/// Error indicating context cannot be built within budget.
#[derive(Debug)]
pub enum ContextBuildError {
    /// Older messages need summarization to fit within budget.
    SummarizationNeeded(SummarizationNeeded),
    /// The most recent N messages alone exceed the budget.
    /// This is unrecoverable - user must reduce input or switch to larger model.
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
        message_count: usize,
    },
}

/// Details about summarization needed to proceed.
#[derive(Debug)]
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}

/// A contiguous set of message IDs to summarize.
#[derive(Debug, Clone)]
pub struct SummarizationScope {
    ids: Vec<MessageId>,
    range: std::ops::Range<MessageId>,
}

/// Result of switching models.
#[derive(Debug)]
pub enum ContextAdaptation {
    /// No change in effective budget.
    NoChange,
    /// Switched to a model with smaller context.
    Shrinking {
        old_budget: u32,
        new_budget: u32,
        needs_summarization: bool,
    },
    /// Switched to a model with larger context.
    Expanding {
        old_budget: u32,
        new_budget: u32,
        /// Number of messages that could potentially be restored.
        can_restore: usize,
    },
}

/// Pending summarization request for async processing.
#[derive(Debug)]
pub struct PendingSummarization {
    pub scope: SummarizationScope,
    pub messages: Vec<(MessageId, Message)>,
    pub original_tokens: u32,
    pub target_tokens: u32,
}

/// Proof that a working context was successfully built.
#[derive(Debug)]
pub struct PreparedContext<'a> {
    manager: &'a ContextManager,
    working_context: WorkingContext,
}

impl<'a> PreparedContext<'a> {
    /// Materialize messages for an API call.
    pub fn api_messages(&self) -> Vec<Message> {
        self.working_context.materialize(&self.manager.history)
    }

    /// Usage stats for UI.
    pub fn usage(&self) -> ContextUsage {
        ContextUsage::from_context(&self.working_context)
    }
}

/// Usage state for the current model.
#[derive(Debug)]
pub enum ContextUsageStatus {
    Ready(ContextUsage),
    NeedsSummarization {
        usage: ContextUsage,
        needed: SummarizationNeeded,
    },
    RecentMessagesTooLarge {
        usage: ContextUsage,
        required_tokens: u32,
        budget_tokens: u32,
    },
}

/// The main context manager.
#[derive(Debug)]
pub struct ContextManager {
    /// Complete history - never discarded.
    history: FullHistory,
    /// Token counter.
    counter: TokenCounter,
    /// Model registry.
    registry: ModelRegistry,
    /// Current model name.
    current_model: String,
    /// Current model's limits.
    current_limits: ModelLimits,
    /// Where the current limits came from.
    current_limits_source: ModelLimitsSource,
    /// Summarization configuration.
    summarization_config: SummarizationConfig,
}

impl ContextManager {
    /// Create a new context manager for the given model.
    pub fn new(initial_model: &str) -> Self {
        let registry = ModelRegistry::new();
        let resolved = registry.get(initial_model);
        let limits = resolved.limits();
        let limits_source = resolved.source();

        Self {
            history: FullHistory::new(),
            counter: TokenCounter::new(),
            registry,
            current_model: initial_model.to_string(),
            current_limits: limits,
            current_limits_source: limits_source,
            summarization_config: SummarizationConfig::default(),
        }
    }

    /// Add a message to history and invalidate working context.
    pub fn push_message(&mut self, message: Message) -> MessageId {
        let token_count = self.counter.count_message(&message);
        self.history.push(message, token_count)
    }

    /// Switch to a different model - triggers context adaptation.
    pub fn switch_model(&mut self, new_model: &str) -> ContextAdaptation {
        let old_limits = self.current_limits;
        let resolved = self.registry.get(new_model);
        let new_limits = resolved.limits();
        let new_source = resolved.source();

        self.current_model = new_model.to_string();
        self.current_limits = new_limits;
        self.current_limits_source = new_source;

        let old_budget = old_limits.effective_input_budget();
        let new_budget = new_limits.effective_input_budget();

        if new_budget < old_budget {
            ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_summarization: self.build_working_context().is_err(),
            }
        } else if new_budget > old_budget {
            ContextAdaptation::Expanding {
                old_budget,
                new_budget,
                can_restore: self.history.summarized_count(),
            }
        } else {
            ContextAdaptation::NoChange
        }
    }

    /// Update model limits without triggering summarization behavior.
    pub fn set_model_without_adaptation(&mut self, new_model: &str) {
        let resolved = self.registry.get(new_model);
        self.current_model = new_model.to_string();
        self.current_limits = resolved.limits();
        self.current_limits_source = resolved.source();
    }

    /// Build the working context for current model.
    ///
    /// Returns an error if summarization is needed to fit within budget, or if
    /// the most recent messages alone exceed the budget (unrecoverable).
    // TODO: Accept configured output limit to use effective_input_budget_with_reserved()
    // instead of reserving the model's full max_output. This would allow more input
    // context when users configure smaller output limits.
    fn build_working_context(&self) -> Result<WorkingContext, ContextBuildError> {
        let budget = self.current_limits.effective_input_budget();
        let mut ctx = WorkingContext::new(budget);

        let entries = self.history.entries();
        let max_preserve = self.summarization_config.preserve_recent.min(entries.len());
        let mut preserve_count = 0usize;
        let mut tokens_for_recent = 0u32;

        // Phase 1: The N most recent messages are always preserved and never summarized.
        // Count them unconditionally - if they exceed budget, that's an unrecoverable error.
        for entry in entries.iter().rev().take(max_preserve) {
            tokens_for_recent = tokens_for_recent.saturating_add(entry.token_count());
            preserve_count += 1;
        }

        if tokens_for_recent > budget {
            return Err(ContextBuildError::RecentMessagesTooLarge {
                required_tokens: tokens_for_recent,
                budget_tokens: budget,
                message_count: preserve_count,
            });
        }

        let recent_start = entries.len().saturating_sub(preserve_count);
        let remaining_budget = budget.saturating_sub(tokens_for_recent);

        #[derive(Debug)]
        enum Block {
            Unsummarized(Vec<(MessageId, u32)>),
            Summarized {
                summary_id: SummaryId,
                messages: Vec<(MessageId, u32)>,
                summary_tokens: u32,
            },
        }

        // Phase 2: Partition older messages into contiguous blocks.
        let older_entries = &entries[..recent_start];
        let mut blocks: Vec<Block> = Vec::new();
        let mut unsummarized: Vec<(MessageId, u32)> = Vec::new();
        let mut summary_block: Option<(SummaryId, u32)> = None;
        let mut summarized: Vec<(MessageId, u32)> = Vec::new();

        for entry in older_entries {
            let summarized_here = entry
                .summary_id()
                .map(|sid| (sid, self.history.summary(sid).token_count()));

            match summarized_here {
                Some((summary_id, summary_tokens)) => {
                    if !unsummarized.is_empty() {
                        blocks.push(Block::Unsummarized(std::mem::take(&mut unsummarized)));
                    }

                    match summary_block {
                        Some((current_id, _)) if current_id == summary_id => {}
                        Some((current_id, current_tokens)) => {
                            blocks.push(Block::Summarized {
                                summary_id: current_id,
                                messages: std::mem::take(&mut summarized),
                                summary_tokens: current_tokens,
                            });
                            summary_block = Some((summary_id, summary_tokens));
                        }
                        None => {
                            summary_block = Some((summary_id, summary_tokens));
                        }
                    }

                    summarized.push((entry.id(), entry.token_count()));
                }
                None => {
                    if let Some((summary_id, summary_tokens)) = summary_block.take() {
                        blocks.push(Block::Summarized {
                            summary_id,
                            messages: std::mem::take(&mut summarized),
                            summary_tokens,
                        });
                    }

                    unsummarized.push((entry.id(), entry.token_count()));
                }
            }
        }

        if let Some((summary_id, summary_tokens)) = summary_block.take() {
            blocks.push(Block::Summarized {
                summary_id,
                messages: summarized,
                summary_tokens,
            });
        } else if !unsummarized.is_empty() {
            blocks.push(Block::Unsummarized(unsummarized));
        }

        // Phase 3: Select older content from newest to oldest within remaining budget.
        let mut selected_rev: Vec<ContextSegment> = Vec::new();
        let mut need_summary_rev: Vec<MessageId> = Vec::new();
        let mut tokens_used: u32 = 0;
        let mut exhausted = false;

        for block in blocks.iter().rev() {
            if exhausted {
                if let Block::Unsummarized(messages) = block {
                    for (id, _) in messages.iter().rev() {
                        need_summary_rev.push(*id);
                    }
                }
                continue;
            }

            match block {
                Block::Summarized {
                    summary_id,
                    messages,
                    summary_tokens,
                } => {
                    let original_tokens: u32 = messages.iter().map(|(_, t)| *t).sum();

                    // Prefer full originals when budget allows; otherwise fall back to the summary.
                    if tokens_used + original_tokens <= remaining_budget {
                        for (id, tokens) in messages.iter().rev() {
                            selected_rev.push(ContextSegment::original(*id, *tokens));
                        }
                        tokens_used += original_tokens;
                    } else if tokens_used + *summary_tokens <= remaining_budget {
                        let replaces: Vec<MessageId> = messages.iter().map(|(id, _)| *id).collect();
                        selected_rev.push(ContextSegment::summarized(
                            *summary_id,
                            replaces,
                            *summary_tokens,
                        ));
                        tokens_used += *summary_tokens;
                    } else {
                        // Can't include this summarized block; stop to avoid holes in history.
                        break;
                    }
                }
                Block::Unsummarized(messages) => {
                    // Include as many of the most recent messages as we can.
                    for i in (0..messages.len()).rev() {
                        let (id, tokens) = messages[i];
                        if tokens_used + tokens <= remaining_budget {
                            selected_rev.push(ContextSegment::original(id, tokens));
                            tokens_used += tokens;
                        } else {
                            // Everything older than this point should be summarized.
                            exhausted = true;
                            for (id, _) in messages[..=i].iter().rev() {
                                need_summary_rev.push(*id);
                            }
                            break;
                        }
                    }
                }
            }
        }

        let mut need_summary: Vec<MessageId> = need_summary_rev.into_iter().rev().collect();

        if !need_summary.is_empty() {
            need_summary.sort_by_key(|id| id.as_u64());
            need_summary.dedup();

            let tokens_to_summarize: u32 = need_summary
                .iter()
                .map(|id| self.history.get_entry(*id).token_count())
                .sum();
            let available_left = remaining_budget.saturating_sub(tokens_used);
            let excess_tokens = tokens_to_summarize.saturating_sub(available_left);

            let msg_count = need_summary.len();
            return Err(ContextBuildError::SummarizationNeeded(
                SummarizationNeeded {
                    excess_tokens,
                    messages_to_summarize: need_summary,
                    suggestion: format!("{} older messages need summarization", msg_count),
                },
            ));
        }

        // Phase 4: Materialize selected older segments in chronological order.
        for segment in selected_rev.into_iter().rev() {
            match segment {
                ContextSegment::Original { id, tokens } => ctx.push_original(id, tokens),
                ContextSegment::Summarized {
                    summary_id,
                    replaces,
                    tokens,
                } => ctx.push_summary(summary_id, replaces, tokens),
            }
        }

        // Phase 5: Always include the N most recent messages.
        for entry in &entries[recent_start..] {
            ctx.push_original(entry.id(), entry.token_count());
        }

        Ok(ctx)
    }

    /// Prepare a summarization request for the given messages.
    pub fn prepare_summarization(
        &mut self,
        message_ids: &[MessageId],
    ) -> Option<PendingSummarization> {
        let mut ids: Vec<MessageId> = message_ids.to_vec();
        ids.sort_by_key(|id| id.as_u64());
        ids.dedup();

        if ids.is_empty() {
            return None;
        }

        // Keep only the first contiguous run - summaries must represent a contiguous slice of history.
        let mut end = 1usize;
        while end < ids.len() {
            if ids[end].as_u64() == ids[end - 1].as_u64() + 1 {
                end += 1;
            } else {
                break;
            }
        }
        ids.truncate(end);

        let messages: Vec<_> = ids
            .iter()
            .map(|id| (*id, self.history.get_entry(*id).message().clone()))
            .collect();

        let original_tokens: u32 = ids
            .iter()
            .map(|id| self.history.get_entry(*id).token_count())
            .sum();

        let ratio = self
            .summarization_config
            .target_ratio
            .clamp(MIN_SUMMARY_RATIO, MAX_SUMMARY_RATIO);
        let target_tokens = ((original_tokens as f64) * ratio as f64).round() as u32;
        let target_tokens = target_tokens.clamp(MIN_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS);

        let first = ids.first().copied()?;
        let last = ids.last().copied()?;
        let end_exclusive = last.next();
        let scope = SummarizationScope {
            ids,
            range: first..end_exclusive,
        };

        Some(PendingSummarization {
            scope,
            messages,
            original_tokens,
            target_tokens,
        })
    }

    /// Complete a summarization by adding the generated summary.
    pub fn complete_summarization(
        &mut self,
        scope: SummarizationScope,
        content: NonEmptyString,
        generated_by: String,
    ) -> Result<SummaryId> {
        let injected = NonEmptyString::from(SUMMARY_PREFIX)
            .append("\n")
            .append(content.as_str());
        let token_count = self.counter.count_message(&Message::system(injected));

        let SummarizationScope { ids, range } = scope;

        let original_tokens: u32 = ids
            .iter()
            .map(|id| self.history.get_entry(*id).token_count())
            .sum();

        let summary_id = self.history.next_summary_id();
        let summary = Summary::new(
            summary_id,
            range,
            content,
            token_count,
            original_tokens,
            generated_by,
        );

        self.history.add_summary(summary)?;
        Ok(summary_id)
    }

    /// Try to restore summarized messages when budget allows.
    ///
    /// This does not mutate history. If the current model's budget can fit original messages for
    /// previously-summarized segments, `build_working_context()` will choose originals.
    pub fn try_restore_messages(&self) -> usize {
        let ctx = match self.build_working_context() {
            Ok(ctx) => ctx,
            Err(_) => return 0,
        };

        ctx.segments()
            .iter()
            .filter(|segment| {
                matches!(
                    segment,
                    ContextSegment::Original { id, .. }
                        if self.history.get_entry(*id).summary_id().is_some()
                )
            })
            .count()
    }

    /// Build a working context proof for the current model.
    pub fn prepare(&self) -> Result<PreparedContext<'_>, ContextBuildError> {
        let working_context = self.build_working_context()?;
        Ok(PreparedContext {
            manager: self,
            working_context,
        })
    }

    /// Get current usage statistics with explicit summarization status.
    pub fn usage_status(&self) -> ContextUsageStatus {
        let fallback_usage = || ContextUsage {
            used_tokens: self.history.total_tokens(),
            budget_tokens: self.current_limits.effective_input_budget(),
            summarized_segments: 0,
        };

        match self.prepare() {
            Ok(prepared) => ContextUsageStatus::Ready(prepared.usage()),
            Err(ContextBuildError::SummarizationNeeded(needed)) => {
                ContextUsageStatus::NeedsSummarization {
                    usage: fallback_usage(),
                    needed,
                }
            }
            Err(ContextBuildError::RecentMessagesTooLarge {
                required_tokens,
                budget_tokens,
                ..
            }) => ContextUsageStatus::RecentMessagesTooLarge {
                usage: fallback_usage(),
                required_tokens,
                budget_tokens,
            },
        }
    }

    /// Access to full history.
    pub fn history(&self) -> &FullHistory {
        &self.history
    }

    /// Current model name.
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// Current model limits.
    pub fn current_limits(&self) -> ModelLimits {
        self.current_limits
    }

    /// Where the current model limits came from.
    pub fn current_limits_source(&self) -> ModelLimitsSource {
        self.current_limits_source
    }

    // === Persistence ===

    /// Save history to a JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(&self.history)?;
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, json)?;

        if let Err(err) = std::fs::rename(&tmp_path, path) {
            if path.exists() {
                std::fs::remove_file(path)?;
                std::fs::rename(&tmp_path, path)?;
            } else {
                return Err(err.into());
            }
        }
        Ok(())
    }

    /// Load history from a JSON file.
    pub fn load(path: impl AsRef<Path>, model: &str) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let history: FullHistory = serde_json::from_str(&json)?;

        let registry = ModelRegistry::new();
        let resolved = registry.get(model);
        let limits = resolved.limits();
        let limits_source = resolved.source();

        Ok(Self {
            history,
            counter: TokenCounter::new(),
            registry,
            current_model: model.to_string(),
            current_limits: limits,
            current_limits_source: limits_source,
            summarization_config: SummarizationConfig::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let manager = ContextManager::new("claude-opus-4");
        assert_eq!(manager.current_model(), "claude-opus-4");
        assert_eq!(manager.history().len(), 0);
    }

    #[test]
    fn test_push_message() {
        let mut manager = ContextManager::new("claude-opus-4");

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        let id2 = manager.push_message(Message::try_user("World").expect("non-empty test message"));

        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);
        assert_eq!(manager.history().len(), 2);
    }

    #[test]
    fn test_switch_model_shrinking() {
        let mut manager = ContextManager::new("claude-opus-4"); // 200k context

        let result = manager.switch_model("gpt-4"); // 8k context

        match result {
            ContextAdaptation::Shrinking { .. } => (),
            _ => panic!("Expected Shrinking adaptation"),
        }
    }

    #[test]
    fn test_switch_model_expanding() {
        let mut manager = ContextManager::new("gpt-4"); // 8k context

        let result = manager.switch_model("claude-opus-4"); // 200k context

        match result {
            ContextAdaptation::Expanding { .. } => (),
            _ => panic!("Expected Expanding adaptation"),
        }
    }

    #[test]
    fn test_build_context_simple() {
        let mut manager = ContextManager::new("claude-opus-4");

        manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        manager.push_message(Message::try_user("World").expect("non-empty test message"));

        let result = manager.build_working_context();
        assert!(result.is_ok());

        let ctx = result.unwrap();
        assert_eq!(ctx.segments().len(), 2);
    }
}

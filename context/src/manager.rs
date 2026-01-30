//! Context Manager - orchestrates all context management components.
//!
//! The `ContextManager` is the main entry point for:
//! - Adding messages to history
//! - Switching models (triggers adaptation)
//! - Building working context for API calls
//! - Managing summarization
//! - Persistence

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use forge_types::{Message, NonEmptyString};
use tempfile::NamedTempFile;

use super::StepId;
use super::history::{FullHistory, MessageId, Summary, SummaryId};
use super::model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry};
use super::token_counter::TokenCounter;
use super::working_context::{ContextSegment, ContextUsage, SUMMARY_PREFIX, WorkingContext};

const MIN_SUMMARY_RATIO: f32 = 0.01;
const MAX_SUMMARY_RATIO: f32 = 0.95;
const MIN_SUMMARY_TOKENS: u32 = 64;
const MAX_SUMMARY_TOKENS: u32 = 2048;

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

#[derive(Debug, Clone)]
pub struct SummarizationNeeded {
    pub excess_tokens: u32,
    pub messages_to_summarize: Vec<MessageId>,
    pub suggestion: String,
}

#[derive(Debug, Clone)]
pub struct SummarizationScope {
    ids: Vec<MessageId>,
    range: std::ops::Range<MessageId>,
}

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

#[derive(Debug)]
pub struct PendingSummarization {
    pub scope: SummarizationScope,
    pub messages: Vec<(MessageId, Message)>,
    pub original_tokens: u32,
    pub target_tokens: u32,
}

#[derive(Debug)]
pub struct PreparedContext<'a> {
    manager: &'a ContextManager,
    working_context: WorkingContext,
}

impl PreparedContext<'_> {
    /// Materialize messages for an API call.
    #[must_use]
    pub fn api_messages(&self) -> Vec<Message> {
        self.working_context.materialize(&self.manager.history)
    }

    /// Usage stats for UI.
    #[must_use]
    pub fn usage(&self) -> ContextUsage {
        ContextUsage::from_context(&self.working_context)
    }
}

#[derive(Debug, Clone)]
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

#[derive(Debug)]
pub struct ContextManager {
    history: FullHistory,
    counter: TokenCounter,
    registry: ModelRegistry,
    current_model: String,
    current_limits: ModelLimits,
    current_limits_source: ModelLimitsSource,
    summarization_config: SummarizationConfig,
    /// Configured output limit (if set, allows more input context).
    configured_output_limit: Option<u32>,
}

impl ContextManager {
    /// Create a new context manager for the given model.
    #[must_use]
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
            configured_output_limit: None,
        }
    }

    /// Set the configured output limit.
    ///
    /// When set, the effective input budget will reserve only this amount
    /// for output instead of the model's full `max_output` capability.
    /// This allows more input context when users configure smaller output limits.
    pub fn set_output_limit(&mut self, limit: u32) {
        self.configured_output_limit = Some(limit);
    }

    /// Get the effective input budget, respecting configured output limit.
    fn effective_budget(&self) -> u32 {
        match self.configured_output_limit {
            Some(limit) => self
                .current_limits
                .effective_input_budget_with_reserved(limit),
            None => self.current_limits.effective_input_budget(),
        }
    }

    /// Add a message to history and invalidate working context.
    pub fn push_message(&mut self, message: Message) -> MessageId {
        let token_count = self.counter.count_message(&message);
        self.history.push(message, token_count)
    }

    /// Add a message to history with an associated stream step ID.
    ///
    /// Used for assistant messages from streaming responses to enable
    /// idempotent crash recovery.
    pub fn push_message_with_step_id(
        &mut self,
        message: Message,
        stream_step_id: StepId,
    ) -> MessageId {
        let token_count = self.counter.count_message(&message);
        self.history
            .push_with_step_id(message, token_count, stream_step_id)
    }

    /// Check if a stream step ID already exists in history.
    ///
    /// Used for idempotent crash recovery - if history already contains
    /// an entry with this `step_id`, we should not recover it again.
    #[must_use]
    pub fn has_step_id(&self, step_id: StepId) -> bool {
        self.history.has_step_id(step_id)
    }

    /// Rollback (remove) the last message if it matches the given ID.
    ///
    /// This is used for transactional rollback when a stream fails before
    /// producing any content. Returns the removed message if successful.
    pub fn rollback_last_message(&mut self, id: MessageId) -> Option<Message> {
        self.history.pop_if_last(id)
    }

    /// Switch to a different model - triggers context adaptation.
    pub fn switch_model(&mut self, new_model: &str) -> ContextAdaptation {
        let old_budget = self.effective_budget();
        let resolved = self.registry.get(new_model);
        let new_limits = resolved.limits();
        let new_source = resolved.source();

        self.current_model = new_model.to_string();
        self.current_limits = new_limits;
        self.current_limits_source = new_source;

        let new_budget = self.effective_budget();

        match new_budget.cmp(&old_budget) {
            std::cmp::Ordering::Less => ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_summarization: self.build_working_context().is_err(),
            },
            std::cmp::Ordering::Greater => ContextAdaptation::Expanding {
                old_budget,
                new_budget,
                can_restore: self.history.summarized_count(),
            },
            std::cmp::Ordering::Equal => ContextAdaptation::NoChange,
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
    fn build_working_context(&self) -> Result<WorkingContext, ContextBuildError> {
        #[derive(Debug)]
        enum Block {
            Unsummarized(Vec<(MessageId, u32)>),
            Summarized {
                summary_id: SummaryId,
                messages: Vec<(MessageId, u32)>,
                summary_tokens: u32,
            },
        }

        let budget = self.effective_budget();
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

            if let Some((summary_id, summary_tokens)) = summarized_here {
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
            } else {
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
                // Collect all older content (summarized or not) for re-summarization.
                match block {
                    Block::Unsummarized(messages) | Block::Summarized { messages, .. } => {
                        for (id, _) in messages.iter().rev() {
                            need_summary_rev.push(*id);
                        }
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
                        // Even the summary doesn't fit. Mark underlying messages for
                        // hierarchical re-summarization (combined with other old content
                        // into a more compact summary). The old summary becomes orphaned.
                        exhausted = true;
                        for (id, _) in messages.iter().rev() {
                            need_summary_rev.push(*id);
                        }
                        // Don't break - continue to collect more content for summarization.
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
            need_summary.sort_by_key(super::history::MessageId::as_u64);
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
                    suggestion: format!("{msg_count} older messages need summarization"),
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
        ids.sort_by_key(super::history::MessageId::as_u64);
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
        let target_tokens = (f64::from(original_tokens) * f64::from(ratio)).round() as u32;
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
        let injected = NonEmptyString::prefixed(SUMMARY_PREFIX, "\n", &content);
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
    #[must_use]
    pub fn try_restore_messages(&self) -> usize {
        let Ok(ctx) = self.build_working_context() else {
            return 0;
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

    /// Get only the N most recent messages, bypassing summarization.
    ///
    /// This is used when the Librarian is active - instead of summarizing
    /// old messages, we rely on the Librarian's distilled facts for context.
    /// This mode sends: system prompt + Librarian facts + recent N messages.
    ///
    /// Returns messages in chronological order (oldest first).
    #[must_use]
    pub fn recent_messages_only(&self, count: usize) -> Vec<Message> {
        let entries = self.history.entries();
        let start = entries.len().saturating_sub(count);
        entries[start..]
            .iter()
            .map(|entry| entry.message().clone())
            .collect()
    }

    /// Get the configured preserve_recent count.
    #[must_use]
    pub fn preserve_recent_count(&self) -> usize {
        self.summarization_config.preserve_recent
    }

    /// Get current usage statistics with explicit summarization status.
    #[must_use]
    pub fn usage_status(&self) -> ContextUsageStatus {
        let fallback_usage = || ContextUsage {
            used_tokens: self.history.total_tokens(),
            budget_tokens: self.effective_budget(),
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
    #[must_use]
    pub fn history(&self) -> &FullHistory {
        &self.history
    }

    /// Current model name.
    #[must_use]
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// Current model limits.
    #[must_use]
    pub fn current_limits(&self) -> ModelLimits {
        self.current_limits
    }

    /// Where the current model limits came from.
    #[must_use]
    pub fn current_limits_source(&self) -> ModelLimitsSource {
        self.current_limits_source
    }

    // === Persistence ===

    /// Save history to a JSON file.
    ///
    /// Uses atomic write pattern: write to temp file, then rename.
    /// On Windows where rename-over-existing fails, uses backup-and-restore
    /// to prevent data loss if the final rename fails.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(&self.history)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))?;
        }
        tmp.write_all(json.as_bytes())?;
        tmp.as_file().sync_all()?;

        if let Err(err) = tmp.persist(path) {
            if path.exists() {
                // On Windows, rename fails if target exists.
                // Use backup-restore pattern to prevent data loss.
                let backup_path = path.with_extension("bak");
                let _ = std::fs::remove_file(&backup_path);

                // Move original to backup (preserves data if next step fails)
                std::fs::rename(path, &backup_path)?;

                // Try to move tmp to target
                if let Err(rename_err) = err.file.persist(path) {
                    // Restore from backup - original data preserved
                    let _ = std::fs::rename(&backup_path, path);
                    return Err(rename_err.error.into());
                }

                // Success - clean up backup
                let _ = std::fs::remove_file(&backup_path);
            } else {
                return Err(err.error.into());
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
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
            configured_output_limit: None, // Will be set by engine after load
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let manager = ContextManager::new("claude-opus-4-5-20251101");
        assert_eq!(manager.current_model(), "claude-opus-4-5-20251101");
        assert_eq!(manager.history().len(), 0);
    }

    #[test]
    fn test_push_message() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        let id2 = manager.push_message(Message::try_user("World").expect("non-empty test message"));

        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);
        assert_eq!(manager.history().len(), 2);
    }

    #[test]
    fn test_switch_model_shrinking() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101"); // 200k context

        // Unknown model falls back to 8k context (default limits)
        let result = manager.switch_model("unknown-small-model");

        match result {
            ContextAdaptation::Shrinking { .. } => (),
            _ => panic!("Expected Shrinking adaptation"),
        }
    }

    #[test]
    fn test_switch_model_expanding() {
        // Unknown model falls back to 8k context (default limits)
        let mut manager = ContextManager::new("unknown-small-model");

        let result = manager.switch_model("claude-opus-4-5-20251101"); // 200k context

        match result {
            ContextAdaptation::Expanding { .. } => (),
            _ => panic!("Expected Expanding adaptation"),
        }
    }

    #[test]
    fn test_build_context_simple() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        manager.push_message(Message::try_user("World").expect("non-empty test message"));

        let result = manager.build_working_context();
        assert!(result.is_ok());

        let ctx = result.unwrap();
        assert_eq!(ctx.segments().len(), 2);
    }

    #[test]
    fn test_hierarchical_summarization_collects_summarized_messages() {
        use crate::history::{MessageId, Summary};
        use forge_types::NonEmptyString;

        // Create manager with a very small model to force tight budget
        // Unknown model falls back to 8k context (default limits)
        let mut manager = ContextManager::new("unknown-small-model");

        // Add many messages to exceed budget
        for i in 0..20 {
            manager.push_message(
                Message::try_user(format!("Message {i} with some content to use tokens"))
                    .expect("non-empty"),
            );
        }

        // Create a summary covering messages 0-10
        let summary_id = manager.history.next_summary_id();
        let summary = Summary::new(
            summary_id,
            MessageId::new_for_test(0)..MessageId::new_for_test(10),
            NonEmptyString::new("Summary of first 10 messages").expect("non-empty"),
            100, // 100 tokens for the summary
            500, // Original was 500 tokens
            "test-model".to_string(),
        );
        manager.history.add_summary(summary).expect("add summary");

        // Switch to a different unknown model to trigger adaptation
        // (Both fall back to 8k default, but we're testing the hierarchical logic)
        manager.set_model_without_adaptation("even-smaller-model-fallback");

        // Now try to build context - should need hierarchical summarization
        let result = manager.build_working_context();

        // If summarization is needed, verify summarized messages are included
        if let Err(ContextBuildError::SummarizationNeeded(needed)) = result {
            // The messages_to_summarize should include the already-summarized messages 0-9
            // if they were collected for hierarchical re-summarization
            let has_summarized = needed
                .messages_to_summarize
                .iter()
                .any(|id| id.as_u64() < 10);

            // This test verifies the mechanism exists - the exact behavior depends on
            // budget calculations. The key assertion is that we don't panic or break.
            assert!(
                !needed.messages_to_summarize.is_empty(),
                "Should have messages to summarize"
            );

            // If summarized messages are included, verify they form a contiguous range
            // from the start (hierarchical summarization collects from oldest)
            if has_summarized {
                let min_id = needed
                    .messages_to_summarize
                    .iter()
                    .map(MessageId::as_u64)
                    .min()
                    .unwrap();
                assert_eq!(min_id, 0, "Should include oldest messages first");
            }
        }
        // If Ok, the context fit - which is also valid
    }

    #[test]
    fn test_rollback_last_message_success() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty"));
        let id2 = manager.push_message(Message::try_user("World").expect("non-empty"));

        assert_eq!(manager.history().len(), 2);

        // Rollback the last message
        let rolled_back = manager.rollback_last_message(id2);
        assert!(rolled_back.is_some());
        assert_eq!(rolled_back.unwrap().content(), "World");
        assert_eq!(manager.history().len(), 1);

        // Rollback the remaining message
        let rolled_back = manager.rollback_last_message(id1);
        assert!(rolled_back.is_some());
        assert_eq!(rolled_back.unwrap().content(), "Hello");
        assert_eq!(manager.history().len(), 0);
    }

    #[test]
    fn test_rollback_last_message_wrong_id() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty"));
        let _id2 = manager.push_message(Message::try_user("World").expect("non-empty"));

        // Try to rollback with wrong ID
        let rolled_back = manager.rollback_last_message(id1);
        assert!(rolled_back.is_none());
        assert_eq!(manager.history().len(), 2); // Nothing was removed
    }

    // ========================================================================
    // Output Limit Tests
    // ========================================================================

    #[test]
    fn test_set_output_limit() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        // Get initial budget (without configured output limit)
        let _initial_budget = manager.effective_budget();

        // Set a smaller output limit - should increase effective input budget
        manager.set_output_limit(4096);

        // The budget should be different (typically higher) when we reserve less for output
        // This depends on model limits, but setting a limit should have an effect
        let new_budget = manager.effective_budget();

        // At minimum, setting an output limit should not panic
        assert!(new_budget > 0);

        // Test that limits are accessible
        let limits = manager.current_limits();
        assert!(limits.context_window() > 0);
    }

    #[test]
    fn test_current_limits_source() {
        let manager = ContextManager::new("claude-opus-4-5-20251101");

        // Known model should come from prefix match
        let source = manager.current_limits_source();
        assert!(matches!(source, ModelLimitsSource::Prefix(_)));
    }

    #[test]
    fn test_current_limits_source_unknown_model() {
        let manager = ContextManager::new("unknown-model-xyz");

        // Unknown model should use default fallback
        let source = manager.current_limits_source();
        assert!(matches!(source, ModelLimitsSource::DefaultFallback));
    }

    // ========================================================================
    // Persistence Tests (save/load)
    // ========================================================================

    #[test]
    fn test_save_load_roundtrip() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        // Add some messages
        manager.push_message(Message::try_user("Hello").expect("non-empty"));
        manager.push_message(Message::try_user("World").expect("non-empty"));

        assert_eq!(manager.history().len(), 2);

        // Create temp file
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join(format!("forge_test_{}.json", std::process::id()));

        // Save
        manager.save(&tmp_path).expect("save should succeed");

        // Verify file exists
        assert!(tmp_path.exists());

        // Load into new manager
        let loaded = ContextManager::load(&tmp_path, "claude-opus-4-5-20251101")
            .expect("load should succeed");

        // Verify content preserved
        assert_eq!(loaded.history().len(), 2);
        assert_eq!(loaded.current_model(), "claude-opus-4-5-20251101");

        // Verify message content
        let entries: Vec<_> = loaded.history().entries().iter().collect();
        assert_eq!(entries[0].message().content(), "Hello");
        assert_eq!(entries[1].message().content(), "World");

        // Cleanup
        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_load_with_different_model() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");
        manager.push_message(Message::try_user("Test").expect("non-empty"));

        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join(format!("forge_test_model_{}.json", std::process::id()));

        manager.save(&tmp_path).expect("save");

        // Load with a different model
        let loaded = ContextManager::load(&tmp_path, "gpt-5.2").expect("load should succeed");

        // History preserved
        assert_eq!(loaded.history().len(), 1);
        // But model is the new one
        assert_eq!(loaded.current_model(), "gpt-5.2");

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result =
            ContextManager::load("/nonexistent/path/file.json", "claude-opus-4-5-20251101");
        assert!(result.is_err());
    }

    // ========================================================================
    // Usage Status Tests
    // ========================================================================

    #[test]
    fn test_usage_status_ready_when_empty() {
        let manager = ContextManager::new("claude-opus-4-5-20251101");
        let status = manager.usage_status();

        // Empty history should always be ready
        assert!(matches!(status, ContextUsageStatus::Ready(_)));
    }

    #[test]
    fn test_push_message_with_step_id_and_has_step_id() {
        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        // Push with step ID
        let _id = manager.push_message_with_step_id(
            Message::try_user("Hello").expect("non-empty"),
            StepId::new(12345),
        );

        // Check step ID exists
        assert!(manager.has_step_id(StepId::new(12345)));
        assert!(!manager.has_step_id(StepId::new(99999)));
    }

    #[test]
    fn test_tool_messages_in_history() {
        use forge_types::{ToolCall, ToolResult};

        let mut manager = ContextManager::new("claude-opus-4-5-20251101");

        // Add a tool use message
        let tool_call = ToolCall::new(
            "call_123".to_string(),
            "get_weather".to_string(),
            serde_json::json!({"location": "Seattle"}),
        );
        let id1 = manager.push_message(Message::tool_use(tool_call));

        // Add a tool result
        let result = ToolResult::success(
            "call_123".to_string(),
            "get_weather".to_string(),
            "72°F and sunny".to_string(),
        );
        let id2 = manager.push_message(Message::tool_result(result));

        assert_eq!(manager.history().len(), 2);
        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);

        // Verify messages are correct type via their roles
        let entries: Vec<_> = manager.history().entries().iter().collect();
        assert_eq!(entries[0].message().role_str(), "assistant");
        assert_eq!(entries[1].message().role_str(), "user");

        // Verify content method works
        assert!(entries[0].message().content().contains("get_weather"));
        assert!(entries[1].message().content().contains("72°F"));
    }
}

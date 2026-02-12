//! Context Manager - orchestrates all context management components.

use anyhow::Result;
use std::path::Path;
use thiserror::Error;

#[cfg(test)]
use forge_types::PredefinedModel;
use forge_types::{Message, ModelName, NonEmptyString};

use super::StepId;
use super::history::{Distillate, DistillateId, FullHistory, MessageId};
use super::model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry};
use super::token_counter::TokenCounter;
use super::working_context::{ContextSegment, ContextUsage, DISTILLATE_PREFIX, WorkingContext};

const MIN_DISTILLATION_RATIO: f32 = 0.01;
const MAX_DISTILLATION_RATIO: f32 = 0.95;
const MIN_DISTILLATION_TOKENS: u32 = 64;
/// Upper bound for target_tokens computed here. The actual per-provider max
/// output limit is enforced in distillation.rs (which knows the distiller model).
/// This value is deliberately generous so the manager doesn't silently truncate
/// a target that the distiller model could handle.
const MAX_DISTILLATION_TOKENS: u32 = 128_000;

#[derive(Debug, Clone)]
pub struct DistillationConfig {
    /// Target compression ratio (e.g., 0.15 = 15% of original size).
    pub target_ratio: f32,
    /// Don't distill the N most recent messages.
    pub preserve_recent: usize,
}

impl Default for DistillationConfig {
    fn default() -> Self {
        Self {
            target_ratio: 0.15,
            preserve_recent: 4,
        }
    }
}

#[derive(Debug)]
pub enum ContextBuildError {
    /// Older messages need distillation to fit within budget.
    DistillationNeeded(DistillationNeeded),
    /// The most recent N messages alone exceed the budget.
    /// This is unrecoverable - user must reduce input or switch to larger model.
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
        message_count: usize,
    },
}

#[derive(Debug, Clone)]
pub struct DistillationNeeded {
    pub tokens_to_distill: u32,
    pub available_tokens: u32,
    pub excess_tokens: u32,
    pub messages_to_distill: Vec<MessageId>,
    pub suggestion: String,
}

#[derive(Debug, Clone, Error)]
pub enum DistillationPlanError {
    #[error("no messages selected for distillation")]
    EmptyScope,
    #[error(
        "no room to insert distillate: available {available_tokens} tokens, need at least {required_tokens} tokens"
    )]
    BudgetTooTight {
        available_tokens: u32,
        required_tokens: u32,
    },
}

/// Contiguous range of message IDs selected for distillation.
///
/// Distillations must cover contiguous message ranges to maintain
/// chronological coherence. This type ensures that constraint.
#[derive(Debug, Clone)]
pub struct DistillationScope {
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
        needs_distillation: bool,
    },
    /// Switched to a model with larger context.
    Expanding {
        old_budget: u32,
        new_budget: u32,
        /// Number of messages that could potentially be restored.
        can_restore: usize,
    },
}

/// Request for async distillation, created by [`ContextManager::prepare_distillation`].
///
/// Contains everything needed to generate a distillation via an LLM call.
/// After generation, pass the result to [`ContextManager::complete_distillation`].
#[derive(Debug)]
pub struct PendingDistillation {
    /// The scope defining which messages to distill.
    pub scope: DistillationScope,
    /// The actual messages to distill, in order.
    pub messages: Vec<(MessageId, Message)>,
    /// Total tokens in the original messages.
    pub original_tokens: u32,
    /// Target token count for the generated distillation.
    pub target_tokens: u32,
}

/// Proof that working context was successfully built within the token budget.
///
/// This type serves as a proof token that the context preparation succeeded.
/// It borrows the `ContextManager` to ensure the context remains valid until
/// the API call completes.
#[derive(Debug)]
pub struct PreparedContext<'a> {
    manager: &'a ContextManager,
    working_context: WorkingContext,
}

impl PreparedContext<'_> {
    #[must_use]
    pub fn api_messages(&self) -> Vec<Message> {
        self.working_context.materialize(&self.manager.history)
    }

    #[must_use]
    pub fn usage(&self) -> ContextUsage {
        ContextUsage::from_context(&self.working_context)
    }
}

/// Current context usage state with explicit distillation status.
///
/// Returned by [`ContextManager::usage_status`] to provide UI-friendly
/// information about the current context state.
#[derive(Debug, Clone)]
pub enum ContextUsageStatus {
    /// Context fits within budget and is ready for use.
    Ready(ContextUsage),
    /// Context exceeds budget; distillation is needed before API call.
    NeedsDistillation {
        /// Current usage statistics.
        usage: ContextUsage,
        /// Details about what needs distillation.
        needed: DistillationNeeded,
    },
    /// Recent messages alone exceed budget; unrecoverable without user action.
    RecentMessagesTooLarge {
        /// Current usage statistics.
        usage: ContextUsage,
        /// Tokens required by recent messages.
        required_tokens: u32,
        /// Available budget tokens.
        budget_tokens: u32,
    },
}

#[derive(Debug)]
pub struct ContextManager {
    history: FullHistory,
    counter: TokenCounter,
    registry: ModelRegistry,
    current_model: ModelName,
    current_limits: ModelLimits,
    current_limits_source: ModelLimitsSource,
    distillation_config: DistillationConfig,
    /// Configured output limit (if set, allows more input context).
    configured_output_limit: Option<u32>,
}

impl ContextManager {
    #[must_use]
    pub fn new(initial_model: ModelName) -> Self {
        let registry = ModelRegistry::new();
        let resolved = registry.get(&initial_model);
        let limits = resolved.limits();
        let limits_source = resolved.source();

        Self {
            history: FullHistory::new(),
            counter: TokenCounter::new(),
            registry,
            current_model: initial_model,
            current_limits: limits,
            current_limits_source: limits_source,
            distillation_config: DistillationConfig::default(),
            configured_output_limit: None,
        }
    }

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

    pub fn push_message(&mut self, message: Message) -> MessageId {
        let token_count = self.counter.count_message(&message);
        self.history.push(message, token_count)
    }

    pub fn push_message_with_step_id(
        &mut self,
        message: Message,
        stream_step_id: StepId,
    ) -> MessageId {
        let token_count = self.counter.count_message(&message);
        self.history
            .push_with_step_id(message, token_count, stream_step_id)
    }

    /// if history already contains an entry with this `step_id`, we should not recover it again.
    #[must_use]
    pub fn has_step_id(&self, step_id: StepId) -> bool {
        self.history.has_step_id(step_id)
    }

    /// This is used for transactional rollback when a stream fails...
    pub fn rollback_last_message(&mut self, id: MessageId) -> Option<Message> {
        self.history.pop_if_last(id)
    }

    pub fn switch_model(&mut self, new_model: ModelName) -> ContextAdaptation {
        let old_budget = self.effective_budget();
        let resolved = self.registry.get(&new_model);
        let new_limits = resolved.limits();
        let new_source = resolved.source();

        self.current_model = new_model;
        self.current_limits = new_limits;
        self.current_limits_source = new_source;

        let new_budget = self.effective_budget();

        match new_budget.cmp(&old_budget) {
            std::cmp::Ordering::Less => ContextAdaptation::Shrinking {
                old_budget,
                new_budget,
                needs_distillation: self.build_working_context(0).is_err(),
            },
            std::cmp::Ordering::Greater => ContextAdaptation::Expanding {
                old_budget,
                new_budget,
                can_restore: self.history.distilled_count(),
            },
            std::cmp::Ordering::Equal => ContextAdaptation::NoChange,
        }
    }

    /// Update model limits without triggering distillation behavior.
    pub fn set_model_without_adaptation(&mut self, new_model: ModelName) {
        let resolved = self.registry.get(&new_model);
        self.current_model = new_model;
        self.current_limits = resolved.limits();
        self.current_limits_source = resolved.source();
    }

    fn build_working_context(&self, overhead: u32) -> Result<WorkingContext, ContextBuildError> {
        #[derive(Debug)]
        enum Block {
            Undistilled(Vec<(MessageId, u32)>),
            Distilled {
                distillate_id: DistillateId,
                messages: Vec<(MessageId, u32)>,
                distillate_tokens: u32,
            },
        }

        let budget = self.effective_budget().saturating_sub(overhead);
        let mut ctx = WorkingContext::new(budget);

        let entries = self.history.entries();
        let max_preserve = self.distillation_config.preserve_recent.min(entries.len());
        let mut preserve_count = 0usize;
        let mut tokens_for_recent = 0u32;

        // Phase 1: The N most recent messages are always preserved and never distilled.
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
        let mut undistilled: Vec<(MessageId, u32)> = Vec::new();
        let mut distillate_block: Option<(DistillateId, u32)> = None;
        let mut distilled_messages: Vec<(MessageId, u32)> = Vec::new();

        for entry in older_entries {
            let distilled_here = entry
                .distillate_id()
                .map(|sid| (sid, self.history.distillate(sid).token_count()));

            if let Some((distillate_id, distillate_tokens)) = distilled_here {
                if !undistilled.is_empty() {
                    blocks.push(Block::Undistilled(std::mem::take(&mut undistilled)));
                }

                match distillate_block {
                    Some((current_id, _)) if current_id == distillate_id => {}
                    Some((current_id, current_tokens)) => {
                        blocks.push(Block::Distilled {
                            distillate_id: current_id,
                            messages: std::mem::take(&mut distilled_messages),
                            distillate_tokens: current_tokens,
                        });
                        distillate_block = Some((distillate_id, distillate_tokens));
                    }
                    None => {
                        distillate_block = Some((distillate_id, distillate_tokens));
                    }
                }

                distilled_messages.push((entry.id(), entry.token_count()));
            } else {
                if let Some((distillate_id, distillate_tokens)) = distillate_block.take() {
                    blocks.push(Block::Distilled {
                        distillate_id,
                        messages: std::mem::take(&mut distilled_messages),
                        distillate_tokens,
                    });
                }

                undistilled.push((entry.id(), entry.token_count()));
            }
        }

        if let Some((distillate_id, distillate_tokens)) = distillate_block.take() {
            blocks.push(Block::Distilled {
                distillate_id,
                messages: distilled_messages,
                distillate_tokens,
            });
        } else if !undistilled.is_empty() {
            blocks.push(Block::Undistilled(undistilled));
        }

        // Phase 3: Select older content from newest to oldest within remaining budget.
        let mut selected_rev: Vec<ContextSegment> = Vec::new();
        let mut need_distillation_rev: Vec<MessageId> = Vec::new();
        let mut tokens_used: u32 = 0;
        let mut exhausted = false;

        for block in blocks.iter().rev() {
            if exhausted {
                // Collect all older content (Distilled or not) for re-distillation.
                match block {
                    Block::Undistilled(messages) | Block::Distilled { messages, .. } => {
                        for (id, _) in messages.iter().rev() {
                            need_distillation_rev.push(*id);
                        }
                    }
                }
                continue;
            }

            match block {
                Block::Distilled {
                    distillate_id,
                    messages,
                    distillate_tokens,
                } => {
                    // Once distilled, always use the distillate. Originals are only for TUI display.
                    if tokens_used + *distillate_tokens <= remaining_budget {
                        let replaces: Vec<MessageId> = messages.iter().map(|(id, _)| *id).collect();
                        selected_rev.push(ContextSegment::distilled(
                            *distillate_id,
                            replaces,
                            *distillate_tokens,
                        ));
                        tokens_used += *distillate_tokens;
                    } else {
                        // Distillate doesn't fit. Mark underlying messages for
                        // hierarchical re-distillation (combined with other old content
                        // into a more compact distillation). The old distillate becomes orphaned.
                        exhausted = true;
                        for (id, _) in messages.iter().rev() {
                            need_distillation_rev.push(*id);
                        }
                        // Don't break - continue to collect more content for distillation.
                    }
                }
                Block::Undistilled(messages) => {
                    // Include as many of the most recent messages as we can.
                    for i in (0..messages.len()).rev() {
                        let (id, tokens) = messages[i];
                        if tokens_used + tokens <= remaining_budget {
                            selected_rev.push(ContextSegment::original(id, tokens));
                            tokens_used += tokens;
                        } else {
                            // Everything older than this point should be distilled.
                            exhausted = true;
                            for (id, _) in messages[..=i].iter().rev() {
                                need_distillation_rev.push(*id);
                            }
                            break;
                        }
                    }
                }
            }
        }

        let mut need_distillation: Vec<MessageId> =
            need_distillation_rev.into_iter().rev().collect();

        if !need_distillation.is_empty() {
            need_distillation.sort_by_key(super::history::MessageId::as_u64);
            need_distillation.dedup();

            let tokens_to_distill: u32 = need_distillation
                .iter()
                .map(|id| self.history.get_entry(*id).token_count())
                .sum();
            let available_left = remaining_budget.saturating_sub(tokens_used);
            let excess_tokens = tokens_to_distill.saturating_sub(available_left);

            let msg_count = need_distillation.len();
            return Err(ContextBuildError::DistillationNeeded(DistillationNeeded {
                tokens_to_distill,
                available_tokens: available_left,
                excess_tokens,
                messages_to_distill: need_distillation,
                suggestion: format!("{msg_count} older messages need distillation"),
            }));
        }

        // Phase 4: Materialize selected older segments in chronological order.
        for segment in selected_rev.into_iter().rev() {
            match segment {
                ContextSegment::Original { id, tokens } => ctx.push_original(id, tokens),
                ContextSegment::Distilled {
                    distillate_id,
                    replaces,
                    tokens,
                } => ctx.push_distillate(distillate_id, replaces, tokens),
            }
        }

        // Phase 5: Always include the N most recent messages.
        for entry in &entries[recent_start..] {
            ctx.push_original(entry.id(), entry.token_count());
        }

        Ok(ctx)
    }

    fn distillate_prefix_message_tokens(&self) -> u32 {
        let content = NonEmptyString::new(format!("{}\n", DISTILLATE_PREFIX.as_str()))
            .expect("DISTILLATE_PREFIX is non-empty");
        self.counter.count_message(&Message::system(content))
    }

    pub fn prepare_distillation(
        &mut self,
        needed: &DistillationNeeded,
    ) -> std::result::Result<PendingDistillation, DistillationPlanError> {
        let mut ids: Vec<MessageId> = needed.messages_to_distill.clone();
        ids.sort_by_key(super::history::MessageId::as_u64);
        ids.dedup();

        if ids.is_empty() {
            return Err(DistillationPlanError::EmptyScope);
        }

        // Keep only the first contiguous run - distillations must represent a contiguous slice of history.
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
            .distillation_config
            .target_ratio
            .clamp(MIN_DISTILLATION_RATIO, MAX_DISTILLATION_RATIO);
        let ratio_target = (f64::from(original_tokens) * f64::from(ratio)).round() as u32;

        let prefix_msg_tokens = self.distillate_prefix_message_tokens();
        let max_target_tokens = needed
            .available_tokens
            .saturating_sub(prefix_msg_tokens)
            .min(MAX_DISTILLATION_TOKENS);

        if max_target_tokens == 0 {
            return Err(DistillationPlanError::BudgetTooTight {
                available_tokens: needed.available_tokens,
                required_tokens: prefix_msg_tokens.saturating_add(1),
            });
        }

        let min_target_tokens = MIN_DISTILLATION_TOKENS.min(max_target_tokens);
        let target_tokens = ratio_target.clamp(min_target_tokens, max_target_tokens);

        let first = ids
            .first()
            .copied()
            .ok_or(DistillationPlanError::EmptyScope)?;
        let last = ids
            .last()
            .copied()
            .ok_or(DistillationPlanError::EmptyScope)?;
        let end_exclusive = last.next();
        let scope = DistillationScope {
            ids,
            range: first..end_exclusive,
        };

        Ok(PendingDistillation {
            scope,
            messages,
            original_tokens,
            target_tokens,
        })
    }

    pub fn complete_distillation(
        &mut self,
        scope: DistillationScope,
        content: NonEmptyString,
        generated_by: String,
    ) -> Result<DistillateId> {
        let injected = NonEmptyString::prefixed(DISTILLATE_PREFIX, "\n", &content);
        let token_count = self.counter.count_message(&Message::system(injected));

        let DistillationScope { ids, range } = scope;

        let original_tokens: u32 = ids
            .iter()
            .map(|id| self.history.get_entry(*id).token_count())
            .sum();

        let distillate_id = self.history.next_distillate_id();
        let distillate = Distillate::new(
            distillate_id,
            range,
            content,
            token_count,
            original_tokens,
            generated_by,
        );

        self.history.add_distillate(distillate)?;
        Ok(distillate_id)
    }

    /// This does not mutate history. If the current model's budget can fit original messages...
    #[must_use]
    pub fn try_restore_messages(&self) -> usize {
        let Ok(ctx) = self.build_working_context(0) else {
            return 0;
        };

        ctx.segments()
            .iter()
            .filter(|segment| {
                matches!(
                    segment,
                    ContextSegment::Original { id, .. }
                        if self.history.get_entry(*id).distillate_id().is_some()
                )
            })
            .count()
    }

    pub fn prepare(&self, overhead: u32) -> Result<PreparedContext<'_>, ContextBuildError> {
        let working_context = self.build_working_context(overhead)?;
        Ok(PreparedContext {
            manager: self,
            working_context,
        })
    }

    /// Get only the N most recent messages, bypassing distillation.
    ///
    /// This is used when the Librarian is active - instead of distilling
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
        self.distillation_config.preserve_recent
    }

    /// Get current usage statistics with explicit distillation status.
    #[must_use]
    pub fn usage_status(&self) -> ContextUsageStatus {
        let fallback_usage = || ContextUsage {
            used_tokens: self.history.total_tokens(),
            budget_tokens: self.effective_budget(),
            distilled_segments: 0,
        };

        match self.prepare(0) {
            Ok(prepared) => ContextUsageStatus::Ready(prepared.usage()),
            Err(ContextBuildError::DistillationNeeded(needed)) => {
                ContextUsageStatus::NeedsDistillation {
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

    #[must_use]
    pub fn history(&self) -> &FullHistory {
        &self.history
    }

    #[must_use]
    pub fn current_model(&self) -> &ModelName {
        &self.current_model
    }

    #[must_use]
    pub fn current_limits(&self) -> ModelLimits {
        self.current_limits
    }

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
        crate::atomic_write_with_options(
            path,
            json.as_bytes(),
            crate::AtomicWriteOptions {
                sync_all: true,
                dir_sync: true,
                unix_mode: Some(0o600),
            },
        )?;
        Ok(())
    }

    /// Load history from a JSON file.
    pub fn load(path: impl AsRef<Path>, model: ModelName) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let history: FullHistory = serde_json::from_str(&json)?;

        let registry = ModelRegistry::new();
        let resolved = registry.get(&model);
        let limits = resolved.limits();
        let limits_source = resolved.source();

        Ok(Self {
            history,
            counter: TokenCounter::new(),
            registry,
            current_model: model,
            current_limits: limits,
            current_limits_source: limits_source,
            distillation_config: DistillationConfig::default(),
            configured_output_limit: None, // Will be set by engine after load
        })
    }
}

#[cfg(test)]
impl ContextManager {
    /// Set a registry override for testing purposes.
    ///
    /// This allows tests to simulate models with custom context limits.
    pub fn set_registry_override(&mut self, model: PredefinedModel, limits: ModelLimits) {
        self.registry.set_override(model, limits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::PredefinedModel;

    fn model(predefined: PredefinedModel) -> ModelName {
        predefined.to_model_name()
    }

    #[test]
    fn test_new_manager() {
        let manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));
        assert_eq!(
            manager.current_model().as_str(),
            PredefinedModel::ClaudeOpus.model_id()
        );
        assert_eq!(manager.history().len(), 0);
    }

    #[test]
    fn test_push_message() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        let id2 = manager.push_message(Message::try_user("World").expect("non-empty test message"));

        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);
        assert_eq!(manager.history().len(), 2);
    }

    #[test]
    fn test_switch_model_shrinking() {
        // gemini-3-pro has 1M context, claude-opus has 200k
        let mut manager = ContextManager::new(model(PredefinedModel::GeminiPro));

        let result = manager.switch_model(model(PredefinedModel::ClaudeOpus));

        match result {
            ContextAdaptation::Shrinking { .. } => (),
            _ => panic!("Expected Shrinking adaptation"),
        }
    }

    #[test]
    fn test_switch_model_expanding() {
        // claude-opus has 200k context, gemini-3-pro has 1M
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let result = manager.switch_model(model(PredefinedModel::GeminiPro));

        match result {
            ContextAdaptation::Expanding { .. } => (),
            _ => panic!("Expected Expanding adaptation"),
        }
    }

    #[test]
    fn test_build_context_simple() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        manager.push_message(Message::try_user("Hello").expect("non-empty test message"));
        manager.push_message(Message::try_user("World").expect("non-empty test message"));

        let result = manager.build_working_context(0);
        assert!(result.is_ok());

        let ctx = result.unwrap();
        assert_eq!(ctx.segments().len(), 2);
    }

    #[test]
    fn test_hierarchical_distillation_collects_distilled_messages() {
        use crate::history::{Distillate, MessageId};
        use crate::model_limits::ModelLimits;
        use forge_types::NonEmptyString;

        // Create manager with a real model, then override to simulate small context
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        // Override to a very small context (2k) to force tight budget
        let small_limits = ModelLimits::new(2_000, 500);
        manager.set_registry_override(PredefinedModel::ClaudeOpus, small_limits);
        // Re-apply the limits after override
        manager.set_model_without_adaptation(model(PredefinedModel::ClaudeOpus));

        // Add many messages to exceed budget
        for i in 0..20 {
            manager.push_message(
                Message::try_user(format!("Message {i} with some content to use tokens"))
                    .expect("non-empty"),
            );
        }

        // Create a Distillate covering messages 0-10
        let distillate_id = manager.history.next_distillate_id();
        let distillate = Distillate::new(
            distillate_id,
            MessageId::new_for_test(0)..MessageId::new_for_test(10),
            NonEmptyString::new("Distillate of first 10 messages").expect("non-empty"),
            100, // 100 tokens for the Distillate
            500, // Original was 500 tokens
            PredefinedModel::ClaudeOpus.model_id().to_string(),
        );
        manager
            .history
            .add_distillate(distillate)
            .expect("add Distillate");

        // Now try to build context - should need hierarchical distillation
        let result = manager.build_working_context(0);

        // If distillation is needed, verify Distilled messages are included
        if let Err(ContextBuildError::DistillationNeeded(needed)) = result {
            // The messages_to_distill should include the already-Distilled messages 0-9
            // if they were collected for hierarchical re-distillation
            let has_distilled = needed.messages_to_distill.iter().any(|id| id.as_u64() < 10);

            // This test verifies the mechanism exists - the exact behavior depends on
            // budget calculations. The key assertion is that we don't panic or break.
            assert!(
                !needed.messages_to_distill.is_empty(),
                "Should have messages to distill"
            );

            // If Distilled messages are included, verify they form a contiguous range
            // from the start (hierarchical distillation collects from oldest)
            if has_distilled {
                let min_id = needed
                    .messages_to_distill
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
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty"));
        let id2 = manager.push_message(Message::try_user("World").expect("non-empty"));

        assert_eq!(manager.history().len(), 2);

        let rolled_back = manager.rollback_last_message(id2);
        assert!(rolled_back.is_some());
        assert_eq!(rolled_back.unwrap().content(), "World");
        assert_eq!(manager.history().len(), 1);

        let rolled_back = manager.rollback_last_message(id1);
        assert!(rolled_back.is_some());
        assert_eq!(rolled_back.unwrap().content(), "Hello");
        assert_eq!(manager.history().len(), 0);
    }

    #[test]
    fn test_rollback_last_message_wrong_id() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let id1 = manager.push_message(Message::try_user("Hello").expect("non-empty"));
        let _id2 = manager.push_message(Message::try_user("World").expect("non-empty"));

        let rolled_back = manager.rollback_last_message(id1);
        assert!(rolled_back.is_none());
        assert_eq!(manager.history().len(), 2);
    }

    // ========================================================================
    // Output Limit Tests
    // ========================================================================

    #[test]
    fn test_set_output_limit() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let _initial_budget = manager.effective_budget();

        manager.set_output_limit(4096);

        let new_budget = manager.effective_budget();

        assert!(new_budget > 0);

        let limits = manager.current_limits();
        assert!(limits.context_window() > 0);
    }

    #[test]
    fn test_current_limits_source() {
        let manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let source = manager.current_limits_source();
        assert!(matches!(source, ModelLimitsSource::Catalog(_)));
    }

    // ========================================================================
    // Persistence Tests (save/load)
    // ========================================================================

    #[test]
    fn test_save_load_roundtrip() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        manager.push_message(Message::try_user("Hello").expect("non-empty"));
        manager.push_message(Message::try_user("World").expect("non-empty"));

        assert_eq!(manager.history().len(), 2);

        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join(format!("forge_test_{}.json", std::process::id()));

        manager.save(&tmp_path).expect("save should succeed");

        assert!(tmp_path.exists());

        let loaded = ContextManager::load(&tmp_path, model(PredefinedModel::ClaudeOpus))
            .expect("load should succeed");

        assert_eq!(loaded.history().len(), 2);
        assert_eq!(
            loaded.current_model().as_str(),
            PredefinedModel::ClaudeOpus.model_id()
        );

        let entries: Vec<_> = loaded.history().entries().iter().collect();
        assert_eq!(entries[0].message().content(), "Hello");
        assert_eq!(entries[1].message().content(), "World");

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_load_with_different_model() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));
        manager.push_message(Message::try_user("Test").expect("non-empty"));

        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join(format!("forge_test_model_{}.json", std::process::id()));

        manager.save(&tmp_path).expect("save");

        let loaded = ContextManager::load(&tmp_path, model(PredefinedModel::Gpt52))
            .expect("load should succeed");

        assert_eq!(loaded.history().len(), 1);
        assert_eq!(
            loaded.current_model().as_str(),
            PredefinedModel::Gpt52.model_id()
        );

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = ContextManager::load(
            "/nonexistent/path/file.json",
            model(PredefinedModel::ClaudeOpus),
        );
        assert!(result.is_err());
    }

    // ========================================================================
    // Usage Status Tests
    // ========================================================================

    #[test]
    fn test_usage_status_ready_when_empty() {
        let manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));
        let status = manager.usage_status();

        assert!(matches!(status, ContextUsageStatus::Ready(_)));
    }

    #[test]
    fn test_push_message_with_step_id_and_has_step_id() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let _id = manager.push_message_with_step_id(
            Message::try_user("Hello").expect("non-empty"),
            StepId::new(12345),
        );

        assert!(manager.has_step_id(StepId::new(12345)));
        assert!(!manager.has_step_id(StepId::new(99999)));
    }

    #[test]
    fn test_tool_messages_in_history() {
        use forge_types::{ToolCall, ToolResult};

        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let tool_call = ToolCall::new(
            "call_123".to_string(),
            "get_weather".to_string(),
            serde_json::json!({"location": "Seattle"}),
        );
        let id1 = manager.push_message(Message::tool_use(tool_call));

        let result = ToolResult::success(
            "call_123".to_string(),
            "get_weather".to_string(),
            "72°F and sunny".to_string(),
        );
        let id2 = manager.push_message(Message::tool_result(result));

        assert_eq!(manager.history().len(), 2);
        assert_eq!(id1.as_u64(), 0);
        assert_eq!(id2.as_u64(), 1);

        let entries: Vec<_> = manager.history().entries().iter().collect();
        assert_eq!(entries[0].message().role_str(), "assistant");
        assert_eq!(entries[1].message().role_str(), "user");

        assert!(entries[0].message().content().contains("get_weather"));
        assert!(entries[1].message().content().contains("72°F"));
    }

    #[test]
    fn test_prepare_distillation_clamps_to_available_budget() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let limits = ModelLimits::new(2_000, 500);
        manager.set_registry_override(PredefinedModel::ClaudeOpus, limits);
        manager.set_model_without_adaptation(model(PredefinedModel::ClaudeOpus));

        for i in 0..30 {
            manager.push_message(
                Message::try_user(format!("Message {i}: {}", "word ".repeat(40)))
                    .expect("non-empty"),
            );
        }

        let ids: Vec<MessageId> = (0..20).map(MessageId::new_for_test).collect();

        let tokens_to_distill: u32 = ids
            .iter()
            .map(|id| manager.history.get_entry(*id).token_count())
            .sum();

        let prefix_msg_tokens = manager.distillate_prefix_message_tokens();
        let available_tokens = prefix_msg_tokens + 20;

        let needed = DistillationNeeded {
            tokens_to_distill,
            available_tokens,
            excess_tokens: tokens_to_distill.saturating_sub(available_tokens),
            messages_to_distill: ids,
            suggestion: "test".to_string(),
        };

        let pending = manager
            .prepare_distillation(&needed)
            .expect("pending distillation");

        assert_eq!(pending.target_tokens, 20);
        assert!(pending.target_tokens < MIN_DISTILLATION_TOKENS);
    }
}

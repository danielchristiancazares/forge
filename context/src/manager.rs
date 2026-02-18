//! Context Manager - orchestrates all context management components.

use anyhow::Result;
use std::path::Path;

#[cfg(test)]
use forge_types::PredefinedModel;
use forge_types::{Message, ModelName, NonEmptyString};

use super::StepId;
use super::history::{CompactionSummary, FullHistory, MessageId};
use super::model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry};
use super::token_counter::TokenCounter;
use super::working_context::{ContextUsage, WorkingContext};

#[derive(Debug)]
pub enum ContextBuildError {
    /// Context exceeds budget; compaction is needed.
    CompactionNeeded,
    /// The most recent messages alone exceed the budget.
    /// This is unrecoverable - user must reduce input or switch to larger model.
    RecentMessagesTooLarge {
        required_tokens: u32,
        budget_tokens: u32,
        message_count: usize,
    },
}

#[derive(Debug, Clone)]
pub enum ContextAdaptation {
    /// No change in effective budget.
    NoChange,
    /// Switched to a model with smaller context.
    Shrinking {
        old_budget: u32,
        new_budget: u32,
        needs_compaction: bool,
    },
    /// Switched to a model with larger context.
    Expanding { old_budget: u32, new_budget: u32 },
}

/// Plan for compaction: messages to compress and metadata.
#[derive(Debug)]
pub struct CompactionPlan {
    /// The actual messages to compact (summary + API entries), in order.
    pub messages: Vec<Message>,
    /// Total tokens in the original messages.
    pub original_tokens: u32,
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
        ContextUsage::from_context(&self.working_context, self.manager.history.is_compacted())
    }
}

/// Current context usage state.
///
/// Returned by [`ContextManager::usage_status`] to provide UI-friendly
/// information about the current context state.
#[derive(Debug, Clone)]
pub enum ContextUsageStatus {
    /// Context fits within budget and is ready for use.
    Ready(ContextUsage),
    /// Context exceeds budget; compaction is needed before API call.
    NeedsCompaction {
        /// Current usage statistics.
        usage: ContextUsage,
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
            configured_output_limit: None,
        }
    }

    /// This allows more input context when users configure smaller output limits.
    pub fn set_output_limit(&mut self, limit: u32) {
        self.configured_output_limit = Some(limit);
    }

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
                needs_compaction: self.build_working_context(0).is_err(),
            },
            std::cmp::Ordering::Greater => ContextAdaptation::Expanding {
                old_budget,
                new_budget,
            },
            std::cmp::Ordering::Equal => ContextAdaptation::NoChange,
        }
    }

    /// Update model limits without triggering adaptation behavior.
    pub fn set_model_without_adaptation(&mut self, new_model: ModelName) {
        let resolved = self.registry.get(&new_model);
        self.current_model = new_model;
        self.current_limits = resolved.limits();
        self.current_limits_source = resolved.source();
    }

    ///
    /// If the history is compacted, only entries after the compaction point
    /// are included. The compaction summary tokens count against the budget
    /// but are not segments — they're injected during materialization.
    fn build_working_context(&self, overhead: u32) -> Result<WorkingContext, ContextBuildError> {
        let budget = self.effective_budget().saturating_sub(overhead);
        let api_entries = self.history.api_entries();

        let summary_tokens = self
            .history
            .compaction_summary()
            .map_or(0, CompactionSummary::token_count);
        let api_tokens: u32 = api_entries
            .iter()
            .map(super::history::HistoryEntry::token_count)
            .sum();
        let total = summary_tokens.saturating_add(api_tokens);

        if total <= budget {
            let mut ctx = WorkingContext::new(budget);
            for entry in api_entries {
                ctx.push(entry.id(), entry.token_count());
            }
            Ok(ctx)
        } else if api_entries.len() <= 1 && self.history.is_compacted() {
            Err(ContextBuildError::RecentMessagesTooLarge {
                required_tokens: total,
                budget_tokens: budget,
                message_count: api_entries.len(),
            })
        } else {
            Err(ContextBuildError::CompactionNeeded)
        }
    }

    /// Prepare for compaction: collect all API-visible messages.
    #[must_use]
    pub fn prepare_compaction(&self) -> CompactionPlan {
        let mut messages = Vec::new();
        let mut total_tokens = 0u32;

        if let Some(summary) = self.history.compaction_summary() {
            messages.push(Message::system(summary.content_non_empty().clone()));
            total_tokens += summary.token_count();
        }

        for entry in self.history.api_entries() {
            messages.push(entry.message().clone());
            total_tokens += entry.token_count();
        }

        CompactionPlan {
            messages,
            original_tokens: total_tokens,
        }
    }

    /// Apply completed compaction.
    pub fn complete_compaction(&mut self, content: NonEmptyString, generated_by: String) {
        let token_count = self
            .counter
            .count_message(&Message::system(content.clone()));
        let summary = CompactionSummary::new(content, token_count, generated_by);
        self.history.compact(summary);
    }

    pub fn prepare(&self, overhead: u32) -> Result<PreparedContext<'_>, ContextBuildError> {
        let working_context = self.build_working_context(overhead)?;
        Ok(PreparedContext {
            manager: self,
            working_context,
        })
    }

    /// Get only the N most recent messages, bypassing compaction.
    ///
    /// This is used when the Librarian is active - instead of compacting
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

    #[must_use]
    pub fn usage_status(&self) -> ContextUsageStatus {
        let fallback_usage = || ContextUsage {
            used_tokens: self.history.total_tokens(),
            budget_tokens: self.effective_budget(),
            compacted: self.history.is_compacted(),
        };

        match self.prepare(0) {
            Ok(prepared) => ContextUsageStatus::Ready(prepared.usage()),
            Err(ContextBuildError::CompactionNeeded) => ContextUsageStatus::NeedsCompaction {
                usage: fallback_usage(),
            },
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

    /// Save history to a JSON file.
    ///
    /// Uses atomic write pattern: write to temp file, then rename.
    /// On Windows where rename-over-existing fails, uses backup-and-restore
    /// to prevent data loss if the final rename fails.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(&self.history)?;
        forge_utils::atomic_write_with_options(
            path,
            json.as_bytes(),
            forge_utils::AtomicWriteOptions {
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
            configured_output_limit: None,
        })
    }
}

#[cfg(test)]
impl ContextManager {
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
    fn test_compaction_needed_when_over_budget() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        let small_limits = ModelLimits::new(200, 50);
        manager.set_registry_override(PredefinedModel::ClaudeOpus, small_limits);
        manager.set_model_without_adaptation(model(PredefinedModel::ClaudeOpus));

        for i in 0..20 {
            manager.push_message(
                Message::try_user(format!("Message {i} with some content to use tokens"))
                    .expect("non-empty"),
            );
        }

        let result = manager.build_working_context(0);
        assert!(matches!(result, Err(ContextBuildError::CompactionNeeded)));
    }

    #[test]
    fn test_prepare_and_complete_compaction() {
        let mut manager = ContextManager::new(model(PredefinedModel::ClaudeOpus));

        manager.push_message(Message::try_user("Hello").expect("non-empty"));
        manager.push_message(Message::try_user("World").expect("non-empty"));

        let plan = manager.prepare_compaction();
        assert_eq!(plan.messages.len(), 2);
        assert!(plan.original_tokens > 0);

        manager.complete_compaction(
            NonEmptyString::new("Summary of conversation").expect("non-empty"),
            "test-model".to_string(),
        );

        assert!(manager.history().is_compacted());
        assert_eq!(manager.history().api_entries().len(), 0);
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
}

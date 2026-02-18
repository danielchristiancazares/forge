//! Long-term memory and adaptive context window management
//!
//! This module provides:
//! - Model-specific context window limits
//! - Exact token counting via tiktoken
//! - Full history preservation with distillation
//! - Stream journal for durability (crash recovery)
//! - Fact extraction and retrieval (Librarian)
//!
//! # Architecture
//!
//! ```text
//! ContextManager
//! ├── history: FullHistory (never discards messages)
//! ├── counter: TokenCounter (tiktoken)
//! ├── registry: ModelRegistry (limits per model)
//! └── journal: StreamJournal (streaming durability)
//!
//! PreparedContext (ephemeral proof)
//! └── working_context: WorkingContext (derived view for API)
//! ```

mod distillation;
mod fact_store;
mod history;
mod librarian;
mod manager;
mod model_limits;
mod sqlite_security;
mod stream_journal;
mod time_utils;
mod token_counter;
mod tool_journal;
mod working_context;

pub use distillation::{distillation_model, generate_distillation};
pub use fact_store::{FactId, FactStore, FactWithStaleness, StoredFact};
pub use history::{CompactionSummary, FullHistory, HistoryEntry, MessageId};
pub use librarian::{
    ExtractionResult, Fact, FactType, Librarian, RetrievalResult, extract_facts,
    format_facts_for_context, retrieve_relevant,
};
pub use manager::{
    CompactionPlan, ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus,
    PreparedContext,
};
pub use model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry, ResolvedModelLimits};
pub use stream_journal::{
    ActiveJournal, BeginSessionError, JournalStats, RecoveredStream, StepId, StreamJournal,
};
pub use token_counter::TokenCounter;
pub use tool_journal::{
    CorruptedToolArgs, RecoveredToolBatch, RecoveredToolCallExecution, ToolBatchId, ToolJournal,
};
pub use working_context::{ContextSegment, ContextUsage, WorkingContext};

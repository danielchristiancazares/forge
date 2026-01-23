//! `ContextInfinity` - Adaptive context window management
//!
//! This module provides:
//! - Model-specific context window limits
//! - Exact token counting via tiktoken
//! - Full history preservation with summarization
//! - Stream journal for durability (crash recovery)
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

mod fact_store;
mod history;
mod librarian;
mod manager;
mod model_limits;
mod stream_journal;
mod summarization;
mod token_counter;
mod tool_journal;
mod working_context;

pub use fact_store::{FactId, FactStore, FactWithStaleness, StoredFact};
pub use history::{FullHistory, HistoryEntry, MessageId, Summary, SummaryId};
pub use librarian::{
    ExtractionResult, Fact, FactType, Librarian, RetrievalResult, extract_facts,
    format_facts_for_context, retrieve_relevant,
};
pub use manager::{
    ContextAdaptation, ContextBuildError, ContextManager, ContextUsageStatus, PendingSummarization,
    PreparedContext, SummarizationNeeded, SummarizationScope,
};
pub use model_limits::{ModelLimits, ModelLimitsSource, ModelRegistry, ResolvedModelLimits};
pub use stream_journal::{ActiveJournal, JournalStats, RecoveredStream, StepId, StreamJournal};
pub use summarization::{generate_summary, summarization_model};
pub use token_counter::TokenCounter;
pub use tool_journal::{RecoveredToolBatch, ToolBatchId, ToolJournal};
pub use working_context::{ContextSegment, ContextUsage, WorkingContext};

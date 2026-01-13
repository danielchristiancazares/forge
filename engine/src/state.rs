//! Operation state machine types.
//!
//! This module contains the core state machine for tracking what the App is currently doing.
//! These types are internal to the engine crate.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use futures_util::future::AbortHandle;
use tokio::sync::mpsc;

use forge_context::{RecoveredToolBatch, StepId, SummarizationScope, ToolBatchId};
use forge_providers::ApiConfig;
use forge_types::{ModelName, ToolCall, ToolResult};

use crate::StreamingMessage;
use crate::tools::{self, ConfirmationRequest};

// ============================================================================
// Data Directory
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataDirSource {
    System,
    Fallback,
}

#[derive(Debug, Clone)]
pub(crate) struct DataDir {
    pub(crate) path: PathBuf,
    pub(crate) source: DataDirSource,
}

impl DataDir {
    pub(crate) fn join(&self, child: &str) -> PathBuf {
        self.path.join(child)
    }
}

// ============================================================================
// Streaming State
// ============================================================================

use crate::ActiveJournal;

#[derive(Debug)]
pub(crate) struct ActiveStream {
    pub(crate) message: StreamingMessage,
    pub(crate) journal: ActiveJournal,
    pub(crate) abort_handle: AbortHandle,
    pub(crate) tool_batch_id: Option<ToolBatchId>,
    pub(crate) tool_call_seq: usize,
}

// ============================================================================
// Summarization State
// ============================================================================

/// A background summarization task.
///
/// Holds the state for an in-progress summarization operation:
/// - The message IDs being summarized
/// - The `JoinHandle` for the async task
#[derive(Debug)]
pub struct SummarizationTask {
    pub(crate) scope: SummarizationScope,
    pub(crate) generated_by: String,
    pub(crate) handle: tokio::task::JoinHandle<anyhow::Result<String>>,
    pub(crate) attempt: u8,
}

#[derive(Debug)]
pub(crate) struct SummarizationRetry {
    pub(crate) attempt: u8,
    pub(crate) ready_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SummarizationStart {
    Started,
    NotNeeded,
    Failed,
}

#[derive(Debug)]
pub(crate) struct SummarizationState {
    pub(crate) task: SummarizationTask,
}

#[derive(Debug)]
pub(crate) struct SummarizationWithQueuedState {
    pub(crate) task: SummarizationTask,
    pub(crate) queued: ApiConfig,
}

#[derive(Debug)]
pub(crate) struct SummarizationRetryState {
    pub(crate) retry: SummarizationRetry,
}

#[derive(Debug)]
pub(crate) struct SummarizationRetryWithQueuedState {
    pub(crate) retry: SummarizationRetry,
    pub(crate) queued: ApiConfig,
}

// ============================================================================
// Tool Execution State
// ============================================================================

/// State for when the assistant has made tool calls and we're waiting for results.
///
/// This is similar to the Summarizing state - a pause in the conversation flow
/// while external processing occurs. Once all tool results are submitted,
/// the conversation resumes with the updated context.
#[derive(Debug)]
pub struct PendingToolExecution {
    /// Text content from assistant before/alongside tool calls (may be empty).
    pub assistant_text: String,
    /// Tool calls waiting for results.
    pub pending_calls: Vec<ToolCall>,
    /// Results received so far.
    pub results: Vec<ToolResult>,
    /// Model that made the tool calls.
    pub model: ModelName,
    /// Journal step ID for recovery.
    pub step_id: StepId,
    /// Tool batch journal ID.
    pub batch_id: ToolBatchId,
}

#[derive(Debug)]
pub(crate) struct ToolBatch {
    pub(crate) assistant_text: String,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) results: Vec<ToolResult>,
    pub(crate) model: ModelName,
    pub(crate) step_id: StepId,
    pub(crate) batch_id: ToolBatchId,
    pub(crate) iteration: u32,
    pub(crate) execute_now: Vec<ToolCall>,
    pub(crate) approval_calls: Vec<ToolCall>,
    pub(crate) approval_requests: Vec<ConfirmationRequest>,
}

#[derive(Debug)]
pub(crate) struct ApprovalState {
    pub(crate) requests: Vec<ConfirmationRequest>,
    pub(crate) selected: Vec<bool>,
    pub(crate) cursor: usize,
    pub(crate) deny_confirm: bool,
    pub(crate) expanded: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct ActiveToolExecution {
    pub(crate) queue: VecDeque<ToolCall>,
    pub(crate) current_call: Option<ToolCall>,
    pub(crate) join_handle: Option<tokio::task::JoinHandle<ToolResult>>,
    pub(crate) event_rx: Option<mpsc::Receiver<tools::ToolEvent>>,
    pub(crate) abort_handle: Option<AbortHandle>,
    pub(crate) output_lines: Vec<String>,
    pub(crate) remaining_capacity_bytes: usize,
}

#[derive(Debug)]
pub(crate) enum ToolLoopPhase {
    AwaitingApproval(ApprovalState),
    Executing(ActiveToolExecution),
}

#[derive(Debug)]
pub(crate) struct ToolLoopState {
    pub(crate) batch: ToolBatch,
    pub(crate) phase: ToolLoopPhase,
}

#[derive(Debug)]
pub(crate) struct ToolRecoveryState {
    pub(crate) batch: RecoveredToolBatch,
    pub(crate) step_id: StepId,
    pub(crate) model: ModelName,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ToolRecoveryDecision {
    Resume,
    Discard,
}

#[derive(Debug)]
pub(crate) struct ToolPlan {
    pub(crate) execute_now: Vec<ToolCall>,
    pub(crate) approval_calls: Vec<ToolCall>,
    pub(crate) approval_requests: Vec<ConfirmationRequest>,
    pub(crate) pre_resolved: Vec<ToolResult>,
}

// ============================================================================
// Main Operation State
// ============================================================================

/// Operation state - what the app is currently doing.
///
/// Note: `ContextInfinity` enablement is tracked separately via `App.context_infinity`,
/// not encoded in this enum. This prevents implicit feature toggling through state
/// transitions.
#[derive(Debug)]
pub(crate) enum OperationState {
    Idle,
    Streaming(ActiveStream),
    AwaitingToolResults(PendingToolExecution),
    ToolLoop(ToolLoopState),
    ToolRecovery(ToolRecoveryState),
    Summarizing(SummarizationState),
    SummarizingWithQueued(SummarizationWithQueuedState),
    SummarizationRetry(SummarizationRetryState),
    SummarizationRetryWithQueued(SummarizationRetryWithQueuedState),
}

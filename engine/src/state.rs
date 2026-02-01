//! Operation state machine types.
//!
//! This module contains the core state machine for tracking what the App is currently doing.
//! These types are internal to the engine crate.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use futures_util::future::AbortHandle;
use tokio::sync::mpsc;

use forge_context::{RecoveredToolBatch, StepId, SummarizationScope, ToolBatchId};
use forge_types::{ModelName, ToolCall, ToolResult};

use crate::StreamingMessage;
use crate::input_modes::{ChangeRecorder, TurnContext};
use crate::tools::{self, ConfirmationRequest};

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

use crate::ActiveJournal;

#[derive(Debug)]
pub(crate) struct ActiveStream {
    pub(crate) message: StreamingMessage,
    pub(crate) journal: ActiveJournal,
    pub(crate) abort_handle: AbortHandle,
    pub(crate) tool_batch_id: Option<ToolBatchId>,
    pub(crate) tool_call_seq: usize,
    /// Tracks bytes of tool arguments written to journal per call ID.
    /// When a call exceeds the limit, we stop appending to the journal.
    pub(crate) tool_args_journal_bytes: HashMap<String, usize>,
    pub(crate) turn: TurnContext,
}

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
    /// When present, a validated user request is waiting to be streamed once
    /// summarization completes.
    pub(crate) queued: Option<crate::QueuedUserMessage>,
}

#[derive(Debug)]
pub(crate) struct SummarizationRetryState {
    pub(crate) retry: SummarizationRetry,
    /// When present, a validated user request is waiting to be streamed once
    /// summarization completes.
    pub(crate) queued: Option<crate::QueuedUserMessage>,
}

#[derive(Debug)]
pub(crate) struct ToolBatch {
    pub(crate) assistant_text: String,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) results: Vec<ToolResult>,
    pub(crate) model: ModelName,
    pub(crate) step_id: StepId,
    /// Journal batch ID. None if journaling failed or was disabled.
    pub(crate) batch_id: Option<ToolBatchId>,
    pub(crate) execute_now: Vec<ToolCall>,
    pub(crate) approval_calls: Vec<ToolCall>,
    pub(crate) approval_requests: Vec<ConfirmationRequest>,
    pub(crate) turn: TurnContext,
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
    pub(crate) output_lines: HashMap<String, Vec<String>>,
    pub(crate) remaining_capacity_bytes: usize,
    pub(crate) turn_recorder: ChangeRecorder,
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

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum OperationState {
    Idle,
    Streaming(ActiveStream),
    ToolLoop(Box<ToolLoopState>),
    ToolRecovery(ToolRecoveryState),
    Summarizing(SummarizationState),
    SummarizationRetry(SummarizationRetryState),
}

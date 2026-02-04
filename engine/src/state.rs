//! Operation state machine types.
//!
//! This module contains the core state machine for tracking what the App is currently doing.
//! These types are internal to the engine crate.

use std::collections::HashMap;
use std::path::PathBuf;

use futures_util::future::AbortHandle;

use forge_context::{DistillationScope, RecoveredToolBatch, StepId, ToolBatchId};
use forge_types::{Message, ModelName, ToolCall, ToolResult};

use crate::StreamingMessage;
use crate::input_modes::TurnContext;
use crate::tool_loop::{ActiveExecution, ToolQueue};
use crate::tools::ConfirmationRequest;

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

/// A background distillation task.
///
/// Holds the state for an in-progress distillation operation:
/// - The message IDs being distilled
/// - The `JoinHandle` for the async task
#[derive(Debug)]
pub struct DistillationTask {
    pub(crate) scope: DistillationScope,
    pub(crate) generated_by: String,
    pub(crate) handle: tokio::task::JoinHandle<anyhow::Result<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DistillationStart {
    Started,
    NotNeeded,
    Failed,
}

#[derive(Debug)]
pub(crate) struct DistillationState {
    pub(crate) task: DistillationTask,
    /// When present, a validated user request is waiting to be streamed once
    /// distillation completes.
    pub(crate) queued: Option<crate::QueuedUserMessage>,
}

#[derive(Debug)]
pub(crate) struct ToolBatch {
    pub(crate) assistant_text: String,
    pub(crate) thinking_message: Option<Message>,
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

/// Tool loop phase state machine (IFA §8.1: State as Location).
///
/// # State Machine
/// ```text
/// ┌────────────────────────┐  approval given   ┌─────────────────────┐
/// │ AwaitingApproval(...)  │ ─────────────────> │ Processing(queue)   │
/// └────────────────────────┘                    └─────────────────────┘
///                                                      │
///                                                      │ spawn_next_tool()
///                                                      v
///                                               ┌─────────────────────┐
///                                               │ Executing(active)   │
///                                               └─────────────────────┘
///                                                      │
///                                                      │ tool completes
///                                                      v
///                                               ┌─────────────────────┐
///                                               │ Processing(queue)   │
///                                               └─────────────────────┘
///                                                      │
///                                      queue empty?    │
///                                                      v
///                                               [commit batch]
/// ```
#[derive(Debug)]
pub(crate) enum ToolLoopPhase {
    /// Awaiting user approval for dangerous tool calls.
    AwaitingApproval(ApprovalState),
    /// Has queue but no active execution (between tools or before first spawn).
    Processing(ToolQueue),
    /// Has active execution (SpawnedTool is required, not optional).
    Executing(ActiveExecution),
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
    Distilling(DistillationState),
}

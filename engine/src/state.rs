//! Operation state machine types.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    Custom,
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

/// Proof that the tool journal batch was persisted to disk.
///
/// Constructed only when `ToolJournal::begin_batch` or `update_assistant_text`
/// succeeds.  `handle_tool_calls` returns early (fail-closed) when persistence
/// fails, so this type is unreachable without a durable journal entry (IFA §10.1).
#[derive(Debug, Clone)]
pub(crate) struct JournalStatus(ToolBatchId);

impl JournalStatus {
    pub(crate) fn new(id: ToolBatchId) -> Self {
        Self(id)
    }

    pub(crate) fn batch_id(&self) -> ToolBatchId {
        self.0
    }
}

#[derive(Debug)]
pub(crate) struct ToolLoopInput {
    pub(crate) assistant_text: String,
    pub(crate) thinking_message: Option<Message>,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) pre_resolved: Vec<ToolResult>,
    pub(crate) model: ModelName,
    pub(crate) step_id: StepId,
    pub(crate) tool_batch_id: Option<ToolBatchId>,
    pub(crate) turn: TurnContext,
}

#[derive(Debug)]
pub(crate) struct ToolCommitPayload {
    pub(crate) assistant_text: String,
    pub(crate) thinking_message: Option<Message>,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) results: Vec<ToolResult>,
    pub(crate) model: ModelName,
    pub(crate) step_id: StepId,
    pub(crate) turn: TurnContext,
}

/// Buffered tool-argument deltas for the tool journal during streaming.
///
/// Providers may emit many tiny `ToolCallDelta` chunks; writing each chunk to
/// SQLite individually can cause UI stalls. This buffer accumulates deltas in
/// memory and flushes them in larger batches (see `engine/src/streaming.rs`).
#[derive(Debug)]
pub(crate) struct ToolArgsJournalBuffer {
    pending_by_call: HashMap<String, String>,
    flushed_calls: HashSet<String>,
    pending_bytes: usize,
    last_flush: Instant,
}

impl ToolArgsJournalBuffer {
    pub(crate) fn new() -> Self {
        Self {
            pending_by_call: HashMap::new(),
            flushed_calls: HashSet::new(),
            pending_bytes: 0,
            last_flush: Instant::now(),
        }
    }

    pub(crate) fn push_delta(&mut self, tool_call_id: &str, delta: &str) {
        self.pending_bytes = self.pending_bytes.saturating_add(delta.len());
        self.pending_by_call
            .entry(tool_call_id.to_string())
            .or_default()
            .push_str(delta);
    }

    pub(crate) fn should_flush(&self, byte_threshold: usize, interval: Duration) -> bool {
        // Flush immediately for the first delta of any call to preserve crash recovery
        // semantics while still buffering subsequent deltas for performance.
        let has_unflushed_call = self
            .pending_by_call
            .keys()
            .any(|id| !self.flushed_calls.contains(id));
        has_unflushed_call
            || self.pending_bytes >= byte_threshold
            || self.last_flush.elapsed() >= interval
    }

    pub(crate) fn take_pending(&mut self) -> Vec<(String, String)> {
        if self.pending_by_call.is_empty() {
            return Vec::new();
        }
        let pending = std::mem::take(&mut self.pending_by_call);
        self.pending_bytes = 0;
        self.last_flush = Instant::now();

        let mut out = Vec::with_capacity(pending.len());
        for (id, delta) in pending {
            self.flushed_calls.insert(id.clone());
            out.push((id, delta));
        }
        out
    }
}

/// Active streaming state with typestate encoding for journal status.
///
/// Transitions: Transient -> Journaled (when first tool call detected)
#[derive(Debug)]
pub(crate) enum ActiveStream {
    /// Stream without tool call journaling (no tool calls yet, or journaling failed).
    Transient {
        message: StreamingMessage,
        journal: ActiveJournal,
        abort_handle: AbortHandle,
        tool_call_seq: usize,
        tool_args_journal_bytes: HashMap<String, usize>,
        turn: TurnContext,
    },
    /// Stream with tool call journaling active (crash-recoverable).
    Journaled {
        tool_batch_id: ToolBatchId,
        message: StreamingMessage,
        journal: ActiveJournal,
        abort_handle: AbortHandle,
        tool_call_seq: usize,
        tool_args_journal_bytes: HashMap<String, usize>,
        tool_args_buffer: ToolArgsJournalBuffer,
        turn: TurnContext,
    },
}

impl ActiveStream {
    /// Transition from Transient to Journaled state (non-reversible).
    pub(crate) fn transition_to_journaled(self, batch_id: ToolBatchId) -> Self {
        match self {
            ActiveStream::Transient {
                message,
                journal,
                abort_handle,
                tool_call_seq,
                tool_args_journal_bytes,
                turn,
            } => ActiveStream::Journaled {
                tool_batch_id: batch_id,
                message,
                journal,
                abort_handle,
                tool_call_seq,
                tool_args_journal_bytes,
                tool_args_buffer: ToolArgsJournalBuffer::new(),
                turn,
            },
            // Already journaled - this shouldn't happen, but return unchanged
            ActiveStream::Journaled { .. } => self,
        }
    }

    /// Access common fields (available in both states).
    pub(crate) fn message(&self) -> &StreamingMessage {
        match self {
            ActiveStream::Transient { message, .. } | ActiveStream::Journaled { message, .. } => {
                message
            }
        }
    }

    pub(crate) fn message_mut(&mut self) -> &mut StreamingMessage {
        match self {
            ActiveStream::Transient { message, .. } | ActiveStream::Journaled { message, .. } => {
                message
            }
        }
    }

    pub(crate) fn journal(&self) -> &ActiveJournal {
        match self {
            ActiveStream::Transient { journal, .. } | ActiveStream::Journaled { journal, .. } => {
                journal
            }
        }
    }

    pub(crate) fn journal_mut(&mut self) -> &mut ActiveJournal {
        match self {
            ActiveStream::Transient { journal, .. } | ActiveStream::Journaled { journal, .. } => {
                journal
            }
        }
    }

    pub(crate) fn into_journal(self) -> ActiveJournal {
        match self {
            ActiveStream::Transient { journal, .. } | ActiveStream::Journaled { journal, .. } => {
                journal
            }
        }
    }

    pub(crate) fn abort_handle(&self) -> &AbortHandle {
        match self {
            ActiveStream::Transient { abort_handle, .. }
            | ActiveStream::Journaled { abort_handle, .. } => abort_handle,
        }
    }

    pub(crate) fn tool_call_seq(&self) -> usize {
        match self {
            ActiveStream::Transient { tool_call_seq, .. }
            | ActiveStream::Journaled { tool_call_seq, .. } => *tool_call_seq,
        }
    }

    pub(crate) fn increment_tool_call_seq(&mut self) {
        match self {
            ActiveStream::Transient { tool_call_seq, .. }
            | ActiveStream::Journaled { tool_call_seq, .. } => {
                *tool_call_seq = tool_call_seq.saturating_add(1);
            }
        }
    }

    pub(crate) fn tool_args_journal_bytes_mut(&mut self) -> &mut HashMap<String, usize> {
        match self {
            ActiveStream::Transient {
                tool_args_journal_bytes,
                ..
            }
            | ActiveStream::Journaled {
                tool_args_journal_bytes,
                ..
            } => tool_args_journal_bytes,
        }
    }

    pub(crate) fn tool_args_buffer_mut(&mut self) -> Option<&mut ToolArgsJournalBuffer> {
        match self {
            ActiveStream::Journaled {
                tool_args_buffer, ..
            } => Some(tool_args_buffer),
            ActiveStream::Transient { .. } => None,
        }
    }

    /// Consume self and return parts for cleanup.
    #[allow(clippy::type_complexity)]
    pub(crate) fn into_parts(
        self,
    ) -> (
        StreamingMessage,
        ActiveJournal,
        AbortHandle,
        Option<ToolBatchId>,
        TurnContext,
    ) {
        match self {
            ActiveStream::Transient {
                message,
                journal,
                abort_handle,
                turn,
                ..
            } => (message, journal, abort_handle, None, turn),
            ActiveStream::Journaled {
                message,
                journal,
                abort_handle,
                tool_batch_id,
                turn,
                ..
            } => (message, journal, abort_handle, Some(tool_batch_id), turn),
        }
    }
}

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

/// Distillation state with typestate encoding for message queueing.
///
/// Transitions: Running -> CompletedWithQueued (when user message arrives during distillation)
#[derive(Debug)]
pub(crate) enum DistillationState {
    /// Distillation in progress, no queued message.
    Running(DistillationTask),
    /// Distillation in progress, with a user message queued to stream after completion.
    CompletedWithQueued {
        task: DistillationTask,
        message: crate::QueuedUserMessage,
    },
}

impl DistillationState {
    /// Get the task reference (available in both states).
    pub(crate) fn task(&self) -> &DistillationTask {
        match self {
            DistillationState::Running(task)
            | DistillationState::CompletedWithQueued { task, .. } => task,
        }
    }

    /// Check if there's a queued user message.
    pub(crate) fn has_queued_message(&self) -> bool {
        matches!(self, DistillationState::CompletedWithQueued { .. })
    }
}

#[derive(Debug)]
pub(crate) struct ToolBatch {
    pub(crate) assistant_text: String,
    pub(crate) thinking_message: Option<Message>,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) results: Vec<ToolResult>,
    pub(crate) model: ModelName,
    pub(crate) step_id: StepId,
    /// Journal persistence status - determined at construction.
    pub(crate) journal_status: JournalStatus,
    pub(crate) execute_now: Vec<ToolCall>,
    pub(crate) approval_calls: Vec<ToolCall>,
    pub(crate) turn: TurnContext,
}

impl ToolBatch {
    pub(crate) fn into_commit(self) -> ToolCommitPayload {
        ToolCommitPayload {
            assistant_text: self.assistant_text,
            thinking_message: self.thinking_message,
            calls: self.calls,
            results: self.results,
            model: self.model,
            step_id: self.step_id,
            turn: self.turn,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ApprovalData {
    pub(crate) requests: Vec<ConfirmationRequest>,
    pub(crate) selected: Vec<bool>,
    pub(crate) cursor: usize,
    pub(crate) expanded: Option<usize>,
    pub(crate) scroll_offset: usize,
}

impl ApprovalData {
    pub(crate) fn new(requests: Vec<ConfirmationRequest>) -> Self {
        let len = requests.len();
        Self {
            requests,
            selected: vec![true; len],
            cursor: 0,
            expanded: None,
            scroll_offset: 0,
        }
    }
}

/// Approval workflow state machine (IFA §8.2: State transitions move between types).
///
/// # State Machine
/// ```text
/// ┌────────────────────┐  'd' pressed   ┌─────────────────────────┐
/// │ Selecting(data)    │ ─────────────> │ ConfirmingDeny(data)    │
/// └────────────────────┘                └─────────────────────────┘
///       ^                                      │
///       │  any key except 'd'/Enter            │ 'd' or Enter
///       └──────────────────────────────────────┘
///                                              v
///                                    [Execute denial - consume state]
/// ```
#[derive(Debug)]
pub(crate) enum ApprovalState {
    /// User is selecting which tools to approve/deny.
    Selecting(ApprovalData),
    /// User pressed 'd'; awaiting confirmation.
    ConfirmingDeny(ApprovalData),
}

impl ApprovalState {
    pub(crate) fn new(requests: Vec<ConfirmationRequest>) -> Self {
        Self::Selecting(ApprovalData::new(requests))
    }

    /// Read-only access to data (allowed in any phase).
    pub(crate) fn data(&self) -> &ApprovalData {
        match self {
            Self::Selecting(d) | Self::ConfirmingDeny(d) => d,
        }
    }

    /// Mutable access to data with automatic cancel of deny confirmation.
    /// Any mutation cancels the ConfirmingDeny state.
    pub(crate) fn data_mut(&mut self) -> &mut ApprovalData {
        *self = match std::mem::replace(self, Self::Selecting(ApprovalData::new(vec![]))) {
            Self::Selecting(d) | Self::ConfirmingDeny(d) => Self::Selecting(d),
        };
        match self {
            Self::Selecting(d) => d,
            Self::ConfirmingDeny(_) => unreachable!(),
        }
    }

    /// Check if in deny confirmation phase.
    pub(crate) fn is_confirming_deny(&self) -> bool {
        matches!(self, Self::ConfirmingDeny(_))
    }

    /// Transition: enter deny confirmation (Selecting -> ConfirmingDeny).
    pub(crate) fn enter_deny_confirmation(&mut self) {
        *self = match std::mem::replace(self, Self::Selecting(ApprovalData::new(vec![]))) {
            Self::Selecting(d) | Self::ConfirmingDeny(d) => Self::ConfirmingDeny(d),
        };
    }
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

/// Tool execution is disabled due to tool journal errors.
///
/// This is an *idle* state: streaming may continue, but any tool calls are
/// pre-resolved to errors rather than executed (fail closed for safety).
#[derive(Debug)]
pub(crate) struct ToolsDisabledState;

/// Crash recovery could not proceed due to journal errors.
///
/// This is a safety state: we refuse to start new streams until the user clears
/// or repairs the journals, preventing chronology corruption (IFA: invalid
/// recovery states are explicit and unforgeable for the core).
#[derive(Debug)]
pub(crate) struct RecoveryBlockedState {
    pub(crate) reason: RecoveryBlockedReason,
}

#[derive(Debug)]
pub(crate) enum RecoveryBlockedReason {
    StreamJournalRecoverFailed {
        error: String,
    },
    ToolBatchStepMismatch {
        batch_id: ToolBatchId,
        tool_batch_step_id: StepId,
        stream_step_id: StepId,
    },
}

impl RecoveryBlockedReason {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::StreamJournalRecoverFailed { error } => {
                format!("Stream journal recovery failed: {error}")
            }
            Self::ToolBatchStepMismatch {
                batch_id,
                tool_batch_step_id,
                stream_step_id,
            } => format!(
                "Tool batch {batch_id} is bound to step {tool_batch_step_id}, but stream journal recovered step {stream_step_id}"
            ),
        }
    }
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
    ToolsDisabled(ToolsDisabledState),
    Streaming(ActiveStream),
    ToolLoop(Box<ToolLoopState>),
    ToolRecovery(ToolRecoveryState),
    RecoveryBlocked(RecoveryBlockedState),
    Distilling(DistillationState),
}

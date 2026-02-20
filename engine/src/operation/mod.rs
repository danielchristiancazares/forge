//! Operation transition graph authority.
//!
//! This module is the single encoding point for named `OperationState` edges
//! and legality checks. App orchestration delegates transition graph decisions
//! here instead of embedding the graph in multiple call sites.

use crate::state::{OperationEdge, OperationTag};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBatchContinuation {
    ResumeStreaming,
    FinishTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransitionReceipt {
    from: OperationTag,
    edge: OperationEdge,
    to: OperationTag,
}

impl TransitionReceipt {
    #[must_use]
    pub(crate) const fn from(self) -> OperationTag {
        self.from
    }

    #[must_use]
    pub(crate) const fn edge(self) -> OperationEdge {
        self.edge
    }

    #[must_use]
    pub(crate) const fn to(self) -> OperationTag {
        self.to
    }
}

#[must_use]
pub(crate) fn transition_receipt(
    from: OperationTag,
    to: OperationTag,
) -> Option<TransitionReceipt> {
    transition_edge(from, to).map(|edge| TransitionReceipt { from, edge, to })
}

#[must_use]
pub(crate) fn receipt_is_legal(receipt: TransitionReceipt) -> bool {
    is_legal_transition(receipt.from, receipt.edge, receipt.to)
}

#[must_use]
pub(crate) fn transition_edge(from: OperationTag, to: OperationTag) -> Option<OperationEdge> {
    use OperationEdge::{
        EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch,
        ResolvePlanApproval, StartDistillation, StartStreaming,
    };
    use OperationTag::{Distilling, Idle, PlanApproval, Streaming, ToolLoop};

    match (from, to) {
        (Idle, Streaming) => Some(StartStreaming),
        (Idle, Distilling) => Some(StartDistillation),
        (Streaming, PlanApproval) => Some(EnterToolLoopAwaitingApproval),
        (Streaming, ToolLoop) => Some(EnterToolLoopExecuting),
        (PlanApproval, ToolLoop) => Some(ResolvePlanApproval),
        (ToolLoop | PlanApproval | Idle, Idle) => Some(FinishToolBatch),
        _ => None,
    }
}

#[must_use]
pub(crate) fn is_legal_transition(
    from: OperationTag,
    edge: OperationEdge,
    to: OperationTag,
) -> bool {
    use OperationEdge::{
        EnterToolLoopAwaitingApproval, EnterToolLoopExecuting, FinishToolBatch, FinishTurn,
        ResolvePlanApproval, StartDistillation, StartStreaming,
    };
    use OperationTag::{Distilling, Idle, PlanApproval, Streaming, ToolLoop};

    match edge {
        StartStreaming => from == Idle && to == Streaming,
        StartDistillation => from == Idle && to == Distilling,
        EnterToolLoopAwaitingApproval => from == Streaming && matches!(to, PlanApproval | ToolLoop),
        EnterToolLoopExecuting => matches!(to, ToolLoop) && matches!(from, Streaming),
        ResolvePlanApproval => from == PlanApproval && matches!(to, ToolLoop),
        FinishToolBatch => to == Idle && matches!(from, ToolLoop | PlanApproval | Idle),
        FinishTurn => from == to && from == Idle,
    }
}

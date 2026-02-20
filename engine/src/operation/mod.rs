//! Operation transition graph authority.
//!
//! This module is the single encoding point for named `OperationState` edges
//! and legality checks. App orchestration delegates transition graph decisions
//! here instead of embedding the graph in multiple call sites.

use crate::state::{OperationEdge, OperationTag};

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

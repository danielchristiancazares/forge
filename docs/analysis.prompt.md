# Prompt Seed: OperationState and FocusState Gap Analysis

## Objective

- Bring `engine` operation and focus state handling into full IFA conformance.
- Centralize transitions and make edge legality explicit.

## Non-Negotiable Facts

- `OperationState` has 8 variants.
- `FocusState` has 3 variants.
- There are 35 direct operation-state write sites in `engine/src` (excluding test-only assignments).
- `FocusState` has ad-hoc writes (`Executing` and `Reviewing`) with no guaranteed operation-edge synchronization.
- `tools_disabled_state` is a shadow owner of state that overlaps `OperationState::ToolsDisabled`.

## Inline Canonical Excerpts

```rust
// engine/src/state.rs
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
```

```rust
// engine/src/state.rs
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum OperationState {
    Idle,
    ToolsDisabled(ToolsDisabledState),
    Streaming(ActiveStream),
    ToolLoop(Box<ToolLoopState>),
    PlanApproval(Box<PlanApprovalState>),
    ToolRecovery(ToolRecoveryState),
    RecoveryBlocked(RecoveryBlockedState),
    Distilling(DistillationState),
}
```

```rust
// engine/src/ui/view_state.rs
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FocusState {
    #[default]
    Idle,
    Executing {
        step_started_at: Option<Instant>,
    },
    Reviewing {
        active_index: usize,
        auto_advance: bool,
    },
}
```

```rust
// engine/src/lib.rs
fn busy_reason(&self) -> Option<&'static str> {
    match &self.state {
        OperationState::Idle | OperationState::ToolsDisabled(_) => None,
        OperationState::Streaming(_) => Some("streaming a response"),
        OperationState::ToolLoop(_) => Some("tool execution in progress"),
        OperationState::PlanApproval(_) => Some("plan approval pending"),
        OperationState::ToolRecovery(_) => Some("tool recovery pending"),
        OperationState::RecoveryBlocked(_) => Some("recovery blocked"),
        OperationState::Distilling(_) => Some("distillation in progress"),
    }
}

fn idle_state(&self) -> OperationState {
    if let Some(disabled) = self.tools_disabled_state.clone() {
        OperationState::ToolsDisabled(disabled)
    } else {
        OperationState::Idle
    }
}

fn replace_with_idle(&mut self) -> OperationState {
    let idle = self.idle_state();
    std::mem::replace(&mut self.state, idle)
}
```

```rust
// engine/src/streaming.rs and engine/src/tool_loop.rs
if self.view.view_mode == crate::ui::ViewMode::Focus {
    self.view.focus_state = crate::ui::FocusState::Executing {
        step_started_at: Some(std::time::Instant::now()),
    };
}

if self.view.view_mode == crate::ui::ViewMode::Focus
    && matches!(self.view.focus_state, crate::ui::FocusState::Executing { .. })
{
    self.view.focus_state = crate::ui::FocusState::Reviewing {
        active_index: 0,
        auto_advance: true,
    };
}
```

```rust
// engine/src/state.rs
pub(crate) fn transition_to_journaled(self, batch_id: ToolBatchId) -> Self {
    match self {
        ActiveStream::Transient { message, journal, abort_handle, tool_call_seq, tool_args_journal_bytes, turn } => {
            ActiveStream::Journaled {
                tool_batch_id: batch_id,
                message,
                journal,
                abort_handle,
                tool_call_seq,
                tool_args_journal_bytes,
                tool_args_buffer: ToolArgsJournalBuffer::new(),
                turn,
            }
        }
        ActiveStream::Journaled { .. } => {
            unreachable!("transition_to_journaled called on already journaled stream")
        }
    }
}
```

## Gap Register (All 12)

| Gap | IFA refs | One-line problem | Risk | Primary remediation |
|-----|----------|------------------|------|---------------------|
| G1 | 6.1, 7.6 | `tools_disabled_state` duplicates machine ownership. | High | Remove shadow field; keep only `OperationState::ToolsDisabled`. |
| G2 | 6.1, 7.6 | `FocusState` and `OperationState` evolve independently. | High | Prefer derived focus; fallback to transition-coupled focus writes. |
| G3 | 7.3, 6.1 | Transition authority is fragmented across files. | High | Single transition API with controlled construction boundary. |
| G4 | 7.4, 9.4, 13.6 | Runtime guards enforce policy after the fact. | Medium | Move policy into transition boundary and state representation. |
| G5 | 7.2, 10.1 | No canonical transition ledger for cross-cutting effects. | Medium | Emit transition event once in centralized transition path. |
| G6 | 11.2, 13 | `FocusState::Executing` has `Option<Instant>`. | Medium | Remove `Option` or split into explicit variants. |
| G7 | 3.3 | `transition_to_journaled` no-op path was remediated to fail-fast. | Low | Keep regression coverage; typestate remains future improvement. |
| G8 | 11.2 | `ToolLoopInput.tool_batch_id` is optional in core payload. | Medium | Encode absence as variant or require non-optional ID. |
| G9 | 11.2, 7.5 | `thinking_message: Option<Message>` triplicated in core payloads. | Medium | Introduce one shared abstraction for with/without thinking. |
| G10 | 8.1, 8.2 | `idle_state()` silently chooses between distinct outcomes. | High | Remove helper and require typed/discriminated transition result. |
| G11 | 13.6, 7.4 | `busy_reason` is a runtime eligibility guard. | Medium | Move command eligibility into state-shaped API surface. |
| G12 | 17 | No explicit Section 17 process artifacts. | Medium | Produce invariant registry, authority map, DRY proof map. |

## Minimal Anchor Set

- `engine/src/state.rs:596` (`OperationState`)
- `engine/src/ui/view_state.rs:51` (`FocusState`)
- `engine/src/lib.rs:1634` (`busy_reason`)
- `engine/src/lib.rs:1679` (`idle_state`)
- `engine/src/lib.rs:1687` (`replace_with_idle`)
- `engine/src/streaming.rs:246` (writes `FocusState::Executing`)
- `engine/src/tool_loop.rs:1862` (writes `FocusState::Reviewing`)

## High-Value Quantitative Context

- Operation-state writes by file:
  - `engine/src/tool_loop.rs`: 15
  - `engine/src/streaming.rs`: 5
  - `engine/src/commands.rs`: 4
  - `engine/src/distillation.rs`: 4
  - `engine/src/persistence.rs`: 4
  - `engine/src/plan.rs`: 3
- Shadow disabled-state writes: 8 total across `commands.rs`, `persistence.rs`, `tool_loop.rs`.

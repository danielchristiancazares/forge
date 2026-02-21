//! UI state types for the TUI layer.
//!
//! Pure data types with no IO, no async, no ratatui dependency.
//! Used by both the engine (state ownership) and tui (rendering/input).

mod animation;
mod history;
mod input;
mod modal;
mod panel;
mod scroll;
mod view_state;

pub use animation::AnimPhase;
pub use history::{InputHistory, NavOutcome};
pub use input::{
    CommandDraftMut, CommandDraftRef, CommandStateOwned, DraftInput, FileSelectMut, FileSelectRef,
    InputMode, InputState, InsertDraftMut, ModelSelectMut, ModelSelectRef, SettingsCategory,
    SettingsFilterMode, SettingsModalMut, SettingsModalRef, SettingsModalState, SettingsSurface,
};
pub use modal::{ModalEffect, ModalEffectKind};
pub use panel::{PanelEffect, PanelEffectKind};
pub use scroll::ScrollState;
pub use view_state::{
    ActiveFilesPanel, ChangeKind, DiffExpansion, FilesPanelState, TranscriptRenderAction,
    UiOptions, ViewState,
};

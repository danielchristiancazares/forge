//! UI-facing types for the engine.
//!
//! Most UI types now live in `forge_types::ui`. Display types live in `forge_core`.
//! This module re-exports them and hosts file_picker (IO dependency).

mod file_picker;

pub use file_picker::{FileEntry, FilePickerState, find_match_positions};
pub use forge_core::{DisplayItem, DisplayLog, DisplayPop, DisplayTail};
pub use forge_types::PredefinedModel;

// Re-export all UI types from forge_types.
pub use forge_types::ui::{
    ActiveFilesPanel, ChangeKind, CommandDraftMut, CommandDraftRef, CommandStateOwned,
    DiffExpansion, DraftInput, FileSelectMut, FileSelectRef, FilesPanelState, InputHistory,
    InputMode, InputState, InsertDraftMut, ModalEffect, ModalEffectKind, ModelSelectMut,
    ModelSelectRef, PanelEffect, PanelEffectKind, ScrollState, SettingsCategory, SettingsModalMut,
    SettingsModalRef, SettingsModalState, SettingsSurface, UiOptions, ViewState,
};

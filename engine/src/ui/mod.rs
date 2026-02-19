//! UI-facing types for the engine.
//!
//! Used by the TUI for rendering/input, intentionally separate from orchestration.

mod animation;
mod display;
mod file_picker;
mod history;
mod input;
mod modal;
mod panel;
mod scroll;
mod view_state;

pub use display::DisplayItem;
pub(crate) use display::DisplayLog;
pub use file_picker::{FileEntry, FilePickerState, find_match_positions};
pub use forge_types::PredefinedModel;
pub use history::InputHistory;
pub use input::{
    CommandDraftRef, DraftInput, FileSelectMut, FileSelectRef, InputMode, InputState,
    ModelSelectRef, SettingsCategory, SettingsModalMut, SettingsModalRef, SettingsModalState,
    SettingsSurface,
};
pub use modal::{ModalEffect, ModalEffectKind};
pub use panel::{PanelEffect, PanelEffectKind};
pub use scroll::ScrollState;
pub use view_state::{ChangeKind, FilesPanelState, FocusState, UiOptions, ViewMode, ViewState};

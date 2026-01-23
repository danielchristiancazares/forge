//! UI-facing types for the engine.
//!
//! This module contains types used by the TUI layer for rendering and input handling.
//! These types are intentionally separate from orchestration concerns.

mod display;
mod input;
mod modal;
mod model_select;
mod scroll;
mod view_state;

pub use display::DisplayItem;
pub use input::{DraftInput, InputMode, InputState};
pub use modal::{ModalEffect, ModalEffectKind};
pub use model_select::PredefinedModel;
pub use scroll::ScrollState;
pub use view_state::{UiOptions, ViewState};

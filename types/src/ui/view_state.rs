//! View state for rendering.
//!
//! This struct groups all state related to rendering and UI display,
//! separating it from orchestration concerns.

use super::{ModalEffect, PanelEffect, ScrollState};
use std::path::PathBuf;
use std::time::Instant;

/// Classification of file changes for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Created,
}

/// Interactive state for the files panel.
#[derive(Debug, Clone, Default)]
pub enum FilesPanelState {
    #[default]
    Hidden,
    Visible(ActiveFilesPanel),
}

#[derive(Debug, Clone)]
pub struct ActiveFilesPanel {
    /// Index into the flattened file list (modified first, then created).
    pub selected: usize,
    /// Which file's diff is currently expanded (None = collapsed).
    pub expansion: DiffExpansion,
    pub diff_scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffExpansion {
    Collapsed,
    Expanded(PathBuf),
}

/// UI configuration options derived from config/environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiOptions {
    pub ascii_only: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// Whether to render provider thinking/reasoning deltas (if available).
    pub show_thinking: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TranscriptRenderAction {
    #[default]
    Preserve,
    Clear,
}

/// Separates view concerns from orchestration state, making it
/// clearer what state is used for rendering vs. what drives the
/// application logic.
#[derive(Debug)]
pub struct ViewState {
    pub scroll: ScrollState,
    /// Maximum scroll offset (content length - viewport).
    pub scroll_max: u16,
    /// Request to clear the visible transcript (handled by the UI).
    pub transcript_action: TranscriptRenderAction,
    pub modal_effect: Option<ModalEffect>,
    pub files_panel_effect: Option<PanelEffect>,
    /// UI options (theme, motion, glyphs).
    pub ui_options: UiOptions,
    /// Timestamp of last frame (for animation timing).
    pub last_frame: Instant,
    pub files_panel: FilesPanelState,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll: ScrollState::default(),
            scroll_max: 0,
            transcript_action: TranscriptRenderAction::default(),
            modal_effect: None,
            files_panel_effect: None,
            ui_options: UiOptions::default(),
            last_frame: Instant::now(),
            files_panel: FilesPanelState::default(),
        }
    }
}

impl ViewState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

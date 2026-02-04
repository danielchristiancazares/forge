//! View state for rendering.
//!
//! This struct groups all state related to rendering and UI display,
//! separating it from orchestration concerns.

use std::path::PathBuf;
use std::time::Instant;

use super::{ModalEffect, PanelEffect, ScrollState};

/// Classification of file changes for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// File existed before and was modified.
    Modified,
    /// File was created during this session.
    Created,
}

/// Interactive state for the files panel.
#[derive(Debug, Clone, Default)]
pub struct FilesPanelState {
    /// Whether the panel is visible.
    pub visible: bool,
    /// Index into the flattened file list (modified first, then created).
    pub selected: usize,
    /// Which file's diff is currently expanded (None = collapsed).
    pub expanded: Option<PathBuf>,
    /// Scroll offset within the diff view.
    pub diff_scroll: usize,
}

/// UI configuration options derived from config/environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiOptions {
    pub ascii_only: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// Whether to render provider thinking/reasoning deltas (if available).
    pub show_thinking: bool,
}

/// State related to rendering and UI display.
///
/// This separates view concerns from orchestration state, making it
/// clearer what state is used for rendering vs. what drives the
/// application logic.
#[derive(Debug)]
pub struct ViewState {
    /// Scroll position for the message view.
    pub scroll: ScrollState,
    /// Maximum scroll offset (content length - viewport).
    pub scroll_max: u16,
    /// Request to toggle between fullscreen and inline UI modes.
    pub toggle_screen_mode: bool,
    /// Request to clear the visible transcript (handled by the UI).
    pub clear_transcript: bool,
    /// Active modal animation effect.
    pub modal_effect: Option<ModalEffect>,
    /// Active files panel animation effect.
    pub files_panel_effect: Option<PanelEffect>,
    /// UI options (theme, motion, glyphs).
    pub ui_options: UiOptions,
    /// Timestamp of last frame (for animation timing).
    pub last_frame: Instant,
    /// Interactive state for the files panel.
    pub files_panel: FilesPanelState,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll: ScrollState::default(),
            scroll_max: 0,
            toggle_screen_mode: false,
            clear_transcript: false,
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

//! View state for rendering.
//!
//! This struct groups all state related to rendering and UI display,
//! separating it from orchestration concerns.

use std::time::Instant;

use super::{ModalEffect, ScrollState};

/// UI configuration options derived from config/environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiOptions {
    pub ascii_only: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
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
    /// UI options (theme, motion, glyphs).
    pub ui_options: UiOptions,
    /// Timestamp of last frame (for animation timing).
    pub last_frame: Instant,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll: ScrollState::default(),
            scroll_max: 0,
            toggle_screen_mode: false,
            clear_transcript: false,
            modal_effect: None,
            ui_options: UiOptions::default(),
            last_frame: Instant::now(),
        }
    }
}

impl ViewState {
    /// Create a new `ViewState` with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

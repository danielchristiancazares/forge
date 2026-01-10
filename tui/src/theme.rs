//! Color theme using Kanagawa Wave palette.
//!
//! Kanagawa is inspired by the famous painting "The Great Wave off Kanagawa"
//! by Katsushika Hokusai. See docs/COLOR_SCHEME.md for full palette reference.

use ratatui::style::{Color, Modifier, Style};

/// Kanagawa Wave color palette
pub mod colors {
    use super::Color;

    // === Backgrounds (Sumi Ink) ===
    pub const BG_DARK: Color = Color::Rgb(22, 22, 29); // sumiInk0
    pub const BG_PANEL: Color = Color::Rgb(31, 31, 40); // sumiInk3
    pub const BG_HIGHLIGHT: Color = Color::Rgb(42, 42, 55); // sumiInk4
    pub const BG_POPUP: Color = Color::Rgb(54, 54, 70); // sumiInk5
    pub const BG_BORDER: Color = Color::Rgb(84, 84, 109); // sumiInk6

    // === Foregrounds (Fuji) ===
    pub const TEXT_PRIMARY: Color = Color::Rgb(220, 215, 186); // fujiWhite
    pub const TEXT_SECONDARY: Color = Color::Rgb(200, 192, 147); // oldWhite
    pub const TEXT_MUTED: Color = Color::Rgb(114, 113, 105); // fujiGray
    pub const TEXT_DISABLED: Color = Color::Rgb(113, 124, 124); // katanaGray

    // === Primary/Brand ===
    pub const PRIMARY: Color = Color::Rgb(149, 127, 184); // oniViolet
    pub const PRIMARY_DIM: Color = Color::Rgb(147, 138, 169); // springViolet1

    // === Accent Colors ===
    pub const BLUE: Color = Color::Rgb(126, 156, 216); // crystalBlue
    pub const CYAN: Color = Color::Rgb(127, 180, 202); // springBlue
    pub const GREEN: Color = Color::Rgb(152, 187, 108); // springGreen
    pub const YELLOW: Color = Color::Rgb(230, 195, 132); // carpYellow
    pub const ORANGE: Color = Color::Rgb(255, 160, 102); // surimiOrange
    pub const PINK: Color = Color::Rgb(210, 126, 153); // sakuraPink
    pub const RED: Color = Color::Rgb(255, 93, 98); // peachRed
    pub const SOFT_RED: Color = Color::Rgb(228, 104, 118); // waveRed

    // === Semantic Aliases ===
    pub const ACCENT: Color = CYAN;
    pub const SUCCESS: Color = GREEN;
    pub const WARNING: Color = YELLOW;
    pub const ERROR: Color = RED;
    pub const PEACH: Color = ORANGE;

    // === Search ===
    pub const SEARCH_MATCH_BG: Color = Color::Rgb(34, 50, 73); // waveBlue1
    pub const SEARCH_ACTIVE_BG: Color = Color::Rgb(45, 79, 103); // waveBlue2

    // === Diff ===
    pub const DIFF_ADD_BG: Color = Color::Rgb(43, 51, 40); // winterGreen
    pub const DIFF_ADD_FG: Color = Color::Rgb(118, 148, 106); // autumnGreen
    pub const DIFF_DEL_BG: Color = Color::Rgb(67, 36, 43); // winterRed
    pub const DIFF_DEL_FG: Color = Color::Rgb(195, 64, 67); // autumnRed
    pub const DIFF_CHG_BG: Color = Color::Rgb(73, 68, 60); // winterYellow
    pub const DIFF_CHG_FG: Color = Color::Rgb(220, 165, 97); // autumnYellow

    // === Diagnostic ===
    pub const CRITICAL: Color = Color::Rgb(232, 36, 36); // samuraiRed
    pub const FLASH: Color = Color::Rgb(255, 158, 59); // roninYellow
}

/// Pre-defined styles for common UI elements
pub mod styles {
    use super::*;

    pub fn user_name() -> Style {
        Style::default()
            .fg(colors::GREEN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn assistant_name() -> Style {
        Style::default()
            .fg(colors::PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_normal() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::TEXT_SECONDARY)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_insert() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::GREEN)
            .add_modifier(Modifier::BOLD)
    }

    pub fn mode_command() -> Style {
        Style::default()
            .fg(colors::BG_DARK)
            .bg(colors::YELLOW)
            .add_modifier(Modifier::BOLD)
    }

    pub fn key_hint() -> Style {
        Style::default().fg(colors::TEXT_MUTED)
    }

    pub fn key_highlight() -> Style {
        Style::default()
            .fg(colors::PEACH)
            .add_modifier(Modifier::BOLD)
    }
}

/// Spinner frames for loading animation
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Get spinner frame based on tick count
pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

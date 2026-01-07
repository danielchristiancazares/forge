//! Color theme and styling constants for the TUI.

use ratatui::style::{Color, Modifier, Style};

/// Claude-inspired color palette
pub mod colors {
    use super::Color;

    // Primary brand colors
    pub const PRIMARY: Color = Color::Rgb(139, 92, 246); // Purple (violet-500)
    pub const PRIMARY_DIM: Color = Color::Rgb(109, 72, 206); // Darker purple

    // Background colors
    pub const BG_DARK: Color = Color::Rgb(17, 17, 27); // Near black
    pub const BG_PANEL: Color = Color::Rgb(30, 30, 46); // Panel background
    pub const BG_HIGHLIGHT: Color = Color::Rgb(44, 46, 68); // Row highlight

    // Text colors
    pub const TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244); // Main text
    pub const TEXT_SECONDARY: Color = Color::Rgb(147, 153, 178); // Dimmed text
    pub const TEXT_MUTED: Color = Color::Rgb(88, 91, 112); // Very dim

    // Accent colors
    pub const GREEN: Color = Color::Rgb(166, 227, 161); // Success/user
    pub const YELLOW: Color = Color::Rgb(249, 226, 175); // Warning
    pub const RED: Color = Color::Rgb(243, 139, 168); // Error
    pub const PEACH: Color = Color::Rgb(250, 179, 135); // Accent
    pub const CYAN: Color = Color::Rgb(137, 220, 235); // Tools/links

    // Semantic aliases for tool rendering
    pub const ACCENT: Color = CYAN;
    pub const SUCCESS: Color = GREEN;
    pub const ERROR: Color = RED;
    pub const WARNING: Color = YELLOW;
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

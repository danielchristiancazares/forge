//! Color theme and glyphs for Forge TUI.
//!
//! Uses Kanagawa Wave palette by default with an optional high-contrast override.

use ratatui::style::{Color, Modifier, Style};

use forge_types::ui::UiOptions;

/// Kanagawa Wave color palette constants.
mod colors {
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
    pub const RED: Color = Color::Rgb(255, 93, 98); // peachRed

    // === Semantic Aliases ===
    pub const ACCENT: Color = CYAN;
    pub const SUCCESS: Color = GREEN;
    pub const WARNING: Color = YELLOW;
    pub const ERROR: Color = RED;
    pub const PEACH: Color = ORANGE;

    // === Provider Colors ===
    pub const PROVIDER_CLAUDE: Color = Color::Rgb(204, 85, 0); // burnt orange
    pub const PROVIDER_OPENAI: Color = Color::Rgb(255, 255, 255); // white
    pub const PROVIDER_GEMINI: Color = Color::Rgb(66, 133, 244); // Google blue
}

/// Resolved theme palette used by the UI.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_dark: Color,
    pub bg_panel: Color,
    pub bg_highlight: Color,
    pub bg_popup: Color,
    pub bg_border: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub primary: Color,
    pub primary_dim: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub peach: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub provider_claude: Color,
    pub provider_openai: Color,
    pub provider_gemini: Color,
}

impl Palette {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            bg_dark: colors::BG_DARK,
            bg_panel: colors::BG_PANEL,
            bg_highlight: colors::BG_HIGHLIGHT,
            bg_popup: colors::BG_POPUP,
            bg_border: colors::BG_BORDER,
            text_primary: colors::TEXT_PRIMARY,
            text_secondary: colors::TEXT_SECONDARY,
            text_muted: colors::TEXT_MUTED,
            text_disabled: colors::TEXT_DISABLED,
            primary: colors::PRIMARY,
            primary_dim: colors::PRIMARY_DIM,
            accent: colors::ACCENT,
            blue: colors::BLUE,
            success: colors::SUCCESS,
            warning: colors::WARNING,
            error: colors::ERROR,
            peach: colors::PEACH,
            green: colors::GREEN,
            yellow: colors::YELLOW,
            red: colors::RED,
            provider_claude: colors::PROVIDER_CLAUDE,
            provider_openai: colors::PROVIDER_OPENAI,
            provider_gemini: colors::PROVIDER_GEMINI,
        }
    }

    #[must_use]
    pub fn high_contrast() -> Self {
        Self {
            bg_dark: Color::Black,
            bg_panel: Color::Black,
            bg_highlight: Color::DarkGray,
            bg_popup: Color::Black,
            bg_border: Color::Gray,
            text_primary: Color::White,
            text_secondary: Color::Gray,
            text_muted: Color::DarkGray,
            text_disabled: Color::DarkGray,
            primary: Color::White,
            primary_dim: Color::Gray,
            accent: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            peach: Color::Yellow,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::Red,
            blue: Color::Blue,
            provider_claude: Color::Yellow,
            provider_openai: Color::White,
            provider_gemini: Color::Cyan,
        }
    }
}

#[must_use]
pub fn palette(options: UiOptions) -> Palette {
    if options.high_contrast {
        Palette::high_contrast()
    } else {
        Palette::standard()
    }
}

/// ASCII/Unicode glyphs for icons and spinners.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub system: &'static str,
    pub user: &'static str,
    pub assistant: &'static str,
    pub thinking: &'static str,
    pub tool: &'static str,
    pub tool_result_ok: &'static str,
    pub tool_result_err: &'static str,
    pub tree_connector: &'static str,
    pub status_ready: &'static str,
    pub status_missing: &'static str,
    pub pending: &'static str,
    pub denied: &'static str,
    pub paused: &'static str,
    pub running: &'static str,
    pub bullet: &'static str,
    pub arrow_up: &'static str,
    pub arrow_down: &'static str,
    pub track: &'static str,
    pub thumb: &'static str,
    pub selected: &'static str,
    pub spinner_frames: &'static [&'static str],
    pub add: &'static str,
    pub modified: &'static str,
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];

#[must_use]
pub fn glyphs(options: UiOptions) -> Glyphs {
    if options.ascii_only {
        Glyphs {
            system: "S",
            user: "U",
            assistant: "A",
            thinking: "?",
            tool: "T",
            tool_result_ok: "OK",
            tool_result_err: "ERR",
            tree_connector: "L",
            status_ready: "*",
            status_missing: "o",
            pending: "*",
            denied: "X",
            paused: "||",
            running: ">",
            bullet: "*",
            arrow_up: "^",
            arrow_down: "v",
            track: "|",
            thumb: "#",
            selected: ">",
            spinner_frames: SPINNER_FRAMES_ASCII,
            add: "+",
            modified: "~",
        }
    } else {
        Glyphs {
            system: "●",
            user: "○",
            assistant: "◇",
            thinking: "◦",
            tool: "⊙",
            tool_result_ok: "✓",
            tool_result_err: "✗",
            tree_connector: "↪",
            status_ready: "●",
            status_missing: "○",
            pending: "•",
            denied: "⊘",
            paused: "⏸",
            running: "▶",
            bullet: "•",
            arrow_up: "↑",
            arrow_down: "↓",
            track: "│",
            thumb: "█",
            selected: "▸",
            spinner_frames: SPINNER_FRAMES,
            add: "+",
            modified: "~",
        }
    }
}

/// When `reduced_motion` is enabled, returns a static glyph instead of cycling.
#[must_use]
pub fn spinner_frame(tick: usize, options: UiOptions) -> &'static str {
    let frames = glyphs(options).spinner_frames;
    if options.reduced_motion {
        frames[0]
    } else {
        frames[tick % frames.len()]
    }
}

/// Pre-defined styles for common UI elements.
pub mod styles {
    use super::{Modifier, Palette, Style};

    #[must_use]
    pub fn user_name(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.green)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn assistant_name(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.primary)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn mode_normal(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.bg_dark)
            .bg(palette.text_secondary)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn mode_insert(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.bg_dark)
            .bg(palette.green)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn mode_command(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.bg_dark)
            .bg(palette.yellow)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn mode_model(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.bg_dark)
            .bg(palette.primary)
            .add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn key_hint(palette: &Palette) -> Style {
        Style::default().fg(palette.text_muted)
    }

    #[must_use]
    pub fn key_highlight(palette: &Palette) -> Style {
        Style::default()
            .fg(palette.peach)
            .add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use forge_types::ui::UiOptions;

    use super::spinner_frame;

    #[test]
    fn spinner_frame_cycles_without_reduced_motion() {
        let options = UiOptions {
            ascii_only: false,
            high_contrast: false,
            reduced_motion: false,
            show_thinking: false,
        };
        let frame0 = spinner_frame(0, options);
        let frame1 = spinner_frame(1, options);
        assert_ne!(frame0, frame1, "spinner should cycle through frames");
    }

    #[test]
    fn spinner_frame_static_with_reduced_motion() {
        let options = UiOptions {
            ascii_only: false,
            high_contrast: false,
            reduced_motion: true,
            show_thinking: false,
        };
        let frame0 = spinner_frame(0, options);
        let frame1 = spinner_frame(1, options);
        let frame100 = spinner_frame(100, options);
        assert_eq!(
            frame0, frame1,
            "spinner should be static with reduced_motion"
        );
        assert_eq!(frame0, frame100, "spinner should remain static at any tick");
    }

    #[test]
    fn spinner_frame_static_with_reduced_motion_ascii() {
        let options = UiOptions {
            ascii_only: true,
            high_contrast: false,
            reduced_motion: true,
            show_thinking: false,
        };
        let frame0 = spinner_frame(0, options);
        let frame1 = spinner_frame(1, options);
        assert_eq!(
            frame0, frame1,
            "ascii spinner should be static with reduced_motion"
        );
        assert_eq!(frame0, "|", "ascii spinner static frame should be '|'");
    }
}

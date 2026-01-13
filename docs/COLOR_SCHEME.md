# Forge Color Scheme

## Kanagawa Wave Palette Reference

**Version:** 1.0
**Date:** 2026-01-09

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-21 | Header & TOC |
| 22-27 | 1. Overview |
| 28-104 | 2. Kanagawa Wave Palette |
| 105-192 | 3. Semantic Role Mapping |
| 193-258 | 4. Rust Constants |
| 259-281 | 5. Accessibility Notes |
| 282-287 | 6. References |

---

## 1. Overview

Forge uses the [Kanagawa](https://github.com/rebelot/kanagawa.nvim) color scheme, specifically the **Wave** (default dark) variant. Kanagawa is inspired by the famous painting "The Great Wave off Kanagawa" by Katsushika Hokusai, featuring deep ink blues, warm earth tones, and carefully balanced contrast.

---

## 2. Kanagawa Wave Palette

### 2.1 Background Colors (Sumi Ink)

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `sumiInk0` | `#16161D` | `(22, 22, 29)` | Darkest background, terminal bg |
| `sumiInk1` | `#181820` | `(24, 24, 32)` | Dark background variant |
| `sumiInk2` | `#1a1a22` | `(26, 26, 34)` | Slightly lighter |
| `sumiInk3` | `#1F1F28` | `(31, 31, 40)` | Default editor background |
| `sumiInk4` | `#2A2A37` | `(42, 42, 55)` | Cursorline, visual selection |
| `sumiInk5` | `#363646` | `(54, 54, 70)` | Popup background, panels |
| `sumiInk6` | `#54546D` | `(84, 84, 109)` | Float border, inactive elements |

### 2.2 Foreground Colors (Fuji)

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `fujiWhite` | `#DCD7BA` | `(220, 215, 186)` | Default foreground text |
| `oldWhite` | `#C8C093` | `(200, 192, 147)` | Secondary text, whitespace |
| `fujiGray` | `#727169` | `(114, 113, 105)` | Comments, muted text |
| `katanaGray` | `#717C7C` | `(113, 124, 124)` | Inactive/disabled text |

### 2.3 Accent Colors

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `oniViolet` | `#957FB8` | `(149, 127, 184)` | Keywords, primary accent |
| `crystalBlue` | `#7E9CD8` | `(126, 156, 216)` | Functions, links |
| `springBlue` | `#7FB4CA` | `(127, 180, 202)` | Identifiers, cyan accent |
| `springGreen` | `#98BB6C` | `(152, 187, 108)` | Strings, success |
| `carpYellow` | `#E6C384` | `(230, 195, 132)` | Warnings, operators |
| `surimiOrange` | `#FFA066` | `(255, 160, 102)` | Constants, highlights |
| `sakuraPink` | `#D27E99` | `(210, 126, 153)` | Numbers, special |
| `peachRed` | `#FF5D62` | `(255, 93, 98)` | Errors, deletions |
| `waveRed` | `#E46876` | `(228, 104, 118)` | Soft error/diff delete |

### 2.4 Diagnostic/Semantic Colors

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `autumnGreen` | `#76946A` | `(118, 148, 106)` | Diff add, hints |
| `autumnYellow` | `#DCA561` | `(220, 165, 97)` | Diff change, warnings |
| `autumnRed` | `#C34043` | `(195, 64, 67)` | Diff delete, errors |
| `samuraiRed` | `#E82424` | `(232, 36, 36)` | Critical errors |
| `roninYellow` | `#FF9E3B` | `(255, 158, 59)` | Strong warning, flash |

### 2.5 Wave/Water Colors

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `waveBlue1` | `#223249` | `(34, 50, 73)` | Popup selection bg |
| `waveBlue2` | `#2D4F67` | `(45, 79, 103)` | Active selection bg |
| `waveAqua1` | `#6A9589` | `(106, 149, 137)` | Types, aqua accent |
| `waveAqua2` | `#7AA89F` | `(122, 168, 159)` | Lighter aqua |
| `dragonBlue` | `#658594` | `(101, 133, 148)` | Muted blue |

### 2.6 Diff Background Highlights

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `winterGreen` | `#2B3328` | `(43, 51, 40)` | Diff add background |
| `winterYellow` | `#49443C` | `(73, 68, 60)` | Diff change background |
| `winterRed` | `#43242B` | `(67, 36, 43)` | Diff delete background |
| `winterBlue` | `#252535` | `(37, 37, 53)` | Fold background |

### 2.7 Spring/Violet Spectrum

| Name | Hex | RGB | Usage |
| --- | --- | --- | --- |
| `springViolet1` | `#938AA9` | `(147, 138, 169)` | Light statements |
| `springViolet2` | `#9CABCA` | `(156, 171, 202)` | Lavender/parameters |
| `boatYellow1` | `#938056` | `(147, 128, 86)` | Dark yellow |
| `boatYellow2` | `#C0A36E` | `(192, 163, 110)` | Warm yellow |

---

## 3. Semantic Role Mapping

Maps Kanagawa colors to Forge TUI semantic roles.

### 3.1 Core UI

| Role | Kanagawa Color | Hex |
| --- | --- | --- |
| `BG_DARK` | `sumiInk0` | `#16161D` |
| `BG_PANEL` | `sumiInk3` | `#1F1F28` |
| `BG_HIGHLIGHT` | `sumiInk4` | `#2A2A37` |
| `TEXT_PRIMARY` | `fujiWhite` | `#DCD7BA` |
| `TEXT_SECONDARY` | `oldWhite` | `#C8C093` |
| `TEXT_MUTED` | `fujiGray` | `#727169` |

### 3.2 Brand/Accent

| Role | Kanagawa Color | Hex | Notes |
| --- | --- | --- | --- |
| `PRIMARY` | `oniViolet` | `#957FB8` | Assistant name, mode indicator |
| `PRIMARY_DIM` | `springViolet1` | `#938AA9` | Dimmed primary |
| `ACCENT` | `springBlue` | `#7FB4CA` | Tools, links, cyan accent |

### 3.3 Status/Semantic

| Role | Kanagawa Color | Hex |
| --- | --- | --- |
| `SUCCESS` / `GREEN` | `springGreen` | `#98BB6C` |
| `WARNING` / `YELLOW` | `carpYellow` | `#E6C384` |
| `ERROR` / `RED` | `peachRed` | `#FF5D62` |
| `PEACH` | `surimiOrange` | `#FFA066` |
| `CYAN` | `springBlue` | `#7FB4CA` |

### 3.4 Input Mode Indicators

| Mode | Background | Foreground |
| --- | --- | --- |
| Normal | `fujiGray` | `sumiInk0` |
| Insert | `springGreen` | `sumiInk0` |
| Command | `carpYellow` | `sumiInk0` |
| Search | `crystalBlue` | `sumiInk0` |
| ModelSelect | `oniViolet` | `sumiInk0` |

### 3.5 Search Highlighting

| Role | Kanagawa Color | Hex | Notes |
| --- | --- | --- | --- |
| `SEARCH_MATCH_BG` | `waveBlue1` | `#223249` | All matches |
| `SEARCH_ACTIVE_BG` | `waveBlue2` | `#2D4F67` | Current/active match |
| `SEARCH_MATCH_FG` | `fujiWhite` | `#DCD7BA` | Text on match |

### 3.6 Diff/Changes

| Role | Kanagawa Color | Hex |
| --- | --- | --- |
| `DIFF_ADD_BG` | `winterGreen` | `#2B3328` |
| `DIFF_ADD_FG` | `autumnGreen` | `#76946A` |
| `DIFF_DELETE_BG` | `winterRed` | `#43242B` |
| `DIFF_DELETE_FG` | `autumnRed` | `#C34043` |
| `DIFF_CHANGE_BG` | `winterYellow` | `#49443C` |
| `DIFF_CHANGE_FG` | `autumnYellow` | `#DCA561` |

### 3.7 Tool UI

| Role | Kanagawa Color | Hex | Notes |
| --- | --- | --- | --- |
| Tool name | `springBlue` | `#7FB4CA` | Tool call headers |
| Tool args | `fujiGray` | `#727169` | JSON arguments |
| Tool result OK | `springGreen` | `#98BB6C` | Success icon/border |
| Tool result error | `peachRed` | `#FF5D62` | Error icon/border |
| Tool pending | `carpYellow` | `#E6C384` | Awaiting spinner |

### 3.8 Markdown Rendering

| Element | Kanagawa Color | Hex |
| --- | --- | --- |
| Heading | `crystalBlue` | `#7E9CD8` |
| Bold | `fujiWhite` + bold | `#DCD7BA` |
| Italic | `springViolet2` | `#9CABCA` |
| Code inline | `surimiOrange` | `#FFA066` |
| Code block bg | `sumiInk4` | `#2A2A37` |
| Link | `springBlue` | `#7FB4CA` |
| List bullet | `oniViolet` | `#957FB8` |
| Blockquote | `fujiGray` | `#727169` |
| Blockquote bar | `sumiInk6` | `#54546D` |

---

## 4. Rust Constants

Reference implementation for `tui/src/theme.rs`:

```rust
//! Color theme using Kanagawa Wave palette.

use ratatui::style::{Color, Modifier, Style};

pub mod colors {
    use super::Color;

    // === Backgrounds (Sumi Ink) ===
    pub const BG_DARK: Color = Color::Rgb(22, 22, 29);       // sumiInk0
    pub const BG_PANEL: Color = Color::Rgb(31, 31, 40);      // sumiInk3
    pub const BG_HIGHLIGHT: Color = Color::Rgb(42, 42, 55);  // sumiInk4
    pub const BG_POPUP: Color = Color::Rgb(54, 54, 70);      // sumiInk5
    pub const BG_BORDER: Color = Color::Rgb(84, 84, 109);    // sumiInk6

    // === Foregrounds (Fuji) ===
    pub const TEXT_PRIMARY: Color = Color::Rgb(220, 215, 186);   // fujiWhite
    pub const TEXT_SECONDARY: Color = Color::Rgb(200, 192, 147); // oldWhite
    pub const TEXT_MUTED: Color = Color::Rgb(114, 113, 105);     // fujiGray
    pub const TEXT_DISABLED: Color = Color::Rgb(113, 124, 124);  // katanaGray

    // === Primary/Brand ===
    pub const PRIMARY: Color = Color::Rgb(149, 127, 184);     // oniViolet
    pub const PRIMARY_DIM: Color = Color::Rgb(147, 138, 169); // springViolet1

    // === Accent Colors ===
    pub const BLUE: Color = Color::Rgb(126, 156, 216);    // crystalBlue
    pub const CYAN: Color = Color::Rgb(127, 180, 202);    // springBlue
    pub const GREEN: Color = Color::Rgb(152, 187, 108);   // springGreen
    pub const YELLOW: Color = Color::Rgb(230, 195, 132);  // carpYellow
    pub const ORANGE: Color = Color::Rgb(255, 160, 102);  // surimiOrange
    pub const PINK: Color = Color::Rgb(210, 126, 153);    // sakuraPink
    pub const RED: Color = Color::Rgb(255, 93, 98);       // peachRed
    pub const SOFT_RED: Color = Color::Rgb(228, 104, 118); // waveRed

    // === Semantic Aliases ===
    pub const ACCENT: Color = CYAN;
    pub const SUCCESS: Color = GREEN;
    pub const WARNING: Color = YELLOW;
    pub const ERROR: Color = RED;
    pub const PEACH: Color = ORANGE;

    // === Search ===
    pub const SEARCH_MATCH_BG: Color = Color::Rgb(34, 50, 73);   // waveBlue1
    pub const SEARCH_ACTIVE_BG: Color = Color::Rgb(45, 79, 103); // waveBlue2

    // === Diff ===
    pub const DIFF_ADD_BG: Color = Color::Rgb(43, 51, 40);    // winterGreen
    pub const DIFF_ADD_FG: Color = Color::Rgb(118, 148, 106); // autumnGreen
    pub const DIFF_DEL_BG: Color = Color::Rgb(67, 36, 43);    // winterRed
    pub const DIFF_DEL_FG: Color = Color::Rgb(195, 64, 67);   // autumnRed
    pub const DIFF_CHG_BG: Color = Color::Rgb(73, 68, 60);    // winterYellow
    pub const DIFF_CHG_FG: Color = Color::Rgb(220, 165, 97);  // autumnYellow

    // === Diagnostic ===
    pub const CRITICAL: Color = Color::Rgb(232, 36, 36);   // samuraiRed
    pub const FLASH: Color = Color::Rgb(255, 158, 59);     // roninYellow
}
```

---

## 5. Accessibility Notes

### 5.1 Contrast Ratios

Kanagawa Wave was designed with readability in mind. Key contrast ratios (approximate):

| Foreground | Background | Ratio | WCAG |
| --- | --- | --- | --- |
| fujiWhite | sumiInk0 | ~12:1 | AAA |
| fujiWhite | sumiInk3 | ~10:1 | AAA |
| oldWhite | sumiInk0 | ~9:1 | AAA |
| fujiGray | sumiInk0 | ~4.5:1 | AA |
| springGreen | sumiInk0 | ~8:1 | AAA |
| peachRed | sumiInk0 | ~6:1 | AA |

### 5.2 Color Blindness Considerations

- Success (green) and error (red) are distinguishable by brightness, not just hue
- Consider using icons/shapes alongside color for critical status indicators
- The violet/blue distinction may be difficult for some; use context to clarify

---

## 6. References

- [Kanagawa.nvim GitHub](https://github.com/rebelot/kanagawa.nvim)
- [Kanagawa palette.lua](https://github.com/rebelot/kanagawa.nvim/blob/master/lua/kanagawa/colors.lua)
- [WCAG Contrast Guidelines](https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum.html)

//! Terminal user interface rendering for Forge.

mod approval;
mod diff_render;
mod effects;
mod format;
mod input;
pub mod markdown;
mod messages;
mod shared;
mod theme;
mod tool_display;
mod tool_result_summary;

pub use effects::apply_modal_effect;
pub use input::{InputPump, handle_events};
pub use theme::{Glyphs, Palette, glyphs, palette, spinner_frame, styles};

use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use forge_core::sanitize_display_text;
use forge_engine::{
    App, ContextUsageStatus, FileDiff, FileSelectAccess, ModelSelectAccess, PredefinedModel,
    Provider, SettingsAccess, command_specs, find_match_positions,
};
use forge_types::sanitize_path_for_display;
use forge_types::ui::{ChangeKind, InputMode, SettingsCategory, SettingsSurface};

use self::diff_render::render_tool_result_lines;
use self::format::{format_api_usage, format_token_count, highlight_file_refs};
pub use self::markdown::clear_render_cache;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let options = app.ui_options();
    let palette = palette(options);
    let glyphs = glyphs(options);
    let bg_block = Block::default().style(Style::default().bg(palette.bg_dark));
    frame.render_widget(bg_block, frame.area());

    let input_height = match app.input_mode() {
        InputMode::Normal | InputMode::ModelSelect | InputMode::Settings => 3,
        _ => 5,
    };

    let elapsed = app.frame_elapsed();

    // Panel width depends on expansion state: 35 chars collapsed, 50% expanded
    let panel_constraint = if app.files_panel_expanded() {
        Constraint::Percentage(50)
    } else {
        Constraint::Length(35)
    };
    let panel_layout = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints([Constraint::Min(40), panel_constraint])
        .split(frame.area());
    let base_panel_area = panel_layout[1];
    let full_main_area = frame.area().inner(Margin::new(1, 1));

    let mut files_panel_area = None;
    if let Some(effect) = app.files_panel_effect_mut() {
        effect.advance(elapsed);
        let animated = effects::apply_files_panel_effect(effect, base_panel_area);
        if animated.width > 0 && animated.height > 0 {
            files_panel_area = Some(animated);
        }
        if effect.is_finished() {
            app.finish_files_panel_effect();
        }
    } else if app.files_panel_visible() {
        files_panel_area = Some(base_panel_area);
    }

    let main_area = if let Some(panel_area) = files_panel_area {
        let width = panel_area.x.saturating_sub(full_main_area.x).max(1);
        Rect {
            x: full_main_area.x,
            y: full_main_area.y,
            width,
            height: full_main_area.height,
        }
    } else {
        full_main_area
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(input_height)])
        .split(main_area);

    messages::draw_messages(frame, app, chunks[0], &palette, &glyphs);
    draw_input(frame, app, chunks[1], &palette, &glyphs);

    if let Some(panel_area) = files_panel_area {
        draw_files_panel(frame, app, panel_area, &palette, &glyphs);
    }

    if app.input_mode() == InputMode::Command {
        draw_command_palette(frame, app, &palette);
    }

    if app.input_mode() == InputMode::ModelSelect {
        draw_model_selector(frame, app, &palette, &glyphs, elapsed);
    }

    if app.input_mode() == InputMode::FileSelect {
        draw_file_selector(frame, app, &palette, &glyphs, elapsed);
    }

    if app.input_mode() == InputMode::Settings {
        draw_settings_modal(frame, app, &palette, &glyphs, elapsed);
    }

    if !matches!(
        app.plan_approval_access(),
        forge_engine::PlanApprovalAccess::Inactive
    ) {
        approval::draw_plan_approval_prompt(frame, app, &palette);
    }

    if !matches!(
        app.tool_approval_access(),
        forge_engine::ToolApprovalAccess::Inactive
    ) {
        approval::draw_tool_approval_prompt(frame, app, &palette);
    }

    if !matches!(
        app.tool_recovery_access(),
        forge_engine::ToolRecoveryAccess::Inactive
    ) {
        approval::draw_tool_recovery_prompt(frame, app, &palette, &glyphs);
    }
}

fn draw_input(frame: &mut Frame, app: &mut App, area: Rect, palette: &Palette, glyphs: &Glyphs) {
    let mode = app.input_mode();
    let options = app.ui_options();
    // Clone command text to avoid borrow conflict with mutable context_usage_status()
    let (command_line, command_cursor_byte_index) = if mode == InputMode::Command {
        match app.command_input_access() {
            forge_engine::CommandInputAccess::Active {
                text,
                cursor_byte_index,
                ..
            } => (Some(text.to_string()), Some(cursor_byte_index)),
            forge_engine::CommandInputAccess::Inactive => (None, None),
        }
    } else {
        (None, None)
    };

    let multiline = mode == InputMode::Insert && app.draft_text().contains('\n');
    let prompt_char = if mode == InputMode::Insert {
        if options.ascii_only { ">" } else { "❯" }
    } else {
        ""
    };

    let (mode_label, mode_style) = match mode {
        InputMode::Normal | InputMode::ModelSelect | InputMode::FileSelect => {
            ("NORMAL", styles::mode_normal(palette))
        }
        InputMode::Settings => ("SETTINGS", styles::mode_command(palette)),
        InputMode::Insert => ("INSERT", styles::mode_insert(palette)),
        InputMode::Command => ("COMMAND", styles::mode_command(palette)),
    };
    let mode_text = if multiline {
        format!(" {mode_label} · MULTI ")
    } else {
        format!(" {mode_label} ")
    };

    // Key hints based on mode
    let hints = match mode {
        InputMode::Normal => vec![
            Span::styled("i", styles::key_highlight(palette)),
            Span::styled(" insert  ", styles::key_hint(palette)),
            Span::styled("/", styles::key_highlight(palette)),
            Span::styled(" command  ", styles::key_hint(palette)),
            Span::styled("f", styles::key_highlight(palette)),
            Span::styled(" files  ", styles::key_hint(palette)),
            Span::styled("PgUp/PgDn", styles::key_highlight(palette)),
            Span::styled(" scroll  ", styles::key_hint(palette)),
            Span::styled("q", styles::key_highlight(palette)),
            Span::styled(" quit ", styles::key_hint(palette)),
        ],
        InputMode::Insert => vec![
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" send  ", styles::key_hint(palette)),
            Span::styled("Ctrl+Enter/Shift+Enter", styles::key_highlight(palette)),
            Span::styled(" newline  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" normal ", styles::key_hint(palette)),
        ],
        InputMode::Command => vec![
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" execute  ", styles::key_hint(palette)),
            Span::styled("Tab", styles::key_highlight(palette)),
            Span::styled(" complete  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" cancel ", styles::key_hint(palette)),
        ],
        InputMode::ModelSelect | InputMode::FileSelect => vec![
            Span::styled("↑↓", styles::key_highlight(palette)),
            Span::styled(" select  ", styles::key_hint(palette)),
            Span::styled("1-9", styles::key_highlight(palette)),
            Span::styled(" quick pick  ", styles::key_hint(palette)),
            Span::styled("Enter", styles::key_highlight(palette)),
            Span::styled(" confirm  ", styles::key_hint(palette)),
            Span::styled("Esc", styles::key_highlight(palette)),
            Span::styled(" cancel ", styles::key_hint(palette)),
        ],
        InputMode::Settings => {
            let is_root_surface = matches!(
                app.settings_access(),
                SettingsAccess::Active {
                    surface: SettingsSurface::Root,
                    ..
                }
            );
            if is_root_surface {
                vec![
                    Span::styled("↑↓/jk", styles::key_highlight(palette)),
                    Span::styled(" navigate  ", styles::key_hint(palette)),
                    Span::styled("/", styles::key_highlight(palette)),
                    Span::styled(" filter  ", styles::key_hint(palette)),
                    Span::styled("Enter", styles::key_highlight(palette)),
                    Span::styled(" select  ", styles::key_hint(palette)),
                    Span::styled("Esc/q", styles::key_highlight(palette)),
                    Span::styled(" back/close ", styles::key_hint(palette)),
                ]
            } else {
                vec![
                    Span::styled("Esc/q", styles::key_highlight(palette)),
                    Span::styled(" close  ", styles::key_hint(palette)),
                    Span::styled("Enter", styles::key_highlight(palette)),
                    Span::styled(" close ", styles::key_hint(palette)),
                ]
            }
        }
    };

    let usage_status = app.context_usage_status();
    // 0 = ready, 1 = needs compaction, 2 = recent messages too large (unrecoverable)
    let (usage, severity_override) = match &usage_status {
        ContextUsageStatus::Ready(usage) => (usage, 0),
        ContextUsageStatus::NeedsCompaction { usage, .. } => (usage, 1),
        ContextUsageStatus::RecentMessagesTooLarge { usage, .. } => (usage, 2),
    };
    let pct = usage.percentage();
    let remaining = (100.0 - pct).clamp(0.0, 100.0);
    let base_usage = format!("Context {remaining:.0}% left");
    let context_str = match severity_override {
        2 => format!("{base_usage} !!"), // Double bang for unrecoverable
        1 => format!("{base_usage} !"),
        _ => base_usage,
    };
    let usage_color = match severity_override {
        1 | 2 => palette.red,
        _ => match usage.severity() {
            0 => palette.green,  // < 70%
            1 => palette.yellow, // 70-90%
            _ => palette.red,    // > 90%
        },
    };
    // Format API usage if available
    let api_usage_str = format_api_usage(app.last_turn_usage());
    let usage_str = if api_usage_str.is_empty() {
        context_str
    } else {
        format!("{context_str}  {api_usage_str}")
    };

    // Format LSP diagnostics indicator
    let lsp_snap = app.lsp_snapshot();
    let diag_str = lsp_snap.status_string();
    let diag_color = if lsp_snap.error_count() > 0 {
        Some(palette.red)
    } else if lsp_snap.warning_count() > 0 {
        Some(palette.yellow)
    } else {
        None
    };

    let padding_v: u16 = match mode {
        InputMode::Normal | InputMode::ModelSelect | InputMode::Settings => 0,
        InputMode::Insert if multiline => 0,
        _ => 1,
    };
    let input_padding = Padding::new(1, 1, padding_v, padding_v);
    let inner_height = area
        .height
        .saturating_sub(2 + padding_v.saturating_mul(2))
        .max(1);

    let prefix = match mode {
        InputMode::Command => " / ".to_string(),
        _ => format!(" {prompt_char} "),
    };
    let prefix_width = prefix.width() as u16;
    let content_width = area
        .width
        .saturating_sub(2)
        .saturating_sub(prefix_width)
        .max(1) as usize;

    let mut cursor_pos: Option<(u16, u16)> = None;
    let input_lines: Vec<Line> = if mode == InputMode::Insert && multiline {
        let draft = app.draft_text();
        let cursor_index = app.draft_cursor_byte_index();
        let before_cursor = &draft[..cursor_index];
        let cursor_line_index = before_cursor.matches('\n').count();
        let cursor_line_start = before_cursor.rsplit('\n').next().unwrap_or("");
        let cursor_display_pos = cursor_line_start.width();

        let raw_lines: Vec<&str> = draft.split('\n').collect();
        let visible_lines = inner_height as usize;
        let start_line = (cursor_line_index + 1).saturating_sub(visible_lines);
        let end_line = (start_line + visible_lines).min(raw_lines.len());

        let mut display_lines = Vec::new();
        let mut horizontal_scroll: u16 = 0;

        for (idx, line) in raw_lines[start_line..end_line].iter().enumerate() {
            let is_cursor_line = start_line + idx == cursor_line_index;
            let mut line_text = (*line).to_string();
            if is_cursor_line && cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
                let mut byte_offset = 0;
                let mut skipped_width = 0;
                for (i, grapheme) in line.grapheme_indices(true) {
                    if skipped_width >= scroll_target {
                        byte_offset = i;
                        break;
                    }
                    skipped_width += grapheme.width();
                }
                line_text = line[byte_offset..].to_string();
                horizontal_scroll = skipped_width as u16;
            }

            let prefix_text = if idx == 0 {
                prefix.clone()
            } else {
                " ".repeat(prefix_width as usize)
            };
            let prefix_style = if mode == InputMode::Command {
                Style::default().fg(palette.yellow)
            } else {
                Style::default().fg(palette.primary)
            };
            display_lines.push(Line::from(vec![
                Span::styled(prefix_text, prefix_style),
                Span::styled(line_text, Style::default().fg(palette.text_primary)),
            ]));
        }

        let cursor_row = cursor_line_index.saturating_sub(start_line) as u16;
        let cursor_x = area
            .x
            .saturating_add(1 + prefix_width)
            .saturating_add(cursor_display_pos as u16)
            .saturating_sub(horizontal_scroll);
        let cursor_y = area
            .y
            .saturating_add(1 + padding_v)
            .saturating_add(cursor_row);
        cursor_pos = Some((cursor_x, cursor_y));

        display_lines
    } else {
        let (display_text, horizontal_scroll) = if mode == InputMode::Insert {
            let cursor_index = app.draft_cursor_byte_index();
            let draft = app.draft_text();
            let text_before_cursor = &draft[..cursor_index];
            let cursor_display_pos = text_before_cursor.width();

            if cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
                let mut byte_offset = 0;
                let mut skipped_width = 0;
                for (idx, grapheme) in draft.grapheme_indices(true) {
                    if skipped_width >= scroll_target {
                        byte_offset = idx;
                        break;
                    }
                    skipped_width += grapheme.width();
                }
                (draft[byte_offset..].to_string(), skipped_width as u16)
            } else {
                (draft.to_string(), 0u16)
            }
        } else if mode == InputMode::Command
            && let Some(cmd) = &command_line
        {
            let cursor_byte_index = command_cursor_byte_index
                .unwrap_or(cmd.len())
                .min(cmd.len());
            let text_before_cursor = &cmd[..cursor_byte_index];
            let cursor_display_pos = text_before_cursor.width();
            if cursor_display_pos >= content_width {
                let scroll_target = cursor_display_pos - content_width + 1;
                let mut byte_offset = 0;
                let mut skipped_width = 0;
                for (idx, grapheme) in cmd.grapheme_indices(true) {
                    if skipped_width >= scroll_target {
                        byte_offset = idx;
                        break;
                    }
                    skipped_width += grapheme.width();
                }
                (cmd[byte_offset..].to_string(), skipped_width as u16)
            } else {
                (cmd.clone(), 0u16)
            }
        } else {
            (
                match mode {
                    InputMode::Insert
                    | InputMode::Normal
                    | InputMode::ModelSelect
                    | InputMode::FileSelect
                    | InputMode::Settings => app.draft_text().to_string(),
                    InputMode::Command => command_line.clone().unwrap_or_default(),
                },
                0u16,
            )
        };

        let prefix_style = if mode == InputMode::Command {
            Style::default().fg(palette.yellow)
        } else {
            Style::default().fg(palette.primary)
        };
        let mut spans = vec![Span::styled(prefix, prefix_style)];
        spans.extend(highlight_file_refs(&display_text, palette));

        if mode == InputMode::Insert {
            let cursor_index = app.draft_cursor_byte_index();
            let text_before_cursor = &app.draft_text()[..cursor_index];
            let cursor_display_pos = text_before_cursor.width() as u16;
            let cursor_x = area
                .x
                .saturating_add(1 + prefix_width)
                .saturating_add(cursor_display_pos)
                .saturating_sub(horizontal_scroll);
            let cursor_y = area.y.saturating_add(1 + padding_v);
            cursor_pos = Some((cursor_x, cursor_y));
        } else if mode == InputMode::Command
            && let Some(command_line) = command_line.as_ref()
        {
            let cursor_byte_index = command_cursor_byte_index
                .unwrap_or(command_line.len())
                .min(command_line.len());
            let text_before_cursor = &command_line[..cursor_byte_index];
            let cursor_display_pos = text_before_cursor.width() as u16;
            let cursor_x = area
                .x
                .saturating_add(1 + prefix_width)
                .saturating_add(cursor_display_pos)
                .saturating_sub(horizontal_scroll);
            let cursor_y = area.y.saturating_add(1 + padding_v);
            cursor_pos = Some((cursor_x, cursor_y));
        }

        vec![Line::from(spans)]
    };

    let (model_text, model_style) = if app.is_loading() {
        let spinner = spinner_frame(app.tick_count(), app.ui_options());
        (
            format!("{spinner} {}", app.model()),
            Style::default().fg(palette.primary),
        )
    } else if app.current_api_key().is_some() {
        (
            format!("{} {}", glyphs.status_ready, app.model()),
            Style::default().fg(palette.success),
        )
    } else {
        (
            format!("{} No API key", glyphs.status_missing),
            Style::default().fg(palette.error),
        )
    };

    let input = Paragraph::new(input_lines).block(
        Block::default()
            .title_top(Line::from(vec![Span::styled(mode_text, mode_style)]))
            .title_top(Line::from(hints).alignment(Alignment::Right))
            .title_bottom(Line::from(vec![Span::styled(model_text, model_style)]))
            .title_bottom({
                let mut spans = Vec::new();
                if let Some(plan_line) = app.plan_status_line() {
                    spans.push(Span::styled(
                        sanitize_display_text(&plan_line),
                        Style::default().fg(palette.primary),
                    ));
                    spans.push(Span::styled("  ", Style::default()));
                }
                spans.push(Span::styled(usage_str, Style::default().fg(usage_color)));
                if let Some(color) = diag_color {
                    spans.push(Span::styled(
                        format!("  {diag_str}"),
                        Style::default().fg(color),
                    ));
                }
                Line::from(spans).alignment(Alignment::Right)
            })
            .padding(input_padding),
    );

    frame.render_widget(input, area);

    if let Some((cursor_x, cursor_y)) = cursor_pos {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_command_palette(frame: &mut Frame, app: &App, palette: &Palette) {
    let area = frame.area();
    let palette_width = 50.min(area.width.saturating_sub(4));
    let palette_height = 14;

    let palette_area = Rect {
        x: area.x + (area.width.saturating_sub(palette_width) / 2),
        y: area.y + (area.height / 3),
        width: palette_width,
        height: palette_height,
    };

    frame.render_widget(Clear, palette_area);

    let filter_raw = match app.command_input_access() {
        forge_engine::CommandInputAccess::Active { text, .. } => text.trim(),
        forge_engine::CommandInputAccess::Inactive => "",
    };
    let filter = filter_raw.trim_start_matches('/').to_ascii_lowercase();

    let commands = command_specs();

    let filtered: Vec<_> = if filter.is_empty() {
        commands.iter().collect()
    } else {
        commands
            .iter()
            .filter(|spec| {
                spec.palette_label.to_ascii_lowercase().contains(&filter)
                    || spec.description.to_ascii_lowercase().contains(&filter)
            })
            .collect()
    };

    let mut lines: Vec<Line> = vec![Line::from("")];
    let filter_line = if filter.is_empty() {
        "  Type to filter commands..."
    } else {
        "  Filter active"
    };
    lines.push(Line::from(Span::styled(
        filter_line,
        Style::default()
            .fg(palette.text_muted)
            .add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));

    if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matching commands",
            Style::default().fg(palette.text_muted),
        )));
    } else {
        for spec in filtered {
            let cmd = spec.palette_label;
            let desc = spec.description;
            lines.push(Line::from(vec![
                Span::styled(format!("  /{cmd}"), Style::default().fg(palette.peach)),
                Span::styled(format!("  {desc}"), Style::default().fg(palette.text_muted)),
            ]));
        }
    }

    let palette = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.primary))
            .style(Style::default().bg(palette.bg_panel))
            .title(Line::from(vec![Span::styled(
                " Commands ",
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
            )])),
    );

    frame.render_widget(palette, palette_area);
}

fn draw_files_panel(frame: &mut Frame, app: &App, area: Rect, palette: &Palette, glyphs: &Glyphs) {
    let files = app.ordered_files();
    let panel = app.files_panel_state().clone();
    let is_expanded = panel.expanded.is_some();

    let hint = if is_expanded {
        " Tab/S-Tab │ Enter: collapse │ C-D/U "
    } else {
        " Tab: cycle files "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.text_muted))
        .title(" Files ")
        .title_style(Style::default().fg(palette.text_secondary))
        .title_bottom(
            Line::from(hint)
                .centered()
                .style(Style::default().fg(palette.text_muted)),
        )
        .style(Style::default().bg(palette.bg_dark));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if files.is_empty() {
        let text = Paragraph::new(Line::styled(
            "  No files modified",
            Style::default().fg(palette.text_muted),
        ));
        frame.render_widget(text, inner);
        return;
    }

    // Split inner area: file list (top) and diff (bottom, if expanded)
    let (list_area, diff_area) = if is_expanded {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(files.len() as u16 + 1),
                Constraint::Min(3),
            ])
            .split(inner);
        (chunks[0], Some(chunks[1]))
    } else {
        (inner, None)
    };

    draw_file_list(frame, list_area, &files, &panel, palette, glyphs);

    if let Some(diff_area) = diff_area {
        draw_diff_view(frame, diff_area, app, &panel, palette);
    }
}

fn draw_file_list(
    frame: &mut Frame,
    area: Rect,
    files: &[(std::path::PathBuf, ChangeKind)],
    panel: &forge_engine::FilesPanelState,
    palette: &Palette,
    _glyphs: &Glyphs,
) {
    let inner_width = area.width.saturating_sub(2) as usize;

    let lines: Vec<Line> = files
        .iter()
        .enumerate()
        .map(|(i, (path, kind))| {
            let display = truncate_path_display(path, inner_width.saturating_sub(4));
            let is_selected = i == panel.selected;
            let is_file_expanded = panel.expanded.as_ref() == Some(path);

            let prefix = if is_selected {
                if is_file_expanded {
                    " ▶ ".to_string()
                } else {
                    " › ".to_string()
                }
            } else {
                "   ".to_string()
            };

            let kind_color = match kind {
                ChangeKind::Modified => palette.warning,
                ChangeKind::Created => palette.success,
            };

            let style = if is_selected {
                Style::default()
                    .fg(kind_color)
                    .bg(palette.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(kind_color)
            };

            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(display, style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_diff_view(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    panel: &forge_engine::FilesPanelState,
    palette: &Palette,
) {
    // Horizontal divider at top
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    let divider_str: String = "─".repeat(area.width as usize);
    let divider =
        Paragraph::new(Line::from(divider_str)).style(Style::default().fg(palette.text_muted));
    frame.render_widget(divider, chunks[0]);

    let diff_area = chunks[1];

    match app.files_panel_diff() {
        Some(FileDiff::Diff(text) | FileDiff::Created(text)) => {
            let lines = render_tool_result_lines(&text, Style::default(), palette, " ");
            let total_lines = lines.len();

            let max_scroll = total_lines.saturating_sub(diff_area.height as usize);
            let scroll = panel.diff_scroll.min(max_scroll);

            let visible: Vec<Line> = lines
                .into_iter()
                .skip(scroll)
                .take(diff_area.height as usize)
                .collect();

            frame.render_widget(Paragraph::new(visible), diff_area);
        }
        Some(FileDiff::Deleted) => {
            let text = Paragraph::new(Line::styled(
                " File no longer exists",
                Style::default().fg(palette.text_muted),
            ));
            frame.render_widget(text, diff_area);
        }
        Some(FileDiff::Binary(size)) => {
            let text = Paragraph::new(Line::styled(
                format!(" Binary file ({size} bytes)"),
                Style::default().fg(palette.text_muted),
            ));
            frame.render_widget(text, diff_area);
        }
        Some(FileDiff::Error(e)) => {
            let text = Paragraph::new(Line::styled(
                format!(" Error: {e}"),
                Style::default().fg(palette.error),
            ));
            frame.render_widget(text, diff_area);
        }
        None => {}
    }
}

/// Truncate a path for display, keeping the filename and as much of the parent as fits.
fn truncate_path_display(path: &std::path::Path, max_width: usize) -> String {
    let display = sanitize_path_for_display(path);
    if display.width() <= max_width {
        return display;
    }
    // Path doesn't fit - try to show just the filename
    if let Some(name) = path.file_name() {
        let name_str = name.to_string_lossy();
        // Check if filename alone fits
        if name_str.width() <= max_width {
            return name_str.into_owned();
        }
        // Filename doesn't fit - truncate it
        if max_width > 3 {
            let truncated: String = name_str
                .graphemes(true)
                .take(max_width.saturating_sub(3))
                .collect();
            return format!("{truncated}...");
        }
    }
    // Fallback: truncate from the right
    if max_width > 3 {
        let truncated: String = display
            .graphemes(true)
            .take(max_width.saturating_sub(3))
            .collect();
        return format!("{truncated}...");
    }
    display
}

fn runtime_detail_lines(
    app: &mut App,
    palette: &Palette,
    content_width: usize,
) -> Vec<Line<'static>> {
    let snapshot = app.runtime_snapshot();
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        " Session",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ───────────────────────────────────────────────────────────",
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled(" Active Profile: ", Style::default().fg(palette.text_muted)),
        Span::styled(
            snapshot.active_profile,
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            " Session Config Hash: ",
            Style::default().fg(palette.text_muted),
        ),
        Span::styled(
            snapshot.session_config_hash,
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Current Mode",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ───────────────────────────────────────────────────────────",
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled(" Mode: ", Style::default().fg(palette.text_muted)),
        Span::styled(snapshot.mode, Style::default().fg(palette.text_secondary)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Active Model: ", Style::default().fg(palette.text_muted)),
        Span::styled(
            snapshot.active_model,
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Provider: ", Style::default().fg(palette.text_muted)),
        Span::styled(
            format!(
                "{} ({})",
                snapshot.provider.display_name(),
                snapshot.provider_status
            ),
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Context",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ───────────────────────────────────────────────────────────",
        Style::default().fg(palette.primary_dim),
    )));
    let usage_pct = if snapshot.context_budget_tokens == 0 {
        0.0
    } else {
        (snapshot.context_used_tokens as f32 / snapshot.context_budget_tokens as f32) * 100.0
    };
    lines.push(Line::from(vec![
        Span::styled(" Usage: ", Style::default().fg(palette.text_muted)),
        Span::styled(
            format!(
                "{usage_pct:.0}% ({} / {})",
                format_token_count(snapshot.context_used_tokens),
                format_token_count(snapshot.context_budget_tokens)
            ),
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            " Distill Threshold: ",
            Style::default().fg(palette.text_muted),
        ),
        Span::styled(
            format_token_count(snapshot.distill_threshold_tokens),
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    let auto_attached = if snapshot.auto_attached.is_empty() {
        "(none)".to_string()
    } else {
        snapshot.auto_attached.join(", ")
    };
    lines.push(Line::from(vec![
        Span::styled(" Auto-Attached: ", Style::default().fg(palette.text_muted)),
        Span::styled(auto_attached, Style::default().fg(palette.text_secondary)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Health",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ───────────────────────────────────────────────────────────",
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            " Rate Limit State: ",
            Style::default().fg(palette.text_muted),
        ),
        Span::styled(
            snapshot.rate_limit_state,
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Last API Call: ", Style::default().fg(palette.text_muted)),
        Span::styled(
            snapshot.last_api_call,
            Style::default().fg(palette.text_secondary),
        ),
    ]));
    let last_error = snapshot.last_error.unwrap_or_else(|| "None".to_string());
    lines.push(Line::from(vec![
        Span::styled(" Last Error: ", Style::default().fg(palette.text_muted)),
        Span::styled(last_error, Style::default().fg(palette.text_secondary)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Session Overrides",
        Style::default()
            .fg(palette.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ───────────────────────────────────────────────────────────",
        Style::default().fg(palette.primary_dim),
    )));
    if snapshot.session_overrides.is_empty() {
        lines.push(Line::from(Span::styled(
            " (none - using profile/default layers)",
            Style::default().fg(palette.text_muted),
        )));
    } else {
        for item in snapshot.session_overrides {
            lines.push(Line::from(vec![
                Span::styled(" - ", Style::default().fg(palette.text_muted)),
                Span::styled(item, Style::default().fg(palette.text_secondary)),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("Esc/q", styles::key_highlight(palette)),
        Span::styled(" close", styles::key_hint(palette)),
    ]));
    lines
}

fn resolve_detail_lines(
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
    content_width: usize,
) -> Vec<Line<'static>> {
    let cascade = app.resolve_cascade();
    let mut lines = Vec::new();
    let selected_index = match app.settings_access() {
        SettingsAccess::Active { selected_index, .. } => selected_index,
        SettingsAccess::Inactive => 0,
    };

    for (index, setting) in cascade.settings.into_iter().enumerate() {
        let is_selected = index == selected_index;
        let marker = if is_selected { glyphs.selected } else { " " };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {}", setting.setting),
            if is_selected {
                Style::default()
                    .fg(palette.text_primary)
                    .bg(palette.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD)
            },
        )));
        lines.push(Line::from(Span::styled(
            " ───────────────────────────────────────────────────────────",
            if is_selected {
                Style::default()
                    .fg(palette.primary_dim)
                    .bg(palette.bg_highlight)
            } else {
                Style::default().fg(palette.primary_dim)
            },
        )));
        for layer in setting.layers {
            let winner = if layer.is_winner { "  <- winner" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<8}", layer.layer),
                    if is_selected {
                        Style::default()
                            .fg(palette.text_muted)
                            .bg(palette.bg_highlight)
                    } else {
                        Style::default().fg(palette.text_muted)
                    },
                ),
                Span::styled(
                    "  ",
                    if is_selected {
                        Style::default()
                            .fg(palette.text_muted)
                            .bg(palette.bg_highlight)
                    } else {
                        Style::default().fg(palette.text_muted)
                    },
                ),
                Span::styled(
                    layer.value,
                    if is_selected {
                        Style::default()
                            .fg(palette.text_secondary)
                            .bg(palette.bg_highlight)
                    } else {
                        Style::default().fg(palette.text_secondary)
                    },
                ),
                Span::styled(
                    winner,
                    if is_selected {
                        Style::default()
                            .fg(palette.success)
                            .bg(palette.bg_highlight)
                    } else {
                        Style::default().fg(palette.success)
                    },
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        format!(" Session Config Hash: {}", cascade.session_config_hash),
        Style::default().fg(palette.text_muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("↑↓/j/k", styles::key_highlight(palette)),
        Span::styled(" move  ", styles::key_hint(palette)),
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" jump to setting  ", styles::key_hint(palette)),
        Span::styled("Esc/q", styles::key_highlight(palette)),
        Span::styled(" close", styles::key_hint(palette)),
    ]));
    lines
}

fn validation_detail_lines(
    app: &App,
    palette: &Palette,
    glyphs: &Glyphs,
    content_width: usize,
) -> Vec<Line<'static>> {
    let report = app.validate_config();
    let mut lines = Vec::new();

    let push_section = |lines: &mut Vec<Line<'static>>,
                        title: String,
                        items: Vec<forge_engine::ValidationFinding>,
                        level_style: Style| {
        lines.push(Line::from(Span::styled(
            title,
            level_style.add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            " ───────────────────────────────────────────────────────────",
            Style::default().fg(palette.primary_dim),
        )));
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (none)",
                Style::default().fg(palette.text_muted),
            )));
        } else {
            for item in items {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", glyphs.bullet), level_style),
                    Span::styled(item.title, Style::default().fg(palette.text_secondary)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(item.detail, Style::default().fg(palette.text_muted)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("    Fix: ", Style::default().fg(palette.text_muted)),
                    Span::styled(item.fix_path, Style::default().fg(palette.text_secondary)),
                ]));
            }
        }
        lines.push(Line::from(""));
    };

    push_section(
        &mut lines,
        format!(" Errors ({})", report.errors.len()),
        report.errors,
        Style::default().fg(palette.error),
    );
    push_section(
        &mut lines,
        format!(" Warnings ({})", report.warnings.len()),
        report.warnings,
        Style::default().fg(palette.warning),
    );
    push_section(
        &mut lines,
        format!(" Healthy ({})", report.healthy.len()),
        report.healthy,
        Style::default().fg(palette.success),
    );

    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("Esc/q", styles::key_highlight(palette)),
        Span::styled(" close", styles::key_hint(palette)),
    ]));
    lines
}

fn settings_category_summary(app: &App, category: SettingsCategory) -> String {
    match category {
        SettingsCategory::Providers => {
            let configured = Provider::all()
                .iter()
                .filter(|provider| app.has_api_key(**provider))
                .count();
            format!("{configured} configured")
        }
        SettingsCategory::Models => {
            let mut summary = format!("{} usable", app.settings_usable_model_count());
            if app.settings_pending_model_apply_next_turn() {
                summary.push_str(" (next turn)");
            }
            summary
        }
        SettingsCategory::Profiles => "planned".to_string(),
        SettingsCategory::Context => {
            let mut summary = if app.settings_configured_context_memory_enabled() {
                "memory on".to_string()
            } else {
                "memory off".to_string()
            };
            if app.settings_pending_context_apply_next_turn() {
                summary.push_str(" (next turn)");
            }
            summary
        }
        SettingsCategory::Tools => {
            let mut summary = format!(
                "{} mode",
                app.settings_configured_tool_approval_mode_label()
            );
            if app.settings_pending_tools_apply_next_turn() {
                summary.push_str(" (next turn)");
            }
            summary
        }
        SettingsCategory::Keybindings => "vim".to_string(),
        SettingsCategory::History => "available".to_string(),
        SettingsCategory::Appearance => {
            let options = app.settings_configured_ui_options();
            let mut summary = if options.high_contrast {
                "high-contrast".to_string()
            } else if options.ascii_only {
                "ascii".to_string()
            } else {
                "default".to_string()
            };
            if app.settings_pending_ui_apply_next_turn() {
                summary.push_str(" (next turn)");
            }
            summary
        }
    }
}

fn settings_detail_lines(
    app: &App,
    category: SettingsCategory,
    palette: &Palette,
    glyphs: &Glyphs,
    content_width: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let phase_label = if category == SettingsCategory::Appearance
        || category == SettingsCategory::Models
        || category == SettingsCategory::Tools
        || category == SettingsCategory::Context
    {
        " Editable defaults (Phase 3)"
    } else {
        " Read-only preview"
    };
    lines.push(Line::from(Span::styled(
        phase_label,
        Style::default().fg(palette.text_muted),
    )));
    lines.push(Line::from(""));

    match category {
        SettingsCategory::Providers => {
            for provider in Provider::all() {
                let configured = app.has_api_key(*provider);
                let icon = if configured {
                    glyphs.status_ready
                } else {
                    glyphs.status_missing
                };
                let status = if configured { "configured" } else { "not set" };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {icon} "),
                        Style::default().fg(if configured {
                            palette.success
                        } else {
                            palette.text_muted
                        }),
                    ),
                    Span::styled(
                        provider.display_name().to_string(),
                        Style::default().fg(palette.text_secondary),
                    ),
                    Span::styled(
                        format!("  {status}"),
                        Style::default().fg(palette.text_muted),
                    ),
                ]));
            }
        }
        SettingsCategory::Models => {
            let editor = app.settings_model_editor_snapshot();
            let configured = app.settings_configured_model();
            let selected = editor.as_ref().map(|state| state.selected);
            let draft = editor.as_ref().map_or(configured, |state| &state.draft);

            lines.push(Line::from(vec![
                Span::styled("  Model: ", Style::default().fg(palette.text_muted)),
                Span::styled(
                    configured.to_string(),
                    Style::default()
                        .fg(palette.text_primary)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));

            for (index, predefined) in PredefinedModel::all().iter().enumerate() {
                let is_selected = selected == Some(index);
                let is_draft = predefined.to_model_name() == *draft;
                let marker = if is_selected { glyphs.selected } else { " " };
                let check = if is_draft {
                    glyphs.status_ready
                } else {
                    glyphs.status_missing
                };
                let label = format!(" {marker} {check} {}", predefined.display_name());
                let value = predefined.model_id();
                let filler = content_width.saturating_sub(label.width() + value.width() + 2);
                let bg = is_selected.then_some(palette.bg_highlight);

                let mut label_style = if is_selected {
                    Style::default()
                        .fg(palette.text_primary)
                        .add_modifier(Modifier::BOLD)
                } else if is_draft {
                    Style::default().fg(palette.text_secondary)
                } else {
                    Style::default().fg(palette.text_muted)
                };
                let mut filler_style = Style::default();
                let mut value_style = Style::default().fg(palette.text_muted);
                if let Some(bg) = bg {
                    label_style = label_style.bg(bg);
                    filler_style = filler_style.bg(bg);
                    value_style = value_style.bg(bg);
                }

                lines.push(Line::from(vec![
                    Span::styled(label, label_style),
                    Span::styled(" ".repeat(filler), filler_style),
                    Span::styled("  ", filler_style),
                    Span::styled(value, value_style),
                ]));
            }

            lines.push(Line::from(""));
            let usable_models = app.settings_usable_model_count();
            let total_models = PredefinedModel::all().len();
            lines.push(Line::from(vec![
                Span::styled("  Usable now: ", Style::default().fg(palette.text_muted)),
                Span::styled(
                    format!("{usable_models}/{total_models}"),
                    Style::default().fg(palette.text_secondary),
                ),
            ]));
            let dirty = editor.as_ref().is_some_and(|state| state.dirty);
            let dirty_value = if dirty { "yes" } else { "no" };
            lines.push(Line::from(vec![
                Span::styled("  Dirty: ", Style::default().fg(palette.text_muted)),
                Span::styled(dirty_value, Style::default().fg(palette.text_secondary)),
            ]));
            let apply_value = if app.settings_pending_model_apply_next_turn() {
                "next turn"
            } else {
                "none"
            };
            lines.push(Line::from(vec![
                Span::styled("  Pending apply: ", Style::default().fg(palette.text_muted)),
                Span::styled(apply_value, Style::default().fg(palette.text_secondary)),
            ]));
        }
        SettingsCategory::Context => {
            let editor = app.settings_context_editor_snapshot();
            let configured_memory = app.settings_configured_context_memory_enabled();
            let selected = editor.map(|state| state.selected);
            let draft_memory_enabled = editor
                .map(|state| state.draft_memory_enabled)
                .unwrap_or(configured_memory);
            let active_memory = app.memory_enabled();

            lines.push(Line::from(vec![
                Span::styled("  Active now: ", Style::default().fg(palette.text_muted)),
                Span::styled(
                    if active_memory { "on" } else { "off" },
                    Style::default().fg(palette.text_secondary),
                ),
            ]));

            lines.push(Line::from(""));
            let is_selected = selected == Some(0);
            let marker = if is_selected { glyphs.selected } else { " " };
            let value = if draft_memory_enabled { "on" } else { "off" };
            let label = format!(" {marker} Memory-enabled context");
            let filler = content_width.saturating_sub(label.width() + value.width() + 2);
            let bg = is_selected.then_some(palette.bg_highlight);
            let mut label_style = if is_selected {
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.text_secondary)
            };
            let mut filler_style = Style::default();
            let mut value_style = Style::default().fg(palette.text_muted);
            if let Some(bg) = bg {
                label_style = label_style.bg(bg);
                filler_style = filler_style.bg(bg);
                value_style = value_style.bg(bg);
            }
            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(" ".repeat(filler), filler_style),
                Span::styled("  ", filler_style),
                Span::styled(value, value_style),
            ]));

            lines.push(Line::from(""));
            let dirty = editor.is_some_and(|state| state.dirty);
            lines.push(Line::from(vec![
                Span::styled("  Dirty: ", Style::default().fg(palette.text_muted)),
                Span::styled(
                    if dirty { "yes" } else { "no" },
                    Style::default().fg(palette.text_secondary),
                ),
            ]));
            let apply_value = if app.settings_pending_context_apply_next_turn() {
                "next turn"
            } else {
                "none"
            };
            lines.push(Line::from(vec![
                Span::styled("  Pending apply: ", Style::default().fg(palette.text_muted)),
                Span::styled(apply_value, Style::default().fg(palette.text_secondary)),
            ]));
        }
        SettingsCategory::Tools => {
            let editor = app.settings_tools_editor_snapshot();
            let selected = editor.map(|state| state.selected);
            let draft_mode = editor
                .map(|state| state.draft_approval_mode)
                .unwrap_or_else(|| app.settings_configured_tool_approval_mode_label());
            let is_selected = selected == Some(0);
            let marker = if is_selected { glyphs.selected } else { " " };
            let label = format!(" {marker} Approval mode");
            let filler = content_width.saturating_sub(label.width() + draft_mode.width() + 2);
            let bg = is_selected.then_some(palette.bg_highlight);
            let mut left_style = if is_selected {
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.text_secondary)
            };
            let mut fill_style = Style::default();
            let mut right_style = Style::default().fg(palette.text_muted);
            if let Some(bg) = bg {
                left_style = left_style.bg(bg);
                fill_style = fill_style.bg(bg);
                right_style = right_style.bg(bg);
            }
            lines.push(Line::from(vec![
                Span::styled(label, left_style),
                Span::styled(" ".repeat(filler), fill_style),
                Span::styled("  ", fill_style),
                Span::styled(draft_mode, right_style),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "  Registered tools: ",
                    Style::default().fg(palette.text_muted),
                ),
                Span::styled(
                    app.tool_definition_count().to_string(),
                    Style::default().fg(palette.text_secondary),
                ),
            ]));
            lines.push(Line::from(""));
            let dirty = editor.is_some_and(|state| state.dirty);
            lines.push(Line::from(vec![
                Span::styled("  Dirty: ", Style::default().fg(palette.text_muted)),
                Span::styled(
                    if dirty { "yes" } else { "no" },
                    Style::default().fg(palette.text_secondary),
                ),
            ]));
            let apply_value = if app.settings_pending_tools_apply_next_turn() {
                "next turn"
            } else {
                "none"
            };
            lines.push(Line::from(vec![
                Span::styled("  Pending apply: ", Style::default().fg(palette.text_muted)),
                Span::styled(apply_value, Style::default().fg(palette.text_secondary)),
            ]));
        }
        SettingsCategory::Keybindings => {
            lines.push(Line::from(Span::styled(
                "  Preset: vim",
                Style::default().fg(palette.text_secondary),
            )));
            lines.push(Line::from(Span::styled(
                "  Rebinding UI arrives in later phases.",
                Style::default().fg(palette.text_muted),
            )));
        }
        SettingsCategory::Profiles => {
            lines.push(Line::from(Span::styled(
                "  Profile management arrives in Phase 4.",
                Style::default().fg(palette.text_muted),
            )));
        }
        SettingsCategory::History => {
            lines.push(Line::from(Span::styled(
                "  History/privacy controls are planned.",
                Style::default().fg(palette.text_muted),
            )));
        }
        SettingsCategory::Appearance => {
            let editor = app.settings_appearance_editor_snapshot();
            let defaults = app.settings_configured_ui_options();
            let draft = editor.map_or(defaults, |state| state.draft);
            let selected = editor.map(|state| state.selected);
            let items = [
                ("ASCII only", draft.ascii_only),
                ("High contrast", draft.high_contrast),
                ("Reduced motion", draft.reduced_motion),
                ("Show thinking blocks", draft.show_thinking),
            ];

            for (index, (label, enabled)) in items.into_iter().enumerate() {
                let is_selected = selected == Some(index);
                let marker = if is_selected { glyphs.selected } else { " " };
                let value = if enabled { "on" } else { "off" };
                let left = format!(" {marker} {label}");
                let filler = content_width.saturating_sub(left.width() + value.width() + 2);
                let bg = is_selected.then_some(palette.bg_highlight);

                let mut left_style = if is_selected {
                    Style::default()
                        .fg(palette.text_primary)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text_secondary)
                };
                let mut fill_style = Style::default();
                let mut right_style = Style::default().fg(palette.text_muted);
                if let Some(bg) = bg {
                    left_style = left_style.bg(bg);
                    fill_style = fill_style.bg(bg);
                    right_style = right_style.bg(bg);
                }

                lines.push(Line::from(vec![
                    Span::styled(left, left_style),
                    Span::styled(" ".repeat(filler), fill_style),
                    Span::styled("  ", fill_style),
                    Span::styled(value, right_style),
                ]));
            }

            lines.push(Line::from(""));
            let dirty = editor.is_some_and(|state| state.dirty);
            let dirty_value = if dirty { "yes" } else { "no" };
            lines.push(Line::from(vec![
                Span::styled("  Dirty: ", Style::default().fg(palette.text_muted)),
                Span::styled(dirty_value, Style::default().fg(palette.text_secondary)),
            ]));
            let apply_value = if app.settings_pending_ui_apply_next_turn() {
                "next turn"
            } else {
                "none"
            };
            lines.push(Line::from(vec![
                Span::styled("  Pending apply: ", Style::default().fg(palette.text_muted)),
                Span::styled(apply_value, Style::default().fg(palette.text_secondary)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    if category == SettingsCategory::Appearance
        || category == SettingsCategory::Models
        || category == SettingsCategory::Tools
        || category == SettingsCategory::Context
    {
        let action_label = if category == SettingsCategory::Models {
            " select  "
        } else if category == SettingsCategory::Tools {
            " cycle  "
        } else {
            " toggle  "
        };
        lines.push(Line::from(vec![
            Span::styled("↑↓", styles::key_highlight(palette)),
            Span::styled(" move  ", styles::key_hint(palette)),
            Span::styled("Space/Enter", styles::key_highlight(palette)),
            Span::styled(action_label, styles::key_hint(palette)),
            Span::styled("s", styles::key_highlight(palette)),
            Span::styled(" save  ", styles::key_hint(palette)),
            Span::styled("r", styles::key_highlight(palette)),
            Span::styled(" revert", styles::key_hint(palette)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Esc/q", styles::key_highlight(palette)),
            Span::styled(" back", styles::key_hint(palette)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Esc/q", styles::key_highlight(palette)),
            Span::styled(" back", styles::key_hint(palette)),
        ]));
    }
    lines
}

fn settings_scope(surface: SettingsSurface) -> &'static str {
    match surface {
        SettingsSurface::Root | SettingsSurface::Validate => "Global",
        SettingsSurface::Runtime | SettingsSurface::Resolve => "Session",
    }
}

fn settings_layer(surface: SettingsSurface) -> &'static str {
    match surface {
        SettingsSurface::Root => "Settings",
        SettingsSurface::Runtime => "Runtime",
        SettingsSurface::Resolve => "Resolve",
        SettingsSurface::Validate => "Validation",
    }
}

fn settings_compass_line(app: &App, surface: SettingsSurface, palette: &Palette) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            " Scope: {}   Layer: {}   Dirty: {}",
            settings_scope(surface),
            settings_layer(surface),
            if app.settings_has_unsaved_edits() {
                "yes"
            } else {
                "no"
            }
        ),
        Style::default().fg(palette.text_muted),
    ))
}

fn draw_settings_modal(
    frame: &mut Frame,
    app: &mut App,
    palette: &Palette,
    glyphs: &Glyphs,
    elapsed: Duration,
) {
    let area = frame.area();
    let (surface, root_filter, root_filter_active, root_detail_view, root_selected_index) =
        match app.settings_access() {
            SettingsAccess::Active {
                surface,
                filter_text,
                filter_active,
                detail_view,
                selected_index,
            } => (
                surface,
                filter_text.to_string(),
                filter_active,
                detail_view,
                selected_index,
            ),
            SettingsAccess::Inactive => (SettingsSurface::Root, String::new(), false, None, 0),
        };
    let selector_width = match surface {
        SettingsSurface::Root => 76.min(area.width.saturating_sub(4)).max(52),
        SettingsSurface::Runtime | SettingsSurface::Resolve | SettingsSurface::Validate => {
            96.min(area.width.saturating_sub(4)).max(60)
        }
    };
    let content_width = selector_width.saturating_sub(4).max(1) as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();

    match surface {
        SettingsSurface::Runtime => {
            lines.extend(runtime_detail_lines(app, palette, content_width));
        }
        SettingsSurface::Resolve => {
            lines.extend(resolve_detail_lines(app, palette, glyphs, content_width));
        }
        SettingsSurface::Validate => {
            lines.extend(validation_detail_lines(app, palette, glyphs, content_width));
        }
        SettingsSurface::Root => {
            let filter = root_filter.clone();
            let filter_active = root_filter_active;
            let detail_view = root_detail_view;
            let categories = app.settings_categories();
            let selected_index = root_selected_index.min(categories.len().saturating_sub(1));

            if let Some(category) = detail_view {
                lines.extend(settings_detail_lines(
                    app,
                    category,
                    palette,
                    glyphs,
                    content_width,
                ));
            } else {
                let filter_style = if filter_active {
                    Style::default()
                        .fg(palette.yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.primary)
                };
                let filter_text = if filter.is_empty() {
                    "Type to filter...".to_string()
                } else {
                    filter.clone()
                };
                let value_style = if filter.is_empty() {
                    Style::default().fg(palette.text_muted)
                } else {
                    Style::default().fg(palette.text_primary)
                };
                lines.push(Line::from(vec![
                    Span::styled(" / ", filter_style),
                    Span::styled(filter_text, value_style),
                ]));
                lines.push(Line::from(""));

                if categories.is_empty() {
                    lines.push(Line::from(Span::styled(
                        " No matching categories",
                        Style::default().fg(palette.warning),
                    )));
                } else {
                    for (idx, category) in categories.iter().enumerate() {
                        let is_selected = idx == selected_index;
                        let marker = if is_selected { glyphs.selected } else { " " };
                        let label = category.label();
                        let summary = settings_category_summary(app, *category);
                        let left = format!(" {marker} {label}");
                        let left_width = left.width();
                        let right_width = summary.width();
                        let filler = content_width.saturating_sub(left_width + right_width + 2);
                        let bg = is_selected.then_some(palette.bg_highlight);
                        let mut left_style = if is_selected {
                            Style::default()
                                .fg(palette.text_primary)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(palette.text_secondary)
                        };
                        let mut fill_style = Style::default();
                        let mut right_style = Style::default().fg(palette.text_muted);
                        if let Some(bg) = bg {
                            left_style = left_style.bg(bg);
                            fill_style = fill_style.bg(bg);
                            right_style = right_style.bg(bg);
                        }

                        lines.push(Line::from(vec![
                            Span::styled(left, left_style),
                            Span::styled(" ".repeat(filler), fill_style),
                            Span::styled("  ", fill_style),
                            Span::styled(summary, right_style),
                        ]));
                    }
                }

                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "─".repeat(content_width),
                    Style::default().fg(palette.primary_dim),
                )));
                if filter_active {
                    lines.push(Line::from(vec![
                        Span::styled("Type", styles::key_highlight(palette)),
                        Span::styled(" filter  ", styles::key_hint(palette)),
                        Span::styled("Backspace", styles::key_highlight(palette)),
                        Span::styled(" delete  ", styles::key_hint(palette)),
                        Span::styled("Enter", styles::key_highlight(palette)),
                        Span::styled(" done  ", styles::key_hint(palette)),
                        Span::styled("Esc", styles::key_highlight(palette)),
                        Span::styled(" stop", styles::key_hint(palette)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("Enter", styles::key_highlight(palette)),
                        Span::styled(" select  ", styles::key_hint(palette)),
                        Span::styled("/", styles::key_highlight(palette)),
                        Span::styled(" filter  ", styles::key_hint(palette)),
                        Span::styled("q", styles::key_highlight(palette)),
                        Span::styled(" quit", styles::key_hint(palette)),
                    ]));
                }
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(settings_compass_line(app, surface, palette));

    let inner_height = lines.len() as u16;
    let selector_height = inner_height
        .saturating_add(4)
        .min(area.height.saturating_sub(2));
    let base_area = Rect {
        x: area.x + (area.width.saturating_sub(selector_width) / 2),
        y: area.y + (area.height.saturating_sub(selector_height) / 2),
        width: selector_width,
        height: selector_height,
    };

    let (selector_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, area),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    frame.render_widget(Clear, selector_area);

    let title = match surface {
        SettingsSurface::Root => {
            if let Some(category) = root_detail_view {
                let separator = if app.ui_options().ascii_only {
                    ">"
                } else {
                    "›"
                };
                format!("{} {separator} {}", surface.title(), category.label())
            } else {
                surface.title().to_string()
            }
        }
        _ => surface.title().to_string(),
    };
    let title_text = if app.settings_has_unsaved_edits() {
        format!(" {title} * ")
    } else {
        format!(" {title} ")
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(
            title_text,
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .title_alignment(Alignment::Center);

    frame.render_widget(Paragraph::new(lines).block(block), selector_area);
}

pub fn draw_model_selector(
    frame: &mut Frame,
    app: &mut App,
    palette: &Palette,
    glyphs: &Glyphs,
    elapsed: Duration,
) {
    let area = frame.area();
    let selected_index = match app.model_select_access() {
        ModelSelectAccess::Active { selected_index } => selected_index,
        ModelSelectAccess::Inactive => 0,
    };

    let selector_width = 60.min(area.width.saturating_sub(4)).max(40);
    let content_width = selector_width.saturating_sub(4).max(1) as usize; // borders + padding

    let divider = Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    ));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(divider);
    lines.push(Line::from(""));

    let models = PredefinedModel::all();
    let mut row_index = 0usize;
    let mut push_row = |label: &str, selected: bool, muted: bool, tag: Option<(&str, Style)>| {
        row_index += 1;

        let marker = if selected { glyphs.selected } else { " " };
        let prefix = format!(" {marker} ");
        let label_text = format!("{row_index:>2}.  {label}");
        let left_width = prefix.width() + label_text.width();
        let (right_text, right_style) = tag.unwrap_or(("", Style::default()));
        let right_width = right_text.width();
        let gap = if right_text.is_empty() { 0 } else { 2 };
        let filler = content_width.saturating_sub(left_width + right_width + gap);

        let bg = if selected {
            Some(palette.bg_highlight)
        } else {
            None
        };

        let mut arrow_style = Style::default().fg(palette.peach);
        if let Some(bg) = bg {
            arrow_style = arrow_style.bg(bg);
        }

        let mut label_style = if selected {
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD)
        } else if muted {
            Style::default().fg(palette.text_muted)
        } else {
            Style::default().fg(palette.text_secondary)
        };
        if let Some(bg) = bg {
            label_style = label_style.bg(bg);
        }

        let mut filler_style = Style::default();
        if let Some(bg) = bg {
            filler_style = filler_style.bg(bg);
        }

        let mut right_style = right_style;
        if let Some(bg) = bg {
            right_style = right_style.bg(bg);
        }

        lines.push(Line::from(vec![
            Span::styled(prefix, arrow_style),
            Span::styled(label_text, label_style),
            Span::styled(" ".repeat(filler), filler_style),
            Span::styled(" ".repeat(gap), filler_style),
            Span::styled(right_text.to_string(), right_style),
        ]));
        lines.push(Line::from(""));
    };

    for (i, model) in models.iter().enumerate() {
        let is_selected = i == selected_index;
        let firm_style = Style::default()
            .fg(palette.text_disabled)
            .add_modifier(Modifier::DIM);
        push_row(
            model.model_name(),
            is_selected,
            false,
            Some((model.firm_name(), firm_style)),
        );
    }

    if matches!(lines.last(), Some(line) if line.width() == 0) {
        lines.pop();
    }

    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" confirm  ", styles::key_hint(palette)),
        Span::styled("Esc", styles::key_highlight(palette)),
        Span::styled(" cancel", styles::key_hint(palette)),
    ]));

    let inner_height = lines.len() as u16;
    let selector_height = inner_height.saturating_add(4); // borders + vertical padding
    let desired_y = area.y + area.height.saturating_sub(12);
    let max_y = area.y + area.height.saturating_sub(selector_height);
    let y = desired_y.min(max_y);

    let base_area = Rect {
        x: area.x + (area.width.saturating_sub(selector_width) / 2),
        y,
        width: selector_width,
        height: selector_height,
    };

    let (selector_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, area),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    frame.render_widget(Clear, selector_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(
            " Select Model ",
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .title_alignment(Alignment::Center);

    let selector = Paragraph::new(lines).block(block);

    frame.render_widget(selector, selector_area);
}

fn draw_file_selector(
    frame: &mut Frame,
    app: &mut App,
    palette: &Palette,
    glyphs: &Glyphs,
    elapsed: Duration,
) {
    let area = frame.area();
    let (selected_index, filter) = match app.file_select_access() {
        FileSelectAccess::Active {
            filter,
            selected_index,
        } => (selected_index, filter.to_string()),
        FileSelectAccess::Inactive => (0, String::new()),
    };
    let files = app.file_select_files();

    let selector_width = 70.min(area.width.saturating_sub(4)).max(40);
    let content_width = selector_width.saturating_sub(4).max(1) as usize;

    let divider = Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    ));

    let mut lines: Vec<Line> = Vec::new();

    // Filter input line
    let filter_display = if filter.is_empty() {
        "Type to filter files...".to_string()
    } else {
        filter.clone()
    };
    let filter_style = if filter.is_empty() {
        Style::default().fg(palette.text_muted)
    } else {
        Style::default().fg(palette.text_primary)
    };
    lines.push(Line::from(vec![
        Span::styled(" @ ", Style::default().fg(palette.primary)),
        Span::styled(filter_display, filter_style),
    ]));

    lines.push(divider.clone());
    lines.push(Line::from(""));

    // File count info
    let file_picker = app.file_picker();
    let total = file_picker.total_count();
    let showing = files.len();
    let count_text = if filter.is_empty() {
        format!(" {showing} of {total} files")
    } else {
        format!(" {showing} matches")
    };
    lines.push(Line::from(Span::styled(
        count_text,
        Style::default().fg(palette.text_muted),
    )));
    lines.push(Line::from(""));

    // File list
    let max_visible = 12;
    let start_idx = if selected_index >= max_visible {
        selected_index - max_visible + 1
    } else {
        0
    };

    for (i, entry) in files.iter().enumerate().skip(start_idx).take(max_visible) {
        let is_selected = i == selected_index;
        let prefix = if is_selected { glyphs.selected } else { " " };

        // Build the file path with fuzzy match highlighting
        let match_positions = find_match_positions(&entry.display, &filter);
        let mut spans: Vec<Span> = Vec::new();

        let bg = if is_selected {
            Some(palette.bg_highlight)
        } else {
            None
        };

        let prefix_style = if let Some(bg) = bg {
            Style::default().fg(palette.primary).bg(bg)
        } else {
            Style::default().fg(palette.primary)
        };
        spans.push(Span::styled(format!(" {prefix} "), prefix_style));

        // Render path with highlighted matches
        let path_chars: Vec<char> = entry.display.chars().collect();
        let mut in_match = false;
        let mut segment = String::new();

        for (char_idx, &c) in path_chars.iter().enumerate() {
            let is_match = match_positions.contains(&char_idx);

            if is_match != in_match {
                // Flush current segment
                if !segment.is_empty() {
                    let style = if in_match {
                        let mut s = Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD);
                        if let Some(bg) = bg {
                            s = s.bg(bg);
                        }
                        s
                    } else {
                        let mut s = if is_selected {
                            Style::default().fg(palette.text_primary)
                        } else {
                            Style::default().fg(palette.text_secondary)
                        };
                        if let Some(bg) = bg {
                            s = s.bg(bg);
                        }
                        s
                    };
                    spans.push(Span::styled(segment.clone(), style));
                    segment.clear();
                }
                in_match = is_match;
            }
            segment.push(c);
        }

        // Flush final segment
        if !segment.is_empty() {
            let style = if in_match {
                let mut s = Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD);
                if let Some(bg) = bg {
                    s = s.bg(bg);
                }
                s
            } else {
                let mut s = if is_selected {
                    Style::default().fg(palette.text_primary)
                } else {
                    Style::default().fg(palette.text_secondary)
                };
                if let Some(bg) = bg {
                    s = s.bg(bg);
                }
                s
            };
            spans.push(Span::styled(segment, style));
        }

        // Pad to full width for consistent highlight
        let line_width: usize = spans.iter().map(|s| s.content.width()).sum();
        if line_width < content_width {
            let padding = content_width - line_width;
            let pad_style = if let Some(bg) = bg {
                Style::default().bg(bg)
            } else {
                Style::default()
            };
            spans.push(Span::styled(" ".repeat(padding), pad_style));
        }

        lines.push(Line::from(spans));
    }

    // Show scroll indicator if there are more files
    if files.len() > max_visible {
        lines.push(Line::from(Span::styled(
            format!(" ... and {} more", files.len() - max_visible),
            Style::default().fg(palette.text_muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(content_width),
        Style::default().fg(palette.primary_dim),
    )));
    lines.push(Line::from(vec![
        Span::styled("Enter", styles::key_highlight(palette)),
        Span::styled(" select  ", styles::key_hint(palette)),
        Span::styled("Esc", styles::key_highlight(palette)),
        Span::styled(" cancel", styles::key_hint(palette)),
    ]));

    let inner_height = lines.len() as u16;
    let selector_height = inner_height.saturating_add(4);
    let y = area.y + (area.height.saturating_sub(selector_height)) / 2;

    let base_area = Rect {
        x: area.x + (area.width.saturating_sub(selector_width) / 2),
        y,
        width: selector_width,
        height: selector_height,
    };

    let (selector_area, effect_done) = if let Some(effect) = app.modal_effect_mut() {
        effect.advance(elapsed);
        (
            apply_modal_effect(effect, base_area, area),
            effect.is_finished(),
        )
    } else {
        (base_area, false)
    };

    if effect_done {
        app.clear_modal_effect();
    }

    frame.render_widget(Clear, selector_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.primary))
        .style(Style::default().bg(palette.bg_panel))
        .padding(Padding::uniform(1))
        .title(Line::from(vec![Span::styled(
            " Select File ",
            Style::default()
                .fg(palette.text_primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .title_alignment(Alignment::Center);

    let selector = Paragraph::new(lines).block(block);

    frame.render_widget(selector, selector_area);
}

//! Modal animation effects for TUI overlays.

use std::f32::consts::TAU;

use ratatui::layout::Rect;

use forge_types::ui::{AnimPhase, ModalEffect, ModalEffectKind, PanelEffect, PanelEffectKind};

/// Apply a modal effect to transform the base rectangle.
#[must_use]
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect {
    let progress = match effect.phase() {
        AnimPhase::Running { progress } => progress,
        AnimPhase::Completed => 1.0,
    };
    match effect.kind() {
        ModalEffectKind::PopScale => {
            let t = ease_out_cubic(progress);
            let scale = 0.6 + 0.4 * t;
            scale_rect(base, scale)
        }
        ModalEffectKind::SlideUp => {
            let t = ease_out_cubic(progress);
            let viewport_bottom = viewport.y.saturating_add(viewport.height);
            let base_bottom = base.y.saturating_add(base.height);
            let max_offset = viewport_bottom.saturating_sub(base_bottom);
            let offset = max_offset.min(base.height.saturating_div(2)).min(6);
            let y_offset = ((1.0 - t) * f32::from(offset)).round() as u16;
            Rect {
                x: base.x,
                y: base.y.saturating_add(y_offset),
                width: base.width,
                height: base.height,
            }
        }
        ModalEffectKind::Shake => {
            let t = progress.clamp(0.0, 1.0);
            let decay = 1.0 - t;
            let oscillations = 4.0;
            let amplitude = 3.0;
            let offset = (f32::sin(t * TAU * oscillations) * amplitude * decay).round() as i32;
            let viewport_left = i32::from(viewport.x);
            let viewport_right = i32::from(viewport.x) + i32::from(viewport.width);
            let max_x = (viewport_right - i32::from(base.width)).max(viewport_left);
            let base_x = i32::from(base.x);
            let x = (base_x + offset).clamp(viewport_left, max_x) as u16;
            Rect { x, ..base }
        }
    }
}

/// Apply a panel effect to the files panel.
#[must_use]
pub fn apply_files_panel_effect(effect: &PanelEffect, base: Rect) -> Rect {
    let progress = match effect.phase() {
        AnimPhase::Running { progress } => progress,
        AnimPhase::Completed => 1.0,
    };
    match effect.kind() {
        PanelEffectKind::SlideOutRight => slide_panel_right(base, 1.0 - progress),
        PanelEffectKind::SlideInRight => slide_panel_right(base, progress),
    }
}

fn slide_panel_right(base: Rect, progress: f32) -> Rect {
    let t = ease_out_cubic(progress);
    let width = (f32::from(base.width) * t).round() as u16;
    let width = width.min(base.width);
    let x = base.x.saturating_add(base.width.saturating_sub(width));
    Rect { x, width, ..base }
}

fn scale_rect(base: Rect, scale: f32) -> Rect {
    let width = (f32::from(base.width) * scale).round() as u16;
    let height = (f32::from(base.height) * scale).round() as u16;
    let width = width.max(1).min(base.width);
    let height = height.max(1).min(base.height);
    let x = base.x + (base.width.saturating_sub(width) / 2);
    let y = base.y + (base.height.saturating_sub(height) / 2);
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

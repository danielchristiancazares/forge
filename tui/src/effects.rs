//! Modal animation effects for TUI overlays.

use ratatui::layout::Rect;

use forge_engine::{ModalEffect, ModalEffectKind};

/// Apply a modal effect to transform the base rectangle.
pub fn apply_modal_effect(effect: &ModalEffect, base: Rect, viewport: Rect) -> Rect {
    match effect.kind() {
        ModalEffectKind::PopScale => {
            let t = ease_out_cubic(effect.progress());
            let scale = 0.6 + 0.4 * t;
            scale_rect(base, scale)
        }
        ModalEffectKind::SlideUp => {
            let t = ease_out_cubic(effect.progress());
            let max_offset = viewport
                .height
                .saturating_sub(base.y.saturating_add(base.height));
            let offset = max_offset.min(base.height.saturating_div(2)).min(6);
            let y_offset = ((1.0 - t) * offset as f32).round() as u16;
            Rect {
                x: base.x,
                y: base.y.saturating_add(y_offset),
                width: base.width,
                height: base.height,
            }
        }
    }
}

fn scale_rect(base: Rect, scale: f32) -> Rect {
    let width = ((base.width as f32) * scale).round() as u16;
    let height = ((base.height as f32) * scale).round() as u16;
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

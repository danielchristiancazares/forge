//! Panel animation effects for UI components.

use std::time::Duration;

use super::animation::normalized_progress;

/// The kind of panel animation effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEffectKind {
    SlideInRight,
    SlideOutRight,
}

/// Animation state for the files panel.
#[derive(Debug, Clone)]
pub struct PanelEffect {
    kind: PanelEffectKind,
    elapsed: Duration,
    duration: Duration,
}

impl PanelEffect {
    #[must_use]
    pub fn slide_in_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideInRight,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    #[must_use]
    pub fn slide_out_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideOutRight,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    pub fn advance(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
    }

    #[must_use]
    pub fn progress(&self) -> f32 {
        normalized_progress(self.elapsed, self.duration)
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    #[must_use]
    pub fn kind(&self) -> PanelEffectKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slide_out_right_initial_state() {
        let effect = PanelEffect::slide_out_right(Duration::from_millis(150));
        assert_eq!(effect.kind(), PanelEffectKind::SlideOutRight);
        assert!(!effect.is_finished());
        assert!(effect.progress() < 0.1);
    }

    #[test]
    fn slide_in_right_initial_state() {
        let effect = PanelEffect::slide_in_right(Duration::from_millis(150));
        assert_eq!(effect.kind(), PanelEffectKind::SlideInRight);
        assert!(!effect.is_finished());
    }

    #[test]
    fn progress_clamped() {
        let mut effect = PanelEffect::slide_out_right(Duration::from_millis(10));
        effect.advance(Duration::from_millis(50));
        assert!(effect.progress() <= 1.0);
        assert!(effect.is_finished());
    }
}

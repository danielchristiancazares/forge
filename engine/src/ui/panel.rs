//! Panel animation effects for UI components.

use std::time::Duration;

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
    /// Create a slide-in-right effect.
    #[must_use]
    pub fn slide_in_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideInRight,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Create a slide-out-right effect.
    #[must_use]
    pub fn slide_out_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideOutRight,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Advance the animation by the given delta time.
    pub fn advance(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
    }

    /// Get the animation progress (0.0 to 1.0).
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = self.elapsed.as_secs_f32();
        let total = self.duration.as_secs_f32();
        (elapsed / total).clamp(0.0, 1.0)
    }

    /// Check if the animation is finished.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    /// Get the effect kind.
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

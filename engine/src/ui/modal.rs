//! Modal animation effects for TUI overlays.

use std::time::Duration;

/// The kind of modal animation effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalEffectKind {
    PopScale,
    SlideUp,
}

/// Modal animation effect state.
#[derive(Debug, Clone)]
pub struct ModalEffect {
    kind: ModalEffectKind,
    elapsed: Duration,
    duration: Duration,
}

impl ModalEffect {
    /// Create a pop-scale effect (used when entering model select).
    #[must_use]
    pub fn pop_scale(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::PopScale,
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Create a slide-up effect.
    #[must_use]
    pub fn slide_up(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::SlideUp,
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
    pub fn kind(&self) -> ModalEffectKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_scale_initial_state() {
        let effect = ModalEffect::pop_scale(Duration::from_millis(200));
        assert_eq!(effect.kind(), ModalEffectKind::PopScale);
        assert!(!effect.is_finished());
        assert!(effect.progress() < 0.1);
    }

    #[test]
    fn slide_up_initial_state() {
        let effect = ModalEffect::slide_up(Duration::from_millis(300));
        assert_eq!(effect.kind(), ModalEffectKind::SlideUp);
        assert!(!effect.is_finished());
    }

    #[test]
    fn advance_increases_progress() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(200));
        effect.advance(Duration::from_millis(100));
        assert!(!effect.is_finished());
    }

    #[test]
    fn finished_after_duration() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(100));
        effect.advance(Duration::from_millis(150));
        assert!(effect.is_finished());
    }

    #[test]
    fn zero_duration_immediately_finished() {
        let effect = ModalEffect::pop_scale(Duration::ZERO);
        assert!(effect.is_finished());
        assert!((effect.progress() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn progress_clamped_at_one() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(10));
        effect.advance(Duration::from_millis(1000));
        assert!(effect.progress() <= 1.0);
    }
}

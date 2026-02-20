//! Modal animation effects for TUI overlays.

use std::time::Duration;

use super::animation::{AnimPhase, EffectTimer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalEffectKind {
    PopScale,
    SlideUp,
    Shake,
}

#[derive(Debug, Clone)]
pub struct ModalEffect {
    kind: ModalEffectKind,
    timer: EffectTimer,
}

impl ModalEffect {
    #[must_use]
    pub fn pop_scale(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::PopScale,
            timer: EffectTimer::new(duration),
        }
    }

    #[must_use]
    pub fn slide_up(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::SlideUp,
            timer: EffectTimer::new(duration),
        }
    }

    #[must_use]
    pub fn shake(duration: Duration) -> Self {
        Self {
            kind: ModalEffectKind::Shake,
            timer: EffectTimer::new(duration),
        }
    }

    pub fn advance(&mut self, delta: Duration) {
        self.timer.advance(delta);
    }

    #[must_use]
    pub fn phase(&self) -> AnimPhase {
        self.timer.phase()
    }

    #[must_use]
    pub fn kind(&self) -> ModalEffectKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimPhase, ModalEffect, ModalEffectKind};
    use std::time::Duration;

    #[test]
    fn pop_scale_initial_state() {
        let effect = ModalEffect::pop_scale(Duration::from_millis(200));
        assert_eq!(effect.kind(), ModalEffectKind::PopScale);
        assert!(matches!(effect.phase(), AnimPhase::Running { progress } if progress < 0.1));
    }

    #[test]
    fn slide_up_initial_state() {
        let effect = ModalEffect::slide_up(Duration::from_millis(300));
        assert_eq!(effect.kind(), ModalEffectKind::SlideUp);
        assert!(matches!(effect.phase(), AnimPhase::Running { .. }));
    }

    #[test]
    fn shake_initial_state() {
        let effect = ModalEffect::shake(Duration::from_millis(250));
        assert_eq!(effect.kind(), ModalEffectKind::Shake);
        assert!(matches!(effect.phase(), AnimPhase::Running { .. }));
    }

    #[test]
    fn advance_keeps_running() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(200));
        effect.advance(Duration::from_millis(100));
        assert!(matches!(effect.phase(), AnimPhase::Running { .. }));
    }

    #[test]
    fn completed_after_duration() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(100));
        effect.advance(Duration::from_millis(150));
        assert!(matches!(effect.phase(), AnimPhase::Completed));
    }

    #[test]
    fn zero_duration_immediately_completed() {
        let effect = ModalEffect::pop_scale(Duration::ZERO);
        assert!(matches!(effect.phase(), AnimPhase::Completed));
    }

    #[test]
    fn progress_clamped_at_one() {
        let mut effect = ModalEffect::pop_scale(Duration::from_millis(10));
        effect.advance(Duration::from_millis(1000));
        assert!(matches!(effect.phase(), AnimPhase::Completed));
    }
}

//! Panel animation effects for UI components.

use std::time::Duration;

use super::animation::{AnimPhase, EffectTimer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEffectKind {
    SlideInRight,
    SlideOutRight,
}

#[derive(Debug, Clone)]
pub struct PanelEffect {
    kind: PanelEffectKind,
    timer: EffectTimer,
}

impl PanelEffect {
    #[must_use]
    pub fn slide_in_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideInRight,
            timer: EffectTimer::new(duration),
        }
    }

    #[must_use]
    pub fn slide_out_right(duration: Duration) -> Self {
        Self {
            kind: PanelEffectKind::SlideOutRight,
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
    pub fn kind(&self) -> PanelEffectKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimPhase, PanelEffect, PanelEffectKind};
    use std::time::Duration;

    #[test]
    fn slide_out_right_initial_state() {
        let effect = PanelEffect::slide_out_right(Duration::from_millis(150));
        assert_eq!(effect.kind(), PanelEffectKind::SlideOutRight);
        assert!(matches!(effect.phase(), AnimPhase::Running { progress } if progress < 0.1));
    }

    #[test]
    fn slide_in_right_initial_state() {
        let effect = PanelEffect::slide_in_right(Duration::from_millis(150));
        assert_eq!(effect.kind(), PanelEffectKind::SlideInRight);
        assert!(matches!(effect.phase(), AnimPhase::Running { .. }));
    }

    #[test]
    fn completed_and_clamped() {
        let mut effect = PanelEffect::slide_out_right(Duration::from_millis(10));
        effect.advance(Duration::from_millis(50));
        assert!(matches!(effect.phase(), AnimPhase::Completed));
    }
}

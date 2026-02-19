use std::time::Duration;

pub(crate) fn normalized_progress(elapsed: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }

    let elapsed = elapsed.as_secs_f32();
    let total = duration.as_secs_f32();
    (elapsed / total).clamp(0.0, 1.0)
}

#[derive(Debug, Clone)]
pub(crate) struct EffectTimer {
    elapsed: Duration,
    duration: Duration,
}

impl EffectTimer {
    #[must_use]
    pub(crate) fn new(duration: Duration) -> Self {
        Self {
            elapsed: Duration::ZERO,
            duration,
        }
    }

    pub(crate) fn advance(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
    }

    #[must_use]
    pub(crate) fn progress(&self) -> f32 {
        normalized_progress(self.elapsed, self.duration)
    }

    #[must_use]
    pub(crate) fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }
}

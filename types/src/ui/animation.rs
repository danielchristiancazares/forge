use std::time::Duration;

/// Phase of a timed animation effect.
///
/// `Running` embeds the current progress, eliminating separate `progress()`
/// and `is_finished()` queries. Zero-duration timers satisfy `0 >= 0` and
/// return `Completed` immediately, so the division-by-zero guard is
/// eliminated by construction.
#[derive(Debug, Clone, Copy)]
pub enum AnimPhase {
    Running { progress: f32 },
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub(crate) fn phase(&self) -> AnimPhase {
        if self.elapsed >= self.duration {
            AnimPhase::Completed
        } else {
            // Safety: duration > 0 because elapsed >= 0 and elapsed < duration implies duration > 0
            let p = self.elapsed.as_secs_f32() / self.duration.as_secs_f32();
            AnimPhase::Running {
                progress: p.clamp(0.0, 1.0),
            }
        }
    }
}

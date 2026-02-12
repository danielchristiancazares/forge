use std::time::Duration;

pub(crate) fn normalized_progress(elapsed: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }

    let elapsed = elapsed.as_secs_f32();
    let total = duration.as_secs_f32();
    (elapsed / total).clamp(0.0, 1.0)
}

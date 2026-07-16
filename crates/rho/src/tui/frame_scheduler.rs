use std::time::{Duration, Instant};

const BACKGROUND_FRAME_INTERVAL: Duration = Duration::from_millis(24);

/// Coalesces background updates while allowing interaction-driven frames to render immediately.
pub(super) struct FrameScheduler {
    last_rendered_at: Instant,
    deferred_deadline: Option<Instant>,
}

impl FrameScheduler {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_rendered_at: now,
            deferred_deadline: None,
        }
    }

    /// Returns true when a background update should render in the current event-loop tick.
    pub(super) fn request_background_frame(&mut self, now: Instant) -> bool {
        let deadline = self.last_rendered_at + BACKGROUND_FRAME_INTERVAL;
        if now >= deadline {
            true
        } else {
            self.deferred_deadline = Some(
                self.deferred_deadline
                    .map_or(deadline, |pending| pending.min(deadline)),
            );
            false
        }
    }

    pub(super) fn deferred_deadline(&self) -> Option<Instant> {
        self.deferred_deadline
    }

    pub(super) fn rendered(&mut self, now: Instant) {
        self.last_rendered_at = now;
        self.deferred_deadline = None;
    }
}

#[cfg(test)]
#[path = "frame_scheduler_tests.rs"]
mod tests;

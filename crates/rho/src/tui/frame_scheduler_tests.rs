use std::time::{Duration, Instant};

use super::FrameScheduler;

#[test]
fn background_frames_are_deferred_until_the_frame_budget_is_available() {
    let started_at = Instant::now();
    let mut scheduler = FrameScheduler::new(started_at);

    assert!(!scheduler.request_background_frame(started_at + Duration::from_millis(1)));
    let deadline = scheduler
        .deferred_deadline()
        .expect("background frame should be scheduled");
    assert!(deadline > started_at + Duration::from_millis(1));
    assert!(scheduler.request_background_frame(deadline));
}

#[test]
fn rendering_clears_deferred_background_work() {
    let started_at = Instant::now();
    let mut scheduler = FrameScheduler::new(started_at);
    assert!(!scheduler.request_background_frame(started_at + Duration::from_millis(1)));

    scheduler.rendered(started_at + Duration::from_millis(2));

    assert_eq!(scheduler.deferred_deadline(), None);
    assert!(!scheduler.request_background_frame(started_at + Duration::from_millis(3)));
}

//! A frame another GPU had not finished is retried one refresh later, still damaged, and is no failure.

use std::time::Duration;

use cupertino_server::schedule::FrameScheduler;

const REFRESH: Duration = Duration::from_micros(16_667);

#[test]
fn a_frame_not_ready_is_retried_a_refresh_later() {
    let mut scheduler = FrameScheduler::default();
    let now = Duration::from_secs(10);
    scheduler.called_back();
    scheduler.not_ready(now, REFRESH);
    assert!(scheduler.damaged());
    assert!(!scheduler.retry_due(now));
    assert!(!scheduler.due(now, false));
    assert!(scheduler.retry_due(now + REFRESH));
    assert!(scheduler.due(now + REFRESH, false));
    assert_eq!(scheduler.deadline(now, false), Some(now + REFRESH));
    assert_eq!(scheduler.drawn(), None, "a late frame is no failure");
}

#[test]
fn a_frame_not_ready_keeps_the_count_of_failures_before() {
    let mut scheduler = FrameScheduler::default();
    let now = Duration::from_secs(10);
    scheduler.failed(now, REFRESH);
    scheduler.not_ready(now + REFRESH, REFRESH);
    assert!(!scheduler.retry_due(now + REFRESH));
    assert_eq!(scheduler.drawn(), Some(1));
}

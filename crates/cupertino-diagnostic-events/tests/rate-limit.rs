//! The rate limit: so many records of one id per window, and one summary of the rest.

#[allow(dead_code)]
mod support;

use std::time::{Duration, Instant};

use cupertino_diagnostic_events::rate::RateLimiter;
use cupertino_diagnostic_events::{self as diag, Record, Severity, Value};

fn record(id: &'static str, n: u64) -> Record {
    diag::event(Severity::Warn, id, "burst")
        .field("n", Value::Count(n))
        .to_record(&support::policy(), &support::origin(), n)
}

#[test]
fn an_id_passes_its_quota_per_window_and_the_rest_become_one_summary_of_the_last() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut rate = RateLimiter::default();
    let mut written = Vec::new();
    for n in 1..=diag::RATE_PER_ID as u64 + 1 {
        written.extend(rate.offer(record("test.rate.a", n), at(n * 100)));
    }
    assert_eq!(written.len(), diag::RATE_PER_ID);
    assert!(written.iter().all(|r| r.suppressed == 0));
    // the summary is due when the window that began with the first record closes, not before
    let closes = at(100) + diag::RATE_WINDOW;
    assert_eq!(rate.next_due(), Some(closes));
    assert!(rate.due(closes - Duration::from_millis(1)).is_empty());
    let summary = rate.due(closes);
    let last = diag::RATE_PER_ID as u64 + 1;
    assert_eq!(
        summary
            .iter()
            .map(|r| (r.count, r.suppressed, r.fields.get("n").cloned()))
            .collect::<Vec<_>>(),
        [(last, 1, Some(last.into()))]
    );
    assert!(rate.due(closes + Duration::from_secs(60)).is_empty());
    // another id has a window of its own, and a summary still held at exit is drained
    let later = closes + Duration::from_secs(1);
    assert_eq!(rate.offer(record("test.rate.a", 20), later).len(), 1);
    for n in 0..diag::RATE_PER_ID as u64 + 2 {
        rate.offer(record("test.rate.b", n), later);
    }
    let drained: Vec<_> = rate.drain().iter().map(|r| r.suppressed).collect();
    assert_eq!(drained, [2]);
}

#[test]
fn a_held_summary_is_written_before_the_record_that_opens_the_next_window() {
    let start = Instant::now();
    let mut rate = RateLimiter::new(1, Duration::from_secs(10));
    assert_eq!(rate.offer(record("test.rate.next", 1), start).len(), 1);
    assert!(rate
        .offer(record("test.rate.next", 2), start + Duration::from_secs(1))
        .is_empty());
    let out = rate.offer(record("test.rate.next", 3), start + Duration::from_secs(11));
    assert_eq!(
        out.iter()
            .map(|r| (r.count, r.suppressed))
            .collect::<Vec<_>>(),
        [(2, 1), (3, 0)]
    );
}

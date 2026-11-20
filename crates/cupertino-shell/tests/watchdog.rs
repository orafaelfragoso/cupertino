//! What the watchdog makes of a loop, or of the render thread, out of its wait (D17).

use std::time::Duration;

use cupertino_kit_shell::hosting::Liveness;
use cupertino_shell::watchdog::{liveness, render_hung};

const fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn a_loop_out_of_its_wait_is_reported_by_how_long_after_the_start_up_grace() {
    let after_grace = Duration::from_secs(30);
    let rows = [
        (None, after_grace, Liveness::Responsive),
        (Some(ms(100)), after_grace, Liveness::Responsive),
        (Some(ms(500)), after_grace, Liveness::Busy),
        (Some(ms(2_000)), after_grace, Liveness::Unresponsive),
        (Some(ms(10_000)), after_grace, Liveness::Lost),
        // its start is never reported, however long it takes
        (
            Some(ms(9_000)),
            Duration::from_secs(9),
            Liveness::Responsive,
        ),
    ];
    for (busy, running, expected) in rows {
        assert_eq!(
            liveness(busy, running),
            expected,
            "{busy:?} after {running:?}"
        );
    }
}

#[test]
fn a_render_thread_out_of_its_wait_for_ten_seconds_hangs_the_host() {
    let after_grace = Duration::from_secs(30);
    assert!(!render_hung(None, after_grace));
    assert!(!render_hung(Some(ms(9_000)), after_grace));
    assert!(render_hung(Some(ms(10_000)), after_grace));
    assert!(!render_hung(Some(ms(10_000)), Duration::from_secs(5)));
}

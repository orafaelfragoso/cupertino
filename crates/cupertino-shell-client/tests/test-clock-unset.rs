//! The display clock without `CUPERTINO_TEST_CLOCK`: the real wall and monotonic clocks, and motion that is not settled.

use cupertino_shell_client::test_clock::{display_now, display_unix_seconds, motion_settled};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn without_the_variable_the_clocks_are_the_real_ones() {
    std::env::remove_var("CUPERTINO_TEST_CLOCK");
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let drawn = display_unix_seconds().unwrap();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(before <= drawn && drawn <= after);
    assert!(!motion_settled());

    let first = display_now();
    std::thread::sleep(Duration::from_millis(2));
    assert!(display_now() > first);
    assert!(display_now() <= Instant::now());
}

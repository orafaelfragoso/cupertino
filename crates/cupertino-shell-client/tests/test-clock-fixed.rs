//! The display clock with `CUPERTINO_TEST_CLOCK` set to a number. The override is read once per process and exists only
//! in a debug build, so this binary holds the one case; the others are in `test-clock-invalid.rs` and
//! `test-clock-unset.rs`.

use cupertino_shell_client::test_clock::{display_now, display_unix_seconds, motion_settled};

#[test]
fn a_fixed_clock_stops_the_drawn_time_and_settles_motion() {
    std::env::set_var("CUPERTINO_TEST_CLOCK", "1700000123");
    if cfg!(debug_assertions) {
        assert_eq!(display_unix_seconds().unwrap(), 1_700_000_123);
        assert!(motion_settled());
        let first = display_now();
        assert_eq!(display_now(), first);
        // reading the variable again does not move a clock already read
        std::env::set_var("CUPERTINO_TEST_CLOCK", "5");
        assert_eq!(display_unix_seconds().unwrap(), 1_700_000_123);
    } else {
        // a release build neither reads the variable nor settles motion
        assert!(!motion_settled());
        assert_ne!(display_unix_seconds().unwrap(), 1_700_000_123);
    }
}

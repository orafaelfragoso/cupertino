//! The display clock with `CUPERTINO_TEST_CLOCK` set to something that is not a decimal number of seconds: no override.
//! The variable is read once per process, so this binary holds the one case.

use cupertino_shell_client::test_clock::{display_now, display_unix_seconds, motion_settled};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[test]
fn a_value_that_is_not_a_decimal_number_is_no_override() {
    std::env::set_var("CUPERTINO_TEST_CLOCK", "12:30");
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
    let first = Instant::now();
    let now = display_now();
    assert!(now >= first);
}

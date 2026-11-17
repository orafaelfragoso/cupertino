//! A process setting how long a reading or a switch waits.
//!
//! The wait is the process's, so this file has one test and the default has a file of its own.

#![cfg(feature = "dbus")]

use cupertino_radios::{call_timeout, set_call_timeout};
use std::time::Duration;

#[test]
fn a_process_sets_the_wait_of_its_readings_within_what_can_be_counted() {
    for (set, expected) in [
        (Duration::from_millis(250), Duration::from_millis(250)),
        (Duration::from_secs(10), Duration::from_secs(10)),
        (Duration::ZERO, Duration::from_millis(1)),
        (Duration::from_micros(400), Duration::from_millis(1)),
        (Duration::MAX, Duration::from_millis(u64::MAX)),
    ] {
        set_call_timeout(set);
        assert_eq!(call_timeout(), expected, "{set:?}");
    }
}

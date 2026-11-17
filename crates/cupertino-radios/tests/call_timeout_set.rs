//! A process setting how long a reading or a switch waits.
//!
//! The wait is the process's, so the tests of this file take turns.

#![cfg(feature = "dbus")]

use cupertino_radios::{call_timeout, set_call_timeout};
use std::sync::Mutex;
use std::time::Duration;

static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_process_can_set_the_wait_of_its_readings() {
    let _turn = turn();
    set_call_timeout(Duration::from_millis(250));
    assert_eq!(call_timeout(), Duration::from_millis(250));
    set_call_timeout(Duration::from_secs(10));
    assert_eq!(call_timeout(), Duration::from_secs(10));
}

#[test]
fn a_wait_of_nothing_is_one_millisecond() {
    let _turn = turn();
    set_call_timeout(Duration::ZERO);
    assert_eq!(call_timeout(), Duration::from_millis(1));
    set_call_timeout(Duration::from_micros(400));
    assert_eq!(call_timeout(), Duration::from_millis(1));
}

#[test]
fn a_wait_too_long_to_count_is_the_longest_there_is() {
    let _turn = turn();
    set_call_timeout(Duration::MAX);
    assert_eq!(call_timeout(), Duration::from_millis(u64::MAX));
}

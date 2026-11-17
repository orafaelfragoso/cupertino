//! The wait of a reading before any process sets another one.

#![cfg(feature = "dbus")]

use cupertino_radios::{call_timeout, CALL_TIMEOUT};

#[test]
fn a_reading_waits_for_the_default_until_a_process_sets_another() {
    assert_eq!(call_timeout(), CALL_TIMEOUT);
}

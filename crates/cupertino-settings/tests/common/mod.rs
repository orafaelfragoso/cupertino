//! A private bus for the integration tests (`dbus-daemon --session`, the daemons' pattern); a test is skipped when the
//! daemon is not installed. The fake services' tests use one as their "system" bus.

pub use cupertino_test_bus::PrivateBus;

/// The private bus, or `None` with a note when `dbus-daemon` is missing.
pub fn bus() -> Option<PrivateBus> {
    let bus = PrivateBus::start();
    if bus.is_none() {
        eprintln!("skipped: dbus-daemon is not available");
    }
    bus
}

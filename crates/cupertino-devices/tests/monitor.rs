//! The hotplug monitor's socket, with no kernel event needed.

use std::os::fd::AsFd;

use cupertino_devices::Monitor;

#[test]
fn a_monitor_hands_a_non_blocking_socket_to_an_event_loop_and_drains_nothing_when_quiet() {
    let monitor = Monitor::new().expect("a uevent socket opens without privilege");
    let flags = rustix::fs::fcntl_getfl(monitor.as_fd()).unwrap();
    assert!(flags.contains(rustix::fs::OFlags::NONBLOCK));
    let drained = monitor.drain().unwrap();
    assert!(drained.errors.is_empty());
    assert!(!drained.overflowed);
    // draining again does not block
    monitor.drain().unwrap();
}

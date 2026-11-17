//! The hotplug monitor's socket, with no kernel event needed.

use std::os::fd::AsFd;

use cupertino_devices::Monitor;

#[test]
fn a_monitor_with_nothing_queued_drains_to_nothing() {
    let monitor = Monitor::new().expect("a uevent socket opens without privilege");
    let drained = monitor.drain().unwrap();
    assert!(drained.errors.is_empty());
    assert!(!drained.overflowed);
    // draining again does not block
    monitor.drain().unwrap();
}

#[test]
fn the_monitor_hands_its_socket_to_an_event_loop_non_blocking() {
    let monitor = Monitor::new().unwrap();
    let flags = rustix::fs::fcntl_getfl(monitor.as_fd()).unwrap();
    assert!(flags.contains(rustix::fs::OFlags::NONBLOCK));
}

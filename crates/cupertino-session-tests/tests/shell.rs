//! `cupertino-shell`'s host program (`cupertino_shell::run`) against the real server, headless
//! (`docs/architecture/shell-host.md`). The server's supervisor starts this test binary as `cupertino-shell`: with
//! [`host::MARK`] among its arguments it runs the host with the components of `shell/components.rs`.
//! - A component that panics as it handles a message, panics again in its teardown, and then panics on a helper
//!   thread is started again each time, alone: the host process and the other component, which keeps drawing, go on.
//! - A component that fails five times within a minute is given up by the server, and the other components hear it
//!   from the host on the host bus.
//! - A component whose loop hangs past the start-up grace is reported busy, unresponsive and lost; the shared host
//!   then asks to be started again, and the server starts it again at its quiet moment.
//! - A restart the person asks for stops the component and starts it again.

#[path = "shell/components.rs"]
mod components;
#[path = "shell/host.rs"]
mod host;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::HashSet;

use cupertino_server::host::{HostKind, Liveness};

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host itself, when the supervisor started this binary with [`host::MARK`]; a test run does nothing here.
#[test]
fn host_program() {
    if let Some((report, args)) = host::arguments() {
        host::run(report, args);
    }
}

/// The area the dock covers: the output's bottom left.
fn dock_area() -> Area {
    let (w, h) = (DOCK_SIZE.width as i32, DOCK_SIZE.height as i32);
    Area::new(0, OUTPUT_SIZE.1 - h, w, h)
}

/// The colours of [`components::Dock`] its area shows now.
fn dock_colour(rig: &mut Rig) -> Option<&'static str> {
    let image = rig.image();
    let area = dock_area();
    let shows = |colour: cupertino_kit::theme::Color| {
        let want = [colour.r, colour.g, colour.b].map(|c| (c * 255.0) as i32);
        count(&image, area, |rgb| {
            (0..3).all(|c| (i32::from(rgb[c]) - want[c]).abs() < 40)
        }) >= area.covered()
    };
    if shows(RED) {
        Some("red")
    } else if shows(BLUE) {
        Some("blue")
    } else {
        None
    }
}

#[test]
fn a_failed_component_restarts_alone_while_another_keeps_drawing() {
    let mut rig = Rig::new("restarts", &["dock", "notifications"], &[]);
    rig.until("the dock draws", |rig| dock_colour(rig).is_some());
    let first = rig
        .host_pid(HostKind::Shared)
        .expect("the shared host runs");

    // the first instance panics as it handles a message and again in its teardown, the second on a helper thread
    let mut colours = HashSet::new();
    rig.until("the third instance of the notifications runs", |rig| {
        if rig.reported("notifications panics") {
            colours.extend(dock_colour(rig));
        }
        rig.reported("notifications steady")
            && rig.state.hosts.component_instance("notifications") == 3
    });
    assert!(
        rig.reported("notifications teardown panics"),
        "{:?}",
        rig.lines()
    );
    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(first),
        "the host went on"
    );
    assert_eq!(
        rig.state.hosts.component_instance("dock"),
        1,
        "the dock was never restarted"
    );
    assert_eq!(
        colours,
        HashSet::from(["red", "blue"]),
        "the dock kept drawing while the notifications started again"
    );

    // a restart the person asks for is no failure: the component stops and starts again
    let rx = rig.restart("dock");
    rig.answer(&rx)
        .expect("the restarted dock says it is ready");
    assert_eq!(rig.state.hosts.component_instance("dock"), 2);
    assert_eq!(rig.host_pid(HostKind::Shared), Some(first));
}

#[test]
fn a_component_that_fails_five_times_is_given_up_and_the_others_hear_it() {
    let mut rig = Rig::new("given-up", &["dock", "controlcenter"], &[]);
    rig.until("the server gave the control panel up", |rig| {
        rig.state.hosts.component_gave_up("controlcenter")
    });
    rig.until_reported("dock heard host ComponentStopped controlcenter");
    let starts = rig
        .lines()
        .iter()
        .filter(|l| *l == "controlcenter started")
        .count();
    assert_eq!(starts, 5, "{:?}", rig.lines());
    assert_eq!(rig.state.hosts.component_instance("dock"), 1);
}

#[test]
fn a_hung_component_is_reported_lost_and_its_host_starts_again() {
    let mut rig = Rig::new("hang", &["dock", "launcher"], &[]);
    rig.until_reported("launcher started");
    let first = rig
        .host_pid(HostKind::Shared)
        .expect("the shared host runs");
    let mut seen = Vec::new();
    rig.until("the shared host was started again", |rig| {
        let now = rig.state.hosts.component_liveness("launcher");
        if seen.last() != Some(&now) {
            seen.push(now);
        }
        rig.host_pid(HostKind::Shared)
            .is_some_and(|pid| pid != first)
    });
    assert!(rig.reported("launcher hangs"));
    assert!(
        seen.contains(&Liveness::Busy) && seen.contains(&Liveness::Unresponsive),
        "the server saw the hang grow: {seen:?}"
    );
    rig.until("the dock runs in the new host", |rig| {
        rig.state.hosts.component_connected("dock")
            && rig.lines().iter().filter(|l| *l == "dock started").count() == 2
    });
}

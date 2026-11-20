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
//! - A component the server runs alone runs in a separate host, started with `--only` and its name.
//! - A component of this development build asked to crash on its bus connection
//!   (`org.cupertino.ShellComponentDebug1`) by `cupertino shell crash` panics as its loop dispatches, as it paints
//!   and on a helper thread, and is started again alone each time.

#[path = "shell/components.rs"]
mod components;
#[path = "shell/host.rs"]
mod host;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};

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
    // the lost component runs alone from then on
    rig.until_reported("launcher runs again");
    let separate = rig
        .host_pid(HostKind::Separate("launcher"))
        .expect("a separate host runs the launcher");
    assert!(
        arguments(separate).ends_with(&["--only".into(), "launcher".into()]),
        "{:?}",
        arguments(separate)
    );
    assert!(!rig
        .host(HostKind::Shared)
        .unwrap()
        .components
        .contains(&"launcher"));
}

/// The arguments of process `pid`.
fn arguments(pid: u32) -> Vec<String> {
    let args = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    args.split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

#[test]
fn a_component_the_server_runs_alone_runs_in_a_host_started_with_only() {
    let mut rig = Rig::new("alone", &["dock", "notifications"], &["dock"]);
    rig.until("the dock draws", |rig| dock_colour(rig).is_some());
    let separate = rig
        .host_pid(HostKind::Separate("dock"))
        .expect("a separate host runs the dock");
    assert!(arguments(separate).ends_with(&["--only".into(), "dock".into()]));
    assert_eq!(
        rig.host(HostKind::Shared).unwrap().components,
        vec!["notifications"]
    );
    // the notifications fail in the shared host; the dock in its own host never notices
    rig.until_reported("notifications steady");
    assert_eq!(rig.host_pid(HostKind::Separate("dock")), Some(separate));
    assert_eq!(rig.state.hosts.component_instance("dock"), 1);
}

/// A session bus of the test's own, whose address the hosts and the command inherit, and the address.
struct Bus(Child, String);

impl Bus {
    fn start() -> Bus {
        let mut daemon = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon gives its address");
        let address = address.trim().to_owned();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The unique bus name instance `instance` of the menu bar reported.
fn bar_bus(rig: &mut Rig, instance: u32) -> String {
    let prefix = format!("menubar {instance} bus ");
    rig.until(
        &format!("the menu bar's instance {instance} is on the bus"),
        |rig| rig.lines().iter().any(|l| l.starts_with(&prefix)),
    );
    let line = rig
        .lines()
        .into_iter()
        .find(|l| l.starts_with(&prefix))
        .unwrap();
    line[prefix.len()..].to_owned()
}

/// `cupertino shell crash menubar --at AT`, run while the server turns.
fn crash_command(rig: &mut Rig, at: &str) -> cupertino_cli::Output {
    let args: Vec<String> = ["shell", "crash", "menubar", "--at", at, "--format", "json"]
        .map(String::from)
        .into();
    let command =
        std::thread::spawn(move || cupertino_cli::run(&args, &cupertino_cli::request_id()));
    rig.until(&format!("cupertino shell crash answers ({at})"), |_| {
        command.is_finished()
    });
    command.join().unwrap()
}

#[test]
fn a_crash_asked_for_on_a_components_connection_restarts_it_alone() {
    let bus = Bus::start();
    let mut rig = Rig::new("crash", &["menubar", "dock"], &[]);
    // the server answers which component a bus connection is, as in a session
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    rig.until("the dock draws", |rig| dock_colour(rig).is_some());
    let host = rig.host_pid(HostKind::Shared).unwrap();
    for (at, instance) in [("dispatch", 1), ("paint", 2), ("helper", 3)] {
        let name = bar_bus(&mut rig, instance);
        assert_ne!(name, "none", "the menu bar has a bus connection");
        let out = crash_command(&mut rig, at);
        assert_eq!(out.code, 0, "{} {}", out.stdout, out.stderr);
        assert!(
            out.stdout.contains(&name),
            "the crash went to {name}: {}",
            out.stdout
        );
        rig.until(
            &format!("the menu bar starts again after a crash at {at}"),
            |rig| {
                rig.state.hosts.component_instance("menubar") == instance + 1
                    && rig.state.hosts.component_connected("menubar")
            },
        );
    }
    bar_bus(&mut rig, 4);
    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(host),
        "the host went on"
    );
    assert_eq!(rig.state.hosts.component_instance("dock"), 1);
    assert!(!rig.state.hosts.component_gave_up("menubar"));

    // a component that runs nowhere is no crash
    let args: Vec<String> = ["shell", "crash", "launcher"].map(String::from).into();
    let command =
        std::thread::spawn(move || cupertino_cli::run(&args, &cupertino_cli::request_id()));
    rig.until("cupertino shell crash answers (launcher)", |_| {
        command.is_finished()
    });
    let out = command.join().unwrap();
    assert_eq!(out.code, 2, "{} {}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains("no component named \"launcher\" runs"),
        "{}",
        out.stderr
    );
}

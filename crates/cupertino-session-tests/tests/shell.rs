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
//! - A component turned off and on again before the instance the host stops has ended starts again: the last list
//!   wins.
//! - A component of this development build asked to crash on its bus connection
//!   (`org.cupertino.ShellComponentDebug1`) by `cupertino shell crash` panics as its loop dispatches, as it paints
//!   and on a helper thread, and is started again alone each time.
//! - The on-screen displays (`cupertino-osd`) own `org.cupertino.Osd1` on their own connection and draw what `Show`
//!   asks; a panic injected as they paint, as their loop dispatches and on a helper thread restarts them alone, fifty
//!   times, while the dock keeps drawing and the host's anonymous memory stays flat; a caller other than the server cannot step
//!   a level.
//! - A media key the server forwards steps the volume through `org.cupertino.Audio1` and the displays show it; the
//!   server sends it from the connection that owns `org.cupertino.Session1`, and another connection of the server's
//!   process is refused like any other caller.

#[path = "shell/components.rs"]
mod components;
#[path = "shell/host.rs"]
mod host;
#[path = "shell/memory.rs"]
mod memory;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cupertino_server::host::{HostKind, Liveness};

use components::{BLUE, DOCK_SIZE, RED};
use memory::{steady_allocator, HostMemory, CRASHES_IN_A_ROW, RESTARTS};
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

#[test]
fn a_component_turned_off_and_on_again_while_it_stops_starts_again() {
    let mut rig = Rig::new("relisted", &["desktop", "dock"], &[]);
    rig.until("the dock draws and the desktop runs", |rig| {
        dock_colour(rig).is_some() && rig.state.hosts.component_connected("desktop")
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();

    // off and on again within one turn: the host reads both lists while the desktop it stops tears down, and the last
    // list wins once it ended
    rig.state.config.shell_off = vec!["desktop".into()];
    rig.state.apply_shell_plan();
    rig.state.config.shell_off.clear();
    rig.state.apply_shell_plan();
    rig.until("the desktop runs again", |rig| {
        rig.state.hosts.component_connected("desktop")
            && rig
                .lines()
                .iter()
                .filter(|l| *l == "desktop started")
                .count()
                == 2
    });
    let lines = rig.lines();
    let ended = lines.iter().position(|l| l == "desktop ended");
    let again = lines.iter().rposition(|l| l == "desktop started");
    assert!(
        ended.is_some() && again.is_some() && ended < again,
        "one instance at a time: {lines:?}"
    );
    assert_eq!(
        rig.state.hosts.component_instance("desktop"),
        2,
        "the second instance has the first connection the host asked for after the lists"
    );
    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(host),
        "the host went on"
    );
    assert_eq!(
        rig.state.hosts.component_instance("dock"),
        1,
        "the dock was never restarted"
    );
}

/// A session bus of the test's own, whose address the hosts and the command inherit, and the address.
struct Bus(Child, String);

/// A session bus that starts no service: nothing the components call reaches a program of the machine.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

impl Bus {
    fn start() -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-bus.conf");
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
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

/// The area of the on-screen displays: their window, centred at the bottom, `BOTTOM_MARGIN` above the edge.
fn osd_area() -> Area {
    let side = cupertino_osd::layout::WINDOW_SIZE as i32;
    Area::new(
        (OUTPUT_SIZE.0 - side) / 2,
        OUTPUT_SIZE.1 - cupertino_osd::layout::BOTTOM_MARGIN - side,
        side,
        side,
    )
}

/// How many pixels of `area` differ between `a` and `b`.
fn changed(a: &image::RgbaImage, b: &image::RgbaImage, area: Area) -> usize {
    let mut n = 0;
    for y in area.y..area.y + area.h {
        for x in area.x..area.x + area.w {
            let (p, q) = (
                a.get_pixel(x as u32, y as u32),
                b.get_pixel(x as u32, y as u32),
            );
            if p.0[..3] != q.0[..3] {
                n += 1;
            }
        }
    }
    n
}

/// The connection that owns `org.cupertino.Osd1` now.
fn osd_owner(session: &zbus::blocking::Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(cupertino_osd::control::BUS_NAME).ok()?;
    dbus.get_name_owner(name)
        .ok()
        .map(|owner| owner.to_string())
}

/// Calls `method` of `org.cupertino.Osd1` with `body`.
fn osd_call<B>(session: &zbus::blocking::Connection, method: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    session
        .call_method(
            Some(cupertino_osd::control::BUS_NAME),
            cupertino_osd::control::OBJECT_PATH,
            Some(cupertino_osd::control::BUS_NAME),
            method,
            body,
        )
        .map(drop)
}

#[test]
fn the_on_screen_displays_restart_alone_after_injected_panics_and_the_host_stays_flat() {
    let bus = Bus::start();
    steady_allocator();
    let mut rig = Rig::new("osd", &["dock", "osd"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = zbus::blocking::Connection::session().expect("the test's bus");
    rig.until("the dock draws and the displays own their name", |rig| {
        dock_colour(rig).is_some() && osd_owner(&session).is_some()
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();

    // Show draws the overlay
    let before = rig.image();
    osd_call(&session, "Show", &("volume", 0.75)).expect("Show is open to every caller");
    rig.until("the overlay shows", |rig| {
        changed(&before, &rig.image(), osd_area()) > osd_area().covered() / 2
    });

    // a caller that is not the server steps nothing; the server answers who it is as its loop turns
    let mut refused = Process::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.cupertino.Osd1",
            "/org/cupertino/Osd1",
            "org.cupertino.Osd1.VolumeStep",
            "int32:1",
            "boolean:false",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dbus-send runs");
    let mut status = None;
    rig.until("the step is answered", |_| {
        status = refused.try_wait().unwrap();
        status.is_some()
    });
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut refused.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(!status.unwrap().success());
    assert!(stderr.contains("AccessDenied"), "{stderr}");

    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance("osd");
    let mut host_memory = HostMemory::of(host);
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let mut owner = None;
        rig.until("the displays own their name", |_| {
            owner = osd_owner(&session);
            owner.is_some()
        });
        let owner = owner.unwrap();
        session
            .call_method(
                Some(owner.as_str()),
                "/org/cupertino/ShellComponentDebug1",
                Some("org.cupertino.ShellComponentDebug1"),
                "Crash",
                &(at,),
            )
            .unwrap_or_else(|e| panic!("the displays take a crash at {at}: {e}"));
        if at == "paint" {
            // what they draw next panics. Something else may have them draw first, and then their name is already gone
            // with them: the crash is taken all the same
            let _ = osd_call(&session, "Show", &("brightness", 0.5));
        }
        if (crash + 1) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the displays up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("osd")
                },
            );
            let rx = rig.restart("osd");
            rig.answer(&rx).expect("the displays start again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the displays start again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("osd") == instance
                    && rig.state.hosts.component_connected("osd")
            },
        );
        host_memory.restarted(crash + 1);
    }
    rig.until("the displays own their name again", |_| {
        osd_owner(&session).is_some()
    });

    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(host),
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
        "the dock kept drawing while the displays started again"
    );
    assert!(!rig.state.hosts.component_gave_up("osd"));
    host_memory.assert_nothing_kept(&mut rig);

    // and the restarted displays draw
    let before = rig.image();
    osd_call(&session, "Show", &("mute", 0.0)).expect("Show");
    rig.until("the restarted overlay shows", |rig| {
        changed(&before, &rig.image(), osd_area()) > osd_area().covered() / 2
    });
}

/// A stand-in for `cupertino-audiod`'s `org.cupertino.Audio1`: one volume for both directions, and how many times it
/// was set.
struct Audio {
    level: Arc<Mutex<f64>>,
    sets: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.cupertino.Audio1")]
impl Audio {
    fn get_volume(&self, _direction: &str) -> (f64, bool) {
        (*self.level.lock().unwrap(), false)
    }

    fn set_volume(&self, _direction: &str, level: f64, muted: bool) -> (f64, bool) {
        *self.level.lock().unwrap() = level;
        self.sets.fetch_add(1, Ordering::SeqCst);
        (level, muted)
    }
}

/// The output volume the stand-in sound service starts at.
const START_VOLUME: f64 = 0.25;

#[test]
fn a_media_key_from_the_server_steps_the_volume_and_another_connection_of_its_process_cannot() {
    let bus = Bus::start();
    let level = Arc::new(Mutex::new(START_VOLUME));
    let sets = Arc::new(AtomicUsize::new(0));
    let _audiod = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.cupertino.Audio1"))
        .and_then(|b| {
            b.serve_at(
                "/org/cupertino/Audio1",
                Audio {
                    level: level.clone(),
                    sets: sets.clone(),
                },
            )
        })
        .and_then(|b| b.build())
        .expect("the stand-in sound service owns its name");
    let mut rig = Rig::new("osd-step", &["dock", "osd"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = zbus::blocking::Connection::session().expect("the test's bus");
    rig.until(
        "the dock draws, the displays own their name and the server its own",
        |rig| {
            dock_colour(rig).is_some()
                && osd_owner(&session).is_some()
                && rig.state.quit.session_connection().is_some()
        },
    );

    // this test runs in the server's process: a connection of its own is not the one that owns Session1
    let before = rig.image();
    let other = std::thread::spawn(|| {
        let connection = zbus::blocking::Connection::session().expect("another connection");
        osd_call(&connection, "VolumeStep", &(1i32, false))
    });
    rig.until("the step from another connection is answered", |_| {
        other.is_finished()
    });
    let refused = other.join().unwrap().expect_err("the step is refused");
    assert!(
        matches!(&refused, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied"),
        "{refused}"
    );
    assert_eq!(sets.load(Ordering::SeqCst), 0, "the volume did not change");
    assert_eq!(
        changed(&before, &rig.image(), osd_area()),
        0,
        "nothing is shown"
    );

    // the server's media key steps the volume and the displays show it
    rig.state
        .osd_key(cupertino_server::input::OsdKey::VolumeUp, false);
    rig.until("the server's step is shown", |rig| {
        sets.load(Ordering::SeqCst) == 1
            && changed(&before, &rig.image(), osd_area()) > osd_area().covered() / 2
    });
    assert!(
        *level.lock().unwrap() > START_VOLUME,
        "the volume went up from {START_VOLUME} to {}",
        level.lock().unwrap()
    );
}

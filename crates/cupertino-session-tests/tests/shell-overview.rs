//! The overview (`cupertino-shell-overview`) as `cupertino-shell` runs it, against the real server, headless, on a
//! session bus of the test's own and with settings and data directories of its own (`docs/architecture/shell-host.md`,
//! D16). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its arguments it
//! runs the host with the test dock of `shell/components.rs`, the real overview (`cupertino_shell::OVERVIEW`) and the
//! real menu bar (`cupertino_shell::MENUBAR`), each test starting the ones it needs.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat.
//! - The swipe up is armed for the overview: released slowly short of half its travel it settles back and gives the
//!   window back; released at the same place in a quick flick, the release's velocity carries it open, and the window
//!   stays in its hole.
//! - The overview a swipe up opened keeps the swipe down once at rest, and the swipe down takes it back.
//! - A window that closes while the overview is open has its hole closed: the claim ends and nothing of it shows.
//! - The toggle (the shortcut, F3) opens the overview and the next one leaves it, giving the window back, more times
//!   than a connection may have transactions, with the same instance.
//! - Going from one application's window to another's arms the gestures anew each time, and never opens the overview.
//! - The menu bar gives way to the overview: it fades out and takes no input while the toggle's overview is open, and
//!   comes back when the toggle or Escape leaves it, when a swipe up that began to open it settles back, and when the
//!   overview fails while it is open; its space stays reserved all the while, so the window keeps its frame.

#[path = "shell-host/app.rs"]
mod app;
#[path = "shell/components.rs"]
#[allow(dead_code)]
mod components;
#[path = "shell/memory.rs"]
mod memory;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::net::Shutdown;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};

use cupertino_input::gesture::GestureEvent;
use cupertino_protocols::server::transaction::cupertino_transaction_v1::Gesture as WireGesture;
use cupertino_server::host::HostKind;
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED};
use memory::{steady_allocator, HostMemory, CRASHES_IN_A_ROW, RESTARTS};
use rig::{count, magenta, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock, the real
/// overview and the real menu bar.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-overview-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 3] = [&DOCK, &cupertino_shell::OVERVIEW, &cupertino_shell::MENUBAR];

    /// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
    pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let at = args.iter().position(|a| a == MARK)?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((report, args[at + 2..].to_vec()))
    }

    /// Be the host, the dock reporting to `report`.
    pub fn run(report: PathBuf, args: Vec<String>) -> ! {
        report_to(report);
        cupertino_shell::run(&ENTRIES, args)
    }
}

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

/// The colour of [`components::Dock`] its area shows now.
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

/// A session bus of the test's own, whose address the hosts inherit, and the address.
struct Bus(Child, String);

/// A session bus that starts no service: nothing the component calls reaches a program of the machine.
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
    /// The bus, for the session and the system alike: the radios, the battery and the session agent the bar reads
    /// are not on it, so it shows none of their items.
    fn start() -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-overview-bus.conf");
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
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700: the session's settings or data.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-overview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// `work` on a thread of its own while the server turns, so a call that waits for the server is answered.
fn aside<T: Send + 'static>(
    rig: &mut Rig,
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let work = std::thread::spawn(work);
    rig.until(what, |_| work.is_finished());
    work.join().unwrap()
}

/// How long a question to the overview's connection waits for its answer: a connection that serves no object yet
/// leaves a call unanswered, so it is asked again.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether the connection `name` serves the components' debug interface yet: the server knows a component's
/// connection as soon as its name is proved, before the component serves anything on it, and a call made before
/// that is never answered. `session` gives up on a call after [`PROBE_TIMEOUT`].
fn serves_crash(rig: &mut Rig, session: &Connection, name: &str) -> bool {
    let (session, name) = (session.clone(), name.to_owned());
    aside(rig, "the overview answers", move || {
        session
            .call_method(
                Some(name.as_str()),
                "/org/cupertino/ShellComponentDebug1",
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            )
            .ok()
            .and_then(|reply| reply.body().deserialize::<String>().ok())
            .is_some_and(|xml| xml.contains("org.cupertino.ShellComponentDebug1"))
    })
}

/// The overview's own connection to the session bus, once the server knows it as the overview's: one not among
/// `gone`, the connections of instances that failed, which the bus may still list for a while.
fn overview_name(rig: &Rig, session: &Connection, gone: &HashSet<String>) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    dbus.list_names()
        .ok()?
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with(':') && !gone.contains(name))
        .find(|name| rig.state.hosts.component_of_bus_name(name) == Some("overview"))
}

/// A session bus, settings and data of the test's own, and the server with `components` serving the session on it.
fn start(name: &str, components: &[&str]) -> (Bus, Rig) {
    let bus = Bus::start();
    // no dock.conf of the person's: the swipe up opens the overview, as by default
    std::env::set_var("XDG_CONFIG_HOME", private_dir(&format!("{name}-config")));
    std::env::set_var("XDG_DATA_HOME", private_dir(&format!("{name}-data")));
    let mut rig = Rig::new(name, components, &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    (bus, rig)
}

/// The first mapped window of the server, and its frame on the output.
fn mapped(rig: &mut Rig) -> (cupertino_wm::WindowId, Area) {
    rig.until("the window is mapped", |rig| {
        rig.state.windows.iter().any(|w| w.mapped)
    });
    let id = rig.state.windows.iter().find(|w| w.mapped).unwrap().id;
    let frame = rig.state.wm.window(id).unwrap().geometry();
    let area = Area::new(
        frame.x as i32,
        frame.y as i32,
        frame.w as i32,
        frame.h as i32,
    );
    (id, area)
}

#[test]
fn the_overview_restarts_alone_after_injected_panics() {
    steady_allocator();
    let (_bus, mut rig) = start("overview-crash", &["dock", "overview"]);
    let session = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(PROBE_TIMEOUT).build())
        .expect("the test's bus");
    let mut gone = HashSet::new();
    rig.until(
        "the dock draws and the overview has its connection",
        |rig| dock_colour(rig).is_some() && overview_name(rig, &session, &gone).is_some(),
    );
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance("overview");
    let mut host_memory = HostMemory::of(host);
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let mut name = None;
        rig.until("the overview has its connection", |rig| {
            name = overview_name(rig, &session, &gone);
            name.is_some()
        });
        let name = name.unwrap();
        gone.insert(name.clone());
        rig.until("the overview serves its debug interface", |rig| {
            serves_crash(rig, &session, &name)
        });
        // no reply is awaited: a crash at dispatch or on a helper may end the component before its reply leaves, and
        // what shows the crash was taken is the next instance
        let crash_call = zbus::Message::method_call("/org/cupertino/ShellComponentDebug1", "Crash")
            .and_then(|m| m.destination(name.as_str()))
            .and_then(|m| m.interface("org.cupertino.ShellComponentDebug1"))
            .and_then(|m| m.with_flags(zbus::message::Flags::NoReplyExpected))
            .and_then(|m| m.build(&(at,)))
            .expect("the crash call");
        session
            .send(&crash_call)
            .unwrap_or_else(|e| panic!("the crash at {at} is sent: {e}"));
        // the person asks for the overview: the component hears it and draws its surface
        rig.state.toggle_overview(false);
        let gives_up = (crash + 1) % CRASHES_IN_A_ROW == 0;
        if gives_up {
            rig.until(
                &format!("the server gives the overview up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("overview")
                },
            );
            let rx = rig.restart("overview");
            rig.answer(&rx).expect("the overview starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the overview starts again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("overview") == instance
                    && rig.state.hosts.component_connected("overview")
            },
        );
        host_memory.restarted(crash + 1);
    }
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
        "the dock kept drawing while the overview started again"
    );
    assert!(!rig.state.hosts.component_gave_up("overview"));
    host_memory.assert_nothing_kept(&mut rig);
}

/// The fingers of the swipe up, and its travel for the whole gesture (the recogniser's `overview_distance`).
const FINGERS: u32 = 3;
const SWIPE_DISTANCE: f64 = 250.0;
/// How far both swipes go before the fingers lift: short of the half at which a release without speed opens.
const SHORT_OF_HALF: f64 = 0.35;
/// The seconds between two touchpad events of the slow swipe, and of the flick.
const SLOW_STEP: f64 = 0.05;
const FLICK_STEP: f64 = 0.008;
/// The events each swipe moves in.
const SWIPE_EVENTS: u32 = 6;
/// The events the slow swipe then rests in place before the fingers lift: longer than the recogniser's velocity
/// window, so it lifts without speed.
const REST_EVENTS: u32 = 6;
/// The turns the window must stay claimed after the flick settled for the overview to have stayed open.
const OPEN_TURNS: u32 = 200;

/// A swipe up of [`SHORT_OF_HALF`] of the travel in [`SWIPE_EVENTS`] events `step` seconds apart, from `time`; with
/// `rest`, the fingers stay still a while before they lift. Returns the time after it.
fn swipe(rig: &mut Rig, time: f64, step: f64, rest: bool) -> f64 {
    swipe_by(rig, time, step, rest, -SHORT_OF_HALF)
}

/// [`swipe`] over `travel` of the whole gesture's travel: up when it is negative, down when it is positive.
fn swipe_by(rig: &mut Rig, mut time: f64, step: f64, rest: bool, travel: f64) -> f64 {
    rig.state.gesture(GestureEvent::SwipeBegin {
        time,
        fingers: FINGERS,
    });
    for _ in 0..SWIPE_EVENTS {
        time += step;
        rig.state.gesture(GestureEvent::SwipeUpdate {
            time,
            dx: 0.0,
            dy: SWIPE_DISTANCE * travel / f64::from(SWIPE_EVENTS),
        });
        rig.turn();
    }
    if rest {
        for _ in 0..REST_EVENTS {
            time += step;
            rig.state.gesture(GestureEvent::SwipeUpdate {
                time,
                dx: 0.0,
                dy: 0.0,
            });
            rig.turn();
        }
    }
    time += step;
    rig.state.gesture(GestureEvent::SwipeEnd {
        time,
        cancelled: false,
    });
    time
}

#[test]
fn a_swipe_up_settles_with_the_velocity_it_is_released_with() {
    let (_bus, mut rig) = start("overview-gesture", &["overview"]);
    let client = rig.client();
    app::spawn(client, "org.cupertino.test.overview");
    let (id, own) = mapped(&mut rig);
    let output = rig.state.outputs[0].id;
    rig.until("the swipe up is armed", |rig| {
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeUp)
    });

    // slowly, and still when the fingers lift: the overview settles back and the window is its own again
    let time = swipe(&mut rig, 1.0, SLOW_STEP, true);
    rig.until("the slow swipe settled back", |rig| {
        !rig.state.transactions.is_window_claimed(id)
            && rig
                .state
                .transactions
                .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    assert!(
        rig.shows(own, magenta, own.covered()),
        "the window shows at its own frame"
    );

    // the same way in a quick flick: the velocity carries it open, and the window stays in its hole
    swipe(&mut rig, time + 1.0, FLICK_STEP, false);
    rig.until("the flick took the window into the overview", |rig| {
        rig.state.transactions.is_window_claimed(id)
    });
    for _ in 0..OPEN_TURNS {
        rig.turn();
        assert!(
            rig.state.transactions.is_window_claimed(id),
            "the overview the flick opened stays open"
        );
    }
    assert!(
        count(&rig.image(), Area::output(), magenta) > 0,
        "the window shows in its hole"
    );
}

#[test]
fn a_swipe_down_takes_back_the_overview_a_swipe_up_opened() {
    let (_bus, mut rig) = start("overview-reverse", &["overview"]);
    let client = rig.client();
    app::spawn(client, "org.cupertino.test.overview");
    let (id, own) = mapped(&mut rig);
    let output = rig.state.outputs[0].id;
    rig.until("the swipe up is armed", |rig| {
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeUp)
    });

    // a flick opens the overview, which comes to rest open and keeps the swipe down that takes it back
    let time = swipe(&mut rig, 1.0, FLICK_STEP, false);
    rig.until("the flick opened the overview and it came to rest", |rig| {
        let now = rig.state.now();
        rig.state.transactions.is_window_claimed(id)
            && rig
                .state
                .transactions
                .gesture_progress_on(output, now)
                .is_none()
    });
    for _ in 0..OPEN_TURNS {
        rig.turn();
    }
    assert!(
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeDown),
        "the open overview keeps the swipe down that takes it back"
    );

    // the swipe down takes it back: the window is its own again, and the swipe up is armed anew
    swipe_by(&mut rig, time + 1.0, FLICK_STEP, false, SHORT_OF_HALF);
    rig.until("the swipe down took the overview back", |rig| {
        !rig.state.transactions.is_window_claimed(id)
            && rig
                .state
                .transactions
                .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    assert!(
        rig.shows(own, magenta, own.covered()),
        "the window shows at its own frame"
    );
}

#[test]
fn a_window_closing_in_the_open_overview_closes_its_hole() {
    let (_bus, mut rig) = start("overview-close", &["overview"]);
    let client = rig.client();
    let closer = client.try_clone().expect("the client's socket");
    app::spawn(client, "org.cupertino.test.overview");
    let (id, _) = mapped(&mut rig);
    let output = rig.state.outputs[0].id;
    rig.until("the overview has the window", |rig| {
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    rig.state.toggle_overview(false);
    rig.until("the overview shows the window in its hole", |rig| {
        rig.state.transactions.is_window_claimed(id)
            && count(&rig.image(), Area::output(), magenta) > 0
    });

    // the application goes: its window closes, and its hole with it
    closer
        .shutdown(Shutdown::Both)
        .expect("the client's connection ends");
    rig.until("the window's hole closed", |rig| {
        rig.state.transactions.active_claim_for(id).is_none()
            && count(&rig.image(), Area::output(), magenta) == 0
    });
}

/// The surface the keyboard's input goes to.
fn keyboard_focus(rig: &Rig) -> Option<wayland_server::protocol::wl_surface::WlSurface> {
    rig.state
        .seat
        .get_keyboard()
        .and_then(|k| k.current_focus())
}

/// The surface of window `id`.
fn window_surface(
    rig: &Rig,
    id: cupertino_wm::WindowId,
) -> Option<wayland_server::protocol::wl_surface::WlSurface> {
    rig.state
        .windows
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.toplevel.wl_surface().clone())
}

/// The times the toggle opens and leaves the overview, each a transaction in and one out: more than a connection may
/// have that have not ended.
const TOGGLE_ROUNDS: usize = cupertino_protocols::transaction::MAX_ACTIVE_TRANSACTIONS / 2 + 2;
/// The turns the overview stays shown before the toggle leaves it: past the way in's motion (0.5 s and more).
const SHOWN_TURNS: u32 = 100;

#[test]
fn the_toggle_opens_the_overview_and_the_next_one_leaves_it() {
    let (_bus, mut rig) = start("overview-toggle", &["overview"]);
    let client = rig.client();
    app::spawn(client, "org.cupertino.test.overview");
    let (id, own) = mapped(&mut rig);
    let output = rig.state.outputs[0].id;
    rig.until("the overview has the window", |rig| {
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    for round in 0..TOGGLE_ROUNDS {
        rig.state.toggle_overview(false);
        rig.until(
            &format!("round {round}: the toggle opened the overview"),
            |rig| {
                rig.state.transactions.is_window_claimed(id)
                    && count(&rig.image(), Area::output(), magenta) > 0
            },
        );
        for _ in 0..SHOWN_TURNS {
            rig.turn();
        }
        rig.until(
            &format!("round {round}: the open overview has the keyboard, not the window"),
            |rig| {
                let focus = keyboard_focus(rig);
                focus.is_some()
                    && focus == rig.state.exclusive_layer
                    && focus != window_surface(rig, id)
            },
        );
        rig.state.toggle_overview(false);
        rig.until(&format!("round {round}: the next toggle left it"), |rig| {
            !rig.state.transactions.is_window_claimed(id)
                && rig
                    .state
                    .transactions
                    .has_gesture_binding(output, WireGesture::SwipeUp)
        });
        assert!(
            rig.shows(own, magenta, own.covered()),
            "round {round}: the window shows at its own frame"
        );
        assert!(
            rig.state.exclusive_layer.is_none() && keyboard_focus(&rig) == window_surface(&rig, id),
            "round {round}: the window has the keyboard again"
        );
    }
    assert_eq!(
        rig.state.hosts.component_instance("overview"),
        1,
        "the overview kept its first instance"
    );
}

/// The turns a test watches after the windows changed: past the re-arm's quiet time (250 ms) and the arming after it.
const CHANGE_TURNS: u32 = 200;

#[test]
fn the_windows_changing_never_open_the_overview() {
    let (_bus, mut rig) = start("overview-changes", &["overview"]);
    app::spawn(rig.client(), "org.cupertino.test.first");
    let (first, _) = mapped(&mut rig);
    app::spawn(rig.client(), "org.cupertino.test.second");
    rig.until("both windows are mapped", |rig| {
        rig.state.windows.iter().filter(|w| w.mapped).count() == 2
    });
    let second = rig
        .state
        .windows
        .iter()
        .find(|w| w.mapped && w.id != first)
        .unwrap()
        .id;
    let output = rig.state.outputs[0].id;
    rig.until("the swipe up is armed", |rig| {
        rig.state
            .transactions
            .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    // the person goes from one application to the other and back: each change arms the gestures anew
    for (round, window) in [second, first, second, first].into_iter().enumerate() {
        rig.state.focus_window_by_input(window);
        let mut disarmed = false;
        for turn in 0..CHANGE_TURNS {
            rig.turn();
            disarmed |= !rig
                .state
                .transactions
                .has_gesture_binding(output, WireGesture::SwipeUp);
            assert!(
                !rig.state.transactions.is_window_claimed(first)
                    && !rig.state.transactions.is_window_claimed(second),
                "change {round}, turn {turn}: no overview opened by itself"
            );
        }
        assert!(disarmed, "change {round}: the gestures were armed anew");
        rig.until(
            &format!("change {round}: the swipe up is armed again"),
            |rig| {
                rig.state
                    .transactions
                    .has_gesture_binding(output, WireGesture::SwipeUp)
            },
        );
    }
}

/// How much of the menu bar may still show while it gives way to the overview, and may be missing once it is back:
/// what is left of its fade once the fade has as good as run.
const FADED: f64 = 0.01;

/// A point of the menu bar: the middle of the output's width, half way down the bar.
fn bar_point() -> (f64, f64) {
    (
        f64::from(OUTPUT_SIZE.0) / 2.0,
        cupertino_kit_shell::design::menu_bar().height / 2.0,
    )
}

/// The menu bar's surface, when it takes the pointer at [`bar_point`], where the pointer goes.
fn bar_under_pointer(rig: &mut Rig) -> Option<wayland_server::protocol::wl_surface::WlSurface> {
    let time = (rig.state.now() * 1000.0) as u32;
    rig.state.pointer_moved_to(bar_point().into(), time, None);
    rig.state
        .pointer
        .current_focus()
        .filter(|surface| rig.state.menu_bars.has_surface(surface))
}

/// How much of the menu bar `bar` shows now: the opacity of its presentation.
fn bar_opacity(rig: &Rig, bar: &wayland_server::protocol::wl_surface::WlSurface) -> f64 {
    cupertino_server::layers_v1::presentation(bar, rig.state.now(), false)
        .map_or(1.0, |p| p.opacity)
}

/// A session of the overview and the menu bar with one window, once the swipe up is armed and the bar takes the
/// pointer: the window, its frame, the bar's surface and the usable area the bar leaves.
struct Bar {
    window: cupertino_wm::WindowId,
    own: Area,
    surface: wayland_server::protocol::wl_surface::WlSurface,
    usable: cupertino_wm::Rect,
}

impl Bar {
    fn start(rig: &mut Rig) -> Bar {
        app::spawn(rig.client(), "org.cupertino.test.overview");
        let (window, _) = mapped(rig);
        let output = rig.state.outputs[0].id;
        rig.until(
            "the swipe up is armed and the bar takes the pointer",
            |rig| {
                rig.state
                    .transactions
                    .has_gesture_binding(output, WireGesture::SwipeUp)
                    && bar_under_pointer(rig).is_some()
            },
        );
        let surface = bar_under_pointer(rig).unwrap();
        // the window's frame within the space the bar left it
        let (_, own) = mapped(rig);
        Bar {
            window,
            own,
            surface,
            usable: rig.state.usable_area(output),
        }
    }

    /// The bar's space stays reserved and the window keeps its frame, `when`.
    fn holds(&self, rig: &Rig, when: &str) {
        let output = rig.state.outputs[0].id;
        assert_eq!(
            rig.state.usable_area(output),
            self.usable,
            "{when}: the bar's space stays reserved"
        );
        let frame = rig.state.wm.window(self.window).unwrap().geometry();
        assert_eq!(
            Area::new(
                frame.x as i32,
                frame.y as i32,
                frame.w as i32,
                frame.h as i32
            ),
            self.own,
            "{when}: the window keeps its frame"
        );
    }

    /// Turn until the overview has the window and the bar has given way to it, the bar's space reserved meanwhile.
    fn gives_way(&self, rig: &mut Rig, when: &str) {
        rig.until(
            &format!("{when}: the bar gives way to the overview"),
            |rig| {
                self.holds(rig, when);
                rig.state.transactions.is_window_claimed(self.window)
                    && bar_opacity(rig, &self.surface) <= FADED
            },
        );
    }

    /// Turn until the overview left the window and the bar is back and takes the pointer, its space reserved
    /// meanwhile.
    fn is_back(&self, rig: &mut Rig, when: &str) {
        rig.until(&format!("{when}: the bar is back"), |rig| {
            self.holds(rig, when);
            !rig.state.transactions.is_window_claimed(self.window)
                && bar_opacity(rig, &self.surface) >= 1.0 - FADED
                && bar_under_pointer(rig).as_ref() == Some(&self.surface)
        });
    }
}

#[test]
fn the_menu_bar_gives_way_to_the_toggles_overview_until_the_toggle_or_escape_leaves_it() {
    let (_bus, mut rig) = start("overview-menubar-toggle", &["overview", "menubar"]);
    let bar = Bar::start(&mut rig);
    assert!(
        bar_opacity(&rig, &bar.surface) >= 1.0 - FADED,
        "the bar shows"
    );

    rig.state.toggle_overview(false);
    bar.gives_way(&mut rig, "the toggle");
    assert!(
        bar_under_pointer(&mut rig).is_none(),
        "the bar takes no input under the overview"
    );
    rig.state.toggle_overview(false);
    bar.is_back(&mut rig, "the next toggle");

    // Escape in the open overview leaves it as well
    rig.state.toggle_overview(false);
    bar.gives_way(&mut rig, "the toggle again");
    rig.until("the open overview has the keyboard", |rig| {
        let focus = keyboard_focus(rig);
        focus.is_some() && focus == rig.state.exclusive_layer
    });
    rig.state.scripted_key("Escape").expect("Escape is a key");
    bar.is_back(&mut rig, "Escape");
}

#[test]
fn the_menu_bar_comes_back_when_a_swipe_up_settles_back() {
    let (_bus, mut rig) = start("overview-menubar-swipe", &["overview", "menubar"]);
    let bar = Bar::start(&mut rig);
    let output = rig.state.outputs[0].id;

    // slowly, and still when the fingers lift: the bar began to give way as the overview came, and comes back
    swipe(&mut rig, 1.0, SLOW_STEP, true);
    let mut least = 1.0_f64;
    rig.until("the slow swipe settled back", |rig| {
        bar.holds(rig, "the slow swipe");
        least = least.min(bar_opacity(rig, &bar.surface));
        !rig.state.transactions.is_window_claimed(bar.window)
            && rig
                .state
                .transactions
                .has_gesture_binding(output, WireGesture::SwipeUp)
    });
    assert!(
        least < 1.0 - FADED,
        "the bar began to give way as the swipe brought the overview in (it showed {least} at the least)"
    );
    bar.is_back(&mut rig, "the swipe settled back");
}

#[test]
fn the_menu_bar_comes_back_when_the_open_overview_fails() {
    let (_bus, mut rig) = start("overview-menubar-crash", &["overview", "menubar"]);
    let session = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(PROBE_TIMEOUT).build())
        .expect("the test's bus");
    let bar = Bar::start(&mut rig);
    let first_instance = rig.state.hosts.component_instance("overview");
    let mut name = None;
    rig.until("the overview has its connection", |rig| {
        name = overview_name(rig, &session, &HashSet::new());
        name.is_some()
    });
    let name = name.unwrap();
    rig.until("the overview serves its debug interface", |rig| {
        serves_crash(rig, &session, &name)
    });
    rig.state.toggle_overview(false);
    bar.gives_way(&mut rig, "the toggle");

    // the overview fails while it is open: its surface goes, and the bar comes back without a word of the next
    // instance
    let crash_call = zbus::Message::method_call("/org/cupertino/ShellComponentDebug1", "Crash")
        .and_then(|m| m.destination(name.as_str()))
        .and_then(|m| m.interface("org.cupertino.ShellComponentDebug1"))
        .and_then(|m| m.with_flags(zbus::message::Flags::NoReplyExpected))
        .and_then(|m| m.build(&("dispatch",)))
        .expect("the crash call");
    session.send(&crash_call).expect("the crash is sent");
    rig.until("the overview starts again", |rig| {
        // the pointer moving over the open overview has it dispatch, where it fails
        bar_under_pointer(rig);
        rig.state.hosts.component_instance("overview") > first_instance
    });
    bar.is_back(&mut rig, "the overview failed");
}

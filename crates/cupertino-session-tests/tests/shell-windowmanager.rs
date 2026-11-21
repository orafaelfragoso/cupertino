//! The window manager (`cupertino-windowmanager`) as `cupertino-shell` runs it, against the real server, headless, on a
//! session bus of the test's own and with settings and data directories of its own (`docs/architecture/shell-host.md`,
//! D16, D19). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs the host with the test dock of `shell/components.rs` and the real window manager
//! (`cupertino_shell::WINDOWMANAGER`).
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; each instance shows the strip again.
//! - The stage turned on is one transaction: the strip's holes claim their windows on the frame the window beyond the
//!   strip's rows is hidden.
//! - The strip shows the stage's other windows in holes of the window manager's surface, which the server draws; the
//!   strip is saved light and dark (`stage-strip-after-{light,dark}.png` in the tests' temporary folder).
//! - A window that closes in the strip has its hole closed: the claim ends and nothing of it shows.
//! - While the component starts again the server holds its placements and its strip (D19): no frame shows the hidden
//!   window or a strip window outside its hole.

#[path = "shell/components.rs"]
#[allow(dead_code)]
mod components;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;
#[path = "shell-windowmanager/windows.rs"]
mod windows;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::net::Shutdown;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};

use cupertino_server::host::HostKind;
use cupertino_wm::WindowId;
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};
use windows::Shown;

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// window manager.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-windowmanager-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::WINDOWMANAGER];

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

/// The component's name in the server's table.
const COMPONENT: &str = "windowmanager";

/// The windows of the tests, opened in this order: the last is the key window, on the stage, and the others go to the
/// strip, the most recent first.
const CYAN: Shown = Shown {
    app_id: "org.cupertino.test.cyan",
    size: (300, 210),
    rgb: [0, 200, 220],
};
const YELLOW: Shown = Shown {
    app_id: "org.cupertino.test.yellow",
    size: (260, 190),
    rgb: [240, 200, 0],
};
const MAGENTA: Shown = Shown {
    app_id: "org.cupertino.test.magenta",
    size: (340, 240),
    rgb: [220, 0, 220],
};

/// Windows enough for more groups than the strip has rows at the output's height (four at 600 points): the first is
/// hidden once the stage is on.
const MANY: [Shown; 6] = [
    Shown {
        app_id: "org.cupertino.test.one",
        size: (200, 150),
        rgb: [200, 40, 40],
    },
    Shown {
        app_id: "org.cupertino.test.two",
        size: (200, 150),
        rgb: [40, 200, 40],
    },
    Shown {
        app_id: "org.cupertino.test.three",
        size: (200, 150),
        rgb: [40, 40, 200],
    },
    Shown {
        app_id: "org.cupertino.test.four",
        size: (200, 150),
        rgb: [200, 200, 40],
    },
    Shown {
        app_id: "org.cupertino.test.five",
        size: (200, 150),
        rgb: [40, 200, 200],
    },
    Shown {
        app_id: "org.cupertino.test.six",
        size: (200, 150),
        rgb: [200, 40, 200],
    },
];

/// The strip's rows at the output's height: the groups beyond them are hidden.
const ROWS: usize = 4;

/// The right edge of the strip's column, in points from the output's left: its edge inset and width
/// (`cupertino_wm::stage::StageConfig`).
const STRIP_RIGHT: f64 = 16.0 + 170.0;

/// How far a pixel's channel may be from a test window's colour and still show it.
const TOLERANCE: i32 = 40;

/// Whether `rgb` shows the colour of `window`.
fn shows(window: Shown) -> impl Fn([u8; 3]) -> bool {
    move |rgb| (0..3).all(|c| (i32::from(rgb[c]) - i32::from(window.rgb[c])).abs() < TOLERANCE)
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
    /// The bus, for the session and the system alike.
    fn start() -> Bus {
        let config =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-windowmanager-bus.conf");
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
        .join(format!("shell-windowmanager-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// The host process's anonymous memory (its heap and stacks, which a leak grows), in KiB.
fn anonymous(pid: u32) -> u64 {
    let rollup =
        std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).expect("the host's memory");
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Anonymous:"))
        .and_then(|kib| kib.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("the host's anonymous memory")
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

/// How long a question to the component's connection waits for its answer: a connection that serves no object yet
/// leaves a call unanswered, so it is asked again.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether the connection `name` serves the components' debug interface yet. `session` gives up on a call after
/// [`PROBE_TIMEOUT`].
fn serves_crash(rig: &mut Rig, session: &Connection, name: &str) -> bool {
    let (session, name) = (session.clone(), name.to_owned());
    aside(rig, "the window manager answers", move || {
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

/// The window manager's own connection to the session bus, once the server knows it as the window manager's: one not
/// among `gone`, the connections of instances that failed, which the bus may still list for a while.
fn component_name(rig: &Rig, session: &Connection, gone: &HashSet<String>) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    dbus.list_names()
        .ok()?
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with(':') && !gone.contains(name))
        .find(|name| rig.state.hosts.component_of_bus_name(name) == Some(COMPONENT))
}

/// Make the window manager's instance now on the bus panic at `at` (`paint`, `dispatch` or `helper`); returns its
/// connection's name, which `gone` then holds.
fn crash(rig: &mut Rig, session: &Connection, gone: &mut HashSet<String>, at: &str) {
    let mut name = None;
    rig.until("the window manager has its connection", |rig| {
        name = component_name(rig, session, gone);
        name.is_some()
    });
    let name = name.unwrap();
    gone.insert(name.clone());
    rig.until("the window manager serves its debug interface", |rig| {
        serves_crash(rig, session, &name)
    });
    // no reply is awaited: a crash at dispatch or on a helper may end the component before its reply leaves, and what
    // shows the crash was taken is the next instance
    let crash_call = zbus::Message::method_call("/org/cupertino/ShellComponentDebug1", "Crash")
        .and_then(|m| m.destination(name.as_str()))
        .and_then(|m| m.interface("org.cupertino.ShellComponentDebug1"))
        .and_then(|m| m.with_flags(zbus::message::Flags::NoReplyExpected))
        .and_then(|m| m.build(&(at,)))
        .expect("the crash call");
    session
        .send(&crash_call)
        .unwrap_or_else(|e| panic!("the crash at {at} is sent: {e}"));
}

/// The test's own session bus.
fn session() -> Connection {
    zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(PROBE_TIMEOUT).build())
        .expect("the test's bus")
}

/// A session bus, settings and data of the test's own, `windowmanager.conf` turning the stage on when `stage`, and the
/// server with `components` serving the session on it.
fn start(name: &str, components: &[&str], stage: bool) -> (Bus, Rig) {
    let bus = Bus::start();
    let config = private_dir(&format!("{name}-config"));
    let conf = config.join("windowmanager.conf");
    std::fs::write(
        &conf,
        if stage {
            "stage = on\n"
        } else {
            "stage = off\n"
        },
    )
    .expect("windowmanager.conf");
    std::env::set_var("CUPERTINO_WINDOWMANAGER_CONFIG", &conf);
    std::env::set_var("XDG_CONFIG_HOME", &config);
    std::env::set_var("XDG_DATA_HOME", private_dir(&format!("{name}-data")));
    let mut rig = Rig::new(name, components, &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    (bus, rig)
}

/// The mapped window of the application `app_id`.
fn window_of(rig: &Rig, app_id: &str) -> Option<WindowId> {
    rig.state
        .windows
        .iter()
        .filter(|w| w.mapped)
        .map(|w| w.id)
        .find(|id| {
            rig.state
                .wm
                .window(*id)
                .and_then(|w| rig.state.wm.app(w.app))
                .is_some_and(|app| app.identity == app_id)
        })
}

/// Open `shown` in order, each made the key window before the next opens; returns their windows, and a second socket of
/// each application's connection, to end it with.
fn open(rig: &mut Rig, shown: &[Shown]) -> Vec<(WindowId, std::os::unix::net::UnixStream)> {
    let mut opened = Vec::new();
    for window in shown {
        let client = rig.client();
        let closer = client.try_clone().expect("the client's socket");
        windows::spawn(client, *window);
        let mut id = None;
        rig.until(&format!("{} is mapped", window.app_id), |rig| {
            id = window_of(rig, window.app_id);
            id.is_some()
        });
        let id = id.unwrap();
        // a window of another application that maps while one is the key window waits behind it: the person clicks it
        rig.state.focus_window_by_input(id);
        rig.until(&format!("{} is the key window", window.app_id), |rig| {
            rig.state.wm.key_window() == Some(id)
        });
        opened.push((id, closer));
    }
    opened
}

/// Whether the window manager's strip claims `id`: a hole of its surface shows the window, and nothing else does.
fn in_strip(rig: &Rig, id: WindowId) -> bool {
    rig.state
        .transactions
        .active_claim_for(id)
        .is_some_and(|hole| hole.component == COMPONENT)
}

/// Where the hole claiming `id` was drawn last, in the output's points.
fn hole_bounds(rig: &Rig, id: WindowId) -> Option<(f64, f64, f64, f64)> {
    let hole = rig.state.transactions.active_claim_for(id)?;
    let (_, r) = hole.shown.get()?;
    Some((r.loc.x, r.loc.y, r.size.w, r.size.h))
}

/// Whether the server holds `id` hidden for the window manager (or fades it out).
fn hidden(rig: &Rig, id: WindowId) -> bool {
    rig.state.window_managers.passes_input(id)
}

/// The strip has come to rest with `strip` in it: each claimed, its hole drawn inside the strip's column, nothing
/// moving.
fn strip_at_rest(rig: &mut Rig, strip: &[WindowId]) -> bool {
    strip.iter().all(|id| {
        in_strip(rig, *id)
            && hole_bounds(rig, *id)
                .is_some_and(|(x, _, w, _)| x >= 0.0 && x + w <= STRIP_RIGHT + 0.5)
    }) && !rig.state.animating()
}

/// Crashes injected into the window manager, after which each restarts alone.
const RESTARTS: u32 = 50;
/// Crashes after which the server gives a component up (D16: five within a minute); a restart the person asks for
/// then starts it again and forgets them.
const CRASHES_IN_A_ROW: u32 = 5;
/// Restarts after which the host's memory is measured the first time: the host's first allocations of a component
/// (its caches, the allocator's arenas) are made by then.
const WARM_RESTARTS: u32 = 10;
/// How much the host's anonymous memory may grow between that measurement and the last restart, in KiB: well below
/// what one leaked instance holds (its 8 MiB stack, its surface's buffers).
const ANONYMOUS_GROWTH_KIB: u64 = 4 * 1024;

#[test]
fn the_window_manager_restarts_alone_after_injected_panics() {
    let (_bus, mut rig) = start("crash", &["dock", COMPONENT], true);
    let session = session();
    let mut gone = HashSet::new();
    let opened = open(&mut rig, &[CYAN, MAGENTA]);
    let cyan = opened[0].0;
    rig.until("the dock draws and the strip shows cyan", |rig| {
        dock_colour(rig).is_some() && in_strip(rig, cyan)
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance(COMPONENT);
    let mut warm = None;
    for crash_at in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash_at as usize % 3];
        crash(&mut rig, &session, &mut gone, at);
        // the stage shortcut: the component hears it and draws its strip again
        rig.state.window_managers.toggle_stage();
        let gives_up = (crash_at + 1) % CRASHES_IN_A_ROW == 0;
        if gives_up {
            rig.until(
                &format!("the server gives the window manager up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up(COMPONENT)
                },
            );
            let rx = rig.restart(COMPONENT);
            rig.answer(&rx).expect("the window manager starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance, which shows the strip: a held one
        // turns the stage on, and `windowmanager.conf` does after the server gave it up
        let instance = first_instance + crash_at + 1;
        rig.until(
            &format!("the window manager starts again after a crash at {at} and shows the strip"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance(COMPONENT) == instance
                    && rig.state.hosts.component_connected(COMPONENT)
                    && rig
                        .state
                        .hosts
                        .wm_placement_hold(rig.state.outputs[0].id)
                        .is_none()
                    && in_strip(rig, cyan)
            },
        );
        if crash_at + 1 == WARM_RESTARTS {
            warm = Some(anonymous(host));
        }
    }
    let (warm, last) = (warm.unwrap(), anonymous(host));
    eprintln!(
        "host anonymous memory after {WARM_RESTARTS} restarts: {warm} KiB; after {RESTARTS}: {last} KiB"
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
    assert_eq!(
        colours,
        HashSet::from(["red", "blue"]),
        "the dock kept drawing while the window manager started again"
    );
    assert!(!rig.state.hosts.component_gave_up(COMPONENT));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );
}

#[test]
fn turning_the_stage_on_places_and_claims_on_one_frame() {
    let (_bus, mut rig) = start("transaction", &[COMPONENT], false);
    let opened: Vec<WindowId> = open(&mut rig, &MANY)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    rig.until("the window manager arranged the windows", |rig| {
        rig.state.hosts.component_connected(COMPONENT) && !rig.state.animating()
    });
    for id in &opened {
        assert!(
            !in_strip(&rig, *id) && !hidden(&rig, *id),
            "the stage is off"
        );
    }
    rig.state.window_managers.toggle_stage();
    // the first turn that shows anything of the change shows all of it: the strip claims its windows as the
    // window beyond its rows is hidden
    rig.until("the stage comes on", |rig| {
        opened
            .iter()
            .any(|id| in_strip(rig, *id) || hidden(rig, *id))
    });
    let (first, rest) = opened.split_at(MANY.len() - ROWS - 1);
    let (strip, key) = rest.split_at(ROWS);
    for id in first {
        assert!(
            hidden(&rig, *id) && !in_strip(&rig, *id),
            "the oldest group is hidden"
        );
    }
    for id in strip {
        assert!(
            in_strip(&rig, *id) && !hidden(&rig, *id),
            "the strip claims {id:?} on the same frame"
        );
    }
    assert!(
        !in_strip(&rig, key[0]) && !hidden(&rig, key[0]),
        "the key window stays on the stage"
    );
    rig.until("the strip comes to rest", |rig| strip_at_rest(rig, strip));
}

/// Save the output as `name` in the tests' temporary folder.
fn save(rig: &mut Rig, name: &str) {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    rig.image().save(&path).expect("the capture is saved");
    eprintln!("saved {}", path.display());
}

#[test]
fn the_strip_shows_the_stage_windows_in_holes() {
    let (_bus, mut rig) = start("holes", &[COMPONENT], true);
    let opened: Vec<WindowId> = open(&mut rig, &[CYAN, YELLOW, MAGENTA])
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let strip = [opened[0], opened[1]];
    rig.until("the strip comes to rest with cyan and yellow", |rig| {
        strip_at_rest(rig, &strip)
    });
    let column = Area::new(0, 0, STRIP_RIGHT as i32, OUTPUT_SIZE.1);
    let stage = Area::new(
        STRIP_RIGHT as i32 + 16,
        0,
        OUTPUT_SIZE.0 - STRIP_RIGHT as i32 - 16,
        OUTPUT_SIZE.1,
    );
    let image = rig.image();
    for window in [CYAN, YELLOW] {
        assert!(
            count(&image, column, shows(window)) > 0,
            "{} shows in its hole in the strip",
            window.app_id
        );
        assert_eq!(
            count(&image, stage, shows(window)),
            0,
            "{} shows nowhere else",
            window.app_id
        );
    }
    assert!(
        count(&image, stage, shows(MAGENTA)) > 0,
        "the key window is on the stage"
    );
    assert_eq!(
        count(&image, column, shows(MAGENTA)),
        0,
        "the key window is not in the strip"
    );
    save(&mut rig, "stage-strip-after-light.png");
    rig.state.toggle_appearance();
    rig.until("the dark appearance is drawn", |rig| !rig.state.animating());
    save(&mut rig, "stage-strip-after-dark.png");
}

#[test]
fn a_window_closing_in_the_strip_closes_its_hole() {
    let (_bus, mut rig) = start("close", &[COMPONENT], true);
    let mut opened = open(&mut rig, &[CYAN, MAGENTA]);
    let (cyan, closer) = opened.remove(0);
    rig.until("the strip shows cyan in its hole", |rig| {
        strip_at_rest(rig, &[cyan]) && count(&rig.image(), Area::output(), shows(CYAN)) > 0
    });
    // the application goes: its window closes, and its hole with it
    closer
        .shutdown(Shutdown::Both)
        .expect("the client's connection ends");
    rig.until("the window's hole closed", |rig| {
        rig.state.transactions.active_claim_for(cyan).is_none()
            && !rig.state.animating()
            && count(&rig.image(), Area::output(), shows(CYAN)) == 0
    });
}

#[test]
fn the_server_holds_the_placements_while_the_window_manager_restarts() {
    let (_bus, mut rig) = start("hold", &[COMPONENT], true);
    let session = session();
    let mut gone = HashSet::new();
    let opened: Vec<WindowId> = open(&mut rig, &MANY)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (first, rest) = opened.split_at(MANY.len() - ROWS - 1);
    let strip = &rest[..ROWS];
    rig.until(
        "the strip comes to rest with the oldest group hidden",
        |rig| strip_at_rest(rig, strip) && first.iter().all(|id| hidden(rig, *id)),
    );
    let output = rig.state.outputs[0].id;
    let instance = rig.state.hosts.component_instance(COMPONENT);
    crash(&mut rig, &session, &mut gone, "dispatch");
    // every turn until the new instance took the hold over, and after: the hidden window stays hidden and every strip
    // window stays in a hole
    let mut held = false;
    rig.until("the new instance took the placements over", |rig| {
        for id in first {
            assert!(hidden(rig, *id), "the hidden window stays hidden");
        }
        for id in strip {
            assert!(in_strip(rig, *id), "{id:?} stays in the strip");
        }
        held |= rig.state.hosts.wm_placement_hold(output).is_some();
        rig.state.hosts.component_instance(COMPONENT) == instance + 1
            && rig.state.hosts.component_connected(COMPONENT)
            && rig.state.hosts.wm_placement_hold(output).is_none()
            && strip_at_rest(rig, strip)
    });
    assert!(
        held,
        "the server held the placements while the window manager started again"
    );
}

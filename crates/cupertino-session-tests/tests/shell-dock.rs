//! The dock (`cupertino-dock`) as `cupertino-shell` runs it, against the real server, headless, on a session bus of
//! the test's own and with a home, settings and data directories of its own (`docs/architecture/shell-host.md`, D16,
//! D19). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs the host with the test menu bar of `shell/components.rs` and the real dock
//! (`cupertino_shell::DOCK`).
//! - The dock takes the server's `dock` role on the output and reserves the bar's depth at its bottom.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the menu bar keeps drawing and the host's anonymous memory stays flat; while it restarts the server holds the
//!   bar's picture and keeps its space, so the usable area never reflows (but for the fifth failure in a row, after
//!   which the server gives it up and holds nothing until it is started again), and every restarted dock takes its
//!   role again.

#[path = "shell/components.rs"]
#[allow(dead_code)]
mod components;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::time::Duration;

use cupertino_server::host::HostKind;
use zbus::blocking::Connection;

use components::{BAR_SIZE, GREEN, YELLOW};
use rig::{count, Area, Rig};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test menu bar and the
/// real dock.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, MENUBAR};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-dock-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&MENUBAR, &cupertino_shell::DOCK];

    /// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
    pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let at = args.iter().position(|a| a == MARK)?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((report, args[at + 2..].to_vec()))
    }

    /// Be the host, the menu bar reporting to `report`.
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

/// The area the test menu bar covers: the output's top left.
fn bar_area() -> Area {
    Area::new(0, 0, BAR_SIZE.width as i32, BAR_SIZE.height as i32)
}

/// The colour of [`components::MenuBar`] its area shows now.
fn bar_colour(rig: &mut Rig) -> Option<&'static str> {
    let image = rig.image();
    let area = bar_area();
    let shows = |colour: cupertino_kit::theme::Color| {
        let want = [colour.r, colour.g, colour.b].map(|c| (c * 255.0) as i32);
        count(&image, area, |rgb| {
            (0..3).all(|c| (i32::from(rgb[c]) - want[c]).abs() < 40)
        }) >= area.covered()
    };
    if shows(GREEN) {
        Some("green")
    } else if shows(YELLOW) {
        Some("yellow")
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
    fn start() -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-dock-bus.conf");
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

/// A directory of the test's own, new and empty, of mode 0700: the session's home, settings or data.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-dock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// The session the dock runs in: a home, settings, data (and so a trash) and a cache of the test's own, no desktop
/// entries of the machine's, and the session bus.
struct Session {
    bus: Bus,
    data: PathBuf,
}

impl Session {
    fn start() -> Session {
        let bus = Bus::start();
        let data = private_dir("data");
        std::env::set_var("HOME", private_dir("home"));
        std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_DATA_DIRS", private_dir("data-dirs"));
        std::env::set_var("XDG_CACHE_HOME", private_dir("cache"));
        Session { bus, data }
    }
}

/// The file whose putting in the trash and taking out has the dock draw again: the trash's tile changes its icon.
const TRASHED: &str = "paint.txt";

/// Turns after which a dock armed to panic as it paints that has not drawn yet is made to draw again: the trash's
/// tile follows the trash at the dock's next look, and a look before the file came in may have seen it already.
const REDRAW_TURNS: u32 = 200;

/// Have the dock draw again: [`TRASHED`] is put in the trash, or taken out when it is there.
fn toggle(data: &Path) {
    let files = data.join("Trash/files");
    std::fs::create_dir_all(&files).expect("the trash's files");
    let file = files.join(TRASHED);
    if file.exists() {
        std::fs::remove_file(file).expect("the trashed file goes");
    } else {
        std::fs::write(file, "").expect("the trashed file is made");
    }
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

/// The dock's own connection to the session bus, once the server knows it as the dock's: one not among `gone`, the
/// connections of instances that failed, which the bus may still list for a while.
fn dock_name(rig: &Rig, session: &Connection, gone: &HashSet<String>) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    dbus.list_names()
        .ok()?
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with(':') && !gone.contains(name))
        .find(|name| rig.state.hosts.component_of_bus_name(name) == Some("dock"))
}

/// How long a question to the dock's connection waits for its answer: a connection that serves no object yet leaves
/// a call unanswered, so it is asked again.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Whether the connection `name` serves the components' debug interface yet: the server knows a component's
/// connection as soon as its name is proved, before the component serves anything on it, and a call made before that
/// is never answered. `session` gives up on a call after [`PROBE_TIMEOUT`].
fn serves_crash(rig: &mut Rig, session: &Connection, name: &str) -> bool {
    let (session, name) = (session.clone(), name.to_owned());
    aside(rig, "the dock answers", move || {
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

/// Have the dock panic `at` `dispatch`, `paint` or `helper` (`org.cupertino.ShellComponentDebug1`), once its
/// current connection serves the interface; that connection joins `gone`.
fn crash_dock(rig: &mut Rig, session: &Connection, gone: &mut HashSet<String>, at: &str) {
    let mut name = None;
    rig.until("the dock has its connection", |rig| {
        name = dock_name(rig, session, gone);
        name.is_some()
    });
    let name = name.unwrap();
    gone.insert(name.clone());
    rig.until("the dock serves its debug interface", |rig| {
        serves_crash(rig, session, &name)
    });
    // no reply is awaited: a crash at dispatch or on a helper may end the component before its reply leaves, and what
    // shows the crash was taken is the next instance
    let call = zbus::Message::method_call("/org/cupertino/ShellComponentDebug1", "Crash")
        .and_then(|m| m.destination(name.as_str()))
        .and_then(|m| m.interface("org.cupertino.ShellComponentDebug1"))
        .and_then(|m| m.with_flags(zbus::message::Flags::NoReplyExpected))
        .and_then(|m| m.build(&(at,)))
        .expect("the crash call");
    session
        .send(&call)
        .unwrap_or_else(|e| panic!("the crash at {at} is sent: {e}"));
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

/// Turns over which the host's memory is sampled for one measurement ([`settled_anonymous`]).
const SETTLE_TURNS: u32 = 100;

/// The host's anonymous memory once what an instance does as it starts is over, in KiB: the least of it over
/// [`SETTLE_TURNS`]. A restarted dock shows its bar at once and reads its start and loads its icons on the I/O pool
/// after, which holds several MiB for a while.
fn settled_anonymous(rig: &mut Rig, pid: u32) -> u64 {
    let mut least = anonymous(pid);
    for _ in 0..SETTLE_TURNS {
        rig.turn();
        least = least.min(anonymous(pid));
    }
    least
}

/// Crashes injected into the dock, after which each restarts alone.
const RESTARTS: u32 = 50;
/// Crashes after which the server gives a component up (D16: five within a minute); a restart the person asks for
/// then starts it again and forgets them.
const CRASHES_IN_A_ROW: u32 = 5;
/// Restarts after which the host's memory is measured the first time: the host's first allocations of a component
/// (its caches, the allocator's arenas) are made by then.
const WARM_RESTARTS: u32 = 10;
/// How much the host's anonymous memory may grow between that measurement and the last restart, in KiB: well below
/// what one leaked instance holds (its 8 MiB stack, its bar's buffers and icons).
const ANONYMOUS_GROWTH_KIB: u64 = 4 * 1024;

#[test]
fn the_dock_restarts_alone_holding_its_place_after_injected_panics() {
    let session = Session::start();
    let mut rig = Rig::new("dock", &["dock", "menubar"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(session.bus.1.clone()));
    let bus = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.method_timeout(PROBE_TIMEOUT).build())
        .expect("the test's bus");
    let mut gone = HashSet::new();
    let output = rig.state.outputs[0].id;
    rig.until(
        "the menu bar draws and the dock is on the dock role, reserving its depth",
        |rig| {
            bar_colour(rig).is_some()
                && rig.state.docks.has_dock(output)
                && rig.state.usable_area(output).h < f64::from(rig::OUTPUT_SIZE.1)
        },
    );
    let host = rig.host_pid(HostKind::Shared).unwrap();
    // the dock may still grow its bar as its icons and settings come in: what it reserves once it is settled
    for _ in 0..50 {
        rig.turn();
    }
    let usable = rig.state.usable_area(output);

    let mut colours = HashSet::new();
    let mut held = false;
    let first_instance = rig.state.hosts.component_instance("dock");
    let mut warm = None;
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        crash_dock(&mut rig, &bus, &mut gone, at);
        if at == "paint" {
            // what it draws next panics: the trash's tile with a file more or less
            toggle(&session.data);
        }
        // every turn until it runs again the bar's space stays reserved and its picture shows, held or drawn anew;
        // but the server holds nothing of a component it gives up, the fifth failure in a row (D16)
        let gives_up = (crash + 1) % CRASHES_IN_A_ROW == 0;
        let mut turns = 0;
        let mut watch = |rig: &mut Rig| {
            colours.extend(bar_colour(rig));
            turns += 1;
            if at == "paint" && turns % REDRAW_TURNS == 0 {
                toggle(&session.data);
            }
            if gives_up {
                return;
            }
            assert_eq!(
                rig.state.usable_area(output),
                usable,
                "the bar's space stays reserved after a crash at {at}"
            );
            assert!(
                rig.state.docks.has_dock(output) || {
                    let holds = rig
                        .state
                        .hosts
                        .held_surfaces()
                        .iter()
                        .any(|h| h.component == "dock" && h.picture.is_some());
                    held |= holds;
                    holds
                },
                "the bar or its held picture shows after a crash at {at}"
            );
        };
        if gives_up {
            rig.until(
                &format!("the server gives the dock up after a crash at {at}"),
                |rig| {
                    watch(rig);
                    rig.state.hosts.component_gave_up("dock")
                },
            );
            let rx = rig.restart("dock");
            rig.answer(&rx).expect("the dock starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the dock starts again after a crash at {at}"),
            |rig| {
                watch(rig);
                rig.state.hosts.component_instance("dock") == instance
                    && rig.state.hosts.component_connected("dock")
            },
        );
        // the new instance's bar on the role, the server holding no picture of the dock any more
        rig.until(
            &format!("the restarted dock takes its role after a crash at {at}"),
            |rig| {
                watch(rig);
                rig.state.docks.has_dock(output)
                    && rig
                        .state
                        .hosts
                        .held_surfaces()
                        .iter()
                        .all(|h| h.component != "dock")
            },
        );
        // the restarted dock reads its start on the I/O pool after it shows its bar: that is over before the next crash
        let settled = settled_anonymous(&mut rig, host);
        if crash + 1 == WARM_RESTARTS {
            warm = Some(settled);
        }
    }
    let (warm, last) = (warm.unwrap(), settled_anonymous(&mut rig, host));
    eprintln!(
        "host anonymous memory after {WARM_RESTARTS} restarts: {warm} KiB; after {RESTARTS}: {last} KiB"
    );

    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(host),
        "the host went on"
    );
    assert_eq!(
        rig.state.hosts.component_instance("menubar"),
        1,
        "the menu bar was never restarted"
    );
    assert_eq!(
        colours,
        HashSet::from(["green", "yellow"]),
        "the menu bar kept drawing while the dock started again"
    );
    assert!(held, "the server held the bar's picture while it restarted");
    assert!(!rig.state.hosts.component_gave_up("dock"));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );
    // the restarted bar keeps its place
    assert_eq!(rig.state.usable_area(output), usable);
}

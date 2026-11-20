//! The control panel (`cupertino-controlcenter`) as `cupertino-shell` runs it, against the real server, headless, on
//! a session bus of the test's own (which stands for the system bus too) and in a runtime directory of its own
//! (`docs/architecture/shell-host.md`, D16, D21). The server's supervisor starts this test binary as `cupertino-shell`:
//! with [`host::MARK`] among its arguments it runs the host with the test dock of `shell/components.rs` and the real
//! control panel component (`cupertino_shell::CONTROLCENTER`).
//! - The component owns `org.cupertino.ControlPanel1` on a connection of its own in the host's process; `Toggle`
//!   opens the panel, which draws at the output's top right, and its `Shown` and `Output` properties say so.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; each restart comes back on a new connection with
//!   the panel closed, which is what a restart loses.

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
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};

use cupertino_controlcenter::control::{BUS_NAME, OBJECT_PATH};
use cupertino_server::host::HostKind;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// control panel component.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-controlcenter-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::CONTROLCENTER];

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

/// The area the panel's card covers once open: the output's top right, clear of the dock.
fn panel_area() -> Area {
    Area::new(
        OUTPUT_SIZE.0 - PANEL_AREA_WIDTH,
        0,
        PANEL_AREA_WIDTH,
        PANEL_AREA_HEIGHT,
    )
}

/// How many pixels of the panel's area differ from `before`, the output as it showed with the panel closed.
fn changed(rig: &mut Rig, before: &image::RgbaImage) -> usize {
    let now = rig.image();
    let area = panel_area();
    let mut changed = 0;
    for y in area.y..area.y + area.h {
        for x in area.x..area.x + area.w {
            let (a, b) = (
                now.get_pixel(x as u32, y as u32),
                before.get_pixel(x as u32, y as u32),
            );
            if (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > PIXEL_TOLERANCE) {
                changed += 1;
            }
        }
    }
    changed
}

/// A session bus of the test's own, whose address the hosts inherit for both their buses, and the address.
struct Bus(Child, String);

/// A bus that starts no service: nothing the component calls reaches a program of the machine.
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
        let config =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-controlcenter-bus.conf");
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
        // the radios and logind are asked on the system bus: this one, where nothing answers for them
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

/// A directory of the test's own, new and empty, of mode 0700: the session's runtime directory, or its settings.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-controlcenter-{name}-{}", std::process::id()));
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

/// The connection that owns `org.cupertino.ControlPanel1` now, and its process.
fn owner(session: &Connection) -> Option<(String, u32)> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(BUS_NAME).ok()?;
    let owner = dbus.get_name_owner(name.clone()).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).ok()?;
    Some((owner.to_string(), pid))
}

/// `Toggle` on the control panel.
fn toggle(session: &Connection) -> zbus::Result<()> {
    session
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(BUS_NAME), "Toggle", &())
        .map(drop)
}

/// The control panel's property `name`, asked of the bus now (no cache).
fn property<T: TryFrom<OwnedValue>>(session: &Connection, name: &str) -> Option<T> {
    let reply = session
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(BUS_NAME, name),
        )
        .ok()?;
    let value: OwnedValue = reply.body().deserialize().ok()?;
    T::try_from(value).ok()
}

/// Whether the panel says it is shown.
fn shown(session: &Connection) -> Option<bool> {
    property(session, "Shown")
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

/// The width and height of the area at the output's top right the open panel's card covers part of, in pixels: the
/// card is about 340 points wide below the menu bar's band, at scale 1.
const PANEL_AREA_WIDTH: i32 = 360;
const PANEL_AREA_HEIGHT: i32 = 320;
/// How far a channel may move before a pixel counts as changed by the panel, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// Pixels of the panel's area the open card changes at least: a fraction of the card itself.
const PANEL_PIXELS: usize = 20_000;
/// Crashes injected into the component, after which each restarts alone.
const RESTARTS: u32 = 50;
/// Crashes after which the server gives a component up (D16: five within a minute); a restart the person asks for
/// then starts it again and forgets them.
const CRASHES_IN_A_ROW: u32 = 5;
/// Restarts after which the host's memory is measured the first time: the host's first allocations of a component
/// (its caches, the allocator's arenas) are made by then.
const WARM_RESTARTS: u32 = 10;
/// How much the host's anonymous memory may grow between that measurement and the last restart, in KiB: well below
/// what one leaked instance holds (its 8 MiB stack, its windows' buffers).
const ANONYMOUS_GROWTH_KIB: u64 = 4 * 1024;

#[test]
fn the_control_panel_restarts_alone_closed_after_injected_panics() {
    let bus = Bus::start();
    let runtime = private_dir("runtime");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    // no settings of the person's: the default arrangement, the dock's defaults
    std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
    let mut rig = Rig::new("controlcenter", &["dock", "controlcenter"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = Connection::session().expect("the test's bus");
    rig.until(
        "the dock draws and the control panel owns its name",
        |rig| dock_colour(rig).is_some() && owner(&session).is_some(),
    );
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let (mut connection, pid) = owner(&session).unwrap();
    assert_eq!(pid, host, "the name is owned in the host's process");
    assert_eq!(shown(&session), Some(false), "the panel starts closed");

    // Toggle opens the panel on the sole output, and it draws at the top right
    let closed = rig.image();
    let bus_call = session.clone();
    aside(&mut rig, "Toggle is answered", move || toggle(&bus_call)).expect("Toggle");
    rig.until("the panel says it is shown", |_| {
        shown(&session) == Some(true)
    });
    let output: Option<String> = property(&session, "Output");
    assert_eq!(
        output.as_deref(),
        Some("headless-1"),
        "the panel names its output"
    );
    rig.until("the panel draws at the top right", |rig| {
        changed(rig, &closed) >= PANEL_PIXELS
    });
    assert!(
        dock_colour(&mut rig).is_some(),
        "the dock draws beside the panel"
    );

    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance("controlcenter");
    let mut warm = None;
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let name = connection.clone();
        let bus_call = session.clone();
        aside(&mut rig, "the crash is taken", move || {
            bus_call
                .call_method(
                    Some(name.as_str()),
                    "/org/cupertino/ShellComponentDebug1",
                    Some("org.cupertino.ShellComponentDebug1"),
                    "Crash",
                    &(at,),
                )
                .map(drop)
        })
        .unwrap_or_else(|e| panic!("the control panel takes a crash at {at}: {e}"));
        if at == "paint" {
            // what it draws next panics: the panel, opened
            let bus_call = session.clone();
            let _ = aside(&mut rig, "the panel is asked to open", move || {
                toggle(&bus_call)
            });
        }
        if (crash + 1) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the control panel up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("controlcenter")
                },
            );
            let rx = rig.restart("controlcenter");
            rig.answer(&rx).expect("the control panel starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the control panel starts again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("controlcenter") == instance
                    && rig.state.hosts.component_connected("controlcenter")
            },
        );
        // the new instance owns the name on a connection of its own, in the same process, the panel closed
        let mut now = None;
        rig.until("the restarted control panel owns its name", |_| {
            now = owner(&session).filter(|(name, _)| *name != connection);
            now.is_some()
        });
        let (name, pid) = now.unwrap();
        assert_eq!(pid, host, "after the restart {} at {at}", crash + 1);
        connection = name;
        assert_eq!(
            shown(&session),
            Some(false),
            "after the restart {} at {at} the panel is closed",
            crash + 1
        );
        if crash + 1 == WARM_RESTARTS {
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
        "the dock kept drawing while the control panel started again"
    );
    assert!(!rig.state.hosts.component_gave_up("controlcenter"));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );

    // the last instance opens again and draws
    rig.until("the closed panel's area settles", |rig| {
        changed(rig, &closed) == 0
    });
    let bus_call = session.clone();
    aside(&mut rig, "Toggle is answered", move || toggle(&bus_call)).expect("Toggle");
    rig.until("the restarted panel draws at the top right", |rig| {
        shown(&session) == Some(true) && changed(rig, &closed) >= PANEL_PIXELS
    });
    let _ = std::fs::remove_dir_all(&runtime);
}

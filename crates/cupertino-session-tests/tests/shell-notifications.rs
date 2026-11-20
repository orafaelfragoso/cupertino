//! The banners and the notification list (`cupertino-notifications`) as `cupertino-shell` runs them, against the real
//! server, headless, on a session bus of the test's own and in a runtime directory of its own
//! (`docs/architecture/shell-host.md`, D16, D21). The server's supervisor starts this test binary as `cupertino-shell`:
//! with [`host::MARK`] among its arguments it runs the host with the test dock of `shell/components.rs` and the real
//! notifications component (`cupertino_shell::NOTIFICATIONS`).
//! - The component owns `org.freedesktop.Notifications` and `org.cupertino.NotificationCenter1` on its own connection
//!   and keeps its list, not a transient notification, in a file of mode 0600 in the runtime directory.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; after each restart the list is back, without
//!   the transient notification.

#[path = "shell/components.rs"]
#[allow(dead_code)]
mod components;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};

use cupertino_notifications::control::{
    BUS_NAME, CENTRE_BUS_NAME, CENTRE_OBJECT_PATH, OBJECT_PATH,
};
use cupertino_server::host::HostKind;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// notifications component.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-notifications-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::NOTIFICATIONS];

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
    fn start() -> Bus {
        let config =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-notifications-bus.conf");
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

/// A directory of the test's own, new and empty, of mode 0700: the session's runtime directory, or its settings.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-notifications-{name}-{}", std::process::id()));
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

/// The connection that owns `org.freedesktop.Notifications` now, once it owns `org.cupertino.NotificationCenter1` too,
/// which the component claims second.
fn owner(session: &Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let owner_of = |name: &str| {
        let name = zbus::names::BusName::try_from(name).ok()?;
        dbus.get_name_owner(name)
            .ok()
            .map(|owner| owner.to_string())
    };
    let owner = owner_of(BUS_NAME)?;
    (owner_of(CENTRE_BUS_NAME)? == owner).then_some(owner)
}

/// `Notify` from `app`, transient or kept in the list; its id.
fn notify(session: &Connection, app: &str, summary: &str, transient: bool) -> zbus::Result<u32> {
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    if transient {
        hints.insert("transient", Value::from(true));
    }
    let actions: Vec<&str> = Vec::new();
    let reply = session.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some(BUS_NAME),
        "Notify",
        &(
            app,
            0u32,
            "",
            summary,
            "A line of its body",
            actions,
            hints,
            -1i32,
        ),
    )?;
    reply.body().deserialize()
}

/// What `Applications` answers: the applications of the notifications on screen and in the list.
fn applications(session: &Connection) -> zbus::Result<Vec<(String, String)>> {
    let reply = session.call_method(
        Some(CENTRE_BUS_NAME),
        CENTRE_OBJECT_PATH,
        Some(CENTRE_BUS_NAME),
        "Applications",
        &(),
    )?;
    reply.body().deserialize()
}

/// The file the list is kept in under the runtime directory `runtime`, with its content, once there is one.
fn kept(runtime: &Path) -> Option<(PathBuf, String)> {
    let entries = std::fs::read_dir(runtime.join("cupertino")).ok()?;
    let path = entries.filter_map(Result::ok).map(|e| e.path()).find(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        name.starts_with("notifications") && name.ends_with(".json")
    })?;
    let content = std::fs::read_to_string(&path).ok()?;
    Some((path, content))
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

/// The application whose notification the list keeps, and the one that sends a transient notification.
const KEPT_APP: &str = "Mail";
const TRANSIENT_APP: &str = "Weather";
/// The kept notification's title, as the file holds it.
const KEPT_SUMMARY: &str = "Lunch at noon";
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
fn the_notifications_restart_alone_with_their_list_after_injected_panics() {
    let bus = Bus::start();
    let runtime = private_dir("runtime");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    // no notifications.conf of the person's: every application with the defaults
    std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
    let mut rig = Rig::new("notifications", &["dock", "notifications"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = Connection::session().expect("the test's bus");
    rig.until(
        "the dock draws and the notifications own their name",
        |rig| dock_colour(rig).is_some() && owner(&session).is_some(),
    );
    let host = rig.host_pid(HostKind::Shared).unwrap();

    // one notification for the list, one transient one that never enters it
    let bus_call = session.clone();
    let ids = aside(&mut rig, "the notifications are answered", move || {
        (
            notify(&bus_call, KEPT_APP, KEPT_SUMMARY, false),
            notify(&bus_call, TRANSIENT_APP, "Rain soon", true),
        )
    });
    let (kept_id, transient_id) = (ids.0.expect("Notify"), ids.1.expect("Notify"));
    assert_ne!(kept_id, transient_id);
    let mut file = None;
    rig.until("the list is kept", |_| {
        file = kept(&runtime).filter(|(_, content)| content.contains(KEPT_SUMMARY));
        file.is_some()
    });
    let (path, content) = file.unwrap();
    assert!(
        !content.contains(TRANSIENT_APP),
        "a transient notification is not kept: {content}"
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{}", path.display());
    let mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);

    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance("notifications");
    let mut warm = None;
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let mut name = None;
        rig.until("the notifications own their names", |_| {
            name = owner(&session);
            name.is_some()
        });
        let name = name.unwrap();
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
        .unwrap_or_else(|e| panic!("the notifications take a crash at {at}: {e}"));
        if at == "paint" {
            // what they draw next panics: the list, opened
            let bus_call = session.clone();
            let _ = aside(&mut rig, "the list is asked to open", move || {
                bus_call
                    .call_method(
                        Some(CENTRE_BUS_NAME),
                        CENTRE_OBJECT_PATH,
                        Some(CENTRE_BUS_NAME),
                        "ToggleCentre",
                        &(),
                    )
                    .map(drop)
            });
        }
        if (crash + 1) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the notifications up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("notifications")
                },
            );
            let rx = rig.restart("notifications");
            rig.answer(&rx).expect("the notifications start again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the notifications start again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("notifications") == instance
                    && rig.state.hosts.component_connected("notifications")
            },
        );
        // the new instance owns the names once the list is back
        rig.until("the restarted notifications own their names", |_| {
            owner(&session).is_some()
        });
        let bus_call = session.clone();
        let listed = aside(&mut rig, "Applications is answered", move || {
            applications(&bus_call)
        })
        .expect("Applications");
        assert_eq!(
            listed,
            [(KEPT_APP.to_owned(), KEPT_APP.to_owned())],
            "after the restart {} at {at} the list holds the kept notification alone",
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
        "the dock kept drawing while the notifications started again"
    );
    assert!(!rig.state.hosts.component_gave_up("notifications"));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );

    // the restarted component takes notifications, and gives the next one an id the earlier runs did not give
    let bus_call = session.clone();
    let next = aside(&mut rig, "the next notification is answered", move || {
        notify(&bus_call, KEPT_APP, "Running late", false)
    })
    .expect("Notify");
    assert!(next > transient_id, "{next} after {transient_id}");
    let _ = std::fs::remove_dir_all(&runtime);
}

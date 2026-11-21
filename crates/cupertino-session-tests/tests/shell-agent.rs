//! The session agent's place in the session (`docs/architecture/shell-host.md`, *The component list* and *Programs the
//! server starts itself*), read from the real server's plan of the shell, and the agent (`cupertino-shell-agent`) as
//! `cupertino-shell` runs it, against the real server, headless, on a session bus of the test's own (which stands for
//! the system bus too: no logind, no session manager, no power daemon answers on it) and with settings, data, state
//! and runtime directories of its own (D16, D20). The server's supervisor starts this test binary as `cupertino-shell`:
//! with [`host::MARK`] among its arguments it runs the host with the test dock of `shell/components.rs` and the real
//! agent (`cupertino_shell::AGENT`).
//! - the `agent` component runs in the shared host like every other component of the table;
//! - the lock screen's process, `cupertino-login`, holds the `session` and `session_lock` roles and no longer the
//!   `session_agent` one, which is the `agent` component's alone;
//! - the agent owns `org.cupertino.SessionAgent1` on the connection the server accepted for it; the power key held
//!   past 1.5 s shows its power-button panel, the server's log out request (`log_out_requested`) its log out panel,
//!   and a restart asked of it over the bus replaces that panel;
//! - a panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; each restart comes back on a new connection with
//!   no panel, which is what a restart loses, and the last one shows its panel again;
//! - the lock screen's `org.cupertino.Login1` has `Lock` alone, which the owner of `org.cupertino.Session1` may call and
//!   no other caller.

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
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::authorization::ShellRole;
use cupertino_server::config::{BuiltIn, HostFacts};
use cupertino_server::host::HostKind;
use cupertino_server::session_agent::AgentRequest;
use cupertino_server::Config;
use cupertino_shell_agent::control::BUS_NAME as AGENT_NAME;
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED};
use memory::{steady_allocator, HostMemory, CRASHES_IN_A_ROW, RESTARTS};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// session agent.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-agent-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::AGENT];

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

#[test]
fn the_session_agent_runs_in_the_shared_host() {
    let config = Config {
        shell_components: vec!["osd".into(), "agent".into()],
        ..Config::default()
    };
    let facts = HostFacts {
        available: true,
        ..HostFacts::default()
    };
    let plan = config.shell_plan(false, &facts);
    assert_eq!(plan.shared, Some(vec!["osd", "agent"]));
}

#[test]
fn the_lock_screen_holds_no_session_agent_role() {
    let login = BuiltIn::Login.process(Path::new("/usr/bin/cupertino-login"));
    assert_eq!(
        login.roles,
        vec![ShellRole::Session, ShellRole::SessionLock]
    );
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

/// The area a panel of the agent covers part of: the output's middle, where it is centred, clear of the dock.
fn panel_area() -> Area {
    Area::new(
        (OUTPUT_SIZE.0 - PANEL_AREA_WIDTH) / 2,
        (OUTPUT_SIZE.1 - PANEL_AREA_HEIGHT) / 2,
        PANEL_AREA_WIDTH,
        PANEL_AREA_HEIGHT,
    )
}

/// Whether two pixels differ by more than [`PIXEL_TOLERANCE`] in a channel.
fn differ(a: &image::Rgba<u8>, b: &image::Rgba<u8>) -> bool {
    (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > PIXEL_TOLERANCE)
}

/// How many pixels of the panel's area differ between `now` and `before`.
fn changed(now: &image::RgbaImage, before: &image::RgbaImage) -> usize {
    let area = panel_area();
    let mut changed = 0;
    for y in area.y..area.y + area.h {
        for x in area.x..area.x + area.w {
            if differ(
                now.get_pixel(x as u32, y as u32),
                before.get_pixel(x as u32, y as u32),
            ) {
                changed += 1;
            }
        }
    }
    changed
}

/// How tall the panel that `now` shows over `before` is, in pixels: from the first to the last row of the panel's area
/// where it changes a run of pixels.
fn panel_height(now: &image::RgbaImage, before: &image::RgbaImage) -> i32 {
    let area = panel_area();
    let rows: Vec<i32> = (area.y..area.y + area.h)
        .filter(|&y| {
            (area.x..area.x + area.w)
                .filter(|&x| {
                    differ(
                        now.get_pixel(x as u32, y as u32),
                        before.get_pixel(x as u32, y as u32),
                    )
                })
                .count()
                >= PANEL_ROW_PIXELS
        })
        .collect();
    match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => last - first + 1,
        _ => 0,
    }
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
    fn start(name: &str) -> Bus {
        let config =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("shell-agent-{name}-bus.conf"));
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
        // logind, the session manager and the power daemon are asked on the system bus: this one, where nothing
        // answers for them
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

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-agent-{name}-{}", std::process::id()));
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

/// The connection that owns `org.cupertino.SessionAgent1` now, and its process.
fn agent_owner(session: &Connection) -> Option<(String, u32)> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(AGENT_NAME).ok()?;
    let owner = dbus.get_name_owner(name.clone()).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).ok()?;
    Some((owner.to_string(), pid))
}

/// Turn the server for `time`.
fn hold(rig: &mut Rig, what: &str, time: Duration) {
    let until = Instant::now() + time;
    rig.until(what, |_| Instant::now() >= until);
}

/// The width and height of the output's middle that a panel of the agent covers part of, in pixels: the panels are at
/// least 260 points wide and about 200 high at scale 1.
const PANEL_AREA_WIDTH: i32 = 320;
const PANEL_AREA_HEIGHT: i32 = 400;
/// How far a channel may move before a pixel counts as changed by a panel, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// Pixels of the panel's area a shown panel changes at least: a fraction of the smallest panel.
const PANEL_PIXELS: usize = 20_000;
/// Pixels of a row a panel changes at least where it covers the row: a fraction of its 260 points.
const PANEL_ROW_PIXELS: usize = 120;
/// How much taller the power-button panel, with its four stacked buttons, is than the log out panel, in pixels at
/// least: three more buttons' rows less the info line and the checkbox row it does not have.
const TALLER_BY: i32 = 40;
/// How long the power key is held: past the 1.5 s after which the power-button panel shows.
const POWER_KEY_HOLD: Duration = Duration::from_millis(1_800);
/// The session of the agent test: the bus, a server whose shell is the dock and the agent, `Session1` served on the
/// bus, and the directories the agent reads and writes, all the test's own.
fn agent_session() -> (Bus, Rig, Connection, Vec<PathBuf>) {
    let bus = Bus::start("restarts");
    let dirs: Vec<PathBuf> = ["runtime", "config", "config-dirs", "data", "state"]
        .into_iter()
        .map(private_dir)
        .collect();
    std::env::set_var("XDG_RUNTIME_DIR", &dirs[0]);
    std::env::set_var("XDG_CONFIG_HOME", &dirs[1]);
    std::env::set_var("XDG_CONFIG_DIRS", &dirs[2]);
    std::env::set_var("XDG_DATA_HOME", &dirs[3]);
    std::env::set_var("XDG_STATE_HOME", &dirs[4]);
    // no logind session: the agent starts no login item and reopens nothing
    std::env::remove_var("XDG_SESSION_ID");
    steady_allocator();
    let mut rig = Rig::new("agent", &["dock", "agent"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = Connection::session().expect("the test's bus");
    (bus, rig, session, dirs)
}

/// Turn until the panel's area differs from `before` by at least [`PANEL_PIXELS`] and holds still for a turn, and
/// return the picture.
fn panel_shows(rig: &mut Rig, what: &str, before: &image::RgbaImage) -> image::RgbaImage {
    let mut last: Option<image::RgbaImage> = None;
    let mut shown = None;
    rig.until(what, |rig| {
        let now = rig.image();
        let steady = last.as_ref().is_some_and(|l| changed(&now, l) == 0);
        let showing = changed(&now, before) >= PANEL_PIXELS;
        last = Some(now.clone());
        if steady && showing {
            shown = Some(now);
        }
        shown.is_some()
    });
    shown.unwrap()
}

#[test]
fn the_session_agent_shows_its_panels_and_restarts_alone_after_injected_panics() {
    let (_bus, mut rig, session, dirs) = agent_session();
    rig.until("the dock draws and the agent owns its name", |rig| {
        dock_colour(rig).is_some() && agent_owner(&session).is_some()
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let (mut connection, pid) = agent_owner(&session).unwrap();
    assert_eq!(pid, host, "the name is owned in the host's process");
    assert_eq!(
        rig.state.hosts.component_of_bus_name(&connection),
        Some("agent"),
        "{connection} is the agent's connection"
    );
    rig.until("the server accepts the agent's connection", |rig| {
        rig.state.hosts.component_bus_name("agent") == Some(connection.as_str())
    });
    let empty = rig.image();
    assert_eq!(changed(&empty, &empty), 0);

    // the power key held past 1.5 s: the power-button panel
    rig.state.power_key(true);
    hold(&mut rig, "the power key is held", POWER_KEY_HOLD);
    rig.state.power_key(false);
    let power = panel_shows(&mut rig, "the power-button panel shows", &empty);
    let power_height = panel_height(&power, &empty);

    // a restart loses the panel
    let first_instance = rig.state.hosts.component_instance("agent");
    crash(&mut rig, &session, &connection, "dispatch");
    rig.until("the agent starts again", |rig| {
        rig.state.hosts.component_instance("agent") == first_instance + 1
            && rig.state.hosts.component_connected("agent")
    });
    connection = new_owner(&mut rig, &session, &connection, host);
    rig.until("the restarted agent shows no panel", |rig| {
        changed(&rig.image(), &empty) == 0
    });

    // the server's log out request: the log out panel, lower than the power-button one
    rig.state.log_out_session();
    let log_out = panel_shows(&mut rig, "the log out panel shows", &empty);
    let log_out_height = panel_height(&log_out, &empty);
    assert!(
        power_height >= log_out_height + TALLER_BY,
        "the power-button panel ({power_height} px) is the taller, the log out panel {log_out_height} px"
    );

    // a restart asked of the agent over the bus replaces it: other words, at the same size
    rig.state.ask_session_agent(AgentRequest::Restart);
    rig.until("the restart panel replaces the log out panel", |rig| {
        let now = rig.image();
        changed(&now, &log_out) > 0 && changed(&now, &empty) >= PANEL_PIXELS
    });
    let restart = panel_shows(&mut rig, "the restart panel settles", &empty);
    assert!(
        (panel_height(&restart, &empty) - log_out_height).abs() <= 2,
        "the restart panel is as tall as the log out panel"
    );

    let mut colours = HashSet::new();
    let mut host_memory = HostMemory::of(host);
    let first_instance = rig.state.hosts.component_instance("agent");
    for crash_number in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash_number as usize % 3];
        crash(&mut rig, &session, &connection, at);
        if at == "paint" {
            // what it draws next panics: a panel
            rig.state.log_out_session();
        }
        // the crash before the loop, which took the power-button panel away, counts in the first row
        if (crash_number + 2) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the agent up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("agent")
                },
            );
            let rx = rig.restart("agent");
            rig.answer(&rx).expect("the agent starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash_number + 1;
        rig.until(
            &format!("the agent starts again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("agent") == instance
                    && rig.state.hosts.component_connected("agent")
            },
        );
        connection = new_owner(&mut rig, &session, &connection, host);
        host_memory.restarted(crash_number + 1);
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
        "the dock kept drawing while the agent started again"
    );
    assert!(!rig.state.hosts.component_gave_up("agent"));
    host_memory.assert_nothing_kept(&mut rig);

    // the last instance shows no panel, and its log out panel again
    rig.until("the restarted agent shows no panel", |rig| {
        changed(&rig.image(), &empty) == 0
    });
    rig.until("the server accepts the last agent's connection", |rig| {
        rig.state.hosts.component_bus_name("agent") == Some(connection.as_str())
    });
    rig.state.log_out_session();
    let again = panel_shows(
        &mut rig,
        "the restarted agent shows its log out panel",
        &empty,
    );
    assert!((panel_height(&again, &empty) - log_out_height).abs() <= 2);
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Make the agent on `connection` panic `at` (`paint`, `dispatch` or `helper`).
fn crash(rig: &mut Rig, session: &Connection, connection: &str, at: &'static str) {
    let (name, bus_call) = (connection.to_owned(), session.clone());
    aside(rig, "the crash is taken", move || {
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
    .unwrap_or_else(|e| panic!("the agent takes a crash at {at}: {e}"));
}

/// The restarted agent's connection: a new one, in the host's process, owning the agent's name.
fn new_owner(rig: &mut Rig, session: &Connection, old: &str, host: u32) -> String {
    let mut now = None;
    rig.until("the restarted agent owns its name", |_| {
        now = agent_owner(session).filter(|(name, _)| name != old);
        now.is_some()
    });
    let (name, pid) = now.unwrap();
    assert_eq!(pid, host, "the restarted agent runs in the host's process");
    name
}

/// The members of `interface` in the introspection `xml`, each as `kind name`.
fn members(xml: &str, interface: &str) -> Vec<String> {
    let start = xml
        .find(&format!("<interface name=\"{interface}\">"))
        .unwrap_or_else(|| panic!("{interface} is served:\n{xml}"));
    let block = &xml[start..];
    let block = &block[..block.find("</interface>").expect("the interface ends")];
    let mut members = Vec::new();
    for kind in ["method", "property", "signal"] {
        let open = format!("<{kind} name=\"");
        for (at, _) in block.match_indices(&open) {
            let name = &block[at + open.len()..];
            members.push(format!(
                "{kind} {}",
                &name[..name.find('"').expect("a name ends")]
            ));
        }
    }
    members
}

#[test]
fn the_lock_screens_login1_has_lock_alone() {
    let bus = Bus::start("login1");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&asked);
    // the lock screen's connections, their executors on threads of zbus's own where the lock screen's loop ticks them:
    // the one that serves Login1 and the one that asks who calls
    let connect = || {
        zbus::block_on(async {
            zbus::connection::Builder::address(bus.1.as_str())?
                .build()
                .await
        })
        .expect("the test's bus")
    };
    let (service, lookup) = (connect(), connect());
    zbus::block_on(cupertino_login::control::serve(
        &service,
        lookup,
        move |request| {
            heard.lock().unwrap().push(request);
        },
    ))
    .expect("the lock screen serves Login1");
    let unique = service.unique_name().expect("a unique name").to_string();
    let caller = Connection::session().expect("the test's bus");
    let reply = caller
        .call_method(
            Some(unique.as_str()),
            cupertino_login::control::OBJECT_PATH,
            Some("org.freedesktop.DBus.Introspectable"),
            "Introspect",
            &(),
        )
        .expect("introspection");
    let xml: String = reply.body().deserialize().expect("the introspection's XML");
    assert_eq!(
        members(&xml, cupertino_login::control::BUS_NAME),
        ["method Lock"]
    );

    // a caller that does not own Session1 is refused, though it is the lock screen's own process: nothing is asked
    let lock = |from: &Connection| {
        from.call_method(
            Some(unique.as_str()),
            cupertino_login::control::OBJECT_PATH,
            Some(cupertino_login::control::BUS_NAME),
            "Lock",
            &(),
        )
    };
    let refused = lock(&caller);
    assert!(
        matches!(&refused, Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "org.cupertino.Error.UntrustedCaller"),
        "only the server may lock: {refused:?}"
    );
    assert!(
        asked.lock().unwrap().is_empty(),
        "the refused call asks nothing"
    );

    // the stand-in server owns Session1, the one caller the lock screen takes: Lock reaches it
    let server = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.cupertino.Session1"))
        .and_then(|b| b.build())
        .expect("the stand-in server owns Session1");
    lock(&server).expect("Lock is answered");
    assert_eq!(
        *asked.lock().unwrap(),
        [cupertino_login::control::Request::Lock]
    );
    for gone in [
        "RequestLogOut",
        "RequestPowerDialog",
        "LogOutNow",
        "LockNow",
    ] {
        let refused = caller.call_method(
            Some(unique.as_str()),
            cupertino_login::control::OBJECT_PATH,
            Some(cupertino_login::control::BUS_NAME),
            gone,
            &(),
        );
        assert!(
            matches!(&refused, Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"),
            "{gone} is no member of Login1: {refused:?}"
        );
    }
}

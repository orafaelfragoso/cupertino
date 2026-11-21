//! The menu bar (`cupertino-menubar`) as `cupertino-shell` runs it, against the real server, headless, on a session bus
//! of the test's own, which stands for the system bus too, and with settings and data directories of its own
//! (`docs/architecture/shell-host.md`, D16, D19). The server's supervisor starts this test binary as `cupertino-shell`:
//! with [`host::MARK`] among its arguments it runs the host with the test dock of `shell/components.rs` and the real
//! menu bar (`cupertino_shell::MENUBAR`).
//! - The menu bar takes the server's `menu_bar` role and reserves its height, owns `org.kde.StatusNotifierWatcher` and
//!   `com.canonical.AppMenu.Registrar` on its own connection, and draws a status item registered with it.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; while it restarts the server holds its
//!   picture and keeps its space, so the usable area never reflows (but for the fifth failure in a row, after which
//!   the server gives it up and holds nothing until it is started again), and the restarted bar draws a status item
//!   again.

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
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::time::{Duration, Instant};

use cupertino_menubar::dbus::registrar::REGISTRAR_NAME;
use cupertino_menubar::dbus::sni::{DEFAULT_ITEM_PATH, WATCHER_NAME, WATCHER_PATH};
use cupertino_server::host::HostKind;
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED, TICK};
use memory::{steady_allocator, HostMemory, CRASHES_IN_A_ROW, RESTARTS};
use rig::{count, magenta, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// menu bar.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-menubar-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::MENUBAR];

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

/// How often a restart watch looks at the dock's colour: a third of [`TICK`], so each colour the dock holds for a tick is
/// seen, while the output is not drawn into an image on every turn of the loop (a whole software frame each, in a debug
/// build).
const DOCK_LOOK: Duration = Duration::from_millis(TICK.as_millis() as u64 / 3);

/// The height of the menu bar, in points (pixels at the output's scale 1).
fn bar_height() -> f64 {
    cupertino_kit_shell::design::menu_bar().height
}

/// The area the menu bar takes: the output's top, as tall as the bar.
fn bar_area() -> Area {
    Area::new(0, 0, OUTPUT_SIZE.0, bar_height() as i32)
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
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-menubar-bus.conf");
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
        .join(format!("shell-menubar-{name}-{}", std::process::id()));
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

/// The connection that owns `name` now.
fn owner_of(session: &Connection, name: &str) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(name).ok()?;
    dbus.get_name_owner(name)
        .ok()
        .map(|owner| owner.to_string())
}

/// The connection of the menu bar, once it owns the status item watcher and the menu registrar both.
fn bar_owner(session: &Connection) -> Option<String> {
    let watcher = owner_of(session, WATCHER_NAME)?;
    (owner_of(session, REGISTRAR_NAME)? == watcher).then_some(watcher)
}

/// The edge of the status item's icon, in pixels: larger than the bar asks for, so it is scaled down whole.
const ICON_EDGE: i32 = 32;

/// A status item of the test's own: its icon a magenta square.
struct Item;

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn id(&self) -> String {
        "test-item".to_owned()
    }

    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".to_owned()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        "Test item".to_owned()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "Active".to_owned()
    }

    /// One pixmap, in ARGB32 in network byte order.
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        let pixels = [255u8, 255, 0, 255].repeat((ICON_EDGE * ICON_EDGE) as usize);
        vec![(ICON_EDGE, ICON_EDGE, pixels)]
    }
}

/// A connection serving [`Item`] at the usual path.
fn item_connection() -> Connection {
    zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(DEFAULT_ITEM_PATH, Item))
        .and_then(|b| b.build())
        .expect("the test's status item")
}

/// Register the item of `item` with the watcher of the menu bar.
fn register(rig: &mut Rig, session: &Connection, item: &Connection) -> zbus::Result<()> {
    let (session, service) = (
        session.clone(),
        item.unique_name().expect("a unique name").to_string(),
    );
    aside(rig, "the item is registered", move || {
        session
            .call_method(
                Some(WATCHER_NAME),
                WATCHER_PATH,
                Some(WATCHER_NAME),
                "RegisterStatusNotifierItem",
                &(service.as_str(),),
            )
            .map(drop)
    })
}

/// Pixels of the item's magenta the bar must show for the item to be drawn: most of its icon at the bar's size.
const ITEM_PIXELS: usize = 100;

/// Whether the bar shows the item.
fn shows_item(rig: &mut Rig) -> bool {
    count(&rig.image(), bar_area(), magenta) >= ITEM_PIXELS
}

#[test]
fn the_menu_bar_restarts_alone_holding_its_place_after_injected_panics() {
    let bus = Bus::start();
    // no menubar.conf or key bindings of the person's, and Recent Items of the test's own
    std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
    std::env::set_var("XDG_DATA_HOME", private_dir("data"));
    steady_allocator();
    let mut rig = Rig::new("menubar", &["dock", "menubar"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = Connection::session().expect("the test's bus");
    let output = rig.state.outputs[0].id;
    rig.until(
        "the dock draws and the bar is on the menu bar role",
        |rig| {
            dock_colour(rig).is_some()
                && rig.state.menu_bars.has_bar(output)
                && bar_owner(&session).is_some()
        },
    );
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let reserved = rig.state.usable_area(output).y;
    assert_eq!(reserved, bar_height(), "the bar's space is reserved");

    // the names are the component's own connection's, which the server knows as the menu bar
    let owner = bar_owner(&session).unwrap();
    assert_eq!(
        rig.state.hosts.component_of_bus_name(&owner),
        Some("menubar"),
        "{owner} is the menu bar's connection"
    );

    // a status item registered with the watcher is drawn in the bar
    let item = item_connection();
    register(&mut rig, &session, &item).expect("the watcher takes the item");
    rig.until("the bar draws the item", shows_item);

    let mut colours = HashSet::new();
    let mut held = false;
    let first_instance = rig.state.hosts.component_instance("menubar");
    let mut host_memory = HostMemory::of(host);
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let mut name = None;
        rig.until("the menu bar owns its names", |_| {
            name = bar_owner(&session);
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
        .unwrap_or_else(|e| panic!("the menu bar takes a crash at {at}: {e}"));
        // what it draws next panics: the bar with an item new to it, so it draws now, not when its clock next turns
        // a minute (the first instance already shows `item`). Something else may have it draw first, and then the
        // watcher is already gone with it: the crash is taken all the same. The item ends with this crash's turn
        let _fresh = (at == "paint").then(|| {
            let fresh = item_connection();
            let _ = register(&mut rig, &session, &fresh);
            fresh
        });
        // every turn until it runs again the bar's space stays reserved and its picture shows, live, held or drawn
        // anew: the crashed instance's bar shows live until its connection ends, also once its teardown destroyed the
        // role object, and is then held as the output's menu bar. But the server holds nothing of a component it gives
        // up, the fifth failure in a row (D16)
        let gives_up = (crash + 1) % CRASHES_IN_A_ROW == 0;
        let mut looked: Option<Instant> = None;
        let mut watch = |rig: &mut Rig| {
            if looked.is_none_or(|at| at.elapsed() >= DOCK_LOOK) {
                looked = Some(Instant::now());
                colours.extend(dock_colour(rig));
            }
            if gives_up {
                return;
            }
            assert_eq!(
                rig.state.usable_area(output).y,
                reserved,
                "the bar's space stays reserved after a crash at {at}"
            );
            assert!(
                rig.state.hosts.persistent_shows_on("menubar", output),
                "the bar's picture shows, live or held, after a crash at {at}"
            );
            let holds: Vec<bool> = rig
                .state
                .hosts
                .held_surfaces()
                .iter()
                .filter(|h| h.component == "menubar")
                .map(|h| h.picture.as_ref().is_some_and(|p| p.menu_bar))
                .collect();
            assert!(
                holds.iter().all(|&bar| bar),
                "the bar is held with its picture, as the output's menu bar, after a crash at {at}"
            );
            held |= !holds.is_empty();
        };
        if gives_up {
            rig.until(
                &format!("the server gives the menu bar up after a crash at {at}"),
                |rig| {
                    watch(rig);
                    rig.state.hosts.component_gave_up("menubar")
                },
            );
            let rx = rig.restart("menubar");
            rig.answer(&rx).expect("the menu bar starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the menu bar starts again after a crash at {at}"),
            |rig| {
                watch(rig);
                rig.state.hosts.component_instance("menubar") == instance
                    && rig.state.hosts.component_connected("menubar")
            },
        );
        host_memory.restarted(crash + 1);
    }
    rig.until("the menu bar owns its names again", |_| {
        bar_owner(&session).is_some()
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
        "the dock kept drawing while the menu bar started again"
    );
    assert!(held, "the server held the bar's picture while it restarted");
    assert!(!rig.state.hosts.component_gave_up("menubar"));
    host_memory.assert_nothing_kept(&mut rig);

    // the restarted bar keeps its place and draws an item registered with it
    assert_eq!(rig.state.usable_area(output).y, reserved);
    let owner = bar_owner(&session).unwrap();
    assert_eq!(
        rig.state.hosts.component_of_bus_name(&owner),
        Some("menubar")
    );
    rig.until("the restarted bar no longer shows the item", |rig| {
        !shows_item(rig)
    });
    register(&mut rig, &session, &item).expect("the watcher takes the item");
    rig.until("the restarted bar draws the item", shows_item);
}

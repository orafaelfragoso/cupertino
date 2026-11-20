//! The desktop items (`cupertino-shell-desktop`) as `cupertino-shell` runs them, against the real server, headless, on
//! a session bus of the test's own and with a home, settings and data directories of its own (`docs/architecture/shell-host.md`,
//! D16, D19). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs the host with the test dock of `shell/components.rs` and the real desktop
//! (`cupertino_shell::DESKTOP`).
//! - The desktop shows the items of the Desktop folder in their cells; a file made there by another program shows in
//!   the next free cell; an item clicked, renamed with Return, a new name typed and Return is renamed on disk; an item
//!   clicked and Primary+Backspace is moved to the trash.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; while it restarts the server holds its picture,
//!   and every restarted desktop takes its place with its item: the item shows on every turn of a restart, unless the
//!   server gave the desktop up and held nothing.

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

use cupertino_kit::geometry::Rect;
use cupertino_server::host::HostKind;
use cupertino_shell_desktop::arrangement::{item_frames, Grid, Slot};
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// desktop.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-desktop-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::DESKTOP];

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

/// The evdev code of the left button.
const BUTTON_LEFT: u32 = 0x110;

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

/// The desktop's grid on the test's output, as the desktop lays it out in the area the server configures it to: the
/// whole output, since the test runs no menu bar and its dock reserves no space.
fn grid() -> Grid {
    Grid::new(f64::from(OUTPUT_SIZE.0), f64::from(OUTPUT_SIZE.1), 0.0)
}

fn area(rect: Rect) -> Area {
    Area::new(
        rect.x().round() as i32,
        rect.y().round() as i32,
        rect.width().round() as i32,
        rect.height().round() as i32,
    )
}

/// The icon of the item in `slot`.
fn icon(slot: Slot) -> Rect {
    item_frames(grid().cell(slot)).icon
}

/// Pixels of an icon that must differ from the wallpaper for the icon to be drawn: a quarter of it.
const ICON_PIXELS: usize = 64 * 64 / 4;

/// What the output shows where the desktop has no item: the wallpaper, taken while the Desktop folder is empty.
struct Ground(image::RgbaImage);

impl Ground {
    /// The wallpaper, once the desktop runs and the dock draws: the desktop draws nothing where it has no item.
    fn take(rig: &mut Rig) -> Ground {
        rig.until("the dock draws and the desktop runs", |rig| {
            dock_colour(rig).is_some() && rig.state.hosts.component_connected("desktop")
        });
        Ground(rig.image())
    }

    /// Whether the icon of the item in `slot` is drawn: enough of its pixels differ from the wallpaper.
    fn shows_icon(&self, rig: &mut Rig, slot: Slot) -> bool {
        let image = rig.image();
        let icon = area(icon(slot));
        let mut differ = 0;
        for y in icon.y..icon.y + icon.h {
            for x in icon.x..icon.x + icon.w {
                let (now, then) = (
                    image.get_pixel(x as u32, y as u32).0,
                    self.0.get_pixel(x as u32, y as u32).0,
                );
                differ += usize::from((0..3).any(|c| now[c].abs_diff(then[c]) > 30));
            }
        }
        differ >= ICON_PIXELS
    }
}

/// Click the icon of the item in `slot`.
fn click(rig: &mut Rig, slot: Slot) {
    let at = icon(slot).center();
    rig.state.scripted_click((at.x, at.y), BUTTON_LEFT);
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
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-desktop-bus.conf");
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

/// A directory of the test's own, new and empty, of mode 0700: the session's home, settings, data or cache.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-desktop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// The session bus connection of the desktop, as the server accepted it from the component.
fn desktop_connection(rig: &Rig, session: &Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    dbus.list_names()
        .ok()?
        .into_iter()
        .map(|name| name.to_string())
        .find(|name| rig.state.hosts.component_of_bus_name(name) == Some("desktop"))
}

/// Have the desktop panic `at` `dispatch`, `paint` or `helper` (`org.cupertino.ShellComponentDebug1`), on the
/// connection the server accepted as the desktop's, which serves the interface from before its announcement.
fn crash_desktop(rig: &mut Rig, bus: &Connection, at: &'static str) {
    let mut name = None;
    rig.until("the server accepted the desktop's connection", |rig| {
        name = desktop_connection(rig, bus);
        name.is_some()
    });
    let (name, bus) = (name.unwrap(), bus.clone());
    let call = std::thread::spawn(move || {
        bus.call_method(
            Some(name.as_str()),
            "/org/cupertino/ShellComponentDebug1",
            Some("org.cupertino.ShellComponentDebug1"),
            "Crash",
            &(at,),
        )
        .map(drop)
    });
    rig.until(&format!("the desktop takes a crash at {at}"), |_| {
        call.is_finished()
    });
    call.join()
        .unwrap()
        .unwrap_or_else(|e| panic!("the desktop takes a crash at {at}: {e}"));
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

/// Crashes injected into the desktop, after which each restarts alone.
const RESTARTS: u32 = 50;
/// Crashes after which the server gives a component up (D16: five within a minute); a restart the person asks for
/// then starts it again and forgets them.
const CRASHES_IN_A_ROW: u32 = 5;
/// Restarts after which the host's memory is measured the first time: the host's first allocations of a component
/// (its caches, the allocator's arenas) are made by then.
const WARM_RESTARTS: u32 = 10;
/// How much the host's anonymous memory may grow between that measurement and the last restart, in KiB: well below
/// what one leaked instance holds (its 8 MiB stack, its desktop's buffers).
const ANONYMOUS_GROWTH_KIB: u64 = 4 * 1024;

/// The item the desktop shows from its start, in the first cell, and the file whose making and removing has the
/// desktop draw again (in the second cell while it is there).
const ITEM: &str = "notes.txt";
const TOGGLED: &str = "paint.txt";

/// The session the desktop runs in: a home of the test's own with an empty Desktop folder, with settings, data (and so
/// a trash) and a thumbnail cache of its own, and the session bus.
struct Session {
    bus: Bus,
    desktop: PathBuf,
    data: PathBuf,
}

impl Session {
    fn start() -> Session {
        let bus = Bus::start();
        let home = private_dir("home");
        let data = private_dir("data");
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_CACHE_HOME", private_dir("cache"));
        let desktop = home.join("Desktop");
        std::fs::create_dir_all(&desktop).expect("the Desktop folder");
        Session { bus, desktop, data }
    }

    /// Make [`ITEM`], the first item.
    fn first_item(&self) {
        std::fs::write(self.desktop.join(ITEM), "Plan for the week\n").expect("the first item");
    }
}

/// Have the desktop draw again: [`TOGGLED`] is made, or removed when it is there.
fn toggle(desktop: &Path) {
    let file = desktop.join(TOGGLED);
    if file.exists() {
        std::fs::remove_file(file).expect("the toggled file goes");
    } else {
        std::fs::write(file, "").expect("the toggled file is made");
    }
}

#[test]
fn the_desktop_shows_renames_and_trashes_what_the_desktop_folder_holds() {
    let session = Session::start();
    let mut rig = Rig::new("desktop-files", &["dock", "desktop"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(session.bus.1.clone()));
    let (first, second) = (Slot::new(0, 0), Slot::new(0, 1));
    let ground = Ground::take(&mut rig);
    session.first_item();
    rig.until("the desktop shows its item", |rig| {
        ground.shows_icon(rig, first)
    });
    assert!(
        !ground.shows_icon(&mut rig, second),
        "the second cell is empty before a file is made"
    );

    // a file another program makes in the Desktop folder shows in the next free cell
    std::fs::write(session.desktop.join("Draft.txt"), "a draft\n").expect("a new file");
    rig.until("the new file shows in the second cell", |rig| {
        ground.shows_icon(rig, second)
    });

    // clicked, Return, a new name and Return: the item is renamed on disk
    click(&mut rig, second);
    for _ in 0..10 {
        rig.turn();
    }
    rig.state.scripted_key("Return").unwrap();
    for _ in 0..10 {
        rig.turn();
    }
    rig.state.scripted_type("Outline");
    rig.state.scripted_key("Return").unwrap();
    let renamed = session.desktop.join("Outline.txt");
    rig.until("the item is renamed on disk", |_| {
        renamed.exists() && !session.desktop.join("Draft.txt").exists()
    });
    rig.until("the renamed item keeps its cell", |rig| {
        ground.shows_icon(rig, second)
    });

    // clicked and Primary+Backspace: the item goes to the trash
    click(&mut rig, second);
    for _ in 0..10 {
        rig.turn();
    }
    rig.state.scripted_key("Primary+BackSpace").unwrap();
    let trashed = session.data.join("Trash/files/Outline.txt");
    rig.until("the item is in the trash", |_| {
        trashed.exists() && !renamed.exists()
    });
    assert!(
        session
            .data
            .join("Trash/info/Outline.txt.trashinfo")
            .exists(),
        "the trash says where the item was"
    );
    rig.until("the second cell is empty again", |rig| {
        !ground.shows_icon(rig, second)
    });
    assert!(session.desktop.join(ITEM).exists(), "the other item stays");
}

#[test]
fn the_desktop_restarts_alone_holding_its_picture_after_injected_panics() {
    let session = Session::start();
    let mut rig = Rig::new("desktop", &["dock", "desktop"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(session.bus.1.clone()));
    let bus = Connection::session().expect("the test's bus");
    let output = rig.state.outputs[0].id;
    let first = Slot::new(0, 0);
    let ground = Ground::take(&mut rig);
    session.first_item();
    rig.until("the desktop shows its item", |rig| {
        ground.shows_icon(rig, first)
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();
    // the desktop reserves nothing: the windows' room is the dock's and the menu bar's business
    let usable = rig.state.usable_area(output);

    let mut colours = HashSet::new();
    let mut held = false;
    let first_instance = rig.state.hosts.component_instance("desktop");
    let mut warm = None;
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        crash_desktop(&mut rig, &bus, at);
        if at == "paint" {
            // what it draws next panics: the listing with a file more or less
            toggle(&session.desktop);
        }
        // the server holds nothing of a component it gives up, the fifth failure in a row (D16)
        let gives_up = (crash + 1) % CRASHES_IN_A_ROW == 0;
        let mut watch = |rig: &mut Rig| {
            colours.extend(dock_colour(rig));
            assert_eq!(rig.state.usable_area(output), usable);
            // the held picture shows the item until the new instance's first frame, which shows it too
            assert!(
                gives_up || ground.shows_icon(rig, first),
                "the item went while the desktop restarted after a crash at {at}"
            );
            held |= rig
                .state
                .hosts
                .held_surfaces()
                .iter()
                .any(|h| h.component == "desktop" && h.picture.is_some());
        };
        if gives_up {
            rig.until(
                &format!("the server gives the desktop up after a crash at {at}"),
                |rig| {
                    watch(rig);
                    rig.state.hosts.component_gave_up("desktop")
                },
            );
            let rx = rig.restart("desktop");
            rig.answer(&rx).expect("the desktop starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the desktop starts again after a crash at {at}"),
            |rig| {
                watch(rig);
                rig.state.hosts.component_instance("desktop") == instance
                    && rig.state.hosts.component_connected("desktop")
            },
        );
        // drawn by the new instance, the server holding no picture of the desktop any more
        rig.until(
            &format!("the restarted desktop draws its item after a crash at {at}"),
            |rig| {
                watch(rig);
                rig.state
                    .hosts
                    .held_surfaces()
                    .iter()
                    .all(|h| h.component != "desktop")
                    && ground.shows_icon(rig, first)
            },
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
        "the dock kept drawing while the desktop started again"
    );
    assert!(
        held,
        "the server held the desktop's picture while it restarted"
    );
    assert!(!rig.state.hosts.component_gave_up("desktop"));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );
    assert!(
        session.desktop.join(ITEM).exists(),
        "no restart touched the item"
    );
}

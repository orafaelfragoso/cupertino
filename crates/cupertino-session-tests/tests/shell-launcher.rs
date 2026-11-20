//! The launcher (`cupertino-launcher`) as `cupertino-shell` runs it, against the real server, headless, on a session
//! bus of the test's own and in a runtime, home and settings folder of its own (`docs/architecture/shell-host.md`, D16,
//! D19, D20). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs the host with the test dock of `shell/components.rs` and the real launcher component
//! (`cupertino_shell::LAUNCHER`).
//! - The component owns `org.cupertino.Launcher1` on a connection of its own in the host's process; a process that is
//!   neither the server nor the launcher is refused, and the panel stays hidden.
//! - The server's launcher action (`toggle_launcher`, what `Super+Space` runs) shows the panel at the output's top
//!   centre, which takes the keyboard from the window that had it.
//! - A component dropped while the panel shows takes the panel with it, and the keyboard returns to that window.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts it alone, fifty times, while
//!   the dock keeps drawing and the host's anonymous memory stays flat; each restart comes back on a new connection,
//!   the panel hidden, which is what a restart loses.

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
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::Ordering;

use cupertino_launcher::control::{BUS_NAME, OBJECT_PATH};
use cupertino_server::host::HostKind;
use zbus::blocking::Connection;

use components::{BLUE, DOCK_SIZE, RED};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// launcher component.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-launcher-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::LAUNCHER];

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

/// A window that says whether it has the keyboard: one toplevel of magenta content and the seat's keyboard on the same
/// connection, on a thread of its own that answers the server until the connection ends.
mod focus {
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    };
    use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
    use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

    /// The size of the window's content, in pixels at scale 1.
    const WINDOW_SIZE: (i32, i32) = (100, 80);
    /// One pixel of the window in `wl_shm` ARGB8888 memory order: an opaque magenta.
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];

    struct Window {
        compositor: Option<wl_compositor::WlCompositor>,
        shm: Option<wl_shm::WlShm>,
        wm_base: Option<xdg_wm_base::XdgWmBase>,
        keyboard: Option<wl_keyboard::WlKeyboard>,
        configured: bool,
        focused: Arc<AtomicBool>,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Window {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name, interface, ..
            } = event
            {
                match interface.as_str() {
                    "wl_compositor" => state.compositor = Some(registry.bind(name, 4, qh, ())),
                    "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                    "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                    "wl_seat" => {
                        registry.bind::<wl_seat::WlSeat, _, _>(name, 5, qh, ());
                    }
                    _ => {}
                }
            }
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for Window {
        fn event(
            state: &mut Self,
            seat: &wl_seat::WlSeat,
            event: wl_seat::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_seat::Event::Capabilities {
                capabilities: WEnum::Value(capabilities),
            } = event
            {
                if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none()
                {
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                }
            }
        }
    }

    impl Dispatch<wl_keyboard::WlKeyboard, ()> for Window {
        fn event(
            state: &mut Self,
            _: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            // the connection has one surface, so an enter is the window's
            match event {
                wl_keyboard::Event::Enter { .. } => state.focused.store(true, Ordering::SeqCst),
                wl_keyboard::Event::Leave { .. } => state.focused.store(false, Ordering::SeqCst),
                _ => {}
            }
        }
    }

    impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Window {
        fn event(
            _: &mut Self,
            base: &xdg_wm_base::XdgWmBase,
            event: xdg_wm_base::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                base.pong(serial);
            }
        }
    }

    impl Dispatch<xdg_surface::XdgSurface, ()> for Window {
        fn event(
            state: &mut Self,
            surface: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                surface.ack_configure(serial);
                state.configured = true;
            }
        }
    }

    wayland_client::delegate_noop!(Window: ignore xdg_toplevel::XdgToplevel);
    wayland_client::delegate_noop!(Window: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Window: ignore wl_surface::WlSurface);
    wayland_client::delegate_noop!(Window: ignore wl_shm::WlShm);
    wayland_client::delegate_noop!(Window: ignore wl_shm_pool::WlShmPool);
    wayland_client::delegate_noop!(Window: ignore wl_buffer::WlBuffer);

    /// Show the window on `stream`; the flag holds whether it has the keyboard.
    pub fn spawn(stream: UnixStream, app_id: &'static str) -> Arc<AtomicBool> {
        let focused = Arc::new(AtomicBool::new(false));
        let theirs = Arc::clone(&focused);
        std::thread::spawn(move || {
            let conn = Connection::from_socket(stream).expect("the client connects");
            let mut queue = conn.new_event_queue();
            let qh = queue.handle();
            let mut window = Window {
                compositor: None,
                shm: None,
                wm_base: None,
                keyboard: None,
                configured: false,
                focused: theirs,
            };
            let _registry = conn.display().get_registry(&qh, ());
            if queue.roundtrip(&mut window).is_err() {
                return;
            }
            let surface = window.compositor.as_ref().unwrap().create_surface(&qh, ());
            let xdg = window
                .wm_base
                .as_ref()
                .unwrap()
                .get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_title("magenta".into());
            toplevel.set_app_id(app_id.into());
            surface.commit();
            while !window.configured {
                if queue.blocking_dispatch(&mut window).is_err() {
                    return;
                }
            }
            let (width, height) = WINDOW_SIZE;
            let len = width * height * 4;
            let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                .expect("a memfd");
            let mut file = std::fs::File::from(fd);
            let bytes: Vec<u8> = MAGENTA.iter().copied().cycle().take(len as usize).collect();
            file.write_all(&bytes).unwrap();
            let pool = window
                .shm
                .as_ref()
                .unwrap()
                .create_pool(file.as_fd(), len, &qh, ());
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                width * 4,
                wl_shm::Format::Argb8888,
                &qh,
                (),
            );
            pool.destroy();
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, width, height);
            surface.commit();
            while queue.blocking_dispatch(&mut window).is_ok() {}
        });
        focused
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

/// The area the shown panel covers: the search capsule, 640 points wide, centred, its top at about a fifth of the
/// output's height (124 points on an output 600 points tall), at scale 1.
fn panel_area() -> Area {
    Area::new(
        (OUTPUT_SIZE.0 - PANEL_AREA_WIDTH) / 2,
        PANEL_AREA_TOP,
        PANEL_AREA_WIDTH,
        PANEL_AREA_HEIGHT,
    )
}

/// How many pixels of the panel's area differ from `before`, the output as it showed with the panel hidden.
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

/// A session bus of the test's own, whose address the hosts inherit, and the address.
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
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-launcher-bus.conf");
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

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-launcher-{name}-{}", std::process::id()));
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

/// The connection that owns `org.cupertino.Launcher1` now, and its process.
fn owner(session: &Connection) -> Option<(String, u32)> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(BUS_NAME).ok()?;
    let owner = dbus.get_name_owner(name.clone()).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).ok()?;
    Some((owner.to_string(), pid))
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

/// The output once it shows the same picture for [`STILL_TURNS`] turns of the server: the window drawn, its
/// opening done.
fn still(rig: &mut Rig) -> image::RgbaImage {
    let mut last = rig.image();
    let mut same = 0;
    rig.until("the output is still", |rig| {
        let now = rig.image();
        same = if now == last { same + 1 } else { 0 };
        last = now;
        same >= STILL_TURNS
    });
    last
}

/// Open the panel, or hide it, as the server's launcher action does (`Super+Space`, the hot corner): a call of
/// `Toggle` from the server's bus worker.
fn launcher_action(rig: &mut Rig) {
    rig.state.toggle_launcher();
}

/// The area the shown panel covers part of, in pixels at scale 1: the capsule is 640 points wide and 57 tall.
const PANEL_AREA_WIDTH: i32 = 640;
const PANEL_AREA_TOP: i32 = 110;
const PANEL_AREA_HEIGHT: i32 = 90;
/// Turns of the server over which the output must not change to count as still.
const STILL_TURNS: u32 = 10;
/// How far a channel may move before a pixel counts as changed by the panel, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// Pixels of the panel's area the shown panel changes at least: a fraction of the capsule itself.
const PANEL_PIXELS: usize = 10_000;
/// The window that has the keyboard before the panel opens.
const WINDOW_APP_ID: &str = "org.cupertino.LauncherTestWindow";
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

/// Inject a crash at `at` into the launcher on its connection `name`.
fn crash(rig: &mut Rig, session: &Connection, name: &str, at: &'static str) {
    let (name, bus_call) = (name.to_owned(), session.clone());
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
    .unwrap_or_else(|e| panic!("the launcher takes a crash at {at}: {e}"));
}

/// Turn until the launcher instance `instance` runs and owns its name on a connection other than `old`; that
/// connection's name.
fn restarted(
    rig: &mut Rig,
    session: &Connection,
    old: &str,
    instance: u32,
    colours: &mut HashSet<&'static str>,
) -> (String, u32) {
    rig.until("the launcher starts again", |rig| {
        colours.extend(dock_colour(rig));
        rig.state.hosts.component_instance("launcher") == instance
            && rig.state.hosts.component_connected("launcher")
    });
    let mut now = None;
    rig.until("the restarted launcher owns its name", |_| {
        now = owner(session).filter(|(name, _)| name != old);
        now.is_some()
    });
    now.unwrap()
}

/// `Toggle` from a process that is neither the server nor the launcher: `dbus-send`, the test's child. Its error
/// name, or none when it was answered.
fn toggle_from_elsewhere(address: &str) -> Option<String> {
    let out = Process::new("dbus-send")
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .args([
            "--session",
            "--print-reply",
            &format!("--dest={BUS_NAME}"),
            OBJECT_PATH,
            &format!("{BUS_NAME}.Toggle"),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("dbus-send runs");
    if out.status.success() {
        return None;
    }
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    Some(said.trim().to_owned())
}

#[test]
fn the_launcher_opens_from_the_server_and_restarts_alone_hidden_after_injected_panics() {
    let bus = Bus::start();
    let runtime = private_dir("runtime");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    // nothing of the person's: no settings, no launch history, no files to index, no thumbnails
    std::env::set_var("XDG_CONFIG_HOME", private_dir("config"));
    std::env::set_var("XDG_DATA_HOME", private_dir("data"));
    std::env::set_var("XDG_CACHE_HOME", private_dir("cache"));
    std::env::set_var("HOME", private_dir("home"));
    let mut rig = Rig::new("launcher", &["dock", "launcher"], &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    rig.state.dbus = cupertino_server::dbus::DbusHandle::start_on(Some(bus.1.clone()), || {});
    let session = Connection::session().expect("the test's bus");
    rig.until("the dock draws and the launcher owns its name", |rig| {
        dock_colour(rig).is_some() && owner(&session).is_some()
    });
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let (mut connection, pid) = owner(&session).unwrap();
    assert_eq!(pid, host, "the name is owned in the host's process");

    // a window has the keyboard
    let stream: UnixStream = rig.client();
    let focused = focus::spawn(stream, WINDOW_APP_ID);
    rig.until("the window has the keyboard", |_| {
        focused.load(Ordering::SeqCst)
    });
    let hidden = still(&mut rig);

    // another process is refused, and the panel stays hidden
    let address = bus.1.clone();
    let refused = aside(&mut rig, "the other caller is answered", move || {
        toggle_from_elsewhere(&address)
    });
    let refused = refused.expect("a call from another process is refused");
    assert!(
        refused.contains("org.cupertino.Error."),
        "the refusal names why: {refused}"
    );
    for _ in 0..20 {
        rig.turn();
    }
    assert_eq!(
        changed(&mut rig, &hidden),
        0,
        "the refused call shows nothing"
    );
    assert!(
        focused.load(Ordering::SeqCst),
        "the window keeps the keyboard"
    );

    // the server's launcher action shows the panel, which takes the keyboard
    launcher_action(&mut rig);
    rig.until("the panel draws at the top centre", |rig| {
        changed(rig, &hidden) >= PANEL_PIXELS
    });
    rig.until("the panel takes the keyboard", |_| {
        !focused.load(Ordering::SeqCst)
    });
    assert!(
        dock_colour(&mut rig).is_some(),
        "the dock draws beside the panel"
    );

    // dropped while it shows, the component takes the panel with it, and the keyboard returns to the window
    let first_instance = rig.state.hosts.component_instance("launcher");
    let mut colours = HashSet::new();
    crash(&mut rig, &session, &connection, "dispatch");
    // the next message its loop dispatches panics: the launcher action's
    launcher_action(&mut rig);
    rig.until("the keyboard returns to the window", |_| {
        focused.load(Ordering::SeqCst)
    });
    rig.until("the panel goes with the component", |rig| {
        changed(rig, &hidden) == 0
    });
    let (name, pid) = restarted(
        &mut rig,
        &session,
        &connection,
        first_instance + 1,
        &mut colours,
    );
    assert_eq!(pid, host, "the restarted launcher runs in the same host");
    connection = name;

    let mut warm = None;
    for crash_number in 1..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash_number as usize % 3];
        crash(&mut rig, &session, &connection, at);
        if at != "helper" {
            // what it draws or dispatches next panics: the panel, asked to open
            launcher_action(&mut rig);
        }
        if (crash_number + 1) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the launcher up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("launcher")
                },
            );
            let rx = rig.restart("launcher");
            rig.answer(&rx).expect("the launcher starts again");
        }
        // each crash, or the restart after the last of a row, starts one instance
        let instance = first_instance + crash_number + 1;
        let (name, pid) = restarted(&mut rig, &session, &connection, instance, &mut colours);
        assert_eq!(pid, host, "after the restart {} at {at}", crash_number + 1);
        connection = name;
        if crash_number + 1 == WARM_RESTARTS {
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
        "the dock kept drawing while the launcher started again"
    );
    assert!(!rig.state.hosts.component_gave_up("launcher"));
    assert!(
        last <= warm + ANONYMOUS_GROWTH_KIB,
        "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts",
        RESTARTS - WARM_RESTARTS
    );

    // what a restart loses: the last instance starts hidden, with the window holding the keyboard, and opens again
    rig.until("the hidden panel's area settles", |rig| {
        changed(rig, &hidden) == 0
    });
    rig.until("the window has the keyboard", |_| {
        focused.load(Ordering::SeqCst)
    });
    launcher_action(&mut rig);
    rig.until("the restarted panel draws and takes the keyboard", |rig| {
        changed(rig, &hidden) >= PANEL_PIXELS && !focused.load(Ordering::SeqCst)
    });
    let _ = std::fs::remove_dir_all(&runtime);
}

//! The server with its shell processes: the supervisor of the `shell` configuration key, then the
//! real `cupertino-dock`, `cupertino-menubar`, `cupertino-notifications`, `cupertino-windowmanager` and `cupertino-screenshot`
//! binaries against the
//! in-process server. The server runs on an offscreen EGL device; tests that need it skip (pass)
//! when there is none, and the tests that need a session bus skip when `dbus-daemon` is missing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cupertino_input::ScreenshotRequest;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::input::{ActionPlan, ShellOp};
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use cupertino_wm::Rect;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::selection::data_device::request_data_device_client_selection;
use smithay::wayland::shm::{with_buffer_contents, BufferData};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
const TIMEOUT: Duration = Duration::from_secs(15);
/// The restart throttle of the supervisor.
const THROTTLE: Duration = Duration::from_secs(1);

// ---- server ----------------------------------------------------------------------------------

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Server {
    /// A server with the shell processes `procs` supervised. `offscreen` adds the headless output.
    /// `None` when the offscreen output is wanted and no EGL device is available.
    fn start(procs: Vec<ShellProcess>, offscreen: bool) -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let cfg = Config {
            shell_processes: procs,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if offscreen {
            if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
                eprintln!("skipping: no EGL device ({e:#})");
                return None;
            }
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        Some(Server { state, event_loop })
    }

    fn step(&mut self) {
        // without the offscreen output this only dispatches the event loop
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    /// Step the server until `cond` holds; panics after [`TIMEOUT`].
    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + TIMEOUT;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step the server until the file has `count` lines and return them; panics after [`TIMEOUT`].
    fn wait_for_lines(&mut self, file: &Path, count: usize) -> Vec<String> {
        let end = Instant::now() + TIMEOUT;
        while Instant::now() < end {
            self.step();
            let lines = read_lines(file);
            if lines.len() >= count {
                return lines;
            }
        }
        panic!("timed out waiting for {count} lines in {}", file.display());
    }
}

fn read_lines(file: &Path) -> Vec<String> {
    match std::fs::read_to_string(file) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        // the process has not written yet
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("reading {}: {e}", file.display()),
    }
}

/// A scratch directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "cupertino-shell-processes-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// A stand-in process: `script` runs under `sh` with the log file as `$1`.
fn stand_in(name: &str, script: &str, log: &Path) -> ShellProcess {
    ShellProcess {
        name: name.into(),
        command: vec![
            "sh".into(),
            "-c".into(),
            script.into(),
            "sh".into(),
            log.display().to_string(),
        ],
        roles: roles_of(name),
    }
}

// ---- supervision -----------------------------------------------------------------------------

/// The server starts every configured process and hands it the socket as `WAYLAND_DISPLAY`.
#[test]
fn every_configured_process_runs_with_the_servers_socket() {
    let dir = TempDir::new("runs");
    let (a, b) = (dir.file("a"), dir.file("b"));
    let script = r#"echo "$WAYLAND_DISPLAY" >> "$1"; exec sleep 60"#;
    let mut server = Server::start(
        vec![stand_in("a", script, &a), stand_in("b", script, &b)],
        false,
    )
    .unwrap();
    let socket = server.state.socket_name.clone().unwrap();
    assert_eq!(server.wait_for_lines(&a, 1), vec![socket.clone()]);
    assert_eq!(server.wait_for_lines(&b, 1), vec![socket]);
}

/// A process killed by SIGKILL is a failure: it comes back, but not before the throttle.
#[test]
fn a_killed_process_is_restarted_after_the_throttle() {
    let dir = TempDir::new("killed");
    let log = dir.file("log");
    let script = r#"echo $$ >> "$1"; exec sleep 60"#;
    // taken before the first poll, so it is not later than the first start
    let before_first_start = Instant::now();
    let mut server = Server::start(vec![stand_in("victim", script, &log)], false).unwrap();
    let first: libc::pid_t = server.wait_for_lines(&log, 1)[0].parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(first, libc::SIGKILL) },
        0,
        "killing the process"
    );
    let lines = server.wait_for_lines(&log, 2);
    let restarted = before_first_start.elapsed();
    let second: libc::pid_t = lines[1].parse().unwrap();
    assert_ne!(first, second);
    assert!(
        restarted >= THROTTLE,
        "restarted {restarted:?} after the first start, before the throttle of {THROTTLE:?}"
    );
}

/// A process that exits with status 0 stays stopped, while a failing one keeps coming back.
#[test]
fn a_process_that_exits_successfully_is_not_restarted() {
    let dir = TempDir::new("success");
    let (done, failing) = (dir.file("done"), dir.file("failing"));
    let mut server = Server::start(
        vec![
            stand_in("done", r#"echo run >> "$1""#, &done),
            stand_in("failing", r#"echo run >> "$1"; exit 1"#, &failing),
        ],
        false,
    )
    .unwrap();
    // the failing process cycling once proves a full throttle period passed since the first start of both
    server.wait_for_lines(&failing, 2);
    assert_eq!(read_lines(&done), vec!["run"]);
}

// ---- real shell processes ----------------------------------------------------------------------

/// The packages of the shipped binaries that are not the server's own, each a `[[bin]]` behind its `app` feature.
const BUILT_PACKAGES: [&str; 15] = [
    "cupertino-dock",
    "cupertino-menubar",
    "cupertino-notifications",
    "cupertino-windowmanager",
    "cupertino-screenshot",
    "cupertino-osd",
    "cupertino-controlcenter",
    "cupertino-launcher",
    "cupertino-login",
    "cupertino-files",
    "cupertino-settings",
    "cupertino-sessiond",
    "cupertino-netd",
    "cupertino-btd",
    "cupertino-powerd",
];

/// Build the `app` feature of the shell crates once (a no-op when current) and return the directory
/// of the binaries. They are `[[bin]]`s of other crates behind a feature, so cargo gives this test
/// no `CARGO_BIN_EXE_*` for them; a failing build or a missing binary fails the test.
fn shell_bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        // <target>/<profile>/deps/<this test> -> <target>/<profile>
        let profile_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let mut cargo =
            Command::new(std::env::var_os("CARGO").expect("CARGO is set by `cargo test`"));
        cargo.arg("build");
        for package in BUILT_PACKAGES {
            cargo.args(["-p", package]);
        }
        let features: Vec<String> = BUILT_PACKAGES
            .iter()
            .map(|package| format!("{package}/app"))
            .collect();
        cargo.args(["--features", &features.join(",")]);
        cargo.args([
            "--target-dir",
            profile_dir.parent().unwrap().to_str().unwrap(),
        ]);
        if profile_dir.file_name().unwrap() == "release" {
            cargo.arg("--release");
        }
        let status = cargo.status().unwrap();
        assert!(
            status.success(),
            "building the shell process binaries: {status}"
        );
        for name in BUILT_PACKAGES {
            assert!(
                profile_dir.join(name).is_file(),
                "missing binary {name} in {}",
                profile_dir.display()
            );
        }
        profile_dir
    })
}

/// A private session bus (`dbus-daemon --session`), killed on drop.
struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    /// `None` when `dbus-daemon` is not installed.
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        assert!(out.status.success(), "dbus-daemon failed: {}", out.status);
        let text = String::from_utf8(out.stdout).unwrap();
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next().unwrap().to_string(),
            pid: lines.next().unwrap().to_string(),
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        assert!(
            Command::new("kill")
                .arg(&self.pid)
                .status()
                .unwrap()
                .success(),
            "killing the private bus"
        );
    }
}

/// A real shell process on the private bus: `env` puts the bus address in its environment, which
/// the supervisor then passes on with `WAYLAND_DISPLAY`. The empty `CUPERTINO_ASSET_SEARCH_PATH` gives it the
/// built-in assets alone, like the test process, whatever asset packs the machine has installed.
fn real_process(binary: &str, bus: &PrivateBus) -> ShellProcess {
    let path = shell_bin_dir().join(binary);
    ShellProcess {
        name: binary.into(),
        command: vec![
            "env".into(),
            format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
            "CUPERTINO_ASSET_SEARCH_PATH=".into(),
            path.display().to_string(),
        ],
        roles: roles_of(binary),
    }
}

/// The roles the server gives the shell binaries that hold any.
fn roles_of(binary: &str) -> Vec<ShellRole> {
    match binary.trim_start_matches("cupertino-") {
        "dock" => vec![ShellRole::Session, ShellRole::Dock],
        "menubar" => vec![ShellRole::Session, ShellRole::MenuBar],
        "windowmanager" => vec![ShellRole::WindowManager],
        "login" => vec![ShellRole::Session, ShellRole::SessionLock],
        _ => vec![ShellRole::Session],
    }
}

/// [`real_process`] that sees no host application data: `XDG_DATA_DIRS`, `XDG_DATA_HOME` and `HOME` point at `empty`, so
/// it finds no desktop entry and no application icon whatever the host has installed.
fn real_process_without_host_data(binary: &str, bus: &PrivateBus, empty: &Path) -> ShellProcess {
    let mut process = real_process(binary, bus);
    let env = [
        format!("XDG_DATA_DIRS={}", empty.display()),
        format!("XDG_DATA_HOME={}", empty.display()),
        format!("HOME={}", empty.display()),
    ];
    process.command.splice(1..1, env);
    process
}

/// A test application window: one 300x200 toplevel, kept alive by its thread until the server is gone.
fn spawn_toplevel(socket: PathBuf) {
    spawn_toplevel_of(socket, "org.example.editor");
}

/// [`spawn_toplevel`] of the application `app_id`.
fn spawn_toplevel_of(socket: PathBuf, app_id: &'static str) {
    std::thread::spawn(move || {
        let conn =
            Connection::from_socket(std::os::unix::net::UnixStream::connect(socket).unwrap())
                .unwrap();
        let (globals, mut queue) = registry_queue_init::<Toplevel>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(app_id.into());
        surface.commit();
        let mut client = Toplevel {
            shm,
            surface,
            attached: false,
        };
        // dispatching ends with an error once the server drops the socket at the end of the test
        while queue.blocking_dispatch(&mut client).is_ok() {}
    });
}

struct Toplevel {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    attached: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Toplevel {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Toplevel {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Toplevel {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Toplevel {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            if !state.attached {
                state.attached = true;
                let (w, h) = (300, 200);
                let fd =
                    rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                        .unwrap();
                rustix::fs::ftruncate(&fd, (w * h * 4) as u64).unwrap();
                let pool = state
                    .shm
                    .create_pool(std::os::fd::AsFd::as_fd(&fd), w * h * 4, qh, ());
                let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
                pool.destroy();
                state.surface.attach(Some(&buffer), 0, 0);
                state.surface.damage_buffer(0, 0, w, h);
                state.surface.commit();
            }
        }
    }
}

/// `GetServerInformation` on the private bus, retried until the process owns the name; the answer
/// comes through the returned channel, so the server keeps stepping meanwhile.
fn ask_notification_server(bus: &PrivateBus) -> Receiver<(String, String, String, String)> {
    let address = bus.address.clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let end = Instant::now() + TIMEOUT;
        let conn = zbus::block_on(
            zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build(),
        )
        .unwrap();
        loop {
            let reply = zbus::block_on(conn.call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "GetServerInformation",
                &(),
            ));
            match reply {
                Ok(msg) => return tx.send(msg.body().deserialize().unwrap()).unwrap(),
                // the name is not owned yet
                Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("GetServerInformation never answered: {e}"),
            }
        }
    });
    rx
}

/// Dock, menu bar, notifications, overlay and control panel as the server's own children, with one application window.
#[test]
fn the_real_shell_processes_work_with_the_server() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut procs = [
        "cupertino-dock",
        "cupertino-menubar",
        "cupertino-notifications",
        "cupertino-osd",
        "cupertino-controlcenter",
    ]
    .map(|b| real_process(b, &bus))
    .to_vec();
    // the launcher indexes no files and finds no host application
    let empty = TempDir::new("launcher-no-host-data");
    let mut launcher = real_process_without_host_data("cupertino-launcher", &bus, &empty.0);
    launcher.command.push("--no-files".into());
    procs.push(launcher);
    let Some(mut server) = Server::start(procs, true) else {
        return;
    };
    // the overlay, the control panel and the launcher map their surfaces at start and keep running to the end; the
    // launcher draws its hidden panel at once
    server.wait_until(
        "the overlay, the control panel and the launcher to map",
        |s| {
            layer_geometry(s, "cupertino-osd").is_some()
                && layer_geometry(s, "cupertino-controlcenter").is_some()
                && committed_buffer(s, "cupertino-launcher").is_some()
        },
    );
    let panels = [
        "cupertino-osd",
        "cupertino-controlcenter",
        "cupertino-launcher",
    ]
    .map(|name| {
        server
            .state
            .supervisor
            .pid_of(name)
            .expect("the process runs")
    });
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    let answer = ask_notification_server(&bus);

    // the dock and the menu bar register on the output, and the usable area makes room for both
    server.wait_until("the dock and the menu bar to register", |s| {
        s.dock_rect(output).is_some() && s.menu_bars.has_bar(output)
    });
    let bar = server.state.config.shell.menu_bar_height;
    server.wait_until("the usable area to lose the dock zone", |s| {
        s.usable_area(output).h < f64::from(OUTPUT.1) - bar
    });
    let usable = server.state.usable_area(output);
    assert_eq!(
        (usable.x, usable.y, usable.w),
        (0.0, bar, f64::from(OUTPUT.0))
    );
    assert!(
        usable.h > 0.0 && usable.y + usable.h < f64::from(OUTPUT.1),
        "the dock zone is reserved at the bottom: {usable:?}"
    );

    // the dock process hears about the window: its bar grows by the tile of the new application
    let bar_before = server.state.dock_rect(output).unwrap();
    spawn_toplevel(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(server.state.socket_name.as_ref().unwrap()),
    );
    server.wait_until("the window to map", |s| s.wm.windows().count() == 1);
    let window = server.state.wm.windows().next().unwrap().id;
    server.wait_until("the dock bar to grow", |s| {
        s.dock_rect(output).is_some_and(|r| r.w > bar_before.w)
    });
    server.wait_until("the dock to set its minimize target", |s| {
        s.minimize_target(window).is_some()
    });
    let slot: Rect = server.state.minimize_target(window).unwrap();
    assert!(
        slot.w > 0.0 && slot.h > 0.0 && slot.y >= usable.y + usable.h,
        "the slot lies in the dock zone below the usable area: {slot:?}"
    );

    // the notifications process owns its bus name
    let mut info = None;
    server.wait_until("GetServerInformation to be answered", |_| {
        info = answer.try_recv().ok();
        info.is_some()
    });
    let (name, vendor, _version, _spec) = info.unwrap();
    assert_eq!(
        (name.as_str(), vendor.as_str()),
        ("cupertino-notifications", "Cupertino")
    );
    assert_eq!(
        ["cupertino-osd", "cupertino-controlcenter", "cupertino-launcher"]
            .map(|name| server.state.supervisor.pid_of(name)),
        panels.map(Some),
        "the overlay, the control panel and the launcher kept their connections and were not restarted"
    );
}

/// The centre puts a backdrop on every visible card and one on the Clear menu, all on one surface: a long history must
/// not end the process's connection.
#[test]
fn the_notification_centre_with_a_long_history_keeps_its_connection() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-notifications", &bus)], true)
    else {
        return;
    };
    let answer = ask_notification_server(&bus);
    server.wait_until("the notification server to answer", |_| {
        answer.try_recv().is_ok()
    });
    let pid = server
        .state
        .supervisor
        .pid_of("cupertino-notifications")
        .expect("the notifications process runs");

    let conn = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .unwrap();
    for n in 0..20 {
        let hints: std::collections::HashMap<&str, zbus::zvariant::Value> =
            std::collections::HashMap::new();
        zbus::block_on(conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "history-test",
                0u32,
                "",
                format!("Notification {n}"),
                "body",
                Vec::<&str>::new(),
                hints,
                0i32,
            ),
        ))
        .unwrap();
    }
    zbus::block_on(conn.call_method(
        Some("org.cupertino.NotificationCenter1"),
        "/org/cupertino/NotificationCenter1",
        Some("org.cupertino.NotificationCenter1"),
        "ToggleCentre",
        &(),
    ))
    .unwrap();

    let output = server.state.outputs.first().unwrap().output.clone();
    let mut most = 0;
    server.wait_until("the centre to put a backdrop behind its cards", |_| {
        most = most.max(cupertino_server::app_effects_v1::layer_backdrops(&output).len());
        most > 8
    });
    let settle = Instant::now() + Duration::from_secs(2);
    while Instant::now() < settle {
        server.step();
    }
    assert_eq!(
        server.state.supervisor.pid_of("cupertino-notifications"),
        Some(pid),
        "the notifications process kept its connection and was not restarted"
    );
}

#[test]
fn the_dock_process_applies_a_changed_dock_conf_at_once() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("dock.conf");
    std::fs::write(&conf, "tile_size = 64\n").unwrap();
    let mut dock = real_process("cupertino-dock", &bus);
    dock.command.splice(
        1..1,
        [
            format!("CUPERTINO_DOCK_CONFIG={}", conf.display()),
            format!(
                "CUPERTINO_DOCK_PINS={}",
                dir.path().join("dock-pins").display()
            ),
        ],
    );
    let Some(mut server) = Server::start(vec![dock], true) else {
        return;
    };
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    // the distance from the bottom edge that windows keep clear for the dock
    let zone = move |s: &Cupertino| {
        let u = s.usable_area(output);
        f64::from(OUTPUT.1) - (u.y + u.h)
    };
    server.wait_until("the dock zone to be reserved", move |s| zone(s) > 0.0);
    let before = zone(&server.state);
    let height_before = layer_geometry(&server.state, "cupertino-dock")
        .expect("the dock surface")
        .size
        .h;

    // the settings application writes atomically; the zone is the tile plus the padding and margin
    cupertino_defaults::atomic::write_atomic(&conf, b"tile_size = 36\nmagnification = on\n")
        .unwrap();
    server.wait_until("the dock zone to shrink with the tile size", move |s| {
        zone(s) < before
    });
    assert_eq!(
        before - zone(&server.state),
        28.0,
        "64 pt tiles became 36 pt tiles"
    );
    server.wait_until("the dock surface to be resized", |s| {
        layer_geometry(s, "cupertino-dock").is_some_and(|g| g.size.h != height_before)
    });

    // magnification alone (the path the separator menu's Turn Magnification On takes too) deepens the surface for the
    // 128 pt magnified tiles
    cupertino_defaults::atomic::write_atomic(&conf, b"tile_size = 36\n").unwrap();
    server.wait_until("magnification to be off", |s| {
        layer_geometry(s, "cupertino-dock").is_some_and(|g| g.size.h < 128)
    });
    let plain = layer_geometry(&server.state, "cupertino-dock")
        .unwrap()
        .size
        .h;
    cupertino_defaults::atomic::write_atomic(&conf, b"tile_size = 36\nmagnification = on\n")
        .unwrap();
    server.wait_until(
        "the surface to make room for the magnified tiles",
        move |s| {
            layer_geometry(s, "cupertino-dock").is_some_and(|g| g.size.h > plain && g.size.h >= 128)
        },
    );
}

/// Brightness (0 to 255) of the strongly opaque pixels, in memory order of an Argb8888 buffer
/// (blue, green, red, alpha, premultiplied).
fn opaque_brightness(bytes: &[u8], data: &BufferData) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..data.height as usize {
        for x in 0..data.width as usize {
            let at = data.offset as usize + y * data.stride as usize + x * 4;
            let [b, g, r, a] =
                [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]].map(u64::from);
            if a >= 200 {
                out.push(((b + g + r) * 255 / (3 * a)) as u8);
            }
        }
    }
    out
}

/// The committed buffer of the shell process surface with this layer namespace: its bytes and
/// layout. `None` until the process has committed one.
fn committed_buffer(state: &Cupertino, namespace: &str) -> Option<(Vec<u8>, BufferData)> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    let buffer = with_renderer_surface_state(layer.wl_surface(), |s| s.buffer().cloned())??;
    Some(
        with_buffer_contents(&buffer, |ptr, len, data| {
            let mut copy = vec![0u8; len];
            // SAFETY: `ptr` is valid for `len` bytes while the callback runs; the client may write the pool, so the
            // bytes are copied out instead of borrowed
            unsafe { std::ptr::copy_nonoverlapping(ptr, copy.as_mut_ptr(), len) };
            (copy, data)
        })
        .expect("shell processes commit shm buffers"),
    )
}

/// Position and size of the layer surface with `namespace` in output points; `None` until it is mapped.
fn layer_geometry(state: &Cupertino, namespace: &str) -> Option<Rectangle<i32, Logical>> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    map.layer_geometry(layer)
}

/// Mean brightness (0 to 255) of the strongly opaque pixels in the committed buffer of the menu bar process's
/// surface. The bar's material is the server's, so these are the process's own labels. `None` until the process
/// has committed a buffer with such pixels.
fn label_brightness(state: &Cupertino) -> Option<u8> {
    let (bytes, data) = committed_buffer(state, "cupertino-menubar")?;
    let pixels = opaque_brightness(&bytes, &data);
    (!pixels.is_empty())
        .then(|| (pixels.iter().map(|&b| u64::from(b)).sum::<u64>() / pixels.len() as u64) as u8)
}

/// The menu bar process follows the appearance the server announces: its labels are dark on the light bar and
/// light on the dark bar once the appearance flips while the process runs.
#[test]
fn the_menu_bar_process_draws_its_labels_again_when_the_appearance_changes() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-menubar", &bus)], true)
    else {
        return;
    };
    server.wait_until("dark labels on the light bar", |s| {
        label_brightness(s).is_some_and(|b| b < 100)
    });
    server.state.set_appearance(Appearance::Dark);
    server.wait_until("light labels on the dark bar", |s| {
        label_brightness(s).is_some_and(|b| b > 155)
    });
}

/// The switcher's panel is the dock process's: after the show delay it commits a panel-sized
/// buffer on its overlay surface centred on the output (whatever the dock bar and the menu bar reserve), and a
/// commit removes the surface after the fade. The application has no desktop entry and no icon of its own, so its tile
/// is the asset directory's generic application icon, pixel for pixel.
#[test]
fn the_dock_process_draws_the_switcher_panel_while_the_switch_is_open() {
    const SUPER: u32 = 125 + 8;
    const TAB: u32 = 15 + 8;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let empty = TempDir::new("switcher-no-host-data");
    let Some(mut server) = Server::start(
        vec![
            real_process_without_host_data("cupertino-dock", &bus, &empty.0),
            real_process("cupertino-menubar", &bus),
        ],
        true,
    ) else {
        return;
    };
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    server.wait_until("the dock and the menu bar to register", |s| {
        s.dock_rect(output).is_some() && s.menu_bars.has_bar(output)
    });
    let bar = server.state.config.shell.menu_bar_height;
    server.wait_until("the usable area to lose the dock zone", |s| {
        s.usable_area(output).h < f64::from(OUTPUT.1) - bar
    });
    let bar_before = server.state.dock_rect(output).unwrap();
    spawn_toplevel(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(server.state.socket_name.as_ref().unwrap()),
    );
    server.wait_until("the window to map", |s| s.wm.windows().count() == 1);
    server.wait_until("the dock bar to grow", |s| {
        s.dock_rect(output).is_some_and(|r| r.w > bar_before.w)
    });

    let key = |server: &mut Server, code: u32, pressed: bool| {
        server.state.keyboard_event_now(
            Keycode::new(code),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            0,
        );
        server.step();
    };
    key(&mut server, SUPER, true);
    key(&mut server, TAB, true);
    key(&mut server, TAB, false);
    assert!(server.state.wm.switcher().is_some());
    // one application: a 128 pt tile in a 174 × 186 pt panel
    server.wait_until("the panel to be drawn", |s| {
        committed_buffer(s, "cupertino-dock-switcher").is_some()
    });
    let (bytes, data) = committed_buffer(&server.state, "cupertino-dock-switcher").unwrap();
    assert_eq!((data.width, data.height), (174, 186));
    // the output is 1000 × 700 pt: (1000 − 174) / 2 and (700 − 186) / 2
    assert_eq!(
        layer_geometry(&server.state, "cupertino-dock-switcher"),
        Some(Rectangle::new((413, 257).into(), (174, 186).into()))
    );
    // the 128 pt tile sits at (23, 29) in the panel; wherever the generic icon is fully opaque the panel shows it. The
    // dock process was started without host data and with the built-in assets only, so its icon is the opaque
    // placeholder; the expected one is computed with the same view (the machine's icon themes must not matter)
    let assets = cupertino_assets::Assets::builtin();
    let icon = cupertino_icons::SystemIcons::without_host(&assets)
        .raster("application.generic", 128)
        .clone();
    assert_eq!((icon.width(), icon.height()), (128, 128));
    let (mut opaque, mut same) = (0, 0);
    for (i, p) in icon.pixels().iter().enumerate() {
        if p.alpha() != 255 {
            continue;
        }
        let (x, y) = (23 + i % 128, 29 + i / 128);
        let at = data.offset as usize + y * data.stride as usize + x * 4;
        let drawn = [bytes[at + 2], bytes[at + 1], bytes[at], bytes[at + 3]];
        opaque += 1;
        same += usize::from(
            drawn[3] == 255
                && drawn
                    .iter()
                    .zip([p.red(), p.green(), p.blue(), 255])
                    .all(|(a, b)| a.abs_diff(b) <= 2),
        );
    }
    assert!(
        opaque > 128 * 128 / 4,
        "the generic icon is mostly opaque, got {opaque} pixels"
    );
    assert!(
        same * 100 >= opaque * 98,
        "the tile shows the generic application icon: {same} of {opaque} opaque pixels match"
    );

    key(&mut server, SUPER, false);
    server.wait_until("the panel to fade out and go", |s| {
        committed_buffer(s, "cupertino-dock-switcher").is_none()
    });
}

/// The application grid is the dock process's alone: the forwarded action makes it map a keyboard-exclusive overlay
/// surface over the output below its 30 pt top band and above the dock bar, and draw it; Escape, delivered to that surface,
/// removes it after the exit animation.
#[test]
fn the_dock_process_opens_the_app_grid_on_the_action_and_escape_closes_it() {
    const ESCAPE: u32 = 1 + 8;
    const GRID: &str = "cupertino-dock-app-grid";
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(
        vec![
            real_process("cupertino-dock", &bus),
            real_process("cupertino-menubar", &bus),
        ],
        true,
    ) else {
        return;
    };
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    server.wait_until("the dock and the menu bar to register", |s| {
        s.dock_rect(output).is_some() && s.menu_bars.has_bar(output)
    });
    let bar = server.state.config.shell.menu_bar_height;
    server.wait_until("the usable area to lose the dock zone", |s| {
        s.usable_area(output).h < f64::from(OUTPUT.1) - bar
    });
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::ToggleAppGrid],
    });
    // the dots and the search field are drawn whatever the host's desktop entries; the entry fades them in over 0.25 s
    server.wait_until("the grid to be drawn", |s| {
        committed_buffer(s, GRID)
            .is_some_and(|(bytes, data)| opaque_brightness(&bytes, &data).len() >= 40)
    });
    let geometry = layer_geometry(&server.state, GRID).expect("the grid is mapped");
    assert_eq!(
        (geometry.loc.x, geometry.loc.y, geometry.size.w),
        (0, 30, OUTPUT.0),
        "the grid starts below the 30 pt top band, full width"
    );
    assert!(
        geometry.size.h < OUTPUT.1 - 30,
        "and stops above the dock bar: {geometry:?}"
    );
    server
        .state
        .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Pressed, 0);
    server.step();
    server
        .state
        .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Released, 0);
    server.step();
    server.wait_until("the grid to leave and go", |s| {
        committed_buffer(s, GRID).is_none()
    });
}

/// The window manager process with the stage on puts the older of two applications in the strip;
/// killed, its windows are back at once, and the restarted process places them again.
#[test]
fn killing_the_window_manager_process_puts_the_windows_back() {
    let dir = TempDir::new("windowmanager");
    let (conf, log) = (dir.file("windowmanager.conf"), dir.file("pid"));
    std::fs::write(&conf, "stage = on\n").unwrap();
    let binary = shell_bin_dir().join("cupertino-windowmanager");
    let script = format!(
        r#"echo $$ >> "$1"; CUPERTINO_WINDOWMANAGER_CONFIG={} exec {}"#,
        conf.display(),
        binary.display()
    );
    let Some(mut server) = Server::start(vec![stand_in("windowmanager", &script, &log)], true)
    else {
        return;
    };
    let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
        .join(server.state.socket_name.as_ref().unwrap());
    spawn_toplevel_of(socket.clone(), "org.example.editor");
    server.wait_until("the first window to map", |s| s.wm.windows().count() == 1);
    spawn_toplevel_of(socket, "org.example.viewer");
    server.wait_until("the second window to map", |s| s.wm.windows().count() == 2);
    let mut ids: Vec<_> = server.state.wm.windows().map(|w| w.id).collect();
    ids.sort();
    let (older, newer) = (ids[0], ids[1]);
    server.wait_until("the older window to go to the strip", |s| {
        s.window_managers.passes_input(older)
    });
    assert!(
        !server.state.window_managers.passes_input(newer),
        "the newest window is on the stage"
    );

    let first: libc::pid_t = server.wait_for_lines(&log, 1)[0].parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(first, libc::SIGKILL) },
        0,
        "killing the window manager"
    );
    server.wait_until("the windows to be released", |s| {
        !s.window_managers.passes_input(older)
    });
    let restarted: libc::pid_t = server.wait_for_lines(&log, 2)[1].parse().unwrap();
    assert_ne!(first, restarted);
    server.wait_until("the restarted process to place the window again", |s| {
        s.window_managers.passes_input(older)
    });
}

/// The overview's layout is the dock process's: on the action it maps a keyboard-exclusive overlay surface over the
/// output and answers with a frame per window, so the server's targets leave the desktop frames; killing the process
/// closes the overview and the windows return (the supervisor brings the dock back after its throttle).
#[test]
fn the_dock_process_lays_out_the_overview_and_its_death_puts_the_windows_back() {
    const OVERVIEW: &str = "cupertino-dock-overview";
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(
        vec![
            real_process("cupertino-dock", &bus),
            real_process("cupertino-menubar", &bus),
        ],
        true,
    ) else {
        return;
    };
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    server.wait_until("the dock and the menu bar to register", |s| {
        s.dock_rect(output).is_some() && s.menu_bars.has_bar(output)
    });
    let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
        .join(server.state.socket_name.as_ref().unwrap());
    spawn_toplevel(socket.clone());
    server.wait_until("the first window to map", |s| s.wm.windows().count() == 1);
    spawn_toplevel(socket);
    server.wait_until("the second window to map", |s| s.wm.windows().count() == 2);
    let desktop: Vec<(cupertino_wm::WindowId, Rect)> = server
        .state
        .wm
        .windows()
        .map(|w| (w.id, w.geometry()))
        .collect();

    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::ToggleOverview],
    });
    assert!(server.state.shell.overview_open());
    server.wait_until("the dock to draw the overview chrome", |s| {
        committed_buffer(s, OVERVIEW).is_some()
    });
    let geometry = layer_geometry(&server.state, OVERVIEW).expect("the overview layer is mapped");
    assert_eq!(
        (
            geometry.loc.x,
            geometry.loc.y,
            geometry.size.w,
            geometry.size.h
        ),
        (0, 0, OUTPUT.0, OUTPUT.1),
        "the chrome covers the output"
    );
    server.wait_until("every window to get a frame off its desktop one", |s| {
        s.shell.overview.as_ref().is_some_and(|o| {
            let targets = o.targets();
            targets.len() == 2
                && targets.iter().all(|t| {
                    desktop
                        .iter()
                        .any(|(id, frame)| *id == t.id && *frame != t.frame)
                })
        })
    });
    let targets = server.state.shell.overview.as_ref().unwrap().targets();
    assert!(
        !targets[0].frame.intersects(&targets[1].frame),
        "the dock's layout keeps the windows apart: {targets:?}"
    );

    let pid = server
        .state
        .supervisor
        .pid_of("cupertino-dock")
        .expect("the dock runs");
    assert_eq!(
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) },
        0,
        "killing the dock"
    );
    server.wait_until("the dock's death to close the overview", |s| {
        !s.shell.overview_open()
    });
    server.wait_until("the windows to settle back", |s| s.shell.overview.is_none());
    for (id, frame) in &desktop {
        let (plan, _) = cupertino_server::render::scene::plan_for(
            &server.state.wm,
            server.state.entry(*id).unwrap(),
            0.0,
            server.state.now(),
            &server.state.fx,
            server.state.display_options(),
        )
        .expect("drawn on the desktop");
        assert_eq!(
            plan.visual, *frame,
            "window {id:?} is back at its desktop frame"
        );
    }
}

/// The selection shortcut starts the screenshot process on a connection of its own: it maps its overlay over the
/// output, and a drag across it writes a PNG of the selection's size to the directory of its settings.
#[test]
fn the_screenshot_action_maps_the_overlay_and_a_drag_writes_a_png_of_the_selection() {
    const BTN_LEFT: u32 = 0x110;
    let dir = TempDir::new("screenshot");
    let (conf, shots) = (dir.file("screenshot.conf"), dir.file("shots"));
    // without the thumbnail the file is written on release instead of five seconds later
    std::fs::write(
        &conf,
        format!("location = {}\nshow-thumbnail = false\n", shots.display()),
    )
    .unwrap();
    let Some(mut server) = Server::start(Vec::new(), true) else {
        return;
    };
    let binary = shell_bin_dir().join("cupertino-screenshot");
    server.state.config.screenshot_command = vec![
        "env".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::Screenshot(ScreenshotRequest::Selection)],
    });
    server.wait_until("the overlay to be drawn", |s| {
        committed_buffer(s, "cupertino-screenshot").is_some()
    });
    assert_eq!(
        layer_geometry(&server.state, "cupertino-screenshot"),
        Some(Rectangle::new((0, 0).into(), OUTPUT.into())),
        "the overlay covers the output"
    );

    let mut time = 0;
    let mut move_to = |server: &mut Server, x: f64, y: f64| {
        time += 10;
        server.state.pointer_moved_to((x, y).into(), time, None);
        server.step();
    };
    move_to(&mut server, 100.0, 100.0);
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Pressed, 20);
    server.step();
    move_to(&mut server, 200.0, 180.0);
    move_to(&mut server, 300.0, 250.0);
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Released, 50);
    server.wait_until("the overlay to go", |s| {
        committed_buffer(s, "cupertino-screenshot").is_none()
    });

    let png = |dir: &Path| -> Option<PathBuf> {
        std::fs::read_dir(dir)
            .ok()?
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "png"))
    };
    server.wait_until("the PNG to be written", |_| png(&shots).is_some());
    let file = png(&shots).unwrap();
    let name = file.file_name().unwrap().to_str().unwrap().to_string();
    assert!(
        name.starts_with("Screenshot ") && name.contains(" at "),
        "named after the date and time: {name}"
    );
    // the PNG header: width and height are the big-endian words at bytes 16 and 20; the output's scale is 1
    let bytes = std::fs::read(&file).unwrap();
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!((word(16), word(20)), (200, 150));
}

/// The toolbar shortcut maps the overlay and the 628 × 104 toolbar centred 12 pt above the bottom; in the remembered
/// screen mode, Return captures the whole output.
#[test]
fn the_screenshot_toolbar_sits_at_the_bottom_and_return_captures_the_screen() {
    const ENTER: u32 = 28 + 8;
    let dir = TempDir::new("screenshot-toolbar");
    let (conf, shots) = (dir.file("screenshot.conf"), dir.file("shots"));
    std::fs::write(
        &conf,
        format!(
            "location = {}\nshow-thumbnail = false\nmode = screen\n",
            shots.display()
        ),
    )
    .unwrap();
    let Some(mut server) = Server::start(Vec::new(), true) else {
        return;
    };
    let binary = shell_bin_dir().join("cupertino-screenshot");
    server.state.config.screenshot_command = vec![
        "env".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::Screenshot(ScreenshotRequest::Toolbar)],
    });
    let toolbar = |s: &Cupertino| {
        let map = layer_map_for_output(&s.outputs.first()?.output);
        let geometries: Vec<_> = map
            .layers()
            .filter(|l| l.namespace() == "cupertino-screenshot")
            .filter_map(|l| map.layer_geometry(l))
            .collect();
        geometries
            .into_iter()
            .find(|g| g.size.w > 0 && g.size.w != OUTPUT.0)
    };
    server.wait_until("the toolbar to map", |s| toolbar(s).is_some());
    // (1000 − 628) / 2 and 700 − 12 − 104
    assert_eq!(
        toolbar(&server.state),
        Some(Rectangle::new((186, 584).into(), (628, 104).into()))
    );
    server.wait_until("the overlay to take the keyboard", |s| {
        s.seat.get_keyboard().unwrap().current_focus().is_some()
    });
    server
        .state
        .keyboard_event_now(Keycode::new(ENTER), KeyState::Pressed, 0);
    server.step();
    server
        .state
        .keyboard_event_now(Keycode::new(ENTER), KeyState::Released, 0);
    let png = |dir: &Path| -> Option<PathBuf> {
        std::fs::read_dir(dir)
            .ok()?
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "png"))
    };
    server.wait_until("the PNG to be written", |_| png(&shots).is_some());
    let bytes = std::fs::read(png(&shots).unwrap()).unwrap();
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!((word(16), word(20)), (OUTPUT.0 as u32, OUTPUT.1 as u32));
}

/// The screen-to-clipboard shortcut captures the output without an overlay and offers it as `image/png` on the
/// clipboard; a paste reads a PNG of the output's size.
#[test]
fn the_clipboard_screenshot_offers_a_png_of_the_output() {
    let dir = TempDir::new("screenshot-clipboard");
    let conf = dir.file("screenshot.conf");
    std::fs::write(&conf, "show-thumbnail = false\n").unwrap();
    let Some(mut server) = Server::start(Vec::new(), true) else {
        return;
    };
    let binary = shell_bin_dir().join("cupertino-screenshot");
    server.state.config.screenshot_command = vec![
        "env".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::Screenshot(ScreenshotRequest::ScreenToClipboard)],
    });
    // the paste is read on a thread while the server forwards it from the process
    let (tx, rx) = channel();
    server.wait_until("the selection to be offered", |s| {
        let (mut reader, writer) = std::io::pipe().unwrap();
        if request_data_device_client_selection(&s.seat, "image/png".into(), writer.into()).is_err()
        {
            return false;
        }
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut png = Vec::new();
            reader.read_to_end(&mut png).unwrap();
            tx.send(png).unwrap();
        });
        true
    });
    let mut png = None;
    server.wait_until("the paste to arrive", |_| {
        png = rx.try_recv().ok();
        png.is_some()
    });
    let png = png.unwrap();
    let word = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!(&png[1..4], b"PNG");
    assert_eq!((word(16), word(20)), (OUTPUT.0 as u32, OUTPUT.1 as u32));
}

/// The file manager and the settings app, started as the supervisor's children, each map a window.
#[test]
fn the_file_manager_and_the_settings_app_map_their_windows() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let empty = TempDir::new("apps-no-host-data");
    let procs = ["cupertino-files", "cupertino-settings"]
        .map(|b| real_process_without_host_data(b, &bus, &empty.0))
        .to_vec();
    let Some(mut server) = Server::start(procs, true) else {
        return;
    };
    server.wait_until("the file manager and the settings app to map", |s| {
        let mut identities: Vec<&str> =
            s.wm.windows()
                .filter_map(|w| s.wm.app(w.app))
                .map(|app| app.identity.as_str())
                .collect();
        identities.sort_unstable();
        identities == ["cupertino-files", "cupertino-settings"]
    });
}

/// Outside a login session the lock-screen process connects to the server with its roles, finds no session to lock and
/// exits with that message, a success exit the supervisor does not restart. The lock and unlock cycle needs logind and
/// is not run here.
#[test]
fn the_login_process_connects_and_exits_outside_a_login_session() {
    let dir = TempDir::new("login-stderr");
    let log = dir.file("stderr");
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut login = real_process("cupertino-login", &bus);
    login.command = vec![
        "sh".into(),
        "-c".into(),
        "exec \"$0\" 2>\"$1\"".into(),
        shell_bin_dir()
            .join("cupertino-login")
            .display()
            .to_string(),
        log.display().to_string(),
    ];
    let Some(mut server) = Server::start(vec![login], true) else {
        return;
    };
    server.wait_until("the login process to say it is in no session", |_| {
        std::fs::read_to_string(&log)
            .is_ok_and(|text| text.contains("this process is in no login session"))
    });
}

/// A daemon child that is killed on drop.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The names `bus` knows now.
fn bus_names(bus: &PrivateBus) -> Vec<String> {
    let connection = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .unwrap();
    zbus::block_on(async {
        zbus::fdo::DBusProxy::new(&connection)
            .await
            .unwrap()
            .list_names()
            .await
            .unwrap()
    })
    .into_iter()
    .map(|name| name.to_string())
    .collect()
}

/// The network, Bluetooth and power daemons start on a session bus with their authorisation off and own their bus
/// names; the power daemon reads a sysfs tree without devices, the network daemon needs no supplicant to own its
/// names. The session manager is not started: it needs root, logind and `/etc/cupertino/login.conf`.
#[test]
fn the_network_bluetooth_and_power_daemons_own_their_bus_names() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let sysfs = TempDir::new("powerd-sysfs");
    for class in ["input", "power_supply", "backlight", "leds"] {
        std::fs::create_dir_all(sysfs.file(&format!("class/{class}"))).unwrap();
    }
    let keys = TempDir::new("btd-keys");
    let daemon = |binary: &str, args: &[&str]| {
        Daemon(
            Command::new(shell_bin_dir().join(binary))
                .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the daemon starts"),
        )
    };
    let _daemons = [
        daemon("cupertino-netd", &["--session-bus", "--no-auth"]),
        daemon(
            "cupertino-btd",
            &[
                "--session-bus",
                "--no-auth",
                "--keys-dir",
                keys.0.to_str().unwrap(),
            ],
        ),
        daemon(
            "cupertino-powerd",
            &[
                "--session-bus",
                "--no-auth",
                "--sysfs-root",
                sysfs.0.to_str().unwrap(),
            ],
        ),
    ];
    let wanted = [
        "org.cupertino.Network1",
        "org.cupertino.Bluetooth1",
        "org.cupertino.Power1",
    ];
    let end = Instant::now() + TIMEOUT;
    loop {
        let names = bus_names(&bus);
        if wanted.iter().all(|name| names.iter().any(|n| n == name)) {
            return;
        }
        assert!(
            Instant::now() < end,
            "the bus names {wanted:?}: only {names:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The session manager refuses a mode it does not know. Its start without an argument is a system service (it binds
/// the greeter socket, takes a terminal and waits for logind): there it is only run where it cannot reach those, as a
/// user other than root on a host without `/etc/cupertino/login.conf`, and must fail naming that file.
#[test]
fn the_session_manager_binary_refuses_what_it_cannot_serve() {
    let sessiond = shell_bin_dir().join("cupertino-sessiond");
    let output = Command::new(&sessiond)
        .arg("no-such-mode")
        .output()
        .expect("the binary starts");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: cupertino-sessiond [worker]"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Safety: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root || Path::new("/etc/cupertino/login.conf").exists() {
        eprintln!("skipping the start: root or a login.conf would let the manager run");
        return;
    }
    let output = Command::new(&sessiond).output().expect("the binary starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cupertino-sessiond: /etc/cupertino/login.conf: "),
        "{stderr}"
    );
}

/// The server binary with `mode` (`--headless` or `--winit`): it listens on its own socket and connects its output.
/// `None` when the output cannot be made for want of an EGL device, unless `CUPERTINO_EGL_REQUIRED=1` (CI), which makes
/// that a failure.
fn start_server_binary(mode: &str) -> Option<Daemon> {
    let config = TempDir::new(&format!("server-binary{mode}"));
    let mut server = Daemon(
        Command::new(env!("CARGO_BIN_EXE_cupertino-server"))
            .args([mode, "--size", "800x600"])
            .env("XDG_CONFIG_HOME", &config.0)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the server starts"),
    );
    let stderr = std::io::BufReader::new(server.0.stderr.take().unwrap());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(stderr).map_while(std::io::Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let end = Instant::now() + TIMEOUT;
    let mut lines: Vec<String> = Vec::new();
    let has = |lines: &[String], text: &str| lines.iter().any(|l| l.contains(text));
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => lines.push(line),
            Err(RecvTimeoutError::Timeout) => {}
            // the server closed its stderr: it has ended
            Err(RecvTimeoutError::Disconnected) => {
                if has(&lines, "display connected") {
                    break;
                }
                let no_egl = lines.iter().any(|l| l.to_lowercase().contains("egl"));
                let required = std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() == Ok("1");
                assert!(
                    no_egl && !required,
                    "the server {mode} ended before it connected its output:\n{}",
                    lines.join("\n")
                );
                eprintln!("SKIPPED: no EGL device for {mode}; set CUPERTINO_EGL_REQUIRED=1 to fail instead");
                return None;
            }
        }
        if has(&lines, "listening") && has(&lines, "display connected") {
            break;
        }
        assert!(
            Instant::now() < end,
            "the server {mode} never connected its output:\n{}",
            lines.join("\n")
        );
    }
    assert!(
        server.0.try_wait().unwrap().is_none(),
        "{}",
        lines.join("\n")
    );
    Some(server)
}

/// The shipped server binary starts headless: it listens and connects its output.
#[test]
fn the_server_starts_headless() {
    let _server = start_server_binary("--headless");
}

/// The server binary as a nested session in a window of the host compositor. Only where the host has a display.
#[test]
fn the_server_starts_as_a_nested_session() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!("skipping: no host display");
        return;
    }
    let _server = start_server_binary("--winit");
}

//! The server with its shell processes: the supervisor of the `shell` configuration key, then the
//! real `cupertino-dock`, `cupertino-menubar`, `cupertino-notifications`, `cupertino-windowmanager` and `cupertino-screenshot`
//! binaries against the
//! in-process server. The server runs on an offscreen EGL device; tests that need it skip (pass)
//! when there is none, and the tests that need a session bus skip when `dbus-daemon` is missing.

use crate::common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cupertino_input::ScreenshotRequest;
use cupertino_server::app_effects_v1::layer_surface_backdrops;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{BuiltIn, Config, ShellProcess};
use cupertino_server::input::{ActionPlan, ShellOp};
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use cupertino_test_pam::PamIsolated;
use cupertino_theme::Appearance;
use cupertino_wm::Rect;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{layer_map_for_output, PopupKind, PopupManager};
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::{Display, Resource};
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::selection::data_device::request_data_device_client_selection;
use smithay::wayland::shm::{with_buffer_contents, BufferData};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
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
        Self::start_with_built_ins(procs, offscreen, Vec::new())
    }

    /// [`Server::start`] that also supervises the built-in processes with the given programs.
    fn start_with_built_ins(
        procs: Vec<ShellProcess>,
        offscreen: bool,
        built_ins: Vec<(BuiltIn, Option<PathBuf>)>,
    ) -> Option<Server> {
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
                common::egl_unavailable(format!("{e:#}"));
                return None;
            }
        }
        state.supervisor.add_built_ins(built_ins, Instant::now());
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

    /// [`common::ping_barrier`] on `client`: it has handled what it was sent, and the server has what it did with it.
    fn settle_on(&mut self, client: &smithay::wayland::shell::xdg::ShellClient) {
        let event_loop = &mut self.event_loop;
        common::ping_barrier(&mut self.state, client, |state| {
            headless::step(state, event_loop, Duration::from_millis(5)).unwrap();
        });
    }

    /// Step the server until `cond` holds; panics after [`common::HANG_GUARD`].
    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step the server until the file has `count` lines and return them; panics after [`common::HANG_GUARD`].
    fn wait_for_lines(&mut self, file: &Path, count: usize) -> Vec<String> {
        let end = Instant::now() + common::HANG_GUARD;
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

/// A stand-in process: `script` runs under `sh` with the log file as `$1`, with the empty asset search path and the
/// test's diagnostic store and journal, as a script may start a binary of the workspace.
fn stand_in(name: &str, script: &str, log: &Path) -> ShellProcess {
    ShellProcess {
        name: name.into(),
        command: [
            vec!["env".into(), "CUPERTINO_ASSET_SEARCH_PATH=".into()],
            common::events::isolated().to_vec(),
            vec![
                "sh".into(),
                "-c".into(),
                script.into(),
                "sh".into(),
                log.display().to_string(),
            ],
        ]
        .concat(),
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

/// While every process runs, nothing but its exit wakes the supervision: no timer is armed, and each child is watched.
#[test]
fn running_processes_are_watched_for_their_exit_and_no_timer_polls_them() {
    let dir = TempDir::new("watched");
    let log = dir.file("log");
    let script = r#"echo $$ >> "$1"; exec sleep 60"#;
    let mut server = Server::start(vec![stand_in("idle", script, &log)], false).unwrap();
    let pid: u32 = server.wait_for_lines(&log, 1)[0].parse().unwrap();
    assert!(server.state.supervision_wakeups.timer.is_none());
    assert!(
        server.state.supervision_wakeups.watched.contains_key(&pid),
        "{:?}",
        server
            .state
            .supervision_wakeups
            .watched
            .keys()
            .collect::<Vec<_>>()
    );
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

/// A supervised process as the session interface sends it.
type Record = (String, String, u32, u64, u64, String, bool, String, String);

/// `RestartShellProcess` over the session bus ends the process and starts it again at once, even though it ends with
/// status 0 on SIGTERM, and answers with the new process; `ListShellProcesses` lists it.
#[test]
fn a_shell_process_is_listed_and_restarted_through_the_session_interface() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let dir = TempDir::new("restart");
    let log = dir.file("log");
    // SIGTERM ends it with status 0, which alone would leave it stopped
    let script = r#"echo $$ >> "$1"; trap 'exit 0' TERM; while :; do sleep 0.05; done"#;
    let mut server = Server::start(vec![stand_in("dock", script, &log)], false).unwrap();
    cupertino_server::quit::start_session_service_on(&mut server.state, Some(bus.address.clone()));
    let first: u32 = server.wait_for_lines(&log, 1)[0].parse().unwrap();

    let address = bus.address.clone();
    let client = std::thread::spawn(move || {
        let conn = zbus::blocking::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .unwrap();
        let end = Instant::now() + common::HANG_GUARD;
        let listed = loop {
            match call_session(&conn, "ListShellProcesses", &()) {
                Ok(reply) => break reply.body().deserialize::<Vec<Record>>().unwrap(),
                Err(e) => {
                    assert!(
                        Instant::now() < end,
                        "the session interface did not appear: {e}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let restarted: Record = call_session(&conn, "RestartShellProcess", &("dock",))
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        let unknown = call_session(&conn, "RestartShellProcess", &("nope",)).unwrap_err();
        (listed, restarted, unknown.to_string())
    });
    let end = Instant::now() + common::HANG_GUARD;
    while !client.is_finished() {
        assert!(Instant::now() < end, "the client did not finish");
        server.step();
    }
    let (listed, restarted, unknown) = client.join().unwrap();

    assert_eq!(listed.len(), 1);
    let (name, state, pid, ticks, started, program, from_dev, installed, dev_program) = &listed[0];
    assert_eq!(
        (name.as_str(), state.as_str(), *pid),
        ("dock", "running", first)
    );
    assert!(*ticks > 0 && *started > 0);
    assert_eq!(
        (program.as_str(), *from_dev, installed.as_str()),
        ("env", false, "env")
    );
    assert_eq!(dev_program, "", "no development folder");

    let second: u32 = server.wait_for_lines(&log, 2)[1].parse().unwrap();
    assert_ne!(first, second);
    assert_eq!(restarted.0, "dock");
    assert_eq!(restarted.1, "running");
    assert_eq!(restarted.2, second, "the answer names the new process");
    assert!(restarted.3 >= *ticks);
    assert!(
        unknown.contains("InvalidArgs") || unknown.contains("no shell process"),
        "{unknown}"
    );
    assert_eq!(server.state.supervisor.pid_of("dock"), Some(second));
}

/// Call `method` of the server's session interface with `body`.
fn call_session<B: serde::Serialize + zbus::zvariant::DynamicType>(
    conn: &zbus::blocking::Connection,
    method: &str,
    body: &B,
) -> zbus::Result<zbus::Message> {
    conn.call_method(
        Some("org.cupertino.Session1"),
        "/org/cupertino/Session1",
        Some("org.cupertino.Session1"),
        method,
        body,
    )
}

/// The process of the Wayland client `client`, as the server identified it.
fn pid_of_client(client: &smithay::reexports::wayland_server::Client) -> Option<u32> {
    let data = client.get_data::<cupertino_server::state::ClientState>()?;
    Some(data.process()?.pid)
}

/// Whether the process `pid` has a Wayland connection the server keeps.
fn connected(state: &Cupertino, pid: u32) -> bool {
    let mut ids = Vec::new();
    state
        .display_handle
        .backend_handle()
        .with_all_clients(|id| ids.push(id));
    ids.into_iter().any(|id| {
        state
            .display_handle
            .backend_handle()
            .get_client_data(id)
            .ok()
            .and_then(|data| {
                let client = data.downcast_ref::<cupertino_server::state::ClientState>()?;
                Some(client.process()?.pid)
            })
            == Some(pid)
    })
}

/// The layer surfaces of the process `pid`, those on an output that went included until the process destroys them.
fn layer_surfaces_of(state: &Cupertino, pid: u32) -> usize {
    use smithay::reexports::wayland_server::Resource;
    state
        .layer_shell_state
        .layer_surfaces()
        .filter(|l| {
            l.wl_surface()
                .client()
                .is_some_and(|c| pid_of_client(&c) == Some(pid))
        })
        .count()
}

/// The xdg-shell client of the first popup of the layer surface with `namespace`, to ping.
fn popup_client(
    state: &Cupertino,
    namespace: &str,
) -> Option<smithay::wayland::shell::xdg::ShellClient> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    PopupManager::popups_for_surface(layer.wl_surface()).find_map(|(popup, _)| match popup {
        PopupKind::Xdg(p) => Some(p.client()),
        _ => None,
    })
}

/// The capabilities of the connection the server made for the process `pid`.
fn capabilities_of_process(
    state: &Cupertino,
    pid: u32,
) -> Option<cupertino_server::authorization::ClientCapabilitySet> {
    let mut ids = Vec::new();
    state
        .display_handle
        .backend_handle()
        .with_all_clients(|id| ids.push(id));
    ids.into_iter()
        .filter_map(|id| {
            state
                .display_handle
                .backend_handle()
                .get_client_data(id)
                .ok()
        })
        .find_map(|data| {
            let client = data.downcast_ref::<cupertino_server::state::ClientState>()?;
            (client.process()?.pid == pid).then(|| client.granted())
        })
}

/// The portal, the polkit agent and the keyring's prompter run supervised, each on a connection that holds its one role and nothing else,
/// beside a configured process that holds the roles of its entry.
#[test]
fn the_built_in_processes_run_with_exactly_their_roles() {
    use cupertino_server::authorization::ClientCapabilitySet;
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("built-ins");
    // a prefix the test's user owns, as a development install
    let bin = dir.file("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = |name: &str| {
        let path = bin.join(name);
        let log = dir.file(&format!("{name}.log"));
        let script = format!("#!/bin/sh\necho $$ >> {}\nexec sleep 60\n", log.display());
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let built_ins = vec![
        (BuiltIn::Portal, Some(program("cupertino-portal"))),
        (BuiltIn::DesktopControl, Some(program("cupertino-control"))),
        (
            BuiltIn::PolkitAgent,
            Some(program("cupertino-polkit-agent")),
        ),
        (
            BuiltIn::KeyringPrompter,
            Some(program("cupertino-keyring-prompter")),
        ),
    ];
    let log = dir.file("dock");
    let dock = stand_in("dock", r#"echo $$ >> "$1"; exec sleep 60"#, &log);
    let mut server = Server::start_with_built_ins(vec![dock], false, built_ins).unwrap();
    for (built_in, log, role) in [
        (BuiltIn::Portal, "cupertino-portal.log", ShellRole::Capture),
        (
            BuiltIn::DesktopControl,
            "cupertino-control.log",
            ShellRole::DesktopControl,
        ),
        (
            BuiltIn::PolkitAgent,
            "cupertino-polkit-agent.log",
            ShellRole::Prompt,
        ),
        (
            BuiltIn::KeyringPrompter,
            "cupertino-keyring-prompter.log",
            ShellRole::Prompt,
        ),
    ] {
        let pid: u32 = server.wait_for_lines(&dir.file(log), 1)[0].parse().unwrap();
        assert_eq!(server.state.supervisor.pid_of(built_in.name()), Some(pid));
        assert_eq!(
            capabilities_of_process(&server.state, pid),
            Some(ClientCapabilitySet::of([role])),
            "{built_in:?}"
        );
    }
    let pid: u32 = server.wait_for_lines(&log, 1)[0].parse().unwrap();
    assert_eq!(
        capabilities_of_process(&server.state, pid),
        Some(ClientCapabilitySet::of([
            ShellRole::Session,
            ShellRole::Dock
        ]))
    );
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

/// The shipped binaries prepared once before the test runner starts.
fn shell_bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let mut directory = None;
        for name in BUILT_PACKAGES {
            let path = common::fixtures::artifact(name, "bin", name);
            let parent = path.parent().unwrap().to_path_buf();
            if let Some(expected) = &directory {
                assert_eq!(&parent, expected, "shell fixture directories differ");
            }
            directory = Some(parent);
        }
        directory.unwrap()
    })
}

/// A process-private home keeps installed desktop preferences out of the shell fixtures.
fn process_home() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("shell-process-home-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    })
}
/// A real shell process on the private bus: `env` puts the bus address in its environment, which
/// the supervisor then passes on with `WAYLAND_DISPLAY`. The empty `CUPERTINO_ASSET_SEARCH_PATH` gives it the
/// built-in assets alone, like the test process, whatever asset packs the machine has installed. The test PAM shim
/// keeps it off the machine's PAM stack, journal and real buses ([`cupertino_test_pam`]).
fn real_process(binary: &str, bus: &PrivateBus) -> ShellProcess {
    let path = shell_bin_dir().join(binary);
    let mut command = vec![
        "env".into(),
        format!("HOME={}", process_home().display()),
        format!(
            "XDG_CONFIG_HOME={}",
            process_home().join("config").display()
        ),
        format!("XDG_DATA_HOME={}", process_home().join("data").display()),
        format!("XDG_CACHE_HOME={}", process_home().join("cache").display()),
        format!("CUPERTINO_SPEC_DIR={}", common::spawn::spec_dir().display()),
        format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
    ];
    command.extend(common::events::isolated());
    command.extend(cupertino_test_pam::shared().env_words());
    command.push(path.display().to_string());
    ShellProcess {
        name: binary.into(),
        command,
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

/// [`real_process_without_host_data`] whose data directory `dir` holds a desktop entry, without an icon, for the
/// application of [`spawn_toplevel`]: the dock counts only an application the desktop shows, so the window gets a tile.
fn real_process_with_editor_entry(binary: &str, bus: &PrivateBus, dir: &Path) -> ShellProcess {
    let applications = dir.join("applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(
        applications.join("org.example.editor.desktop"),
        "[Desktop Entry]\nType=Application\nName=Editor\nExec=org.example.editor\n",
    )
    .unwrap();
    real_process_without_host_data(binary, bus, dir)
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
        let end = Instant::now() + common::HANG_GUARD;
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
        "cupertino-menubar",
        "cupertino-notifications",
        "cupertino-osd",
        "cupertino-controlcenter",
    ]
    .map(|b| real_process(b, &bus))
    .to_vec();
    // the dock knows the test application from its desktop entry
    let dock_data = TempDir::new("dock-editor-entry");
    procs.insert(
        0,
        real_process_with_editor_entry("cupertino-dock", &bus, &dock_data.0),
    );
    // the launcher indexes no files and finds no host application
    let empty = TempDir::new("launcher-no-host-data");
    let mut launcher = real_process_without_host_data("cupertino-launcher", &bus, &empty.0);
    launcher.command.push("--no-files".into());
    procs.push(launcher);
    let Some(mut server) = Server::start(procs, true) else {
        return;
    };
    // the overlay and the launcher map their surfaces at start, the launcher drawing its hidden panel at once; the
    // control panel maps none until it opens. All three keep running to the end
    server.wait_until("the overlay and the launcher to map", |s| {
        layer_geometry(s, "cupertino-osd").is_some()
            && committed_buffer(s, "cupertino-launcher").is_some()
    });
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

/// The open centre puts each card on glass of its own and nothing behind its header row, whose title and clear button
/// stand on the wallpaper.
#[test]
fn the_open_notification_centre_puts_its_card_on_glass_and_no_panel_behind_its_header() {
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
    let conn = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .unwrap();
    let hints: std::collections::HashMap<&str, zbus::zvariant::Value> =
        std::collections::HashMap::new();
    zbus::block_on(conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "glass-test",
            0u32,
            "",
            "Lunch",
            "Are you free at noon?",
            Vec::<&str>::new(),
            hints,
            0i32,
        ),
    ))
    .unwrap();
    zbus::block_on(conn.call_method(
        Some("org.cupertino.NotificationCenter1"),
        "/org/cupertino/NotificationCenter1",
        Some("org.cupertino.NotificationCenter1"),
        "ToggleCentre",
        &(),
    ))
    .unwrap();

    server.wait_until("the centre to put its one card on glass", |s| {
        centre_backdrop_heights(s).is_some_and(|h| h.len() == 1 && h[0] >= 58)
    });
}

/// Heights in points of the glass the compositor draws under the open centre's surface (the notification layer that
/// spans the output's height), top to bottom. `None` while the centre is not open.
fn centre_backdrop_heights(state: &Cupertino) -> Option<Vec<usize>> {
    let output = &state.outputs.first()?.output;
    let map = layer_map_for_output(output);
    let layer = map.layers().find(|l| {
        l.namespace() == "cupertino-notifications"
            && map.layer_geometry(l).is_some_and(|g| g.size.h > 300)
    })?;
    let mut areas = layer_surface_backdrops(&map, layer);
    areas.sort_by(|a, b| a.area.loc.y.total_cmp(&b.area.loc.y));
    Some(
        areas
            .into_iter()
            .map(|b| b.area.size.h.round() as usize)
            .collect(),
    )
}

/// The centre draws every visible card and the Clear menu on one surface: a long history must not end the process's
/// connection.
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

    server.wait_until("the centre to draw its cards", |s| {
        centre_backdrop_heights(s).is_some_and(|h| !h.is_empty())
    });
    // the commit that drew the cards was handled: a request the server refuses ends the connection as it is handled
    assert!(
        connected(&server.state, pid),
        "the notifications process lost its connection"
    );
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

    // the settings application writes atomically; the zone is the bar (the tile and a padding that scales with it) and
    // its margin: 84 + 5 pt at 64 pt tiles, 47.25 + 5 pt (53 whole) at 36
    cupertino_defaults::atomic::write_atomic(&conf, b"tile_size = 36\nmagnification = on\n")
        .unwrap();
    server.wait_until("the dock zone to shrink with the tile size", move |s| {
        zone(s) < before
    });
    assert_eq!(
        before - zone(&server.state),
        36.0,
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

/// Width and height of the buffer committed by the first popup of the layer surface `namespace`; `None` without one.
fn popup_buffer_size(state: &Cupertino, namespace: &str) -> Option<(i32, i32)> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    let (popup, _) = PopupManager::popups_for_surface(layer.wl_surface()).next()?;
    let buffer = with_renderer_surface_state(popup.wl_surface(), |s| s.buffer().cloned())??;
    with_buffer_contents(&buffer, |_, _, data| (data.width, data.height)).ok()
}

/// Where the first popup of the layer surface with `namespace` sits, in output points.
fn popup_location(state: &Cupertino, namespace: &str) -> Option<Point<i32, Logical>> {
    let parent = layer_geometry(state, namespace)?;
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    let (_, offset) = PopupManager::popups_for_surface(layer.wl_surface()).next()?;
    Some(parent.loc + offset)
}

/// Position and size of the layer surface with `namespace` in output points; `None` until it is mapped.
fn layer_geometry(state: &Cupertino, namespace: &str) -> Option<Rectangle<i32, Logical>> {
    let map = layer_map_for_output(&state.outputs.first()?.output);
    let layer = map.layers().find(|l| l.namespace() == namespace)?;
    map.layer_geometry(layer)
}

/// Assert that the PNG `bytes`, a capture of the `width` x `height` area at `(x, y)` of the output at scale 1, holds
/// what the server draws there now, the pointer left out: every channel within a few levels. A capture taken while
/// one of the screenshot tool's surfaces (overlay dim, border, crosshair, toolbar) was still up would differ there.
fn assert_capture_shows_the_output(
    server: &mut Server,
    bytes: &[u8],
    (x, y, width, height): (u32, u32, u32, u32),
) {
    let shot = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8();
    assert_eq!(shot.dimensions(), (width, height));
    server.wait_until("the screenshot surfaces to go", |s| {
        layer_geometry(s, "cupertino-screenshot").is_none()
    });
    let mut backend = server.state.backend.take().expect("a backend");
    #[allow(irrefutable_let_patterns)]
    let cupertino_server::backend::Backend::Headless(ref mut h) = backend
    else {
        panic!("the headless backend")
    };
    let output = server
        .state
        .draw_output_image(&mut h.renderer, 0, false, false)
        .expect("the output drawn");
    server.state.backend = Some(backend);
    let mut worst = (0u8, (0, 0));
    for (px, py, p) in shot.enumerate_pixels() {
        let q = output.get_pixel(x + px, y + py);
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap();
        if d > worst.0 {
            worst = (d, (px, py));
        }
    }
    assert!(
        worst.0 <= 2,
        "the capture differs from the output by {} at {:?}: a surface of the tool is in it",
        worst.0,
        worst.1
    );
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

/// A click on the system menu's slot (the 28 pt after the 6 pt inset) opens its menu as a grabbing popup of the bar,
/// drawn below it; a click outside ends the grab and the popup goes.
#[test]
fn the_menu_bar_process_opens_a_menu_popup_and_a_click_outside_closes_it() {
    const BTN_LEFT: u32 = 0x110;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-menubar", &bus)], true)
    else {
        return;
    };
    server.wait_until("the bar to be drawn", |s| {
        committed_buffer(s, "cupertino-menubar").is_some()
    });
    let mut time = 0;
    let mut click = |server: &mut Server, x: f64, y: f64| {
        time += 10;
        server.state.pointer_moved_to((x, y).into(), time, None);
        server.step();
        server
            .state
            .pointer_button(BTN_LEFT, ButtonState::Pressed, time + 1);
        server.step();
        server
            .state
            .pointer_button(BTN_LEFT, ButtonState::Released, time + 2);
        server.step();
    };
    click(&mut server, 20.0, 12.0);
    server.wait_until("the system menu's popup to be drawn", |s| {
        popup_buffer_size(s, "cupertino-menubar").is_some_and(|(w, h)| w > 0 && h > 0)
    });
    // it hangs from the bar's bottom edge, under the slot that was clicked
    let bar = layer_geometry(&server.state, "cupertino-menubar").unwrap();
    let at = popup_location(&server.state, "cupertino-menubar").unwrap();
    let (width, _) = popup_buffer_size(&server.state, "cupertino-menubar").unwrap();
    assert_eq!(at.y, bar.loc.y + bar.size.h);
    assert!(
        at.x <= 20 && at.x + width > 20,
        "the popup at {at:?}, {width} wide, is under x 20"
    );
    click(&mut server, 500.0, 400.0);
    server.wait_until("the popup to go", |s| {
        popup_buffer_size(s, "cupertino-menubar").is_none()
    });
    // the server drops a dismissed popup itself; only a process that closed its menu opens it again on the next click
    click(&mut server, 20.0, 12.0);
    server.wait_until("the system menu's popup to be drawn again", |s| {
        popup_buffer_size(s, "cupertino-menubar").is_some_and(|(w, h)| w > 0 && h > 0)
    });
}

/// The menu bar process with an application window that has the keyboard. A click on empty bar space while a menu is
/// open closes the menu and reaches nothing else: no item runs (the window stays the key window and no other popup
/// opens), and the keyboard goes back to the window rather than to the bar. Hover and Right switch the open menu,
/// Escape closes it, and the keyboard comes back to the window each time. Run with `--no-capture` for the trace.
#[test]
fn a_click_on_empty_bar_space_closes_the_menu_and_the_keyboard_goes_back_to_the_window() {
    const BTN_LEFT: u32 = 0x110;
    const ESCAPE: u32 = 1 + 8;
    const RIGHT: u32 = 106 + 8;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-menubar", &bus)], true)
    else {
        return;
    };
    server.wait_until("the bar to be drawn", |s| {
        committed_buffer(s, "cupertino-menubar").is_some()
    });
    spawn_toplevel(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(server.state.socket_name.as_ref().unwrap()),
    );
    server.wait_until("the window to map and take the keyboard", |s| {
        s.wm.key_window().is_some_and(|w| {
            s.entry(w).is_some_and(|e| {
                s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(e.surface())
            })
        })
    });
    let window = server.state.wm.key_window().unwrap();
    let window_has_keyboard = |s: &Cupertino| {
        s.seat.get_keyboard().unwrap().current_focus().as_ref()
            == s.entry(window).map(|e| e.surface())
    };
    let focus = |s: &Cupertino| -> &'static str {
        let focus = s.seat.get_keyboard().unwrap().current_focus();
        if focus.as_ref() == s.entry(window).map(|e| e.surface()) {
            "the window"
        } else if focus.is_none() {
            "nothing"
        } else if s.popup_grab.is_some() {
            "the menu (its grab)"
        } else {
            "another surface"
        }
    };
    let mut time = 0;
    let mut click = |server: &mut Server, x: f64, y: f64| {
        time += 10;
        server.state.pointer_moved_to((x, y).into(), time, None);
        server.step();
        server
            .state
            .pointer_button(BTN_LEFT, ButtonState::Pressed, time + 1);
        server.step();
        server
            .state
            .pointer_button(BTN_LEFT, ButtonState::Released, time + 2);
        server.step();
    };
    let key = |server: &mut Server, code: u32| {
        for state in [KeyState::Pressed, KeyState::Released] {
            server
                .state
                .keyboard_event_now(Keycode::new(code), state, 0);
            server.step();
        }
    };
    let open = |s: &Cupertino| popup_buffer_size(s, "cupertino-menubar").is_some();

    click(&mut server, 20.0, 12.0);
    server.wait_until("the system menu to open with the grab", |s| {
        open(s) && s.popup_grab.is_some() && !window_has_keyboard(s)
    });
    let system = popup_location(&server.state, "cupertino-menubar").unwrap();
    let menubar = popup_client(&server.state, "cupertino-menubar").unwrap();
    eprintln!(
        "trace: click on the system menu at (20, 12): menu open at {system:?}, keyboard on {}",
        focus(&server.state)
    );

    // empty bar space, right of the application's menus and left of the status items
    click(&mut server, 560.0, 12.0);
    server.wait_until("the menu to close", |s| !open(s) && s.popup_grab.is_none());
    server.wait_until("the keyboard back on the window", window_has_keyboard);
    eprintln!(
        "trace: click on empty bar space at (560, 12): menu closed, grab ended, keyboard on {}, key window {:?}",
        focus(&server.state),
        server.state.wm.key_window()
    );
    // the menu bar handled the click, and what it did about it is on the server
    server.settle_on(&menubar);
    assert!(!open(&server.state), "the click opened no other menu");
    assert_eq!(server.state.wm.key_window(), Some(window));
    assert_eq!(
        server.state.wm.windows().count(),
        1,
        "no item ran on the window"
    );
    assert!(window_has_keyboard(&server.state));

    // Right moves the open menu to the next title, hovering the system menu's title brings it back, Escape closes it
    click(&mut server, 20.0, 12.0);
    server.wait_until("the system menu to open again", |s| {
        popup_location(s, "cupertino-menubar") == Some(system)
    });
    key(&mut server, RIGHT);
    server.wait_until("Right to open the next menu", |s| {
        popup_location(s, "cupertino-menubar").is_some_and(|at| at.x > system.x)
    });
    let next = popup_location(&server.state, "cupertino-menubar").unwrap();
    eprintln!(
        "trace: Right: the menu moved to {next:?}, keyboard on {}",
        focus(&server.state)
    );
    time += 10;
    server
        .state
        .pointer_moved_to((20.0, 12.0).into(), time, None);
    server.wait_until("hovering the system title to switch back", |s| {
        popup_location(s, "cupertino-menubar") == Some(system)
    });
    eprintln!("trace: hover on the system title: the menu moved back to {system:?}");
    key(&mut server, ESCAPE);
    server.wait_until("Escape to close the menu", |s| {
        !open(s) && s.popup_grab.is_none()
    });
    server.wait_until(
        "the keyboard back on the window after Escape",
        window_has_keyboard,
    );
    eprintln!(
        "trace: Escape: menu closed, keyboard on {}",
        focus(&server.state)
    );
}

/// Toolkits hide their in-window menu bar while `com.canonical.AppMenu.Registrar` has an owner, so the menu bar process
/// owns it only while it shows a bar: the name goes with the output of the last bar.
#[test]
fn the_menu_bar_process_owns_the_app_menu_registrar_only_while_it_has_a_bar() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-menubar", &bus)], true)
    else {
        return;
    };
    let conn = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .unwrap();
    let owned = || -> bool {
        zbus::block_on(conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &("com.canonical.AppMenu.Registrar",),
        ))
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
    };
    server.wait_until("the bar to be drawn", |s| {
        committed_buffer(s, "cupertino-menubar").is_some()
    });
    server.wait_until("the registrar to be owned", |_| owned());
    let output = server.state.outputs[0].id;
    server.state.remove_output(output);
    server.wait_until("the registrar to be released", |_| !owned());
}

/// Unplugging a display closes its bar before its output goes: the menu bar process drops that bar and keeps running.
#[test]
fn the_menu_bar_process_outlives_an_unplugged_display() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-menubar", &bus)], true)
    else {
        return;
    };
    let display = headless::HeadlessDisplay {
        name: "headless-2".into(),
        size: (800, 600),
        physical_mm: (0, 0),
    };
    let head = headless::add_display(&mut server.state, &display, None).unwrap();
    server.wait_until("a bar on each display", |s| {
        s.outputs.len() == 2
            && s.outputs.iter().all(|o| {
                layer_map_for_output(&o.output)
                    .layers()
                    .any(|l| l.namespace() == "cupertino-menubar")
            })
    });
    let pid = server.state.supervisor.pid_of("cupertino-menubar");
    let bars = layer_surfaces_of(&server.state, pid.unwrap());
    headless::remove_display(&mut server.state, head).unwrap();
    // the process heard that the bar of the display that went is closed, and destroyed it
    server.wait_until("the menu bar to destroy the closed bar", |s| {
        layer_surfaces_of(s, pid.unwrap()) == bars - 1
    });
    assert!(connected(&server.state, pid.unwrap()));
    assert_eq!(server.state.supervisor.pid_of("cupertino-menubar"), pid);
    assert!(committed_buffer(&server.state, "cupertino-menubar").is_some());
}

// ---- the control panel and its output ---------------------------------------------------------------------------

/// Layer namespace of the control panel's card and catcher.
const PANEL: &str = "cupertino-controlcenter";

/// One of the control panel's layer surfaces, on an output that exists.
struct PanelSurface {
    /// The card (the overlay layer), else the catcher.
    card: bool,
    /// Index of its output in `Cupertino::outputs`.
    output: usize,
    surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    /// Where it is in global points.
    rect: Rectangle<i32, Logical>,
}

/// The control panel's layer surfaces on every output.
fn panel_surfaces(state: &Cupertino) -> Vec<PanelSurface> {
    let mut out = Vec::new();
    for (output, entry) in state.outputs.iter().enumerate() {
        let map = layer_map_for_output(&entry.output);
        let origin = entry.output.current_location();
        for layer in map.layers().filter(|l| l.namespace() == PANEL) {
            let Some(geometry) = map.layer_geometry(layer) else {
                continue;
            };
            out.push(PanelSurface {
                card: layer.layer() == smithay::wayland::shell::wlr_layer::Layer::Overlay,
                output,
                surface: layer.wl_surface().clone(),
                rect: Rectangle::new(origin + geometry.loc, geometry.size),
            });
        }
    }
    out
}

/// The card, once it has committed a buffer.
fn drawn_card(state: &Cupertino) -> Option<PanelSurface> {
    panel_surfaces(state)
        .into_iter()
        .find(|p| p.card && surface_bytes(&p.surface).is_some())
}

/// The bytes of the buffer committed on `surface`; `None` before its first.
fn surface_bytes(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Option<Vec<u8>> {
    let buffer = with_renderer_surface_state(surface, |s| s.buffer().cloned())??;
    with_buffer_contents(&buffer, |ptr, len, _| {
        let mut copy = vec![0u8; len];
        // SAFETY: `ptr` is valid for `len` bytes while the callback runs; the bytes are copied out
        unsafe { std::ptr::copy_nonoverlapping(ptr, copy.as_mut_ptr(), len) };
        copy
    })
    .ok()
}

/// `Toggle` on the control panel's bus name, retried until the process owns it; the answer comes through the returned
/// channel, so the server keeps stepping meanwhile.
fn toggle_panel(bus: &PrivateBus) -> Receiver<()> {
    let address = bus.address.clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let end = Instant::now() + common::HANG_GUARD;
        let conn = zbus::block_on(
            zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build(),
        )
        .unwrap();
        loop {
            let reply = zbus::block_on(conn.call_method(
                Some("org.cupertino.ControlPanel1"),
                "/org/cupertino/ControlPanel1",
                Some("org.cupertino.ControlPanel1"),
                "Toggle",
                &(),
            ));
            match reply {
                Ok(_) => return tx.send(()).unwrap(),
                // the name is not owned yet
                Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("Toggle never answered: {e}"),
            }
        }
    });
    rx
}

impl Server {
    /// `Toggle`, answered. The panel answers once it has queued the call and carries it out afterwards, so the caller
    /// waits for what the toggle leads to.
    fn toggle_panel(&mut self, bus: &PrivateBus) {
        let answer = toggle_panel(bus);
        self.wait_until("Toggle to be answered", |_| answer.try_recv().is_ok());
    }

    /// `Toggle` while the server has no output, carried out by the panel (the process `pid`, with no surface left): the
    /// panel opens its surfaces, the server closes them at once for want of an output, and the panel, hidden again,
    /// destroys them. Only then does the next toggle open the panel instead of dismissing it. The surfaces are watched
    /// from before the call, as the panel may be done before its answer is seen. The creation cannot be missed between
    /// server steps: the client can destroy the surfaces only after the server flushes their close events at the end
    /// of the step that received their creation.
    fn toggle_panel_with_no_output(&mut self, bus: &PrivateBus, pid: u32) {
        assert!(self.state.outputs.is_empty());
        assert_eq!(layer_surfaces_of(&self.state, pid), 0);
        let answer = toggle_panel(bus);
        let (mut answered, mut opened) = (false, false);
        self.wait_until(
            "the panel to open on no output and take its surfaces back",
            |s| {
                answered |= answer.try_recv().is_ok();
                let open = layer_surfaces_of(s, pid);
                opened |= open > 0;
                answered && opened && open == 0
            },
        );
    }

    fn keyboard_focus(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.seat.get_keyboard().unwrap().current_focus()
    }

    /// Move the pointer to `at`, global points, and press and release the left button there.
    fn click_at(&mut self, at: Point<i32, Logical>) {
        const BTN_LEFT: u32 = 0x110;
        self.state
            .pointer_moved_to((f64::from(at.x), f64::from(at.y)).into(), 0, None);
        self.step();
        self.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        self.step();
        self.state
            .pointer_button(BTN_LEFT, ButtonState::Released, 0);
        self.step();
    }

    /// The head that shows the output at `index`.
    fn head_of(&self, index: usize) -> cupertino_server::displays::HeadId {
        let output = self.state.outputs[index].id;
        self.state
            .displays
            .heads
            .iter()
            .find(|h| h.output == Some(output))
            .expect("the head of the output")
            .id
    }

    /// Open the control panel and wait until its card is drawn with the keyboard; returns the card.
    fn open_panel(&mut self, bus: &PrivateBus) -> PanelSurface {
        self.toggle_panel(bus);
        self.wait_until("the card to be drawn with the keyboard", |s| {
            drawn_card(s).is_some_and(|card| {
                s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(&card.surface)
            })
        });
        drawn_card(&self.state).unwrap()
    }

    /// Wait until the panel's surfaces are destroyed and none of them holds the keyboard or the pointer grab.
    fn wait_panel_gone(&mut self, what: &str, surfaces: &[PanelSurface]) {
        use smithay::reexports::wayland_server::Resource;
        self.wait_until(what, |s| {
            surfaces.iter().all(|p| !p.surface.is_alive())
                && panel_surfaces(s).is_empty()
                && s.exclusive_layer.is_none()
                && s.popup_grab.is_none()
        });
    }
}

/// Toggling the card's Bluetooth switch and pressing its sound slider at `sound_x` reach the panel's (in-memory)
/// backends: the card draws the new state, and still does after the panel has read its backends again. The level goes
/// where the press is, so each call presses at another point.
fn interact_with_card(server: &mut Server, card: &PanelSurface, sound_x: i32) {
    /// Centre of the Bluetooth switch and the line of the sound slider's track, in card points (the panel's tiles
    /// start 8 points in from the window's corner).
    const BLUETOOTH: (i32, i32) = (40, 116);
    /// Where the window's corner lies in the card's surface, inside the margin its shadow is drawn in: a press in that
    /// margin is outside the window and dismisses the panel.
    const WINDOW_ORIGIN: (i32, i32) = (122, 24);
    let sound_track = (sound_x, 430);
    use smithay::reexports::wayland_server::Resource;
    /// Observe the selected state through the controller's one-second backend refresh. An unchanged reading produces
    /// no new Wayland frame, so time is the subject of this check: keep dispatching and checking the exact card and
    /// its pixels for this interval after its input frames settle, rather than sleeping until a frame is assumed ready.
    const BACKEND_REFRESH: Duration = Duration::from_millis(1500);
    let at = |(x, y): (i32, i32)| {
        card.rect.loc + Point::from((WINDOW_ORIGIN.0 + x, WINDOW_ORIGIN.1 + y))
    };
    let frame = || {
        assert!(
            card.surface.is_alive(),
            "the card was destroyed while its controls were being exercised"
        );
        surface_bytes(&card.surface)
    };
    for (what, point) in [("switch", BLUETOOTH), ("slider", sound_track)] {
        let before = frame().expect("the open card has a committed frame");
        server.click_at(at(point));
        let mut after = before.clone();
        server.wait_until("the card to draw the new state", |_| {
            if let Some(bytes) = frame() {
                after = bytes;
            }
            after != before
        });
        let mut changed = Instant::now();
        server.wait_until(
            "the selected state to persist through a backend refresh",
            |_| {
                let Some(bytes) = frame() else {
                    return false;
                };
                if bytes != after {
                    after = bytes;
                    changed = Instant::now();
                    return false;
                }
                changed.elapsed() >= BACKEND_REFRESH
            },
        );
        assert!(
            after != before,
            "the backend kept the {what}'s new state through the refresh"
        );
    }
}

/// The commit of the card's surface that last carried a buffer or damage.
fn buffer_commit(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Option<smithay::backend::renderer::utils::CommitCounter> {
    with_renderer_surface_state(surface, |s| s.current_commit())
}

/// The control panel's fade in and out runs on the compositor's clock, as the card's presentation: while it fades the
/// card draws no frame of the fade (at most a single one after its first while it opens), and
/// its opacity rises and falls on the server. Run with
/// `--no-capture` for the counts.
#[test]
fn the_control_panel_fades_on_the_compositor_s_clock_without_a_new_buffer() {
    use cupertino_server::layers_v1::{presentation, settled};
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut panel = real_process(PANEL, &bus);
    panel.command.push("--fake-backends".into());
    let Some(mut server) = Server::start(vec![panel], true) else {
        return;
    };
    let _answer = toggle_panel(&bus);
    server.wait_until("the card's first frame", |s| drawn_card(s).is_some());
    let card = drawn_card(&server.state).unwrap();
    let first = buffer_commit(&card.surface);
    // the fade in: sampled as the scene shows it, until the presentation settles
    let (mut between, mut frames) = (0, 0);
    let mut new_buffers = 0;
    let mut last = first;
    server.wait_until("the fade in to settle", |s| {
        let now = s.now();
        let shown = presentation(&card.surface, now, false).expect("the card has a presentation");
        frames += 1;
        if shown.opacity > 0.0 && shown.opacity < 1.0 {
            between += 1;
        }
        let commit = buffer_commit(&card.surface);
        if commit != last {
            new_buffers += 1;
            last = commit;
        }
        shown.opacity == 1.0 && settled(&card.surface, now)
    });
    eprintln!(
        "trace: fade in: {frames} server steps, {between} of them part way, {new_buffers} new buffers of the card"
    );
    assert!(between > 0, "the fade in ran on the server");
    // the card may draw once more after its first frame as it opens; never a frame per step of the fade
    assert!(
        new_buffers <= 1,
        "the card drew {new_buffers} frames while it faded in"
    );
    // the fade out after Escape, until the card goes
    const ESCAPE: u32 = 1 + 8;
    for state in [KeyState::Pressed, KeyState::Released] {
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), state, 0);
        server.step();
    }
    let (mut between, mut new_buffers) = (0, 0);
    let mut last = buffer_commit(&card.surface);
    server.wait_until("the card to go after its fade out", |s| {
        if !card.surface.is_alive() {
            return true;
        }
        if let Some(shown) = presentation(&card.surface, s.now(), false) {
            if shown.opacity > 0.0 && shown.opacity < 1.0 {
                between += 1;
            }
        }
        let commit = buffer_commit(&card.surface);
        if commit != last {
            new_buffers += 1;
            last = commit;
        }
        false
    });
    eprintln!(
        "trace: fade out: {between} server steps part way, {new_buffers} new buffers of the card"
    );
    assert!(between > 0, "the fade out ran on the server");
    assert_eq!(new_buffers, 0, "the card drew no frame while it faded out");
}

/// The control panel open on an output that is unplugged is dismissed at once: its surfaces go, the keyboard goes back
/// to the window and nothing keeps the pointer. The process keeps running (it is not restarted), and the panel opens
/// again on the remaining output with its switches and sliders acting. With the last output gone a toggle opens
/// nothing; once a display is plugged in again the panel opens there. Run with `--no-capture` for the trace.
#[test]
fn the_control_panel_goes_with_its_output_and_opens_again_on_the_remaining_or_reconnected_one() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut panel = real_process(PANEL, &bus);
    panel.command.push("--fake-backends".into());
    let Some(mut server) = Server::start(vec![panel], true) else {
        return;
    };
    spawn_toplevel(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(server.state.socket_name.as_ref().unwrap()),
    );
    server.wait_until("the window to map and take the keyboard", |s| {
        s.wm.key_window().is_some_and(|w| {
            s.entry(w).is_some_and(|e| {
                s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(e.surface())
            })
        })
    });
    let window = server.state.wm.key_window().unwrap();
    let window_surface = server.state.entry(window).unwrap().surface().clone();
    let second = headless::HeadlessDisplay {
        name: "headless-2".into(),
        size: (800, 600),
        physical_mm: (0, 0),
    };
    headless::add_display(&mut server.state, &second, None).unwrap();
    server.wait_until("two outputs", |s| s.outputs.len() == 2);

    // open on the first output, which is then unplugged
    let card = server.open_panel(&bus);
    let pid = server
        .state
        .supervisor
        .pid_of(PANEL)
        .expect("the panel runs");
    let opened = panel_surfaces(&server.state);
    assert_eq!(opened.len(), 2, "the card and the catcher");
    assert!(
        opened.iter().all(|p| p.output == 0),
        "both on the first output"
    );
    eprintln!(
        "trace: Toggle: card at {:?} on output 0 of 2, keyboard on the card",
        card.rect
    );
    let head = server.head_of(0);
    headless::remove_display(&mut server.state, head).unwrap();
    server.wait_panel_gone("the panel to go with its output", &opened);
    server.wait_until("the keyboard back on the window", |s| {
        s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(&window_surface)
    });
    eprintln!(
        "trace: output 0 unplugged: card and catcher destroyed, no exclusive layer, no popup grab, keyboard on the window"
    );
    // the panel destroyed its surfaces when it heard they were closed: it has handled the unplug
    assert!(
        panel_surfaces(&server.state).is_empty(),
        "nothing reopens by itself"
    );
    assert!(connected(&server.state, pid));
    assert_eq!(
        server.state.supervisor.pid_of(PANEL),
        Some(pid),
        "no restart"
    );

    // the remaining output: open, act, dismiss with Escape
    let card = server.open_panel(&bus);
    assert!(panel_surfaces(&server.state).iter().all(|p| p.output == 0));
    eprintln!(
        "trace: Toggle: card at {:?} on the remaining output, keyboard on the card",
        card.rect
    );
    interact_with_card(&mut server, &card, 208);
    eprintln!("trace: Bluetooth switch and sound slider drew their new state through a refresh");
    const ESCAPE: u32 = 1 + 8;
    for state in [KeyState::Pressed, KeyState::Released] {
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), state, 0);
        server.step();
    }
    let shown = panel_surfaces(&server.state);
    server.wait_panel_gone("Escape to close the panel", &shown);
    server.wait_until("the keyboard back on the window after Escape", |s| {
        s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(&window_surface)
    });

    // the last output: unplugged while the panel is open, then a toggle with no output
    server.open_panel(&bus);
    let opened = panel_surfaces(&server.state);
    let head = server.head_of(0);
    headless::remove_display(&mut server.state, head).unwrap();
    server.wait_until("no output", |s| s.outputs.is_empty());
    server.wait_panel_gone("the panel to go with the last output", &opened);
    let focus = server.keyboard_focus();
    assert!(
        opened.iter().all(|p| focus.as_ref() != Some(&p.surface)),
        "no surface of the panel keeps the keyboard"
    );
    eprintln!(
        "trace: last output unplugged: card and catcher destroyed, keyboard on {}",
        if focus.as_ref() == Some(&window_surface) {
            "the window"
        } else {
            "no panel surface"
        }
    );
    // the panel has carried the toggle out, not only answered it: one still queued when the display below is plugged
    // in would open the panel there, and the toggle after it would dismiss it
    server.toggle_panel_with_no_output(&bus, pid);
    assert!(connected(&server.state, pid));
    assert_eq!(
        server.state.supervisor.pid_of(PANEL),
        Some(pid),
        "a toggle with no output neither fails nor restarts the panel"
    );
    eprintln!("trace: Toggle with no output: nothing opened, the process kept running");

    // a display plugged in again
    let third = headless::HeadlessDisplay {
        name: "headless-3".into(),
        size: (1000, 700),
        physical_mm: (0, 0),
    };
    headless::add_display(&mut server.state, &third, None).unwrap();
    server.wait_until("the reconnected output", |s| s.outputs.len() == 1);
    let card = server.open_panel(&bus);
    eprintln!(
        "trace: display plugged in, Toggle: card at {:?} on it, keyboard on the card",
        card.rect
    );
    interact_with_card(&mut server, &card, 80);
    eprintln!("trace: Bluetooth switch and sound slider drew their new state through a refresh");
    assert_eq!(
        server.state.supervisor.pid_of(PANEL),
        Some(pid),
        "the same process throughout"
    );
}

/// Each output holds one dock role, the bar's or an edge strip's. Resting the pointer on the second display's strip
/// moves the bar there and a strip to the first display, and unplugging the display the bar is on moves it back: the
/// process keeps running through both, so no role was asked for on an output whose old one was still held.
#[test]
fn the_dock_process_moves_between_displays_and_keeps_running() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-dock", &bus)], true) else {
        return;
    };
    let first = server.state.outputs[0].id;
    server.wait_until("the dock to draw its bar", |s| s.dock_rect(first).is_some());
    let display = headless::HeadlessDisplay {
        name: "headless-2".into(),
        size: (800, 600),
        physical_mm: (0, 0),
    };
    let head = headless::add_display(&mut server.state, &display, None).unwrap();
    // the strip is drawn, so the pointer resting on it has it under it
    let strip_drawn = |s: &Cupertino, output: usize| {
        s.outputs.get(output).is_some_and(|o| {
            layer_map_for_output(&o.output).layers().any(|l| {
                l.namespace() == "cupertino-dock-edge"
                    && with_renderer_surface_state(l.wl_surface(), |r| r.buffer().is_some())
                        .unwrap_or(false)
            })
        })
    };
    server.wait_until("an edge strip on the second display", |s| strip_drawn(s, 1));
    let pid = server.state.supervisor.pid_of("cupertino-dock");
    let second = server.state.outputs[1].id;
    let rect = server.state.outputs[1].rect;
    // the strip is the display's bottom row
    let at = (rect.x + rect.w / 2.0, rect.y + rect.h - 0.5);
    server.state.pointer_moved_to(at.into(), 10, None);
    server.step();
    server.wait_until("the bar on the second display", |s| {
        s.dock_rect(second).is_some()
    });
    server.wait_until("an edge strip on the first display", |s| strip_drawn(s, 0));
    assert_eq!(server.state.supervisor.pid_of("cupertino-dock"), pid);

    headless::remove_display(&mut server.state, head).unwrap();
    server.wait_until("the bar back on the first display", |s| {
        s.dock_rect(first).is_some()
    });
    assert_eq!(server.state.supervisor.pid_of("cupertino-dock"), pid);
}

/// An application a shell process starts (the launcher or the dock opening an application) is an application, not a
/// part of the shell: its process connects to the public socket and its window gets a dock tile. The stand-in launcher
/// drops its own connection from the child's environment as a shell binary's Wayland client does on connecting.
#[test]
fn an_application_a_shell_process_starts_gets_a_dock_tile() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let dir = TempDir::new("launched-application");
    let data = TempDir::new("launched-editor-entry");
    let client = common::fixtures::artifact("cupertino-testclient", "bin", "cupertino-testclient");
    let script = format!(
        r#"env -u WAYLAND_SOCKET '{}' --app-id org.example.editor --title Editor --output "$1" & wait"#,
        client.display()
    );
    let Some(mut server) = Server::start(
        vec![
            real_process_with_editor_entry("cupertino-dock", &bus, &data.0),
            stand_in("cupertino-launcher", &script, &dir.file("typed")),
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
    server.wait_until("the dock to register", |s| s.dock_rect(output).is_some());
    let bar_before = server.state.dock_rect(output).unwrap();
    server.wait_until("the launched application to map", |s| {
        s.wm.windows().count() == 1
    });
    let launcher = server
        .state
        .supervisor
        .pid_of("cupertino-launcher")
        .expect("the stand-in launcher runs");
    let window = server.state.wm.windows().next().unwrap().id;
    let pid = server
        .state
        .entry(window)
        .and_then(|e| e.surface().client())
        .and_then(|c| {
            c.get_data::<cupertino_server::state::ClientState>()
                .filter(|d| d.capabilities.is_empty())
                .and_then(|d| d.process())
                .map(|p| p.pid)
        });
    assert!(
        pid.is_some_and(|pid| pid != launcher),
        "the window is the child's, on a public connection holding nothing: {pid:?} (launcher {launcher})"
    );
    server.wait_until("the dock bar to grow by the application's tile", |s| {
        s.dock_rect(output).is_some_and(|r| r.w > bar_before.w)
    });
}

/// The switcher's panel is the dock process's: after the show delay it commits a panel-sized
/// buffer on its overlay surface centred on the output (whatever the dock bar and the menu bar reserve), and a
/// commit removes the surface after the fade. The application has a desktop entry but no icon of its own, so its tile
/// is the asset directory's generic application icon, pixel for pixel.
#[test]
fn the_dock_process_draws_the_switcher_panel_while_the_switch_is_open() {
    const SUPER: u32 = 125 + 8;
    const TAB: u32 = 15 + 8;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let data = TempDir::new("switcher-editor-entry");
    let Some(mut server) = Server::start(
        vec![
            real_process_with_editor_entry("cupertino-dock", &bus, &data.0),
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

/// A right click on the bar opens its menu as a popup of the bar surface. The surface is at the bottom of the output and
/// the menu would open below what was clicked, so the server flips it above, inside the output.
#[test]
fn the_dock_process_opens_a_menu_above_what_was_clicked_inside_the_output() {
    const BTN_RIGHT: u32 = 0x111;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(mut server) = Server::start(vec![real_process("cupertino-dock", &bus)], true) else {
        return;
    };
    let output = server
        .state
        .outputs
        .first()
        .expect("the headless output")
        .id;
    server.wait_until("the dock to draw its bar", |s| {
        s.dock_rect(output).is_some()
    });
    let bar = server.state.dock_rect(output).unwrap();
    let (x, y) = (bar.x + bar.w / 2.0, bar.y + bar.h / 2.0);
    server.state.pointer_moved_to((x, y).into(), 10, None);
    server.step();
    server
        .state
        .pointer_button(BTN_RIGHT, ButtonState::Pressed, 11);
    server.step();
    server
        .state
        .pointer_button(BTN_RIGHT, ButtonState::Released, 12);
    server.step();
    server.wait_until("the menu to be drawn", |s| {
        popup_buffer_size(s, "cupertino-dock").is_some_and(|(w, h)| w > 0 && h > 0)
    });
    let at = popup_location(&server.state, "cupertino-dock").unwrap();
    let (width, height) = popup_buffer_size(&server.state, "cupertino-dock").unwrap();
    assert!(
        at.y >= 0 && f64::from(at.y + height) <= y,
        "the menu at {at:?}, {height} tall, is above what was clicked at y {y} on the bar at {bar:?}"
    );
    assert!(
        at.x >= 0 && at.x + width <= OUTPUT.0,
        "the menu at {at:?}, {width} wide, is inside the output"
    );
}

/// The launcher's layer whose window is `width` points wide (the panel; its catcher covers the output and its hidden
/// window is a point).
fn launcher_panel(state: &Cupertino, width: i32) -> bool {
    let Some(output) = state.outputs.first() else {
        return false;
    };
    let map = layer_map_for_output(&output.output);
    let found = map
        .layers()
        .filter(|l| l.namespace() == "cupertino-launcher")
        .filter_map(|l| map.layer_geometry(l))
        .any(|g| g.size.w == width);
    found
}

/// The app-grid action opens the launcher process's application browser (the server calls it on the session bus), a
/// panel wider than the search field's; Escape folds it into the search field, and a second Escape closes it.
#[test]
fn the_app_grid_action_opens_the_launcher_s_browser_and_escape_folds_then_closes_it() {
    const ESCAPE: u32 = 1 + 8;
    // the panels' windows: the browser and the search field, each with the room for its shadow
    const BROWSER: i32 = 844 + 120;
    const SEARCH: i32 = 640 + 120;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let empty = TempDir::new("launcher-browser-no-host-data");
    let mut launcher = real_process_without_host_data("cupertino-launcher", &bus, &empty.0);
    launcher.command.push("--no-files".into());
    let Some(mut server) = Server::start(vec![launcher], true) else {
        return;
    };
    // the server asks the launcher on the session bus, the test's own
    server.state.dbus = Some(
        cupertino_server::dbus::DbusHandle::start_on(Some(bus.address.clone()), || {})
            .expect("the control bus worker"),
    );
    let connection = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .expect("connect to the test bus");
    let owned = || {
        zbus::block_on(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &"org.cupertino.Launcher1",
        ))
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
    };
    server.wait_until(
        "the launcher to map its hidden window and own its name",
        |s| committed_buffer(s, "cupertino-launcher").is_some() && owned(),
    );
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::ToggleAppGrid],
    });
    server.wait_until("the browser to open", |s| launcher_panel(s, BROWSER));
    let escape = |server: &mut Server| {
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Pressed, 0);
        server.step();
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Released, 0);
        server.step();
    };
    escape(&mut server);
    server.wait_until("the browser to fold into the search field", |s| {
        launcher_panel(s, SEARCH) && !launcher_panel(s, BROWSER)
    });
    escape(&mut server);
    server.wait_until("the panel to close", |s| {
        !launcher_panel(s, SEARCH) && !launcher_panel(s, BROWSER)
    });
}

/// The launcher's window `width` points wide, with its layers: the window's surface and each layer's, the layers in
/// the order they were made.
fn launcher_surfaces(
    state: &Cupertino,
    width: i32,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Vec<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
)> {
    let output = state.outputs.first()?;
    let map = layer_map_for_output(&output.output);
    let panel = map
        .layers()
        .filter(|l| l.namespace() == "cupertino-launcher")
        .find(|l| map.layer_geometry(l).is_some_and(|g| g.size.w == width))?
        .wl_surface()
        .clone();
    let layers = cupertino_server::layers_v1::with_parent(&panel, |layers| {
        layers
            .iter()
            .filter(|l| l.surface.is_alive() && !l.whole)
            .map(|l| l.surface.clone())
            .collect()
    });
    Some((panel, layers))
}

/// The launcher's search field at rest closes on the compositor's clock: the field draws itself as layers once it
/// has settled, and from Escape until the panel goes neither its window nor any of its layers commits a buffer while
/// the compositor fades them part way. Run with `--no-capture` for the counts.
#[test]
fn the_launcher_closes_on_the_compositor_s_clock_without_a_new_buffer() {
    use cupertino_server::layers_v1::{present, settled};
    const ESCAPE: u32 = 1 + 8;
    // the search field's window, with the room for its shadow
    const SEARCH: i32 = 640 + 120;
    // the field's glass, its edge, its content and the mode button's glass and face
    const LAYERS: usize = 5;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let empty = TempDir::new("launcher-close-no-host-data");
    let mut launcher = real_process_without_host_data("cupertino-launcher", &bus, &empty.0);
    launcher.command.push("--no-files".into());
    let Some(mut server) = Server::start(vec![launcher], true) else {
        return;
    };
    server.state.dbus = Some(
        cupertino_server::dbus::DbusHandle::start_on(Some(bus.address.clone()), || {})
            .expect("the control bus worker"),
    );
    server.wait_until("the launcher to map its hidden window", |s| {
        committed_buffer(s, "cupertino-launcher").is_some()
    });
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::ToggleLauncher],
    });
    // the field opens drawn by the application, then rests as layers
    server.wait_until("the search field to rest as layers", |s| {
        launcher_surfaces(s, SEARCH).is_some_and(|(_, layers)| {
            layers.len() == LAYERS && layers.iter().all(|l| buffer_commit(l).is_some())
        })
    });
    let (panel, layers) = launcher_surfaces(&server.state, SEARCH).unwrap();
    let commits = |surfaces: &[_]| -> Vec<_> { surfaces.iter().map(buffer_commit).collect() };
    let all: Vec<_> = std::iter::once(panel.clone()).chain(layers).collect();
    // the last frames of the switch to layers are let through
    let mut last = commits(&all);
    let quiet_since = Instant::now();
    let mut quiet = Instant::now();
    server.wait_until("the layers to stop drawing", |_| {
        let now = commits(&all);
        if now != last {
            last = now;
            quiet = Instant::now();
        }
        quiet.elapsed() > Duration::from_millis(300)
    });
    eprintln!(
        "trace: the layers were quiet {} ms after they showed",
        (quiet - quiet_since).as_millis()
    );
    for state in [KeyState::Pressed, KeyState::Released] {
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), state, 0);
        server.step();
    }
    let (mut steps, mut between, mut new_buffers) = (0, 0, 0);
    let mut last = commits(&all);
    server.wait_until("the panel to go after its close", |s| {
        if !panel.is_alive() || launcher_surfaces(s, SEARCH).is_none() {
            return true;
        }
        let now = s.now();
        let shown = present(&panel, now, false);
        // the close is over once every layer has faded out; the hidden window that follows is not the close's
        let closing = shown.iter().any(|l| l.opacity > 0.0) || !settled(&panel, now);
        if !closing {
            return true;
        }
        steps += 1;
        if shown.iter().any(|l| l.opacity > 0.0 && l.opacity < 1.0) {
            between += 1;
        }
        let commit = commits(&all);
        if commit != last {
            new_buffers += commit.iter().zip(&last).filter(|(a, b)| a != b).count();
            last = commit;
        }
        false
    });
    eprintln!(
        "trace: close: {steps} server steps, {between} of them part way, {new_buffers} new buffers of the window and its layers"
    );
    assert!(between > 0, "the close ran on the server");
    assert_eq!(
        new_buffers, 0,
        "the window and its layers drew no frame while they closed"
    );
}

/// The launcher's application browser opens from hidden on the compositor's clock: its window draws one frame, with
/// its layers (the glass, the content and the edge) at rest, and while the compositor scales and fades them in from
/// where the open starts neither the window nor any of its layers commits a buffer. The browser opens once first and
/// closes, as a resident launcher's has by the time it is asked again, so the renderer has warmed up (its first frames
/// are the CPU's, then the GPU draws them again). Run with `--no-capture` for the counts.
#[test]
fn the_launcher_opens_a_scope_on_the_compositor_s_clock_without_a_new_buffer() {
    use cupertino_server::layers_v1::present;
    // the browser's window, with the room for its shadow
    const BROWSER: i32 = 844 + 120;
    // the browser's glass, its content and its edge
    const LAYERS: usize = 3;
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let empty = TempDir::new("launcher-open-no-host-data");
    let mut launcher = real_process_without_host_data("cupertino-launcher", &bus, &empty.0);
    launcher.command.push("--no-files".into());
    let Some(mut server) = Server::start(vec![launcher], true) else {
        return;
    };
    server.state.dbus = Some(
        cupertino_server::dbus::DbusHandle::start_on(Some(bus.address.clone()), || {})
            .expect("the control bus worker"),
    );
    let connection = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build(),
    )
    .expect("connect to the test bus");
    let owned = || {
        zbus::block_on(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &"org.cupertino.Launcher1",
        ))
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
    };
    server.wait_until(
        "the launcher to map its hidden window and own its name",
        |s| committed_buffer(s, "cupertino-launcher").is_some() && owned(),
    );
    let toggle = |server: &mut Server| {
        server.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::ToggleAppGrid],
        })
    };
    // the open's first frame: the window and its layers at rest, which the compositor shows from where the open starts
    let first_frame = |server: &mut Server| {
        server.wait_until("the browser's first frame with its layers", |s| {
            launcher_surfaces(s, BROWSER).is_some_and(|(panel, layers)| {
                buffer_commit(&panel).is_some()
                    && layers.len() == LAYERS
                    && layers.iter().all(|l| buffer_commit(l).is_some())
            })
        });
        launcher_surfaces(&server.state, BROWSER).unwrap()
    };
    let commits = |surfaces: &[_]| -> Vec<_> { surfaces.iter().map(buffer_commit).collect() };
    // the first open, the renderer's warm-up in it, then the close
    toggle(&mut server);
    let (panel, layers) = first_frame(&mut server);
    let all: Vec<_> = std::iter::once(panel).chain(layers).collect();
    let mut last = commits(&all);
    let mut quiet = Instant::now();
    server.wait_until("the browser to stop drawing", |_| {
        let now = commits(&all);
        if now != last {
            last = now;
            quiet = Instant::now();
        }
        quiet.elapsed() > Duration::from_millis(500)
    });
    toggle(&mut server);
    server.wait_until("the browser to close", |s| !launcher_panel(s, BROWSER));
    // the open counted
    toggle(&mut server);
    let (panel, layers) = first_frame(&mut server);
    let all: Vec<_> = std::iter::once(panel.clone()).chain(layers).collect();
    let (mut steps, mut between, mut new_buffers, mut largest) = (0, 0, 0, 0.0_f64);
    let mut last = commits(&all);
    server.wait_until("the open to be over", |s| {
        let shown = present(&panel, s.now(), false);
        // part way: a layer fading in, or scaled from where the open starts
        let part_way = shown
            .iter()
            .any(|l| l.opacity < 1.0 || (l.scale - 1.0).abs() > 1e-9);
        steps += 1;
        let commit = commits(&all);
        if part_way {
            between += 1;
            largest = shown.iter().fold(largest, |m, l| m.max(l.scale));
            new_buffers += commit.iter().zip(&last).filter(|(a, b)| a != b).count();
        }
        last = commit;
        steps > 1 && !part_way
    });
    eprintln!(
        "trace: open: {steps} server steps, {between} of them part way, {new_buffers} new buffers of the window and its \
         layers"
    );
    assert!(between > 0, "the open ran on the server");
    assert!(
        largest > 1.05,
        "the edge scaled from where the open starts: {largest}"
    );
    assert_eq!(
        new_buffers, 0,
        "the window and its layers drew no frame while they opened"
    );
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
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server
        .state
        .config
        .screenshot_command
        .splice(1..1, common::events::isolated());
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
    // the overlay, its border and the crosshair at the release point went before the copy
    assert_capture_shows_the_output(&mut server, &bytes, (100, 100, 200, 150));
}

/// The toolbar shortcut maps the overlay and the 560 × 48 toolbar centred 40 pt above the bottom, which takes the
/// keyboard; in the remembered screen mode, Return captures the whole output.
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
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server
        .state
        .config
        .screenshot_command
        .splice(1..1, common::events::isolated());
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
    // (1000 − 560) / 2 and 700 − 40 − 48
    assert_eq!(
        toolbar(&server.state),
        Some(Rectangle::new((220, 612).into(), (560, 48).into()))
    );
    server.wait_until("the toolbar to take the keyboard", |s| {
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
    // the overlay and the toolbar went before the copy
    assert_capture_shows_the_output(
        &mut server,
        &bytes,
        (0, 0, OUTPUT.0 as u32, OUTPUT.1 as u32),
    );
}

/// Escape on the toolbar takes the overlay and the toolbar away and gives the keyboard back to the window that had it,
/// without a capture.
#[test]
fn escape_on_the_screenshot_toolbar_removes_its_surfaces_and_gives_the_keyboard_back() {
    const ESCAPE: u32 = 1 + 8;
    let dir = TempDir::new("screenshot-escape");
    let (conf, shots) = (dir.file("screenshot.conf"), dir.file("shots"));
    std::fs::write(
        &conf,
        format!("location = {}\nshow-thumbnail = false\n", shots.display()),
    )
    .unwrap();
    let Some(mut server) = Server::start(Vec::new(), true) else {
        return;
    };
    spawn_toplevel(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(server.state.socket_name.as_ref().unwrap()),
    );
    server.wait_until("the window to map", |s| s.wm.windows().count() == 1);
    let window = server.state.wm.windows().next().unwrap().id;
    let surface = server.state.entry(window).unwrap().surface().clone();
    let on_window = move |s: &Cupertino| {
        s.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(&surface)
    };
    server.wait_until("the keyboard on the window", on_window.clone());
    let binary = shell_bin_dir().join("cupertino-screenshot");
    server.state.config.screenshot_command = vec![
        "env".into(),
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server
        .state
        .config
        .screenshot_command
        .splice(1..1, common::events::isolated());
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::Screenshot(ScreenshotRequest::Toolbar)],
    });
    let surfaces = |s: &Cupertino| {
        let Some(output) = s.outputs.first() else {
            return 0;
        };
        layer_map_for_output(&output.output)
            .layers()
            .filter(|l| l.namespace().starts_with("cupertino-screenshot"))
            .count()
    };
    server.wait_until("the overlay and the toolbar to map", |s| surfaces(s) == 2);
    server.wait_until("the toolbar to take the keyboard", {
        let on_window = on_window.clone();
        move |s: &Cupertino| {
            !on_window(s) && s.seat.get_keyboard().unwrap().current_focus().is_some()
        }
    });
    server
        .state
        .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Pressed, 0);
    server.step();
    server
        .state
        .keyboard_event_now(Keycode::new(ESCAPE), KeyState::Released, 0);
    server.wait_until("the surfaces to go", |s| surfaces(s) == 0);
    server.wait_until("the keyboard back on the window", on_window);
    assert!(
        !shots.exists() || std::fs::read_dir(&shots).unwrap().next().is_none(),
        "nothing was captured"
    );
}

/// The toolbar's Options menu is a popup of a layer surface at the bottom of the output: with no room below the
/// button the server flips it upward, whole on the output.
#[test]
fn the_screenshot_options_menu_opens_above_the_toolbar_on_its_output() {
    const BTN_LEFT: u32 = 0x110;
    let dir = TempDir::new("screenshot-menu");
    let conf = dir.file("screenshot.conf");
    std::fs::write(&conf, "show-thumbnail = false\n").unwrap();
    let Some(mut server) = Server::start(Vec::new(), true) else {
        return;
    };
    let binary = shell_bin_dir().join("cupertino-screenshot");
    server.state.config.screenshot_command = vec![
        "env".into(),
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server
        .state
        .config
        .screenshot_command
        .splice(1..1, common::events::isolated());
    server.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::Screenshot(ScreenshotRequest::Toolbar)],
    });
    type Area = Rectangle<i32, Logical>;
    // the toolbar and, once it is open, its menu, in output points
    let toolbar = |s: &Cupertino| -> Option<(Area, Option<Area>)> {
        let map = layer_map_for_output(&s.outputs.first()?.output);
        let layer = map
            .layers()
            .filter(|l| l.namespace() == "cupertino-screenshot")
            .find(|l| {
                map.layer_geometry(l)
                    .is_some_and(|g| g.size.w > 0 && g.size.w != OUTPUT.0)
            })?;
        let at = map.layer_geometry(layer)?;
        // where the server placed the popup, relative to the toolbar
        let menu = PopupManager::popups_for_surface(layer.wl_surface())
            .find_map(|(popup, _)| match popup {
                PopupKind::Xdg(p) => Some(p.with_pending_state(|s| s.geometry)),
                _ => None,
            })
            .map(|g| Rectangle::new(at.loc + g.loc, g.size));
        Some((at, menu))
    };
    server.wait_until("the toolbar to map", |s| toolbar(s).is_some());
    let (bar, _) = toolbar(&server.state).unwrap();
    // the Options button: left of Capture at the toolbar's trailing end
    let options = (
        f64::from(bar.loc.x + bar.size.w - 120),
        f64::from(bar.loc.y + bar.size.h / 2),
    );
    server.wait_until("the toolbar to take the keyboard", |s| {
        s.seat.get_keyboard().unwrap().current_focus().is_some()
    });
    // the process hears the pointer enter, then the press and the release, in that order on its connection
    server.state.pointer_moved_to(options.into(), 10, None);
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Pressed, 20);
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Released, 30);
    server.wait_until("the menu to open", |s| {
        toolbar(s).is_some_and(|(_, menu)| menu.is_some())
    });
    let (bar, menu) = toolbar(&server.state).unwrap();
    let menu = menu.unwrap();
    // below the button it would run past the bottom of the output; flipped, it ends above the toolbar's bottom
    assert!(
        menu.loc.y >= 0
            && menu.loc.y + menu.size.h <= OUTPUT.1
            && menu.loc.y + menu.size.h <= bar.loc.y + bar.size.h
            && menu.loc.y < bar.loc.y,
        "the menu {menu:?} opens upward from the toolbar {bar:?}, on the output"
    );
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
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("CUPERTINO_SCREENSHOT_CONFIG={}", conf.display()),
        binary.display().to_string(),
    ];
    server
        .state
        .config
        .screenshot_command
        .splice(1..1, common::events::isolated());
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
/// exits with that message, a success exit the supervisor does not restart. Its system bus is a private one whose
/// stand-in logind knows no session for any process, and the test's own `XDG_SESSION_ID` is not passed on, so the
/// process is outside a session wherever the test runs, a real login session included. The lock and unlock cycle is
/// `lock_screen.rs`'s.
#[test]
fn the_login_process_connects_and_exits_outside_a_login_session() {
    let dir = TempDir::new("login-stderr");
    let log = dir.file("stderr");
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let Some(system) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let _logind = no_session_logind::serve(&system.address);
    let mut login = real_process("cupertino-login", &bus);
    login
        .command
        .splice(1..1, ["-u".to_owned(), "XDG_SESSION_ID".to_owned()]);
    let program = login.command.pop().unwrap();
    // inside a real login session the process gets as far as its login items and the applications saved to reopen: every
    // directory it reads them from is the test's, so it never starts the host user's programs
    use std::os::unix::fs::PermissionsExt;
    let runtime = dir.file("runtime");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let isolated = |sub: &str| {
        let path = dir.file(sub);
        std::fs::create_dir_all(&path).unwrap();
        path.display().to_string()
    };
    login.command.extend([
        format!("DBUS_SYSTEM_BUS_ADDRESS={}", system.address),
        format!("HOME={}", isolated("home")),
        format!("XDG_CONFIG_HOME={}", isolated("config")),
        format!("XDG_CONFIG_DIRS={}", isolated("config-dirs")),
        format!("XDG_DATA_HOME={}", isolated("data")),
        format!("XDG_DATA_DIRS={}", isolated("data-dirs")),
        format!("XDG_STATE_HOME={}", isolated("state")),
        format!("XDG_CACHE_HOME={}", isolated("cache")),
        format!("XDG_RUNTIME_DIR={}", runtime.display()),
        "sh".into(),
        "-c".into(),
        "exec \"$0\" 2>\"$1\"".into(),
        program,
        log.display().to_string(),
    ]);
    let Some(mut server) = Server::start(vec![login], true) else {
        return;
    };
    server.wait_until("the login process to say it is in no session", |_| {
        std::fs::read_to_string(&log)
            .is_ok_and(|text| text.contains("this process is in no login session"))
    });
}

/// A stand-in for logind on a private system bus that knows no session for any process, as logind answers a process
/// outside every session.
mod no_session_logind {
    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.login1")]
    enum Error {
        #[zbus(error)]
        ZBus(zbus::Error),
        NoSessionForPID(String),
    }

    struct Manager;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        #[zbus(name = "GetSessionByPID")]
        fn get_session_by_pid(&self, pid: u32) -> Result<zbus::zvariant::OwnedObjectPath, Error> {
            Err(Error::NoSessionForPID(format!(
                "PID {pid} does not belong to any known session"
            )))
        }
    }

    /// Serve the stand-in on the bus at `address` for as long as the connection lives.
    pub fn serve(address: &str) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(address)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at("/org/freedesktop/login1", Manager)
            .unwrap()
            .build()
            .unwrap()
    }
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
    let logs = TempDir::new("daemon-logs");
    let daemon = |binary: &str, args: &[&str]| {
        Daemon(
            Command::new(shell_bin_dir().join(binary))
                .envs(common::events::isolated_vars())
                .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                .env("CUPERTINO_ASSET_SEARCH_PATH", "")
                .args(args)
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(logs.file(binary)).unwrap())
                .spawn()
                .expect("the daemon starts"),
        )
    };
    let mut daemons = [
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
    let end = Instant::now() + common::HANG_GUARD;
    loop {
        let names = bus_names(&bus);
        if wanted.iter().all(|name| names.iter().any(|n| n == name)) {
            return;
        }
        if Instant::now() >= end {
            let report: Vec<String> = ["cupertino-netd", "cupertino-btd", "cupertino-powerd"]
                .into_iter()
                .zip(daemons.iter_mut())
                .map(|(binary, daemon)| {
                    let status = daemon.0.try_wait().unwrap();
                    let log = std::fs::read_to_string(logs.file(binary)).unwrap_or_default();
                    format!("{binary}: exit {status:?}, stderr {log:?}")
                })
                .collect();
            panic!(
                "the bus names {wanted:?}: only {names:?}\n{}",
                report.join("\n")
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `cupertino-sessiond --check-config` passes a settings file it understands whole, and fails naming each line it does not
/// understand, so a packaging check or an administrator sees what the manager only logs.
#[test]
fn the_session_managers_check_fails_on_each_line_it_does_not_understand() {
    let sessiond = shell_bin_dir().join("cupertino-sessiond");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("login.conf");
    let check = |text: &str| {
        std::fs::write(&file, text).unwrap();
        let output = Command::new(&sessiond)
            .envs(common::events::isolated_vars())
            .pam_isolated(cupertino_test_pam::shared())
            .arg("--check-config")
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("CUPERTINO_LOGIN_CONF", &file)
            .output()
            .expect("the binary starts");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let required = "greeter-command = /usr/bin/cupertino-server\nsession-command = /usr/bin/cupertino-session\n";
    assert_eq!(check(required), (true, String::new()));

    let (ok, errors) = check(&format!(
        "{required}hide-admin-users false\nhiden-users = alice\n"
    ));
    assert!(!ok);
    let lines: Vec<&str> = errors.lines().collect();
    assert_eq!(lines.len(), 2, "{errors}");
    assert!(
        lines[0].contains("line 3: expected `key = value`"),
        "{errors}"
    );
    assert!(
        lines[1].contains("line 4: unknown setting \"hiden-users\""),
        "{errors}"
    );

    let (ok, errors) = check("session-command = /usr/bin/cupertino-session\n");
    assert!(!ok);
    assert!(
        errors.contains("greeter-command is required; the manager refuses the file"),
        "{errors}"
    );

    // Safety: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    // the test's own file is not root's alone, so the manager would ignore the keys only root may set
    let (ok, errors) = check(&format!("{required}auto-login = alice\n"));
    assert!(!ok);
    assert_eq!(errors.lines().count(), 1, "{errors}");
    assert!(errors.contains("auto-login is ignored"), "{errors}");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let output = Command::new(&sessiond)
        .envs(common::events::isolated_vars())
        .pam_isolated(cupertino_test_pam::shared())
        .arg("--check-config")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("CUPERTINO_LOGIN_CONF", &file)
        .output()
        .expect("the binary starts");
    assert!(!output.status.success());
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        errors.contains("cannot be read by this user; run the check as root"),
        "{errors}"
    );
}

/// The session manager refuses a mode it does not know. Its start without an argument is a system service (it binds
/// the greeter socket, takes a terminal and waits for logind): there it is only run where it cannot reach those, as a
/// user other than root on a host without `/etc/cupertino/login.conf`, and must fail naming that file.
#[test]
fn the_session_manager_binary_refuses_what_it_cannot_serve() {
    let sessiond = shell_bin_dir().join("cupertino-sessiond");
    let output = Command::new(&sessiond)
        .envs(common::events::isolated_vars())
        .pam_isolated(cupertino_test_pam::shared())
        .arg("no-such-mode")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .expect("the binary starts");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("usage: cupertino-sessiond [worker | --check-config]"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Safety: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root || Path::new("/etc/cupertino/login.conf").exists() {
        eprintln!("skipping the start: root or a login.conf would let the manager run");
        return;
    }
    let output = Command::new(&sessiond)
        .envs(common::events::isolated_vars())
        .pam_isolated(cupertino_test_pam::shared())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .expect("the binary starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cupertino-sessiond: /etc/cupertino/login.conf: "),
        "{stderr}"
    );
}

/// The server asked for a toplevel and never committed its surface: the host was never asked to configure it.
const NEVER_COMMITTED: &str =
    "the server asked for an xdg_toplevel but never committed its surface, so the host was never asked to configure it";
/// The server committed, the host answered a later sync, and no configure came: the host read the commit and stalled.
const HOST_STALLED: &str = "the host compositor never configured the window: the server committed its surface and \
     the host answered a later wl_display.sync, but sent no xdg_surface.configure, so the host session stalled";
/// The server committed and no configure or answer to a later sync came.
const NOTHING_READ: &str =
    "no xdg_surface.configure was received after the initial commit, and no answer to a \
     later wl_display.sync: most likely the host session stalled";

/// What the `--winit` server's own Wayland traffic (`WAYLAND_DEBUG=client`, requests marked ` -> `) says about a window
/// that was never configured; `None` when no toplevel was asked for or the host configured it. A host configures a
/// surface only after its first commit, and the answer to a `wl_display.sync` sent after the commit shows the host
/// read the commit (and the server read the answer).
fn unconfigured_window(lines: &[String]) -> Option<&'static str> {
    let request = |l: &String| l.contains(" -> ");
    let event = |l: &String| l.starts_with('[') && !request(l);
    let asked = lines
        .iter()
        .position(|l| request(l) && l.contains(".get_toplevel("))?;
    let surface = lines[..asked]
        .iter()
        .rev()
        .find(|l| request(l) && l.contains(".get_xdg_surface("))
        .and_then(|l| l.split("wl_surface").nth(1))
        .map(|id| format!("wl_surface{}.commit()", id.trim_end_matches(')')))?;
    if lines
        .iter()
        .any(|l| event(l) && l.contains("xdg_surface") && l.contains(".configure("))
    {
        return None;
    }
    let Some(committed) = lines[asked..]
        .iter()
        .position(|l| request(l) && l.contains(&surface))
    else {
        return Some(NEVER_COMMITTED);
    };
    let after = &lines[asked + committed..];
    let answered = after.iter().enumerate().any(|(i, l)| {
        request(l)
            && l.contains("wl_display")
            && l.contains(".sync(")
            && l.split("new id ").nth(1).is_some_and(|callback| {
                let done = format!("{}.done(", callback.trim_end_matches(')'));
                after[i..].iter().any(|l| event(l) && l.contains(&done))
            })
    });
    Some(if answered { HOST_STALLED } else { NOTHING_READ })
}

/// The lines of the server's log to show on a failure: its own messages and the window's protocol traffic (the
/// `xdg_`, `wl_surface`, `wl_display.sync` and `wl_callback` lines), without the rest of `WAYLAND_DEBUG`.
fn shown(lines: &[String]) -> String {
    lines
        .iter()
        .filter(|l| {
            !l.starts_with('[')
                || ["xdg_", "wl_surface", "wl_callback", ".sync("]
                    .iter()
                    .any(|part| l.contains(part))
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// The server binary with `mode` (`--headless` or `--winit`): it listens on its own socket and connects its output.
/// `None` when the output cannot be made for want of an EGL device, unless `CUPERTINO_EGL_REQUIRED=1` (CI), which makes
/// that a failure. A nested server logs its Wayland traffic with the host, so a host that never configures the window
/// is named as such. The server gets a bus of its own (none without `dbus-daemon`): on the desktop's session bus, its
/// names would meet those of the person's session and of another test run at the same time.
fn start_server_binary(mode: &str) -> Option<(Daemon, Option<PrivateBus>)> {
    let config = TempDir::new(&format!("server-binary{mode}"));
    let bus = PrivateBus::start();
    let address = bus
        .as_ref()
        .map_or("unix:path=/nonexistent", |bus| bus.address.as_str());
    let conf = config.file("compositor.json");
    std::fs::write(&conf, b"{\"shell\":[]}").unwrap();
    let mut command =
        common::spawn::cleared(env!("CARGO_BIN_EXE_cupertino-server"), process_home());
    command
        .args([mode, "--size", "800x600"])
        .env(cupertino_server::config::CONFIG_ENV, &conf)
        .env("RUST_LOG", "info")
        .env(
            "XDG_RUNTIME_DIR",
            std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
        )
        .pam_isolated(cupertino_test_pam::shared())
        .env("XDG_CONFIG_HOME", &config.0)
        .env("XDG_STATE_HOME", config.0.join("state"))
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if mode == "--winit" {
        command.env("WAYLAND_DEBUG", "client");
        for name in ["WAYLAND_DISPLAY", "DISPLAY"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    let mut server = Daemon(command.spawn().expect("the server starts"));
    let stderr = std::io::BufReader::new(server.0.stderr.take().unwrap());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(stderr).map_while(std::io::Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let end = Instant::now() + common::HANG_GUARD;
    let mut lines: Vec<String> = Vec::new();
    let has = |lines: &[String], text: &str| lines.iter().any(|l| l.contains(text));
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => lines.push(line),
            Err(RecvTimeoutError::Timeout) => {}
            // the server closed its stderr: it has ended
            Err(RecvTimeoutError::Disconnected) => {
                if has(&lines, "listening") && has(&lines, "display connected") {
                    break;
                }
                let no_egl = has(&lines, headless::NO_USABLE_DEVICE);
                let required = std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() == Ok("1");
                assert!(
                    no_egl && !required,
                    "the server {mode} ended before it connected its output:\n{}",
                    shown(&lines)
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
            "{}:\n{}",
            unconfigured_window(&lines).map_or_else(
                || format!("the server {mode} never connected its output"),
                str::to_owned
            ),
            shown(&lines)
        );
    }
    assert!(
        server.0.try_wait().unwrap().is_none(),
        "{}",
        lines.join("\n")
    );
    // the server is dropped, and so stopped, before its bus
    Some((server, bus))
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

/// A window the host never configured is told apart from a server that never committed its surface, one that stopped
/// reading after its commit, and a window the host configured, in the Rust backend's and libwayland's `WAYLAND_DEBUG`
/// formats.
#[test]
fn a_window_the_host_never_configured_is_told_apart_from_a_server_failure() {
    let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let asked = [
        "[21:00:18.613230]  -> xdg_wm_base#12.get_xdg_surface(new id xdg_surface#26, wl_surface#25)",
        "[21:00:18.613236]  -> xdg_surface#26.get_toplevel(new id xdg_toplevel#27)",
        "[21:00:18.613288]  -> xdg_toplevel#27.set_title(\"Cupertino\")",
    ];
    assert_eq!(unconfigured_window(&lines(&asked)), Some(NEVER_COMMITTED));

    let mut committed = asked.to_vec();
    committed.push("[21:00:18.613302]  -> wl_surface#25.commit()");
    assert_eq!(unconfigured_window(&lines(&committed)), Some(NOTHING_READ));

    // another surface's commit is not the window's
    let mut other = asked.to_vec();
    other.push("[21:00:18.613302]  -> wl_surface#2.commit()");
    other.push("[21:00:18.613311]  -> wl_display#1.sync(new id wl_callback#31)");
    other.push("[21:00:18.614827] wl_callback#31.done(15462)");
    assert_eq!(unconfigured_window(&lines(&other)), Some(NEVER_COMMITTED));

    // events read after the commit, but no sync answered: not proof the host read the commit
    let mut events = committed.clone();
    events.push("[21:00:18.614814] wl_keyboard#17.keymap(1, fd 32, 35724)");
    events.push("[21:00:18.614823] wl_keyboard#17.repeat_info(33, 500)");
    assert_eq!(unconfigured_window(&lines(&events)), Some(NOTHING_READ));

    // a sync sent before the commit does not count, and an answer to another callback is not this sync's
    let mut early = asked.to_vec();
    early.insert(
        0,
        "[21:00:18.611176]  -> wl_display#1.sync(new id wl_callback#17)",
    );
    early.insert(1, "[21:00:18.611300] wl_callback#17.done(15400)");
    early.push("[21:00:18.613302]  -> wl_surface#25.commit()");
    early.push("[21:00:18.613311]  -> wl_display#1.sync(new id wl_callback#31)");
    early.push("[21:00:18.614000] wl_callback#3.done(15460)");
    assert_eq!(unconfigured_window(&lines(&early)), Some(NOTHING_READ));

    // the shape of the stalled host's trace: commit, sync, the sync answered, no configure
    let mut reading = events.clone();
    reading.insert(
        4,
        "[21:00:18.613311]  -> wl_display#1.sync(new id wl_callback#31)",
    );
    reading.push("[21:00:18.614827] wl_callback#31.done(15462)");
    assert_eq!(unconfigured_window(&lines(&reading)), Some(HOST_STALLED));

    let mut configured = reading.clone();
    configured.push("[21:00:18.620101] xdg_toplevel#27.configure(800, 600, [])");
    configured.push("[21:00:18.620110] xdg_surface#26.configure(4)");
    configured.push("[21:00:18.620200]  -> xdg_surface#26.ack_configure(4)");
    assert_eq!(unconfigured_window(&lines(&configured)), None);

    let libwayland = [
        "[1234567.880]  -> xdg_wm_base@12.get_xdg_surface(new id xdg_surface@26, wl_surface@25)",
        "[1234567.890]  -> xdg_surface@26.get_toplevel(new id xdg_toplevel@27)",
        "[1234567.900]  -> wl_surface@25.commit()",
        "[1234567.910]  -> wl_display@1.sync(new id wl_callback@31)",
        "[1234568.010] wl_callback@31.done(15462)",
        "[1234570.120] xdg_surface@26.configure(4)",
    ];
    assert_eq!(
        unconfigured_window(&lines(&libwayland[..5])),
        Some(HOST_STALLED)
    );
    assert_eq!(unconfigured_window(&lines(&libwayland)), None);

    // no window asked for: the server failed before it reached the host
    assert_eq!(
        unconfigured_window(&lines(&["listening on wayland-1"])),
        None
    );
}

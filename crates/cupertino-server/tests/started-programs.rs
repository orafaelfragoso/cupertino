//! The programs the server starts itself are known by their processes (`docs/architecture/shell-host.md`, *Identity*):
//! - `org.cupertino.Session1.StartScreenshot` is the portal's alone: it starts the screenshot program with `--serve` on
//!   a connection of its own, once while that one runs, again after it exited, and never on the login screen.
//!
//! This test binary stands in for both programs. Hard-linked as `cupertino-portal` under an install root of the
//! test's own, the supervisor starts it as the portal with no arguments, so every test of the binary starts in it and
//! first holds the portal's connection there ([`as_started_program`]). Started with [`STAND_IN`] it is the screenshot
//! program ([`screenshot_stand_in`]).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_peer::{Caller, Denial, ProcessHandle};
use cupertino_server::capture::launch::SERVE_ARG;
use cupertino_server::config::BuiltIn;
use cupertino_server::quit::{Call, Refusal, Reject};
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

/// How long a test waits for a process to start or end.
const WAIT: Duration = Duration::from_secs(20);

/// The argument that makes this test binary the screenshot program: a filter after `--` that names no test.
const STAND_IN: &str = "stand-in";

/// The file name the portal stand-in is linked as.
const PORTAL_FILE: &str = "cupertino-portal";

/// A program with nothing to do: it holds its connection until the server closes it.
struct Plain;

impl Dispatch<wl_registry::WlRegistry, ()> for Plain {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Hold the connection of `WAYLAND_SOCKET` until the server closes it.
fn hold_connection() {
    let conn = Connection::connect_to_env().expect("started with a WAYLAND_SOCKET");
    let mut queue = conn.new_event_queue::<Plain>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    while queue.blocking_dispatch(&mut Plain).is_ok() {}
}

/// In the portal stand-in, be the portal: one test holds the connection, the others wait, and the process ends with it.
fn as_started_program() {
    let exe = std::env::current_exe().expect("the test binary");
    if exe.file_name().is_some_and(|name| name == PORTAL_FILE) {
        static HELD: Once = Once::new();
        HELD.call_once(hold_connection);
        std::process::exit(0);
    }
}

/// The screenshot program, when the server started this binary with [`STAND_IN`]; a test run does nothing here.
#[test]
fn screenshot_stand_in() {
    as_started_program();
    if std::env::args().any(|arg| arg == STAND_IN) {
        hold_connection();
    }
}

/// This test binary, running [`screenshot_stand_in`] alone.
fn screenshot_command() -> Vec<String> {
    let exe = std::env::current_exe().unwrap().display().to_string();
    [
        exe.as_str(),
        "--exact",
        "screenshot_stand_in",
        "--nocapture",
        "--",
        STAND_IN,
    ]
    .map(String::from)
    .to_vec()
}

/// An install root of the test's own whose `bin/cupertino-portal` is this binary, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(name: &str) -> Root {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("started-programs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for dir in [&root, &bin] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let portal = bin.join(PORTAL_FILE);
        std::fs::hard_link(std::env::current_exe().unwrap(), &portal).unwrap();
        std::fs::set_permissions(&portal, std::fs::Permissions::from_mode(0o755)).unwrap();
        Root(root)
    }

    fn portal(&self) -> PathBuf {
        self.0.join("bin").join(PORTAL_FILE)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A server that runs the portal stand-in and starts this binary as its screenshot program.
struct Rig {
    _event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    _root: Root,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let root = Root::new(name);
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut config = Config::default();
        config.shell_processes.clear();
        config.screenshot_command = screenshot_command();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .expect("server starts");
        state
            .supervisor
            .add_built_ins([(BuiltIn::Portal, Some(root.portal()))], Instant::now());
        state.supervise_shell(Instant::now());
        Rig {
            _event_loop: event_loop,
            state,
            _root: root,
        }
    }

    /// The portal stand-in's process, started by the supervisor's first pass.
    fn portal(&self) -> u32 {
        self.state
            .supervisor
            .pid_of(BuiltIn::Portal.name())
            .unwrap_or_else(|| panic!("the portal runs: {:?}", self.state.supervisor.processes()))
    }

    fn start_screenshot(&mut self, caller: Caller) -> Result<(), Reject> {
        let (reply, answer) = async_channel::bounded(1);
        self.state
            .on_quit_call(Call::StartScreenshot(caller, reply));
        answer.try_recv().expect("answered at once")
    }
}

/// The bus caller that process `pid` is.
fn caller(pid: u32) -> Caller {
    Caller::of_process(ProcessHandle::open(pid).unwrap(), Path::new("/proc"))
}

/// The children of this process that run, have not exited, and were started with `--serve`.
fn serving() -> Vec<u32> {
    let mut pids = Vec::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let children = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
        pids.extend(
            children
                .split_whitespace()
                .filter_map(|p| p.parse::<u32>().ok()),
        );
    }
    pids.retain(|pid| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let running = stat
            .rfind(')')
            .is_some_and(|end| !stat[end + 1..].trim_start().starts_with('Z'));
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        running
            && cmdline
                .split(|b| *b == 0)
                .any(|arg| arg == SERVE_ARG.as_bytes())
    });
    pids.sort_unstable();
    pids
}

fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kill `pid` and wait until it no longer runs.
fn end(pid: u32) {
    ProcessHandle::open(pid)
        .unwrap()
        .send(libc::SIGKILL)
        .unwrap();
    until("the screenshot service ended", || !serving().contains(&pid));
}

#[test]
fn only_the_portal_starts_the_screenshot_service_once_while_it_runs() {
    as_started_program();
    let mut rig = Rig::new("start");
    let portal = rig.portal();
    assert_eq!(
        rig.start_screenshot(Caller::Anonymous),
        Err(Reject::Caller(Denial::Anonymous))
    );
    assert_eq!(
        rig.start_screenshot(caller(std::process::id())),
        Err(Reject::Caller(Denial::Untrusted)),
        "a process the server did not start as the portal is refused"
    );
    assert!(serving().is_empty(), "a refused call starts nothing");

    assert_eq!(rig.start_screenshot(caller(portal)), Ok(()));
    let first = serving();
    assert_eq!(first.len(), 1, "the portal's call started the service");
    assert_eq!(rig.start_screenshot(caller(portal)), Ok(()));
    assert_eq!(serving(), first, "a second call finds it running");

    end(first[0]);
    assert_eq!(rig.start_screenshot(caller(portal)), Ok(()));
    let second = serving();
    assert_eq!(second.len(), 1, "a call after it ended starts it again");
    assert_ne!(second, first);

    end(second[0]);
    rig.state.login_screen = true;
    assert!(matches!(
        rig.start_screenshot(caller(portal)),
        Err(Reject::Refusal(Refusal::Failed(_)))
    ));
    assert!(serving().is_empty(), "the login screen starts no programs");
}

//! Clients of `cupertino-wayland-client`, which speak standard protocols only, against the in-process server on a
//! public connection: the `simple-window` example maps with server-side decorations, presents frames paced by frame
//! callbacks at the configured size and closes on Escape; `cupertino-testclient` maps with its app id and title,
//! writes what is typed into it and exits on the close request. The server runs on an offscreen EGL device; the tests
//! skip (pass) when there is none.

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::spawn;
use cupertino_server::backend::headless;
use cupertino_server::shell_connection::{connect_client, hand_over};
use cupertino_server::state::ClientState;
use cupertino_server::Cupertino;
use cupertino_wm::Command as WmCommand;
use smithay::backend::input::KeyState;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

const OUTPUT: (i32, i32) = (1280, 800);
const TIMEOUT: Duration = Duration::from_secs(20);
/// The xkb offset of evdev key codes.
const XKB_OFFSET: u32 = 8;
/// Evdev codes of Escape, H, I and Return.
const KEY_ESC: u32 = 1;
const KEY_H: u32 = 35;
const KEY_I: u32 = 23;
const KEY_ENTER: u32 = 28;
/// Frames the animated run presents.
const FRAMES: u32 = 30;

/// A binary of another crate, which cargo builds for these tests.
enum Binary {
    SimpleWindow,
    TestClient,
}

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Server {
    /// The server on the headless output; `None` without an EGL device.
    fn start() -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Default::default(),
            false,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        Some(Server { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    /// Step the server until `cond` holds; panics after [`TIMEOUT`].
    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&mut Cupertino) -> bool) {
        let end = Instant::now() + TIMEOUT;
        while Instant::now() < end {
            self.step();
            if cond(&mut self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// `binary` with `args`, connected as an application the server did not start.
    fn launch(&mut self, binary: Binary, args: &[&str]) -> Child {
        let home = std::env::temp_dir();
        let mut command = spawn::cleared(built(binary), &home);
        command.args(args).stdout(Stdio::piped());
        let stream = connect_client(&self.state.display_handle, ClientState::default()).unwrap();
        hand_over(&mut command, &stream);
        command.spawn().unwrap()
    }

    /// Press and release the key with evdev code `code`.
    fn tap(&mut self, code: u32) {
        for pressed in [KeyState::Pressed, KeyState::Released] {
            self.state
                .keyboard_event_now(Keycode::new(code + XKB_OFFSET), pressed, 0);
            self.step();
        }
    }

    /// Step the server until `child` exits; its standard output.
    fn wait_for_exit(&mut self, child: &mut Child) -> String {
        let mut status = None;
        self.wait_until("the client to exit", |_| {
            status = child.try_wait().unwrap();
            status.is_some()
        });
        assert!(status.unwrap().success(), "the client failed: {status:?}");
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        out
    }
}

/// Build `binary` (a no-op when current) and return its path. It belongs to another crate, so cargo gives these tests
/// no `CARGO_BIN_EXE_*` for it.
fn built(binary: Binary) -> PathBuf {
    // <target>/<profile>/deps/<this test> -> <target>/<profile>
    let profile_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (args, path): (&[&str], _) = match binary {
        Binary::SimpleWindow => (
            &[
                "-p",
                "cupertino-wayland-client",
                "--example",
                "simple-window",
            ],
            profile_dir.join("examples/simple-window"),
        ),
        Binary::TestClient => (
            &["-p", "cupertino-testclient"],
            profile_dir.join("cupertino-testclient"),
        ),
    };
    let mut cargo = Command::new(std::env::var_os("CARGO").expect("CARGO is set by `cargo test`"));
    cargo.arg("build").args(args).args([
        "--target-dir",
        profile_dir.parent().unwrap().to_str().unwrap(),
    ]);
    if profile_dir.file_name().unwrap() == "release" {
        cargo.arg("--release");
    }
    let status = cargo.status().unwrap();
    assert!(status.success(), "building {}: {status}", path.display());
    assert!(path.is_file(), "missing binary {}", path.display());
    path
}

/// The app id and title of the mapped window.
fn identity(state: &Cupertino) -> (Option<String>, Option<String>) {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    with_states(window.surface(), |s| {
        let d = s
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();
        (d.app_id.clone(), d.title.clone())
    })
}

/// A file path unique to this test run, removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str) -> TempFile {
        TempFile(std::env::temp_dir().join(format!("cupertino-{name}-{}", std::process::id())))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn simple_window_presents_frames_with_server_side_decorations() {
    let Some(mut server) = Server::start() else {
        return;
    };
    let mut child = server.launch(Binary::SimpleWindow, &["--frames", &FRAMES.to_string()]);
    server.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    assert!(
        server
            .state
            .windows
            .iter()
            .find(|w| w.mapped)
            .unwrap()
            .server_side,
        "the server draws the frame of a window that asks for it"
    );
    let out = server.wait_for_exit(&mut child);
    assert_eq!(
        out.lines().last(),
        Some(format!("frames {FRAMES} size 320x240 scale 1 decorations server").as_str())
    );
}

#[test]
fn simple_window_closes_on_escape() {
    let Some(mut server) = Server::start() else {
        return;
    };
    let mut child = server.launch(Binary::SimpleWindow, &[]);
    server.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    server.tap(KEY_ESC);
    server.wait_for_exit(&mut child);
}

type Extent = (i32, i32);

/// The size the mapped window's client acknowledged in a full-screen configure and the size of the buffer it committed
/// last, in logical points.
fn fullscreen_sizes(state: &Cupertino) -> Option<(Extent, Option<Extent>)> {
    let window = state.windows.iter().find(|w| w.mapped)?;
    let acked = window.toplevel.current_state();
    if !acked.states.contains(xdg_toplevel::State::Fullscreen) {
        return None;
    }
    let buffer = with_renderer_surface_state(window.surface(), |s| s.buffer_size())
        .flatten()
        .map(|size| (size.w, size.h));
    acked.size.map(|size| ((size.w, size.h), buffer))
}

#[test]
fn testclient_writes_what_is_typed_and_exits_on_close() {
    let Some(mut server) = Server::start() else {
        return;
    };
    let output = TempFile::new("testclient-output");
    let mut child = server.launch(
        Binary::TestClient,
        &[
            "--app-id",
            "org.cupertino.TestClient",
            "--title",
            "Typed",
            "--output",
            output.path().to_str().unwrap(),
        ],
    );
    server.wait_until("the window to map and the output file", |s| {
        s.windows.iter().any(|w| w.mapped) && output.path().exists()
    });
    assert_eq!(
        identity(&server.state),
        (
            Some("org.cupertino.TestClient".to_owned()),
            Some("Typed".to_owned())
        )
    );
    let window = server.state.windows.iter().find(|w| w.mapped).unwrap();
    assert!(
        window.server_side,
        "the server draws the frame of a window that asks for it"
    );
    let id = window.id;
    for code in [KEY_H, KEY_I, KEY_ENTER] {
        server.tap(code);
    }
    server.wait_until("the typed text in the output file", |_| {
        std::fs::read_to_string(output.path()).is_ok_and(|text| text == "hi\n")
    });
    server.state.command(WmCommand::Fullscreen {
        requested_by: cupertino_wm::Requester::Client,
        window: id,
        output: None,
    });
    server.wait_until("a frame at the size of the full-screen configure", |s| {
        fullscreen_sizes(s).is_some_and(|(configured, buffer)| buffer == Some(configured))
    });
    let (configured, _) = fullscreen_sizes(&server.state).unwrap();
    assert_ne!(
        configured,
        (480, 320),
        "the configure is not the client's own size"
    );
    server
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .unwrap()
        .close();
    server.wait_for_exit(&mut child);
}

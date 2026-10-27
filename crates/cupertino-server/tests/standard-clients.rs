//! The `simple-window` example of `cupertino-wayland-client`, a client of standard protocols only, against the
//! in-process server on a public connection: its window maps with server-side decorations, presents frames paced by
//! frame callbacks at the configured size and closes on Escape. The server runs on an offscreen EGL device; the test
//! skips (passes) when there is none.

mod common;

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::spawn;
use cupertino_server::backend::headless;
use cupertino_server::shell_connection::{connect_client, hand_over};
use cupertino_server::state::ClientState;
use cupertino_server::Cupertino;
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (1280, 800);
const TIMEOUT: Duration = Duration::from_secs(20);
/// Evdev code of Escape plus the xkb offset of 8.
const ESCAPE: u32 = 1 + 8;
/// Frames the animated run presents.
const FRAMES: u32 = 30;

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
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            eprintln!("skipping: no EGL device ({e:#})");
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

    /// `simple-window` with `args`, connected as an application the server did not start.
    fn launch(&mut self, args: &[&str]) -> Child {
        let home = std::env::temp_dir();
        let mut command = spawn::cleared(simple_window_binary(), &home);
        command.args(args).stdout(Stdio::piped());
        let stream = connect_client(&self.state.display_handle, ClientState::default()).unwrap();
        hand_over(&mut command, &stream);
        command.spawn().unwrap()
    }

    /// Step the server until `child` exits; its standard output.
    fn wait_for_exit(&mut self, child: &mut Child) -> String {
        let mut status = None;
        self.wait_until("simple-window to exit", |_| {
            status = child.try_wait().unwrap();
            status.is_some()
        });
        assert!(
            status.unwrap().success(),
            "simple-window failed: {status:?}"
        );
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

/// Build the example (a no-op when current) and return the binary. It belongs to another crate, so cargo gives this test
/// no `CARGO_BIN_EXE_*` for it.
fn simple_window_binary() -> PathBuf {
    // <target>/<profile>/deps/<this test> -> <target>/<profile>
    let profile_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut cargo = Command::new(std::env::var_os("CARGO").expect("CARGO is set by `cargo test`"));
    cargo.args([
        "build",
        "-p",
        "cupertino-wayland-client",
        "--example",
        "simple-window",
        "--target-dir",
        profile_dir.parent().unwrap().to_str().unwrap(),
    ]);
    if profile_dir.file_name().unwrap() == "release" {
        cargo.arg("--release");
    }
    let status = cargo.status().unwrap();
    assert!(status.success(), "building simple-window: {status}");
    let binary = profile_dir.join("examples/simple-window");
    assert!(binary.is_file(), "missing binary {}", binary.display());
    binary
}

#[test]
fn simple_window_presents_frames_with_server_side_decorations() {
    let Some(mut server) = Server::start() else {
        return;
    };
    let mut child = server.launch(&["--frames", &FRAMES.to_string()]);
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
    let mut child = server.launch(&[]);
    server.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    for pressed in [KeyState::Pressed, KeyState::Released] {
        server
            .state
            .keyboard_event_now(Keycode::new(ESCAPE), pressed, 0);
        server.step();
    }
    server.wait_for_exit(&mut child);
}

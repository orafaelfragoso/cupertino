//! The `simple-window` example against Weston's headless backend, a compositor that offers no decoration protocol and
//! no Cupertino protocol: the window maps, draws its own frame and presents frames paced by frame callbacks. Needs
//! `weston` on `PATH`; CI runs it in a job that installs it (`cargo nextest run --run-ignored only`).

#[allow(dead_code)]
mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);
const SOCKET: &str = "wayland-simple-window-test";
/// Frames the window presents before it exits.
const FRAMES: u32 = 30;

/// A process killed on drop.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A private runtime directory, removed on drop.
struct RuntimeDir(PathBuf);

impl RuntimeDir {
    fn new() -> RuntimeDir {
        let dir = std::env::temp_dir().join(format!("cupertino-weston-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        RuntimeDir(dir)
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The example artifact prepared before the test runner starts.
fn simple_window_binary() -> PathBuf {
    fixtures::artifact("cupertino-wayland-client", "example", "simple-window")
}

fn start_weston(runtime: &Path) -> Running {
    let weston = Command::new("weston")
        .args(["--backend=headless", "--no-config", "--idle-time=0"])
        .arg(format!("--socket={SOCKET}"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("XDG_RUNTIME_DIR", runtime)
        .stdout(Stdio::null())
        .spawn()
        .expect("weston is on PATH");
    let socket = runtime.join(SOCKET);
    let end = Instant::now() + TIMEOUT;
    while !socket.exists() {
        assert!(
            Instant::now() < end,
            "weston did not create {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    Running(weston)
}

#[test]
#[ignore = "needs weston"]
fn simple_window_presents_frames_with_its_own_decorations_on_weston() {
    let runtime = RuntimeDir::new();
    let _weston = start_weston(&runtime.0);
    let child = Command::new(simple_window_binary())
        .args(["--frames", &FRAMES.to_string()])
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_RUNTIME_DIR", &runtime.0)
        .env("WAYLAND_DISPLAY", SOCKET)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut window = Running(child);
    let end = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = window.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < end, "simple-window did not finish");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "simple-window failed: {status}");
    let mut out = String::new();
    window
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(
        out.lines().last(),
        Some(format!("frames {FRAMES} size 320x240 scale 1 decorations client").as_str())
    );
}

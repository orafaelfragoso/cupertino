//! The running greeter process is undumpable (its owner cannot read its `/proc` environment) and has no core file size, read
//! while it waits for a compositor that never answers. Run as root the dumpable check is skipped.

use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cupertino_test_pam::{Pam, PamIsolated, Script};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn field(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .unwrap_or_else(|| panic!("no {name} in\n{text}"))
        .trim()
        .to_string()
}

#[test]
fn the_greeter_is_undumpable_and_has_no_core_file() {
    let dir = tempfile::tempdir().unwrap();
    let manager = UnixListener::bind(dir.path().join("greeter.sock")).unwrap();
    let compositor = dir.path().join("wayland-0");
    let _compositor = UnixListener::bind(&compositor).unwrap();
    // the login screen starts no PAM transaction itself (its manager does); the shim keeps it off the machine's
    // journal and buses all the same
    let pam = Pam::new(&Script::default());
    let process = Process(
        Command::new(env!("CARGO_BIN_EXE_cupertino-login"))
            .pam_isolated(&pam)
            .arg("--greeter")
            .env("CUPERTINO_LOGIN_SOCK", dir.path().join("greeter.sock"))
            .env("WAYLAND_DISPLAY", &compositor)
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // the greeter has connected to the manager's socket, then waits on the compositor
    manager.set_nonblocking(true).unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while manager.accept().is_err() {
        assert!(Instant::now() < end, "the greeter never connected");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = process.0.id();
    // after the connect it still runs for a moment, then blocks on the compositor: a loaded machine stretches that
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let status = match std::fs::read_to_string(format!("/proc/{pid}/status")) {
            Ok(status) => status,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                panic!("the greeter exited before it blocked on the compositor")
            }
            Err(e) => panic!("cannot read the greeter's state: {e}"),
        };
        if field(&status, "State:").starts_with('S') {
            break;
        }
        assert!(
            Instant::now() < end,
            "the greeter never blocked on the compositor\n{status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    let core = field(&limits, "Max core file size");
    assert_eq!(
        core.split_whitespace().take(2).collect::<Vec<_>>(),
        ["0", "0"],
        "{core}"
    );
    // a process that is not dumpable refuses its owner the read of its environment
    // Safety: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        let environment = std::fs::read(format!("/proc/{pid}/environ"));
        assert!(
            environment
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied),
            "the greeter is dumpable: {environment:?}"
        );
    }
}

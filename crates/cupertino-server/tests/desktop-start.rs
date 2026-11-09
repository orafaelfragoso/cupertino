//! What the server tells the program that started it: `--ready-fd` and the runtime file, on the headless server alone and
//! under `cupertino desktop start`. Both skip without an EGL device, unless `CUPERTINO_EGL_REQUIRED=1`.

mod common;

use std::io::{BufRead, BufReader, Read};

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;

use common::HANG_GUARD;
use cupertino_server::backend::headless::NO_USABLE_DEVICE;
use cupertino_server::logging::LOG_FILE_ENV;
use serde_json::Value;

/// A home, configuration, state and runtime directory of the test's own.
struct Dirs {
    dir: tempfile::TempDir,
}

impl Dirs {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `program` with nothing of the host's session: no display, bus or configuration of its own.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .env(LOG_FILE_ENV, self.path("server.log"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        c
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.path("server.log")).unwrap_or_default()
    }

    /// Whether the server could not start for want of an EGL device, which skips the test unless one is required.
    fn no_device(&self, output: &str) -> bool {
        let missing = self.log().contains(NO_USABLE_DEVICE) || output.contains(NO_USABLE_DEVICE);
        if missing {
            common::egl_unavailable(NO_USABLE_DEVICE);
        }
        missing
    }
}

/// Kills a process the test started when the test ends, also when it fails, so no server or command outlives it holding
/// what it inherited. It holds a pidfd, so a process already gone and its reused pid are never signalled.
struct KillOnDrop(Option<OwnedFd>);

impl KillOnDrop {
    fn new(pid: u32) -> Self {
        // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        // SAFETY: a non-negative result is a new descriptor owned here.
        Self((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as i32) }))
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(fd) = &self.0 {
            // SAFETY: a signal through a pidfd this guard owns; one to a process that ended fails harmlessly.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    std::os::fd::AsRawFd::as_raw_fd(fd),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
        }
    }
}

/// The lines of `from`, as a reader thread sends them; the channel closes at its end.
fn lines(from: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The exit status of `child`, waited for on a thread of its own so the wait has a bound.
fn exit_of(mut child: Child) -> ExitStatus {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    rx.recv_timeout(HANG_GUARD)
        .expect("the process ended")
        .unwrap()
}

/// A pipe whose write end survives the exec of a program started after it, as `--ready-fd` needs.
fn inherited_pipe() -> (OwnedFd, OwnedFd) {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe writes; without O_CLOEXEC a child inherits them.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: pipe returned two new descriptors this function owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    // the read end stays here alone
    // SAFETY: setting the close-on-exec flag of an open descriptor.
    assert_ne!(
        unsafe { libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC) },
        -1
    );
    (read, write)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_server_announces_its_socket_on_the_ready_descriptor_and_writes_its_runtime_file() {
    let dirs = Dirs::new();
    let (read, write) = inherited_pipe();
    let fd = std::os::fd::AsRawFd::as_raw_fd(&write);
    // `--run-for` ends it normally, which removes the file; the file is read as soon as the server is ready
    let child = dirs
        .command(env!("CARGO_BIN_EXE_cupertino-server"))
        .args([
            "--headless",
            "--size",
            "64x48",
            "--run-for",
            "5",
            "--ready-fd",
            &fd.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _guard = KillOnDrop::new(child.id());
    drop(write);
    let pid = child.id();
    let announced = lines(std::fs::File::from(read)).recv_timeout(HANG_GUARD);
    let Ok(line) = announced else {
        let status = exit_of(child);
        if dirs.no_device("") {
            return;
        }
        panic!(
            "no announcement; the server exited {status}: {}",
            dirs.log()
        );
    };
    let display = line
        .strip_prefix("WAYLAND_DISPLAY=")
        .unwrap_or_else(|| panic!("{line}"))
        .to_owned();
    assert!(display.starts_with("wayland-"), "{display}");
    let file = dirs
        .path("runtime/cupertino")
        .join(format!("server-{display}.json"));
    let info: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&dirs.path("runtime/cupertino")), 0o700);
    assert_eq!(info["schema"], 1);
    assert_eq!(info["pid"], pid);
    assert_eq!(info["backend"], "headless");
    assert_eq!(info["renderer"], "gles");
    assert_eq!(info["wayland_display"], display.as_str());
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        info["commit"],
        serde_json::json!(cupertino_build_info::COMMIT)
    );
    assert!(info["started"].as_u64().unwrap() > 1_600_000_000);

    let status = exit_of(child);
    assert!(status.success(), "{status}: {}", dirs.log());
    assert!(
        !file.exists(),
        "a server that ends normally removes its runtime file"
    );
}

#[test]
fn a_ready_descriptor_that_is_not_open_ends_the_start() {
    let dirs = Dirs::new();
    let mut child = dirs
        .command(env!("CARGO_BIN_EXE_cupertino-server"))
        .args([
            "--headless",
            "--size",
            "64x48",
            "--run-for",
            "5",
            "--ready-fd",
            "1000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = lines(child.stderr.take().unwrap());
    let status = exit_of(child);
    let stderr: Vec<String> = stderr.iter().collect();
    let stderr = stderr.join("\n");
    if dirs.no_device(&stderr) {
        return;
    }
    assert!(!status.success());
    let said = format!("{stderr}{}", dirs.log());
    assert!(
        said.contains("--ready-fd 1000 is not an open file descriptor"),
        "{said}"
    );
}

#[test]
fn desktop_start_runs_the_headless_server_until_it_is_stopped() {
    let dirs = Dirs::new();
    let mut child = dirs
        .command(common::fixtures::artifact(
            "cupertino-cli",
            "bin",
            "cupertino",
        ))
        .args([
            "--format",
            "jsonl",
            "desktop",
            "start",
            "--backend",
            "headless",
            "--size",
            "64x48",
            "--spawn",
            "true",
        ])
        .env("CUPERTINO_SERVER", env!("CARGO_BIN_EXE_cupertino-server"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _guard = KillOnDrop::new(child.id());
    let stdout = lines(child.stdout.take().unwrap());
    // the server's own output goes to the command's stderr, read as it comes so it never fills
    let stderr = lines(child.stderr.take().unwrap());
    let mut records: Vec<Value> = Vec::new();
    while let Ok(line) = stdout.recv_timeout(HANG_GUARD) {
        let record: Value = serde_json::from_str(&line).unwrap();
        let ready = record["event"] == "ready";
        records.push(record);
        if ready {
            break;
        }
    }
    if records.last().is_some_and(|r| r["type"] == "end") {
        // the server ended before it was ready
        let status = exit_of(child);
        let said: Vec<String> = stderr.iter().collect();
        let said = said.join("\n");
        if dirs.no_device(&said) {
            return;
        }
        panic!("{status}: {records:?}\n{said}\n{}", dirs.log());
    }
    let display = records.last().unwrap()["data"]["wayland_display"]
        .as_str()
        .expect("the ready event")
        .to_owned();
    let file = dirs
        .path("runtime/cupertino")
        .join(format!("server-{display}.json"));
    let info: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(info["backend"], "headless");

    // SAFETY: a signal to the child, which is not reaped yet.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    while let Ok(line) = stdout.recv_timeout(HANG_GUARD) {
        records.push(serde_json::from_str(&line).unwrap());
    }
    let status = exit_of(child);
    assert_eq!(status.code(), Some(0), "{records:?}");
    let kinds: Vec<String> = records
        .iter()
        .map(|r| match r["event"].as_str() {
            Some(event) => format!("event:{event}"),
            None => r["type"].as_str().unwrap().to_owned(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "event:starting",
            "event:ready",
            "event:exited",
            "result",
            "end"
        ]
    );
    assert_eq!(
        records[0]["data"]["args"],
        serde_json::json!(["--headless", "--size", "64x48", "--spawn", "true"])
    );
    assert_eq!(
        records[0]["data"]["server"],
        env!("CARGO_BIN_EXE_cupertino-server")
    );
    assert_eq!(records[2]["data"]["signal"], libc::SIGTERM);
    assert_eq!(records[3]["data"]["wayland_display"], display.as_str());
    assert_eq!(records[4]["reason"], "stopped");
    assert!(
        !file.exists(),
        "the command removes the runtime file of the server it stopped"
    );
}

/// The headless server on a ready pipe, with `--run-for` long enough to be ended by the test, and no core dump.
fn start_announced(dirs: &Dirs) -> (Child, mpsc::Receiver<String>) {
    use std::os::unix::process::CommandExt;
    let (read, write) = inherited_pipe();
    let fd = std::os::fd::AsRawFd::as_raw_fd(&write);
    let mut command = dirs.command(env!("CARGO_BIN_EXE_cupertino-server"));
    command
        .args([
            "--headless",
            "--size",
            "64x48",
            "--run-for",
            "120",
            "--ready-fd",
            &fd.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setrlimit is async-signal-safe; an abort then leaves no core file.
    unsafe {
        command.pre_exec(|| {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &none);
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(write);
    (child, lines(std::fs::File::from(read)))
}

/// The process `pid`, not a child of the test, has ended: a pidfd of it becomes readable. Bounded by [`HANG_GUARD`].
fn ended(pid: u32) -> bool {
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 {
        // already gone
        return true;
    }
    // SAFETY: a non-negative result is a new descriptor owned here.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let mut poll = libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut poll, 1, HANG_GUARD.as_millis() as libc::c_int) == 1 }
}

#[test]
fn a_server_ended_by_a_signal_removes_its_runtime_file_and_still_ends_by_that_signal() {
    use std::os::unix::process::ExitStatusExt;
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGABRT] {
        let dirs = Dirs::new();
        let (child, announced) = start_announced(&dirs);
        let _guard = KillOnDrop::new(child.id());
        let Ok(line) = announced.recv_timeout(HANG_GUARD) else {
            let status = exit_of(child);
            if dirs.no_device("") {
                return;
            }
            panic!(
                "no announcement; the server exited {status}: {}",
                dirs.log()
            );
        };
        let display = line.strip_prefix("WAYLAND_DISPLAY=").unwrap().to_owned();
        let file = dirs
            .path("runtime/cupertino")
            .join(format!("server-{display}.json"));
        assert!(file.exists(), "{signal}: written once ready");
        // SAFETY: a signal to the child, which is not reaped yet.
        assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
        let status = exit_of(child);
        assert_eq!(
            status.signal(),
            Some(signal),
            "it still ends by the signal: {status}"
        );
        assert!(!file.exists(), "signal {signal} removed the runtime file");
    }
}

#[test]
fn a_killed_desktop_start_ends_its_server_which_removes_its_runtime_file() {
    let dirs = Dirs::new();
    let mut child = dirs
        .command(common::fixtures::artifact(
            "cupertino-cli",
            "bin",
            "cupertino",
        ))
        .args([
            "--format",
            "jsonl",
            "desktop",
            "start",
            "--backend",
            "headless",
            "--size",
            "64x48",
        ])
        .env("CUPERTINO_SERVER", env!("CARGO_BIN_EXE_cupertino-server"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _guard = KillOnDrop::new(child.id());
    let stdout = lines(child.stdout.take().unwrap());
    let stderr = lines(child.stderr.take().unwrap());
    let mut ready = None;
    while let Ok(line) = stdout.recv_timeout(HANG_GUARD) {
        let record: Value = serde_json::from_str(&line).unwrap();
        if record["event"] == "ready" {
            ready = Some(record);
            break;
        }
    }
    let Some(ready) = ready else {
        let status = exit_of(child);
        let said: Vec<String> = stderr.iter().collect();
        if dirs.no_device(&said.join("\n")) {
            return;
        }
        panic!("{status}: never ready\n{}", dirs.log());
    };
    let display = ready["data"]["wayland_display"].as_str().unwrap();
    let file = dirs
        .path("runtime/cupertino")
        .join(format!("server-{display}.json"));
    let info: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let server = u32::try_from(info["pid"].as_u64().unwrap()).unwrap();
    let _server_guard = KillOnDrop::new(server);
    // nothing of the command runs after SIGKILL: the server hears of it through its parent-death signal
    // SAFETY: a signal to the child, which is not reaped yet.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) },
        0
    );
    let _ = exit_of(child);
    assert!(ended(server), "the server {server} ended with the command");
    assert!(
        !file.exists(),
        "the server ended by SIGTERM removed its runtime file"
    );
}

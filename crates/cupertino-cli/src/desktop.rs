//! `cupertino desktop start`: the compositor nested in a window, offscreen, or as the session on a text console.

use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Sink, Stability,
    ValueHint,
};
use serde::Serialize;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub(crate) const GROUP: Group = Group {
    name: "desktop",
    summary: "Starts the desktop",
    description: "Commands that run the desktop itself. The compositor is the `cupertino-server` program installed \
                  beside this command (or in /usr/bin); the session on a text console is `cupertino-session`.",
    commands: &[COMMANDS],
};

/// The backends `--backend` takes, in the order the help lists them.
const BACKENDS: &[&str] = &["nested", "tty", "headless"];

const COMMANDS: &[Command] = &[Command {
    path: &["desktop", "start"],
    summary: "Starts the compositor in a window of the current desktop, offscreen, or as the session of a text \
              console, and runs as long as it does",
    description: "Starts the desktop with the backend given with --backend and stays attached to it: the command \
                  ends when the compositor does. `nested` runs the compositor in a window of the desktop the command \
                  runs in, which needs WAYLAND_DISPLAY or DISPLAY (no_host_display, exit code 3 otherwise). \
                  `headless` runs it offscreen, for tests and scripts. Both need XDG_RUNTIME_DIR for the compositor's \
                  socket (no_runtime_dir) and take --size, --scale and --spawn, a command line the compositor starts \
                  once it runs (repeatable).\n\n\
                  `tty` turns this command into `cupertino-session`, the session of a text console, as a login \
                  would start it; it takes no other option (option_not_for_backend). It refuses to start inside a \
                  graphical session (inside_graphical_session, exit code 4) and from anything but a text console \
                  (not_a_vt, exit code 3). Once the session starts, its exit status is the command's, and after the \
                  starting event nothing more is printed.\n\n\
                  The compositor is CUPERTINO_SERVER when it is set (for development), else the cupertino-server \
                  beside this command, else /usr/bin/cupertino-server (server_not_installed when there is none); PATH \
                  is never searched. The command reports `starting`, then `ready` with the compositor's \
                  WAYLAND_DISPLAY once its socket takes clients, then `exited` with its exit status or signal and how \
                  long it ran. A compositor that ends before it is ready, or ends with a failure, fails the command \
                  with exit code 3 (server_failed). Ctrl-C or SIGTERM ask the compositor to end with SIGTERM; one \
                  that is still running 10 seconds later is killed, and the command ends with exit code 0.",
    examples: &[
        (
            "cupertino desktop start --backend nested --spawn foot",
            "Try the desktop in a window of the current one, with a terminal in it.",
        ),
        (
            "cupertino --format jsonl desktop start --backend headless --size 1280x800",
            "Run the compositor offscreen and print its events as JSON lines.",
        ),
        (
            "cupertino desktop start --backend tty",
            "Start the desktop session from a text console.",
        ),
    ],
    stability: Stability::Stable,
    args: &[
        Arg::option("backend", "where the compositor shows the desktop").values(BACKENDS),
        Arg::option("size", "the output size in pixels, WIDTHxHEIGHT (nested and headless)"),
        Arg::option("scale", "the output scale, above zero (nested and headless)"),
        Arg::option(
            "spawn",
            "a command line the compositor starts once it runs (nested and headless)",
        )
        .repeat()
        .hint(ValueHint::Shell),
    ],
    run: Run::Stream(start),
}];

/// How long a compositor asked to end with SIGTERM may take before it is killed. A test hands in a shorter one with
/// [`Context::provide`].
#[derive(Debug, Clone, Copy)]
pub struct StopGrace(pub Duration);

/// The grace of a real run.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How long one wait for the compositor lasts at most before the stop flag is looked at again.
const WAIT_STEP: Duration = Duration::from_millis(200);

#[derive(Serialize)]
struct Starting<'a> {
    backend: &'a str,
    server: &'a Path,
    args: &'a [String],
}

#[derive(Serialize)]
struct Ready<'a> {
    wayland_display: &'a str,
}

#[derive(Serialize)]
struct Exited {
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<i32>,
    seconds: f64,
}

#[derive(Serialize)]
struct Ran<'a> {
    backend: &'a str,
    wayland_display: Option<&'a str>,
    #[serde(flatten)]
    exited: Exited,
}

fn start(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.follow();
    let backend = args.value("backend").ok_or_else(|| {
        Failure::usage(
            "missing_option",
            "`desktop start` needs --backend nested, tty or headless",
        )
    })?;
    if backend == "tty" {
        return session(args, cx, sink);
    }
    let mut server_args: Vec<String> = vec![if backend == "nested" {
        "--winit"
    } else {
        "--headless"
    }
    .into()];
    if let Some(size) = args.value("size") {
        if parse_size(size).is_none() {
            return Err(Failure::usage(
                "invalid_value",
                format!("--size takes WIDTHxHEIGHT in pixels, both above zero, not `{size}`"),
            ));
        }
        server_args.extend(["--size".into(), size.into()]);
    }
    if let Some(scale) = args.value("scale") {
        if !scale.parse::<f64>().is_ok_and(|s| s.is_finite() && s > 0.0) {
            return Err(Failure::usage(
                "invalid_value",
                format!("--scale takes a number above zero, not `{scale}`"),
            ));
        }
        server_args.extend(["--scale".into(), scale.into()]);
    }
    for spawn in args.values("spawn") {
        server_args.extend(["--spawn".into(), spawn.clone()]);
    }
    let set = |name: &str| cx.var(name).filter(|v| !v.is_empty());
    if backend == "nested" && set("WAYLAND_DISPLAY").is_none() && set("DISPLAY").is_none() {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "no_host_display",
            "the nested backend runs in a window of the current desktop, and neither WAYLAND_DISPLAY nor DISPLAY \
             names one",
        ));
    }
    let Some(runtime_dir) = set("XDG_RUNTIME_DIR").map(PathBuf::from) else {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "no_runtime_dir",
            "XDG_RUNTIME_DIR is not set, so the compositor has no place for its socket",
        ));
    };
    let server = installed(cx, "CUPERTINO_SERVER", "cupertino-server").ok_or_else(|| {
        Failure::new(
            FailureKind::Unavailable,
            "server_not_installed",
            "cupertino-server is installed neither beside this command nor in /usr/bin",
        )
    })?;
    let grace = cx.provided::<StopGrace>().map_or(STOP_GRACE, |g| g.0);
    sink.event(
        "starting",
        &Starting {
            backend,
            server: &server,
            args: &server_args,
        },
        &format!("starting {} {}", server.display(), server_args.join(" ")),
    )?;
    let mut running = Server::spawn(cx, &server, &server_args, grace).map_err(|e| {
        Failure::new(
            FailureKind::Unavailable,
            "server_failed",
            format!("cannot start {}: {e}", server.display()),
        )
    })?;
    let mut display: Option<String> = None;
    let status = loop {
        if let Some(line) = running.ready_line() {
            if display.is_none() {
                if let Some(name) = line
                    .strip_prefix("WAYLAND_DISPLAY=")
                    .filter(|n| !n.is_empty())
                {
                    display = Some(name.to_owned());
                    sink.event(
                        "ready",
                        &Ready {
                            wayland_display: name,
                        },
                        &format!("ready: WAYLAND_DISPLAY={name}"),
                    )?;
                }
            }
            continue;
        }
        if let Some(status) = running.exited() {
            break status;
        }
        if sink.stop_requested() {
            running.terminate();
        }
        running.wait_step();
    };
    let seconds = (running.started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
    if let Some(name) = &display {
        remove_runtime_file(&runtime_dir, name, running.pid());
    }
    let exited = Exited {
        status: status.code(),
        signal: status.signal(),
        seconds,
    };
    let how = match (exited.status, exited.signal) {
        (Some(code), _) => format!("with status {code}"),
        (None, Some(signal)) => format!("by signal {signal}"),
        (None, None) => "without a status".to_owned(),
    };
    sink.event(
        "exited",
        &exited,
        &format!("the compositor exited {how} after {seconds:.1} s"),
    )?;
    if !running.stopped && !(display.is_some() && status.success()) {
        let when = if display.is_some() {
            "after it was ready"
        } else {
            "before it was ready"
        };
        return Err(Failure::new(
            FailureKind::Unavailable,
            "server_failed",
            format!("the compositor exited {how} {when}"),
        ));
    }
    Ok(Reply::new(
        &Ran {
            backend,
            wayland_display: display.as_deref(),
            exited,
        },
        "",
    ))
}

/// `--backend tty`: becomes `cupertino-session` after its checks.
fn session(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    if let Some(name) = ["size", "scale", "spawn"]
        .into_iter()
        .find(|name| !args.values(name).is_empty())
    {
        return Err(Failure::usage(
            "option_not_for_backend",
            format!("--{name} is for the nested and headless backends, not tty"),
        ));
    }
    let session_type = cx.var("XDG_SESSION_TYPE").unwrap_or_default();
    if session_type == "wayland" || session_type == "x11" {
        return Err(Failure::new(
            FailureKind::Denied,
            "inside_graphical_session",
            format!(
                "this runs inside a {} session; the tty backend starts a session of its own from a text console",
                session_type.to_string_lossy()
            ),
        ));
    }
    if !stdin_is_vt() {
        return Err(Failure::new(
            FailureKind::Unavailable,
            "not_a_vt",
            "the tty backend needs a text console: its standard input is not one",
        ));
    }
    let program = installed(cx, "", "cupertino-session").ok_or_else(|| {
        Failure::new(
            FailureKind::Unavailable,
            "session_not_installed",
            "cupertino-session is installed neither beside this command nor in /usr/bin",
        )
    })?;
    sink.event(
        "starting",
        &Starting {
            backend: "tty",
            server: &program,
            args: &[],
        },
        &format!("starting {}", program.display()),
    )?;
    // only returns when it could not run the program
    let e = cx.command(&program).exec();
    Err(Failure::new(
        FailureKind::Unavailable,
        "session_failed",
        format!("cannot run {}: {e}", program.display()),
    ))
}

/// The console request for the keyboard type, which only a virtual terminal of the console answers.
const KDGKBTYPE: libc::Ioctl = 0x4B33;

/// Whether standard input is a virtual terminal of the console (it answers the keyboard type request).
fn stdin_is_vt() -> bool {
    let mut kind: libc::c_char = 0;
    // SAFETY: KDGKBTYPE writes one byte into `kind`; on another file it fails and writes nothing.
    unsafe { libc::ioctl(0, KDGKBTYPE, &mut kind) == 0 }
}

/// `WIDTHxHEIGHT`, both above zero.
fn parse_size(size: &str) -> Option<(u32, u32)> {
    let (w, h) = size.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// A program of the desktop: the path in `variable` when it is set (development), else the one beside this command,
/// else the one in `/usr/bin` of the context's root. `PATH` is never searched.
fn installed(cx: &Context, variable: &str, name: &str) -> Option<PathBuf> {
    if !variable.is_empty() {
        if let Some(path) = cx.var(variable).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(path)).filter(|p| is_executable(p));
        }
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)));
    beside
        .into_iter()
        .chain([cx.path("/usr/bin").join(name)])
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The compositor's runtime file, `$XDG_RUNTIME_DIR/cupertino/server-<display>.json`; removed once the compositor
/// that wrote it is gone, since one ended by a signal cannot remove it itself.
fn remove_runtime_file(runtime_dir: &Path, display: &str, pid: u32) {
    let path = runtime_dir
        .join("cupertino")
        .join(format!("server-{display}.json"));
    let written_by = std::fs::read(&path)
        .ok()
        .and_then(|text| serde_json::from_slice::<serde_json::Value>(&text).ok())
        .and_then(|v| v["pid"].as_u64());
    if written_by == Some(u64::from(pid)) {
        let _ = std::fs::remove_file(path);
    }
}

/// A running compositor: its process, the pipe it says it is ready on and a pidfd that wakes when it exits.
struct Server {
    child: Child,
    ready: Option<OwnedFd>,
    pending: Vec<u8>,
    pidfd: Option<OwnedFd>,
    started: Instant,
    /// When it was asked to end with SIGTERM.
    terminated: Option<Instant>,
    killed: bool,
    /// It was asked to end because the command was asked to stop.
    stopped: bool,
    grace: Duration,
    status: Option<ExitStatus>,
}

impl Server {
    fn spawn(
        cx: &Context,
        program: &Path,
        args: &[String],
        grace: Duration,
    ) -> std::io::Result<Self> {
        let (read, write) = pipe()?;
        let fd = write.as_raw_fd();
        // the compositor's stdout carries its own lines; ours carries the command's output alone
        // SAFETY: duplicating the stderr descriptor gives a new descriptor owned here.
        let out = unsafe { OwnedFd::from_raw_fd(cloexec_dup(libc::STDERR_FILENO)?) };
        let parent = std::process::id() as libc::pid_t;
        let mut command = cx.command(program);
        command
            .args(args)
            .arg("--ready-fd")
            .arg(fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            // its own process group: a Ctrl-C of the terminal reaches this command, which asks it to end
            .process_group(0);
        // SAFETY: only async-signal-safe calls (fcntl, prctl, getppid, kill) run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                // the write end alone survives the exec, under the number --ready-fd names
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // ended with this command, even when it is killed (also when that happened before this line)
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                if libc::getppid() != parent {
                    libc::kill(libc::getpid(), libc::SIGTERM);
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop(write);
        set_nonblocking(read.as_raw_fd())?;
        let pidfd = pidfd_open(child.id());
        Ok(Self {
            child,
            ready: Some(read),
            pending: Vec::new(),
            pidfd,
            started: Instant::now(),
            terminated: None,
            killed: false,
            stopped: false,
            grace,
            status: None,
        })
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The next whole line the compositor wrote on the ready pipe, reading what is there without waiting.
    fn ready_line(&mut self) -> Option<String> {
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Some(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            let fd = self.ready.as_ref()?;
            let mut chunk = [0u8; 256];
            // SAFETY: the descriptor is open and owned by `self`; the file does not close it.
            let mut file =
                std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) });
            match file.read(&mut chunk) {
                Ok(0) => {
                    // closed: a last line without its end counts too
                    self.ready = None;
                    if self.pending.is_empty() {
                        return None;
                    }
                    let line = std::mem::take(&mut self.pending);
                    return Some(String::from_utf8_lossy(&line).into_owned());
                }
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }

    /// Its exit status once it has exited, reaping it.
    fn exited(&mut self) -> Option<ExitStatus> {
        if self.status.is_none() {
            self.status = self.child.try_wait().ok().flatten();
        }
        self.status
    }

    /// Asks it to end: SIGTERM now, SIGKILL once the grace has passed.
    fn terminate(&mut self) {
        self.stopped = true;
        match self.terminated {
            None => {
                self.terminated = Some(Instant::now());
                // SAFETY: a signal to the child, which is not reaped yet, so its pid is still its own.
                unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
            }
            Some(at) if !self.killed && at.elapsed() >= self.grace => {
                self.killed = true;
                let _ = self.child.kill();
            }
            Some(_) => {}
        }
    }

    /// Waits until the ready pipe has something, it exits, or [`WAIT_STEP`] passed (less near the end of the grace).
    fn wait_step(&self) {
        let mut fds: Vec<libc::pollfd> = [self.ready.as_ref(), self.pidfd.as_ref()]
            .into_iter()
            .flatten()
            .map(|fd| libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let mut step = WAIT_STEP;
        if let Some(at) = self.terminated.filter(|_| !self.killed) {
            step = step.min(self.grace.saturating_sub(at.elapsed()));
        }
        // SAFETY: `fds` holds valid pollfd entries for its whole length; an interrupted poll just returns early.
        unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                step.as_millis() as libc::c_int,
            )
        };
    }
}

impl Drop for Server {
    /// A command that returns early (its output was closed) does not leave the compositor running.
    fn drop(&mut self) {
        if self.exited().is_some() {
            return;
        }
        while self.exited().is_none() {
            self.terminate();
            self.wait_step();
        }
    }
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: pipe2 writes two descriptors into the array, which this call then owns.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned by nothing else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn cloexec_dup(fd: RawFd) -> std::io::Result<RawFd> {
    // SAFETY: F_DUPFD_CLOEXEC on an open descriptor returns a new one or fails.
    let new = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if new == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(new)
    }
}

fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: reading and setting the status flags of an open descriptor.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A descriptor that becomes readable when the process exits; `None` on a kernel without pidfds, where the wait falls
/// back to its steps.
fn pidfd_open(pid: u32) -> Option<OwnedFd> {
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    // SAFETY: a non-negative result is a new descriptor owned here.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_two_numbers_above_zero() {
        assert_eq!(parse_size("1280x800"), Some((1280, 800)));
        for bad in ["0x10", "10x0", "10", "x", "10x", "axb", "-1x5"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_runtime_file_is_removed_only_for_the_compositor_that_wrote_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cupertino/server-wayland-9.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"pid": 41}"#).unwrap();
        remove_runtime_file(dir.path(), "wayland-9", 42);
        assert!(file.exists(), "another compositor's file stays");
        remove_runtime_file(dir.path(), "wayland-9", 41);
        assert!(!file.exists());
    }
}

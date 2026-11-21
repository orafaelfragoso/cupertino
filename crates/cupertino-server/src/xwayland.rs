//! X11 applications through `xwayland-satellite`, started on demand. The server takes an X display (its lock file and
//! the filesystem and abstract sockets of `/tmp/.X11-unix`) and listens on it; the first connection starts the
//! satellite with the two sockets (`-listenfd`), which runs Xwayland and shows each X11 window as an ordinary Wayland
//! client of the public socket. The satellite gets no shell role. Programs the server starts get `DISPLAY` and
//! `XAUTHORITY`: the display takes only clients that hold its cookie, since the abstract socket has no file permissions
//! and any local user could reach it. When the satellite ends, the display listens again: after a failure only once
//! the restart throttle ([`crate::restart_policy`]) has passed, so the next X11 program starts it again and a satellite
//! that cannot run is not started in a loop. Its exit wakes the supervision pass of the shell processes (a pidfd). A
//! satellite that cannot be started is the diagnostic event `server.xwayland.start_failed`, its end
//! `server.xwayland.exited`.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixListener};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};

use crate::restart_policy::{with_exit_status, Backoff};

/// The program, found on `PATH`.
pub const SATELLITE: &str = "xwayland-satellite";
/// Where X servers keep their lock files; the sockets are in its `.X11-unix`.
pub const X_TMP: &str = "/tmp";
/// Displays tried, from the first; an X server started by another session takes the low ones.
const DISPLAYS: std::ops::Range<u32> = 0..64;
const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";
/// A satellite that ends successfully sooner than this after its start did not run: it goes through the restart throttle
/// like a failure.
const QUICK_EXIT: Duration = Duration::from_secs(1);
/// An authority entry for every address, so the entry holds whatever host name a client looks up.
const FAMILY_WILD: u16 = 0xffff;

/// A taken X display: removed (lock file, socket file, authority file) when dropped.
#[derive(Debug)]
pub struct XDisplay {
    number: u32,
    lock: PathBuf,
    socket: PathBuf,
    authority: PathBuf,
    /// The abstract and the filesystem socket.
    listeners: [OwnedFd; 2],
}

impl XDisplay {
    /// Takes the first free display under `tmp` (`/tmp` in a session) and writes its authority file to `runtime_dir`.
    pub fn open(tmp: &Path, runtime_dir: &Path) -> std::io::Result<Self> {
        let socket_dir = tmp.join(".X11-unix");
        check_socket_dir(&socket_dir)?;
        for number in DISPLAYS {
            let lock = tmp.join(format!(".X{number}-lock"));
            if !take_lock(&lock)? {
                continue;
            }
            let socket = socket_dir.join(format!("X{number}"));
            match listen(&socket) {
                Ok(listeners) => {
                    let authority = runtime_dir.join(format!("cupertino-xauth-{number}"));
                    let display = Self {
                        number,
                        lock,
                        socket,
                        authority,
                        listeners,
                    };
                    write_authority(&display.authority, number)?;
                    return Ok(display);
                }
                // another server holds the abstract name without a lock file we can see
                Err(e) if e.kind() == ErrorKind::AddrInUse => {
                    let _ = std::fs::remove_file(&lock);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&lock);
                    return Err(e);
                }
            }
        }
        Err(std::io::Error::other(format!(
            "every X display from :{} to :{} is taken",
            DISPLAYS.start,
            DISPLAYS.end - 1
        )))
    }

    /// `DISPLAY` for X11 clients.
    pub fn name(&self) -> String {
        format!(":{}", self.number)
    }

    /// `XAUTHORITY` for X11 clients.
    pub fn authority(&self) -> &Path {
        &self.authority
    }
}

/// Toolkits that would pick X11 when `DISPLAY` is set, and the preference that makes them try Wayland first and fall back
/// to X11. Qt 5.11 and later take a list (Qt 6 already prefers Wayland in a Wayland session, outside GNOME); Electron
/// before 38 reads the hint (38 removed it and picks Wayland from `XDG_SESSION_TYPE`). SDL is left alone: an SDL 2 bundled
/// with a game may not take a list, and a driver name it does not know fails its video start.
pub const WAYLAND_FIRST: [(&str, &str); 2] = [
    ("QT_QPA_PLATFORM", "wayland;xcb"),
    ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
];

/// Points a program the server starts at the X display, or, without one, keeps it from an X server of the session
/// around the server: an X11 toolkit that finds `DISPLAY` prefers it. With the X display, toolkits are told to prefer
/// Wayland ([`WAYLAND_FIRST`]) unless the program or the session already chose.
pub fn set_client_env(command: &mut Command, display: Option<&XDisplay>) {
    let Some(display) = display else {
        command.env_remove("DISPLAY");
        return;
    };
    command
        .env("DISPLAY", display.name())
        .env("XAUTHORITY", display.authority());
    for (name, value) in WAYLAND_FIRST {
        let chosen = match command.get_envs().find(|(k, _)| *k == name) {
            Some((_, value)) => value.is_some(),
            None => std::env::var_os(name).is_some(),
        };
        if !chosen {
            command.env(name, value);
        }
    }
}

impl Drop for XDisplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.authority);
        let _ = std::fs::remove_file(&self.lock);
    }
}

/// The socket directory must exist and be one other users cannot swap: a real directory of root or of this user, sticky
/// when it is shared. It is not created when missing (systemd-tmpfiles makes it, owned by root, at boot): the owner of a
/// sticky directory may replace every socket in it, so one made by a user would let that user take over the X displays
/// of others.
fn check_socket_dir(dir: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("the X socket directory {}: {e}", dir.display()),
        )
    })?;
    let owner_ok = meta.uid() == 0 || meta.uid() == rustix::process::getuid().as_raw();
    let shared = meta.mode() & 0o022 != 0;
    if !meta.is_dir() || !owner_ok || (shared && meta.mode() & 0o1000 == 0) {
        return Err(std::io::Error::other(format!(
            "{} is not a directory of root or this user that others cannot change",
            dir.display()
        )));
    }
    Ok(())
}

/// Creates the lock file of a display with this process's pid. A lock whose process is gone is removed and taken; false
/// when the display is in use.
fn take_lock(lock: &Path) -> std::io::Result<bool> {
    for _ in 0..2 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o444)
            .open(lock)
        {
            Ok(mut file) => {
                // the X servers' format: the pid right-aligned in ten columns
                writeln!(file, "{:>10}", std::process::id())?;
                return Ok(true);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                if !lock_is_stale(lock) || std::fs::remove_file(lock).is_err() {
                    return Ok(false);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// A lock file names a process that no longer exists. Unreadable or malformed locks count as live: they are skipped.
fn lock_is_stale(lock: &Path) -> bool {
    let mut text = String::new();
    let read = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock)
        .and_then(|f| f.take(32).read_to_string(&mut text));
    let pid = read
        .ok()
        .and_then(|_| text.trim().parse::<i32>().ok())
        .and_then(rustix::process::Pid::from_raw);
    pid.is_some_and(|pid| rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH))
}

/// Binds the abstract socket first, so a display whose name another server holds is skipped, then the filesystem
/// socket in place of any left by an earlier server of this display (whose lock is ours now).
fn listen(socket: &Path) -> std::io::Result<[OwnedFd; 2]> {
    let name = socket.as_os_str().as_encoded_bytes();
    let abstract_listener = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name)?)?;
    let _ = std::fs::remove_file(socket);
    let file_listener = UnixListener::bind(socket)?;
    Ok([abstract_listener.into(), file_listener.into()])
}

/// One `MIT-MAGIC-COOKIE-1` entry with a fresh random cookie, readable by this user only.
fn write_authority(path: &Path, number: u32) -> std::io::Result<()> {
    let mut cookie = [0u8; 16];
    let mut filled = 0;
    while filled < cookie.len() {
        filled +=
            rustix::rand::getrandom(&mut cookie[filled..], rustix::rand::GetRandomFlags::empty())?;
    }
    let mut entry = Vec::new();
    entry.extend(FAMILY_WILD.to_be_bytes());
    for field in [
        &b""[..],
        number.to_string().as_bytes(),
        COOKIE_NAME,
        &cookie,
    ] {
        entry.extend((field.len() as u16).to_be_bytes());
        entry.extend(field);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    // a file an earlier session left keeps its mode: it is made private before the cookie goes in
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(&entry)
}

#[derive(Debug)]
enum Phase {
    /// The display waits for its first client.
    Listening,
    Running {
        child: Child,
        started: Instant,
    },
    /// The satellite failed: the display listens again at this time.
    Waiting(Instant),
}

/// The X display and its satellite.
#[derive(Debug)]
pub struct XWayland {
    display: XDisplay,
    program: PathBuf,
    /// `WAYLAND_DISPLAY` of the satellite.
    socket: String,
    phase: Phase,
    backoff: Backoff,
    /// [`QUICK_EXIT`]; tests set it long, so a loaded machine cannot turn a quick exit into a run.
    quick_exit: Duration,
    sources: Vec<RegistrationToken>,
    /// Whether `sources` are enabled. They are switched only when this changes: calloop registers the fds again on
    /// every enable, and epoll answers EEXIST for sources that are already on.
    watching: bool,
}

impl XWayland {
    /// Listens on `display` from `handle`'s loop; `get` finds this value in the loop's state, and `started` runs after a
    /// waiting client may have started the satellite (the server watches the new child's exit there).
    pub fn start<D: 'static>(
        display: XDisplay,
        program: PathBuf,
        socket: String,
        handle: &LoopHandle<'static, D>,
        get: fn(&mut D) -> Option<&mut XWayland>,
        started: fn(&mut D),
    ) -> std::io::Result<Self> {
        let mut sources = Vec::new();
        for listener in &display.listeners {
            let watched = listener.try_clone()?;
            let loop_handle = handle.clone();
            let token = handle
                .insert_source(
                    Generic::new(watched, Interest::READ, Mode::Level),
                    move |_, _, state| {
                        if let Some(x) = get(state) {
                            x.connection_waiting(&loop_handle, Instant::now());
                        }
                        started(state);
                        Ok(PostAction::Continue)
                    },
                )
                .map_err(|e| std::io::Error::other(format!("watching the X socket: {e}")))?;
            sources.push(token);
        }
        let name = display.name();
        tracing::info!(display = name, "X display listening for X11 programs");
        Ok(Self {
            display,
            program,
            socket,
            phase: Phase::Listening,
            backoff: Backoff::default(),
            quick_exit: QUICK_EXIT,
            sources,
            watching: true,
        })
    }

    pub fn display(&self) -> &XDisplay {
        &self.display
    }

    /// When the display listens again after a failed satellite.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Waiting(at) => Some(at),
            Phase::Listening | Phase::Running { .. } => None,
        }
    }

    /// The pid of the running satellite.
    pub fn pid(&self) -> Option<u32> {
        match &self.phase {
            Phase::Running { child, .. } => Some(child.id()),
            Phase::Listening | Phase::Waiting(_) => None,
        }
    }

    /// A client waits on the display: the satellite takes over the sockets until it ends.
    fn connection_waiting<D>(&mut self, handle: &LoopHandle<'_, D>, now: Instant) {
        if !matches!(self.phase, Phase::Listening) {
            return;
        }
        self.watch(handle, false);
        self.phase = match self.spawn() {
            Ok(child) => {
                tracing::info!(
                    pid = child.id(),
                    display = self.display.name(),
                    "xwayland-satellite started"
                );
                Phase::Running {
                    child,
                    started: now,
                }
            }
            Err(e) => {
                tracing::warn!(program = %self.program.display(), error = %e, "starting xwayland-satellite failed");
                let retry = self.backoff.next_interval(Duration::ZERO);
                diag::event(
                    Severity::Warn,
                    "server.xwayland.start_failed",
                    "xwayland-satellite could not be started",
                )
                .field("program", Value::Path(&self.program))
                .field("error", Value::Public(&e.to_string()))
                .field("restart_in", Value::Duration(retry))
                .emit();
                Phase::Waiting(now + retry)
            }
        };
    }

    /// Finds an ended satellite and listens again when the restart throttle allows.
    pub fn poll<D>(&mut self, handle: &LoopHandle<'_, D>, now: Instant) {
        self.phase = match std::mem::replace(&mut self.phase, Phase::Listening) {
            Phase::Running { mut child, started } => match child.try_wait() {
                Ok(None) => Phase::Running { child, started },
                Ok(Some(status)) => {
                    tracing::info!(%status, "xwayland-satellite exited: the X display waits for the next X11 program");
                    let ran = now.saturating_duration_since(started);
                    if status.success() && ran >= self.quick_exit {
                        // it ran: a run of failures before it is over
                        self.backoff = Backoff::default();
                        record_exit(Some(status), ran, None, None);
                        Phase::Listening
                    } else {
                        // a failure, or a success at once (a build that cannot run, a usage error), which the waiting
                        // client would otherwise restart on every poll
                        let at = started + self.backoff.next_interval(ran);
                        record_exit(
                            Some(status),
                            ran,
                            Some(at.saturating_duration_since(now)),
                            None,
                        );
                        Phase::Waiting(at)
                    }
                }
                // the child cannot be waited for (it was reaped elsewhere): it counts as a failed exit, not as running
                Err(e) => {
                    tracing::warn!(error = %e, "waiting on xwayland-satellite failed: treating it as ended");
                    let _ = child.kill();
                    let ran = now.saturating_duration_since(started);
                    let at = started + self.backoff.next_interval(ran);
                    record_exit(None, ran, Some(at.saturating_duration_since(now)), Some(&e));
                    Phase::Waiting(at)
                }
            },
            Phase::Waiting(at) if at > now => Phase::Waiting(at),
            Phase::Waiting(_) | Phase::Listening => Phase::Listening,
        };
        if matches!(self.phase, Phase::Listening) {
            self.watch(handle, true);
        }
    }

    fn watch<D>(&mut self, handle: &LoopHandle<'_, D>, on: bool) {
        if self.watching == on {
            return;
        }
        self.watching = on;
        for token in &self.sources {
            let result = if on {
                handle.enable(token)
            } else {
                handle.disable(token)
            };
            if let Err(e) = result {
                tracing::warn!(error = %e, on, "switching the X socket watch failed");
            }
        }
    }

    fn spawn(&self) -> std::io::Result<Child> {
        let fds: [i32; 2] = [
            self.display.listeners[0].as_raw_fd(),
            self.display.listeners[1].as_raw_fd(),
        ];
        let mut command = Command::new(&self.program);
        command.arg(self.display.name());
        for fd in fds {
            command.args(["-listenfd", &fd.to_string()]);
        }
        command
            .arg("-auth")
            .arg(&self.display.authority)
            .env("WAYLAND_DISPLAY", &self.socket)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DISPLAY");
        // Safety: the closure runs in the child between fork and exec and only clears the close-on-exec flag of two
        // descriptors the parent keeps open, with `fcntl`, a system call.
        unsafe {
            command.pre_exec(move || {
                for fd in fds {
                    let fd = std::os::fd::BorrowedFd::borrow_raw(fd);
                    rustix::io::fcntl_setfd(fd.as_fd(), rustix::io::FdFlags::empty())?;
                }
                Ok(())
            });
        }
        crate::file_limit::restore_in_child(&mut command);
        command.spawn()
    }
}

/// `server.xwayland.exited`: the satellite ended with `status` (unknown when it could not be waited for, for `error`)
/// after a run of `ran`; the display listens again after `restart_in`, or at once. `info` for a run that ended
/// successfully, `warn` for a failure or a satellite that ended at once.
fn record_exit(
    status: Option<std::process::ExitStatus>,
    ran: Duration,
    restart_in: Option<Duration>,
    error: Option<&std::io::Error>,
) {
    let error = error.map(ToString::to_string);
    let event = if restart_in.is_none() {
        diag::event(
            Severity::Info,
            "server.xwayland.exited",
            "xwayland-satellite exited",
        )
    } else {
        diag::event(
            Severity::Warn,
            "server.xwayland.exited",
            "xwayland-satellite exited",
        )
    };
    let mut event = match status {
        Some(status) => with_exit_status(event, status),
        None => event,
    };
    event = event.field("ran", Value::Duration(ran)).field(
        "restart_in",
        Value::Duration(restart_in.unwrap_or_default()),
    );
    if let Some(error) = &error {
        event = event.field("error", Value::Public(error));
    }
    event.emit();
}

impl Drop for XWayland {
    /// The satellite, and with it Xwayland, does not outlive the server.
    fn drop(&mut self) {
        if let Phase::Running { child, .. } = &mut self.phase {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

//! The screen reader at the login screen. The login screen's worker starts it as a second child, so it runs in the
//! greeter's logind session and cgroup, as the greeter user, with a clean environment and private writable directories.
//! Only the compositor of the login screen turns it on or off, over [`READER_FD`], a socket the worker gives the leader
//! alone; the manager says whether the login screen is on the console ([`Wish::shown`]), and it runs only while it is.
//!
//! * The program is the first of [`READERS`] that only root can change; never one from `$PATH` or a setting.
//! * The bus is the greeter's own ([`bus_address`]).
//! * A root process never follows a path in the greeter's runtime directory: the reader's directory is made by the
//!   reader's own process after the privilege drop, and removed by a child that dropped to the greeter too.
//! * Stopping it ends its process group, then every process of the greeter in the session's cgroup that the leader did
//!   not start ([`strays`]): a daemon the reader started leaves its group, but not the session.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use cupertino_peer::ProcessHandle;

/// The screen readers the login screen may start, by absolute path, in order of preference.
pub const READERS: &[&str] = &["/usr/bin/orca"];
/// The leader's end of the toggle socket.
pub const READER_FD: libc::c_int = 3;
/// Names [`READER_FD`] in the leader's environment.
pub const READER_FD_ENV: &str = "CUPERTINO_READER_FD";
/// The longest request the compositor may send.
pub const MAX_REQUEST: usize = 64;
/// How long the reader has to end after `SIGTERM` before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// The file of the first of `candidates` that only root can change, every directory and link on the way included
/// ([`cupertino_defaults::system::root_only_target`]).
pub fn resolve(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .find_map(|path| cupertino_defaults::system::root_only_target(Path::new(path)))
}

/// The reader's `DBUS_SESSION_BUS_ADDRESS`: the greeter's user bus in `runtime`. A value PAM gave (`pam`) is used only
/// when it is exactly `unix:path=` and a path directly in `runtime`.
pub fn bus_address(runtime: &Path, pam: Option<&str>) -> Result<String, String> {
    let plain = |path: &Path| {
        path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    };
    if !plain(runtime) {
        return Err(format!(
            "the runtime directory {runtime:?} is not a plain absolute path"
        ));
    }
    let Some(given) = pam else {
        return Ok(format!("unix:path={}/bus", runtime.display()));
    };
    let path = given
        .strip_prefix("unix:path=")
        .filter(|p| !p.contains([',', ';', '%', '=']))
        .map(Path::new)
        .ok_or_else(|| format!("the session bus {given:?} is not a unix:path= address"))?;
    if !plain(path) || path.parent() != Some(runtime) {
        return Err(format!(
            "the session bus {given:?} is not in {}",
            runtime.display()
        ));
    }
    Ok(given.to_owned())
}

/// What the compositor asks, with its Wayland socket name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Sent once at start: the socket the reader connects to.
    Ready(String),
    /// Super+Alt+S was pressed.
    Toggle(String),
}

/// One request of the compositor: `ready <socket>` or `toggle <socket>`, the socket `wayland-<digits>`.
pub fn parse_request(frame: &[u8]) -> Result<Request, String> {
    if frame.len() > MAX_REQUEST {
        return Err(format!("a request of {} bytes", frame.len()));
    }
    let text = std::str::from_utf8(frame).map_err(|_| "a request that is not text".to_owned())?;
    let (verb, socket) = text
        .split_once(' ')
        .ok_or_else(|| format!("unknown request {text:?}"))?;
    let digits = socket.strip_prefix("wayland-").unwrap_or("");
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{socket:?} is not a Wayland socket name"));
    }
    match verb {
        "ready" => Ok(Request::Ready(socket.to_owned())),
        "toggle" => Ok(Request::Toggle(socket.to_owned())),
        _ => Err(format!("unknown request {text:?}")),
    }
}

/// What the worker answers a toggle with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    On,
    Off,
    /// No reader is installed, or it cannot start.
    Unavailable,
}

impl Reply {
    pub fn text(self) -> &'static [u8] {
        match self {
            Reply::On => b"on",
            Reply::Off => b"off",
            Reply::Unavailable => b"unavailable",
        }
    }
}

/// Whether the reader should run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wish {
    /// The machine's `screen_reader_at_login` at the start, then flipped by every toggle.
    pub wanted: bool,
    /// The login screen is on the console: false while a person's session is, and for good once the manager is gone.
    pub shown: bool,
}

impl Wish {
    pub fn runs(self) -> bool {
        self.wanted && self.shown
    }
}

/// The parent of `pid` in `proc_root` (`/proc`), from its `stat`.
fn parent(proc_root: &Path, pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    // the name in parentheses may hold spaces and parentheses: the fields follow the last `)`
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Whether `pid` is `ancestor` or one of its descendants, by the parents in `proc_root`. A process that cannot be looked
/// at, a loop, or a chain that ends at pid 1 or 0 first is not.
pub fn descends_from(proc_root: &Path, pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..4096 {
        if current == ancestor {
            return true;
        }
        if current <= 1 {
            return false;
        }
        match parent(proc_root, current) {
            Some(next) if next != current => current = next,
            _ => return false,
        }
    }
    false
}

/// The pids of `procs` (a cgroup's `cgroup.procs`) for which `keep` is false.
pub fn strays(procs: &str, keep: impl Fn(u32) -> bool) -> Vec<u32> {
    procs
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| !keep(*pid))
        .collect()
}

/// The real user of `pid` in `proc_root`.
fn owner(proc_root: &Path, pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status")).ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Kill with `SIGKILL` every process of `uid` listed in `procs` that is neither this process nor `leader` or one of its
/// descendants. Each is held by a pidfd while it is judged and is signalled through it, so a reused pid is never hit.
pub fn kill_strays(proc_root: &Path, procs: &str, uid: u32, leader: u32) -> usize {
    let me = std::process::id();
    let mut killed = 0;
    for pid in strays(procs, |pid| pid == me) {
        let Ok(process) = ProcessHandle::open(pid) else {
            continue;
        };
        let stray = owner(proc_root, pid) == Some(uid) && !descends_from(proc_root, pid, leader);
        if stray && !process.has_exited() && process.send(libc::SIGKILL).is_ok() {
            killed += 1;
        }
    }
    killed
}

/// `cgroup.procs` of this process's cgroup when it is the scope of logind session `session_id`; `None` for any other
/// cgroup, so nothing outside the session is ever swept.
pub fn session_procs(session_id: &str) -> Option<PathBuf> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let scope = format!("session-{session_id}.scope");
    (Path::new(path).file_name()? == scope.as_str()).then(|| {
        Path::new("/sys/fs/cgroup")
            .join(path.trim_start_matches('/'))
            .join("cgroup.procs")
    })
}

/// The greeter account the reader runs as.
pub struct Account {
    pub login: CString,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

/// Everything the reader's child needs, built before `fork`.
struct Launch {
    program: CString,
    runtime: CString,
    name: CString,
    /// Owns what `pointers` points to.
    environment: Vec<CString>,
}

/// A running reader.
struct Running {
    process: ProcessHandle,
    pid: libc::pid_t,
    directory: PathBuf,
}

/// The reader of one login screen, supervised by its worker.
pub struct Reader {
    account: Account,
    program: Option<CString>,
    runtime: PathBuf,
    bus: Result<String, String>,
    leader: u32,
    /// `cgroup.procs` of the greeter's session, swept at every stop.
    procs: Option<PathBuf>,
    socket: Option<String>,
    pub wish: Wish,
    running: Option<Running>,
    started: u32,
}

impl Reader {
    /// The reader of the login screen whose leader is `leader`; `runtime` is the greeter's `XDG_RUNTIME_DIR` and `bus`
    /// the `DBUS_SESSION_BUS_ADDRESS` PAM gave, if any. `program` is the file [`resolve`] found or `None`.
    pub fn new(
        account: Account,
        program: Option<&Path>,
        runtime: PathBuf,
        bus: Option<&str>,
        leader: u32,
        procs: Option<PathBuf>,
        wanted: bool,
    ) -> Self {
        Reader {
            account,
            program: program.and_then(|p| CString::new(p.as_os_str().as_bytes()).ok()),
            bus: bus_address(&runtime, bus),
            runtime,
            leader,
            procs,
            socket: None,
            wish: Wish {
                wanted,
                shown: true,
            },
            running: None,
            started: 0,
        }
    }

    /// The pidfd of the running reader, readable when it exits.
    pub fn pidfd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.running.as_ref().map(|r| r.process.fd())
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Handle one request of the compositor; a toggle is answered with what the reader is now.
    pub fn request(&mut self, frame: &[u8]) -> Option<Reply> {
        let request = match parse_request(frame) {
            Ok(request) => request,
            Err(e) => {
                eprintln!("cupertino-sessiond: worker: refused a screen reader request: {e}");
                return None;
            }
        };
        match request {
            Request::Ready(socket) => {
                self.socket = Some(socket);
                self.apply();
                None
            }
            Request::Toggle(socket) => {
                self.socket = Some(socket);
                self.wish.wanted = !self.wish.wanted;
                self.apply();
                Some(match (self.wish.wanted, self.running.is_some()) {
                    (false, _) => Reply::Off,
                    (true, true) => Reply::On,
                    (true, false) => {
                        // a reader that cannot start is not wanted either: the next toggle tries again
                        self.wish.wanted = false;
                        Reply::Unavailable
                    }
                })
            }
        }
    }

    /// The login screen is on the console (`true`) or not.
    pub fn shown(&mut self, shown: bool) {
        self.wish.shown = shown;
        self.apply();
    }

    /// Start or stop the reader to match [`Self::wish`].
    pub fn apply(&mut self) {
        match (self.wish.runs(), self.running.is_some()) {
            (true, false) => {
                if let Err(e) = self.start() {
                    eprintln!("cupertino-sessiond: worker: the screen reader cannot start: {e}");
                }
            }
            (false, true) => self.stop(),
            _ => {}
        }
    }

    /// The reader ended by itself: reap it and clean up after it. It stays wanted; the next toggle turns it off.
    pub fn exited(&mut self) {
        if let Some(running) = self.running.take() {
            self.clean_up(running);
        }
    }

    /// End the reader and everything it started.
    pub fn stop(&mut self) {
        let Some(running) = self.running.take() else {
            return;
        };
        // Safety: `kill` takes integers; the reader is unreaped, so its pid (and its group's) names no other process.
        unsafe { libc::kill(-running.pid, libc::SIGTERM) };
        wait_readable(running.process.fd(), STOP_GRACE);
        // Safety: as above.
        unsafe { libc::kill(-running.pid, libc::SIGKILL) };
        self.clean_up(running);
    }

    fn clean_up(&mut self, running: Running) {
        reap(running.pid);
        if let Some(procs) = &self.procs {
            if let Ok(text) = std::fs::read_to_string(procs) {
                let killed = kill_strays(Path::new("/proc"), &text, self.account.uid, self.leader);
                if killed > 0 {
                    eprintln!("cupertino-sessiond: worker: ended {killed} process(es) the screen reader left");
                }
            }
        }
        if let Err(e) = self.remove(&running.directory) {
            eprintln!("cupertino-sessiond: worker: the screen reader's directory stays: {e}");
        }
    }

    fn start(&mut self) -> Result<(), String> {
        let program = self
            .program
            .clone()
            .ok_or("no screen reader is installed")?;
        let socket = self
            .socket
            .clone()
            .ok_or("the compositor has not said its socket yet")?;
        let bus = self.bus.clone()?;
        self.started += 1;
        let name = format!("cupertino-reader-{}-{}", std::process::id(), self.started);
        let directory = self.runtime.join(&name);
        let path = |sub: &str| format!("{}/{sub}", directory.display());
        let cstring = |text: String| CString::new(text).map_err(|_| "a nul byte".to_owned());
        let environment = [
            format!("XDG_RUNTIME_DIR={}", self.runtime.display()),
            format!("DBUS_SESSION_BUS_ADDRESS={bus}"),
            format!("XDG_CONFIG_HOME={}", path("config")),
            format!("XDG_DATA_HOME={}", path("data")),
            format!("XDG_CACHE_HOME={}", path("cache")),
            format!("WAYLAND_DISPLAY={socket}"),
            format!("HOME={}", directory.display()),
        ]
        .into_iter()
        .map(cstring)
        .collect::<Result<Vec<_>, _>>()?;
        let launch = Launch {
            program,
            runtime: cstring(self.runtime.display().to_string())?,
            name: cstring(name)?,
            environment,
        };
        let pid = spawn(&self.account, &launch)?;
        let process = ProcessHandle::open(pid as u32).map_err(|e| format!("pidfd_open: {e}"))?;
        eprintln!("cupertino-sessiond: worker: the screen reader started");
        self.running = Some(Running {
            process,
            pid,
            directory,
        });
        Ok(())
    }

    /// Remove `directory` in a child that dropped to the greeter: a root process never walks the greeter's files.
    fn remove(&self, directory: &Path) -> Result<(), String> {
        // Safety: the worker has no other thread, so the child of `fork` may allocate.
        match unsafe { libc::fork() } {
            -1 => Err(format!("fork: {}", io::Error::last_os_error())),
            0 => {
                // Safety: in the child, which ends with `_exit`.
                unsafe {
                    let code = if drop_to(&self.account).is_ok()
                        && std::fs::remove_dir_all(directory).is_ok()
                    {
                        0
                    } else {
                        1
                    };
                    libc::_exit(code)
                }
            }
            child => match reap(child) {
                Some(0) => Ok(()),
                _ => Err(format!("removing {} failed", directory.display())),
            },
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wait up to `limit` for `fd` (a pidfd) to turn readable.
fn wait_readable(fd: std::os::fd::BorrowedFd<'_>, limit: Duration) {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    let timeout = Timespec {
        tv_sec: limit.as_secs() as i64,
        tv_nsec: i64::from(limit.subsec_nanos()),
    };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let _ = poll(&mut fds, Some(&timeout));
}

/// Reap `pid`; its exit code, `None` when it was signalled or cannot be waited for.
fn reap(pid: libc::pid_t) -> Option<i32> {
    let mut status = 0;
    loop {
        // Safety: `status` is a valid out pointer.
        if unsafe { libc::waitpid(pid, &mut status, 0) } >= 0 {
            break;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
    libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status))
}

/// Become `account` for good: as root, its groups, then its group, then the user; as that user already, nothing to
/// drop. Either way root must not be regainable after.
///
/// # Safety
/// Call in a child of `fork` only.
unsafe fn drop_to(account: &Account) -> Result<(), &'static str> {
    if libc::geteuid() == 0 {
        if libc::initgroups(account.login.as_ptr(), account.gid) != 0 {
            return Err("initgroups");
        }
        if libc::setgid(account.gid) != 0 {
            return Err("setgid");
        }
        if libc::setuid(account.uid) != 0 {
            return Err("setuid");
        }
    }
    if libc::setuid(0) == 0 || libc::getuid() != account.uid || libc::geteuid() != account.uid {
        return Err("the privilege drop is not permanent");
    }
    Ok(())
}

/// `fork`, and in the child become the greeter, make the private directory and `execve` the reader. A failure in the
/// child comes back as the error, through a pipe that closes on a successful `execve`.
fn spawn(account: &Account, launch: &Launch) -> Result<libc::pid_t, String> {
    let mut pipe = [0 as libc::c_int; 2];
    // Safety: `pipe` has room for two descriptors.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(format!("pipe2: {}", io::Error::last_os_error()));
    }
    // Safety: both descriptors are fresh and owned here.
    let (report_read, report_write) =
        unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    let mut environment: Vec<*const libc::c_char> =
        launch.environment.iter().map(|e| e.as_ptr()).collect();
    environment.push(std::ptr::null());
    // Safety: `getpid` takes nothing.
    let parent = unsafe { libc::getpid() };
    // Safety: the child allocates nothing until `execve` but on a failure, and the worker has no other thread.
    match unsafe { libc::fork() } {
        -1 => Err(format!("fork: {}", io::Error::last_os_error())),
        0 => {
            drop(report_read);
            // Safety: in the child, which never returns.
            unsafe {
                become_reader(
                    account,
                    launch,
                    &environment,
                    parent,
                    report_write.as_raw_fd(),
                )
            }
        }
        pid => {
            drop(report_write);
            let mut message = [0u8; 512];
            let count = loop {
                match rustix::io::read(report_read.as_fd(), &mut message) {
                    Err(rustix::io::Errno::INTR) => continue,
                    other => break other.unwrap_or(0),
                }
            };
            if count > 0 {
                reap(pid);
                return Err(String::from_utf8_lossy(&message[..count]).into_owned());
            }
            Ok(pid)
        }
    }
}

/// The reader's child: a session and group of its own, the privilege drop, the death signal, no new privileges, the
/// private directory made as the greeter without following a link, then `execve` with the explicit environment and
/// nothing open but standard input (`/dev/null`), output and error.
///
/// # Safety
/// Call in the child of `fork` only. It never returns.
unsafe fn become_reader(
    account: &Account,
    launch: &Launch,
    environment: &[*const libc::c_char],
    parent: libc::pid_t,
    report: libc::c_int,
) -> ! {
    let fail = |step: &str| crate::worker::fail(report, step);
    if libc::setsid() < 0 {
        fail("setsid");
    }
    if let Err(step) = drop_to(account) {
        crate::worker::fail_with(report, step);
    }
    // the death signal is cleared by a change of credentials, so it is set after the drop; a parent gone already is seen
    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
        fail("PR_SET_PDEATHSIG");
    }
    if libc::getppid() != parent {
        crate::worker::fail_with(report, "the worker ended");
    }
    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
        fail("PR_SET_NO_NEW_PRIVS");
    }
    libc::umask(0o077);
    let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let runtime = libc::open(launch.runtime.as_ptr(), flags);
    if runtime < 0 {
        fail("opening the runtime directory");
    }
    if libc::mkdirat(runtime, launch.name.as_ptr(), 0o700) != 0 {
        fail("making the screen reader's directory");
    }
    let directory = libc::openat(runtime, launch.name.as_ptr(), flags);
    if directory < 0 {
        fail("opening the screen reader's directory");
    }
    for sub in [c"config", c"data", c"cache"] {
        if libc::mkdirat(directory, sub.as_ptr(), 0o700) != 0 {
            fail("making the screen reader's directories");
        }
    }
    if libc::fchdir(directory) != 0 {
        fail("fchdir");
    }
    for signal in 1..=libc::SIGRTMAX() {
        if signal != libc::SIGKILL && signal != libc::SIGSTOP {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
    let mut empty: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut empty);
    libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
    let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if null < 0 || libc::dup2(null, libc::STDIN_FILENO) < 0 {
        fail("opening /dev/null");
    }
    let close_range = |first: libc::c_uint, last: libc::c_uint| {
        libc::syscall(libc::SYS_close_range, first, last, 0)
    };
    let kept = report as libc::c_uint;
    if (kept > 3 && close_range(3, kept - 1) != 0) || close_range(kept + 1, libc::c_uint::MAX) != 0
    {
        fail("close_range");
    }
    let arguments = [launch.program.as_ptr(), std::ptr::null()];
    libc::execve(
        launch.program.as_ptr(),
        arguments.as_ptr(),
        environment.as_ptr(),
    );
    fail("execve")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_only_root_can_change_is_taken_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("orca");
        std::fs::write(&mine, "#!/bin/sh\n").unwrap();
        let mine = mine.to_str().unwrap();
        // the file's own path: some systems link `/usr/bin/env` to a multi-call binary
        let env = std::fs::canonicalize("/usr/bin/env").unwrap();
        assert_eq!(
            resolve(&[mine, "/usr/bin/env"]),
            Some(env.clone()),
            "a file of the test's user is skipped"
        );
        assert_eq!(
            resolve(&[mine, "/bin/env"]),
            Some(env),
            "a program behind root's links is the file they lead to"
        );
        assert_eq!(
            resolve(&[mine, "orca", "/nonexistent/orca"]),
            None,
            "a bare name is never looked up"
        );
    }

    #[test]
    fn the_bus_is_the_greeters_own() {
        let runtime = Path::new("/run/user/970");
        assert_eq!(
            bus_address(runtime, None).unwrap(),
            "unix:path=/run/user/970/bus"
        );
        assert_eq!(
            bus_address(runtime, Some("unix:path=/run/user/970/bus")).unwrap(),
            "unix:path=/run/user/970/bus"
        );
        for refused in [
            "unix:abstract=/tmp/dbus-x",
            "tcp:host=localhost,port=1",
            "unix:path=/run/user/970/../1000/bus",
            "unix:path=/run/user/1000/bus",
            "unix:path=/run/user/970/sub/bus",
            "unix:path=/run/user/970/bus,guid=00",
            "unix:path=/run/user/970/bus;unix:path=/tmp/b",
            "unix:path=bus",
        ] {
            assert!(bus_address(runtime, Some(refused)).is_err(), "{refused}");
        }
        assert!(bus_address(Path::new("/run/user/970/.."), None).is_err());
    }

    #[test]
    fn only_the_two_requests_with_a_socket_name_are_understood() {
        assert_eq!(
            parse_request(b"ready wayland-0"),
            Ok(Request::Ready("wayland-0".into()))
        );
        assert_eq!(
            parse_request(b"toggle wayland-12"),
            Ok(Request::Toggle("wayland-12".into()))
        );
        for refused in [
            &b"toggle"[..],
            b"toggle wayland-",
            b"toggle wayland-1 ",
            b"toggle ../wayland-1",
            b"toggle /run/user/970/wayland-1",
            b"toggle wayland-1x",
            b"start wayland-1",
            b"toggle wayland-1234567890",
            b"toggle \xffwayland-1",
        ] {
            assert!(
                parse_request(refused).is_err(),
                "{:?}",
                String::from_utf8_lossy(refused)
            );
        }
        assert!(parse_request(&[b'a'; MAX_REQUEST + 1]).is_err());
    }

    #[test]
    fn the_reader_runs_only_while_wanted_and_shown() {
        let cases = [
            (true, true, true),
            (true, false, false),
            (false, true, false),
            (false, false, false),
        ];
        for (wanted, shown, runs) in cases {
            assert_eq!(Wish { wanted, shown }.runs(), runs, "{wanted} {shown}");
        }
    }

    /// A fake `/proc` with `pid → parent`.
    fn fake_proc(tree: &[(u32, u32)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (pid, parent) in tree {
            let dir = root.path().join(pid.to_string());
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(
                dir.join("stat"),
                format!("{pid} (a (b) c) S {parent} 1 1 0"),
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn only_the_leaders_descendants_descend_from_it() {
        // worker 10 → leader 20 → login 30 → 31; worker 10 → reader 40 → 41; 50 is a loop; 60 has no entry above it
        let root = fake_proc(&[
            (10, 1),
            (20, 10),
            (30, 20),
            (31, 30),
            (40, 10),
            (41, 40),
            (50, 50),
            (60, 70),
        ]);
        let from_leader = |pid| descends_from(root.path(), pid, 20);
        assert!(from_leader(20) && from_leader(30) && from_leader(31));
        assert!(
            !from_leader(10) && !from_leader(40) && !from_leader(41),
            "the reader is a sibling"
        );
        assert!(!from_leader(50) && !from_leader(60) && !from_leader(99));
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < end, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn alive(pid: u32) -> bool {
        // a zombie is not alive: it has ended, only its parent has not reaped it
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat[stat.rfind(')').unwrap() + 1..]
                .trim_start()
                .starts_with('Z')
        })
    }

    /// A fake reader that writes what it was given to `out`, starts a daemon of its own session and waits.
    fn fake_reader(dir: &Path, out: &Path) -> PathBuf {
        let script = dir.join("fake-reader");
        let out = out.display();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 /usr/bin/env > {out}/env\n\
                 /usr/bin/ls /proc/self/fd > {out}/fd\n\
                 /usr/bin/stat -c %a \"$HOME\" \"$XDG_CONFIG_HOME\" \"$XDG_DATA_HOME\" \"$XDG_CACHE_HOME\" > {out}/modes\n\
                 /usr/bin/setsid /usr/bin/sleep 300 &\n\
                 echo $! > {out}/daemon\n\
                 echo ready > {out}/ready\n\
                 exec /usr/bin/sleep 300\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        script
    }

    #[test]
    fn the_reader_gets_only_its_environment_and_directories_and_its_strays_end_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, out) = (dir.path().join("run"), dir.path().join("out"));
        std::fs::create_dir(&runtime).unwrap();
        std::fs::create_dir(&out).unwrap();
        let script = fake_reader(dir.path(), &out);
        // Safety: `getuid`, `getgid` and `open` with a valid path have no other preconditions.
        let (uid, gid, leaked) = unsafe {
            (
                libc::getuid(),
                libc::getgid(),
                libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY),
            )
        };
        assert!(leaked > 2);
        let login = crate::users::by_uid(uid).unwrap().unwrap().login;
        let mut leader = std::process::Command::new("/usr/bin/sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let procs = dir.path().join("cgroup.procs");
        let mut reader = Reader::new(
            Account {
                login: CString::new(login).unwrap(),
                uid,
                gid,
            },
            Some(script.as_path()),
            runtime.clone(),
            None,
            leader.id(),
            Some(procs.clone()),
            true,
        );
        assert_eq!(reader.request(b"ready wayland-7"), None);
        assert!(
            reader.is_running(),
            "the machine's setting starts it once the socket is known"
        );
        wait_for("the fake reader wrote", || out.join("ready").exists());
        let env = std::fs::read_to_string(out.join("env")).unwrap();
        let names: std::collections::BTreeSet<&str> = env
            .lines()
            .filter_map(|l| l.split_once('=').map(|(name, _)| name))
            .filter(|name| !["PWD", "SHLVL", "_", "OLDPWD"].contains(name))
            .collect();
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            [
                "DBUS_SESSION_BUS_ADDRESS",
                "HOME",
                "WAYLAND_DISPLAY",
                "XDG_CACHE_HOME",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_RUNTIME_DIR"
            ],
            "{env}"
        );
        assert!(env.contains("WAYLAND_DISPLAY=wayland-7\n"), "{env}");
        assert!(
            env.contains(&format!(
                "DBUS_SESSION_BUS_ADDRESS=unix:path={}/bus\n",
                runtime.display()
            )),
            "{env}"
        );
        // `ls` opens the directory it lists as descriptor 3; nothing else is open
        assert_eq!(
            std::fs::read_to_string(out.join("fd")).unwrap(),
            "0\n1\n2\n3\n",
            "descriptor {leaked} leaked?"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("modes")).unwrap(),
            "700\n700\n700\n700\n"
        );
        let home = env
            .lines()
            .find_map(|l| l.strip_prefix("HOME="))
            .unwrap()
            .to_owned();
        assert!(Path::new(&home).parent() == Some(runtime.as_path()));

        let daemon: u32 = std::fs::read_to_string(out.join("daemon"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for("the daemon started", || alive(daemon));
        std::fs::write(
            &procs,
            format!("{}\n{daemon}\n{}\n", leader.id(), std::process::id()),
        )
        .unwrap();
        assert_eq!(reader.request(b"toggle wayland-7"), Some(Reply::Off));
        assert!(!reader.is_running());
        wait_for("the daemon that left the reader's group ended", || {
            !alive(daemon)
        });
        assert!(alive(leader.id()), "the leader's tree stays");
        assert!(!Path::new(&home).exists(), "the directory is removed");

        assert_eq!(reader.request(b"toggle wayland-7"), Some(Reply::On));
        assert!(reader.is_running());
        reader.shown(false);
        assert!(!reader.is_running(), "another session took the console");
        reader.shown(true);
        assert!(
            reader.is_running(),
            "back at the login screen, still wanted"
        );
        drop(reader);
        leader.kill().unwrap();
        leader.wait().unwrap();
        // Safety: the descriptor was opened above and is not used after.
        unsafe { libc::close(leaked) };
    }

    #[test]
    fn without_a_reader_a_toggle_says_so_and_wants_nothing() {
        let mut reader = Reader::new(
            Account {
                login: CString::new("nobody").unwrap(),
                uid: 65534,
                gid: 65534,
            },
            None,
            PathBuf::from("/run/user/65534"),
            None,
            1,
            None,
            false,
        );
        assert_eq!(
            reader.request(b"toggle wayland-0"),
            Some(Reply::Unavailable)
        );
        assert!(!reader.wish.wanted);
        assert_eq!(reader.request(b"start wayland-0"), None);
    }

    #[test]
    fn the_strays_are_every_process_not_kept() {
        assert_eq!(strays("10\n20\nbad\n30\n", |pid| pid == 20), [10, 30]);
    }
}

//! Who is on the other end of a session bus call. Every client of the bus runs as the same user, so the user id says
//! nothing; the caller is the process behind its connection, taken from the `ProcessFD` the bus daemon gives
//! (`GetConnectionCredentials`). The descriptor keeps the process id from being reused for another process while the
//! call is judged. A daemon that gives no descriptor (older than 1.15) is asked for the `ProcessID` instead and the
//! pidfd is opened here: until then the process id could name another process if the caller exited and its connection
//! lived on in a child. A caller without either is anonymous and trusted with nothing.
//!
//! A caller in a sandbox shares the user id as well, so it is told apart the way the desktop portal does it: a Flatpak
//! has `/.flatpak-info` in its root, a Snap has an AppArmor label that starts with `snap.`. A sandboxed caller may not
//! change anything.
//!
//! A service of the shell answers only the server, which started it: a caller is judged by its [`Relation`] to the
//! service's own process, itself or its parent. Other shell processes ask the server to call for them. (The server also
//! starts the applications, so "started by the server" would let every application in.)
//!
//! Two processes are one sandbox instance ([`same_instance`]) when they are the same process, or two processes of one
//! Flatpak instance (a Flatpak's bus connection is made by its `xdg-dbus-proxy`, another process with the same
//! `/.flatpak-info`). An unsandboxed process could make a `/.flatpak-info` of its own in a user namespace and pass for a
//! Flatpak instance, but it can reach every name on the bus directly already, so this gains it nothing.
//!
//! [`signal`] tells who sent a signal, on either bus. [`lookup::Lookup`] asks who called on a connection of its own, for a
//! service that must not wait on the connection that reads its calls.

pub mod lookup;
pub mod signal;

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use zbus::fdo::ConnectionCredentials;
use zbus::names::UniqueName;

/// A process held by a pidfd.
#[derive(Debug)]
pub struct ProcessHandle {
    pub pid: u32,
    fd: OwnedFd,
}

impl ProcessHandle {
    /// Hold the process `pid`; fails when it is gone.
    pub fn open(pid: u32) -> std::io::Result<Self> {
        let raw = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let fd = rustix::process::pidfd_open(raw, rustix::process::PidfdFlags::empty())?;
        Ok(Self { pid, fd })
    }

    /// Hold the process behind `fd`, a pidfd the bus daemon passed (`ProcessFD`); its pid is the one the kernel names
    /// for the descriptor. Fails when `fd` is not a pidfd of a process in this pid namespace.
    pub fn from_pidfd(fd: OwnedFd) -> std::io::Result<Self> {
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))?;
        let pid = info
            .lines()
            .find_map(|l| l.strip_prefix("Pid:"))
            .and_then(|p| p.trim().parse::<i64>().ok())
            .filter(|p| *p > 0)
            .and_then(|p| u32::try_from(p).ok())
            .ok_or_else(|| {
                std::io::Error::other("the descriptor is not a pidfd of a visible process")
            })?;
        Ok(Self { pid, fd })
    }

    /// Hold the process that connected the other end of `socket`, as the kernel names it (`SO_PEERPIDFD`, Linux 6.5).
    pub fn of_peer(socket: &impl AsFd) -> std::io::Result<Self> {
        let mut fd: libc::c_int = -1;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // Safety: getsockopt writes at most `len` bytes into the int it is given.
        let result = unsafe {
            libc::getsockopt(
                socket.as_fd().as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERPIDFD,
                (&mut fd as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if fd < 0 {
            return Err(std::io::Error::other("the socket has no peer process"));
        }
        // Safety: the kernel just made the descriptor for this call and nothing else owns it.
        Self::from_pidfd(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// The pidfd: readable once the process has exited.
    pub fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Another handle on the same process.
    pub fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self {
            pid: self.pid,
            fd: self.fd.try_clone()?,
        })
    }

    /// Whether both handles hold one process: the same pid, and neither has exited (a pidfd of a process that exited
    /// is readable, so a pid reused since is never taken for it).
    pub fn is(&self, other: &ProcessHandle) -> bool {
        self.pid == other.pid && !self.has_exited() && !other.has_exited()
    }

    /// Whether the process has exited: its pidfd is readable then.
    pub fn has_exited(&self) -> bool {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        matches!(poll(&mut fds, Some(&now)), Ok(ready) if ready > 0)
            && fds[0].revents().contains(PollFlags::IN)
    }

    /// Send `signal` to the process; fails when it has exited.
    pub fn send(&self, signal: i32) -> std::io::Result<()> {
        let signal = rustix::process::Signal::from_named_raw(signal)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        rustix::process::pidfd_send_signal(&self.fd, signal)?;
        Ok(())
    }
}

/// The sandbox a caller runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    Flatpak,
    Snap,
}

/// A process the bus daemon named for a connection.
#[derive(Debug)]
pub struct Identified {
    pub process: ProcessHandle,
    pub sandbox: Option<Sandbox>,
    /// The program the process runs, as `/proc/<pid>/exe` names it.
    pub exe: Option<PathBuf>,
}

/// How a caller is related to the process that serves the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Itself,
    Parent,
    Other,
}

/// The process behind a connection of the bus.
#[derive(Debug)]
pub enum Caller {
    /// The daemon gave no usable process descriptor, or the process could not be classified or is already gone.
    Anonymous,
    /// The process is known (by its descriptor) but `/proc` refused its sandbox check, as it does for a process that is not
    /// dumpable. It is never taken for unsandboxed: [`Caller::unsandboxed`] and [`Caller::require_relation`] refuse it, and
    /// only a caller that knows the process by other means (the server's own supervised child) may accept it.
    Unclassified(ProcessHandle),
    Process(Identified),
}

/// Why a caller was refused. Each has an error name of its own on the bus ([`CallerError`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    Anonymous,
    Sandboxed(Sandbox),
    Untrusted,
}

/// What a method of a shell service fails with when its caller is refused, one name for each way.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Error")]
pub enum CallerError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AnonymousCaller(String),
    FlatpakCaller(String),
    SnapCaller(String),
    UntrustedCaller(String),
}

impl From<zbus::fdo::Error> for CallerError {
    fn from(e: zbus::fdo::Error) -> Self {
        CallerError::ZBus(e.into())
    }
}

impl From<Denial> for CallerError {
    fn from(denial: Denial) -> Self {
        match denial {
            Denial::Anonymous => {
                CallerError::AnonymousCaller("the bus gave no process for this caller".into())
            }
            Denial::Sandboxed(Sandbox::Flatpak) => {
                CallerError::FlatpakCaller("a Flatpak may not use this method".into())
            }
            Denial::Sandboxed(Sandbox::Snap) => {
                CallerError::SnapCaller("a Snap may not use this method".into())
            }
            Denial::Untrusted => {
                CallerError::UntrustedCaller("this process may not use this method".into())
            }
        }
    }
}

/// The process a daemon names: the `ProcessFD` when it gives one, else a pidfd opened for its `ProcessID` (a daemon
/// older than 1.15 has no descriptor to give).
fn process_of(credentials: &ConnectionCredentials) -> Option<ProcessHandle> {
    if let Some(fd) = credentials.process_fd() {
        let fd = std::os::fd::AsFd::as_fd(fd).try_clone_to_owned().ok()?;
        let process = ProcessHandle::from_pidfd(fd).ok()?;
        let agrees = credentials
            .process_id()
            .is_none_or(|pid| pid == process.pid);
        return agrees.then_some(process);
    }
    let pid = credentials.process_id()?;
    tracing::debug!(
        pid,
        "the bus gave no process descriptor; the caller is identified by its process id"
    );
    ProcessHandle::open(pid).ok()
}

impl Caller {
    pub fn pid(&self) -> Option<u32> {
        match self {
            Caller::Anonymous => None,
            Caller::Unclassified(process) => Some(process.pid),
            Caller::Process(p) => Some(p.process.pid),
        }
    }

    /// Read the credentials the daemon gave for a connection; `proc_root` is `/proc`.
    pub fn from_credentials(credentials: &ConnectionCredentials, proc_root: &Path) -> Caller {
        let Some(process) = process_of(credentials) else {
            return Caller::Anonymous;
        };
        Self::of_labelled(
            process,
            Ok(credentials.linux_security_label().cloned()),
            proc_root,
        )
    }

    /// Like [`Caller::from_credentials`], but only the `ProcessFD` the daemon passed names the process: a daemon that
    /// gives a bare `ProcessID` gives an anonymous caller.
    pub fn from_process_fd(credentials: &ConnectionCredentials, proc_root: &Path) -> Caller {
        if credentials.process_fd().is_none() {
            return Caller::Anonymous;
        }
        Self::from_credentials(credentials, proc_root)
    }

    /// The process `process` holds, classified by its own AppArmor label and root (a Wayland client, from its
    /// [`ProcessHandle::of_peer`]).
    pub fn of_process(process: ProcessHandle, proc_root: &Path) -> Caller {
        let label = security_label(proc_root, process.pid);
        Self::of_labelled(process, label, proc_root)
    }

    fn of_labelled(
        process: ProcessHandle,
        label: std::io::Result<Option<Vec<u8>>>,
        proc_root: &Path,
    ) -> Caller {
        let sandbox = match label
            .and_then(|label| classify(proc_root, process.pid, label.as_deref()))
        {
            Ok(sandbox) => sandbox,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && !process.has_exited() => {
                return Caller::Unclassified(process)
            }
            Err(_) => return Caller::Anonymous,
        };
        let exe = std::fs::read_link(proc_root.join(process.pid.to_string()).join("exe")).ok();
        // the answers above describe the process only while it is the one the descriptor names
        if process.has_exited() {
            return Caller::Anonymous;
        }
        Caller::Process(Identified {
            process,
            sandbox,
            exe,
        })
    }

    /// The process, when the caller is neither anonymous nor sandboxed.
    pub fn unsandboxed(&self) -> Result<&Identified, Denial> {
        let caller = match self {
            Caller::Process(caller) => caller,
            Caller::Unclassified(_) => return Err(Denial::Untrusted),
            Caller::Anonymous => return Err(Denial::Anonymous),
        };
        match caller.sandbox {
            Some(sandbox) => Err(Denial::Sandboxed(sandbox)),
            None => Ok(caller),
        }
    }

    /// Allow an unsandboxed caller that stands in one of `allowed` to the process `own_pid`, whose parent is `own_parent`.
    pub fn require_relation(
        &self,
        own_pid: u32,
        own_parent: u32,
        allowed: &[Relation],
    ) -> Result<(), Denial> {
        let caller = self.unsandboxed()?;
        if allowed.contains(&caller.relation(own_pid, own_parent)) {
            Ok(())
        } else {
            Err(Denial::Untrusted)
        }
    }
}

impl Identified {
    pub fn relation(&self, own_pid: u32, own_parent: u32) -> Relation {
        let pid = self.process.pid;
        if pid == own_pid {
            Relation::Itself
        } else if pid == own_parent {
            Relation::Parent
        } else {
            Relation::Other
        }
    }
}

/// Refuse the caller of a method unless it is unsandboxed and stands in one of `allowed` to this process. A refusal is
/// logged once with the method, never with the arguments.
pub async fn check_call(
    connection: &zbus::Connection,
    header: &zbus::message::Header<'_>,
    interface: &'static str,
    method: &'static str,
    allowed: &[Relation],
) -> Result<(), CallerError> {
    let caller = match header.sender() {
        Some(sender) => identify(connection, sender).await,
        None => Caller::Anonymous,
    };
    caller
        .require_relation(std::process::id(), own_parent(), allowed)
        .map_err(|denial| {
            tracing::warn!(
                interface,
                method,
                pid = caller.pid(),
                ?denial,
                "the caller is refused"
            );
            denial.into()
        })
}

/// The parent of this process.
pub fn own_parent() -> u32 {
    // Safety: getppid has no preconditions.
    unsafe { libc::getppid() as u32 }
}

/// The sandbox of process `pid`; an error when it cannot be told.
fn classify(proc_root: &Path, pid: u32, label: Option<&[u8]>) -> std::io::Result<Option<Sandbox>> {
    if label.is_some_and(|l| l.starts_with(b"snap.")) {
        return Ok(Some(Sandbox::Snap));
    }
    match std::fs::metadata(
        proc_root
            .join(pid.to_string())
            .join("root")
            .join(".flatpak-info"),
    ) {
        Ok(_) => Ok(Some(Sandbox::Flatpak)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The AppArmor label of process `pid`: AppArmor's own entry, else the one of the first security module. `None` when no
/// module that keeps a label is active (the kernel answers `EINVAL`).
fn security_label(proc_root: &Path, pid: u32) -> std::io::Result<Option<Vec<u8>>> {
    let attr = proc_root.join(pid.to_string()).join("attr");
    let read = match std::fs::read(attr.join("apparmor").join("current")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::read(attr.join("current")),
        read => read,
    };
    match read {
        Ok(label) => Ok(Some(label)),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The most of a `/.flatpak-info` that is read; the file is a few hundred bytes.
const FLATPAK_INFO_LIMIT: u64 = 64 * 1024;

/// The application id and the instance id of the Flatpak process `pid`, from its `/.flatpak-info`; `None` when the file
/// cannot be read or lacks either.
fn flatpak_instance(proc_root: &Path, pid: u32) -> Option<(String, String)> {
    let path = proc_root
        .join(pid.to_string())
        .join("root")
        .join(".flatpak-info");
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(FLATPAK_INFO_LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    let (mut section, mut app, mut instance) = ("", None, None);
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
        } else if let Some((key, value)) = line.split_once('=') {
            match (section, key.trim()) {
                ("Application", "name") => app = Some(value.trim().to_owned()),
                ("Instance", "instance-id") => instance = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    let present = |v: Option<String>| v.filter(|v| !v.is_empty());
    Some((present(app)?, present(instance)?))
}

/// Whether `a` and `b` are one sandbox instance: the same process, or two processes of one Flatpak instance (the same
/// application id and instance id in both `/.flatpak-info` files). A Snap has no instance to compare, so it matches
/// only itself, and so does an unsandboxed process; a sandboxed and an unsandboxed process never match. Anything that
/// cannot be read is no match.
pub fn same_instance(a: &Identified, b: &Identified, proc_root: &Path) -> bool {
    if a.sandbox != b.sandbox {
        return false;
    }
    if a.process.is(&b.process) {
        return true;
    }
    if a.sandbox != Some(Sandbox::Flatpak) {
        return false;
    }
    let (Some(first), Some(second)) = (
        flatpak_instance(proc_root, a.process.pid),
        flatpak_instance(proc_root, b.process.pid),
    ) else {
        return false;
    };
    // the files describe the processes only while they are the ones the descriptors name
    first == second && !a.process.has_exited() && !b.process.has_exited()
}

/// What a service logs once at start when the bus gives no `ProcessFD` for its own connection: every caller
/// [`Caller::from_process_fd`] judges is then refused, and nothing else says why.
pub const NO_PROCESS_FD: &str = "the bus gives no ProcessFD (dbus 1.16 or later does): every caller that must be named by its process descriptor is refused";

/// [`NO_PROCESS_FD`] when `own`, the credentials the bus gives for a connection of this process, holds no `ProcessFD`.
pub fn process_fd_notice(own: &ConnectionCredentials) -> Option<&'static str> {
    own.process_fd().is_none().then_some(NO_PROCESS_FD)
}

/// The caller behind `sender` on `connection`.
pub async fn identify(connection: &zbus::Connection, sender: &UniqueName<'_>) -> Caller {
    let Ok(proxy) = zbus::fdo::DBusProxy::new(connection).await else {
        return Caller::Anonymous;
    };
    match proxy
        .get_connection_credentials(sender.clone().into())
        .await
    {
        Ok(credentials) => Caller::from_credentials(&credentials, Path::new("/proc")),
        Err(_) => Caller::Anonymous,
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;

    use super::*;

    /// The threads of this process the bus connections' pool for blocking calls runs.
    fn pool_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
            .filter(|name| name.starts_with("blocking-"))
            .count()
    }

    /// Set in the copy of the test program that runs the pool test alone.
    const POOL_ALONE: &str = "CUPERTINO_POOL_TEST_ALONE";

    /// A bus connection runs its connect and its socket's shutdown on a pool of threads. Once a call ends, the pool's
    /// thread ends too: a thread kept idle would wake twice a second in every process with a bus connection, on a
    /// desktop where nothing happens. The pool is one per process, so the test runs again in a process of its own,
    /// where every pool thread is one its call made, whatever else the test program runs beside it.
    #[test]
    fn the_blocking_pool_ends_its_threads_once_idle() {
        if std::env::var_os(POOL_ALONE).is_none() {
            let name = concat!(
                module_path!(),
                "::the_blocking_pool_ends_its_threads_once_idle"
            );
            let name = name.split_once("::").map_or(name, |(_, path)| path);
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--test-threads=1", "--nocapture"])
                .env(POOL_ALONE, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && stdout.contains("1 passed"),
                "{stdout}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        assert_eq!(pool_threads(), 0, "no pool thread before the call");
        async_io::block_on(blocking::unblock(|| ()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool_threads() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "{} pool threads are still waiting for work",
                pool_threads()
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn own_pidfd() -> OwnedFd {
        // Safety: pidfd_open returns a new descriptor for our own pid, which nothing else owns.
        unsafe {
            std::os::fd::FromRawFd::from_raw_fd(libc::syscall(
                libc::SYS_pidfd_open,
                std::process::id() as libc::pid_t,
                0,
            ) as i32)
        }
    }

    fn proc_root_with_flatpak(pid: u32) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(pid.to_string()).join("root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".flatpak-info"), "[Application]\n").unwrap();
        root
    }

    fn process(sandbox: Option<Sandbox>, exe: &str) -> Caller {
        Caller::Process(Identified {
            process: ProcessHandle::from_pidfd(own_pidfd()).unwrap(),
            sandbox,
            exe: Some(exe.into()),
        })
    }

    #[test]
    fn a_connection_without_any_process_is_anonymous() {
        let caller =
            Caller::from_credentials(&ConnectionCredentials::default(), Path::new("/proc"));
        assert!(matches!(caller, Caller::Anonymous));
        assert_eq!(caller.unsandboxed().err(), Some(Denial::Anonymous));
    }

    #[test]
    fn a_daemon_without_a_descriptor_is_judged_by_the_process_id() {
        let credentials = ConnectionCredentials::default().set_process_id(std::process::id());
        let Caller::Process(host) = Caller::from_credentials(&credentials, Path::new("/proc"))
        else {
            panic!("identified");
        };
        assert_eq!(host.process.pid, std::process::id());
        let gone = ConnectionCredentials::default().set_process_id(i32::MAX as u32);
        assert!(matches!(
            Caller::from_credentials(&gone, Path::new("/proc")),
            Caller::Anonymous
        ));
    }

    #[test]
    fn a_host_process_is_classified_from_its_descriptor() {
        let credentials = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        let Caller::Process(host) = Caller::from_credentials(&credentials, Path::new("/proc"))
        else {
            panic!("a pidfd of a live process identifies it");
        };
        assert_eq!(host.process.pid, std::process::id());
        assert_eq!(host.sandbox, None);
        assert_eq!(
            host.exe,
            std::fs::read_link("/proc/self/exe").ok(),
            "the program comes from /proc/<pid>/exe"
        );
    }

    #[test]
    fn a_flatpak_is_found_by_the_info_file_in_its_root() {
        let proc_root = proc_root_with_flatpak(std::process::id());
        let credentials = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        let Caller::Process(flatpak) = Caller::from_credentials(&credentials, proc_root.path())
        else {
            panic!("identified");
        };
        assert_eq!(flatpak.sandbox, Some(Sandbox::Flatpak));
    }

    #[test]
    fn a_snap_is_found_by_its_security_label() {
        let proc_root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            proc_root
                .path()
                .join(std::process::id().to_string())
                .join("root"),
        )
        .unwrap();
        let credentials = ConnectionCredentials::default()
            .set_process_fd(own_pidfd().into())
            .set_linux_security_label(b"snap.firefox.firefox (enforce)\0".to_vec());
        let Caller::Process(snap) = Caller::from_credentials(&credentials, proc_root.path()) else {
            panic!("identified");
        };
        assert_eq!(snap.sandbox, Some(Sandbox::Snap));
    }

    #[test]
    fn a_process_id_that_disagrees_with_the_descriptor_is_anonymous() {
        let credentials = ConnectionCredentials::default()
            .set_process_fd(own_pidfd().into())
            .set_process_id(std::process::id() + 1);
        assert!(matches!(
            Caller::from_credentials(&credentials, Path::new("/proc")),
            Caller::Anonymous
        ));
    }

    #[test]
    fn a_process_that_cannot_be_classified_is_anonymous() {
        let proc_root = tempfile::tempdir().unwrap();
        let root = proc_root.path().join(std::process::id().to_string());
        std::fs::create_dir_all(&root).unwrap();
        // `root` is a file here: looking for .flatpak-info under it fails with something other than "not found"
        std::fs::write(root.join("root"), "").unwrap();
        let credentials = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        assert!(matches!(
            Caller::from_credentials(&credentials, proc_root.path()),
            Caller::Anonymous
        ));
    }

    #[test]
    fn a_process_whose_root_cannot_be_read_is_known_but_never_unsandboxed() {
        use std::os::unix::fs::PermissionsExt;
        // Safety: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let proc_root = tempfile::tempdir().unwrap();
        let root = proc_root
            .path()
            .join(std::process::id().to_string())
            .join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let credentials = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        let caller = Caller::from_credentials(&credentials, proc_root.path());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(caller, Caller::Unclassified(_)), "{caller:?}");
        assert_eq!(caller.pid(), Some(std::process::id()));
        assert_eq!(caller.unsandboxed().err(), Some(Denial::Untrusted));
        let me = std::process::id();
        assert_eq!(
            caller.require_relation(
                me,
                1,
                &[Relation::Itself, Relation::Parent, Relation::Other]
            ),
            Err(Denial::Untrusted)
        );
    }

    #[test]
    fn a_process_that_is_not_dumpable_is_unclassified_by_the_real_proc() {
        // Safety: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        // Safety: the child calls only prctl, pause and _exit, which are async-signal-safe.
        let child = unsafe { libc::fork() };
        if child == 0 {
            unsafe {
                libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
                libc::pause();
                libc::_exit(0);
            }
        }
        assert!(child > 0, "fork failed");
        let handle = ProcessHandle::open(child as u32).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let caller = loop {
            let credentials = ConnectionCredentials::default().set_process_id(child as u32);
            let caller = Caller::from_credentials(&credentials, Path::new("/proc"));
            // the child may not have made itself undumpable yet
            if !matches!(caller, Caller::Process(_)) || std::time::Instant::now() > end {
                break caller;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        handle.send(libc::SIGKILL).unwrap();
        // Safety: reaps the child forked above.
        unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
        assert!(matches!(caller, Caller::Unclassified(_)), "{caller:?}");
        assert_eq!(caller.pid(), Some(child as u32));
        assert_eq!(caller.unsandboxed().err(), Some(Denial::Untrusted));
    }

    #[test]
    fn a_caller_is_judged_by_its_relation_to_the_service() {
        let me = std::process::id();
        let host = process(None, "/usr/bin/cupertino-files");
        let relation = |own_pid, own_parent, allowed: &[Relation]| {
            host.require_relation(own_pid, own_parent, allowed)
        };
        assert_eq!(relation(me, 1, &[Relation::Itself]), Ok(()));
        assert_eq!(relation(1, me, &[Relation::Parent]), Ok(()));
        assert_eq!(
            relation(1, 1, &[Relation::Itself, Relation::Parent]),
            Err(Denial::Untrusted),
            "a process with the same parent is no relation"
        );
    }

    /// Another live process, killed and reaped on drop.
    struct Other(std::process::Child);

    impl Other {
        fn start() -> Other {
            Other(
                std::process::Command::new("sleep")
                    .arg("60")
                    .spawn()
                    .expect("sleep"),
            )
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Other {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn held(pid: u32, sandbox: Option<Sandbox>) -> Identified {
        Identified {
            process: ProcessHandle::open(pid).unwrap(),
            sandbox,
            exe: None,
        }
    }

    fn write_flatpak_info(proc_root: &Path, pid: u32, app: &str, instance: &str) {
        let dir = proc_root.join(pid.to_string()).join("root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".flatpak-info"),
            format!("[Application]\nname={app}\nruntime=runtime/x\n\n[Instance]\ninstance-id={instance}\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_process_is_its_own_instance_and_another_unsandboxed_process_is_not() {
        let other = Other::start();
        let me = std::process::id();
        let proc_root = tempfile::tempdir().unwrap();
        assert!(same_instance(
            &held(me, None),
            &held(me, None),
            proc_root.path()
        ));
        assert!(!same_instance(
            &held(me, None),
            &held(other.pid(), None),
            proc_root.path()
        ));
    }

    #[test]
    fn a_process_that_exited_is_not_the_same_process_any_more() {
        let mut other = Other::start();
        let first = held(other.pid(), None);
        let second = held(other.pid(), None);
        other.0.kill().unwrap();
        other.0.wait().unwrap();
        assert!(!same_instance(&first, &second, Path::new("/proc")));
    }

    #[test]
    fn the_proxy_of_a_flatpak_is_the_same_instance_as_the_application() {
        let proxy = Other::start();
        let me = std::process::id();
        let proc_root = tempfile::tempdir().unwrap();
        write_flatpak_info(proc_root.path(), me, "org.example.App", "1234");
        write_flatpak_info(proc_root.path(), proxy.pid(), "org.example.App", "1234");
        let flatpak = Some(Sandbox::Flatpak);
        assert!(same_instance(
            &held(me, flatpak),
            &held(proxy.pid(), flatpak),
            proc_root.path()
        ));
    }

    #[test]
    fn a_flatpak_process_that_exited_is_not_the_instance_its_info_file_names() {
        let mut proxy = Other::start();
        let me = std::process::id();
        let proc_root = tempfile::tempdir().unwrap();
        write_flatpak_info(proc_root.path(), me, "org.example.App", "1234");
        write_flatpak_info(proc_root.path(), proxy.pid(), "org.example.App", "1234");
        let flatpak = Some(Sandbox::Flatpak);
        let gone = held(proxy.pid(), flatpak);
        proxy.0.kill().unwrap();
        proxy.0.wait().unwrap();
        assert!(!same_instance(&held(me, flatpak), &gone, proc_root.path()));
    }

    #[test]
    fn another_flatpak_instance_or_application_is_not_the_same_instance() {
        let other = Other::start();
        let me = std::process::id();
        let flatpak = Some(Sandbox::Flatpak);
        for (app, instance) in [("org.example.App", "5678"), ("org.example.Other", "1234")] {
            let proc_root = tempfile::tempdir().unwrap();
            write_flatpak_info(proc_root.path(), me, "org.example.App", "1234");
            write_flatpak_info(proc_root.path(), other.pid(), app, instance);
            assert!(
                !same_instance(
                    &held(me, flatpak),
                    &held(other.pid(), flatpak),
                    proc_root.path()
                ),
                "{app} {instance}"
            );
        }
    }

    #[test]
    fn a_flatpak_info_that_cannot_be_read_or_lacks_the_instance_is_no_match() {
        let other = Other::start();
        let me = std::process::id();
        let flatpak = Some(Sandbox::Flatpak);
        let proc_root = tempfile::tempdir().unwrap();
        write_flatpak_info(proc_root.path(), me, "org.example.App", "1234");
        assert!(!same_instance(
            &held(me, flatpak),
            &held(other.pid(), flatpak),
            proc_root.path()
        ));
        let dir = proc_root.path().join(other.pid().to_string()).join("root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".flatpak-info"),
            "[Application]\nname=org.example.App\n",
        )
        .unwrap();
        assert!(!same_instance(
            &held(me, flatpak),
            &held(other.pid(), flatpak),
            proc_root.path()
        ));
    }

    #[test]
    fn a_sandboxed_and_an_unsandboxed_process_are_never_one_instance() {
        let other = Other::start();
        let me = std::process::id();
        let proc_root = tempfile::tempdir().unwrap();
        write_flatpak_info(proc_root.path(), me, "org.example.App", "1234");
        write_flatpak_info(proc_root.path(), other.pid(), "org.example.App", "1234");
        for sandbox in [Sandbox::Flatpak, Sandbox::Snap] {
            let (sandboxed, host) = (held(me, Some(sandbox)), held(other.pid(), None));
            assert!(!same_instance(&sandboxed, &host, proc_root.path()));
            assert!(!same_instance(&host, &sandboxed, proc_root.path()));
            // one process taken for sandboxed on one side only is no match either
            assert!(!same_instance(
                &sandboxed,
                &held(me, None),
                proc_root.path()
            ));
        }
    }

    #[test]
    fn two_processes_of_a_snap_are_not_one_instance() {
        let other = Other::start();
        let me = std::process::id();
        let snap = Some(Sandbox::Snap);
        let proc_root = tempfile::tempdir().unwrap();
        assert!(same_instance(
            &held(me, snap),
            &held(me, snap),
            proc_root.path()
        ));
        assert!(!same_instance(
            &held(me, snap),
            &held(other.pid(), snap),
            proc_root.path()
        ));
    }

    #[test]
    fn credentials_without_a_process_descriptor_name_no_owner() {
        let credentials = ConnectionCredentials::default().set_process_id(std::process::id());
        assert!(matches!(
            Caller::from_process_fd(&credentials, Path::new("/proc")),
            Caller::Anonymous
        ));
        let credentials = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        assert!(matches!(
            Caller::from_process_fd(&credentials, Path::new("/proc")),
            Caller::Process(_)
        ));
    }

    #[test]
    fn a_bus_that_gives_no_process_descriptor_is_named_with_the_version_it_needs() {
        let bare = ConnectionCredentials::default().set_process_id(std::process::id());
        assert_eq!(process_fd_notice(&bare), Some(NO_PROCESS_FD));
        assert!(NO_PROCESS_FD.contains("dbus 1.16"));
        let held = ConnectionCredentials::default().set_process_fd(own_pidfd().into());
        assert_eq!(process_fd_notice(&held), None);
    }

    #[test]
    fn the_peer_of_a_socket_is_the_process_that_connected() {
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = ProcessHandle::of_peer(&ours).unwrap();
        assert_eq!(peer.pid, std::process::id());
        assert!(peer.is(&ProcessHandle::open(std::process::id()).unwrap()));
    }

    #[test]
    fn a_wayland_client_is_classified_by_its_own_label_and_root() {
        let me = std::process::id();
        let classified = |label: &str, flatpak: bool| {
            let proc_root = tempfile::tempdir().unwrap();
            let pid = proc_root.path().join(me.to_string());
            std::fs::create_dir_all(pid.join("attr").join("apparmor")).unwrap();
            std::fs::create_dir_all(pid.join("root")).unwrap();
            std::fs::write(pid.join("attr").join("apparmor").join("current"), label).unwrap();
            if flatpak {
                write_flatpak_info(proc_root.path(), me, "org.example.App", "1");
            }
            match Caller::of_process(ProcessHandle::open(me).unwrap(), proc_root.path()) {
                Caller::Process(p) => p.sandbox,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(classified("unconfined\n", false), None);
        assert_eq!(
            classified("snap.firefox.firefox (enforce)\n", false),
            Some(Sandbox::Snap)
        );
        assert_eq!(classified("unconfined\n", true), Some(Sandbox::Flatpak));
        // the real /proc of a kernel without AppArmor: no label, unsandboxed
        assert!(matches!(
            Caller::of_process(ProcessHandle::open(me).unwrap(), Path::new("/proc")),
            Caller::Process(Identified { sandbox: None, .. })
        ));
    }

    #[test]
    fn a_wayland_client_whose_label_cannot_be_read_is_anonymous() {
        let proc_root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            proc_root
                .path()
                .join(std::process::id().to_string())
                .join("root"),
        )
        .unwrap();
        let process = ProcessHandle::open(std::process::id()).unwrap();
        assert!(matches!(
            Caller::of_process(process, proc_root.path()),
            Caller::Anonymous
        ));
    }

    #[test]
    fn an_anonymous_or_sandboxed_caller_has_no_relation_that_counts() {
        let me = std::process::id();
        assert_eq!(
            Caller::Anonymous.require_relation(me, 1, &[Relation::Itself]),
            Err(Denial::Anonymous)
        );
        for sandbox in [Sandbox::Flatpak, Sandbox::Snap] {
            assert_eq!(
                process(Some(sandbox), "/usr/bin/x").require_relation(me, 1, &[Relation::Itself]),
                Err(Denial::Sandboxed(sandbox))
            );
        }
    }
}

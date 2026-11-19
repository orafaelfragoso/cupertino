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

pub mod component;
pub mod lookup;
pub mod signal;

pub use component::{resolve_component, ComponentCaller};

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

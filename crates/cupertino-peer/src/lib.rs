//! Who is on the other end of a session bus call. Every client of the bus runs as the same user, so the user id says
//! nothing; the caller is the process behind its connection, taken from the `ProcessFD` the bus daemon gives
//! (`GetConnectionCredentials`). The descriptor keeps the process id from being reused for another process while the
//! call is judged. A caller without one is anonymous and trusted with nothing.
//!
//! A caller in a sandbox shares the user id as well, so it is told apart the way the desktop portal does it: a Flatpak
//! has `/.flatpak-info` in its root, a Snap has an AppArmor label that starts with `snap.`. A sandboxed caller may not
//! change anything.
//!
//! A service of the shell answers only the server, which started it: a caller is judged by its [`Relation`] to the
//! service's own process, itself or its parent. Other shell processes ask the server to call for them. (The server also
//! starts the applications, so "started by the server" would let every application in.)

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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
        // Safety: pidfd_open takes a pid and flags and returns a new file descriptor or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Safety: the descriptor was just returned by the kernel and nothing else owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
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

    /// Whether the process has exited: its pidfd is readable then.
    pub fn has_exited(&self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Safety: poll on one valid pollfd with a zero timeout.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        ready > 0 && poll.revents & libc::POLLIN != 0
    }

    /// Send `signal` to the process; fails when it has exited.
    pub fn send(&self, signal: i32) -> std::io::Result<()> {
        // Safety: pidfd_send_signal on a descriptor this value owns, with no siginfo and no flags.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
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

impl Caller {
    pub fn pid(&self) -> Option<u32> {
        match self {
            Caller::Anonymous => None,
            Caller::Process(p) => Some(p.process.pid),
        }
    }

    /// Read the credentials the daemon gave for a connection; `proc_root` is `/proc`.
    pub fn from_credentials(credentials: &ConnectionCredentials, proc_root: &Path) -> Caller {
        let Some(fd) = credentials
            .process_fd()
            .and_then(|fd| std::os::fd::AsFd::as_fd(fd).try_clone_to_owned().ok())
        else {
            return Caller::Anonymous;
        };
        let Ok(process) = ProcessHandle::from_pidfd(fd) else {
            return Caller::Anonymous;
        };
        if credentials
            .process_id()
            .is_some_and(|pid| pid != process.pid)
        {
            return Caller::Anonymous;
        }
        let Ok(sandbox) = classify(
            proc_root,
            process.pid,
            credentials.linux_security_label().map(Vec::as_slice),
        ) else {
            return Caller::Anonymous;
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
        let Caller::Process(caller) = self else {
            return Err(Denial::Anonymous);
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
    fn a_connection_without_a_process_descriptor_is_anonymous() {
        let none = ConnectionCredentials::default().set_process_id(std::process::id());
        let caller = Caller::from_credentials(&none, Path::new("/proc"));
        assert!(matches!(caller, Caller::Anonymous));
        assert_eq!(caller.unsandboxed().err(), Some(Denial::Anonymous));
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

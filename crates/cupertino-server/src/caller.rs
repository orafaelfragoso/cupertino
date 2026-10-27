//! Who is on the other end of a session bus call. Every client of the bus runs as the same user, so the user id says
//! nothing; the caller is the process behind its connection, taken from the `ProcessFD` the bus daemon gives
//! (`GetConnectionCredentials`). The descriptor keeps the process id from being reused for another process while the
//! call is judged. A caller without one is anonymous and trusted with nothing.
//!
//! A caller in a sandbox shares the user id as well, so it is told apart the way the desktop portal does it: a Flatpak
//! has `/.flatpak-info` in its root, a Snap has an AppArmor label that starts with `snap.`. A sandboxed caller may not
//! change anything.

use std::path::{Path, PathBuf};

use zbus::fdo::ConnectionCredentials;
use zbus::names::UniqueName;

use crate::force_quit::ProcessHandle;

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

/// The process behind a connection of the bus.
#[derive(Debug)]
pub enum Caller {
    /// The daemon gave no usable process descriptor, or the process could not be classified or is already gone.
    Anonymous,
    Process(Identified),
}

/// Why a caller was refused. Each has an error name of its own on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    Anonymous,
    Sandboxed(Sandbox),
    Untrusted,
}

/// What a method needs of its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The session agent the server supervises (`cupertino-login`).
    SessionAgent,
    /// The settings application, or the session agent.
    SettingsClient,
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

    /// Whether the caller may use a method that needs `need`. `agents` are the processes the server supervises with the
    /// session agent's role, `own_pid` is the server itself, and `settings_client` says whether a program is the
    /// settings application.
    pub fn authorize(
        &self,
        need: Need,
        agents: &[u32],
        own_pid: u32,
        settings_client: impl Fn(&Path) -> bool,
    ) -> Result<(), Denial> {
        let Caller::Process(caller) = self else {
            return Err(Denial::Anonymous);
        };
        if let Some(sandbox) = caller.sandbox {
            return Err(Denial::Sandboxed(sandbox));
        }
        let pid = caller.process.pid;
        if pid == own_pid || agents.contains(&pid) {
            return Ok(());
        }
        match need {
            Need::SettingsClient if caller.exe.as_deref().is_some_and(settings_client) => Ok(()),
            Need::SessionAgent | Need::SettingsClient => Err(Denial::Untrusted),
        }
    }
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
        assert_eq!(
            caller.authorize(Need::SessionAgent, &[], 0, |_| true),
            Err(Denial::Anonymous)
        );
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
    fn only_the_server_the_agent_and_for_settings_the_settings_program_are_trusted() {
        let me = std::process::id();
        let settings = |exe: &Path| exe == Path::new("/usr/bin/cupertino-settings");
        let host = process(None, "/usr/bin/cupertino-files");
        assert_eq!(
            host.authorize(Need::SessionAgent, &[], 1, settings),
            Err(Denial::Untrusted)
        );
        assert_eq!(
            host.authorize(Need::SettingsClient, &[], 1, settings),
            Err(Denial::Untrusted)
        );
        assert_eq!(
            host.authorize(Need::SessionAgent, &[me], 1, settings),
            Ok(())
        );
        assert_eq!(
            host.authorize(Need::SessionAgent, &[], me, settings),
            Ok(())
        );

        let settings_app = process(None, "/usr/bin/cupertino-settings");
        assert_eq!(
            settings_app.authorize(Need::SettingsClient, &[], 1, settings),
            Ok(())
        );
        assert_eq!(
            settings_app.authorize(Need::SessionAgent, &[], 1, settings),
            Err(Denial::Untrusted),
            "the settings program does not quit applications"
        );
    }

    #[test]
    fn a_sandboxed_caller_is_refused_even_as_the_settings_program_or_the_agent() {
        let me = std::process::id();
        for sandbox in [Sandbox::Flatpak, Sandbox::Snap] {
            let caller = process(Some(sandbox), "/usr/bin/cupertino-settings");
            for need in [Need::SessionAgent, Need::SettingsClient] {
                assert_eq!(
                    caller.authorize(need, &[me], me, |_| true),
                    Err(Denial::Sandboxed(sandbox))
                );
            }
        }
    }
}

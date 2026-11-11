//! Who is on the other end of a session bus call (the identification is `cupertino-peer`'s), and what the methods of the
//! session interface need of their caller.

use std::path::Path;

pub use cupertino_peer::{identify, Caller, Denial, Identified, Sandbox};

/// What a method needs of its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The session agent the server supervises (`cupertino-login`).
    SessionAgent,
    /// The settings application, or the session agent.
    SettingsClient,
}

/// Whether `caller` may use a method that needs `need`. `agents` are the processes the server supervises with the
/// session agent's role, `own_pid` is the server itself, and `settings_client` says whether a program is the settings
/// application.
pub fn authorize(
    caller: &Caller,
    need: Need,
    agents: &[u32],
    own_pid: u32,
    settings_client: impl Fn(&Path) -> bool,
) -> Result<(), Denial> {
    if let Caller::Unclassified(process) = caller {
        // `/proc` refused the sandbox check: only the agents the server itself supervises are known to be unsandboxed
        return if agents.contains(&process.pid) && !process.has_exited() {
            Ok(())
        } else {
            Err(Denial::Untrusted)
        };
    }
    let caller = caller.unsandboxed()?;
    let pid = caller.process.pid;
    if pid == own_pid || agents.contains(&pid) {
        return Ok(());
    }
    match need {
        Need::SettingsClient if caller.exe.as_deref().is_some_and(settings_client) => Ok(()),
        Need::SessionAgent | Need::SettingsClient => Err(Denial::Untrusted),
    }
}

/// Whether `caller`, whose connection the bus daemon gave the user id `caller_uid`, may manage the session's shell
/// processes (list them, restart one): an unsandboxed process of the user the server runs as, `own_uid`. Root and
/// other users are refused, and so are a Flatpak, a Snap and a process whose sandbox cannot be checked.
pub fn authorize_session_user(
    caller: &Caller,
    caller_uid: Option<u32>,
    own_uid: u32,
) -> Result<(), Denial> {
    caller.unsandboxed()?;
    if caller_uid == Some(own_uid) {
        Ok(())
    } else {
        Err(Denial::Untrusted)
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;

    use cupertino_peer::ProcessHandle;

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

    fn process(sandbox: Option<Sandbox>, exe: &str) -> Caller {
        Caller::Process(Identified {
            process: ProcessHandle::from_pidfd(own_pidfd()).unwrap(),
            sandbox,
            exe: Some(exe.into()),
        })
    }

    #[test]
    fn only_the_server_the_agent_and_for_settings_the_settings_program_are_trusted() {
        let me = std::process::id();
        let settings = |exe: &Path| exe == Path::new("/usr/bin/cupertino-settings");
        let host = process(None, "/usr/bin/cupertino-files");
        assert_eq!(
            authorize(&host, Need::SessionAgent, &[], 1, settings),
            Err(Denial::Untrusted)
        );
        assert_eq!(
            authorize(&host, Need::SettingsClient, &[], 1, settings),
            Err(Denial::Untrusted)
        );
        assert_eq!(
            authorize(&host, Need::SessionAgent, &[me], 1, settings),
            Ok(())
        );
        assert_eq!(
            authorize(&host, Need::SessionAgent, &[], me, settings),
            Ok(())
        );

        let settings_app = process(None, "/usr/bin/cupertino-settings");
        assert_eq!(
            authorize(&settings_app, Need::SettingsClient, &[], 1, settings),
            Ok(())
        );
        assert_eq!(
            authorize(&settings_app, Need::SessionAgent, &[], 1, settings),
            Err(Denial::Untrusted),
            "the settings program does not quit applications"
        );
    }

    #[test]
    fn a_process_whose_sandbox_cannot_be_checked_is_trusted_only_as_a_supervised_agent() {
        let me = std::process::id();
        let unclassified = || Caller::Unclassified(ProcessHandle::from_pidfd(own_pidfd()).unwrap());
        for need in [Need::SessionAgent, Need::SettingsClient] {
            assert_eq!(authorize(&unclassified(), need, &[me], 1, |_| true), Ok(()));
            assert_eq!(
                authorize(&unclassified(), need, &[], 1, |_| true),
                Err(Denial::Untrusted),
                "not a supervised agent"
            );
            assert_eq!(
                authorize(&unclassified(), need, &[], me, |_| true),
                Err(Denial::Untrusted),
                "being the server's own pid does not make a non-dumpable process one"
            );
        }
    }

    #[test]
    fn a_really_non_dumpable_child_is_allowed_only_while_it_is_a_supervised_agent() {
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
        let pid = child as u32;
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let caller = loop {
            let credentials = zbus::fdo::ConnectionCredentials::default().set_process_id(pid);
            let caller = Caller::from_credentials(&credentials, Path::new("/proc"));
            if !matches!(caller, Caller::Process(_)) || std::time::Instant::now() > end {
                break caller;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let allowed = authorize(&caller, Need::SessionAgent, &[pid], 1, |_| false);
        let refused = authorize(&caller, Need::SessionAgent, &[], 1, |_| false);
        // Safety: kills and reaps the child forked above.
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        assert!(matches!(caller, Caller::Unclassified(_)), "{caller:?}");
        assert_eq!(allowed, Ok(()));
        assert_eq!(refused, Err(Denial::Untrusted));
    }

    #[test]
    fn only_an_unsandboxed_process_of_the_sessions_own_user_manages_the_shell_processes() {
        let me = 1000;
        let host = process(None, "/usr/bin/cupertino");
        assert_eq!(authorize_session_user(&host, Some(me), me), Ok(()));
        assert_eq!(
            authorize_session_user(&host, Some(0), me),
            Err(Denial::Untrusted),
            "root on the session's bus"
        );
        assert_eq!(
            authorize_session_user(&host, Some(me + 1), me),
            Err(Denial::Untrusted),
            "another user"
        );
        assert_eq!(
            authorize_session_user(&host, None, me),
            Err(Denial::Untrusted),
            "no user id from the bus"
        );
        for sandbox in [Sandbox::Flatpak, Sandbox::Snap] {
            assert_eq!(
                authorize_session_user(&process(Some(sandbox), "/usr/bin/cupertino"), Some(me), me),
                Err(Denial::Sandboxed(sandbox))
            );
        }
        let unclassified = Caller::Unclassified(ProcessHandle::from_pidfd(own_pidfd()).unwrap());
        assert_eq!(
            authorize_session_user(&unclassified, Some(me), me),
            Err(Denial::Untrusted)
        );
        assert_eq!(
            authorize_session_user(&Caller::Anonymous, Some(me), me),
            Err(Denial::Anonymous)
        );
    }

    #[test]
    fn a_sandboxed_caller_is_refused_even_as_the_settings_program_or_the_agent() {
        let me = std::process::id();
        for sandbox in [Sandbox::Flatpak, Sandbox::Snap] {
            let caller = process(Some(sandbox), "/usr/bin/cupertino-settings");
            for need in [Need::SessionAgent, Need::SettingsClient] {
                assert_eq!(
                    authorize(&caller, need, &[me], me, |_| true),
                    Err(Denial::Sandboxed(sandbox))
                );
            }
        }
    }
}

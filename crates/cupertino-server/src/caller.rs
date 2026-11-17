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

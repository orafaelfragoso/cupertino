//! Who is on the other end of a session bus call (the identification is `cupertino-peer`'s), and what the methods of the
//! session interface need of their caller.

use std::path::Path;

pub use cupertino_peer::{identify, Caller, Denial, Identified, Sandbox};

/// Who calls a method of the session interface: the process the bus daemon names, and the unique name of the connection
/// the call came on, which is the session agent's when the server accepted it for the `agent` component (D20).
#[derive(Debug)]
pub struct Asker {
    pub caller: Caller,
    pub sender: Option<String>,
}

/// What a method needs of its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The session agent: the connection of the `agent` component, whose unique name the server accepted from its host
    /// (`org.cupertino.Session1.ComponentOf`, D20).
    SessionAgent,
    /// The settings application, or the session agent.
    SettingsClient,
}

/// Whether `caller` may use a method that needs `need`. `agent` says the call came on the session agent's connection
/// (the `agent` component's accepted bus name, so by its identity, never by its process: the host that runs it runs
/// other components too), `own_pid` is the server itself, and `settings_client` says whether a program is the settings
/// application.
pub fn authorize(
    caller: &Caller,
    need: Need,
    agent: bool,
    own_pid: u32,
    settings_client: impl Fn(&Path) -> bool,
) -> Result<(), Denial> {
    if agent {
        return Ok(());
    }
    if let Caller::Unclassified(_) = caller {
        // `/proc` refused the sandbox check: only the session agent, known by its connection, is trusted then
        return Err(Denial::Untrusted);
    }
    let caller = caller.unsandboxed()?;
    if caller.process.pid == own_pid {
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

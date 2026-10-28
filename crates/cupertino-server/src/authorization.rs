//! What the server lets a connection do. A connection holds capabilities only when the server made it for a process it
//! starts ([`crate::shell_connection`]); a client of the public socket holds none. The capabilities live in the server's
//! data of the connection, so they cannot be claimed, passed on or copied: a process that is not on the other end of
//! the connection has no way to use them.
//!
//! Each privileged global is offered only to the connections that hold its role (`can_view`), and each privileged
//! request checks the role again in its handler, so a request on an object id the client guessed is a protocol error
//! of its own.

use cupertino_defaults::compositor::ShellRoleName;
use cupertino_protocols::server::cupertino_shell_v1::Error as ShellError;
use smithay::reexports::wayland_server::{Client, Resource};

use crate::force_quit::ProcessHandle;
use crate::state::{ClientState, Cupertino};

/// A privilege of a shell process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellRole {
    /// Bind `cupertino_shell_v1` and follow the appearance, display options and lock state of the session.
    Session,
    /// Be the menu bar of an output.
    MenuBar,
    /// Be the dock of an output, which can end applications.
    Dock,
    /// Arrange the windows of an output.
    WindowManager,
    /// Take the session lock: the lock screen.
    SessionLock,
    /// See the capture globals and the window feed of the screenshot process.
    Capture,
}

impl ShellRole {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

impl From<ShellRoleName> for ShellRole {
    fn from(name: ShellRoleName) -> Self {
        match name {
            ShellRoleName::Session => ShellRole::Session,
            ShellRoleName::MenuBar => ShellRole::MenuBar,
            ShellRoleName::Dock => ShellRole::Dock,
            ShellRoleName::WindowManager => ShellRole::WindowManager,
            ShellRoleName::SessionLock => ShellRole::SessionLock,
        }
    }
}

/// The roles one connection holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientCapabilitySet(u8);

impl ClientCapabilitySet {
    /// The set of a client of the public socket.
    pub const NONE: Self = Self(0);

    pub fn of(roles: impl IntoIterator<Item = ShellRole>) -> Self {
        Self(roles.into_iter().fold(0, |bits, r| bits | r.bit()))
    }

    pub fn contains(self, role: ShellRole) -> bool {
        self.0 & role.bit() != 0
    }

    /// Whether the set holds a role of the shell protocol, which is what makes `cupertino_shell_v1` visible.
    pub fn holds_shell_role(self) -> bool {
        [
            ShellRole::Session,
            ShellRole::MenuBar,
            ShellRole::Dock,
            ShellRole::WindowManager,
        ]
        .into_iter()
        .any(|r| self.contains(r))
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What a connection's capabilities depend on.
#[derive(Debug)]
pub enum Holder {
    /// The process the server started for the connection: the capabilities end when it exits.
    Process(ProcessHandle),
    /// A client in the server's own process (tests, the accent bridge): nothing ends the capabilities.
    Detached,
}

impl Holder {
    fn has_exited(&self) -> bool {
        match self {
            Holder::Process(process) => process.has_exited(),
            Holder::Detached => false,
        }
    }
}

impl ClientState {
    /// The capabilities in force: none until a holder is bound, and none once the process the connection was made for
    /// has exited.
    pub fn granted(&self) -> ClientCapabilitySet {
        match self.holder.get() {
            Some(holder) if !holder.has_exited() => self.capabilities,
            _ => ClientCapabilitySet::NONE,
        }
    }

    /// A state for a client that runs inside the server's process: `capabilities` hold for the life of the connection.
    /// Its holder is set here, so it is never bound to a process ([`crate::shell_connection::bind_holder`] fails on it),
    /// and its process is the server's own ([`ClientState::process`]).
    pub fn detached(capabilities: ClientCapabilitySet) -> Self {
        let state = ClientState {
            capabilities,
            ..ClientState::default()
        };
        let _ = state.holder.set(Holder::Detached);
        state
    }
}

/// Whether `client` is not in a sandbox: it did not connect through a security context. The globals that read or change
/// what other clients own (the clipboard managers, display configuration and power, and the security context global
/// itself, which a sandboxed client must not use to make a context of its own) are offered to these only.
pub fn unsandboxed(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|data| !data.sandboxed)
}

/// The capabilities of `client`; none when its data is not a [`ClientState`] or its holder has exited.
pub fn capabilities(client: &Client) -> ClientCapabilitySet {
    client
        .get_data::<ClientState>()
        .map_or(ClientCapabilitySet::NONE, ClientState::granted)
}

impl Cupertino {
    /// End the dock, menu bar and window manager roles whose connection no longer holds them. A restarted process asks
    /// for its role before the server has seen the old connection close; this is what frees it.
    pub fn release_revoked_roles(&mut self) {
        self.release_revoked_docks();
        self.release_revoked_menu_bars();
        self.release_revoked_window_managers();
    }
}

/// Whether `client` holds `role`.
pub fn authorized(client: &Client, role: ShellRole) -> bool {
    capabilities(client).contains(role)
}

/// Request-time check of a role object: the connection must still hold `role`. When it does not, the object's client
/// gets the shell protocol's permission error and the request is not to be carried out.
pub fn require<R: Resource>(client: &Client, role: ShellRole, object: &R) -> bool {
    if authorized(client, role) {
        return true;
    }
    refuse(object, role);
    false
}

/// Refuse a request that needs `role`: one warning with the client, the interface and the role (never a credential),
/// and the shell protocol's permission error on `object`.
pub fn refuse<R: Resource>(object: &R, role: ShellRole) {
    let interface = object.id().interface().name;
    tracing::warn!(
        client = ?object.client().map(|c| c.id()),
        interface,
        ?role,
        "a request needs a role the connection does not hold"
    );
    object.post_error(
        ShellError::Permission,
        format!("{interface} needs the {role:?} role, which this connection does not hold"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_holds_exactly_its_roles() {
        let set = ClientCapabilitySet::of([ShellRole::Dock, ShellRole::Capture]);
        assert!(set.contains(ShellRole::Dock) && set.contains(ShellRole::Capture));
        assert!(!set.contains(ShellRole::WindowManager) && !set.contains(ShellRole::Session));
        assert!(!ClientCapabilitySet::NONE.holds_shell_role());
        assert!(ClientCapabilitySet::NONE.is_empty());
    }

    #[test]
    fn capture_alone_does_not_make_the_shell_global_visible() {
        assert!(!ClientCapabilitySet::of([ShellRole::Capture]).holds_shell_role());
        assert!(ClientCapabilitySet::of([ShellRole::MenuBar]).holds_shell_role());
    }
}

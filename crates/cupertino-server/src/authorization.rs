//! What the server lets a connection do. A connection holds capabilities only when the server made it for a process it
//! starts ([`crate::shell_connection`]); a client of the public socket holds none. The capabilities live in the server's
//! data of the connection, so they cannot be claimed, passed on or copied: a process that is not on the other end of
//! the connection has no way to use them.
//!
//! Each privileged global is offered only to the connections that hold its role (`can_view`), and each privileged
//! request checks the role again in its handler, so a request on an object id the client guessed is a protocol error
//! of its own. A refused request is also the diagnostic event `server.authorization.refused`.

use std::time::{Duration, Instant};

use cupertino_defaults::compositor::ShellRoleName;
use cupertino_diagnostic_events::{self as diag, Severity, Value};
use cupertino_protocols::server::cupertino_shell_v1::Error as ShellError;
use smithay::desktop::{LayerMap, LayerSurface};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::shell::wlr_layer::Layer;

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
    /// Draw the authentication prompt: its overlay surfaces are hidden from capture, and while one is mapped no
    /// application holds the pointer or the shortcuts.
    Prompt,
    /// See and bind the data control globals ([`crate::data_control`]).
    DataControl,
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
            ShellRoleName::Clipboard => ShellRole::DataControl,
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
    /// A client in the server's own process (tests): the capabilities end at a deadline, so a connection nothing
    /// closes does not hold them for good.
    Detached(Instant),
}

/// How long a connection made for a client in the server's own process holds its capabilities.
pub const DETACHED_LIFETIME: Duration = Duration::from_secs(3600);

impl Holder {
    fn has_exited(&self) -> bool {
        match self {
            Holder::Process(process) => process.has_exited(),
            Holder::Detached(until) => Instant::now() >= *until,
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

    /// A state for a client that runs inside the server's process: `capabilities` hold for [`DETACHED_LIFETIME`]. Its
    /// holder is set here, so it is never bound to a process ([`crate::shell_connection::bind_holder`] fails on it),
    /// and its process is the server's own ([`ClientState::process`]).
    pub fn detached(capabilities: ClientCapabilitySet) -> Self {
        Self::detached_for(capabilities, DETACHED_LIFETIME)
    }

    /// [`ClientState::detached`] for `lifetime`.
    pub fn detached_for(capabilities: ClientCapabilitySet, lifetime: Duration) -> Self {
        let state = ClientState {
            capabilities,
            ..ClientState::default()
        };
        let _ = state
            .holder
            .set(Holder::Detached(Instant::now() + lifetime));
        state
    }
}

/// Whether `client` is not in a sandbox: it did not connect through a security context. The globals that read or change
/// what other clients own (display configuration and power, and the security context global itself, which a sandboxed
/// client must not use to make a context of its own) are offered to these only; data control asks for more
/// ([`crate::data_control`]).
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

/// The roles as types, so a function that needs one asks for the [`Permit`] of exactly that role.
pub mod role {
    use super::ShellRole;

    /// A role of the shell protocol, named by a type.
    pub trait Role {
        const ROLE: ShellRole;
    }

    macro_rules! roles {
        ($($name:ident),+ $(,)?) => {$(
            #[derive(Clone, Copy, Debug)]
            pub struct $name;
            impl Role for $name {
                const ROLE: ShellRole = ShellRole::$name;
            }
        )+};
    }

    roles!(Session, MenuBar, Dock, WindowManager);
}

/// The proof that a connection held role `R` when its request came in. Only [`permit`] makes one, so a function that
/// takes a `Permit<role::Dock>` cannot be handed the outcome of a check for another role, and a boolean cannot stand
/// in for it.
#[derive(Debug)]
pub struct Permit<R: role::Role>(std::marker::PhantomData<R>);

/// The permit of `client` for role `R`; `None` when its connection does not hold the role.
pub fn permit<R: role::Role>(client: &Client) -> Option<Permit<R>> {
    authorized(client, R::ROLE).then_some(Permit(std::marker::PhantomData))
}

/// Whether `client` holds `role`.
pub fn authorized(client: &Client, role: ShellRole) -> bool {
    capabilities(client).contains(role)
}

/// Whether `surface` is drawn by the authentication prompt: its client holds [`ShellRole::Prompt`].
pub fn is_prompt(surface: &WlSurface) -> bool {
    surface
        .client()
        .is_some_and(|client| authorized(&client, ShellRole::Prompt))
}

/// The surfaces of `layer` from bottom to top, with the authentication prompt's overlay surfaces above every other one, so
/// the prompt is drawn and hit first whatever was mapped after it.
pub fn layers_stacked(
    map: &LayerMap,
    layer: Layer,
) -> impl DoubleEndedIterator<Item = &LayerSurface> {
    let raised = move |l: &&LayerSurface| layer == Layer::Overlay && is_prompt(l.wl_surface());
    map.layers_on(layer)
        .filter(move |l| !raised(l))
        .chain(map.layers_on(layer).filter(raised))
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
/// the event `server.authorization.refused` with the interface, the client's program as its `app_id` and the role, and
/// the shell protocol's permission error on `object`. The event's rate limit bounds a client that keeps trying.
pub fn refuse<R: Resource>(object: &R, role: ShellRole) {
    let interface = object.id().interface().name;
    tracing::warn!(
        client = ?object.client().map(|c| c.id()),
        interface,
        ?role,
        "a request needs a role the connection does not hold"
    );
    let program = object.client().as_ref().and_then(program_of);
    diag::event(
        Severity::Warn,
        "server.authorization.refused",
        "a request needs a role the connection does not hold",
    )
    .field("interface", Value::Public(interface))
    .field(
        "app_id",
        Value::Public(program.as_deref().unwrap_or("unknown")),
    )
    .field("role_required", Value::Public(&format!("{role:?}")))
    .emit();
    object.post_error(
        ShellError::Permission,
        format!("{interface} needs the {role:?} role, which this connection does not hold"),
    );
}

/// The name of the program behind `client` (`/proc/<pid>/exe`), which names it in a diagnostic event; `None` when its
/// process is not known or has gone.
fn program_of(client: &Client) -> Option<String> {
    let process = client.get_data::<ClientState>()?.process()?;
    let exe = std::fs::read_link(format!("/proc/{}/exe", process.pid)).ok()?;
    // the answer names the process only while it is the one the descriptor holds
    if process.has_exited() {
        return None;
    }
    Some(exe.file_name()?.to_string_lossy().into_owned())
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

    #[test]
    fn the_prompt_role_does_not_make_the_shell_global_visible() {
        let prompt = ClientCapabilitySet::of([ShellRole::Prompt]);
        assert!(!prompt.holds_shell_role());
        assert!(prompt.contains(ShellRole::Prompt) && !prompt.contains(ShellRole::Capture));
    }

    #[test]
    fn the_capabilities_of_a_connection_in_the_servers_process_end_at_its_deadline() {
        let roles = ClientCapabilitySet::of([ShellRole::Dock]);
        let live = ClientState::detached_for(roles, Duration::from_secs(60));
        assert_eq!(live.granted(), roles);
        let expired = ClientState::detached_for(roles, Duration::ZERO);
        assert_eq!(expired.granted(), ClientCapabilitySet::NONE);
        // the connection the tests use is good for as long as a test lasts
        assert_eq!(ClientState::detached(roles).granted(), roles);
    }
}

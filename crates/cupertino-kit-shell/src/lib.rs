//! The shell's extension to CupertinoKit (`docs/architecture/shell-host.md#cupertino-kit-shell`).
//!
//! Shell components and privileged shell programs reach the server's shell protocols, transactions,
//! holes, host bus, and hosting runtime exclusively through this crate and `cupertino-kit`, without
//! naming Wayland objects, transport types, raw surfaces, or file descriptors.

pub mod bus;
pub mod capture;
mod component;
pub mod consent;
mod context;
#[cfg(debug_assertions)]
mod crash;
pub mod design;
pub mod dock;
pub mod host;
pub mod hosting;
mod link;
pub mod lock;
pub mod menu_bar;
mod oneshot;
pub mod outputs;
pub mod overview;
mod runtime;
pub mod session;
pub mod transaction;
pub mod window_manager;

use std::fmt;

use cupertino_kit::core::{Application, Environment, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::SystemAppearance;
use serde::{Deserialize, Serialize};

pub use component::ComponentName;
use outputs::OutputId;

/// Clocks for what the shell draws. Never use a display clock for authentication, waits or IPC deadlines.
pub mod test_clock {
    pub use cupertino_shell_client::test_clock::{
        display_now, display_unix_seconds, motion_settled,
    };
}

/// Errors returned by `cupertino-kit-shell` startup and hosting operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server offered a different protocol version than this build expects.
    Version { expected: u32, found: u32 },
    /// Protocol, socket, or startup failure.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version { expected, found } => {
                write!(
                    f,
                    "protocol version mismatch: expected {expected}, found {found}"
                )
            }
            Self::Protocol(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

/// A window of the session by the server's id (stable for the window's lifetime; distinct from Kit's [`WindowKey`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerWindowId(pub u32);

/// A workspace of an output by the server's id (stable for the workspace's lifetime).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub u32);

/// A Kit application that shows its windows through the shell's roles.
pub trait Program: Application {
    /// How each window of `view` reaches the screen; asked for every window after each `view`.
    fn surface(model: &Self::Model, window: &WindowKey) -> Surface;
    /// Convert a shell event into an application message, or `None` to ignore it.
    fn event(event: Event) -> Option<Self::Message>;
    /// Whether the view shows what the program has to show. Until it does, no view counts as the first, so `ready`
    /// waits and the server keeps holding the surfaces of the instance before (`holds`). A program that shows nothing
    /// until it has read what it shows says so until then; every view counts by default.
    fn showing(model: &Self::Model) -> bool {
        let _ = model;
        true
    }
}

/// A part of the shell that runs inside a host process (`cupertino-shell`).
pub trait Component: Program {
    const NAME: ComponentName;
}

/// Type-erased runner for a registered host [`Component`].
type ComponentRunner = fn(&runtime::Hosting, hosting::Connection) -> Result<runtime::Ran, Error>;

/// A component as a host lists and starts it (`hosting::Runtime::start`).
pub struct Entry {
    name: ComponentName,
    pub(crate) runner: ComponentRunner,
}

impl Entry {
    /// Create a host entry for component type `C`: the host runs it on Kit's UI loop, on the component's own
    /// connection.
    pub const fn of<C: Component>() -> Entry {
        Entry {
            name: C::NAME,
            runner: runtime::run_component::<C>,
        }
    }

    /// Component name registered in this entry.
    pub fn name(&self) -> ComponentName {
        self.name
    }
}

/// How one window of a [`Program`] maps onto shell roles, holds, and holes.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub role: Role,
    /// Persistent surface hold key; `None` marks a transient surface dismissed on hang or drop.
    pub persistent: Option<&'static str>,
    /// Layers of this window that act as window/thumbnail/workspace holes.
    pub holes: Vec<Hole>,
}

/// Shell role of one window.
#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    /// Kit's own toplevel or popup window.
    Window,
    /// A `zwlr_layer_surface_v1` layer surface.
    Layer(LayerSpec),
    /// The menu bar of the spec's output (`menu_bar` role).
    MenuBar(LayerSpec),
    /// The dock of the spec's output (`dock` role).
    Dock(LayerSpec),
    /// A lock surface of an output shown while the session lock is held (`session_lock` role).
    Lock(OutputId),
}

/// Placement and input properties of a layer surface.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerSpec {
    pub namespace: &'static str,
    /// `None` leaves output selection to the server (active output).
    pub output: Option<OutputId>,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Logical points; an extent of 0 stretches between anchored opposite edges.
    pub size: Size,
    pub margin: Margin,
    pub exclusive_zone: i32,
    pub keyboard: Keyboard,
}

/// Layer-shell stacking plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

/// Edges a layer surface anchors to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

/// Layer surface margins in logical points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Margin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// Keyboard focus policy for a layer surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keyboard {
    #[default]
    None,
    OnDemand,
    Exclusive,
}

/// Content source bound to a surface layer hole (`cupertino_hole_v1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleSource {
    Window(ServerWindowId),
    Thumbnail(ServerWindowId),
    Workspace(WorkspaceId),
}

/// One hole attached to a window's surface layer (`cupertino_kit::core::SurfaceLayerMark`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hole {
    /// Key of the window's surface layer that carries the hole.
    pub layer: u64,
    pub source: HoleSource,
    /// Whether the hole claims the window off its own frame while shown.
    pub claim: bool,
}

/// Events delivered from the shell protocols and host bus to [`Program::event`].
#[derive(Debug)]
pub enum Event {
    Session(session::Change),
    /// The session's appearance and display options (Reduce Motion, Reduce Transparency, Increase Contrast, the
    /// steady caret), the ones a Kit application's theme follows: once at start and on every change.
    Appearance(SystemAppearance),
    /// An output appeared, disappeared, or changed: read [`outputs::all`].
    Outputs,
    /// The server closed or dismissed `WindowKey`.
    Closed(WindowKey),
    /// The server configured the layer or lock surface of `window` to `size`, in points: the size the window is laid
    /// out at, the area its anchors and margins leave it within the space other surfaces reserve. With every configure,
    /// before the frame drawn at that size.
    Configured {
        window: WindowKey,
        size: Size,
    },
    /// The window or workspace of the hole on `layer` of `window` closed.
    HoleClosed {
        window: WindowKey,
        layer: u64,
    },
    MenuBar(WindowKey, Box<menu_bar::Update>),
    Dock(WindowKey, dock::Update),
    WindowManager(OutputId, window_manager::Update),
    Overview(OutputId, overview::Update),
    Transaction(transaction::Event),
    Bus {
        from: bus::Sender,
        message: bus::Message,
    },
    /// Whether the server accepted this component's announced session-bus connection (D20).
    BusIdentity {
        accepted: bool,
    },
    Lock(lock::Event),
    Consent(consent::Event),
    Capture(capture::Event),
    /// Caps Lock is on (`true`) or off, as the keyboard's modifiers say: once at start and on every change. The lock
    /// and login screens show it in their password field.
    CapsLock(bool),
}

/// Run a standalone privileged shell program (`cupertino-login`, `cupertino-prompts`, `cupertino-screenshot`) on
/// Kit's UI loop, on the connection the server gave the process, until it exits.
pub fn run<P: Program>(environment: Environment) -> Result<(), Error> {
    run_as::<P>(environment, false)
}

/// [`run`] for the lock screen, whose accessibility tree is never muted while locked.
pub fn run_lock_screen<P: Program>(environment: Environment) -> Result<(), Error> {
    run_as::<P>(environment, true)
}

fn run_as<P: Program>(environment: Environment, lock_screen: bool) -> Result<(), Error> {
    let ctx = context::ComponentContext {
        environment: Some(environment),
        ..context::ComponentContext::default()
    };
    context::install(ctx);
    let ran = runtime::run_program::<P>(lock_screen);
    drop(runtime::take_state());
    drop(context::take());
    ran
}

/// The theme, text system, and icons of the calling thread's component.
pub fn environment() -> Environment {
    context::with_ctx(|ctx| ctx.environment.clone())
        .flatten()
        .unwrap_or_else(|| {
            cupertino_kit::app::environment().expect("toolkit environment should initialize")
        })
}

/// Keys of the persistent surfaces the server currently holds from the prior instance; empty on first start.
pub fn holds() -> Vec<String> {
    context::with_ctx(|ctx| ctx.holds.clone()).unwrap_or_default()
}

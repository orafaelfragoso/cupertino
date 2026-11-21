//! Shell host protocol (`cupertino_host_v1`, `cupertino_host_component_v1`, `cupertino_component_v1`),
//! per-component connections and roles, host bus bridges, D20 bus identity nonce verification,
//! D16/D17 liveness and failure restart tracking, and D19 persistent surface holds.

use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cupertino_peer::{Caller, Denial, ProcessHandle};
use cupertino_protocols::server::host::cupertino_component_v1::{
    self, BusNameResult, CupertinoComponentV1, Error as ComponentError,
};
use cupertino_protocols::server::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, Error as HostComponentError, StopReason, UnavailableReason,
};
use cupertino_protocols::server::host::cupertino_host_v1::{
    self, CupertinoHostV1, Error as HostError, Kind as WireHostKind,
};
use cupertino_wm::{OutputId, Rect};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::{Logical, Rectangle, Transform};
use smithay::wayland::compositor::{get_role, with_states, SurfaceAttributes};
use smithay::wayland::shell::wlr_layer::{
    Anchor, ExclusiveZone, Layer, LayerSurfaceCachedState, Margins, LAYER_SURFACE_ROLE,
};
use smithay::wayland::shm::with_buffer_contents;

use crate::authorization::{authorized, ClientCapabilitySet, Holder, ShellRole};
use crate::menu_bar::is_unique_bus_name;
use crate::state::{ClientState, Cupertino};

/// Protocol version offered for `cupertino_host_v1` and `cupertino_component_v1`.
pub const HOST_PROTOCOL_VERSION: u32 = 1;
/// Initial restart backoff after a component's first failure (D16).
pub const FAILURE_BACKOFF_BASE: Duration = Duration::from_millis(250);
/// Maximum restart backoff between consecutive component failures (D16).
pub const FAILURE_BACKOFF_CAP: Duration = Duration::from_secs(8);
/// Stable run time after which a component's failure count and backoff reset (D16).
pub const FAILURE_RESET_AFTER: Duration = Duration::from_secs(60);
/// Failures within [`GIVE_UP_WINDOW`] that stop a component until explicit restart (D16).
pub const GIVE_UP_FAILURES: usize = 5;
/// Rolling window in which [`GIVE_UP_FAILURES`] stop a component (D16).
pub const GIVE_UP_WINDOW: Duration = Duration::from_secs(60);
/// Shared-host crashes within [`SHARED_HOST_CRASH_WINDOW`] that isolate all components into separate hosts (D18).
pub const SHARED_HOST_CRASH_ISOLATE_COUNT: usize = 3;
/// Rolling window for counting shared-host crashes (D18).
pub const SHARED_HOST_CRASH_WINDOW: Duration = Duration::from_secs(60);
/// Maximum age of a bus-identity announcement nonce (D20).
pub const BUS_NONCE_TTL: Duration = Duration::from_secs(5);
/// Grace period after handing out a component connection before un-taken-over holds expire without `ready` (D19).
pub const HOLD_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Required seat idle duration before answering a shared host's `request_restart` (D17).
pub const RESTART_QUIET_IDLE: Duration = Duration::from_secs(2);
/// Latest time after `request_restart` by which the server answers `restart` regardless of activity (D17).
pub const RESTART_QUIET_MAX: Duration = Duration::from_secs(60);
/// Grace before killing a host expected to exit after `restart`, `lost` (separate), or empty list (separate).
pub const EXPECTED_EXIT_KILL_GRACE: Duration = Duration::from_secs(2);
/// Minimum interval between creating fresh host-bus bridge socket pairs after `bus_dropped`.
pub const BRIDGE_THROTTLE: Duration = Duration::from_secs(1);
/// Maximum length in bytes of a `cupertino_component_v1.persist` surface key.
pub const MAX_PERSIST_KEY_BYTES: usize = 64;
/// The exit status of a host that found another version of `cupertino_host_v1` than it was built with
/// (`docs/architecture/shell-host.md`, *Versions*).
pub const HOST_VERSION_EXIT_STATUS: i32 = 76;

/// Static definition of one component in the server's component table.
#[derive(Clone, Copy, Debug)]
pub struct ComponentSpec {
    pub name: &'static str,
    pub roles: &'static [ShellRole],
}

/// The server's fixed 11-component table in protocol list order (`docs/architecture/shell-host.md#components`).
pub const COMPONENTS: [ComponentSpec; 11] = [
    ComponentSpec {
        name: "menubar",
        roles: &[ShellRole::Session, ShellRole::MenuBar],
    },
    ComponentSpec {
        name: "dock",
        roles: &[ShellRole::Session, ShellRole::Dock, ShellRole::Windows],
    },
    ComponentSpec {
        name: "desktop",
        roles: &[ShellRole::Session],
    },
    ComponentSpec {
        name: "windowmanager",
        roles: &[
            ShellRole::Session,
            ShellRole::WindowManager,
            ShellRole::Windows,
        ],
    },
    ComponentSpec {
        name: "overview",
        roles: &[ShellRole::Session, ShellRole::Overview, ShellRole::Windows],
    },
    ComponentSpec {
        name: "launcher",
        roles: &[ShellRole::Session],
    },
    ComponentSpec {
        name: "notifications",
        roles: &[ShellRole::Session],
    },
    ComponentSpec {
        name: "controlcenter",
        roles: &[ShellRole::Session],
    },
    ComponentSpec {
        name: "osd",
        roles: &[ShellRole::Session],
    },
    ComponentSpec {
        name: "agent",
        roles: &[ShellRole::Session, ShellRole::SessionAgent],
    },
    ComponentSpec {
        name: "dialogs",
        roles: &[ShellRole::Session],
    },
];

/// Look up a component in [`COMPONENTS`] by its table name.
pub fn component_spec(name: &str) -> Option<&'static ComponentSpec> {
    COMPONENTS.iter().find(|spec| spec.name == name)
}

/// Whether a host connection is the shared host or a separate single-component host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Shared,
    Separate(&'static str),
}

/// Liveness of a running component as reported by its host (D17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Liveness {
    #[default]
    Responsive,
    Busy,
    Unresponsive,
    Lost,
}

/// Identity attached to a component's Wayland [`ClientState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentConnection {
    pub name: &'static str,
    pub instance: u32,
    pub host_id: u64,
}

/// User data on a `cupertino_host_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct HostGlobalData {
    host_id: u64,
}

/// User data on a `cupertino_host_component_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct HostComponentData {
    host_id: u64,
    component: &'static str,
}

/// User data on a `cupertino_component_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct ComponentGlobalData {
    component: &'static str,
    instance: u32,
}

/// Pending D20 bus-name announcement awaiting `Session1.AnnounceComponent(nonce)`.
#[derive(Clone, Debug)]
struct PendingBusNonce {
    name: String,
    nonce: String,
    issued_at: Instant,
    resource: CupertinoComponentV1,
}

/// A persisted layer surface registered via `cupertino_component_v1.persist` before its first buffer commit.
#[derive(Clone)]
struct PersistedSurface {
    component: &'static str,
    client_id: ClientId,
    surface_id: ObjectId,
    key: String,
    output: OutputId,
    /// True once the surface has committed its first buffer (`error::mapped` if persisted again).
    mapped: bool,
    /// The picture taken as its `wl_surface` went, when that came before the end of its layer role.
    last_picture: Option<HeldPicture>,
    /// Where the surface lay on its output as its `wl_surface` went: by the end of its layer role the surface has no
    /// buffer, and the layer map no size for it.
    last_geometry: Option<Rectangle<i32, Logical>>,
    /// The surface was its output's menu bar as of its last commit. A failing component can end the role before its
    /// connection ends (its teardown destroys the role object first), and what is held then is still the menu bar.
    menu_bar: bool,
}

/// The last picture of a held persistent surface (D19), taken once as its hold starts, with what the server drew for
/// the surface besides its pixels, so the held surface shows as the live one did.
#[derive(Clone)]
pub struct HeldPicture {
    /// The surface's pixels and its layers' content.
    pub content: HeldContent,
    /// The backdrops the surface, its subsurfaces and its layers drew, which sample what lies behind them: drawn live
    /// where the live surface drew them, among the slices of the picture and behind it.
    pub backdrops: crate::render::scene::HeldBackdrops,
    /// The surface was its output's menu bar: it is hidden, revealed and dimmed as the output's menu bar is, and it does
    /// not give way to a full-screen window.
    pub menu_bar: bool,
}

/// The pixels of a held persistent surface (D19).
#[derive(Clone)]
pub enum HeldContent {
    /// The surface tree and its layers' content drawn by the renderer into textures it keeps, whatever kind of buffer
    /// the surface showed (shm, dmabuf): cut into slices, front to back, at each glass its layers and subsurfaces draw
    /// among them, which the server draws live between them; `None` for a slice with nothing in it. Each texture
    /// covers only the bounds of what its slice draws, and lives as long as the hold.
    Texture {
        slices: Rc<[Option<HeldSlice>]>,
        /// Logical size the surface covers.
        size: (i32, i32),
    },
    /// The pixels of the surface's shm buffer, copied when there is no renderer to draw it with.
    Memory {
        buffer: MemoryRenderBuffer,
        /// Buffer size in pixels.
        size: (i32, i32),
    },
}

/// One slice of a held surface's picture drawn by the renderer (D19, [`HeldContent::Texture`]): its pixels (`T`, a
/// texture as captured, then the snapshot that keeps it), where they lie and the opacity they are drawn at.
#[derive(Clone)]
pub struct HeldSlice<T = crate::render::snapshots::SnapshotOwner> {
    pub pixels: T,
    /// What the pixels cover, in logical points from the surface's top-left corner, on device pixels of the output
    /// they were drawn for: the bounds of what the slice draws.
    pub rect: Rect,
    /// The opacity the pixels are drawn at, before the surface's own: below 1 for a vibrant layer's coverage, which
    /// stands for the lift it screens in over a dark face.
    pub opacity: f32,
}

/// How far a restarted window manager has come in taking over the placements held on an output (D19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WmPlacementHold {
    /// The placements of the instance before stay in force; the new instance's placement commits for the output are
    /// kept, not applied, until its takeover frame.
    Held,
    /// A surface of an instance that had committed no placement took the hold over: the held placements stay in force
    /// until its first `commit`.
    AwaitingCommit,
}

/// A persistent surface held after its component connection ended (D19).
#[derive(Clone)]
pub struct HeldSurface {
    pub component: &'static str,
    pub key: String,
    pub output: OutputId,
    pub layer: Layer,
    pub geometry: Rectangle<i32, Logical>,
    pub anchor: Anchor,
    pub exclusive_zone: ExclusiveZone,
    pub margin: Margins,
    pub picture: Option<HeldPicture>,
    pub held_since: Instant,
}

/// One connected shell host process (shared or separate).
struct HostEntry {
    id: u64,
    kind: HostKind,
    /// The host's Wayland connection.
    client: ClientId,
    /// The host process started from the development folder.
    from_dev_folder: bool,
    /// The supervisor started the host ([`Cupertino::apply_shell_plan`] keeps its list), not a caller of
    /// [`Cupertino::attach_host_connection`].
    spawned: bool,
    /// Components currently assigned to this host in table order.
    components: Vec<&'static str>,
    /// Bound `cupertino_host_v1` resource, once the host binds the global.
    resource: Option<CupertinoHostV1>,
    /// Bound `cupertino_host_component_v1` objects keyed by component name.
    component_objects: HashMap<&'static str, CupertinoHostComponentV1>,
    /// Components for which `connect` is currently waiting for an answer.
    connect_pending: HashSet<&'static str>,
    /// When the host sent `request_restart` (waiting for a quiet moment on a shared host).
    restart_requested_at: Option<Instant>,
    /// If the server expects this host to exit, the deadline after which it is killed via pidfd. It stays set once
    /// the deadline passed, so the exit is still an expected one.
    expected_exit_kill_at: Option<Instant>,
    /// The server sent the expected-exit host `SIGKILL` after its grace.
    kill_sent: bool,
    /// Process handle (for pidfd kill and `ShellHosts` reporting), if external.
    process: Option<ProcessHandle>,
    /// Cached start ticks of `process` for `ShellHosts`.
    start_ticks: Option<u64>,
    /// When the bridge pair for this separate host was last created (for 1s throttle).
    last_bridge_at: Option<Instant>,
    /// Deferred bridge creation time when `bus_dropped` arrived within the throttle window.
    bridge_retry_at: Option<Instant>,
}

/// Callback invoked once a restarted component sends `cupertino_component_v1.ready` or times out.
pub type RestartWaiter = Box<dyn FnOnce(Result<(), String>) + Send>;

/// Per-component state that outlives individual host processes (D16/D17/D19/D20).
#[derive(Default)]
struct ComponentRecord {
    /// Monotonic instance counter handed out over `cupertino_component_v1.component` (starts at 1).
    instance: u32,
    /// Active component Wayland client id and owning host id, if connected.
    active: Option<(ClientId, u64)>,
    /// When the active instance was handed out (for 60s stability reset and 10s hold timeout).
    started_at: Option<Instant>,
    /// Bound `cupertino_component_v1` resource for the active instance.
    component_resource: Option<CupertinoComponentV1>,
    /// Current liveness reported by the active host.
    liveness: Liveness,
    /// Failure timestamps (adjusted to exclude run durations < 60s of `lost` instances).
    failures: Vec<Instant>,
    /// Consecutive failure count since last 60s stable run (drives exponential backoff).
    consecutive_failures: u32,
    /// Earliest time the next `connect` may be answered.
    backoff_until: Option<Instant>,
    /// Stopped by give-up (`5` failures in `60s`) until an explicit restart.
    gave_up: bool,
    /// Once `lost`, stays in a separate host for the rest of the session or until explicit restart.
    separate_for_session: bool,
    /// Accepted session-bus unique name for D20 identity.
    accepted_bus_name: Option<String>,
    /// Pending nonce awaiting `Session1.AnnounceComponent`.
    pending_nonce: Option<PendingBusNonce>,
    /// Deadline by which un-taken-over holds expire if `ready` does not arrive.
    hold_expire_at: Option<Instant>,
    /// Whether this component has received `ready` for its current instance.
    ready: bool,
    /// Pending `cupertino shell restart` completion callbacks waiting for `ready`.
    /// Each with its deadline and the instance at the request: only the `ready` of a later instance answers it.
    restart_waiters: Vec<(Instant, u32, RestartWaiter)>,
}

/// Server-side manager for shell hosts, per-component connections, liveness, holds, and bus identity.
pub struct HostManager {
    _host_global: GlobalId,
    _component_global: GlobalId,
    next_host_id: u64,
    hosts: HashMap<u64, HostEntry>,
    host_by_client: HashMap<ClientId, u64>,
    components: HashMap<&'static str, ComponentRecord>,
    /// Persisted surfaces keyed by `wl_surface` object id.
    persisted: HashMap<ObjectId, PersistedSurface>,
    /// Currently held persistent surfaces across all components and outputs.
    holds: Vec<HeldSurface>,
    /// Outputs whose window-manager placements are currently held across a WM component restart.
    held_wm_outputs: HashMap<OutputId, WmPlacementHold>,
    /// Shared-host unexpected crash timestamps (3 within 60s isolates all components).
    shared_host_crashes: Vec<Instant>,
    /// Once 3 shared-host crashes occur within 60s, every component runs in a separate host.
    pub all_separate_for_session: bool,
    /// Last time seat input occurred (for shared-host `request_restart` quiet-moment check).
    last_input_at: Instant,
    /// Components a host was built without (`unavailable` with `absent`): they run as their process entries.
    absent: HashSet<&'static str>,
    /// Components a host of the development folder gave back to the installed host, until a restart asks for them.
    dev_given_back: HashSet<&'static str>,
    /// Whether each host connection that ended was expected to end, until the supervisor reaps its process.
    ended: HashMap<u64, bool>,
    /// The list the shared host gets, from the last plan.
    planned_shared: Vec<&'static str>,
    /// The components of the last plan that run in a separate host.
    planned_separate: Vec<&'static str>,
    /// The process entries of hosted components that were logged as left alone.
    left_alone: Vec<String>,
    /// What the hosts said changes where the components run: the plan is worked out again after the supervision pass.
    pub(crate) plan_dirty: bool,
}

/// One host connection as the server sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostView {
    pub kind: HostKind,
    /// Its list, in the table's order.
    pub components: Vec<&'static str>,
    /// Its process, once the server bound the connection to it.
    pub pid: Option<u32>,
    /// It bound `cupertino_host_v1`.
    pub bound: bool,
}

impl HostManager {
    pub fn new(dh: &DisplayHandle) -> Self {
        let host_global =
            dh.create_global::<Cupertino, CupertinoHostV1, ()>(HOST_PROTOCOL_VERSION, ());
        let component_global =
            dh.create_global::<Cupertino, CupertinoComponentV1, ()>(HOST_PROTOCOL_VERSION, ());
        let mut components = HashMap::new();
        for spec in COMPONENTS {
            components.insert(spec.name, ComponentRecord::default());
        }
        Self {
            _host_global: host_global,
            _component_global: component_global,
            next_host_id: 1,
            hosts: HashMap::new(),
            host_by_client: HashMap::new(),
            components,
            persisted: HashMap::new(),
            holds: Vec::new(),
            held_wm_outputs: HashMap::new(),
            shared_host_crashes: Vec::new(),
            all_separate_for_session: false,
            last_input_at: Instant::now(),
            absent: HashSet::new(),
            dev_given_back: HashSet::new(),
            ended: HashMap::new(),
            planned_shared: Vec::new(),
            planned_separate: Vec::new(),
            left_alone: Vec::new(),
            plan_dirty: false,
        }
    }

    /// The host connections, in the order they were made.
    pub fn host_views(&self) -> Vec<HostView> {
        let mut hosts: Vec<&HostEntry> = self.hosts.values().collect();
        hosts.sort_by_key(|h| h.id);
        hosts
            .into_iter()
            .map(|h| HostView {
                kind: h.kind,
                components: h.components.clone(),
                pid: h.process.as_ref().map(|p| p.pid),
                bound: h.resource.is_some(),
            })
            .collect()
    }

    /// Whether `component` has a connection a host was handed for it.
    pub fn component_connected(&self, component: &str) -> bool {
        self.components
            .get(component)
            .is_some_and(|r| r.active.is_some())
    }

    /// The instance number the server last handed out for `component`, 0 before the first.
    pub fn component_instance(&self, component: &str) -> u32 {
        self.components.get(component).map_or(0, |r| r.instance)
    }

    /// Whether a host answered `unavailable` with `absent` for `component`, so it runs as its process entry.
    pub fn component_absent(&self, component: &str) -> bool {
        self.absent.contains(component)
    }

    /// The host `component` runs in by the last plan, if any.
    pub fn planned_host(&self, component: &str) -> Option<HostKind> {
        if let Some(&name) = self.planned_separate.iter().find(|c| **c == component) {
            Some(HostKind::Separate(name))
        } else {
            self.planned_shared
                .contains(&component)
                .then_some(HostKind::Shared)
        }
    }

    /// Record user input so `request_restart` knows when the seat has been quiet for [`RESTART_QUIET_IDLE`].
    pub fn note_input(&mut self, now: Instant) {
        self.last_input_at = now;
    }

    /// Whether `client` belongs to a component currently marked `unresponsive` or `lost`.
    pub fn is_client_unresponsive(&self, client: &Client) -> bool {
        let Some(conn) = client.get_data::<ClientState>().and_then(|s| s.component) else {
            return false;
        };
        self.components.get(conn.name).is_some_and(|rec| {
            rec.active
                .as_ref()
                .is_some_and(|(id, _)| *id == client.id())
                && matches!(rec.liveness, Liveness::Unresponsive | Liveness::Lost)
        })
    }

    /// Whether `component` is currently busy, unresponsive, lost, stopped, or disconnected — in which case a
    /// transaction that expects it does not wait for it.
    pub fn component_not_waited_for(&self, component: &str) -> bool {
        let Some(rec) = self.components.get(component) else {
            return true;
        };
        rec.gave_up
            || rec.active.is_none()
            || matches!(
                rec.liveness,
                Liveness::Busy | Liveness::Unresponsive | Liveness::Lost
            )
    }

    /// Current liveness of `component`.
    pub fn component_liveness(&self, component: &str) -> Liveness {
        self.components
            .get(component)
            .map_or(Liveness::Responsive, |r| r.liveness)
    }

    /// Whether `component` has been stopped by the give-up policy.
    pub fn component_gave_up(&self, component: &str) -> bool {
        self.components.get(component).is_some_and(|r| r.gave_up)
    }

    /// Whether `component` is marked for a separate host for the rest of the session.
    pub fn component_separate_for_session(&self, component: &str) -> bool {
        self.all_separate_for_session
            || self
                .components
                .get(component)
                .is_some_and(|r| r.separate_for_session)
    }

    /// Accepted component name for a session-bus unique name (`org.cupertino.Session1.ComponentOf`).
    pub fn component_of_bus_name(&self, unique_name: &str) -> Option<&'static str> {
        self.components.iter().find_map(|(&name, rec)| {
            (rec.accepted_bus_name.as_deref() == Some(unique_name)).then_some(name)
        })
    }

    /// The session-bus unique name the server accepted for `component`, while it has one (D20).
    pub fn component_bus_name(&self, component: &str) -> Option<&str> {
        self.components
            .get(component)
            .and_then(|r| r.accepted_bus_name.as_deref())
    }

    /// PIDs and start ticks of all currently connected shell host processes (`org.cupertino.Session1.ShellHosts`).
    pub fn shell_host_processes(&self) -> Vec<(u32, u64)> {
        let mut out = Vec::new();
        for host in self.hosts.values() {
            if let (Some(proc), Some(ticks)) = (&host.process, host.start_ticks) {
                if !proc.has_exited() {
                    out.push((proc.pid, ticks));
                }
            }
        }
        out
    }

    /// All currently held persistent surfaces on `output` and `layer`.
    pub fn held_surfaces_on(&self, output: OutputId, layer: Layer) -> Vec<&HeldSurface> {
        self.holds
            .iter()
            .filter(|h| h.output == output && h.layer == layer)
            .collect()
    }

    /// All currently held persistent surfaces (for inspection and tests).
    pub fn held_surfaces(&self) -> &[HeldSurface] {
        &self.holds
    }

    /// Whether `component` shows a persistent surface on `output` (for inspection and tests): one of its connection that
    /// has shown a buffer, which shows live until the connection ends, also once its role object is gone, or the held
    /// picture of one (D19).
    pub fn persistent_shows_on(&self, component: &str, output: OutputId) -> bool {
        self.persisted
            .values()
            .any(|p| p.component == component && p.output == output && p.mapped)
            || self
                .holds
                .iter()
                .any(|h| h.component == component && h.output == output && h.picture.is_some())
    }

    /// Whether window-manager placements are currently held on `output`.
    pub fn wm_placements_held(&self, output: OutputId) -> bool {
        self.held_wm_outputs.contains_key(&output)
    }

    /// The state of the window-manager placements held on `output`, if any.
    pub fn wm_placement_hold(&self, output: OutputId) -> Option<WmPlacementHold> {
        self.held_wm_outputs.get(&output).copied()
    }

    /// Hold the window-manager placements of `output` until a new instance takes them over or the holds end.
    pub(crate) fn hold_wm_placements(&mut self, output: OutputId) {
        self.held_wm_outputs.insert(output, WmPlacementHold::Held);
    }

    /// Move the held placements of `output` to `state`, or end the hold with `None`.
    pub(crate) fn set_wm_placement_hold(
        &mut self,
        output: OutputId,
        state: Option<WmPlacementHold>,
    ) {
        match state {
            Some(s) => self.held_wm_outputs.insert(output, s),
            None => self.held_wm_outputs.remove(&output),
        };
    }

    /// Whether the component connection `client` of `component` has a persistent surface on `output`, or a hold of
    /// `component` lasts there: what a role it loses keeps held with it.
    pub fn component_persists_on(
        &self,
        component: &str,
        client: &ClientId,
        output: OutputId,
    ) -> bool {
        self.persisted
            .values()
            .any(|p| &p.client_id == client && p.output == output)
            || self
                .holds
                .iter()
                .any(|h| h.component == component && h.output == output)
    }

    /// Non-exclusive area left on `output` (`output_rect` in global logical coordinates) after subtracting the
    /// exclusive zones of all held persistent surfaces on that output. Returns `None` when no held surface reserves
    /// space on `output`.
    pub fn held_non_exclusive_zone(&self, output: OutputId, output_rect: Rect) -> Option<Rect> {
        let mut top = 0.0f64;
        let mut bottom = 0.0f64;
        let mut left = 0.0f64;
        let mut right = 0.0f64;
        let mut any = false;
        for hold in self.holds.iter().filter(|h| h.output == output) {
            let ExclusiveZone::Exclusive(px) = hold.exclusive_zone else {
                continue;
            };
            if px == 0 {
                continue;
            }
            let z = f64::from(px);
            let a = hold.anchor;
            // Match wlr-layer-shell exclusive zone edge semantics.
            if a.contains(Anchor::TOP) && !a.contains(Anchor::BOTTOM) {
                top += z + f64::from(hold.margin.top);
                any = true;
            } else if a.contains(Anchor::BOTTOM) && !a.contains(Anchor::TOP) {
                bottom += z + f64::from(hold.margin.bottom);
                any = true;
            } else if a.contains(Anchor::LEFT) && !a.contains(Anchor::RIGHT) {
                left += z + f64::from(hold.margin.left);
                any = true;
            } else if a.contains(Anchor::RIGHT) && !a.contains(Anchor::LEFT) {
                right += z + f64::from(hold.margin.right);
                any = true;
            }
        }
        if !any {
            return None;
        }
        let w = (output_rect.w - left - right).max(0.0);
        let h = (output_rect.h - top - bottom).max(0.0);
        Some(Rect::new(output_rect.x + left, output_rect.y + top, w, h))
    }

    /// Copy the current shm pixel contents of `surface` if accessible: a held surface's picture without a renderer.
    fn capture_shm_picture(surface: &WlSurface) -> Option<HeldContent> {
        let buffer = with_states(surface, |states| {
            let from_renderer = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|d| {
                    d.lock()
                        .ok()
                        .and_then(|s| s.buffer().map(|b| (**b).clone()))
                });
            if from_renderer.is_some() {
                return from_renderer;
            }
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.current();
            match &attrs.buffer {
                Some(smithay::wayland::compositor::BufferAssignment::NewBuffer(buf)) => {
                    Some(buf.clone())
                }
                _ => None,
            }
        })?;
        let captured = with_buffer_contents(&buffer, |ptr, len, data| {
            let w = data.width;
            let h = data.height;
            if w <= 0 || h <= 0 {
                return None;
            }
            let row_bytes = (w as usize).checked_mul(4)?;
            let offset = data.offset as usize;
            let stride = data.stride as usize;
            if stride < row_bytes || offset + stride * (h as usize - 1) + row_bytes > len {
                return None;
            }
            // Safety: Smithay maps the shm pool for the closure duration; bounds checked above.
            let mem = unsafe { std::slice::from_raw_parts(ptr, len) };
            let mut tightly_packed = Vec::with_capacity(row_bytes * h as usize);
            for y in 0..h as usize {
                let start = offset + y * stride;
                tightly_packed.extend_from_slice(&mem[start..start + row_bytes]);
            }
            let fourcc = held_shm_fourcc(data.format)?;
            let mem_buf = MemoryRenderBuffer::from_slice(
                &tightly_packed,
                fourcc,
                (w, h),
                1,
                Transform::Normal,
                None,
            );
            Some(HeldContent::Memory {
                buffer: mem_buf,
                size: (w, h),
            })
        });
        captured.ok().flatten()
    }
}

/// The pixel format of a held picture copied from a `wl_shm` buffer of `format` (D19), its bytes kept as they are:
/// the four-byte formats a renderer imports from memory, so a toolkit's `abgr8888` keeps its colours; `None` for
/// another format, which leaves the hold without a picture rather than one in the wrong colours.
pub fn held_shm_fourcc(
    format: smithay::reexports::wayland_server::protocol::wl_shm::Format,
) -> Option<Fourcc> {
    use smithay::reexports::wayland_server::protocol::wl_shm::Format;
    match format {
        Format::Argb8888 => Some(Fourcc::Argb8888),
        Format::Xrgb8888 => Some(Fourcc::Xrgb8888),
        Format::Abgr8888 => Some(Fourcc::Abgr8888),
        Format::Xbgr8888 => Some(Fourcc::Xbgr8888),
        _ => None,
    }
}

/// Generate a 128-bit cryptographically random nonce formatted as 32 lowercase hexadecimal characters, from the
/// kernel's random source; `None` when that fails, so no guessable nonce is ever handed out.
fn generate_nonce() -> Option<String> {
    let mut bytes = [0u8; 16];
    let mut filled = 0;
    while filled < bytes.len() {
        match rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty()) {
            Ok(n) => filled += n,
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => {
                tracing::warn!(error = %e, "the kernel's random source failed: no bus identity nonce");
                return None;
            }
        }
    }
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    Some(out)
}

/// Create a close-on-exec Unix stream socket pair for the host bus bridge.
fn bridge_socketpair() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let (a, b) = UnixStream::pair()?;
    Ok((OwnedFd::from(a), OwnedFd::from(b)))
}

/// Compute exponential failure backoff (`0.25s`, `0.5s`, ..., capped at `8s`).
fn backoff_for(consecutive_failures: u32) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(5);
    let ms = FAILURE_BACKOFF_BASE.as_millis() * (1u128 << shift);
    Duration::from_millis(ms as u64).min(FAILURE_BACKOFF_CAP)
}

mod dispatch;
mod picture;
mod spawn;

pub use spawn::find_host_program;
